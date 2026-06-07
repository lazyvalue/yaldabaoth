//! GUI-side client for `sketch-session-server`.
//!
//! Connects to the server over a Unix domain socket, sends requests, and
//! receives notifications. Provides a channel-like API that mirrors
//! `AcpChannelClient` so the GUI's pump task can drain events with
//! `try_recv()`.

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::acp_channel::PermissionMode;
use crate::session_proto::*;

pub use crate::session_proto::AttachMode;

/// Client connection to the session server. One per GUI instance,
/// shared across all agent slots.
pub struct SessionServerClient {
    /// Outbound requests. The writer thread picks these up and sends
    /// them over the socket.
    request_tx: std_mpsc::Sender<(Frame, Option<std_mpsc::Sender<Response>>)>,
    /// Inbound notifications from the server. `Option` so the pump task can
    /// `take` it and drain the channel *outside* the GUI model lock — channel
    /// reads need no `&mut SketchGpuiView`, so taking the lock to call
    /// `try_recv` was pure contention.
    notification_rx: Option<std_mpsc::Receiver<Notification>>,
    /// Event-driven wake signal. The reader thread pushes `()` after every
    /// notification so the pump can wake immediately (via `select_biased!`)
    /// instead of polling on a fixed timer. Taken once by the pump task.
    wake_rx: Option<futures::channel::mpsc::UnboundedReceiver<()>>,
    /// Connection liveness.
    connected: Arc<AtomicBool>,
    /// Monotonic request id counter. `Arc` so a [`SessionServerHandle`] can
    /// share the same id space — off-thread requests must not collide with
    /// the main client's ids or two callers could steal each other's
    /// responses.
    next_id: Arc<AtomicU64>,
    /// Background threads.
    _reader: Option<JoinHandle<()>>,
    _writer: Option<JoinHandle<()>>,
    /// Pending response channels, keyed by request id.
    pending: Arc<Mutex<std::collections::HashMap<u64, std_mpsc::Sender<Response>>>>,
}

