//! Headless resilience harness for the session server.
//!
//! Goal: reproduce — without the GPUI app — the property the user actually
//! wants: *agent sessions keep running on the server across GUI restarts, with
//! no flapping*. The reconnect logic lives in `SessionServerClient` (the GUI's
//! half) and `sketch-session-server` (the daemon), both drivable from a plain
//! test process. So a "GUI restart" here is just dropping one client and
//! standing up another against the same live server.
//!
//! These tests spawn the REAL `sketch-session-server` binary (via
//! `CARGO_BIN_EXE_sketch-session-server`) on a private socket, so they exercise
//! the same connect/attach/reconnect code paths the app uses. No real ACP agent
//! is required: `create_session` returns immediately and the session stays in
//! the manager's map even if the agent never spawns (the agent only fills the
//! event log; it's not in the connect/reconnect loop). We point
//! `SKETCH_ACP_AGENT` at a no-op so nothing real is launched.
//!
//! The "storm" we're hunting: the GUI client flapping its connection in a tight
//! loop (one field log had 489 reconnects). If that's a socket-layer bug it
//! shows up here as repeated disconnects in the server log under a stable
//! client; if a single client stays cleanly connected and sessions survive
//! repeated client teardown, the storm is elsewhere (agent/pump timing) and
//! these tests pin down that it is NOT the bare socket path.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use sketch::acp_channel::PermissionMode;
use sketch::session_client::SessionServerClient;
use sketch::session_proto::{socket_path, AdminSnapshot, AttachMode};

/// A running server instance bound to a private socket, with its stderr
/// captured to a file we can scan for connect/disconnect lines.
struct TestServer {
    child: Child,
    socket: PathBuf,
    log: PathBuf,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

impl TestServer {
    fn start() -> TestServer {
        // Unique socket + log per test instance. pid is stable per process;
        // SEQ disambiguates multiple servers within one test.
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir();
        let socket = dir.join(format!("sketch-restest-{pid}-{n}.sock"));
        let log = dir.join(format!("sketch-restest-{pid}-{n}.log"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&log);

        let logfile = std::fs::File::create(&log).expect("create server log");
        let bin = env!("CARGO_BIN_EXE_sketch-session-server");
        let child = Command::new(bin)
            .env("SKETCH_SESSION_SOCKET", &socket)
            // Point the agent at a binary that exits immediately. Spawn-fail is
            // fine: the session is created and persists regardless; we are
            // testing the socket/attach/reconnect layer, not the agent.
            .env("SKETCH_ACP_AGENT", "/usr/bin/true")
            // Hermetic: force the no-config path so default-mode assertions
            // see the built-in default (Yolo), not whatever ~/.config/sketch/
            // config.kdl the dev box happens to have. config_path() returns
            // this nonexistent path → Config::default().
            .env("SKETCH_CONFIG", "/nonexistent/sketch-test-config.kdl")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(logfile))
            .spawn()
            .expect("spawn sketch-session-server");

        let server = TestServer { child, socket, log };
        server.wait_for_socket();
        server
    }

