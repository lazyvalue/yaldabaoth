//! Durable per-session write-ahead log (ADR-0009 / spec-event-stream §D4).
//!
//! The session server's `event_log` is already the ordered, append-only source
//! of truth for a session's transcript. This module makes it *durable* so a
//! crash — power loss, OOM, `kill -9`, or a panic — no longer loses every
//! session since the last clean shutdown (the old JSON snapshot was written
//! only on SIGINT/SIGTERM).
//!
//! ## Layout
//!
//! One append-only NDJSON file per session, `<dir>/<server_session_id>.log`.
//! The first line is a [`WalRecord::Header`] (creation-time metadata); later
//! records contain transcript events, last-write-wins metadata changes, and
//! prompt intent/terminal pairs. Recovery replays the file into both the event
//! log and the current session state. The `acp_session_id` needed to `--resume`
//! the agent is re-derived from the last `SessionAttached` event, so the log is
//! self-describing.
//!
//! ## Durability contract (ADR-0009)
//!
//! - Every event is `write()`-n immediately to the OS (no userspace buffering),
//!   so a *process* crash loses nothing — the kernel still flushes its page
//!   cache to disk.
//! - `fsync` (`sync_data`) is issued at **turn boundaries** (`UserPrompt`,
//!   `TurnEnded`) and for lifecycle/metadata/prompt-intent transitions — never
//!   per streamed token. Guarantee: **never lose a completed turn, admitted
//!   prompt, or acknowledged session setting**; the worst case on power loss
//!   is an in-flight stream tail (some `Chunk`s of an unfinished turn)
//!   truncating.
//! - Recovery tolerates a torn final line (a partial write interrupted by power
//!   loss): it is skipped rather than aborting the whole replay. `reopen`
//!   REPAIRS a torn final line (terminating it if parseable, truncating it
//!   away otherwise) before resuming appends, so the next record can never
//!   land directly after the torn bytes and turn a tolerated torn tail into
//!   newline-terminated interior corruption — which `recover_one` treats as
//!   fatal — on the following restart.
//!
//! Log compaction / snapshotting is deferred (ADR-0009) until a long session
//! measurably hurts memory or recovery latency; until then the full log is
//! replayed and `seq`/`turns` stay simple absolute counts.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::acp_channel::{AgentProvider, ImageAttachment, PermissionMode, PromptPayload};
use crate::session_proto::Notification;

/// One line in a session WAL file.
// wire/event enum — boxing the large variant would ripple through serialization + every match site
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum WalRecord {
    /// Always the first record: session metadata not carried in the event
    /// stream. `version` lets the on-disk format evolve (spec §Constraints:
    /// "version from day one").
    Header {
        version: u32,
        server_session_id: String,
        label: String,
        cwd: PathBuf,
        permission_mode: PermissionMode,
        #[serde(default)]
        provider: AgentProvider,
    },
    /// One transcript event, in `event_log` order.
    Event(Notification),
    /// A session rename. Renames are metadata (like the header `label`), NOT
    /// transcript events, so they are persisted as their own record rather than
    /// pushed through the event_log — which keeps them out of the replay stream
    /// and immune to event-log compaction. `recover_one` applies the LAST
    /// rename over the header label.
    ///
    /// Current policy: under the strict reader, an unknown variant on a
    /// newline-terminated line is a FATAL startup error (durable interior
    /// corruption), not a skippable graceful-downgrade path — there is no
    /// "older binaries silently ignore it" escape hatch anymore. Any
    /// additive record variant, this one included, therefore requires a
    /// `WAL_VERSION` bump; an older binary fails closed on a newer-format
    /// record instead of silently ignoring it.
    Rename {
        label: String,
    },
    /// Durable cold-storage lifecycle flag. Last record wins. Separate from
    /// the transcript event stream so archive bookkeeping never paints as an
    /// agent turn and survives event-log compaction.
    Archive {
        archived: bool,
    },
    /// Last permission selection. Metadata records are last-write-wins so a
    /// safety-sensitive Yolo selection cannot silently revert after recovery.
    Permission {
        mode: PermissionMode,
    },
    /// Desired model selection, re-applied to every replacement transport.
    Model {
        model_id: String,
    },
    /// Durable prompt intent. A matching terminal record removes it from the
    /// recovery queue; an unmatched intent is retried after a crash.
    PromptIntent {
        id: u64,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        text_blocks: Vec<String>,
        images: Vec<ImageAttachment>,
    },
    PromptTerminal {
        id: u64,
        outcome: PromptOutcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptOutcome {
    Delivered,
    Rejected,
    Cancelled,
}

/// On-disk WAL format version.
///
/// - 1→2: phase-4 lease migration (`OwnerChanged → LeaseChanged` wire rename).
/// - 2→3: phase-8 Stage A — the `Notification::{ReplyEvent, TurnEnded,
///   UserPrompt}` + `WorkerEvent::Reply` collapse into `Notification::Agent {
///   event: AgentEvent }` (spec-event-stream §1). A v3 log may now interleave the
///   new `agent` records (carrying `turn`/`seq` per-event in the §2 envelope) with
///   the legacy variants kept for the additive rollout (spec §9).
///
/// Versions 1 through 3 remain readable. Both migrations were wire-level
/// changes to records that either were never WAL-appended (v1→v2) or remain as
/// legacy `Notification` variants (v2→v3), so dropping an older file would
/// destroy recoverable data for no technical reason. Unknown future versions
/// fail closed while leaving the original file untouched.
const WAL_VERSION: u32 = 3;
const MIN_READABLE_WAL_VERSION: u32 = 1;

/// A live write handle to one session's WAL file. The session server's
/// `ManagedSession` owns exactly one of these and is its only writer.
pub struct SessionWal {
    file: File,
    path: PathBuf,
}

fn lock_writer(file: &File, path: &Path) -> std::io::Result<()> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let source = std::io::Error::last_os_error();
    // Preserve whatever `flock` actually failed with (e.g. ENOLCK, EBADF)
    // instead of flattening every failure to WouldBlock; the real-contention
    // case (EWOULDBLOCK/EAGAIN) still maps to `ErrorKind::WouldBlock` via the
    // platform's standard errno→kind mapping, so existing callers are unaffected.
    let kind = source.kind();
    Err(std::io::Error::new(
        kind,
        format!(
            "WAL {} already has a live writer; refusing concurrent access: {source}",
            path.display()
        ),
    ))
}

/// Repair a torn (unterminated) final line left by a crash mid-append, so a
/// subsequent `append` cannot land directly after it and glue the next
/// record onto the torn bytes — which would turn a tolerated torn tail into
/// newline-terminated interior corruption (fatal on the next `recover_one`).
///
/// Only ever touches bytes after the LAST `\n` in the file — exactly the
/// bytes `recover_one` already either accepts (a parseable unterminated
/// final record) or discards (an unparseable one), so this repairs the file
/// to match what recovery already keeps rather than losing anything new.
fn repair_torn_tail(file: &mut File, path: &Path) -> std::io::Result<()> {
    let contents = std::fs::read(path)?;
    if contents.is_empty() || contents.last() == Some(&b'\n') {
        return Ok(()); // nothing torn: empty file, or already newline-terminated
    }
    let tail_start = contents
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |newline_pos| newline_pos + 1);
    let tail = &contents[tail_start..];
    if serde_json::from_slice::<WalRecord>(tail).is_ok() {
        // The tail is a complete, parseable record simply missing its
        // trailing newline (the crash landed between `write_all` and the
        // NEXT append's write). `recover_one` already accepts this shape as
        // the final line, so terminating it changes nothing recovery keeps.
        file.write_all(b"\n")?;
    } else {
        // The tail is genuinely torn mid-record. The durability contract
        // already declares these bytes lost (recovery stops here and drops
        // them), so truncating is not new data loss — it just prevents the
        // next append from being silently concatenated onto them.
        file.set_len(tail_start as u64)?;
    }
    file.sync_data()
}

