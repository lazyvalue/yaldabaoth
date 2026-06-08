//! `sketch-session-server` — thin daemon that owns ACP agent subprocesses.
//!
//! The GUI (`sketch-gpui`) connects over a Unix domain socket and
//! creates/attaches/prompts sessions. When the GUI is rebuilt and
//! relaunched, it reconnects to the same running server — agent sessions
//! survive the transition.
//!
//! Run:
//!     cargo run --bin sketch-session-server
//!
//! The GUI auto-launches this binary if not already running.

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};

use sketch::acp_channel::{AcpChannelClient, PermissionMode, SketchFrontend, TransportHandle};
use sketch::session_proto::*;

mod launchd;

// ── Actor command inlet ────────────────────────────────────────────
//
// All session-state mutation flows through this single inlet, drained by the
// single-writer `run_manager` actor task that OWNS the HashMap (no Mutex).
// `sid` = ServerSessionId. Oneshot replies are used where the caller needs a
// consistent read/ack; pump-sourced commands carry no reply.
//
// `generation` on the pump-sourced commands (Record/TurnCount/AgentDisconnected)
// is the fence (Blocker B): the actor ignores any whose generation !=
// session.channel_generation.
enum Command {
    // ── External (connection-handler sourced; each carries a oneshot) ──
    Create {
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<SessionInfo>,
    },
    Attach {
        sid: ServerSessionId,
        mode: AttachMode,
        conn_id: u64,
        /// Optional reconnect cursor `(generation, index)`. Resolved by
        /// `do_attach` against the session's `channel_generation` +
        /// `event_log.len()` into the forwarder's initial `sent` value (the
        /// `usize` in the reply): the tail starts there. `None` / stale /
        /// out-of-range ⇒ `0` ⇒ full replay (unchanged behavior).
        cursor: Option<(u64, u64)>,
        reply: tokio::sync::oneshot::Sender<
            Result<
                (
                    watch::Receiver<bool>,
                    watch::Receiver<Arc<Vec<Notification>>>,
                    usize,
                ),
                String,
            >,
        >,
    },
    Detach {
        sid: ServerSessionId,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Promote {
        sid: ServerSessionId,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Prompt {
        sid: ServerSessionId,
        text: String,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Headless "start-work" enqueue (ADR-0015): same as `Prompt` but with NO
    /// owner gate. The handler calls `enqueue_prompt` directly, so a non-GUI
    /// caller can drive a turn on a session it does not own.
    AdminPrompt {
        session_id: ServerSessionId,
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Cancel {
        sid: ServerSessionId,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Close {
        sid: ServerSessionId,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Restart {
        sid: ServerSessionId,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Rename {
        sid: ServerSessionId,
        label: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetPermissionMode {
        sid: ServerSessionId,
        mode: PermissionMode,
        conn_id: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    ListSessions {
        reply: tokio::sync::oneshot::Sender<Vec<SessionInfo>>,
    },
    AdminQuery {
        reply: tokio::sync::oneshot::Sender<AdminSnapshot>,
    },
    SessionCount {
        reply: tokio::sync::oneshot::Sender<usize>,
    },

    // ── Spawn-worker sourced (channel (re)spawn completed) ──
    // The freshly-spawned client's `handle` (Send surface) is installed in the
    // map; the OWNING pump thread is spawned by the worker AFTER the actor
    // replies the committed generation. The actor never receives or drops the
    // client. `is_respawn` bumps generation (and gen_watch) so the old pump
    // self-terminates and drops its client off-actor (Blocker A).
    PublishChannel {
        sid: ServerSessionId,
        handle: TransportHandle,
        is_respawn: bool,
        // On success: (committed generation, gen_watch subscription, replay
        // fence) — everything the OWNING pump needs to drive + self-terminate.
        // `None` if the session was closed while spawning.
        reply: tokio::sync::oneshot::Sender<Option<(u64, watch::Receiver<u64>, usize)>>,
    },
    SpawnFailed {
        sid: ServerSessionId,
        reason: String,
    },

    // ── Pump-thread sourced (fire-and-forget; generation-fenced) ──
    Record {
        sid: ServerSessionId,
        generation: u64,
        event: sketch::acp_channel::ReplyEvent,
    },
    TurnCount {
        sid: ServerSessionId,
        generation: u64,
        turns: usize,
    },
    AgentDisconnected {
        sid: ServerSessionId,
        generation: u64,
    },
}

/// CLI: with no subcommand the binary runs the server (the default the GUI
/// auto-launches); subcommands manage launchd supervision.
#[derive(clap::Parser)]
#[command(name = "sketch-session-server", about = "Sketch ACP session-server daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Subcmd>,
}

#[derive(clap::Subcommand)]
enum Subcmd {
    /// Install + load the launchd LaunchAgent: the server starts at login and
    /// is restarted automatically if it crashes (so agent sessions run with no
    /// GUI present). Hands off any running server losslessly via its WAL.
    Install,
    /// Unload + remove the launchd LaunchAgent.
    Uninstall,
    /// Show whether the LaunchAgent is installed/loaded and the socket is live.
    Status,
    /// Enqueue a prompt to an existing session with no GUI attached (headless
    /// start-work). Connects to the already-running server and drives a turn on
    /// a session this CLI does not own (ADR-0015); the agent runs it to
    /// completion with no GUI ever attaching.
    Prompt {
        /// The id of the existing session to enqueue the prompt to.
        session_id: String,
        /// The prompt text to send to the agent.
        text: String,
    },
}

// ── Managed session ────────────────────────────────────────────────

struct ManagedSession {
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    /// The live ACP transport surface — the Send sub-handles of the
    /// `AcpChannelClient` whose `reply_rx` is owned by the pump thread. `None`
    /// while the subprocess is being spawned. The actor never holds the client
    /// itself (so its blocking `Drop` never runs on the actor task).
    channel: Option<TransportHandle>,
    /// Bumped every time `channel` is replaced (force-restart). The apply
    /// handlers fence stale pump messages on this (Blocker B, CP5), and the
    /// `gen_watch` mirror lets the old pump self-terminate (Blocker A).
    channel_generation: u64,
    /// Mirrors `channel_generation` so each pump thread can observe a restart
    /// (generation bump) and self-terminate + drop its owned client off the
    /// actor task (Blocker A).
    gen_watch: watch::Sender<u64>,
    turns: usize,
    permission_mode: PermissionMode,
    /// Per-session transcript log channel. Holds the latest snapshot of
    /// `event_log` (as a cloned `Arc`); every `record`/`log_only` sends the
    /// updated snapshot via `send_replace`. The forwarder tails `[sent..]` of
    /// the latest snapshot lock-free — watch coalescing self-heals exactly like
    /// the old broadcast `Lagged` path.
    log_tx: watch::Sender<Arc<Vec<Notification>>>,
    /// Per-session ownership control channel. Holds the current
    /// `owner.is_some()`. The forwarder selects on this and synthesizes a single
    /// `OwnerChanged` control note on change — replaces the broadcast-as-wake
    /// path for ownership state.
    owner_tx: watch::Sender<bool>,
    /// Connection id of the current owner — the only connection allowed to
    /// drive the session (prompt / set permission / close). `None` when no
    /// owner is attached, in which case an observer may `Promote` to claim it.
    owner: Option<u64>,
    /// Prompts that arrived before the ACP subprocess finished spawning.
    /// Drained in submission order once `channel` becomes `Some`.
    pending_prompts: Vec<String>,
    /// Every notification ever broadcast for this session, so a
    /// re-attaching GUI can replay the full transcript.
    ///
    /// Wrapped in `Arc` so `attach` clones a *pointer* under the
    /// global lock, not the whole (unbounded) `Vec`. Pushes go through
    /// `Arc::make_mut`, which is a cheap in-place mutation whenever the only
    /// reference is this field (the common case — snapshots are short-lived and
    /// released before the next push).
    event_log: Arc<Vec<Notification>>,
    /// Persisted turn count at restore time. The pump thread suppresses
    /// logging while the ACP agent's turn counter is ≤ this value, since
    /// those events are replays of turns already in `event_log`. Once the
    /// agent moves past the fence (a genuinely new turn), normal logging
    /// resumes. Zero for fresh (non-restored) sessions.
    replay_fence: usize,
    /// Durable write-ahead log for this session (ADR-0009). Every logged event
    /// is appended here so a crash (not just a clean shutdown) preserves the
    /// transcript. `None` only if the WAL couldn't be opened (we degrade to
    /// in-memory-only rather than refusing to run).
    wal: Option<sketch::session_wal::SessionWal>,
}

impl ManagedSession {
    fn info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.id.clone(),
            acp_session_id: self.channel.as_ref().and_then(|c| c.session_id()),
            label: self.label.clone(),
            cwd: self.cwd.clone(),
            turns: self.turns,
            connected: self.channel.as_ref().is_some_and(|c| c.is_connected()),
            permission_mode: self.permission_mode,
            has_owner: self.owner.is_some(),
        }
    }

    /// Record that an event happened: append it to the durable `event_log`
    /// (source of truth) **and** fire the broadcast wake in one step. This is
    /// the single mutator for "a logged event happened" — every log+broadcast
    /// site routes through here so the two writes can never skew (one appended
    /// without waking subscribers, or one broadcast without being logged).
    ///
    /// The `OwnerChanged` broadcast-only path (`broadcast_owner_changed`) is
    /// deliberately NOT routed through here: it is transient connection state,
    /// not transcript, and must never land in `event_log`.
    fn record(&mut self, note: Notification) {
        self.wal_append(&note);
        Arc::make_mut(&mut self.event_log).push(note);
        let _ = self.log_tx.send_replace(Arc::clone(&self.event_log));
    }

    /// Append a transcript event to `event_log` + WAL and fire the watch wake.
    /// Used for the user's own prompt: the live GUI already rendered it locally,
    /// and its transcript reconciler dedups the prompt it then sees replayed via
    /// the log tail (the watch delivers every event_log entry, same as the old
    /// broadcast tail did). Distinguished from [`record`] only in intent — both
    /// now publish through the per-session `log_tx` watch.
    fn log_only(&mut self, note: Notification) {
        self.wal_append(&note);
        Arc::make_mut(&mut self.event_log).push(note);
        let _ = self.log_tx.send_replace(Arc::clone(&self.event_log));
    }

    /// Append `note` to the durable WAL. `fsync`s at turn boundaries
    /// (`UserPrompt` / `TurnEnded`) so a completed turn or a sent prompt is
    /// never lost on power loss, but not per streamed chunk (ADR-0009). A WAL
    /// error is logged, never fatal — the in-memory `event_log` still holds the
    /// event for live subscribers.
    fn wal_append(&mut self, note: &Notification) {
        if let Some(wal) = self.wal.as_mut() {
            let boundary = matches!(
                note,
                Notification::UserPrompt { .. } | Notification::TurnEnded { .. }
            );
            if let Err(e) = wal.append(note, boundary) {
                tracing::error!(
                    session_id = %&self.id[..8.min(self.id.len())],
                    error = %e,
                    "WAL append failed"
                );
            }
        }
    }

    /// Broadcast an `OwnerChanged` to all attached connections. Not appended
    /// to `event_log` — ownership is transient connection state, not part of
    /// the conversation transcript a late observer needs to replay.
    fn broadcast_owner_changed(&self) {
        let _ = self.owner_tx.send_replace(self.owner.is_some());
    }

    /// Publish a freshly-spawned channel's `TransportHandle` as this session's
    /// live transport, running the full attach choreography atomically under the
    /// caller's lock. The single chokepoint for create / restore / restart (9′)
    /// so the three can't drift:
    /// 1. Re-apply the session's `permission_mode` (a fresh channel starts at
    ///    its default — without this the configured mode silently reverts).
    /// 2. Drain `pending_prompts` in arrival order onto the new transport BEFORE
    ///    publishing it, so they're enqueued at the ACP driver before any
    ///    future prompt races in. Doing this under the lock also closes the
    ///    take-then-publish window where a concurrent `prompt()` could re-queue
    ///    onto a `pending_prompts` we'd already drained.
    /// 3. On a respawn (force-restart), bump `channel_generation` AND the
    ///    `gen_watch` mirror so (a) the apply handlers fence the old pump's
    ///    in-flight messages (Blocker B, CP5) and (b) the OLD pump thread
    ///    observes the bump and self-terminates + drops its owned client off the
    ///    actor task (Blocker A).
    /// 4. Swap the handle in and `record(SessionAttached)`.
    ///
    /// Unlike the old client-owning version this returns nothing: the actor only
    /// ever holds the cheap Send `TransportHandle`; the owning `AcpChannelClient`
    /// (and its blocking `Drop`) lives on the pump's OS thread.
    fn apply_channel_state(&mut self, mut handle: TransportHandle, is_respawn: bool) {
        handle.set_permission_mode(self.permission_mode);
        for text in std::mem::take(&mut self.pending_prompts) {
            if let Err(e) = handle.send(&text) {
                tracing::error!(error = %e, "failed to flush queued prompt");
            }
        }
        let acp_session_id = handle.session_id();
        if is_respawn {
            self.channel_generation = self.channel_generation.wrapping_add(1);
            let _ = self.gen_watch.send_replace(self.channel_generation);
        }
        handle.generation = self.channel_generation;
        self.channel = Some(handle);
        self.record(Notification::SessionAttached {
            session_id: self.id.clone(),
            acp_session_id,
        });
    }
}

// ── Session manager ────────────────────────────────────────────────

/// Build a fresh `ManagedSession` for a brand-new session.
fn new_managed_session(
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    permission_mode: PermissionMode,
    wal: Option<sketch::session_wal::SessionWal>,
) -> ManagedSession {
    let event_log = Arc::new(Vec::new());
    let (log_tx, _) = watch::channel(Arc::clone(&event_log));
    let (owner_tx, _) = watch::channel(false);
    let (gen_watch, _) = watch::channel(0u64);
    ManagedSession {
        id,
        label,
        cwd,
        channel: None,
        channel_generation: 0,
        gen_watch,
        turns: 0,
        permission_mode,
        log_tx,
        owner_tx,
        owner: None,
        pending_prompts: Vec::new(),
        event_log,
        replay_fence: 0,
        wal,
    }
}

/// A pending ACP resume job produced by WAL recovery — the seed map plus the
/// data each resume worker needs to re-spawn its subprocess.
struct ResumeJob {
    session_id: ServerSessionId,
    cwd: PathBuf,
    acp_session_id: String,
}

/// The single-writer actor state: it OWNS the sessions map (no Mutex). Mutated
/// only on the `run_manager` task, one command at a time.
struct Manager {
    sessions: HashMap<ServerSessionId, ManagedSession>,
    /// Manager-level broadcast for session-list changes (create/close/rename).
    events: broadcast::Sender<Notification>,
    default_permission_mode: PermissionMode,
    /// The inlet sender — cloned into spawn workers so they can post back
    /// `PublishChannel` / `SpawnFailed` without touching the map directly.
    cmd_tx: mpsc::UnboundedSender<Command>,
}

/// The public handle the connection handlers hold. All mutation goes through
/// `cmd_tx`; the single-writer `run_manager` actor owns the sessions map. The
/// handle keeps only the inlet sender and the manager-level (session-list)
/// broadcast — there is no shared map and no lock.
struct SessionManager {
    /// Manager-level broadcast for session-list changes (create/close/rename).
    events: broadcast::Sender<Notification>,
    /// The actor command inlet — every request becomes a Command sent here.
    cmd_tx: mpsc::UnboundedSender<Command>,
}

/// Open a fresh durable WAL for a session, or `None` (degrade to in-memory) if
/// no WAL dir is resolvable or the file can't be created — logged, never fatal.
fn open_session_wal(
    id: &str,
    label: &str,
    cwd: &std::path::Path,
    permission_mode: PermissionMode,
) -> Option<sketch::session_wal::SessionWal> {
    let dir = session_wal_dir()?;
    match sketch::session_wal::SessionWal::create(&dir, id, label, cwd, permission_mode) {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::error!(
                session_id = %&id[..8.min(id.len())],
                error = %e,
                "WAL create failed (in-memory only)"
            );
            None
        }
    }
}

impl SessionManager {
    fn new_with_inlet(
        default_permission_mode: PermissionMode,
    ) -> (Self, mpsc::UnboundedReceiver<Command>, PermissionMode) {
        let (events, _) = broadcast::channel(1024);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        (
            Self { events, cmd_tx },
            cmd_rx,
            default_permission_mode,
        )
    }

    /// Subscribe to manager-level session-list notifications.
    fn subscribe_events(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    // ── Async request wrappers (oneshot round-trip to the actor) ──

    async fn send_create(
        &self,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Create {
            cwd,
            label,
            resume_session_id,
            reply,
        });
        rx.await.expect("actor dropped a Create reply")
    }

    async fn send_attach(
        &self,
        sid: &str,
        mode: AttachMode,
        conn_id: u64,
        cursor: Option<(u64, u64)>,
    ) -> Result<
        (
            watch::Receiver<bool>,
            watch::Receiver<Arc<Vec<Notification>>>,
            usize,
        ),
        String,
    > {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Attach {
            sid: sid.to_string(),
            mode,
            conn_id,
            cursor,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_detach(&self, sid: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Detach {
            sid: sid.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_promote(&self, sid: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Promote {
            sid: sid.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_prompt(&self, sid: &str, text: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Prompt {
            sid: sid.to_string(),
            text: text.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    /// Headless ungated enqueue (ADR-0015). No `conn_id` / owner check.
    async fn send_admin_prompt(&self, sid: &str, text: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::AdminPrompt {
            session_id: sid.to_string(),
            text: text.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_cancel(&self, sid: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Cancel {
            sid: sid.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_close(&self, sid: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Close {
            sid: sid.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_restart(&self, sid: &str, conn_id: u64) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Restart {
            sid: sid.to_string(),
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_rename(&self, sid: &str, label: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Rename {
            sid: sid.to_string(),
            label,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_set_permission_mode(
        &self,
        sid: &str,
        mode: PermissionMode,
        conn_id: u64,
    ) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SetPermissionMode {
            sid: sid.to_string(),
            mode,
            conn_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_list_sessions(&self) -> Vec<SessionInfo> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::ListSessions { reply });
        rx.await.unwrap_or_default()
    }

    async fn send_admin_status(&self) -> AdminSnapshot {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::AdminQuery { reply });
        rx.await.unwrap_or(AdminSnapshot {
            session_count: 0,
            sessions: Vec::new(),
        })
    }

    async fn send_session_count(&self) -> usize {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SessionCount { reply });
        rx.await.unwrap_or(0)
    }
}

/// Recover sessions from their durable WALs (ADR-0009). Returns the SEED map
/// (moved into `run_manager` before the actor starts) plus the resume jobs whose
/// workers re-spawn the ACP subprocesses (each posting `PublishChannel` back
/// into the actor). Runs once at startup before accepting connections.
fn restore_seed_from_disk() -> (HashMap<ServerSessionId, ManagedSession>, Vec<ResumeJob>) {
    let mut sessions = HashMap::new();
    let mut jobs = Vec::new();
    let Some(dir) = session_wal_dir() else {
        return (sessions, jobs);
    };
    let recovered = sketch::session_wal::recover_all(&dir);
    for rs in recovered {
        let sid = rs.server_session_id.clone();
        let Some(acp_session_id) = rs.acp_session_id.clone() else {
            tracing::warn!(
                session_id = %&sid[..8.min(sid.len())],
                "discarding recovered session: no acp_session_id to resume"
            );
            let _ = std::fs::remove_file(&rs.path);
            continue;
        };

        let wal = match sketch::session_wal::SessionWal::reopen(rs.path.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::error!(
                    session_id = %&sid[..8.min(sid.len())],
                    error = %e,
                    "WAL reopen failed"
                );
                None
            }
        };

        let event_log = Arc::new(rs.event_log);
        // Seed the watch with the recovered log so the first tail sees history.
        let (log_tx, _) = watch::channel(Arc::clone(&event_log));
        let (owner_tx, _) = watch::channel(false);
        let (gen_watch, _) = watch::channel(0u64);
        let session = ManagedSession {
            id: sid.clone(),
            label: rs.label.clone(),
            cwd: rs.cwd.clone(),
            channel: None,
            channel_generation: 0,
            gen_watch,
            turns: rs.turns,
            permission_mode: rs.permission_mode,
            log_tx,
            owner_tx,
            owner: None,
            pending_prompts: Vec::new(),
            event_log,
            replay_fence: rs.turns,
            wal,
        };

        tracing::info!(
            session_id = %&sid[..8.min(sid.len())],
            events = session.event_log.len(),
            turns = rs.turns,
            acp_session_id = %&acp_session_id[..8.min(acp_session_id.len())],
            "recovering session"
        );

        sessions.insert(sid.clone(), session);
        jobs.push(ResumeJob {
            session_id: sid,
            cwd: rs.cwd,
            acp_session_id,
        });
    }
    (sessions, jobs)
}

/// Spawn the OS thread that re-spawns a recovered session's ACP subprocess with
/// `--resume`, then publishes the transport via the actor inlet.
fn spawn_resume_worker(cmd_tx: mpsc::UnboundedSender<Command>, job: ResumeJob) {
    let ResumeJob {
        session_id,
        cwd,
        acp_session_id,
    } = job;
    std::thread::Builder::new()
        .name(format!("acp-resume-{}", &session_id[..8.min(session_id.len())]))
        .spawn(move || {
            // SAFETY: dedicated spawn thread; see create worker.
            unsafe {
                std::env::set_var("SKETCH_SESSION_MANAGED", "1");
            }
            let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
            match AcpChannelClient::spawn_with_resume_in(
                &cmd,
                Some(cwd),
                Some(acp_session_id),
                SketchFrontend::Gpui,
            ) {
                Ok(client) => {
                    // Resume from disk → is_respawn=false (generation stays 0).
                    publish_channel(&cmd_tx, &session_id, client, false);
                }
                Err(e) => {
                    tracing::error!(
                        session_id = %&session_id[..8.min(session_id.len())],
                        error = %e,
                        "failed to resume session"
                    );
                    let _ = cmd_tx.send(Command::SpawnFailed {
                        sid: session_id,
                        reason: format!("resume failed: {e}"),
                    });
                }
            }
        })
        .ok();
}

// ── Manager actor task ─────────────────────────────────────────────

/// The single-writer actor: owns the sessions map and drains the inlet, one
/// command at a time. Replaces the old mutex-guarded map + per-method locking.
async fn run_manager(
    mut rx: mpsc::UnboundedReceiver<Command>,
    sessions: HashMap<ServerSessionId, ManagedSession>,
    events: broadcast::Sender<Notification>,
    default_permission_mode: PermissionMode,
    cmd_tx: mpsc::UnboundedSender<Command>,
) {
    let mut mgr = Manager {
        sessions,
        events,
        default_permission_mode,
        cmd_tx,
    };
    while let Some(cmd) = rx.recv().await {
        mgr.apply(cmd);
    }
}

impl Manager {
    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Create {
                cwd,
                label,
                resume_session_id,
                reply,
            } => {
                let info = self.do_create(cwd, label, resume_session_id);
                let _ = reply.send(info);
            }
            Command::Attach {
                sid,
                mode,
                conn_id,
                cursor,
                reply,
            } => {
                let _ = reply.send(self.do_attach(&sid, mode, conn_id, cursor));
            }
            Command::Detach { sid, conn_id, reply } => {
                let _ = reply.send(self.do_detach(&sid, conn_id));
            }
            Command::Promote { sid, conn_id, reply } => {
                let _ = reply.send(self.do_promote(&sid, conn_id));
            }
            Command::Prompt {
                sid,
                text,
                conn_id,
                reply,
            } => {
                let _ = reply.send(self.do_prompt(&sid, &text, conn_id));
            }
            Command::AdminPrompt {
                session_id,
                text,
                reply,
            } => {
                // Ungated: enqueue directly, no owner check (ADR-0015).
                let _ = reply.send(self.enqueue_prompt(&session_id, &text));
            }
            Command::Cancel { sid, conn_id, reply } => {
                let _ = reply.send(self.do_cancel(&sid, conn_id));
            }
            Command::Close { sid, conn_id, reply } => {
                let _ = reply.send(self.do_close(&sid, conn_id));
            }
            Command::Restart { sid, conn_id, reply } => {
                let _ = reply.send(self.do_restart(&sid, conn_id));
            }
            Command::Rename { sid, label, reply } => {
                let _ = reply.send(self.do_rename(&sid, label));
            }
            Command::SetPermissionMode {
                sid,
                mode,
                conn_id,
                reply,
            } => {
                let _ = reply.send(self.do_set_permission_mode(&sid, mode, conn_id));
            }
            Command::ListSessions { reply } => {
                let _ = reply.send(self.sessions.values().map(|s| s.info()).collect());
            }
            Command::AdminQuery { reply } => {
                let _ = reply.send(self.do_admin_status());
            }
            Command::SessionCount { reply } => {
                let _ = reply.send(self.sessions.len());
            }
            Command::PublishChannel {
                sid,
                handle,
                is_respawn,
                reply,
            } => {
                let published = match self.sessions.get_mut(&sid) {
                    Some(s) => {
                        s.apply_channel_state(handle, is_respawn);
                        Some((
                            s.channel_generation,
                            s.gen_watch.subscribe(),
                            s.replay_fence,
                        ))
                    }
                    None => None,
                };
                let _ = reply.send(published);
            }
            Command::SpawnFailed { sid, reason } => {
                if let Some(s) = self.sessions.get_mut(&sid) {
                    s.record(Notification::SessionDetached {
                        session_id: sid.clone(),
                        reason,
                    });
                }
            }
            Command::Record {
                sid,
                generation,
                event,
            } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                s.record(Notification::ReplyEvent {
                    session_id: sid.clone(),
                    event,
                });
            }
            Command::TurnCount {
                sid,
                generation,
                turns,
            } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                // A `turns <= replay_fence` signal is the pump telling us replay
                // is complete: clear the fence, no TurnEnded for a replay turn.
                if s.replay_fence > 0 && turns <= s.replay_fence {
                    s.replay_fence = 0;
                    return;
                }
                s.turns = turns;
                let channel_generation = s.channel_generation;
                s.record(Notification::TurnEnded {
                    session_id: sid.clone(),
                    turn_count: turns,
                    generation: channel_generation,
                });
            }
            Command::AgentDisconnected { sid, generation } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                s.record(Notification::SessionDetached {
                    session_id: sid.clone(),
                    reason: "agent disconnected".into(),
                });
                s.channel = None;
            }
        }
    }

    fn do_create(
        &mut self,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let permission_mode = self.default_permission_mode;
        // Open the durable WAL up front so even a crash immediately after create
        // can recover the session's identity.
        let wal = open_session_wal(&id, &label, &cwd, permission_mode);
        let session = new_managed_session(id.clone(), label, cwd.clone(), permission_mode, wal);

        let info = session.info();
        self.sessions.insert(id.clone(), session);
        let _ = self.events.send(Notification::SessionCreated {
            session: info.clone(),
        });

        // Spawn the ACP agent on a background thread (blocking handshake), which
        // posts `PublishChannel` back into the actor when ready.
        let cmd_tx = self.cmd_tx.clone();
        let session_id = id.clone();
        std::thread::Builder::new()
            .name(format!("acp-spawn-{}", &id[..8]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; single-purpose server.
                unsafe {
                    std::env::set_var("SKETCH_SESSION_MANAGED", "1");
                }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    Some(cwd),
                    resume_session_id,
                    SketchFrontend::Gpui,
                ) {
                    Ok(client) => {
                        // Fresh spawn → is_respawn = false, generation stays 0.
                        publish_channel(&cmd_tx, &session_id, client, false);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid: session_id,
                            reason: format!("spawn failed: {e}"),
                        });
                    }
                }
            })
            .ok();

        info
    }

    fn do_close(&mut self, session_id: &str, conn_id: u64) -> Result<(), String> {
        match self.sessions.get(session_id) {
            Some(s) if s.owner == Some(conn_id) => {
                // Removing the session drops its TransportHandle (prompt_tx
                // clone). The owning pump observes the close (inlet still open
                // but no map entry → its generation check / disconnect breaks it)
                // and drops its client off-actor. Bump gen_watch so any owning
                // pump wakes immediately to self-terminate.
                let session = self.sessions.remove(session_id);
                if let Some(s) = &session {
                    let _ = s.gen_watch.send_replace(s.channel_generation.wrapping_add(1));
                }
                // Explicit close = intentional end of life: delete the durable
                // WAL so this session is NOT recovered on the next start.
                if let Some(wal) = session.and_then(|s| s.wal) {
                    wal.remove();
                }
                let _ = self.events.send(Notification::SessionClosed {
                    session_id: session_id.to_string(),
                });
                Ok(())
            }
            Some(_) => Err("only the session owner can close the session".into()),
            None => Err(format!("no such session: {session_id}")),
        }
    }

    fn do_attach(
        &mut self,
        session_id: &str,
        mode: AttachMode,
        conn_id: u64,
        cursor: Option<(u64, u64)>,
    ) -> Result<
        (
            watch::Receiver<bool>,
            watch::Receiver<Arc<Vec<Notification>>>,
            usize,
        ),
        String,
    > {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if mode == AttachMode::Owner {
            match session.owner {
                Some(existing) if existing != conn_id => {
                    return Err("another GUI already owns this session".into());
                }
                _ => {
                    let was_unowned = session.owner.is_none();
                    session.owner = Some(conn_id);
                    if was_unowned {
                        session.broadcast_owner_changed();
                    }
                }
            }
        }
        let owner_rx = session.owner_tx.subscribe();
        let log_rx = session.log_tx.subscribe();
        let log_len = session.event_log.len();

        // Resolve the reconnect cursor into the forwarder's initial `sent`.
        //
        // Incremental tail ONLY when the cursor's generation matches the
        // session's current `channel_generation` AND its index is within the
        // current log. Falls back to `0` ⇒ full replay (exactly today's
        // behavior) for any of:
        //   - no cursor (every client today);
        //   - generation mismatch — a *force-restart* bumped the epoch, so the
        //     client's pre-restart cursor is stale;
        //   - index past the log — WAL compaction-past-cursor or a bogus client.
        // NOTE on server restart: WAL recovery resets `channel_generation` to 0
        // AND restores the full durable log as a faithful append-ordered prefix.
        // So a never-force-restarted client's (gen 0, idx) cursor MATCHES the
        // restored gen 0 and correctly tails the restored log — [0..idx] is
        // exactly what it already saw, [idx..] the right suffix. If un-fsynced
        // mid-turn chunks were lost on crash, idx > log_len trips the
        // full-replay fallback. Safe either way; behavior-preserving.
        let initial_sent = match cursor {
            Some((cursor_gen, idx))
                if cursor_gen == session.channel_generation && (idx as usize) <= log_len =>
            {
                idx as usize
            }
            _ => 0,
        };
        Ok((owner_rx, log_rx, initial_sent))
    }

    fn do_promote(&mut self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        match session.owner {
            None => {
                session.owner = Some(conn_id);
                session.broadcast_owner_changed();
                Ok(())
            }
            Some(existing) if existing == conn_id => Ok(()),
            Some(_) => Err("session is still owned by another GUI".into()),
        }
    }

    fn do_detach(&mut self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner == Some(conn_id) {
            session.owner = None;
            session.broadcast_owner_changed();
        }
        Ok(())
    }

    fn do_prompt(&mut self, session_id: &str, text: &str, conn_id: u64) -> Result<(), String> {
        {
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if session.owner != Some(conn_id) {
                return Err("only the session owner can send prompts".into());
            }
        }
        self.enqueue_prompt(session_id, text)
    }

    /// Owner-gate-free core of the prompt path: log the user's prompt durably,
    /// then hand it to the live channel (or queue it if the agent is still
    /// spawning). Used by both the owner-gated [`do_prompt`] and the ungated
    /// headless [`Command::AdminPrompt`] path (ADR-0015). The ONLY difference
    /// between the two is the owner check; everything that makes a prompt
    /// durable + drives the turn lives here.
    fn enqueue_prompt(&mut self, session_id: &str, text: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        // Log the user's prompt so re-attaching GUIs can replay it (and so it
        // survives a crash — UserPrompt is a turn boundary that fsyncs). Only
        // appended to event_log + WAL, not broadcast.
        session.log_only(Notification::UserPrompt {
            session_id: session_id.to_string(),
            text: text.to_string(),
        });
        match session.channel.as_ref() {
            Some(channel) => channel.send(text).map_err(|e| format!("send failed: {e}")),
            None => {
                session.pending_prompts.push(text.to_string());
                Ok(())
            }
        }
    }

    fn do_cancel(&mut self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner != Some(conn_id) {
            return Err("only the session owner can cancel".into());
        }
        if let Some(channel) = session.channel.as_ref() {
            channel.cancel();
        }
        Ok(())
    }

    fn do_restart(&mut self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let (cwd, resume_id) = {
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if session.owner != Some(conn_id) {
                return Err("only the session owner can restart".into());
            }
            let resume = session.channel.as_ref().and_then(|c| c.session_id());
            (session.cwd.clone(), resume)
        };

        let cmd_tx = self.cmd_tx.clone();
        let sid = session_id.to_string();
        std::thread::Builder::new()
            .name(format!("acp-restart-{}", &sid[..8.min(sid.len())]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; see do_create.
                unsafe {
                    std::env::set_var("SKETCH_SESSION_MANAGED", "1");
                }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    Some(cwd),
                    resume_id,
                    SketchFrontend::Gpui,
                ) {
                    Ok(client) => {
                        // is_respawn=true bumps generation + gen_watch so the OLD
                        // pump self-terminates and drops its client off-actor.
                        publish_channel(&cmd_tx, &sid, client, true);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid,
                            reason: format!("restart failed: {e}"),
                        });
                    }
                }
            })
            .ok();
        Ok(())
    }

    fn do_rename(&mut self, session_id: &str, label: String) -> Result<(), String> {
        {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            session.label = label.clone();
        }
        let _ = self.events.send(Notification::SessionRenamed {
            session_id: session_id.to_string(),
            label,
        });
        Ok(())
    }

    fn do_set_permission_mode(
        &mut self,
        session_id: &str,
        mode: PermissionMode,
        conn_id: u64,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner != Some(conn_id) {
            return Err("only the session owner can change permission mode".into());
        }
        session.permission_mode = mode;
        if let Some(channel) = &session.channel {
            channel.set_permission_mode(mode);
        }
        Ok(())
    }

    fn do_admin_status(&self) -> AdminSnapshot {
        let infos = self
            .sessions
            .values()
            .map(|s| AdminSessionInfo {
                session_id: s.id.clone(),
                label: s.label.clone(),
                connected: s.channel.is_some(),
                has_owner: s.owner.is_some(),
                owner_conn_id: s.owner,
                turns: s.turns,
                event_log_len: s.event_log.len(),
                subscriber_count: s.log_tx.receiver_count(),
                channel_generation: s.channel_generation,
                permission_mode: s.permission_mode,
            })
            .collect();
        AdminSnapshot {
            session_count: self.sessions.len(),
            sessions: infos,
        }
    }
}

// ── Session pump task ──────────────────────────────────────────────

/// Publish a freshly-spawned `AcpChannelClient` as a session's live transport,
/// from a (blocking) spawn worker thread:
///
/// 1. Derive its [`TransportHandle`] (the Send surface the actor stores).
/// 2. Send `PublishChannel` into the actor inlet and BLOCK on the oneshot reply.
///    The actor installs the handle via `apply_channel_state` (drains queued
///    prompts, re-applies permission mode, bumps generation + `gen_watch` on
///    respawn) and replies with (committed generation, gen_watch subscription,
///    replay fence). The actor never holds the client.
/// 3. Spawn the OWNING pump thread with the client moved into it, stamped with
///    that generation and wired to the gen_watch + fence.
///
/// On `is_respawn`, the generation bump wakes any OLD pump (via `gen_watch`) so
/// it self-terminates and drops its own client off the actor task (Blocker A).
/// If the session was closed mid-spawn, the client is dropped here on the
/// worker's OWN thread (its blocking Drop never runs on the actor).
fn publish_channel(
    cmd_tx: &mpsc::UnboundedSender<Command>,
    session_id: &ServerSessionId,
    client: AcpChannelClient,
    is_respawn: bool,
) {
    let handle = client.handle();
    let (reply, rx) = tokio::sync::oneshot::channel();
    if cmd_tx
        .send(Command::PublishChannel {
            sid: session_id.clone(),
            handle,
            is_respawn,
            reply,
        })
        .is_err()
    {
        drop(client); // actor gone — drop the client on this worker thread.
        return;
    }
    // Blocking recv on this OS worker thread — never on the actor task.
    match rx.blocking_recv() {
        Ok(Some((generation, gen_rx, replay_fence))) => {
            spawn_pump_thread(
                cmd_tx.clone(),
                session_id.clone(),
                client,
                generation,
                gen_rx,
                replay_fence,
            );
        }
        Ok(None) | Err(_) => {
            // Session closed while spawning (or actor gone) — drop the client
            // here, on this worker thread (its Drop joins the worker / kills the
            // child; must never run on the actor task).
            drop(client);
        }
    }
}

/// Background thread that OWNS an `AcpChannelClient`, drains its `ReplyEvent`s,
/// and forwards them to the actor inlet as generation-stamped `Command`s.
///
/// Runs on a dedicated OS thread (not a tokio task) because `AcpChannelClient`
/// contains a `std::sync::mpsc::Receiver` which isn't `Sync`. The pump is the
/// SOLE owner of the client: it drops it (running the blocking `Drop`) on its
/// OWN thread when it observes a generation bump (restart) or a closed inlet
/// (close) — never on the actor (Blocker A).
fn spawn_pump_thread(
    cmd_tx: mpsc::UnboundedSender<Command>,
    session_id: ServerSessionId,
    client: AcpChannelClient,
    my_generation: u64,
    gen_rx: watch::Receiver<u64>,
    initial_replay_fence: usize,
) {
    std::thread::Builder::new()
        .name(format!("pump-{}", &session_id[..8.min(session_id.len())]))
        .spawn(move || {
            // Per-session generation watch: a restart (generation bump) wakes us
            // to self-terminate + drop the client off the actor task.
            let gen_rx = gen_rx;

            let mut last_turns: usize = 0;
            // Local mirror of the session's replay fence. Suppression decisions
            // stay pump-side (cycle granularity); the actor only sees Records
            // that should be logged.
            let mut replay_fence: usize = initial_replay_fence;

            const PUMP_IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(16);

            loop {
                // A newer generation means a restart (or close) superseded us —
                // break and drop the client off the actor task.
                if *gen_rx.borrow() > my_generation {
                    break;
                }
                // Inlet closed (manager gone) — terminate.
                if cmd_tx.is_closed() {
                    break;
                }

                // Liveness.
                if !client.is_connected() {
                    let _ = cmd_tx.send(Command::AgentDisconnected {
                        sid: session_id.clone(),
                        generation: my_generation,
                    });
                    break;
                }

                // Drain events up to a budget. If we hit the budget and more
                // events are pending, defer turn-end detection to a later cycle.
                const PUMP_EVENT_BUDGET: usize = 64;
                let mut events = Vec::new();
                while events.len() < PUMP_EVENT_BUDGET {
                    match client.try_recv() {
                        Some(ev) => events.push(ev),
                        None => break,
                    }
                }
                let more_pending = events.len() == PUMP_EVENT_BUDGET
                    && match client.try_recv() {
                        Some(ev) => {
                            events.push(ev);
                            true
                        }
                        None => false,
                    };

                let current_turns = client.turn_count();
                let turn_ended = !more_pending && current_turns > last_turns;

                let tail_events: Vec<sketch::acp_channel::ReplyEvent> = if turn_ended {
                    std::iter::from_fn(|| client.try_recv()).collect()
                } else {
                    Vec::new()
                };

                // ── Replay fence: suppress duplicate events ──────────
                // A restored/resumed session replays prior turns. Drain them
                // (so the channel doesn't back up) but emit no Records until the
                // agent moves past the fence. The fence-clear is signalled to
                // the actor via a TurnCount whose `turns <= replay_fence`.
                if replay_fence > 0 && current_turns <= replay_fence {
                    let drained = !events.is_empty();
                    if turn_ended {
                        last_turns = current_turns;
                        if current_turns == replay_fence {
                            // Replay complete — tell the actor to clear the
                            // session's fence (no TurnEnded for a replay turn).
                            let _ = cmd_tx.send(Command::TurnCount {
                                sid: session_id.clone(),
                                generation: my_generation,
                                turns: current_turns,
                            });
                            replay_fence = 0;
                            tracing::info!(
                                session_id = %&session_id[..8.min(session_id.len())],
                                turn = current_turns,
                                "replay fence cleared"
                            );
                        }
                    }
                    if !drained && !more_pending {
                        std::thread::sleep(PUMP_IDLE_SLEEP);
                    }
                    continue;
                }

                let drained_events = !events.is_empty();

                // Forward events first (in order).
                for ev in events {
                    if std::env::var("SKETCH_CHUNKLOG").is_ok() {
                        if let sketch::acp_channel::ReplyEvent::Chunk(t) = &ev {
                            tracing::info!("[chunklog srv] {t:?}");
                        }
                    }
                    let _ = cmd_tx.send(Command::Record {
                        sid: session_id.clone(),
                        generation: my_generation,
                        event: ev,
                    });
                }

                if turn_ended {
                    // Tail events recorded after budget events, before TurnEnded.
                    for ev in tail_events {
                        let _ = cmd_tx.send(Command::Record {
                            sid: session_id.clone(),
                            generation: my_generation,
                            event: ev,
                        });
                    }
                    last_turns = current_turns;
                    let _ = cmd_tx.send(Command::TurnCount {
                        sid: session_id.clone(),
                        generation: my_generation,
                        turns: current_turns,
                    });
                }

                if !drained_events && !more_pending && !turn_ended {
                    std::thread::sleep(PUMP_IDLE_SLEEP);
                }
            }

            // Drop the client on THIS thread (blocking Drop: kills child +
            // joins worker). Never runs on the actor task (Blocker A).
            drop(client);
        })
        .ok();
}


// ── Connection handler ─────────────────────────────────────────────

/// Handle a single GUI connection on the Unix socket. `conn_id` uniquely
/// identifies this connection so the session manager can track which
/// connection owns each session and gate driving operations accordingly.
async fn handle_connection(stream: UnixStream, manager: Arc<SessionManager>, conn_id: u64) {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    // Track which sessions this connection is subscribed to, so we can
    // clean up on disconnect.
    let mut subscribed: HashMap<ServerSessionId, tokio::task::JoinHandle<()>> = HashMap::new();

    // Manager-level forwarder: pushes session-list changes (create/close/
    // rename) to this GUI so its session list stays consistent with every
    // other connection. Independent of per-session attach state.
    let manager_events = {
        let mut rx = manager.subscribe_events();
        let w = Arc::clone(&writer);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(note) => {
                        let frame = Frame::Notification { note };
                        if let Ok(mut line) = serde_json::to_string(&frame) {
                            line.push('\n');
                            let mut w = w.lock().await;
                            // Same slow-subscriber reaping as the per-session
                            // forwarder: a peer that never drains this fd would
                            // otherwise park this task forever once its kernel
                            // send buffer fills under session-list churn.
                            match tokio::time::timeout(
                                slow_sub_write_timeout(),
                                w.write_all(line.as_bytes()),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => return, // client gone
                                Err(_) => {
                                    tracing::warn!(
                                        "slow subscriber: session-list write stalled \
                                         >{}ms — disconnecting",
                                        slow_sub_write_timeout().as_millis()
                                    );
                                    return;
                                }
                            }
                        }
                    }
                    // Lagged: a few list events were dropped under load. The
                    // GUI reconciles on next open/reconnect, so skip and
                    // continue rather than tearing down the forwarder.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    };

    let started = std::time::Instant::now();
    let mut lines = reader.lines();

    // Read loop. Captures WHY it exits so the teardown log can name the cause
    // of a disconnect — the reconnect-storm diagnostic. Distinguishes client
    // EOF vs socket read error vs a failed response write (client already gone).
    let exit_reason: String = loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break "client closed connection (EOF)".to_string(),
            Err(e) => break format!("socket read error: {e}"),
        };
        let frame: Frame = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(error = %e, "bad frame");
                continue;
            }
        };

        let Frame::Request { id, req } = frame else {
            continue;
        };

        let response = match req {
            Request::Ping => Response::Ok {
                data: ResponseData::Pong,
            },

            Request::ListSessions => {
                let sessions = manager.send_list_sessions().await;
                Response::Ok {
                    data: ResponseData::Sessions { sessions },
                }
            }

            Request::CreateSession {
                cwd,
                label,
                resume_session_id,
            } => {
                let info = manager.send_create(cwd, label, resume_session_id).await;
                Response::Ok {
                    data: ResponseData::Session { session: info },
                }
            }

            Request::Attach {
                session_id,
                mode,
                cursor,
            } => {
                match manager.send_attach(&session_id, mode, conn_id, cursor).await {
                    Ok((owner_rx, log_rx, initial_sent)) => {
                        // `initial_sent` is the actor-resolved tail start (see
                        // `do_attach`): 0 for a full replay (no/stale cursor —
                        // the forwarder tails `event_log` from index 0, the
                        // unchanged behavior), or the cursor index for an
                        // incremental reconnect that streams only `[idx..]`.
                        // Either way history + live events flow over one
                        // ordered, gap-free path.
                        tracing::info!(
                            session_id = %&session_id[..8],
                            initial_sent,
                            cursor = ?cursor,
                            "attach: forwarder tail start resolved"
                        );
                        let w = Arc::clone(&writer);
                        let handle = tokio::spawn(forward_notifications(
                            Arc::clone(&manager),
                            session_id.clone(),
                            w,
                            owner_rx,
                            log_rx,
                            initial_sent,
                        ));
                        subscribed.insert(session_id, handle);
                        Response::Ok {
                            data: ResponseData::Ack,
                        }
                    }
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Detach { session_id } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_detach(&session_id, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Promote { session_id } => {
                match manager.send_promote(&session_id, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Prompt { session_id, text } => {
                match manager.send_prompt(&session_id, &text, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::AdminPrompt { session_id, text } => {
                match manager.send_admin_prompt(&session_id, &text).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Cancel { session_id } => {
                match manager.send_cancel(&session_id, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::RestartSession { session_id } => {
                match manager.send_restart(&session_id, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetPermissionMode { session_id, mode } => {
                match manager.send_set_permission_mode(&session_id, mode, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::CloseSession { session_id } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_close(&session_id, conn_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::RenameSession { session_id, label } => {
                match manager.send_rename(&session_id, label).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::AdminStatus => Response::Ok {
                data: ResponseData::AdminStatus {
                    snapshot: manager.send_admin_status().await,
                },
            },
        };

        let resp_frame = Frame::Response {
            id,
            result: response,
        };
        let mut line = serde_json::to_string(&resp_frame).unwrap();
        line.push('\n');
        let mut w = writer.lock().await;
        // Bound the reply write too: a client that issued a request but stopped
        // draining its socket must not park this read loop forever.
        match tokio::time::timeout(slow_sub_write_timeout(), w.write_all(line.as_bytes())).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break "response write failed (client gone)".to_string(),
            Err(_) => break "response write stalled (slow client)".to_string(),
        }
    };

    tracing::info!(
        conn_id,
        attached = subscribed.len(),
        "conn {conn_id} closed after {:.1}s — {exit_reason}; was attached to {} session(s)",
        started.elapsed().as_secs_f64(),
        subscribed.len(),
    );

    // Connection closed — detach all sessions and cancel forwarders. This
    // releases ownership of any sessions this connection owned, which
    // broadcasts OwnerChanged so an observing candidate GUI can promote.
    for (sid, handle) in &subscribed {
        handle.abort();
        let _ = manager.send_detach(sid, conn_id).await;
    }
    manager_events.abort();
}

/// Forward a session's notifications to one GUI connection's writer.
///
/// **Source of truth is `event_log`, not the broadcast.** The broadcast
/// channel is used only as a wake signal: on any wake (including a `Lagged`
/// overflow) we re-read `event_log[sent..]` and forward whatever the client
/// hasn't seen. This makes a slow/lagging subscriber *self-healing* — it can
/// never permanently lose transcript content the way the old "forward the
/// broadcast payload and drop on Lagged" path did (that was the source of the
/// `fingerLet`-style merge artifacts). The first tail pass (`sent == 0`) also
/// subsumes the attach-time replay, so history and live stream share one
/// ordered path with no replay/live seam.
async fn forward_notifications(
    _manager: Arc<SessionManager>,
    session_id: ServerSessionId,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    mut owner_rx: watch::Receiver<bool>,
    mut log_rx: watch::Receiver<Arc<Vec<Notification>>>,
    initial_sent: usize,
) {
    // Number of `event_log` entries already written to this client. Starts at
    // the actor-resolved `initial_sent`: `0` for a full replay (no/stale
    // cursor — unchanged behavior), or the reconnect cursor index so the first
    // tail pass streams only `[initial_sent..]` (incremental reconnect, spec
    // phase 5). Everything below (tail loop, owner watch, slow-subscriber
    // timeout) is identical regardless.
    let mut sent = initial_sent;

    // First pass: `watch::Sender::subscribe()` marks the current value as
    // already-seen, so the initial transcript replay IS the first tail. Mark
    // the current snapshot seen with `borrow_and_update()` and tail it from
    // `sent` (the cursor index, or 0 for full replay) to subsume attach replay
    // (no separate replay path).
    {
        let snapshot = log_rx.borrow_and_update().clone();
        if snapshot.len() > sent {
            if !flush_tail(&writer, &session_id, &snapshot[sent..]).await {
                return;
            }
            sent = snapshot.len();
        }
    }

    // Once the control (OwnerChanged) channel closes, stop selecting on it so
    // a closed broadcast doesn't busy-loop; keep serving the transcript log.
    let mut owner_open = true;

    loop {
        tokio::select! {
            // Transcript log channel: a new snapshot was published. Tail the
            // latest snapshot lock-free from the cloned `Arc` — no manager lock
            // in the hot path. Coalesced wakes self-heal: we always tail
            // [sent..] of whatever the latest snapshot is.
            changed = log_rx.changed() => {
                match changed {
                    Ok(()) => {
                        let snapshot = log_rx.borrow_and_update().clone();
                        if snapshot.len() > sent {
                            if !flush_tail(&writer, &session_id, &snapshot[sent..]).await {
                                return;
                            }
                            sent = snapshot.len();
                        }
                    }
                    Err(_) => {
                        // Sender dropped (session closing). One final tail of
                        // the last snapshot, then exit.
                        let snapshot = log_rx.borrow().clone();
                        if snapshot.len() > sent {
                            let _ = flush_tail(&writer, &session_id, &snapshot[sent..]).await;
                        }
                        return;
                    }
                }
            }

            // Control channel: ownership state (watch<bool>). On change,
            // synthesize a single `OwnerChanged` control note and forward it —
            // the only control note, never logged.
            changed = owner_rx.changed(), if owner_open => {
                match changed {
                    Ok(()) => {
                        let has_owner = *owner_rx.borrow_and_update();
                        let frame = Frame::Notification {
                            note: Notification::OwnerChanged {
                                session_id: session_id.clone(),
                                has_owner,
                            },
                        };
                        if let Ok(mut line) = serde_json::to_string(&frame) {
                            line.push('\n');
                            let dur = slow_sub_write_timeout();
                            let mut w = writer.lock().await;
                            match tokio::time::timeout(dur, w.write_all(line.as_bytes())).await {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => return,
                                Err(_) => {
                                    tracing::warn!(
                                        session_id = %&session_id[..8.min(session_id.len())],
                                        "slow subscriber: OwnerChanged write stalled >{}ms — disconnecting",
                                        dur.as_millis()
                                    );
                                    return;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Control channel closed: keep serving the transcript
                        // log channel until IT closes.
                        owner_open = false;
                    }
                }
            }
        }
    }
}

/// Per-write timeout for forwarder socket writes. A subscriber whose socket
/// stops draining (dead/stuck peer) would otherwise make `write_all` block
/// indefinitely, parking the forwarder task + its fd forever. We bound every
/// forwarder write by this duration; on elapse we drop the subscriber (its
/// write half closes → the client sees EOF and cleanly reconnects, replaying
/// from the watch snapshot, so no events are lost).
///
/// Default is GENEROUS (60s) so a healthy slow-but-progressing client is never
/// falsely reaped. Override via `SKETCH_SLOW_SUB_TIMEOUT_MS` (u64 ms); `0` or
/// unset → the 60s default.
fn slow_sub_write_timeout() -> std::time::Duration {
    // Resolved once per process (env can't change mid-run) so the hot
    // streaming write path doesn't lock + parse the env on every write.
    static TIMEOUT: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        const DEFAULT_MS: u64 = 60_000;
        let ms = std::env::var("SKETCH_SLOW_SUB_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .unwrap_or(DEFAULT_MS);
        std::time::Duration::from_millis(ms)
    })
}

/// Serialize and write a tail slice of notifications in one buffered write.
/// Returns `false` if the write failed (client gone) or stalled past the
/// slow-subscriber timeout (non-draining peer), in which case the caller drops
/// the forwarder.
async fn flush_tail(
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    session_id: &str,
    tail: &[Notification],
) -> bool {
    let mut buf = String::new();
    for note in tail {
        let frame = Frame::Notification { note: note.clone() };
        if let Ok(line) = serde_json::to_string(&frame) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    let dur = slow_sub_write_timeout();
    let mut w = writer.lock().await;
    match tokio::time::timeout(dur, w.write_all(buf.as_bytes())).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            // Socket error — client gone.
            tracing::warn!(
                session_id = %&session_id[..8.min(session_id.len())],
                this_pass = tail.len(),
                "forwarder write failed — client gone"
            );
            false
        }
        Err(_) => {
            // Elapsed — the peer's socket buffer is full and not draining.
            tracing::warn!(
                session_id = %&session_id[..8.min(session_id.len())],
                this_pass = tail.len(),
                "slow subscriber: write stalled >{}ms — disconnecting",
                dur.as_millis()
            );
            false
        }
    }
}

// ── Main ─────────────────────────────────────────���─────────────────

#[tokio::main]
async fn main() -> io::Result<()> {
    // Structured logging FIRST, before any other work. Route to STDERR (the
    // launchd/test harness captures the server's stderr to a log file and greps
    // it), with ANSI colors off so the log file stays clean grep-able text.
    // Defaults to "info" when RUST_LOG is unset.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    use clap::Parser;
    // Subcommands manage launchd supervision and exit; no subcommand = run the
    // server (the default path the GUI auto-launches).
    if let Some(command) = Cli::parse().command {
        return match command {
            Subcmd::Install => launchd::install(),
            Subcmd::Uninstall => launchd::uninstall(),
            Subcmd::Status => launchd::status(),
            Subcmd::Prompt { session_id, text } => {
                // Headless start-work (ADR-0015). Connect to an ALREADY-RUNNING
                // server (never auto-launch a throwaway daemon — a CLI prompt
                // targets a session in a live server), then enqueue via the
                // ungated admin path. Print ok/error and exit.
                let client = match sketch::session_client::SessionServerClient::connect_existing() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "error: could not connect to a running session server ({e}). \
                             Start one with `sketch-session-server` (or `sketch-session-server install`)."
                        );
                        std::process::exit(1);
                    }
                };
                match client.admin_prompt(&session_id, &text) {
                    Ok(()) => {
                        println!("ok");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        };
    }

    let socket_path = socket_path();
    let pid_path = pid_file_path();

    // Single-instance guard. If a server is ALREADY listening on this socket,
    // exit cleanly instead of removing the socket and re-binding — which would
    // silently steal it from the live server and orphan every session that
    // server is running. The client auto-launches a server on any failed
    // connect, so spurious concurrent launches genuinely happen; this makes
    // them harmless (the loser exits, the client's connect-retry finds the
    // winner). A socket file that exists but is NOT connectable is stale (a
    // prior crash left it behind), so we clear it and take over.
    if socket_path.exists() {
        if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
            tracing::warn!(
                "another server already listening on {} — exiting",
                socket_path.display()
            );
            return Ok(());
        }
        let _ = std::fs::remove_file(&socket_path);
    }

    // Owner-only socket: nobody else on the box can connect to (and drive)
    // our agent sessions. The mode must be tight from the instant the inode
    // exists — `bind()` starts queueing `connect()`s immediately, so a
    // chmod-after-bind leaves a TOCTOU window where a same-host attacker can
    // slip into the backlog. Clamp the umask around the bind so the socket is
    // created 0600 atomically; the explicit set_permissions is a belt-and-
    // suspenders assertion (and covers any platform that ignores umask on
    // socket inodes). We are single-threaded here (pre-accept-loop), so the
    // process-global umask flip is safe to restore right after.
    let prev_umask = unsafe { libc::umask(0o177) };
    let bind_result = UnixListener::bind(&socket_path);
    unsafe { libc::umask(prev_umask) };
    let listener = bind_result?;
    let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));

    // Write PID file.
    let _ = std::fs::write(&pid_path, std::process::id().to_string());

    tracing::info!("listening on {}", socket_path.display());

    // Load the config once at startup to pick up the user's default permission
    // mode. Config::load() is a plain lib fn (no GUI deps) and returns the
    // Default config when no file is present, so this is safe in the headless
    // server. Any parse error degrades to the hard-coded default rather than
    // refusing to start.
    let config = sketch::config::Config::load().unwrap_or_default();
    let default_permission_mode = config.default_permission_mode;
    tracing::info!(
        default_permission_mode = config.default_permission_mode.short_label(),
        "loaded config"
    );

    let (mgr, cmd_rx, default_permission_mode) =
        SessionManager::new_with_inlet(default_permission_mode);
    let manager = Arc::new(mgr);

    // Recover sessions from a prior run BEFORE the actor starts (recovery must
    // precede the accept loop). The seed map is moved into the actor; the resume
    // jobs spawn workers that re-spawn ACP subprocesses and post `PublishChannel`
    // back into the actor once it's running.
    let (seed_sessions, resume_jobs) = restore_seed_from_disk();

    // Spawn the single-writer manager actor: it OWNS the sessions map and drains
    // the inlet (external requests, spawn-worker publishes, pump-sourced records)
    // one command at a time.
    tokio::spawn(run_manager(
        cmd_rx,
        seed_sessions,
        manager.events.clone(),
        default_permission_mode,
        manager.cmd_tx.clone(),
    ));

    // Now the actor is running, kick off the resume workers.
    for job in resume_jobs {
        spawn_resume_worker(manager.cmd_tx.clone(), job);
    }

    // Handle graceful shutdown — persist sessions before exiting.
    // Listen for both SIGINT (Ctrl-C) and SIGTERM (kill / process manager).
    let mgr_shutdown = Arc::clone(&manager);
    let socket_path_cleanup = socket_path.clone();
    let pid_path_cleanup = pid_path.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
        }
        // No explicit persist needed: each session's durable WAL is written
        // continuously (ADR-0009), so sessions already survive this shutdown
        // (and a crash). Just clean up the socket + pid so the next start is
        // tidy; the WAL dir is intentionally left for recovery.
        let _ = &mgr_shutdown;
        tracing::info!("shutting down (WALs are durable)");
        let _ = std::fs::remove_file(&socket_path_cleanup);
        let _ = std::fs::remove_file(&pid_path_cleanup);
        std::process::exit(0);
    });

    // Monotonic connection id — identifies which connection owns a session.
    let next_conn_id = std::sync::atomic::AtomicU64::new(1);

    loop {
        let (stream, _) = listener.accept().await?;
        let mgr = Arc::clone(&manager);
        let conn_id = next_conn_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Every GUI relaunch is a fresh accept (no persistent client identity),
        // so a "reconnect" surfaces here as conn_id > 1 and/or pre-existing
        // sessions — the session count is what tells you the client rejoined
        // live state rather than starting cold.
        let active_sessions = manager.send_session_count().await;
        if conn_id == 1 {
            tracing::info!(
                conn_id,
                active_sessions,
                "client connected (conn {conn_id}); {active_sessions} active session(s)"
            );
        } else {
            tracing::info!(
                conn_id,
                active_sessions,
                "client reconnected (conn {conn_id}); {active_sessions} active session(s)"
            );
        }
        tokio::spawn(handle_connection(stream, mgr, conn_id));
    }
}