/// Drop every pending response channel, unblocking any thread parked in
/// [`SessionServerClient::request`]. Called the instant either background
/// thread observes a dead socket — otherwise a blocking request (e.g.
/// `close_session`) parks on its 30s timeout while the GPUI main thread is
/// frozen. Dropping the sender makes the waiting `recv_timeout` return
/// `Err(Disconnected)` immediately, which surfaces as a `BrokenPipe` error
/// the caller can react to (reconnect) instead of a multi-second hang.
fn fail_all_pending(
    pending: &Arc<Mutex<std::collections::HashMap<u64, std_mpsc::Sender<Response>>>>,
) {
    if let Ok(mut map) = pending.lock() {
        map.clear();
    }
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

        // Launch the server DETACHED so it outlives this GUI: its own process
        // group (no SIGINT/SIGHUP from the terminal that launched the GUI), and
        // stdout/stderr go to a log file rather than the GUI's terminal so the
        // daemon is fully decoupled from the launching session.
        let server_bin = Self::find_server_binary()?;
        let mut cmd = std::process::Command::new(&server_bin);
        cmd.stdin(std::process::Stdio::null());
        match Self::server_log_file() {
            Some(log) => {
                let err = log.try_clone().ok();
                cmd.stdout(std::process::Stdio::from(log));
                match err {
                    Some(err) => {
                        cmd.stderr(std::process::Stdio::from(err));
                    }
                    None => {
                        cmd.stderr(std::process::Stdio::null());
                    }
                }
            }
            None => {
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
            }
        }
        cmd.process_group(0); // detach from the GUI's process group
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

    /// Append-mode log file for the detached server's stdout/stderr, so the
    /// daemon's output survives the terminal that launched the GUI. Lives at
    /// `<cache>/sketch/session-server.log` (macOS: `~/Library/Caches/...`).
    /// `None` if the cache dir or file can't be opened (caller falls back to
    /// discarding output).
    fn server_log_file() -> Option<std::fs::File> {
        let path = dirs::cache_dir()?.join("sketch").join("session-server.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
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
        let (wake_tx, wake_rx) = futures::channel::mpsc::unbounded::<()>();
        let (request_tx, request_rx) =
            std_mpsc::channel::<(Frame, Option<std_mpsc::Sender<Response>>)>();

        // Clone the stream for reading. The writer keeps `write_stream` and is
        // responsible for shutting the whole socket down once the request
        // channel closes — AFTER it has flushed every queued frame — which in
        // turn unblocks this detached reader (its blocking read returns EOF).
        let read_stream = stream.try_clone()?;
        let write_stream = stream;

        // Reader thread: reads NDJSON lines, routes responses and notifications.
        let connected_r = Arc::clone(&connected);
        let pending_r = Arc::clone(&pending);
        let reader = std::thread::Builder::new()
            .name("session-client-reader".into())
            .spawn(move || {
                let reader = io::BufReader::new(read_stream);
                // Why this thread exits = why the GUI saw a disconnect. Logged
                // below so a reconnect storm is diagnosable (the default — the
                // `for` loop ending — is a clean server-side EOF).
                let mut exit_reason = String::from("server closed connection (EOF)");
                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            exit_reason = format!("socket read error: {e}");
                            break;
                        }
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
                                exit_reason =
                                    String::from("pump dropped the notification receiver");
                                break;
                            }
                            // Wake the pump. Unbounded send never blocks; an
                            // error just means the pump dropped its receiver,
                            // which is harmless (it falls back to its timer).
                            let _ = wake_tx.unbounded_send(());
                        }
                        Frame::Request { .. } => {
                            // Server should not send requests to GUI.
                        }
                    }
                }
                connected_r.store(false, Ordering::SeqCst);
                eprintln!("[sketch-gpui] session-client reader exiting — {exit_reason}");
                // Socket closed / EOF: unblock anyone parked in `request`.
                fail_all_pending(&pending_r);
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
                // The request channel closed (client dropped / reconnecting) or
                // a write failed: every queued frame above has been flushed.
                // NOW shut the socket down — this unblocks the detached reader
                // (its blocking read returns EOF) AND makes the server observe
                // the disconnect and release session ownership. Doing it here,
                // *after* the drain, is what lets a fire-and-forget prompt sent
                // just before drop still reach the server (shutting down in
                // `Drop` instead raced this flush and silently dropped it).
                let _ = stream.shutdown(std::net::Shutdown::Both);
                // Writer exiting: unblock any in-flight blocking request so it
                // fails fast.
                fail_all_pending(&pending_w);
            })?;

        Ok(Self {
            request_tx,
            notification_rx: Some(notification_rx),
            wake_rx: Some(wake_rx),
            connected,
            next_id: Arc::new(AtomicU64::new(1)),
            _reader: Some(reader),
            _writer: Some(writer),
            pending,
        })
    }

    /// Re-establish the connection in place after a disconnect. Rebuilds the
    /// socket, reader/writer threads, and the notification/wake channels. On
    /// success the notification + wake receivers are `Some` again, so the pump
    /// must re-take them via [`take_notification_receiver`] /
    /// [`take_wake_receiver`] and re-attach every live slot (the server
    /// replays each session's full event log on attach, so the GUI rebuilds
    /// its transcript from scratch).
    ///
    /// Returns the freshly-built receivers on success so the caller can splice
    /// them into the running pump without a second lock round-trip.
    pub fn reconnect(
        &mut self,
    ) -> io::Result<(
        std_mpsc::Receiver<Notification>,
        futures::channel::mpsc::UnboundedReceiver<()>,
    )> {
        let path = socket_path();
        let stream = Self::connect_or_launch(&path)?;
        let fresh = Self::from_stream(stream)?;
        // Replace our internals wholesale. Assigning `*self` drops the old
        // value (its threads have already exited — that's why we're
        // reconnecting — and its `pending` map was drained on disconnect), and
        // moves `fresh` in without running its destructor.
        *self = fresh;
        let note_rx = self
            .notification_rx
            .take()
            .expect("from_stream always sets notification_rx");
        let wake_rx = self
            .wake_rx
            .take()
            .expect("from_stream always sets wake_rx");
        Ok((note_rx, wake_rx))
    }

    /// Move the notification receiver out for exclusive ownership by the pump
    /// task. After this, `try_recv` returns `None`. Call once.
    pub fn take_notification_receiver(&mut self) -> Option<std_mpsc::Receiver<Notification>> {
        self.notification_rx.take()
    }

    /// Move the wake receiver out for the pump task. Call once.
    pub fn take_wake_receiver(
        &mut self,
    ) -> Option<futures::channel::mpsc::UnboundedReceiver<()>> {
        self.wake_rx.take()
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
            .map_err(|e| match e {
                // The reader/writer thread dropped our pending sender on
                // disconnect — fail fast as BrokenPipe so callers can
                // distinguish "server gone" (→ reconnect) from a genuine
                // 30s stall on a live connection.
                std_mpsc::RecvTimeoutError::Disconnected => {
                    self.connected.store(false, Ordering::SeqCst);
                    io::Error::new(io::ErrorKind::BrokenPipe, "session server disconnected")
                }
                std_mpsc::RecvTimeoutError::Timeout => {
                    io::Error::new(io::ErrorKind::TimedOut, "request timed out")
                }
            })
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

    /// Build a cheap, cloneable, `Send + Sync` handle that can issue the same
    /// blocking requests from any thread. Used by the GUI to move
    /// `list_sessions` / `create_session` / `close_session` round-trips off
    /// the GPUI paint thread: those calls park on a 30s `recv_timeout`, so a
    /// stalled server would otherwise freeze the window. The handle shares the
    /// writer channel, the pending-response map, the liveness flag, and the
    /// request-id counter with this client, so responses route correctly and
    /// the same disconnect logic unblocks both. The notification stream stays
    /// exclusively on the [`SessionServerClient`] / pump task; a handle never
    /// reads notifications.
    pub fn handle(&self) -> SessionServerHandle {
        SessionServerHandle {
            request_tx: self.request_tx.clone(),
            connected: Arc::clone(&self.connected),
            next_id: Arc::clone(&self.next_id),
            pending: Arc::clone(&self.pending),
        }
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

    /// Fetch a diagnostic snapshot of the server's live session state
    /// (ownership, subscriber counts, channel generation). Read-only.
    pub fn admin_status(&self) -> io::Result<AdminSnapshot> {
        match self.request(Request::AdminStatus)? {
            Response::Ok {
                data: ResponseData::AdminStatus { snapshot },
            } => Ok(snapshot),
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

    /// Attach to a session as `Owner` (can drive) or `Observer` (read-only
    /// mirror). The server replays the full event log before the Ack, so the
    /// pump picks up the entire transcript on its first drain cycle.
    pub fn attach(&self, session_id: &str, mode: AttachMode) -> io::Result<()> {
        match self.request(Request::Attach {
            session_id: session_id.to_string(),
            mode,
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }

    /// Attach as `Owner`, tolerating the brief window after a *previous*
    /// connection of ours dropped but before the server has finished tearing
    /// it down and releasing ownership. That window is real on an in-place
    /// [`reconnect`](Self::reconnect): the new socket can re-attach before the
    /// server has processed the old socket's EOF, so a bare [`attach`] races
    /// and is rejected with "another GUI already owns this session".
    ///
    /// Retries on *ownership contention only* (any other error is fatal and
    /// returned at once) for a bounded window, then falls back to an `Observer`
    /// attach so the transcript still replays. Returns `Ok(true)` if we became
    /// owner, `Ok(false)` if we fell back to observer (a genuinely live peer
    /// still owns it). Mirrors the open-path retry but lives here so the
    /// reconnect path and tests share one implementation.
    pub fn attach_owner_with_retry(&self, session_id: &str) -> io::Result<bool> {
        // ~1s total (20 × 50ms). The clean-shutdown → server-detach window is
        // sub-millisecond once the socket is closed (see `Drop`), so this
        // almost always succeeds on the first or second try; the budget only
        // covers scheduling jitter under load.
        let mut last = String::new();
        for _ in 0..20 {
            match self.attach(session_id, AttachMode::Owner) {
                Ok(()) => return Ok(true),
                Err(e) => {
                    let msg = e.to_string();
                    if !msg.contains("already own") {
                        return Err(e);
                    }
                    last = msg;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // Still owned after the window: a real peer holds it. Observe instead
        // so we at least receive the replay + live stream.
        self.attach(session_id, AttachMode::Observer).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("{last}; observer fallback failed: {e}"),
            )
        })?;
        Ok(false)
    }

    /// Claim ownership of a session this connection is observing. Succeeds
    /// only once the previous owner has disconnected (server reports the
    /// session as ownerless). Used by a candidate GUI to take over.
    pub fn promote(&self, session_id: &str) -> io::Result<()> {
        match self.request(Request::Promote {
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
        // Fire-and-forget: the server only returns an Ack and we don't
        // need it. Blocking here stalls the GPUI main thread and
        // prevents the server pump from draining reply notifications,
        // making the UI appear to miss messages.
        self.request_fire(Request::Prompt {
            session_id: session_id.to_string(),
            text: text.to_string(),
        })
    }

    /// Interrupt the in-flight turn for `session_id`. Fire-and-forget,
    /// same as [`prompt`] — the server only Acks and blocking would stall
    /// the GPUI main thread.
    pub fn cancel(&self, session_id: &str) -> io::Result<()> {
        self.request_fire(Request::Cancel {
            session_id: session_id.to_string(),
        })
    }

    /// Force-restart the agent subprocess for `session_id` (kill + resume).
    /// Fire-and-forget: the server respawns off-thread and broadcasts a
    /// `SessionAttached` when the replacement is live.
    pub fn restart_session(&self, session_id: &str) -> io::Result<()> {
        self.request_fire(Request::RestartSession {
            session_id: session_id.to_string(),
        })
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

    pub fn rename_session(&self, session_id: &str, label: &str) -> io::Result<()> {
        self.request_fire(Request::RenameSession {
            session_id: session_id.to_string(),
            label: label.to_string(),
        })
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
        self.notification_rx.as_ref()?.try_recv().ok()
    }
}

impl Drop for SessionServerClient {
    fn drop(&mut self) {
        // Dropping `self` drops `request_tx` (once this body returns and the
        // fields fall), which ends the writer thread's recv loop. The writer
        // then FLUSHES every queued frame — including a fire-and-forget prompt
        // sent right before drop/reconnect — and only THEN shuts the socket
        // down (see `from_stream`), which unblocks the detached reader and lets
        // the server see the disconnect. Shutting the socket down *here* would
        // race that flush and silently drop the last prompt, so we don't.
        //
        // The writer only exits once ALL `request_tx` clones are gone; a
        // `SessionServerHandle` holds one, but handles are short-lived
        // (created per off-thread request, never stored), so the connection
        // tears down promptly. We just unblock parked requests and mark dead.
        self.connected.store(false, Ordering::SeqCst);
        fail_all_pending(&self.pending);
    }
}

/// A `Send + Sync`, cloneable handle for issuing session-server requests from
/// a background thread. Created via [`SessionServerClient::handle`]. Holds
/// clones of the writer channel, liveness flag, request-id counter, and the
/// pending-response map, so its blocking requests share the exact same routing
/// and disconnect behaviour as the owning client. It deliberately exposes only
/// the request/response calls the GUI needs to move off the paint thread; the
/// notification stream and reconnect logic remain on the owning client.
#[derive(Clone)]
pub struct SessionServerHandle {
    request_tx: std_mpsc::Sender<(Frame, Option<std_mpsc::Sender<Response>>)>,
    connected: Arc<AtomicBool>,
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<std::collections::HashMap<u64, std_mpsc::Sender<Response>>>>,
}

impl SessionServerHandle {
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Blocking request/response — identical semantics to
    /// [`SessionServerClient::request`] but callable off-thread.
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
            .map_err(|e| match e {
                std_mpsc::RecvTimeoutError::Disconnected => {
                    self.connected.store(false, Ordering::SeqCst);
                    io::Error::new(io::ErrorKind::BrokenPipe, "session server disconnected")
                }
                std_mpsc::RecvTimeoutError::Timeout => {
                    io::Error::new(io::ErrorKind::TimedOut, "request timed out")
                }
            })
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

    pub fn attach(&self, session_id: &str, mode: AttachMode) -> io::Result<()> {
        match self.request(Request::Attach {
            session_id: session_id.to_string(),
            mode,
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

    pub fn close_session(&self, session_id: &str) -> io::Result<()> {
        match self.request(Request::CloseSession {
            session_id: session_id.to_string(),
        })? {
            Response::Ok { .. } => Ok(()),
            Response::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        }
    }
}