impl SessionWal {
    /// Create a new WAL for a freshly-created session: open the file and write
    /// (and fsync) the header so even a crash immediately after `create` can
    /// recover the session's identity.
    pub fn create(
        dir: &Path,
        server_session_id: &str,
        label: &str,
        cwd: &Path,
        permission_mode: PermissionMode,
    ) -> std::io::Result<SessionWal> {
        Self::create_for_provider(
            dir,
            server_session_id,
            label,
            cwd,
            permission_mode,
            AgentProvider::Claude,
        )
    }

    pub fn create_for_provider(
        dir: &Path,
        server_session_id: &str,
        label: &str,
        cwd: &Path,
        permission_mode: PermissionMode,
        provider: AgentProvider,
    ) -> std::io::Result<SessionWal> {
        std::fs::create_dir_all(dir)?;
        let path = wal_path(dir, server_session_id);
        // A session id collision must never overwrite durable history. UUID
        // reuse is extraordinarily unlikely, but create-new makes the WAL's
        // sacredness structural rather than probabilistic.
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        lock_writer(&file, &path)?;
        let mut wal = SessionWal { file, path };
        let header = WalRecord::Header {
            version: WAL_VERSION,
            server_session_id: server_session_id.to_string(),
            label: label.to_string(),
            cwd: cwd.to_path_buf(),
            permission_mode,
            provider,
        };
        wal.write_record(&header)?;
        wal.file.sync_data()?;
        Ok(wal)
    }

    /// Re-open an existing WAL in append mode after recovery, so the restored
    /// session keeps logging to the same file.
    ///
    /// Before returning the handle, repairs a torn (unterminated) final line
    /// left by a crash mid-append: `recover_one` tolerates such a line by
    /// keeping the intact prefix, but a plain append would land the next
    /// record directly after the torn bytes, producing ONE newline-terminated
    /// malformed line — which the next recovery treats as fatal durable
    /// interior corruption instead of a tolerable torn tail. See
    /// `repair_torn_tail`.
    pub fn reopen(path: PathBuf) -> std::io::Result<SessionWal> {
        // `.read(true)` so the repair scan below can read back what's on disk
        // through the same handle's underlying file; `.append(true)` keeps
        // every subsequent write landing at the true end regardless of the
        // read position (O_APPEND).
        let mut file = OpenOptions::new().read(true).append(true).open(&path)?;
        lock_writer(&file, &path)?;
        repair_torn_tail(&mut file, &path)?;
        Ok(SessionWal { file, path })
    }

    /// Append one event. `fsync` true → `sync_data` after the write (turn
    /// boundaries); false → write only (the OS page cache survives a process
    /// crash, per the durability contract). Errors are returned for the caller
    /// to log; a WAL write failure must not take down the session.
    pub fn append(&mut self, note: &Notification, fsync: bool) -> std::io::Result<()> {
        self.write_record(&WalRecord::Event(note.clone()))?;
        if fsync {
            self.file.sync_data()?;
        }
        Ok(())
    }

    /// Persist a session rename. Always fsynced: a rename is a small, rare,
    /// user-visible metadata change, and losing it on a crash is exactly the
    /// bug this record exists to prevent. Recovery applies the last such record
    /// over the header label.
    pub fn append_rename(&mut self, label: &str) -> std::io::Result<()> {
        self.write_record(&WalRecord::Rename {
            label: label.to_string(),
        })?;
        self.file.sync_data()
    }

    /// Persist a lifecycle transition before the caller drops (archive) or
    /// retains (unarchive) this handle. Always fsynced: recovery must never
    /// accidentally respawn a session the user archived.
    pub fn append_archived(&mut self, archived: bool) -> std::io::Result<()> {
        self.write_record(&WalRecord::Archive { archived })?;
        self.file.sync_data()
    }

    pub fn append_permission_mode(&mut self, mode: PermissionMode) -> std::io::Result<()> {
        self.write_record(&WalRecord::Permission { mode })?;
        self.file.sync_data()
    }

    pub fn append_model(&mut self, model_id: &str) -> std::io::Result<()> {
        self.write_record(&WalRecord::Model {
            model_id: model_id.to_string(),
        })?;
        self.file.sync_data()
    }

    pub fn append_prompt_intent(
        &mut self,
        id: u64,
        payload: &PromptPayload,
    ) -> std::io::Result<()> {
        self.write_record(&WalRecord::PromptIntent {
            id,
            text: payload.text.clone(),
            text_blocks: payload.text_blocks.clone(),
            images: payload.images.clone(),
        })?;
        self.file.sync_data()
    }

