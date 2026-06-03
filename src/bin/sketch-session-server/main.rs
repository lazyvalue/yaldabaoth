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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use serde::{Deserialize, Serialize};

use sketch::acp_channel::{AcpChannelClient, PermissionMode, SketchFrontend};
use sketch::session_proto::*;

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
    /// Wrapped in `Arc` so `attach`/`save_to_disk` clone a *pointer* under the
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

    /// Broadcast an `OwnerChanged` to all attached connections. Not appended
    /// to `event_log` — ownership is transient connection state, not part of
    /// the conversation transcript a late observer needs to replay.
    fn broadcast_owner_changed(&self) {
        let _ = self.event_tx.send(Notification::OwnerChanged {
            session_id: self.id.clone(),
            has_owner: self.owner.is_some(),
        });
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

/// Serializable snapshot of a single session, saved to disk so the server
/// can restore sessions across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSession {
    server_session_id: String,
    acp_session_id: Option<String>,
    label: String,
    cwd: PathBuf,
    turns: usize,
    permission_mode: PermissionMode,
    event_log: Vec<Notification>,
}

impl SessionManager {
    fn new() -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            sessions: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// Subscribe to manager-level session-list notifications. Each connection
    /// calls this once and forwards everything it receives to its GUI.
    fn subscribe_events(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    /// Snapshot all sessions to disk so they survive a server restart.
    fn save_to_disk(&self) {
        let Some(path) = session_server_persist_path() else {
            return;
        };
        // Snapshot lightweight metadata + an `Arc` *pointer* to each session's
        // event_log under the lock, then release it. The expensive deep copy of
        // each log into the owned `PersistedSession.event_log` happens off-lock,
        // so a large/unbounded transcript no longer stalls every other session
        // for the duration of the clone+serialize.
        struct Snap {
            server_session_id: String,
            acp_session_id: Option<String>,
            label: String,
            cwd: PathBuf,
            turns: usize,
            permission_mode: PermissionMode,
            event_log: Arc<Vec<Notification>>,
        }
        let snaps: Vec<Snap> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .values()
                .map(|s| Snap {
                    server_session_id: s.id.clone(),
                    acp_session_id: s.channel.as_ref().and_then(|c| c.session_id()),
                    label: s.label.clone(),
                    cwd: s.cwd.clone(),
                    turns: s.turns,
                    permission_mode: s.permission_mode,
                    event_log: Arc::clone(&s.event_log),
                })
                .collect()
        };
        let persisted: Vec<PersistedSession> = snaps
            .into_iter()
            .map(|s| PersistedSession {
                server_session_id: s.server_session_id,
                acp_session_id: s.acp_session_id,
                label: s.label,
                cwd: s.cwd,
                turns: s.turns,
                permission_mode: s.permission_mode,
                event_log: (*s.event_log).clone(),
            })
            .collect();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&persisted) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("[session-server] failed to persist sessions: {e}");
                }
            }
            Err(e) => {
                eprintln!("[session-server] failed to serialize sessions: {e}");
            }
        }
    }

    /// Load persisted sessions from disk and re-spawn their ACP subprocesses.
    /// Called once at startup before accepting connections.
    fn restore_from_disk(self: &Arc<Self>) {
        let Some(path) = session_server_persist_path() else {
            return;
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return, // No persist file — first run.
        };
        let persisted: Vec<PersistedSession> = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[session-server] failed to parse persisted sessions: {e}");
                return;
            }
        };

        // Clear the file immediately — if we crash mid-restore, stale entries
        // won't re-spawn on the next boot.
        let _ = std::fs::remove_file(&path);

        for ps in persisted {
            // Only restore sessions that had an ACP session id — without one,
            // there's nothing to --resume.
            let Some(acp_session_id) = ps.acp_session_id.clone() else {
                eprintln!(
                    "[session-server] skipping restore of {}: no acp_session_id",
                    &ps.server_session_id[..8],
                );
                continue;
            };

            let (event_tx, _) = broadcast::channel(16384);

            let session = ManagedSession {
                id: ps.server_session_id.clone(),
                label: ps.label.clone(),
                cwd: ps.cwd.clone(),
                channel: None,
                channel_generation: 0,
                turns: ps.turns,
                permission_mode: ps.permission_mode,
                event_tx: event_tx.clone(),
                owner: None,
                pending_prompts: Vec::new(),
                event_log: Arc::new(ps.event_log),
                replay_fence: ps.turns,
            };

            eprintln!(
                "[session-server] restoring session {} (acp {})",
                &ps.server_session_id[..8],
                &acp_session_id[..8.min(acp_session_id.len())],
            );

            self.sessions
                .lock()
                .unwrap()
                .insert(ps.server_session_id.clone(), session);

            // Re-spawn the ACP subprocess with --resume.
            let manager = Arc::clone(self);
            let session_id = ps.server_session_id.clone();
            let cwd = ps.cwd.clone();
            std::thread::Builder::new()
                .name(format!("acp-resume-{}", &session_id[..8]))
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
                            let new_acp_id = client.session_id();
                            {
                                let mut sessions = manager.sessions.lock().unwrap();
                                if let Some(s) = sessions.get_mut(&session_id) {
                                    s.channel = Some(client);
                                    let note = Notification::SessionAttached {
                                        session_id: session_id.clone(),
                                        acp_session_id: new_acp_id,
                                    };
                                    Arc::make_mut(&mut s.event_log).push(note.clone());
                                    let _ = s.event_tx.send(note);
                                }
                            }
                            spawn_pump_thread(Arc::clone(&manager), session_id);
                        }
                        Err(e) => {
                            eprintln!(
                                "[session-server] failed to resume session {}: {e}",
                                &session_id[..8],
                            );
                            let mut sessions = manager.sessions.lock().unwrap();
                            if let Some(s) = sessions.get_mut(&session_id) {
                                let note = Notification::SessionDetached {
                                    session_id: session_id.clone(),
                                    reason: format!("resume failed: {e}"),
                                };
                                Arc::make_mut(&mut s.event_log).push(note.clone());
                                let _ = s.event_tx.send(note);
                            }
                        }
                    }
                })
                .ok();
        }
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.values().map(|s| s.info()).collect()
    }

    fn create_session(
        self: &Arc<Self>,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let (event_tx, _) = broadcast::channel(16384);

        let session = ManagedSession {
            id: id.clone(),
            label,
            cwd: cwd.clone(),
            channel: None,
            channel_generation: 0,
            turns: 0,
            permission_mode: PermissionMode::Yolo,
            event_tx: event_tx.clone(),
            owner: None,
            pending_prompts: Vec::new(),
            event_log: Arc::new(Vec::new()),
            replay_fence: 0,
        };

        let info = session.info();
        self.sessions.lock().unwrap().insert(id.clone(), session);
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
                    Ok(mut client) => {
                        let acp_id = client.session_id();
                        // Drain any prompts that the GUI submitted while we
                        // were spawning. Send them in arrival order before
                        // we publish the channel, so they're queued at the
                        // ACP driver loop before any future prompt races in.
                        let queued = {
                            let mut sessions = manager.sessions.lock().unwrap();
                            sessions
                                .get_mut(&session_id)
                                .map(|s| std::mem::take(&mut s.pending_prompts))
                                .unwrap_or_default()
                        };
                        for text in queued {
                            if let Err(e) = client.send(&text) {
                                eprintln!(
                                    "[session-server] failed to flush queued prompt: {e}"
                                );
                            }
                        }
                        {
                            let mut sessions = manager.sessions.lock().unwrap();
                            if let Some(s) = sessions.get_mut(&session_id) {
                                s.channel = Some(client);
                                let note = Notification::SessionAttached {
                                    session_id: session_id.clone(),
                                    acp_session_id: acp_id,
                                };
                                Arc::make_mut(&mut s.event_log).push(note.clone());
                                let _ = s.event_tx.send(note);
                            }
                        }
                        // Start the pump thread now that the channel is live.
                        spawn_pump_thread(Arc::clone(&manager), session_id);
                    }
                    Err(e) => {
                        let mut sessions = manager.sessions.lock().unwrap();
                        if let Some(s) = sessions.get_mut(&session_id) {
                            let note = Notification::SessionDetached {
                                session_id: session_id.clone(),
                                reason: format!("spawn failed: {e}"),
                            };
                            Arc::make_mut(&mut s.event_log).push(note.clone());
                            let _ = s.event_tx.send(note);
                        }
                    }
                }
            })
            .ok();

        info
    }

    fn close_session(&self, session_id: &str, conn_id: u64) -> Result<(), String> {
        let removed = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(s) if s.owner == Some(conn_id) => {
                    // Dropping ManagedSession drops AcpChannelClient → kills subprocess.
                    sessions.remove(session_id);
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
        let mut sessions = self.sessions.lock().unwrap();
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
        let mut sessions = self.sessions.lock().unwrap();
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
        let mut sessions = self.sessions.lock().unwrap();
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
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.owner != Some(conn_id) {
            return Err("only the session owner can send prompts".into());
        }

        // Log the user's prompt so re-attaching GUIs can replay it.
        // Only appended to event_log (not broadcast) — the live GUI
        // already inserted the text locally in submit_chatbox before
        // calling prompt(). Broadcasting would duplicate it.
        Arc::make_mut(&mut session.event_log).push(Notification::UserPrompt {
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
        let mut sessions = self.sessions.lock().unwrap();
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
            let sessions = self.sessions.lock().unwrap();
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
                        let acp_id = client.session_id();
                        // Swap under the lock so the pump never sees a None
                        // channel; drop the old one *after* releasing the
                        // lock (Drop joins the worker / kills the child).
                        let old = {
                            let mut sessions = manager.sessions.lock().unwrap();
                            let Some(s) = sessions.get_mut(&sid) else { return };
                            let old = s.channel.take();
                            s.channel = Some(client);
                            s.channel_generation = s.channel_generation.wrapping_add(1);
                            let note = Notification::SessionAttached {
                                session_id: sid.clone(),
                                acp_session_id: acp_id,
                            };
                            Arc::make_mut(&mut s.event_log).push(note.clone());
                            let _ = s.event_tx.send(note);
                            old
                        };
                        drop(old);
                    }
                    Err(e) => {
                        let mut sessions = manager.sessions.lock().unwrap();
                        if let Some(s) = sessions.get_mut(&sid) {
                            let note = Notification::SessionDetached {
                                session_id: sid.clone(),
                                reason: format!("restart failed: {e}"),
                            };
                            Arc::make_mut(&mut s.event_log).push(note.clone());
                            let _ = s.event_tx.send(note);
                        }
                    }
                }
            })
            .ok();
        Ok(())
    }

    fn rename_session(&self, session_id: &str, label: String) -> Result<(), String> {
        {
            let mut sessions = self.sessions.lock().unwrap();
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
        let mut sessions = self.sessions.lock().unwrap();
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
                let mut sessions = manager.sessions.lock().unwrap();
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
                    let note = Notification::SessionDetached {
                        session_id: session_id.clone(),
                        reason: "agent disconnected".into(),
                    };
                    Arc::make_mut(&mut session.event_log).push(note.clone());
                    let _ = session.event_tx.send(note);
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
                            eprintln!(
                                "[session-server] replay fence cleared for {} at turn {}",
                                &session_id[..8], current_turns,
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
                            eprintln!("[chunklog srv] {t:?}");
                        }
                    }
                    let note = Notification::ReplyEvent {
                        session_id: session_id.clone(),
                        event: ev,
                    };
                    Arc::make_mut(&mut session.event_log).push(note.clone());
                    let _ = session.event_tx.send(note);
                }

                if turn_ended {
                    // Drain any tail events that landed between our budget
                    // drain and the `turn_count()` read so they reach the
                    // GUI before the TurnEnded that closes the turn.
                    while let Some(ev) = channel.try_recv() {
                        let note = Notification::ReplyEvent {
                            session_id: session_id.clone(),
                            event: ev,
                        };
                        Arc::make_mut(&mut session.event_log).push(note.clone());
                        let _ = session.event_tx.send(note);
                    }
                    last_turns = current_turns;
                    session.turns = current_turns;
                    let note = Notification::TurnEnded {
                        session_id: session_id.clone(),
                        turn_count: current_turns,
                    };
                    Arc::make_mut(&mut session.event_log).push(note.clone());
                    let _ = session.event_tx.send(note);
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

    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let frame: Frame = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[session-server] bad frame: {e}");
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
                        eprintln!(
                            "[session-server] attach {}: forwarder will replay {} logged events",
                            &session_id[..8],
                            replay_len,
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
        };

        let resp_frame = Frame::Response {
            id,
            result: response,
        };
        let mut line = serde_json::to_string(&resp_frame).unwrap();
        line.push('\n');
        let mut w = writer.lock().await;
        if w.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }

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
            let sessions = manager.sessions.lock().unwrap();
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
                eprintln!(
                    "[session-server] subscriber lagged by {n} wakes — \
                     recovering missed events from event_log"
                );
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Producer gone (session closing). One final tail to flush any
                // trailing logged events, then exit.
                let tail: Vec<Notification> = {
                    let sessions = manager.sessions.lock().unwrap();
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
    let socket_path = socket_path();
    let pid_path = pid_file_path();

    // Clean up stale socket.
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;

    // Write PID file.
    let _ = std::fs::write(&pid_path, std::process::id().to_string());

    eprintln!(
        "[sketch-session-server] listening on {}",
        socket_path.display()
    );

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
        eprintln!("[sketch-session-server] shutting down — persisting sessions");
        mgr_shutdown.save_to_disk();
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
        tokio::spawn(handle_connection(stream, mgr, conn_id));
    }
}
