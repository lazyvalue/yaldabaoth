//! GUI-side client for `sketch-session-server`.
//!
//! Connects to the server over a Unix domain socket, sends requests, and
//! receives notifications. Provides a channel-like API that mirrors
//! `AcpChannelClient` so the GUI's pump task can drain events with
//! `try_recv()`.

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::acp_channel::PermissionMode;
use crate::session_proto::*;

/// Client connection to the session server. One per GUI instance,
/// shared across all agent slots.
pub struct SessionServerClient {
    /// Outbound requests. The writer thread picks these up and sends
    /// them over the socket.
    request_tx: std_mpsc::Sender<(Frame, Option<std_mpsc::Sender<Response>>)>,
    /// Inbound notifications from the server.
    notification_rx: std_mpsc::Receiver<Notification>,
    /// Connection liveness.
    connected: Arc<AtomicBool>,
    /// Monotonic request id counter.
    next_id: AtomicU64,
    /// Background threads.
    _reader: Option<JoinHandle<()>>,
    _writer: Option<JoinHandle<()>>,
    /// Pending response channels, keyed by request id.
    pending: Arc<Mutex<std::collections::HashMap<u64, std_mpsc::Sender<Response>>>>,
}

impl SessionServerClient {
    /// Connect to the session server. Auto-launches it if not running.
    pub fn connect() -> io::Result<Self> {
        let path = socket_path();
        let stream = Self::connect_or_launch(&path)?;
        Self::from_stream(stream)
    }

    fn connect_or_launch(path: &std::path::Path) -> io::Result<UnixStream> {
        // Try connecting first.
        match UnixStream::connect(path) {
            Ok(s) => return Ok(s),
            Err(_) => {}
        }

        // Try launching the server.
        let server_bin = Self::find_server_binary()?;
        let mut cmd = std::process::Command::new(&server_bin);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());
        cmd.spawn().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to launch session server at {}: {e}", server_bin.display()),
            )
        })?;

        // Retry with backoff.
        let backoffs = [50, 100, 200, 400, 800, 1600];
        for ms in backoffs {
            std::thread::sleep(Duration::from_millis(ms));
            if let Ok(s) = UnixStream::connect(path) {
                return Ok(s);
            }
        }

        Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "session server did not start in time",
        ))
    }

    fn find_server_binary() -> io::Result<PathBuf> {
        // Try the same directory as the current executable first.
        if let Ok(exe) = std::env::current_exe() {
            let sibling = exe.with_file_name("sketch-session-server");
            if sibling.exists() {
                return Ok(sibling);
            }
        }
        // Fall back to PATH lookup.
        Ok(PathBuf::from("sketch-session-server"))
    }

    fn from_stream(stream: UnixStream) -> io::Result<Self> {
        let connected = Arc::new(AtomicBool::new(true));
        let pending: Arc<Mutex<std::collections::HashMap<u64, std_mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let (notification_tx, notification_rx) = std_mpsc::channel();
        let (request_tx, request_rx) =
            std_mpsc::channel::<(Frame, Option<std_mpsc::Sender<Response>>)>();

        // Clone the stream for reading.
        let read_stream = stream.try_clone()?;
        let write_stream = stream;

        // Reader thread: reads NDJSON lines, routes responses and notifications.
        let connected_r = Arc::clone(&connected);
        let pending_r = Arc::clone(&pending);
        let reader = std::thread::Builder::new()
            .name("session-client-reader".into())
            .spawn(move || {
                let reader = io::BufReader::new(read_stream);
                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break,
                    };
                    let frame: Frame = match serde_json::from_str(&line) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    match frame {
                        Frame::Response { id, result } => {
                            let mut map = pending_r.lock().unwrap();
                            if let Some(tx) = map.remove(&id) {
                                let _ = tx.send(result);
                            }
                        }
                        Frame::Notification { note } => {
                            if notification_tx.send(note).is_err() {
                                break;
                            }
                        }
                        Frame::Request { .. } => {
                            // Server should not send requests to GUI.
                        }
                    }
                }
                connected_r.store(false, Ordering::SeqCst);
            })?;

        // Writer thread: sends outbound requests and registers pending slots.
        let connected_w = Arc::clone(&connected);
        let pending_w = Arc::clone(&pending);
        let writer = std::thread::Builder::new()
            .name("session-client-writer".into())
            .spawn(move || {
                let mut stream = write_stream;
                while let Ok((frame, resp_tx)) = request_rx.recv() {
                    // Register the pending response channel before writing.
                    if let (Frame::Request { id, .. }, Some(tx)) = (&frame, resp_tx) {
                        pending_w.lock().unwrap().insert(*id, tx);
                    }
                    let mut line = match serde_json::to_string(&frame) {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    line.push('\n');
                    if stream.write_all(line.as_bytes()).is_err() {
                        connected_w.store(false, Ordering::SeqCst);
                        break;
                    }
                    if stream.flush().is_err() {
                        connected_w.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            })?;

        Ok(Self {
            request_tx,
            notification_rx,
            connected,
            next_id: AtomicU64::new(1),
            _reader: Some(reader),
            _writer: Some(writer),
            pending,
        })
    }

    /// Send a request and wait for the response (blocking).
    fn request(&self, req: Request) -> io::Result<Response> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "session server disconnected",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (resp_tx, resp_rx) = std_mpsc::channel();
        let frame = Frame::Request { id, req };
        self.request_tx
            .send((frame, Some(resp_tx)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer thread gone"))?;

        resp_rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request timed out"))
    }

    /// Send a request without waiting for a response (fire-and-forget).
    fn request_fire(&self, req: Request) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "session server disconnected",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let frame = Frame::Request { id, req };
        self.request_tx
            .send((frame, None))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer thread gone"))
    }

    // ── Public API ─────────────────────────────────────────────────

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn list_sessions(&self) -> io::Result<Vec<SessionInfo>> {
        match self.request(Request::ListSessions)? {
            Response::Ok {
                data: ResponseData::Sessions { sessions },
            } => Ok(sessions),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    pub fn create_session(
        &self,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> io::Result<SessionInfo> {
        match self.request(Request::CreateSession {
            cwd,
            label,
            resume_session_id,
        })? {
            Response::Ok {
                data: ResponseData::Session { session },
            } => Ok(session),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response",
            )),
        }
    }

    pub fn attach(&self, session_id: &str) -> io::Result<()> {
        match self.request(Request::Attach {
            session_id: session_id.to_string(),
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    pub fn detach(&self, session_id: &str) -> io::Result<()> {
        match self.request(Request::Detach {
            session_id: session_id.to_string(),
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    pub fn prompt(&self, session_id: &str, text: &str) -> io::Result<()> {
        match self.request(Request::Prompt {
            session_id: session_id.to_string(),
            text: text.to_string(),
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    pub fn set_permission_mode(
        &self,
        session_id: &str,
        mode: PermissionMode,
    ) -> io::Result<()> {
        match self.request(Request::SetPermissionMode {
            session_id: session_id.to_string(),
            mode,
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    pub fn close_session(&self, session_id: &str) -> io::Result<()> {
        match self.request(Request::CloseSession {
            session_id: session_id.to_string(),
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    pub fn ping(&self) -> io::Result<()> {
        match self.request(Request::Ping)? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    /// Non-blocking poll for the next server notification.
    pub fn try_recv(&self) -> Option<Notification> {
        self.notification_rx.try_recv().ok()
    }
}