    pub fn append_prompt_terminal(
        &mut self,
        id: u64,
        outcome: PromptOutcome,
    ) -> std::io::Result<()> {
        self.write_record(&WalRecord::PromptTerminal { id, outcome })?;
        self.file.sync_data()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Delete the WAL file — the session was explicitly closed, so its
    /// transcript should not be recovered on the next start.
    pub fn remove(self) -> std::io::Result<()> {
        std::fs::remove_file(&self.path)
    }

    fn write_record(&mut self, rec: &WalRecord) -> std::io::Result<()> {
        // One JSON object per line. Serialize fully first so a serialization
        // error never writes a half line; then a single `write_all` hands the
        // bytes to the OS in one syscall.
        let mut line = serde_json::to_string(rec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        self.file.write_all(line.as_bytes())
    }
}

/// A session reconstructed from its WAL, ready for the server to re-insert and
/// re-spawn its agent.
#[derive(Debug, Clone)]
pub struct RecoveredSession {
    pub path: PathBuf,
    pub server_session_id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub permission_mode: PermissionMode,
    /// Last durable model selection, if the user chose one.
    pub model_id: Option<String>,
    pub provider: AgentProvider,
    /// Number of transcript events on disk — the length of the session's
    /// durable prefix, logical event-log positions `[0, durable_events)`. The
    /// events themselves stay on disk; see [`stream_durable_prefix`].
    pub durable_events: u64,
    /// The channel generation the first post-recovery channel must use:
    /// strictly newer than every durable `Agent` event (`max + 1`), or 0 for a
    /// log with none. Kept at the recovery boundary so seed state, watch
    /// channels, and the resume worker cannot choose different generations.
    pub next_generation: u64,
    /// Re-derived from the last `SessionAttached` event — the id needed to
    /// `--resume` the agent. `None` if the agent never finished its handshake
    /// before the crash (nothing to resume).
    pub acp_session_id: Option<String>,
    /// Completed-turn count, from `TurnEnded` events — the `replay_fence`.
    pub turns: usize,
    /// Last durable archive marker; false for pre-archive WALs.
    pub archived: bool,
    /// Intents without a terminal record, in original submission order.
    pub pending_prompts: Vec<RecoveredPrompt>,
    /// Next durable prompt identity (strictly greater than every seen id).
    pub next_prompt_id: u64,
}

#[derive(Debug, Clone)]
pub struct RecoveredPrompt {
    pub id: u64,
    pub payload: PromptPayload,
}

/// Recover every session WAL in `dir`. Missing dir → empty (first run). A file
/// that can't be opened or whose header is unreadable is skipped with a log
/// line rather than aborting the whole recovery.
pub fn recover_all(dir: &Path) -> Vec<RecoveredSession> {
    let mut out = Vec::new();
    recover_each(dir, |session| out.push(session));
    out
}

/// Recover session WALs one at a time, releasing each replay buffer before the
/// next file is opened when the caller does not retain it.  Startup uses this
/// form so a directory of long transcripts does not require all replay images
/// to coexist in memory before per-session compaction.
pub fn recover_each(dir: &Path, mut visit: impl FnMut(RecoveredSession)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // no dir yet
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        match recover_one(&path) {
            Ok(Some(s)) => visit(s),
            Ok(None) => {
                eprintln!(
                    "[session-wal] skipping {}: empty or headerless",
                    path.display()
                );
            }
            Err(e) => {
                eprintln!("[session-wal] skipping {}: {e}", path.display());
            }
        }
    }
}

/// Strict production recovery. Unlike [`recover_each`], this never silently
/// turns a WAL problem into a MISSING session, and never modifies a file.
///
/// What fails the WHOLE startup: an I/O error reading the directory or a
/// file, and any `visit` error (e.g. the writer reopen finding another
/// process holding the WAL). Those are environment problems a supervisor
/// restart can plausibly clear, and a server must not advertise a roster it
/// could not actually read.
///
/// What is skipped PER FILE, with a loud `[session-wal]` warning on stderr
/// (→ `session-server.log`), the bytes left untouched for surgery:
///
/// - a headerless file (`recover_one` → `Ok(None)`): a create-crash artifact
///   (zero bytes, or torn on its very first line) with nothing to lose;
/// - a file `recover_one` refuses as `InvalidData` — durable interior
///   corruption (a newline-terminated malformed record) or an unsupported
///   header version. bug-0064, 2026-09-09: the Sep 1–2 twin-writer
///   split-brain left one orphan tail fragment mid-file in two week-old WALs;
///   the first restart since then crash-looped the systemd service and took
///   all 55 sessions offline. A restart loop can never repair content, so
///   failing the roster over it buys no safety — it converts one session's
///   visibility problem into everyone's availability loss. `recover_one`
///   itself stays strict per record (the hole is never silent): the warning
///   names the file and the exact line, and the file is retained.
pub fn try_recover_each(
    dir: &Path,
    mut visit: impl FnMut(RecoveredSession) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("log") {
            continue;
        }
        match recover_one(&path) {
            Ok(Some(session)) => visit(session)?,
            Ok(None) => {
                eprintln!(
                    "[session-wal] skipping {}: empty or headerless (create-crash artifact, \
                     no recoverable data)",
                    path.display()
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                eprintln!(
                    "[session-wal] SKIPPING CORRUPT WAL {}: {error} — this session is NOT \
                     recovered; the file is retained untouched for manual repair (bug-0064)",
                    path.display()
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// The non-transcript state of one WAL file, folded record by record by
/// [`walk_wal`]. Everything here is metadata: a handful of small values per
/// file, whatever the transcript's size.
struct WalMeta {
    /// `(server_session_id, label, cwd, permission_mode, provider)` from the
    /// header record.
    header: (String, String, PathBuf, PermissionMode, AgentProvider),
    /// The most recent rename, if any. Applied over the header label so a
    /// session recovered after a server restart keeps its renamed name rather
    /// than reverting to the creation-time label.
    renamed_label: Option<String>,
    archived: bool,
    permission_override: Option<PermissionMode>,
    model_id: Option<String>,
    prompt_intents: Vec<RecoveredPrompt>,
    prompt_terminal: std::collections::HashSet<u64>,
    max_prompt_id: Option<u64>,
}

/// Walk one WAL file record by record without retaining the transcript.
///
/// Metadata records fold into the returned [`WalMeta`]; every transcript
/// `Event` record is handed to `on_event` in file order, which returns
/// `Ok(false)` to stop the walk early. Returns `Ok(None)` if the file has no
/// valid header. A torn/partial final line (interrupted write on power loss)
/// is skipped — that is the bounded data loss the contract permits — while a
/// newline-terminated malformed record is durable interior corruption and
/// fails with `InvalidData`.
///
/// This is the ONE reader over the on-disk format: [`recover_one`] folds a
/// summary through it at startup, and [`stream_durable_prefix`] streams the
/// transcript through it at attach time. Neither ever holds more than one
/// record in memory — the whole point (a 1.1 GB WAL directory used to cost
/// 7.2 GB of RSS at boot when every event was decoded into a `Vec`).
fn walk_wal(
    path: &Path,
    mut on_event: impl FnMut(Notification) -> std::io::Result<bool>,
) -> std::io::Result<Option<WalMeta>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    let mut header: Option<(String, String, PathBuf, PermissionMode, AgentProvider)> = None;
    let mut renamed_label: Option<String> = None;
    let mut archived = false;
    let mut permission_override: Option<PermissionMode> = None;
    let mut model_id: Option<String> = None;
    let mut prompt_intents: Vec<RecoveredPrompt> = Vec::new();
    let mut prompt_terminal = std::collections::HashSet::new();
    let mut max_prompt_id: Option<u64> = None;

    let mut line_number = 0usize;
    loop {
        let mut bytes = Vec::new();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        line_number += 1;
        let terminated = bytes.last() == Some(&b'\n');
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let rec: WalRecord = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            // Only an unterminated malformed final record is a tolerable torn
            // write. A newline-terminated malformed record is durable interior
            // corruption and must fail visibly instead of silently punching a
            // hole in the transcript.
            Err(_) if !terminated => break,
            Err(error) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("malformed WAL record at line {line_number}: {error}"),
                ));
            }
        };
        match rec {
            WalRecord::Header {
                version,
                server_session_id,
                label,
                cwd,
                permission_mode,
                provider,
            } => {
                if !(MIN_READABLE_WAL_VERSION..=WAL_VERSION).contains(&version) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "unsupported WAL version {version} at line {line_number}; \
                             supported versions are {MIN_READABLE_WAL_VERSION}..={WAL_VERSION}"
                        ),
                    ));
                }
                header = Some((server_session_id, label, cwd, permission_mode, provider));
            }
            WalRecord::Event(note) => {
                if !on_event(note)? {
                    break;
                }
            }
            WalRecord::Rename { label } => renamed_label = Some(label),
            WalRecord::Archive { archived: value } => archived = value,
            WalRecord::Permission { mode } => permission_override = Some(mode),
            WalRecord::Model { model_id: value } => model_id = Some(value),
            WalRecord::PromptIntent {
                id,
                text,
                text_blocks,
                images,
            } => {
                max_prompt_id = Some(max_prompt_id.map_or(id, |seen| seen.max(id)));
                prompt_intents.push(RecoveredPrompt {
                    id,
                    payload: PromptPayload {
                        text,
                        text_blocks,
                        images,
                    },
                });
            }
            WalRecord::PromptTerminal { id, .. } => {
                max_prompt_id = Some(max_prompt_id.map_or(id, |seen| seen.max(id)));
                prompt_terminal.insert(id);
            }
        }
    }

    let Some(header) = header else {
        return Ok(None);
    };
    Ok(Some(WalMeta {
        header,
        renamed_label,
        archived,
        permission_override,
        model_id,
        prompt_intents,
        prompt_terminal,
        max_prompt_id,
    }))
}

