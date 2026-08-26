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
//!   loss): it is skipped rather than aborting the whole replay.
//!
//! Log compaction / snapshotting is deferred (ADR-0009) until a long session
//! measurably hurts memory or recovery latency; until then the full log is
//! replayed and `seq`/`turns` stay simple absolute counts.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
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
    /// rename over the header label. Older binaries that don't know this variant
    /// skip it on the `serde` error path and fall back to the header label
    /// (graceful downgrade), so no version bump is needed.
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
/// `recover_one` discards any header whose version != `WAL_VERSION` (no
/// converter — locked decision), so pre-v3 logs are dropped and those sessions
/// resume empty (re-load from the agent). This reuses the EXACT phase-4 v1→v2
/// discard-on-read machinery: the `Header` is always the first record, so the
/// version gate fires before any incompatible `Event` line reaches serde.
const WAL_VERSION: u32 = 3;

/// A live write handle to one session's WAL file. The session server's
/// `ManagedSession` owns exactly one of these and is its only writer.
pub struct SessionWal {
    file: File,
    path: PathBuf,
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
        // Truncate: a fresh session starts a fresh log. (A reused id would be a
        // bug; truncating is the safe choice.)
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
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
    pub fn reopen(path: PathBuf) -> std::io::Result<SessionWal> {
        let file = OpenOptions::new().append(true).open(&path)?;
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
    /// The replayed transcript, in order.
    pub event_log: Vec<Notification>,
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

/// Replay a single WAL file. Returns `Ok(None)` if the file has no valid
/// header. A torn/partial final line (interrupted write on power loss) is
/// skipped — that is the bounded data loss the contract permits.
pub fn recover_one(path: &Path) -> std::io::Result<Option<RecoveredSession>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut header: Option<(String, String, PathBuf, PermissionMode, AgentProvider)> = None;
    let mut event_log: Vec<Notification> = Vec::new();
    // The most recent rename, if any. Applied over the header label so a
    // session recovered after a server restart keeps its renamed name rather
    // than reverting to the creation-time label.
    let mut renamed_label: Option<String> = None;
    let mut archived = false;
    let mut permission_override: Option<PermissionMode> = None;
    let mut model_id: Option<String> = None;
    let mut prompt_intents: Vec<RecoveredPrompt> = Vec::new();
    let mut prompt_terminal = std::collections::HashSet::new();
    let mut max_prompt_id: Option<u64> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            // An I/O error mid-file (e.g. a torn final line on power loss):
            // stop replaying here, keep what we have.
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let rec: WalRecord = match serde_json::from_str(&line) {
            Ok(r) => r,
            // A partial/corrupt line (almost always the last, torn by a crash
            // mid-write): skip it. Earlier lines already parsed are intact.
            Err(_) => continue,
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
                // Version gate: a header from any prior schema is discarded
                // wholesale — the session resumes empty and re-loads from the
                // live agent on the next attach (locked decision: no converter).
                // This validates the v1/v2 → v3 discard: an older log can carry
                // Event variants that the current `Notification` enum no longer
                // deserializes, so loading it line-by-line would silently drop
                // them; discarding at the header (always the FIRST record) avoids
                // a half-parsed log. (The retired `owner_changed`/`lease_changed`
                // control notes were broadcast-only and never appended to the
                // event_log, so the gate is NOT what keeps them out — they simply
                // were never WAL records.)
                if version != WAL_VERSION {
                    eprintln!(
                        "[session-wal] discarding pre-v{WAL_VERSION} WAL {} (version {version}); \
                         session resumes empty",
                        path.display()
                    );
                    return Ok(None);
                }
                header = Some((server_session_id, label, cwd, permission_mode, provider));
            }
            WalRecord::Event(note) => event_log.push(note),
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

    let Some((server_session_id, header_label, cwd, header_permission_mode, provider)) = header
    else {
        return Ok(None);
    };
    // Last rename wins over the creation-time header label.
    let label = renamed_label.unwrap_or(header_label);
    let permission_mode = permission_override.unwrap_or(header_permission_mode);
    let pending_prompts = prompt_intents
        .into_iter()
        .filter(|intent| !prompt_terminal.contains(&intent.id))
        .collect();
    let next_prompt_id = max_prompt_id.map_or(1, |id| id.saturating_add(1));

    // Re-derive the agent resume id from the last SessionAttached. KEPT as a
    // control variant in the collapse (spec §1) precisely so this recovery
    // dependency survives — folding it into ChannelOpened would lose the id.
    let acp_session_id = event_log.iter().rev().find_map(|n| match n {
        Notification::SessionAttached { acp_session_id, .. } => acp_session_id.clone(),
        _ => None,
    });

    // Completed-turn count = the durable tip (spec §5). During the additive
    // rollout (spec §9) a log may carry BOTH legacy `TurnEnded` records AND the
    // new `Agent { TurnEnded }` records describing the SAME boundaries, so we
    // take the max of the two interpretations rather than summing (which would
    // double-count). Legacy: count of `TurnEnded`. Agent: `max(turn)+1` over
    // `Agent` events whose kind is a real `TurnEnded` (excluding `ReplayEnd`,
    // which marks the end of a replayed prefix, not a completed live turn).
    use crate::agent_event::{AgentEventKind, TurnOutcome};
    let legacy_turns = event_log
        .iter()
        .filter(|n| matches!(n, Notification::TurnEnded { .. }))
        .count();
    let agent_turns = event_log
        .iter()
        .filter_map(|n| match n {
            Notification::Agent { event } => match &event.kind {
                AgentEventKind::TurnEnded { outcome }
                    if !matches!(outcome, TurnOutcome::ReplayEnd) =>
                {
                    Some(event.turn)
                }
                _ => None,
            },
            _ => None,
        })
        .max()
        // `turn` is 0-based in the envelope; a completed turn `k` means `k+1`
        // turns have settled. `max(turn)+1` is the count.
        .map(|max_turn| (max_turn + 1) as usize)
        .unwrap_or(0);
    let turns = legacy_turns.max(agent_turns);

    Ok(Some(RecoveredSession {
        path: path.to_path_buf(),
        server_session_id,
        label,
        cwd,
        permission_mode,
        model_id,
        provider,
        event_log,
        acp_session_id,
        turns,
        archived,
        pending_prompts,
        next_prompt_id,
    }))
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
        assert_eq!(s.event_log.len(), 4);
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
        assert_eq!(s.event_log.len(), 2);
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
        assert_eq!(s.event_log.len(), 2);
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
        assert_eq!(recovered[0].event_log.len(), 2);
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
        assert_eq!(cold.event_log.len(), 1, "archive is metadata, not a turn");

        {
            let mut wal = SessionWal::reopen(path.clone()).unwrap();
            wal.append_archived(false).unwrap();
        }
        let live = recover_one(&path).unwrap().unwrap();
        assert!(!live.archived, "the last lifecycle marker is authoritative");
        assert_eq!(live.event_log.len(), 1);
    }