    /// Block until the server is accepting connections (socket connectable).
    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::os::unix::net::UnixStream::connect(&self.socket).is_ok() {
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "server never became connectable on {}; log:\n{}",
                    self.socket.display(),
                    self.read_log()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Make this server's socket the one `SessionServerClient::connect()` will
    /// use. The client reads `socket_path()` (env-driven) at connect time.
    /// Tests run single-threaded (see `#[test]` note) so this is safe.
    fn activate_env(&self) {
        // SAFETY: tests in this file run serially (cargo runs each test in its
        // own thread but we guard shared env with the SERIAL mutex below).
        unsafe { std::env::set_var("SKETCH_SESSION_SOCKET", &self.socket) };
        assert_eq!(socket_path(), self.socket);
    }

    fn read_log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Count server-side connection accept lines, split by first-vs-subsequent.
    /// The server logs `client connected` for conn 1 and `client reconnected`
    /// for conn_id > 1 — the exact counters from the storm report.
    fn accept_counts(&self) -> (usize, usize) {
        let log = self.read_log();
        let connected = log.matches("client connected").count();
        let reconnected = log.matches("client reconnected").count();
        (connected, reconnected)
    }

    /// Count connection-close lines the server emits (one per dropped conn).
    fn close_count(&self) -> usize {
        self.read_log().matches("closed after").count()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
        // Clean up durable state (WAL dir + any state file) colocated with the
        // socket so repeated runs don't accumulate.
        let _ = std::fs::remove_file(self.socket.with_extension("state.json"));
        let _ = std::fs::remove_dir_all(self.socket.with_extension("wal"));
        // Leave the log on disk for post-mortem if a test failed; temp dir is
        // cleaned by the OS. (Removing here would hide failures.)
    }
}

/// Serialize tests: they share process-wide env (`SKETCH_SESSION_SOCKET`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lock the serial guard, recovering from a prior test's panic-poisoning so
/// one failing test doesn't cascade into spurious `PoisonError`s in the rest.
fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn drain_log_lines(log: &str) -> Vec<String> {
    log.lines().map(|l| l.to_string()).collect()
}

/// Baseline: a session created on the server survives the client going away
/// and a fresh client re-attaching — i.e. the core "GUI restart" property.
#[test]
fn session_survives_client_restart() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    // GUI #1: connect, create a session, attach as owner.
    let sid = {
        let client = SessionServerClient::connect().expect("connect #1");
        let info = client
            .create_session(std::env::temp_dir(), "restest".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach #1");
        info.session_id
        // client dropped here → simulates GUI #1 exiting.
    };

    // Give the server a beat to notice the disconnect.
    std::thread::sleep(Duration::from_millis(100));

    // GUI #2: fresh client, the session must still be there and re-attachable.
    let client2 = SessionServerClient::connect().expect("connect #2");
    let sessions = client2.list_sessions().expect("list after restart");
    assert!(
        sessions.iter().any(|s| s.session_id == sid),
        "session {sid} vanished after client restart; server log:\n{}",
        server.read_log()
    );
    client2
        .attach(&sid, AttachMode::Owner)
        .expect("re-attach after restart");

    // No storm: a small bounded number of accepts, and crucially every
    // connection that opened also CLOSED (no zombie connections holding stale
    // ownership). `accepts` includes the one-shot `wait_for_socket` probe, so
    // we assert the balance/bound rather than an exact split.
    drop(client2);
    std::thread::sleep(Duration::from_millis(100));
    let (connected, reconnected) = server.accept_counts();
    let accepts = connected + reconnected;
    let closes = server.close_count();
    assert!(
        accepts <= 4,
        "unexpected reconnect storm: {accepts} accepts; log:\n{}",
        server.read_log()
    );
    assert_eq!(
        closes, accepts,
        "every connection must close (no zombies); accepts={accepts} closes={closes}; log:\n{}",
        server.read_log()
    );
}

/// Stress: many sequential client restarts (the thing that produced 489
/// reconnects in the field). Each "restart" is connect → attach → drop. The
/// session must survive all of them and the server must see exactly one accept
/// per restart — no flapping, no failed round-trips.
#[test]
fn repeated_restarts_no_storm() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let sid = {
        let client = SessionServerClient::connect().expect("initial connect");
        let info = client
            .create_session(std::env::temp_dir(), "storm".into(), None)
            .expect("create_session");
        info.session_id
    };

    const RESTARTS: usize = 30;
    for i in 0..RESTARTS {
        let client = SessionServerClient::connect()
            .unwrap_or_else(|e| panic!("connect on restart {i} failed: {e}"));
        // A close/create-style round-trip is what failed in the field when it
        // landed in a "down" window. list_sessions is the same request shape.
        let sessions = client
            .list_sessions()
            .unwrap_or_else(|e| panic!("list_sessions on restart {i} failed: {e}"));
        assert!(
            sessions.iter().any(|s| s.session_id == sid),
            "session lost on restart {i}; log:\n{}",
            server.read_log()
        );
        // Owner re-attach with retry — exactly what the GUI does on restart.
        let became_owner = client
            .attach_owner_with_retry(&sid)
            .unwrap_or_else(|e| panic!("attach on restart {i} failed: {e}"));
        assert!(
            became_owner,
            "restart {i} fell back to observer — previous owner never released; log:\n{}",
            server.read_log()
        );
        // client drops → next iteration is a fresh "GUI".
    }

    std::thread::sleep(Duration::from_millis(100));

    // No flapping: accept count is bounded by the restarts we performed (plus
    // the initial client and the one-shot wait_for_socket probe) — NOT the
    // hundreds seen in the field storm. And every connection closed: a leaked
    // (zombie) connection would show as accepts > closes.
    let (connected, reconnected) = server.accept_counts();
    let accepts = connected + reconnected;
    let closes = server.close_count();
    let expected = RESTARTS + 2; // initial client + probe + one per restart
    assert!(
        accepts <= expected + 1,
        "accept count {accepts} exceeds expected ~{expected} — flapping; log:\n{}",
        server.read_log()
    );
    assert!(
        closes >= accepts - 1,
        "connections leaked (zombies): accepts={accepts} closes={closes}; log:\n{}",
        server.read_log()
    );
}

/// Single-instance guard: a second server started against a socket that's
/// already live must exit cleanly WITHOUT stealing the socket — otherwise it
/// would orphan the first server's running sessions. This is the duplicate-
/// server fork that the client's auto-launch-on-failed-connect could trigger.
#[test]
fn second_server_does_not_steal_socket() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    // Create a session on the live server.
    let sid = {
        let client = SessionServerClient::connect().expect("connect");
        let info = client
            .create_session(std::env::temp_dir(), "guard".into(), None)
            .expect("create_session");
        info.session_id
    };