/// Recover a single WAL file's session SUMMARY: identity, settings, prompt
/// state, the durable transcript length, and the stream position the next
/// channel must resume from. The transcript itself is NOT loaded — it stays on
/// disk as the session's durable prefix and is streamed to a client on attach
/// by [`stream_durable_prefix`]. Memory is O(1) per file regardless of size.
///
/// Returns `Ok(None)` for a headerless file; see [`walk_wal`] for the torn-
/// tail / interior-corruption semantics.
pub fn recover_one(path: &Path) -> std::io::Result<Option<RecoveredSession>> {
    use crate::agent_event::{AgentEventKind, TurnOutcome};

    let mut durable_events = 0u64;
    // Re-derived from the LAST `SessionAttached` carrying an id. KEPT as a
    // control variant in the collapse (spec §1) precisely so this recovery
    // dependency survives — folding it into ChannelOpened would lose the id.
    let mut acp_session_id: Option<String> = None;
    // Completed-turn count = the durable tip (spec §5). During the additive
    // rollout (spec §9) a log may carry BOTH legacy `TurnEnded` records AND the
    // new `Agent { TurnEnded }` records describing the SAME boundaries, so we
    // take the max of the two interpretations rather than summing (which would
    // double-count). Legacy: count of `TurnEnded`. Agent: `max(turn)+1` over
    // `Agent` events whose kind is a real `TurnEnded` (excluding `ReplayEnd`,
    // which marks the end of a replayed prefix, not a completed live turn).
    let mut legacy_turns = 0usize;
    let mut agent_max_turn: Option<u64> = None;
    // Highest `Agent` channel generation on disk: the next channel is strictly
    // newer, so a recovered server never re-uses a generation whose seqs
    // already exist durably.
    let mut max_generation: Option<u64> = None;

    let Some(meta) = walk_wal(path, |note| {
        durable_events += 1;
        match &note {
            Notification::SessionAttached {
                acp_session_id: Some(id),
                ..
            } => acp_session_id = Some(id.clone()),
            Notification::TurnEnded { .. } => legacy_turns += 1,
            Notification::Agent { event } => {
                max_generation =
                    Some(max_generation.map_or(event.generation, |g| g.max(event.generation)));
                if let AgentEventKind::TurnEnded { outcome } = &event.kind
                    && !matches!(outcome, TurnOutcome::ReplayEnd)
                {
                    agent_max_turn = Some(agent_max_turn.map_or(event.turn, |t| t.max(event.turn)));
                }
            }
            _ => {}
        }
        Ok(true)
    })?
    else {
        return Ok(None);
    };

    let (server_session_id, header_label, cwd, header_permission_mode, provider) = meta.header;
    // Last rename wins over the creation-time header label.
    let label = meta.renamed_label.unwrap_or(header_label);
    let permission_mode = meta.permission_override.unwrap_or(header_permission_mode);
    let pending_prompts = meta
        .prompt_intents
        .into_iter()
        .filter(|intent| !meta.prompt_terminal.contains(&intent.id))
        .collect();
    let next_prompt_id = meta.max_prompt_id.map_or(1, |id| id.saturating_add(1));

    // `turn` is 0-based in the envelope; a completed turn `k` means `k+1`
    // turns have settled. `max(turn)+1` is the count.
    let agent_turns = agent_max_turn.map_or(0, |max_turn| (max_turn + 1) as usize);
    let turns = legacy_turns.max(agent_turns);
    // `AgentEvent.seq` is per generation. A recovered server channel is a new
    // generation, so its first `ChannelOpened` starts at seq 0 rather than
    // continuing the maximum seq of an older generation.
    let next_generation = max_generation.map_or(0, |g| {
        g.checked_add(1)
            .expect("durable channel generation exhausted")
    });

    Ok(Some(RecoveredSession {
        path: path.to_path_buf(),
        server_session_id,
        label,
        cwd,
        permission_mode,
        model_id: meta.model_id,
        provider,
        durable_events,
        next_generation,
        acp_session_id,
        turns,
        archived: meta.archived,
        pending_prompts,
        next_prompt_id,
    }))
}

/// Stream the first `len` transcript events of a WAL file — the session's
/// durable prefix, positions `[0, len)` of its logical event log — to `sink`
/// in order, one record in memory at a time. Used by the forwarder to serve a
/// from-base attach straight from disk instead of from a resident copy.
///
/// `len` is the `durable_events` recorded at recovery (or folded at archive);
/// the appender only ever writes PAST it, so reading `[0, len)` through a
/// separate handle is race-free against a live writer (the writer's advisory
/// flock is untouched). Returns the number streamed. A file that yields fewer
/// than `len` events (truncated, or a WAL-degraded session whose resident
/// events never reached disk) streams what exists and then fails with
/// `InvalidData` naming the shortfall, so the caller can be loud rather than
/// silently gap the client. `len == 0` reads nothing.
pub fn stream_durable_prefix(
    path: &Path,
    len: u64,
    mut sink: impl FnMut(Notification) -> std::io::Result<()>,
) -> std::io::Result<u64> {
    if len == 0 {
        return Ok(0);
    }
    let mut streamed = 0u64;
    walk_wal(path, |note| {
        sink(note)?;
        streamed += 1;
        Ok(streamed < len)
    })?;
    if streamed < len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "WAL {} holds {streamed} transcript event(s) but its durable prefix expects {len}",
                path.display()
            ),
        ));
    }
    Ok(streamed)
}

/// Every transcript event of a WAL file, decoded into memory. Test/diagnostic
/// helper only — production never materialises a whole transcript (that is
/// exactly what this module's streaming design exists to avoid).
#[cfg(test)]
pub(crate) fn read_all_events(path: &Path) -> std::io::Result<Vec<Notification>> {
    let mut out = Vec::new();
    walk_wal(path, |note| {
        out.push(note);
        Ok(true)
    })?;
    Ok(out)
}

