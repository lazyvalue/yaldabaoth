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

use sketch::acp_channel::{AcpChannelClient, PermissionMode, SketchFrontend};
use sketch::session_proto::*;

// ── Managed session ────────────────────────────────────────────────

struct ManagedSession {
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    /// The live ACP channel. `None` while the subprocess is being spawned.
    channel: Option<AcpChannelClient>,
    turns: usize,
    permission_mode: PermissionMode,
    /// Broadcast sender — attached GUI connections subscribe here.
    event_tx: broadcast::Sender<Notification>,
    /// Whether a GUI connection is currently attached.
    has_subscriber: bool,
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
        }
    }
}

// ── Session manager ────────────────────────────────────────────────

struct SessionManager {
    sessions: Mutex<HashMap<ServerSessionId, ManagedSession>>,
}

impl SessionManager {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
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
        let (event_tx, _) = broadcast::channel(1024);

        let session = ManagedSession {
            id: id.clone(),
            label,
            cwd: cwd.clone(),
            channel: None,
            turns: 0,
            permission_mode: PermissionMode::Yolo,
            event_tx: event_tx.clone(),
            has_subscriber: false,
        };

        let info = session.info();
        self.sessions.lock().unwrap().insert(id.clone(), session);

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
                        let acp_id = client.session_id();
                        {
                            let mut sessions = manager.sessions.lock().unwrap();
                            if let Some(s) = sessions.get_mut(&session_id) {
                                s.channel = Some(client);
                                let _ = s.event_tx.send(Notification::SessionAttached {
                                    session_id: session_id.clone(),
                                    acp_session_id: acp_id,
                                });
                            }
                        }
                        // Start the pump thread now that the channel is live.
                        spawn_pump_thread(Arc::clone(&manager), session_id);
                    }
                    Err(e) => {
                        let sessions = manager.sessions.lock().unwrap();
                        if let Some(s) = sessions.get(&session_id) {
                            let _ = s.event_tx.send(Notification::SessionDetached {
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

    fn close_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.remove(session_id).is_some() {
            // Dropping ManagedSession drops AcpChannelClient → kills subprocess.
            Ok(())
        } else {
            Err(format!("no such session: {session_id}"))
        }
    }

    fn attach(
        &self,
        session_id: &str,
    ) -> Result<broadcast::Receiver<Notification>, String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.has_subscriber {
            return Err("another GUI is already attached to this session".into());
        }
        session.has_subscriber = true;
        Ok(session.event_tx.subscribe())
    }

    fn detach(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        session.has_subscriber = false;
        Ok(())
    }

    fn prompt(&self, session_id: &str, text: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        let channel = session
            .channel
            .as_mut()
            .ok_or("session not yet attached to agent")?;
        channel
            .send(text)
            .map_err(|e| format!("send failed: {e}"))
    }

    fn set_permission_mode(
        &self,
        session_id: &str,
        mode: PermissionMode,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
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

            loop {
                std::thread::sleep(std::time::Duration::from_millis(16));

                let mut sessions = manager.sessions.lock().unwrap();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return; // Session was closed.
                };
                let Some(channel) = &session.channel else {
                    drop(sessions);
                    continue; // Not yet spawned.
                };

                // Check liveness.
                if !channel.is_connected() {
                    let _ = session.event_tx.send(Notification::SessionDetached {
                        session_id: session_id.clone(),
                        reason: "agent disconnected".into(),
                    });
                    session.channel = None;
                    return;
                }

                // Drain events.
                let mut events = Vec::new();
                while let Some(ev) = channel.try_recv() {
                    events.push(ev);
                    if events.len() >= 64 {
                        break;
                    }
                }

                let current_turns = channel.turn_count();

                // Detect turn end.
                let turn_ended = events.is_empty() && current_turns > last_turns;
                if current_turns > last_turns {
                    last_turns = current_turns;
                    session.turns = current_turns;
                }

                // Broadcast events.
                for ev in events {
                    let _ = session.event_tx.send(Notification::ReplyEvent {
                        session_id: session_id.clone(),
                        event: ev,
                    });
                }

                if turn_ended {
                    let _ = session.event_tx.send(Notification::TurnEnded {
                        session_id: session_id.clone(),
                        turn_count: current_turns,
                    });
                }

                drop(sessions);
            }
        })
        .ok();
}

// ── Connection handler ─────────────────────────────────────────────

/// Handle a single GUI connection on the Unix socket.
async fn handle_connection(stream: UnixStream, manager: Arc<SessionManager>) {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    // Track which sessions this connection is subscribed to, so we can
    // clean up on disconnect.
    let mut subscribed: HashMap<ServerSessionId, tokio::task::JoinHandle<()>> = HashMap::new();

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

            Request::Attach { session_id } => {
                match manager.attach(&session_id) {
                    Ok(rx) => {
                        // Spawn a writer task that forwards notifications.
                        let w = Arc::clone(&writer);
                        let sid = session_id.clone();
                        let handle = tokio::spawn(forward_notifications(rx, w, sid));
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
                match manager.detach(&session_id) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Prompt { session_id, text } => {
                match manager.prompt(&session_id, &text) {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetPermissionMode { session_id, mode } => {
                match manager.set_permission_mode(&session_id, mode) {
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
                match manager.close_session(&session_id) {
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

    // Connection closed — detach all sessions and cancel forwarders.
    for (sid, handle) in &subscribed {
        handle.abort();
        let _ = manager.detach(sid);
    }
}

/// Forward broadcast notifications to a GUI connection's writer.
async fn forward_notifications(
    mut rx: broadcast::Receiver<Notification>,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    _session_id: ServerSessionId,
) {
    loop {
        match rx.recv().await {
            Ok(note) => {
                let frame = Frame::Notification { note };
                let mut line = match serde_json::to_string(&frame) {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                line.push('\n');
                let mut w = writer.lock().await;
                if w.write_all(line.as_bytes()).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("[session-server] subscriber lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => {
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

    // Handle graceful shutdown.
    let socket_path_cleanup = socket_path.clone();
    let pid_path_cleanup = pid_path.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        eprintln!("[sketch-session-server] shutting down");
        let _ = std::fs::remove_file(&socket_path_cleanup);
        let _ = std::fs::remove_file(&pid_path_cleanup);
        std::process::exit(0);
    });

    loop {
        let (stream, _) = listener.accept().await?;
        let mgr = Arc::clone(&manager);
        tokio::spawn(handle_connection(stream, mgr));
    }
}