    // Start a SECOND server on the very same socket. It must detect the live
    // one and exit cleanly rather than rebinding.
    let bin = env!("CARGO_BIN_EXE_sketch-session-server");
    let mut intruder = Command::new(bin)
        .env("SKETCH_SESSION_SOCKET", &server.socket)
        .env("SKETCH_ACP_AGENT", "/usr/bin/true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn intruder server");
    let status = {
        // Bounded wait for the intruder to exit on its own.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match intruder.try_wait().expect("try_wait") {
                Some(s) => break s,
                None if Instant::now() > deadline => {
                    let _ = intruder.kill();
                    panic!("second server did not exit — it likely stole the socket");
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    };
    assert!(status.success(), "intruder server exited non-zero: {status:?}");

    // The intruder's stderr should say it deferred to the running server.
    let mut err = String::new();
    if let Some(mut e) = intruder.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    assert!(
        err.contains("already listening"),
        "intruder should report deferring to the live server; stderr:\n{err}"
    );

    // The ORIGINAL server is intact and still owns the session.
    let client = SessionServerClient::connect().expect("reconnect to original");
    let sessions = client.list_sessions().expect("list after intruder");
    assert!(
        sessions.iter().any(|s| s.session_id == sid),
        "original session lost — socket was stolen; log:\n{}",
        server.read_log()
    );
}

/// Default-mode contract at the wire level: with no config file present (tests
/// run without SKETCH_CONFIG), a freshly created session comes back in the
/// no-config default mode, which is now `Yolo` (the default was reverted from
/// the safe modes pending an inline-approval UI; see ADR-0014 addendum). The
/// owner can then explicitly change the mode and that change must be reflected
/// in the session's reported mode — pinning the owner-driven mode contract end
/// to end. (Mode changes remain owner-gated; see
/// `non_owner_cannot_change_permission_mode`.)
#[test]
fn new_session_defaults_to_safe_permission_mode() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client = SessionServerClient::connect().expect("connect");
    let info = client
        .create_session(std::env::temp_dir(), "perms".into(), None)
        .expect("create_session");
    assert_eq!(
        info.permission_mode,
        PermissionMode::Yolo,
        "new session must start in the no-config default mode (Yolo); log:\n{}",
        server.read_log()
    );

    // A mode change is an owner action. Attach as Owner first (the server only
    // honours mode changes from the owner), then flip to a safe mode.
    client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    client
        .set_permission_mode(&info.session_id, PermissionMode::ReadOnly)
        .expect("set_permission_mode ReadOnly");

    // The new mode must be reflected in the session metadata.
    let sessions = client.list_sessions().expect("list after change");
    let s = sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session present after change");
    assert_eq!(
        s.permission_mode,
        PermissionMode::ReadOnly,
        "explicit owner mode change not reflected; log:\n{}",
        server.read_log()
    );
}

/// The 0600 contract, pinned (not just code-read): the server socket must be
/// owner-only the moment it is connectable, so no other local user can reach
/// the session-driving surface. Asserts the actual on-disk mode bits.
#[test]
fn server_socket_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let _g = serial_lock();
    let server = TestServer::start();
    let mode = std::fs::metadata(&server.socket)
        .expect("stat socket")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "session-server socket must be 0600 (owner-only), got {mode:o}; log:\n{}",
        server.read_log()
    );
}

/// The escalation gate is owner-only: a connection that does NOT own the
/// session cannot flip its permission mode. Pins the other half of the
/// "safe by default, only the owner escalates" contract.
#[test]
fn non_owner_cannot_change_permission_mode() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client = SessionServerClient::connect().expect("connect");
    let info = client
        .create_session(std::env::temp_dir(), "perms-gate".into(), None)
        .expect("create_session");

    // No Owner attach → not the owner → the mode change must be rejected, and
    // the mode must stay at the no-config default (Yolo). Aim at a *different*
    // mode so the rejection is observable rather than a no-op.
    let res = client.set_permission_mode(&info.session_id, PermissionMode::ReadOnly);
    assert!(
        res.is_err(),
        "non-owner was allowed to change permission mode; log:\n{}",
        server.read_log()
    );
    let sessions = client.list_sessions().expect("list");
    let s = sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session present");
    assert_eq!(
        s.permission_mode,
        PermissionMode::Yolo,
        "rejected change must leave the no-config default intact; log:\n{}",
        server.read_log()
    );
}