    #[test]
    fn v1_wal_is_discarded_on_read() {
        // Hand-write a pre-v3 (version:1) log with a header + a couple events,
        // including the retired `owner_changed` control line. The current reader
        // must discard the whole file (Ok(None)) and not crash.
        let dir = tmp_dir("v1discard");
        std::fs::create_dir_all(&dir).unwrap();
        let path = wal_path(&dir, "old1");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"t":"header","version":1,"server_session_id":"old1","label":"l","cwd":"/tmp","permission_mode":"yolo"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"t":"event","type":"owner_changed","session_id":"old1","has_owner":true}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"t":"event","type":"user_prompt","session_id":"old1","text":"hi"}}"#
        )
        .unwrap();
        drop(f);

        let one = recover_one(&path).expect("recover_one must not error on a v1 log");
        assert!(one.is_none(), "v1 log must be discarded (Ok(None))");
        assert!(
            recover_all(&dir).is_empty(),
            "discarded v1 session must be absent from recovery"
        );

        // A fresh v3 create→append→recover round-trip still works afterward.
        {
            let mut wal =
                SessionWal::create(&dir, "new3", "l", Path::new("/tmp"), PermissionMode::Yolo)
                    .unwrap();
            wal.append(&attached("acp-v3"), false).unwrap();
            wal.append(&turn_ended(1), true).unwrap();
        }
        let recovered = recover_all(&dir);
        assert_eq!(recovered.len(), 1, "v3 session must recover normally");
        assert_eq!(recovered[0].server_session_id, "new3");
    }

    /// Phase-8 Stage A: a pre-v3 (version:2) log — which may carry a legacy
    /// `reply_event` line that the post-collapse reader still understands but
    /// whose schema is nonetheless retired — is discarded wholesale by the
    /// version gate before any Event line is parsed (mirrors `v1_wal_is_...`).
    #[test]
    fn v2_wal_is_discarded_on_read() {
        let dir = tmp_dir("v2discard");
        std::fs::create_dir_all(&dir).unwrap();
        let path = wal_path(&dir, "old2");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"t":"header","version":2,"server_session_id":"old2","label":"l","cwd":"/tmp","permission_mode":"yolo"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"t":"event","type":"reply_event","session_id":"old2","event":{{"Chunk":"hi"}}}}"#
        )
        .unwrap();
        drop(f);

        let one = recover_one(&path).expect("recover_one must not error on a v2 log");
        assert!(one.is_none(), "v2 log must be discarded (Ok(None))");
        assert!(recover_all(&dir).is_empty());
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

        // The Agent record's envelope survived intact.
        let agent_ev = s.event_log.iter().find_map(|n| match n {
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
}