fn wal_path(dir: &Path, server_session_id: &str) -> PathBuf {
    dir.join(format!("{server_session_id}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "yalda-wal-test-{}-{}-{tag}",
            std::process::id(),
            // a per-call counter avoids collisions without needing a clock
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn chunk(text: &str) -> Notification {
        Notification::ReplyEvent {
            session_id: "s1".into(),
            event: crate::acp_channel::ReplyEvent::Chunk(text.into()),
        }
    }
    fn turn_ended(n: usize) -> Notification {
        Notification::TurnEnded {
            session_id: "s1".into(),
            turn_count: n,
            generation: 0,
        }
    }
    fn attached(acp: &str) -> Notification {
        Notification::SessionAttached {
            session_id: "s1".into(),
            acp_session_id: Some(acp.into()),
        }
    }

    #[test]
    fn create_append_recover_roundtrip() {
        let dir = tmp_dir("roundtrip");
        {
            let mut wal = SessionWal::create(
                &dir,
                "s1",
                "my label",
                Path::new("/tmp/work"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&attached("acp-123"), false).unwrap();
            wal.append(&chunk("hello "), false).unwrap();
            wal.append(&chunk("world"), false).unwrap();
            wal.append(&turn_ended(1), true).unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1);
        let s = &recovered[0];
        assert_eq!(s.server_session_id, "s1");
        assert_eq!(s.label, "my label");
        assert_eq!(s.cwd, Path::new("/tmp/work"));
        assert_eq!(s.acp_session_id.as_deref(), Some("acp-123"));
        assert_eq!(s.turns, 1);
        // header is not an event; 4 events were appended.
        assert_eq!(s.durable_events, 4);
        assert_eq!(s.provider, AgentProvider::Claude);
    }

    #[test]
    fn codex_provider_survives_recovery() {
        let dir = tmp_dir("codex-provider");
        SessionWal::create_for_provider(
            &dir,
            "codex-s1",
            "codex-1",
            Path::new("/tmp/work"),
            PermissionMode::Yolo,
            AgentProvider::Codex,
        )
        .unwrap();

        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].provider, AgentProvider::Codex);
    }

    #[test]
    fn rename_survives_recovery_overriding_header_label() {
        // The "names keep getting forgotten" regression: a session renamed
        // after creation must recover under its NEW name, not the header's
        // creation-time label.
        let dir = tmp_dir("rename");
        {
            let mut wal = SessionWal::create(
                &dir,
                "s1",
                "claude-1",
                Path::new("/tmp/work"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&attached("acp-123"), false).unwrap();
            wal.append_rename("first-rename").unwrap();
            wal.append(&chunk("hi"), false).unwrap();
            // Last rename wins.
            wal.append_rename("final-name").unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1);
        let s = &recovered[0];
        assert_eq!(s.label, "final-name");
        // Rename records are metadata, not transcript events: the event_log
        // holds only the two real events (attached + chunk).
        assert_eq!(s.durable_events, 2);
    }

    #[test]
    fn header_label_kept_when_no_rename() {
        // Guard the non-renamed path: absent any Rename record, the header
        // label is used unchanged.
        let dir = tmp_dir("no-rename");
        {
            let mut wal = SessionWal::create(
                &dir,
                "s9",
                "keep-me",
                Path::new("/tmp"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&chunk("x"), false).unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered[0].label, "keep-me");
    }

    #[test]
    fn torn_final_line_is_skipped_not_fatal() {
        // Simulate a crash mid-write: a valid log followed by a partial JSON
        // line with no newline. Recovery must keep the intact prefix.
        let dir = tmp_dir("torn");
        {
            let mut wal =
                SessionWal::create(&dir, "s2", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&attached("acp-x"), false).unwrap();
            wal.append(&chunk("good"), true).unwrap();
        }
        // Append a torn record by hand (no trailing newline, truncated JSON).
        let path = wal_path(&dir, "s2");
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"t\":\"event\",\"Event\":{\"type\":\"reply_ev")
            .unwrap();
        drop(f);

        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1, "torn line must not lose the session");
        let s = &recovered[0];
        assert_eq!(s.acp_session_id.as_deref(), Some("acp-x"));
        // The two good events survive; the torn one is dropped.
        assert_eq!(s.durable_events, 2);
    }

    #[test]
    fn recovery_is_byte_preserving_and_repeatable() {
        let dir = tmp_dir("read-only-repeatable");
        let path = {
            let mut wal = SessionWal::create(
                &dir,
                "s-repeat",
                "l",
                Path::new("/tmp"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&attached("acp-repeat"), false).unwrap();
            wal.append(&chunk("one"), false).unwrap();
            wal.append(&chunk("two"), true).unwrap();
            wal.path().to_path_buf()
        };
        let before = std::fs::read(&path).unwrap();
        let first = recover_one(&path).unwrap().unwrap();
        let second = recover_one(&path).unwrap().unwrap();
        assert_eq!(first.durable_events, 3);
        assert_eq!(second.durable_events, first.durable_events);
        assert_eq!(second.server_session_id, first.server_session_id);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "recovery must never rewrite, migrate in place, or truncate the WAL"
        );
    }

    #[test]
    fn create_collision_refuses_without_changing_existing_wal() {
        let dir = tmp_dir("collision");
        let path = {
            let mut wal = SessionWal::create(
                &dir,
                "same-id",
                "first",
                Path::new("/tmp"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&chunk("irreplaceable"), true).unwrap();
            wal.path().to_path_buf()
        };
        let before = std::fs::read(&path).unwrap();
        let error = match SessionWal::create(
            &dir,
            "same-id",
            "replacement",
            Path::new("/tmp"),
            PermissionMode::ReadOnly,
        ) {
            Ok(_) => panic!("an existing WAL identity must make fresh creation fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let recovered = recover_one(&path).unwrap().unwrap();
        assert_eq!(recovered.label, "first");
        assert_eq!(recovered.durable_events, 1);
    }

    #[test]
    fn second_live_writer_is_refused_without_changing_wal() {
        let dir = tmp_dir("exclusive-writer");
        let mut owner = SessionWal::create(
            &dir,
            "owned",
            "owner",
            Path::new("/tmp"),
            PermissionMode::Yolo,
        )
        .unwrap();
        owner.append(&chunk("before"), true).unwrap();
        let path = owner.path().to_path_buf();
        let before = std::fs::read(&path).unwrap();

        let error = match SessionWal::reopen(path.clone()) {
            Ok(_) => panic!("a second live WAL writer must be refused"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::read(&path).unwrap(), before);

        drop(owner);
        SessionWal::reopen(path).expect("the lease must release when its owner drops");
    }

    #[test]
    fn malformed_interior_record_fails_visibly_without_mutating_wal() {
        let dir = tmp_dir("interior-corruption");
        std::fs::create_dir_all(&dir).unwrap();
        let path = wal_path(&dir, "corrupt");
        let bytes = concat!(
            r#"{"t":"header","version":3,"server_session_id":"corrupt","label":"l","cwd":"/tmp","permission_mode":"Yolo"}"#,
            "\n",
            "{definitely not json}\n",
            r#"{"t":"event","type":"user_prompt","session_id":"corrupt","text":"must not skip over corruption"}"#,
            "\n",
        )
        .as_bytes()
        .to_vec();
        std::fs::write(&path, &bytes).unwrap();

        let error = recover_one(&path).expect_err("interior corruption must not be skipped");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("line 2"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn future_version_fails_closed_without_mutating_wal() {
        let dir = tmp_dir("future-version");
        std::fs::create_dir_all(&dir).unwrap();
        let path = wal_path(&dir, "future");
        let bytes = format!(
            "{{\"t\":\"header\",\"version\":{},\"server_session_id\":\"future\",\"label\":\"l\",\"cwd\":\"/tmp\",\"permission_mode\":\"Yolo\"}}\n",
            WAL_VERSION + 1
        )
        .into_bytes();
        std::fs::write(&path, &bytes).unwrap();

        let error = recover_one(&path).expect_err("future WAL must fail visibly");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unsupported WAL version"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn strict_recovery_skips_interior_corrupt_file_and_recovers_the_rest() {
        // bug-0064 (2026-09-09): the production shape left by the Sep 1–2
        // twin-writer split-brain — a complete record, then ONE
        // newline-terminated orphan tail fragment (the head of that record
        // was clobbered by the concurrent appender), then more complete
        // records. `recover_one` must still refuse the file (durable interior
        // corruption is never silently skipped per record), but ONE such file
        // must not take every other session offline: a restart loop can never
        // repair content, so failing the whole roster buys no safety.
        let dir = tmp_dir("interior-corrupt-skip");
        std::fs::create_dir_all(&dir).unwrap();
        let corrupt_path = wal_path(&dir, "00-corrupt");
        SessionWal::create(
            &dir,
            "00-corrupt",
            "twin-writer victim",
            Path::new("/tmp"),
            PermissionMode::Yolo,
        )
        .unwrap();
        let good = serde_json::to_string(&WalRecord::Event(chunk("complete record"))).unwrap();
        {
            let mut f = OpenOptions::new().append(true).open(&corrupt_path).unwrap();
            f.write_all(good.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
            // Verbatim from a1204e30 line 11811: the surviving tail of a
            // ToolCallUpdated record, newline-terminated.
            f.write_all(b"pected\":false},\"toolName\":\"Bash\"}}}}}\n")
                .unwrap();
            f.write_all(good.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
        let corrupt = std::fs::read(&corrupt_path).unwrap();
        SessionWal::create(
            &dir,
            "99-valid",
            "must stay reachable",
            Path::new("/tmp"),
            PermissionMode::Yolo,
        )
        .unwrap();

        // Per-file strictness is unchanged: the corrupt file itself is refused.
        let error = recover_one(&corrupt_path).expect_err("interior corruption must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("malformed WAL record at line 3"),
            "unexpected error: {error}"
        );

        let mut visited = Vec::new();
        try_recover_each(&dir, |session| {
            visited.push(session.server_session_id);
            Ok(())
        })
        .expect("one interior-corrupt WAL must not take the whole roster offline");
        assert_eq!(
            visited,
            vec!["99-valid".to_string()],
            "every other session is recovered; the corrupt one is skipped"
        );
        // The damaged file is retained byte-for-byte for manual surgery.
        assert_eq!(std::fs::read(&corrupt_path).unwrap(), corrupt);
    }

    #[test]
    fn strict_recovery_skips_headerless_create_crash_artifacts_instead_of_failing_the_roster() {
        // A zero-byte file is exactly what a crash between `create_new` and
        // the header write leaves behind: no header, no data, nothing
        // recoverable. Unlike durable interior corruption (a newline-
        // terminated malformed line), there is nothing to lose by skipping
        // it — failing the WHOLE roster over it is pure availability loss.
        let dir = tmp_dir("headerless-skip");
        std::fs::create_dir_all(&dir).unwrap();
        let empty_path = wal_path(&dir, "00-empty-create-crash");
        std::fs::write(&empty_path, b"").unwrap();
        SessionWal::create(
            &dir,
            "99-valid",
            "the only real session",
            Path::new("/tmp"),
            PermissionMode::Yolo,
        )
        .unwrap();

        let mut visited = Vec::new();
        try_recover_each(&dir, |session| {
            visited.push(session.server_session_id);
            Ok(())
        })
        .expect("a headerless create-crash artifact must not fail the whole roster");
        assert_eq!(
            visited,
            vec!["99-valid".to_string()],
            "exactly the valid session must be visited"
        );
        // The artifact itself is left completely untouched.
        assert_eq!(std::fs::read(&empty_path).unwrap(), b"");
    }

    #[test]
    fn strict_recovery_skips_torn_unterminated_header_only_artifact() {
        // A crash mid-write on the very first line: torn, unterminated, and
        // unparseable as a header. `recover_one` returns `Ok(None)` for this
        // (same as empty) — no header ever landed, so nothing is recoverable.
        let dir = tmp_dir("headerless-torn-skip");
        std::fs::create_dir_all(&dir).unwrap();
        let torn_path = wal_path(&dir, "00-torn-header-only");
        let torn_bytes = b"{\"t\":\"header\",\"version\":3,\"server_sess".to_vec();
        std::fs::write(&torn_path, &torn_bytes).unwrap();
        SessionWal::create(
            &dir,
            "99-valid",
            "the only real session",
            Path::new("/tmp"),
            PermissionMode::Yolo,
        )
        .unwrap();

        let mut visited = Vec::new();
        try_recover_each(&dir, |session| {
            visited.push(session.server_session_id);
            Ok(())
        })
        .expect("a torn header-only artifact must not fail the whole roster");
        assert_eq!(visited, vec!["99-valid".to_string()]);
        assert_eq!(std::fs::read(&torn_path).unwrap(), torn_bytes);
    }

    #[test]
    fn reopen_appends_to_existing() {
        let dir = tmp_dir("reopen");
        let path = {
            let mut wal =
                SessionWal::create(&dir, "s3", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&chunk("a"), true).unwrap();
            wal.path.clone()
        };
        {
            let mut wal = SessionWal::reopen(path).unwrap();
            wal.append(&chunk("b"), true).unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered[0].durable_events, 2);
    }

    #[test]
    fn reopen_truncates_unparseable_torn_tail_so_later_recovery_survives() {
        // The two-restart poison sequence this guards: (1) a crash mid-append
        // leaves an unterminated, unparseable torn final line — tolerated by
        // `recover_one` (prefix kept). (2) The server restarts, recovers the
        // session, and `reopen`s the WAL. A plain append-mode open would land
        // the NEXT record directly after the torn bytes, producing one
        // newline-terminated MALFORMED line. (3) The next restart's
        // `recover_one` would then hard-error ("malformed WAL record"),
        // which `try_recover_each` turns into a fatal, whole-server-refuses-
        // to-start error. `reopen`'s repair must truncate the unparseable
        // torn tail so step (3) never happens.
        let dir = tmp_dir("torn-tail-unparseable");
        let path = {
            let mut wal =
                SessionWal::create(&dir, "s5", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&chunk("good-one"), true).unwrap();
            wal.path().to_path_buf()
        };
        // Hand-append a torn, unparseable record (no trailing newline).
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"{\"t\":\"event\",\"Event\":{\"type\":\"reply_ev")
                .unwrap();
        }
        // First restart's recovery: tolerated, prefix kept.
        let first = recover_one(&path)
            .expect("a torn unparseable tail must not be fatal")
            .expect("the header + good event must still recover");
        assert_eq!(first.durable_events, 1);

        // `reopen` (as the restarted server does) must repair the torn tail
        // before the caller appends the next record.
        {
            let mut wal = SessionWal::reopen(path.clone()).unwrap();
            wal.append(&chunk("good-two"), true).unwrap();
        }

        // Second restart's recovery must succeed cleanly with BOTH good
        // events and no error — the poison sequence is broken.
        let second = recover_one(&path)
            .expect("repaired WAL must recover without error on the next restart")
            .expect("session must still be present");
        assert_eq!(second.durable_events, 2);
    }

    #[test]
    fn reopen_terminates_parseable_unterminated_tail_and_keeps_it() {
        // A torn tail that happens to be a COMPLETE, parseable record just
        // missing its trailing newline (crash landed between the write and
        // the newline, or between two back-to-back appends) — `recover_one`
        // already accepts this as the final line, so `reopen`'s repair must
        // terminate (not discard) it, and recovery must keep it.
        let dir = tmp_dir("torn-tail-parseable");
        let path = {
            let wal = SessionWal::create(&dir, "s6", "l", Path::new("/tmp"), PermissionMode::Yolo)
                .unwrap();
            wal.path().to_path_buf()
        };
        let unterminated = serde_json::to_string(&WalRecord::Event(chunk("no-newline-yet")))
            .unwrap()
            .into_bytes();
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&unterminated).unwrap();
        }
        assert_ne!(
            std::fs::read(&path).unwrap().last(),
            Some(&b'\n'),
            "test setup must actually leave the tail unterminated"
        );

        {
            let mut wal = SessionWal::reopen(path.clone()).unwrap();
            wal.append(&chunk("after"), true).unwrap();
        }

        let recovered = recover_one(&path).unwrap().unwrap();
        // Both the repaired-and-kept event and the newly appended one survive.
        assert_eq!(recovered.durable_events, 2);
    }

    #[test]
    fn archive_marker_is_durable_and_last_transition_wins() {
        let dir = tmp_dir("archive-state");
        let path = {
            let mut wal =
                SessionWal::create(&dir, "cold-1", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&attached("acp-cold"), true).unwrap();
            wal.append_archived(true).unwrap();
            wal.path().to_path_buf()
        };

        let cold = recover_one(&path).unwrap().unwrap();
        assert!(cold.archived);
        assert_eq!(cold.acp_session_id.as_deref(), Some("acp-cold"));
        assert_eq!(cold.durable_events, 1, "archive is metadata, not a turn");

        {
            let mut wal = SessionWal::reopen(path.clone()).unwrap();
            wal.append_archived(false).unwrap();
        }
        let live = recover_one(&path).unwrap().unwrap();
        assert!(!live.archived, "the last lifecycle marker is authoritative");
        assert_eq!(live.durable_events, 1);
    }

    #[test]
    fn historical_wals_preserve_session_identity_and_every_decodable_event() {
        for version in [1, 2] {
            let dir = tmp_dir(&format!("v{version}-preserved"));
            std::fs::create_dir_all(&dir).unwrap();
            let sid = format!("old-v{version}");
            let path = wal_path(&dir, &sid);
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(
                f,
                r#"{{"t":"header","version":{version},"server_session_id":"{sid}","label":"historical","cwd":"/tmp","permission_mode":"Yolo"}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"t":"event","type":"session_attached","session_id":"{sid}","acp_session_id":"acp-{version}"}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"t":"event","type":"user_prompt","session_id":"{sid}","text":"preserve me"}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"t":"event","type":"reply_event","session_id":"{sid}","event":{{"Chunk":"complete history"}}}}"#
            )
            .unwrap();
            drop(f);

            let one = recover_one(&path)
                .unwrap_or_else(|error| panic!("v{version} recovery failed: {error}"))
                .unwrap_or_else(|| panic!("v{version} session was discarded"));
            assert_eq!(one.server_session_id, sid);
            assert_eq!(one.label, "historical");
            assert_eq!(
                one.acp_session_id.as_deref(),
                Some(&*format!("acp-{version}"))
            );
            assert_eq!(
                one.durable_events, 3,
                "v{version} recovery must retain every decodable event"
            );
            assert_eq!(recover_all(&dir).len(), 1);
        }
    }

    /// Phase-8 Stage A: a v3 log persists the `Agent { AgentEvent }` record and
    /// the `turn`/`seq` ride the envelope verbatim. `turns` derives from the
    /// agent `TurnEnded` (max(turn)+1), and `acp_session_id` still derives from
    /// the kept `SessionAttached` control variant.
    #[test]
    fn v3_agent_event_round_trips_turn_and_seq() {
        use crate::agent_event::{AgentEvent, AgentEventKind, ChunkRole, TurnOutcome};

        fn agent(seq: u64, turn: u64, kind: AgentEventKind) -> Notification {
            Notification::Agent {
                event: AgentEvent::new("s1".into(), 0, turn, seq, kind),
            }
        }

        let dir = tmp_dir("v3agent");
        {
            let mut wal =
                SessionWal::create(&dir, "s1", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&attached("acp-v3"), false).unwrap();
            wal.append(
                &agent(
                    0,
                    0,
                    AgentEventKind::Chunk {
                        text: "hi".into(),
                        role: ChunkRole::Message,
                    },
                ),
                false,
            )
            .unwrap();
            wal.append(
                &agent(
                    1,
                    0,
                    AgentEventKind::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
                true,
            )
            .unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1);
        let s = &recovered[0];
        assert_eq!(s.acp_session_id.as_deref(), Some("acp-v3"));
        // One completed turn at envelope turn 0 ⇒ turns == 1.
        assert_eq!(s.turns, 1, "turns derive from agent TurnEnded max(turn)+1");

        // The Agent record's envelope survived intact (on disk — the summary
        // holds no events; read them back through the same walker).
        let events = read_all_events(&s.path).unwrap();
        assert_eq!(events.len() as u64, s.durable_events);
        let agent_ev = events.iter().find_map(|n| match n {
            Notification::Agent { event } => Some(event),
            _ => None,
        });
        let ev = agent_ev.expect("an Agent record must survive recovery");
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.seq, 0); // first Agent record's local seq persisted verbatim
    }

    /// ReplayEnd is NOT a completed live turn — it must not bump the recovered
    /// turn count.
    #[test]
    fn v3_replay_end_does_not_count_as_turn() {
        use crate::agent_event::{AgentEvent, AgentEventKind, TurnOutcome};
        let dir = tmp_dir("v3replayend");
        {
            let mut wal =
                SessionWal::create(&dir, "s1", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(
                &Notification::Agent {
                    event: AgentEvent::new(
                        "s1".into(),
                        0,
                        5,
                        0,
                        AgentEventKind::TurnEnded {
                            outcome: TurnOutcome::ReplayEnd,
                        },
                    ),
                },
                true,
            )
            .unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].turns, 0,
            "ReplayEnd is a replay-prefix marker, not a completed turn"
        );
    }

    #[test]
    fn metadata_and_unsettled_prompt_intents_survive_recovery() {
        let dir = tmp_dir("metadata-prompts");
        let path = {
            let mut wal = SessionWal::create_for_provider(
                &dir,
                "durable-state",
                "durable state",
                Path::new("/tmp/work"),
                PermissionMode::ReadOnly,
                AgentProvider::Codex,
            )
            .unwrap();
            wal.append_permission_mode(PermissionMode::Yolo).unwrap();
            wal.append_model("gpt-durable").unwrap();
            wal.append_prompt_intent(
                7,
                &PromptPayload {
                    text: "retry after crash".into(),
                    text_blocks: vec!["one".into(), "two".into()],
                    images: vec![ImageAttachment {
                        data: "aW1hZ2U=".into(),
                        mime_type: "image/png".into(),
                    }],
                },
            )
            .unwrap();
            wal.append_prompt_intent(8, &PromptPayload::text("already delivered"))
                .unwrap();
            wal.append_prompt_terminal(8, PromptOutcome::Delivered)
                .unwrap();
            wal.path().to_path_buf()
        };

        let recovered = recover_one(&path).unwrap().unwrap();
        assert_eq!(recovered.permission_mode, PermissionMode::Yolo);
        assert_eq!(recovered.model_id.as_deref(), Some("gpt-durable"));
        assert_eq!(recovered.pending_prompts.len(), 1);
        assert_eq!(recovered.pending_prompts[0].id, 7);
        assert_eq!(
            recovered.pending_prompts[0].payload.text,
            "retry after crash"
        );
        assert_eq!(
            recovered.pending_prompts[0].payload.text_blocks,
            ["one", "two"]
        );
        assert_eq!(recovered.pending_prompts[0].payload.images.len(), 1);
        assert_eq!(recovered.next_prompt_id, 9);
    }

    #[test]
    fn remove_deletes_file() {
        let dir = tmp_dir("remove");
        let wal =
            SessionWal::create(&dir, "s4", "l", Path::new("/tmp"), PermissionMode::Yolo).unwrap();
        wal.remove().unwrap();
        assert!(recover_all(&dir).is_empty());
    }

    /// ADR-0038: the boot-time summary and the attach-time prefix stream are
    /// two views of ONE walker over the file, and they must agree — the
    /// summary's `durable_events` is exactly what `stream_durable_prefix`
    /// yields, in order, equal to a full decode; and the folded fields
    /// (`turns`, `acp_session_id`, `next_generation`) equal what a fold over
    /// the fully decoded events computes. Mixed legacy + Agent records, a
    /// rename, an archive flip and a torn tail are all in the fixture.
    #[test]
    fn summary_and_prefix_stream_agree_with_a_full_decode() {
        use crate::agent_event::{AgentEvent, AgentEventKind, ChunkRole, TurnOutcome};

        let dir = tmp_dir("summary-vs-stream");
        let path = {
            let mut wal = SessionWal::create(
                &dir,
                "s1",
                "before",
                Path::new("/tmp"),
                PermissionMode::Yolo,
            )
            .unwrap();
            wal.append(&attached("acp-first"), false).unwrap();
            wal.append(&chunk("legacy chunk"), false).unwrap();
            wal.append(&turn_ended(1), true).unwrap();
            wal.append_rename("after").unwrap();
            for (g, turn, seq) in [(0u64, 0u64, 0u64), (0, 1, 1), (2, 3, 0)] {
                wal.append(
                    &Notification::Agent {
                        event: AgentEvent::new(
                            "s1".into(),
                            g,
                            turn,
                            seq,
                            AgentEventKind::Chunk {
                                text: format!("g{g}t{turn}"),
                                role: ChunkRole::Message,
                            },
                        ),
                    },
                    false,
                )
                .unwrap();
                wal.append(
                    &Notification::Agent {
                        event: AgentEvent::new(
                            "s1".into(),
                            g,
                            turn,
                            seq + 1,
                            AgentEventKind::TurnEnded {
                                outcome: TurnOutcome::Completed,
                            },
                        ),
                    },
                    true,
                )
                .unwrap();
            }
            wal.append(&attached("acp-last"), false).unwrap();
            wal.append_archived(true).unwrap();
            wal.append_archived(false).unwrap();
            wal.path().to_path_buf()
        };
        // A torn tail: tolerated by every reader, counted by none.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"{\"t\":\"event\",\"type\":\"reply_ev")
                .unwrap();
        }

        let full = read_all_events(&path).unwrap();
        let summary = recover_one(&path).unwrap().unwrap();
        assert_eq!(summary.durable_events, full.len() as u64);
        assert_eq!(summary.durable_events, 10);
        assert_eq!(summary.label, "after");
        assert!(!summary.archived);
        assert_eq!(summary.acp_session_id.as_deref(), Some("acp-last"));
        // legacy: one TurnEnded; agent: max(turn)=3 ⇒ 4. max ⇒ 4.
        assert_eq!(summary.turns, 4);
        // max Agent generation 2 ⇒ next 3.
        assert_eq!(summary.next_generation, 3);

        let mut streamed = Vec::new();
        let n = stream_durable_prefix(&path, summary.durable_events, |note| {
            streamed.push(note);
            Ok(())
        })
        .unwrap();
        assert_eq!(n, 10);
        assert_eq!(
            serde_json::to_string(&streamed).unwrap(),
            serde_json::to_string(&full).unwrap(),
            "the streamed prefix must be the full decode, in order"
        );

        // A shorter prefix stops early — it never reads past `len`.
        let mut head = Vec::new();
        assert_eq!(
            stream_durable_prefix(&path, 3, |note| {
                head.push(note);
                Ok(())
            })
            .unwrap(),
            3
        );
        assert_eq!(
            serde_json::to_string(&head).unwrap(),
            serde_json::to_string(&full[..3]).unwrap()
        );
        assert_eq!(stream_durable_prefix(&path, 0, |_| Ok(())).unwrap(), 0);

        // A file shorter than the prefix it is expected to hold streams what
        // exists and then fails loudly, naming the shortfall.
        let mut partial = Vec::new();
        let error = stream_durable_prefix(&path, 11, |note| {
            partial.push(note);
            Ok(())
        })
        .expect_err("a short file must not silently gap the prefix");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("holds 10"), "{error}");
        assert!(error.to_string().contains("expects 11"), "{error}");
        assert_eq!(
            partial.len(),
            10,
            "everything that exists was still streamed"
        );

        // A sink error aborts the stream at that record.
        let mut seen = 0;
        let error = stream_durable_prefix(&path, 10, |_| {
            seen += 1;
            if seen == 4 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "client gone",
                ))
            } else {
                Ok(())
            }
        })
        .expect_err("sink errors propagate");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(seen, 4);
    }
}