/// The diagnostic `admin_status` verb: a read-only snapshot of every managed
/// session's live server-side state. Asserts the snapshot is empty before any
/// session exists, tracks created sessions (with the safe default permission
/// mode), and reflects ownership + subscriber count once an Owner attaches.
#[test]
fn admin_status_reports_live_sessions() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client = SessionServerClient::connect().expect("connect");

    // Empty to start.
    let snap: AdminSnapshot = client.admin_status().expect("admin_status (empty)");
    assert_eq!(
        snap.session_count, 0,
        "fresh server must report zero sessions; log:\n{}",
        server.read_log()
    );
    assert!(snap.sessions.is_empty(), "sessions vec must be empty too");

    // Create two sessions.
    let s1 = client
        .create_session(std::env::temp_dir(), "admin-a".into(), None)
        .expect("create_session a");
    let s2 = client
        .create_session(std::env::temp_dir(), "admin-b".into(), None)
        .expect("create_session b");

    let snap = client.admin_status().expect("admin_status (two)");
    assert_eq!(
        snap.session_count, 2,
        "both sessions must appear in the snapshot; log:\n{}",
        server.read_log()
    );
    assert_eq!(snap.sessions.len(), 2);

    // Each created session is present with the safe default permission mode and
    // a present (>= 0) event-log length field.
    for sid in [&s1.session_id, &s2.session_id] {
        let entry = snap
            .sessions
            .iter()
            .find(|e| &e.session_id == sid)
            .unwrap_or_else(|| {
                panic!(
                    "created session {sid} missing from snapshot; log:\n{}",
                    server.read_log()
                )
            });
        assert_eq!(
            entry.permission_mode,
            PermissionMode::Yolo,
            "snapshot must report the no-config default permission mode (Yolo); log:\n{}",
            server.read_log()
        );
        // A freshly-created session (agent is /usr/bin/true → no events) has an
        // empty transcript; the snapshot must report that, not a stale/garbage len.
        assert_eq!(
            entry.event_log_len, 0,
            "fresh session should have an empty event log; log:\n{}",
            server.read_log()
        );
    }

    // Attach as Owner to one session, then re-snapshot: that entry must report
    // ownership and at least one active broadcast subscriber.
    client
        .attach(&s1.session_id, AttachMode::Owner)
        .expect("attach owner");
    // Give the server a beat to register the subscriber/forwarder.
    std::thread::sleep(Duration::from_millis(100));

    let snap = client.admin_status().expect("admin_status (after attach)");
    let owned = snap
        .sessions
        .iter()
        .find(|e| e.session_id == s1.session_id)
        .expect("owned session present after attach");
    assert!(
        owned.has_owner,
        "owned session must report has_owner == true; log:\n{}",
        server.read_log()
    );
    assert!(
        owned.subscriber_count >= 1,
        "owned session must report >= 1 subscriber after Owner attach, got {}; log:\n{}",
        owned.subscriber_count,
        server.read_log()
    );
}

/// The in-place `reconnect()` path the GUI pump uses (not a brand-new client).
/// This is the exact code that runs when the server reader thread sees EOF and
/// the pump rebuilds the connection. It must succeed against a live server and
/// leave the session attachable.
#[test]
fn in_place_reconnect_reattaches() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let mut client = SessionServerClient::connect().expect("connect");
    let info = client
        .create_session(std::env::temp_dir(), "reconn".into(), None)
        .expect("create_session");
    client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach");

    // Drive the in-place reconnect the pump uses. Against a live server this
    // must rebuild cleanly and hand back fresh receivers.
    let result = client.reconnect();
    assert!(
        result.is_ok(),
        "in-place reconnect failed: {:?}; log:\n{}",
        result.err(),
        server.read_log()
    );
    assert!(client.is_connected(), "client not connected after reconnect");

    // Re-attach as Owner on the rebuilt connection. The old connection's
    // teardown races this, so the retry helper (what the GUI uses) is required:
    // a bare attach here is the exact call that failed with "another GUI
    // already owns this session" before the Drop-shutdown + retry fix.
    let became_owner = client
        .attach_owner_with_retry(&info.session_id)
        .expect("re-attach after in-place reconnect");
    assert!(
        became_owner,
        "reconnect re-attach fell back to observer — old owner never released; log:\n{}",
        server.read_log()
    );

    // Surface the log for eyeballing even on success when run with --nocapture.
    let lines = drain_log_lines(&server.read_log());
    eprintln!("--- server log ({} lines) ---", lines.len());
    for l in &lines {
        eprintln!("{l}");
    }
}
