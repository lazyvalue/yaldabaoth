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
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use sketch::acp_channel::{
    AcpChannelClient, PermissionMode, SketchFrontend, DEFAULT_PERMISSION_MODE,
};
use sketch::session_proto::*;

mod launchd;

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
}

// ── Managed session ────────────────────────────────────────────────

struct ManagedSession {
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    /// The live ACP channel. `None` while the subprocess is being spawned.
    channel: Option<AcpChannelClient>,
    /// Bumped every time `channel` is replaced (force-restart). The pump
    /// thread watches this and resets its `last_turns` baseline so the fresh
    /// channel's turn counter (which restarts at 0) is tracked correctly.
    channel_generation: u64,
    turns: usize,
    permission_mode: PermissionMode,
    /// Broadcast sender — attached GUI connections subscribe here. Any number
    /// of connections (one owner + N observers) may subscribe concurrently.
    event_tx: broadcast::Sender<Notification>,
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
        Arc::make_mut(&mut self.event_log).push(note.clone());
        self.wal_append(&note);
        let _ = self.event_tx.send(note);
    }

    /// Append a transcript event to `event_log` durably WITHOUT broadcasting.
    /// Used for the user's own prompt, which the live GUI already rendered
    /// locally (so it must be logged for replay but not re-broadcast). The
    /// broadcast path goes through [`record`].
    fn log_only(&mut self, note: Notification) {
        Arc::make_mut(&mut self.event_log).push(note.clone());
        self.wal_append(&note);
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
        let _ = self.event_tx.send(Notification::OwnerChanged {
            session_id: self.id.clone(),
            has_owner: self.owner.is_some(),
        });
    }

    /// Publish a freshly-spawned `channel` as this session's live channel,
    /// running the full attach choreography atomically under the caller's
    /// lock. The single chokepoint for create / restore / restart (9′) so the
    /// three can't drift:
    /// 1. Re-apply the session's `permission_mode` (a fresh channel starts at
    ///    its default — without this the configured mode silently reverts).
    /// 2. Drain `pending_prompts` in arrival order onto the new channel BEFORE
    ///    publishing it, so they're enqueued at the ACP driver before any
    ///    future prompt races in. Doing this under the lock also closes the
    ///    take-then-publish window where a concurrent `prompt()` could re-queue
    ///    onto a `pending_prompts` we'd already drained.
    /// 3. On a respawn (force-restart), bump `channel_generation` so the pump
    ///    rebaselines its `last_turns` against the new channel's zeroed counter.
    /// 4. Swap the channel in and `record(SessionAttached)`.
    ///
    /// Returns the OLD channel (if any) WITHOUT dropping it — the caller must
    /// drop it AFTER releasing the sessions lock, because `AcpChannelClient`'s
    /// `Drop` joins the worker thread / kills the child and must never run
    /// while the global mutex is held.
    #[must_use = "drop the returned old channel after releasing the lock"]
    fn apply_channel_state(
        &mut self,
        mut channel: AcpChannelClient,
        is_respawn: bool,
    ) -> Option<AcpChannelClient> {
        channel.set_permission_mode(self.permission_mode);
        for text in std::mem::take(&mut self.pending_prompts) {
            if let Err(e) = channel.send(&text) {
                tracing::error!(error = %e, "failed to flush queued prompt");
            }
        }
        let acp_session_id = channel.session_id();
        if is_respawn {
            self.channel_generation = self.channel_generation.wrapping_add(1);
        }
        let old = self.channel.replace(channel);
        self.record(Notification::SessionAttached {
            session_id: self.id.clone(),
            acp_session_id,
        });
        old
    }
}

// ── Session manager ────────────────────────────────────────────────

struct SessionManager {
    sessions: Mutex<HashMap<ServerSessionId, ManagedSession>>,
    /// Manager-level broadcast for session-list changes (create/close/rename).
    /// Distinct from each session's `event_tx`: those carry one session's
    /// transcript to its subscribers, whereas this reaches **every** connected
    /// GUI so closures/renames done in one panel or one GUI instance propagate
    /// everywhere and the session lists stay consistent without polling.
    events: broadcast::Sender<Notification>,
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
    fn new() -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            sessions: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// THE single accessor for the shared sessions map — poison-tolerant.
    ///
    /// If any thread panics *while holding* the lock, a plain `.lock().unwrap()`
    /// at every other site would then panic too, so one stray panic cascades
    /// into "every session is dead" (the failure mode this centralization
    /// closes). The guarded data is a plain `HashMap` mutated by short,
    /// non-compound critical sections — there is no half-applied multi-step
    /// invariant that a mid-mutation panic could leave torn — so recovering the
    /// guard via `into_inner()` keeps the surviving sessions serving instead of
    /// taking the whole server down. The recovery is surfaced (once per poison
    /// observation) rather than swallowed, so the originating panic stays
    /// visible. Every `sessions` access MUST go through here.
    fn lock_sessions(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ServerSessionId, ManagedSession>> {
        self.sessions.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                "sessions mutex was poisoned by a prior panic; \
                 recovering the guard to keep other sessions alive"
            );
            poisoned.into_inner()
        })
    }

    /// Subscribe to manager-level session-list notifications. Each connection
    /// calls this once and forwards everything it receives to its GUI.
    fn subscribe_events(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    /// Recover sessions from their durable WALs (ADR-0009) and re-spawn their
    /// ACP subprocesses. Called once at startup before accepting connections.
    /// Replaces the old clean-shutdown-only JSON snapshot: the WAL is written
    /// continuously, so recovery survives a CRASH, not just a graceful exit.
    /// Idempotent — unlike the old delete-on-restore hack, the WAL files are
    /// kept (a session's WAL is removed only when it is explicitly closed), so a
    /// crash mid-recovery just replays again next boot.
    fn restore_from_disk(self: &Arc<Self>) {
        let Some(dir) = session_wal_dir() else {
            return;
        };
        let recovered = sketch::session_wal::recover_all(&dir);
        for rs in recovered {
            let sid = rs.server_session_id.clone();
            // Without an acp_session_id we can't --resume the agent; the
            // transcript is preserved but the session is inert. Drop it (and its
            // WAL) rather than leaving a zombie that can never make progress.
            let Some(acp_session_id) = rs.acp_session_id.clone() else {
                tracing::warn!(
                    session_id = %&sid[..8.min(sid.len())],
                    "discarding recovered session: no acp_session_id to resume"
                );
                let _ = std::fs::remove_file(&rs.path);
                continue;
            };

            let (event_tx, _) = broadcast::channel(16384);
            // Re-open the WAL in append mode so the restored session keeps
            // logging to the same file. If reopen fails, degrade to
            // in-memory-only (still better than dropping the session).
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

            let session = ManagedSession {
                id: sid.clone(),
                label: rs.label.clone(),
                cwd: rs.cwd.clone(),
                channel: None,
                channel_generation: 0,
                turns: rs.turns,
                permission_mode: rs.permission_mode,
                event_tx: event_tx.clone(),
                owner: None,
                pending_prompts: Vec::new(),
                event_log: Arc::new(rs.event_log),
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

            self.lock_sessions().insert(sid.clone(), session);

            // Re-spawn the ACP subprocess with --resume.
            let manager = Arc::clone(self);
            let session_id = sid.clone();
            let cwd = rs.cwd.clone();
            std::thread::Builder::new()
                .name(format!("acp-resume-{}", &session_id[..8.min(session_id.len())]))
                .spawn(move || {
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
                            // Publish via the shared choreography (9′). Resume
                            // from disk → is_respawn=false (generation stays 0).
                            let old = {
                                let mut sessions = manager.lock_sessions();
                                match sessions.get_mut(&session_id) {
                                    Some(s) => s.apply_channel_state(client, false),
                                    None => Some(client),
                                }
                            };
                            drop(old);
                            spawn_pump_thread(Arc::clone(&manager), session_id);
                        }
                        Err(e) => {
                            tracing::error!(
                                session_id = %&session_id[..8.min(session_id.len())],
                                error = %e,
                                "failed to resume session"
                            );
                            let mut sessions = manager.lock_sessions();
                            if let Some(s) = sessions.get_mut(&session_id) {
                                s.record(Notification::SessionDetached {
                                    session_id: session_id.clone(),
                                    reason: format!("resume failed: {e}"),
                                });
                            }
                        }
                    }
                })
                .ok();
        }
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.lock_sessions();
        sessions.values().map(|s| s.info()).collect()
    }

    /// Diagnostic snapshot of every managed session's live server-side state.
    /// Read-only — exposes internals (owner conn id, broadcast receiver count,
    /// channel generation) for observability without affecting any session.
    fn admin_status(&self) -> AdminSnapshot {
        let sessions = self.lock_sessions();
        let infos = sessions
            .values()
            .map(|s| AdminSessionInfo {
                session_id: s.id.clone(),
                label: s.label.clone(),
                connected: s.channel.is_some(),
                has_owner: s.owner.is_some(),
                owner_conn_id: s.owner,
                turns: s.turns,
                event_log_len: s.event_log.len(),
                subscriber_count: s.event_tx.receiver_count(),
                channel_generation: s.channel_generation,
                permission_mode: s.permission_mode,
            })
            .collect();
        AdminSnapshot {
            session_count: sessions.len(),
            sessions: infos,
        }
    }

    fn create_session(
        self: &Arc<Self>,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let (event_tx, _) = broadcast::channel(16384);
        let permission_mode = DEFAULT_PERMISSION_MODE;

        // Open the durable WAL up front and write its header, so even a crash
        // immediately after create can recover the session's identity. Degrade
        // to in-memory-only if it can't be opened.
        let wal = open_session_wal(&id, &label, &cwd, permission_mode);

        let session = ManagedSession {
            id: id.clone(),
            label,
            cwd: cwd.clone(),
            channel: None,
            channel_generation: 0,
            turns: 0,
            permission_mode,
            event_tx: event_tx.clone(),
            owner: None,
            pending_prompts: Vec::new(),
            event_log: Arc::new(Vec::new()),
            replay_fence: 0,
            wal,
        };

        let info = session.info();
        self.lock_sessions().insert(id.clone(), session);
        let _ = self.events.send(Notification::SessionCreated {
            session: info.clone(),
        });

        // Spawn the ACP agent on a background thread (blocking handshake).
        let manager = Arc::clone(self);
        let session_id = id.clone();
        std::thread::Builder::new()
            .name(format!("acp-spawn-{}", &id[..8]))
            .spawn(move || {
                // Tell the agent subprocess it's running under the session
                // server (not a direct GUI spawn). Agents can check
                // `SKETCH_SESSION_MANAGED=1` to branch on client/server mode.
                // Safe to set process-wide: every agent this server spawns
                // is server-managed by definition.
                // SAFETY: this runs on a dedicated spawn thread; the session
                // server is single-purpose and no other thread reads this var.
                unsafe { std::env::set_var("SKETCH_SESSION_MANAGED", "1"); }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    Some(cwd),
                    resume_session_id,
                    SketchFrontend::Gpui,
                ) {
                    Ok(client) => {
                        // Publish the channel + drain queued prompts atomically
                        // (9′, `apply_channel_state`). Fresh spawn → is_respawn
                        // = false, generation stays 0.
                        let old = {
                            let mut sessions = manager.lock_sessions();
                            match sessions.get_mut(&session_id) {
                                Some(s) => s.apply_channel_state(client, false),
                                // Session closed while we were spawning — return
                                // the orphan so it Drops after the lock releases.
                                None => Some(client),
                            }
                        };
                        drop(old);
                        // Start the pump thread now that the channel is live.
                        spawn_pump_thread(Arc::clone(&manager), session_id);
                    }
                    Err(e) => {
                        let mut sessions = manager.lock_sessions();
                        if let Some(s) = sessions.get_mut(&session_id) {
                            s.record(Notification::SessionDetached {
                                session_id: session_id.clone(),
                                reason: format!("spawn failed: {e}"),
                            });
                        }
                    }
                }
            })
            .ok();

        info
    }

    fn close_session(&self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let removed = {
            let mut sessions = self.lock_sessions();
            match sessions.get(session_id) {
                Some(s) if s.owner == Some(conn_id) => {
                    // Dropping ManagedSession drops AcpChannelClient → kills subprocess.
                    let session = sessions.remove(session_id);
                    // Explicit close = intentional end of life: delete the
                    // durable WAL so this session is NOT recovered on the next
                    // start. (Crash/disconnect leave the WAL; only an explicit
                    // close removes it.)
                    if let Some(wal) = session.and_then(|s| s.wal) {
                        wal.remove();
                    }
                    true
                }
                Some(_) => return Err("only the session owner can close the session".into()),
                None => return Err(format!("no such session: {session_id}")),
            }
        };
        if removed {
            // Tell every connection to drop this session from its list — the
            // owner that closed it *and* any observer panels / other GUIs.
            let _ = self.events.send(Notification::SessionClosed {
                session_id: session_id.to_string(),
            });
        }
        Ok(())
    }

    /// Attach a connection to a session. `Owner` mode requires the session
    /// to have no current owner (or already be owned by this connection);
    /// `Observer` mode always succeeds and never touches ownership. Returns
    /// the live broadcast receiver plus the current event-log length (the
    /// forwarder re-derives the actual transcript by tailing `event_log` from
    /// index 0, so the bytes never need to leave the lock — only the count, for
    /// the attach log line). Subscribing while holding the lock keeps the
    /// replay/live-subscription seam gap-free.
    fn attach(
        &self,
        session_id: &str,
        mode: AttachMode,
        conn_id: u64,
    ) -> Result<(broadcast::Receiver<Notification>, usize), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
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
        let rx = session.event_tx.subscribe();
        let log_len = session.event_log.len();
        Ok((rx, log_len))
    }

    /// An observer claims ownership of a currently-unowned session.
    fn promote(&self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
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

    /// Detach a connection. If it was the owner, ownership is released and an
    /// `OwnerChanged` is broadcast so observers can promote. Observer detach
    /// is a no-op on ownership (the dropped receiver cleans itself up).
    fn detach(&self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner == Some(conn_id) {
            session.owner = None;
            session.broadcast_owner_changed();
        }
        Ok(())
    }

    fn prompt(&self, session_id: &str, text: &str, conn_id: u64) -> Result<(), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner != Some(conn_id) {
            return Err("only the session owner can send prompts".into());
        }

        // Log the user's prompt so re-attaching GUIs can replay it (and so it
        // survives a crash — UserPrompt is a turn boundary that fsyncs). Only
        // appended to event_log + WAL, not broadcast — the live GUI already
        // inserted the text locally in submit_chatbox before calling prompt().
        // Broadcasting would duplicate it.
        session.log_only(Notification::UserPrompt {
            session_id: session_id.to_string(),
            text: text.to_string(),
        });

        // If the ACP subprocess is still spawning, queue the prompt; the
        // create-session worker drains `pending_prompts` and forwards them
        // as soon as the channel is live. Otherwise send straight through.
        match session.channel.as_mut() {
            Some(channel) => channel
                .send(text)
                .map_err(|e| format!("send failed: {e}")),
            None => {
                session.pending_prompts.push(text.to_string());
                Ok(())
            }
        }
    }

    /// Interrupt the in-flight turn. Owner-only, like `prompt`. Best-effort:
    /// if the ACP subprocess is still spawning (no channel yet) there is
    /// nothing in flight to cancel, so it's a no-op.
    fn cancel(&self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
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

    /// Hard recovery: kill the agent subprocess and respawn it, resuming the
    /// same ACP session so prior context survives. The escalation when a
    /// graceful `cancel` won't unstick a turn wedged on a hung upstream
    /// request. The new channel is spawned off-thread (blocking handshake)
    /// and swapped in atomically once ready, so the pump never observes a
    /// gap and the old (wedged) subprocess stays up until the replacement is
    /// live. Owner-only.
    fn restart_session(self: &Arc<Self>, session_id: &str, conn_id: u64) -> Result<(), String> {
        let (cwd, resume_id) = {
            let sessions = self.lock_sessions();
            let session = sessions
                .get(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if session.owner != Some(conn_id) {
                return Err("only the session owner can restart".into());
            }
            let resume = session.channel.as_ref().and_then(|c| c.session_id());
            (session.cwd.clone(), resume)
        };

        let manager = Arc::clone(self);
        let sid = session_id.to_string();
        std::thread::Builder::new()
            .name(format!("acp-restart-{}", &sid[..8.min(sid.len())]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; see create_session.
                unsafe { std::env::set_var("SKETCH_SESSION_MANAGED", "1"); }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    Some(cwd),
                    resume_id,
                    SketchFrontend::Gpui,
                ) {
                    Ok(client) => {
                        // Swap via the shared choreography (9′): re-apply the
                        // permission policy, drain queued prompts (the fix —
                        // restart previously dropped prompts queued mid-restart),
                        // and bump generation (is_respawn=true). The pump never
                        // sees a None channel (swap is under the lock); the OLD
                        // channel is dropped AFTER releasing the lock — its Drop
                        // joins the worker / kills the child and must not run
                        // under the mutex.
                        let old = {
                            let mut sessions = manager.lock_sessions();
                            match sessions.get_mut(&sid) {
                                Some(s) => s.apply_channel_state(client, true),
                                None => Some(client),
                            }
                        };
                        drop(old);
                    }
                    Err(e) => {
                        let mut sessions = manager.lock_sessions();
                        if let Some(s) = sessions.get_mut(&sid) {
                            s.record(Notification::SessionDetached {
                                session_id: sid.clone(),
                                reason: format!("restart failed: {e}"),
                            });
                        }
                    }
                }
            })
            .ok();
        Ok(())
    }

    fn rename_session(&self, session_id: &str, label: String) -> Result<(), String> {
        {
            let mut sessions = self.lock_sessions();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            session.label = label.clone();
        }
        // Propagate the new label to every panel and every GUI instance.
        let _ = self.events.send(Notification::SessionRenamed {
            session_id: session_id.to_string(),
            label,
        });
        Ok(())
    }

    fn set_permission_mode(
        &self,
        session_id: &str,
        mode: PermissionMode,
        conn_id: u64,
    ) -> Result<(), String> {
        let mut sessions = self.lock_sessions();
        let session = sessions
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
}

// ── Session pump task ──────────────────────────────────────────────

/// Background thread that drains `ReplyEvent`s from an `AcpChannelClient`
/// and broadcasts them as `Notification`s to attached GUIs.
///
/// Runs on a dedicated OS thread (not a tokio task) because
/// `AcpChannelClient` contains `std::sync::mpsc::Receiver` which isn't
/// `Sync`, and holding a `std::sync::Mutex` guard across `.try_recv()`
/// is fine on a real thread but problematic across `.await` points.
fn spawn_pump_thread(manager: Arc<SessionManager>, session_id: ServerSessionId) {
    std::thread::Builder::new()
        .name(format!("pump-{}", &session_id[..8]))
        .spawn(move || {
            let mut last_turns: usize = 0;
            let mut synced_gen: u64 = 0;
            // Drain-then-sleep: only park the thread when a full cycle
            // produced no work, so the first token of a turn isn't held
            // for a fixed tick. When a cycle drains events (or more remain
            // queued past our budget) we loop immediately. Mirrors the GUI
            // pump's `more_pending` fast-loop.
            const PUMP_IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(16);

            loop {
                let mut sessions = manager.lock_sessions();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return; // Session was closed.
                };
                // A force-restart swapped in a fresh channel whose turn
                // counter restarts at 0 — rebaseline so we don't wait for it
                // to climb past the old channel's count before firing
                // turn-end again.
                if session.channel_generation != synced_gen {
                    synced_gen = session.channel_generation;
                    last_turns = 0;
                }
                let Some(channel) = &session.channel else {
                    drop(sessions);
                    std::thread::sleep(PUMP_IDLE_SLEEP); // Not yet spawned — idle.
                    continue;
                };

                // Check liveness.
                if !channel.is_connected() {
                    session.record(Notification::SessionDetached {
                        session_id: session_id.clone(),
                        reason: "agent disconnected".into(),
                    });
                    session.channel = None;
                    return;
                }

                // Drain events up to a budget. If we hit the budget and
                // more events are pending, defer turn-end detection to a
                // later cycle so we don't fire it before all of this turn's
                // chunks have been broadcast.
                const PUMP_EVENT_BUDGET: usize = 64;
                let mut events = Vec::new();
                while events.len() < PUMP_EVENT_BUDGET {
                    match channel.try_recv() {
                        Some(ev) => events.push(ev),
                        None => break,
                    }
                }
                let more_pending = events.len() == PUMP_EVENT_BUDGET
                    && match channel.try_recv() {
                        Some(ev) => {
                            events.push(ev);
                            true
                        }
                        None => false,
                    };

                let current_turns = channel.turn_count();
                // Turn ended when (a) the queue is fully drained for this
                // cycle and (b) the ACP driver loop has bumped the turn
                // counter past what we last reported. Mirrors the direct
                // path in sketch-gpui's per-session pump.
                let turn_ended = !more_pending && current_turns > last_turns;

                // If the turn ended, drain any tail events that landed between
                // our budget drain and the `turn_count()` read *now*, while the
                // `&session.channel` borrow is still free. Collecting here ends
                // that borrow before the `session.record` calls below (which
                // need `&mut session`); the tail is recorded after the budget
                // events and before TurnEnded, preserving log order.
                let tail_events: Vec<sketch::acp_channel::ReplyEvent> = if turn_ended {
                    std::iter::from_fn(|| channel.try_recv()).collect()
                } else {
                    Vec::new()
                };

                // ── Replay fence: suppress duplicate events ──────────
                // When a session is restored from disk, the persisted
                // event_log already contains all events up to
                // `replay_fence` turns. The freshly-spawned ACP agent
                // replays those same turns as new ReplyEvents. We drain
                // them (so the channel doesn't back up) but skip logging
                // and broadcasting until the agent moves past the fence.
                let fence = session.replay_fence;
                if fence > 0 && current_turns <= fence {
                    // Still replaying — discard events silently.
                    let drained = !events.is_empty();
                    if turn_ended {
                        last_turns = current_turns;
                        if current_turns == fence {
                            // Replay complete — clear the fence so
                            // subsequent turns log normally.
                            session.replay_fence = 0;
                            tracing::info!(
                                session_id = %&session_id[..8],
                                turn = current_turns,
                                "replay fence cleared"
                            );
                        }
                    }
                    drop(sessions);
                    // Drained discarded replay events this cycle (or more are
                    // queued past the budget) → loop immediately; otherwise idle.
                    if !drained && !more_pending {
                        std::thread::sleep(PUMP_IDLE_SLEEP);
                    }
                    continue;
                }

                // Did this cycle produce any work? Drives drain-then-sleep.
                let drained_events = !events.is_empty();

                // Broadcast events first.
                for ev in events {
                    if std::env::var("SKETCH_CHUNKLOG").is_ok() {
                        if let sketch::acp_channel::ReplyEvent::Chunk(t) = &ev {
                            // info! so the SKETCH_CHUNKLOG env gate alone re-enables
                            // it (the default env-filter is "info"); otherwise this
                            // dev trace would silently also require RUST_LOG=debug.
                            tracing::info!("[chunklog srv] {t:?}");
                        }
                    }
                    session.record(Notification::ReplyEvent {
                        session_id: session_id.clone(),
                        event: ev,
                    });
                }

                if turn_ended {
                    // Tail events were drained above (while the channel borrow
                    // was free); record them before TurnEnded closes the turn.
                    for ev in tail_events {
                        session.record(Notification::ReplyEvent {
                            session_id: session_id.clone(),
                            event: ev,
                        });
                    }
                    last_turns = current_turns;
                    session.turns = current_turns;
                    session.record(Notification::TurnEnded {
                        session_id: session_id.clone(),
                        turn_count: current_turns,
                        generation: session.channel_generation,
                    });
                }

                drop(sessions);

                // Sleep ONLY when this cycle was fully idle: no events drained,
                // no more queued past the budget, and no turn-end fired. When we
                // did real work, loop immediately — more may already be queued.
                if !drained_events && !more_pending && !turn_ended {
                    std::thread::sleep(PUMP_IDLE_SLEEP);
                }
            }
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
                            if w.write_all(line.as_bytes()).await.is_err() {
                                return;
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
                let sessions = manager.list_sessions();
                Response::Ok {
                    data: ResponseData::Sessions { sessions },
                }
            }

            Request::CreateSession {
                cwd,
                label,
                resume_session_id,
            } => {
                let info = manager.create_session(cwd, label, resume_session_id);
                Response::Ok {
                    data: ResponseData::Session { session: info },
                }
            }

            Request::Attach { session_id, mode } => {
                match manager.attach(&session_id, mode, conn_id) {
                    Ok((rx, replay_len)) => {
                        // No explicit replay write here anymore: the forwarder
                        // tails `event_log` from index 0, so it streams the
                        // full history and then live events over one ordered,
                        // gap-free path. (We still receive the log length from
                        // attach for the count log; the contents are re-derived
                        // by the forwarder.)
                        tracing::info!(
                            session_id = %&session_id[..8],
                            replay_len,
                            "attach: forwarder will replay logged events"
                        );
                        let w = Arc::clone(&writer);
                        let handle = tokio::spawn(forward_notifications(
                            Arc::clone(&manager),
                            session_id.clone(),
                            w,
                            rx,
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
                match manager.detach(&session_id, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Promote { session_id } => {
                match manager.promote(&session_id, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Prompt { session_id, text } => {
                match manager.prompt(&session_id, &text, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Cancel { session_id } => {
                match manager.cancel(&session_id, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::RestartSession { session_id } => {
                match manager.restart_session(&session_id, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetPermissionMode { session_id, mode } => {
                match manager.set_permission_mode(&session_id, mode, conn_id) {
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
                match manager.close_session(&session_id, conn_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::RenameSession { session_id, label } => {
                match manager.rename_session(&session_id, label) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::AdminStatus => Response::Ok {
                data: ResponseData::AdminStatus {
                    snapshot: manager.admin_status(),
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
        if w.write_all(line.as_bytes()).await.is_err() {
            break "response write failed (client gone)".to_string();
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
        let _ = manager.detach(sid, conn_id);
    }
    manager_events.abort();
}

/// True for notifications that are recorded in `event_log` (the durable
/// transcript). `OwnerChanged` is the lone exception — it's transient
/// connection state, broadcast-only, and must be forwarded directly since
/// the log-tailing path below will never carry it.
fn is_logged(note: &Notification) -> bool {
    !matches!(note, Notification::OwnerChanged { .. })
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
    manager: Arc<SessionManager>,
    session_id: ServerSessionId,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    mut rx: broadcast::Receiver<Notification>,
) {
    // Number of `event_log` entries already written to this client.
    let mut sent = 0usize;

    loop {
        // 1. Tail: flush any logged events appended since we last sent.
        //    Snapshot under the lock, then release it before awaiting the
        //    socket write (never hold a std Mutex across `.await`).
        let new: Vec<Notification> = {
            let sessions = manager.lock_sessions();
            match sessions.get(&session_id) {
                Some(s) if s.event_log.len() > sent => s.event_log[sent..].to_vec(),
                Some(_) => Vec::new(),
                None => return, // session gone
            }
        };
        if !new.is_empty() {
            // Serialize the whole tail snapshot into one buffer and issue a
            // single write — same bytes/order, far fewer syscalls under
            // streaming. (Failed serializations are skipped, as before.)
            let mut buf = String::new();
            for note in &new {
                let frame = Frame::Notification { note: note.clone() };
                if let Ok(line) = serde_json::to_string(&frame) {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            let mut w = writer.lock().await;
            if w.write_all(buf.as_bytes()).await.is_err() {
                tracing::warn!(
                    session_id = %&session_id[..8.min(session_id.len())],
                    sent,
                    this_pass = new.len(),
                    "forwarder write failed — client gone"
                );
                return;
            }
            drop(w);
            sent += new.len();
        }

        // 2. Wait for a wake.
        match rx.recv().await {
            Ok(note) => {
                // Transient, never-logged notifications (OwnerChanged) must be
                // forwarded straight through — the tail above can't see them.
                // Logged notifications are ignored here; the next loop's tail
                // delivers them exactly once, in log order.
                if !is_logged(&note) {
                    let frame = Frame::Notification { note };
                    if let Ok(mut line) = serde_json::to_string(&frame) {
                        line.push('\n');
                        let mut w = writer.lock().await;
                        if w.write_all(line.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Harmless now: the wake was dropped but the data lives in
                // event_log, which the next tail pass recovers in full.
                tracing::warn!(
                    lagged = n,
                    "subscriber lagged — recovering missed events from event_log"
                );
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Producer gone (session closing). One final tail to flush any
                // trailing logged events, then exit.
                let tail: Vec<Notification> = {
                    let sessions = manager.lock_sessions();
                    match sessions.get(&session_id) {
                        Some(s) if s.event_log.len() > sent => {
                            s.event_log[sent..].to_vec()
                        }
                        _ => Vec::new(),
                    }
                };
                if !tail.is_empty() {
                    let mut buf = String::new();
                    for note in &tail {
                        let frame = Frame::Notification { note: note.clone() };
                        if let Ok(line) = serde_json::to_string(&frame) {
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                    }
                    let mut w = writer.lock().await;
                    if w.write_all(buf.as_bytes()).await.is_err() {
                        return;
                    }
                }
                return;
            }
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

    let manager = Arc::new(SessionManager::new());

    // Restore sessions from a prior run, if any.
    manager.restore_from_disk();

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
        let active_sessions = manager.lock_sessions().len();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The cascade guard: once any holder panics while holding the sessions
    /// lock the std `Mutex` is poisoned, and a plain `.lock().unwrap()` at every
    /// other site would then panic too — one stray panic killing every session.
    /// `lock_sessions()` must recover the guard so the server keeps serving.
    #[test]
    fn lock_sessions_recovers_from_a_poisoned_mutex() {
        let mgr = SessionManager::new();

        // Poison the mutex: panic while holding the raw lock (caught so the test
        // process survives — the panic message on stderr is expected noise).
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = mgr.sessions.lock().unwrap();
            panic!("boom while holding the sessions lock");
        }));
        assert!(res.is_err(), "the critical section must have panicked");
        assert!(mgr.sessions.is_poisoned(), "the mutex must now be poisoned");

        // A plain `.lock().unwrap()` here would cascade-panic; the helper must
        // hand back a usable, intact map instead (no torn state — the panic
        // happened before any insert).
        let guard = mgr.lock_sessions();
        assert!(guard.is_empty(), "recovered map is intact");
        drop(guard);

        // Recovery is durable, not one-shot: subsequent accesses keep working.
        assert!(mgr.lock_sessions().is_empty(), "server keeps serving after poison");
    }
}
