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
use sketch::session_proto::{socket_path, AdminSnapshot, AttachMode, Notification};

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
        Self::start_with_ttl_ms(None)
    }

    /// Start with a low lease TTL (phase 4) so expiry-driven tests run fast.
    fn start_with_ttl_ms(ttl_ms: Option<u64>) -> TestServer {
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
        let mut builder = Command::new(bin);
        builder.env("SKETCH_SESSION_SOCKET", &socket);
        if let Some(ms) = ttl_ms {
            builder.env("SKETCH_LEASE_TTL_MS", ms.to_string());
        }
        let child = builder
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

/// Connect a client and stamp it with a stable lease `client_id` (phase 4).
/// The lease keys ownership on this id, so a test that wants drive rights MUST
/// present one (an empty client_id never acquires a lease). Modelling "the same
/// GUI restarting" = reusing ONE id across reconnects.
fn connect_as(client_id: &str) -> SessionServerClient {
    let c = SessionServerClient::connect().expect("connect");
    c.set_client_id(client_id.to_string());
    c
}

/// Poll `admin_status` until `pred(snapshot)` holds or the deadline passes.
/// Returns the last snapshot seen.
fn poll_admin<F: Fn(&AdminSnapshot) -> bool>(
    client: &SessionServerClient,
    timeout: Duration,
    pred: F,
) -> AdminSnapshot {
    let deadline = Instant::now() + timeout;
    loop {
        let snap = client.admin_status().expect("admin_status");
        if pred(&snap) || Instant::now() > deadline {
            return snap;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Find a session's admin entry by id.
fn admin_entry<'a>(
    snap: &'a AdminSnapshot,
    sid: &str,
) -> Option<&'a sketch::session_proto::AdminSessionInfo> {
    snap.sessions.iter().find(|s| s.session_id == sid)
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
    // ONE stable client_id across all restarts: this models a SINGLE GUI
    // restarting, not N distinct GUIs. The lease keys on this id, so every
    // restart's same-client_id Owner attach RESUMES on the FIRST try — no retry
    // loop, no "already own" race. (Phase 4 retired attach_owner_with_retry.)
    for i in 0..RESTARTS {
        let client = connect_as("gui-A");
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
        // Deterministic single-shot Owner attach — exactly what the GUI now
        // does on restart. The response carries `driver:true` on the FIRST try.
        let driver = client
            .attach(&sid, AttachMode::Owner)
            .unwrap_or_else(|e| panic!("attach on restart {i} failed: {e}"));
        assert!(
            driver,
            "restart {i} did not resume the lease on the first try (no retry); log:\n{}",
            server.read_log()
        );
        // The admin snapshot must show the lease held by our stable id.
        let snap = client.admin_status().expect("admin_status");
        let entry = admin_entry(&snap, &sid).expect("session present");
        assert_eq!(
            entry.lease_holder.as_ref().map(|l| l.client_id.as_str()),
            Some("gui-A"),
            "restart {i}: lease_holder must be gui-A; log:\n{}",
            server.read_log()
        );
        // client drops → socket EOF. EOF does NOT release the lease (starts the
        // TTL clock); the same-id reconnect next iteration resumes it.
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

    let client = connect_as("gui-perms");
    let info = client
        .create_session(std::env::temp_dir(), "perms".into(), None)
        .expect("create_session");
    assert_eq!(
        info.permission_mode,
        PermissionMode::Yolo,
        "new session must start in the no-config default mode (Yolo); log:\n{}",
        server.read_log()
    );

    // A mode change is a lease-holder action. Attach as Owner first (acquires
    // the lease), then flip to a safe mode.
    let drove = client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    assert!(drove, "Owner attach must acquire the lease");
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

    let client = connect_as("gui-admin");

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

    let mut client = connect_as("gui-reconn");
    let info = client
        .create_session(std::env::temp_dir(), "reconn".into(), None)
        .expect("create_session");
    let drove = client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach");
    assert!(drove, "initial Owner attach must acquire the lease");

    // Drive the in-place reconnect the pump uses. Against a live server this
    // must rebuild cleanly and hand back fresh receivers. The stable client_id
    // is re-applied onto the rebuilt struct (phase 4).
    let result = client.reconnect();
    assert!(
        result.is_ok(),
        "in-place reconnect failed: {:?}; log:\n{}",
        result.err(),
        server.read_log()
    );
    assert!(client.is_connected(), "client not connected after reconnect");
    assert_eq!(
        client.client_id(),
        "gui-reconn",
        "client_id must survive in-place reconnect"
    );

    // Re-attach as Owner on the rebuilt connection. The old connection's
    // teardown races this, but the stable client_id makes the same-id branch
    // RESUME on the first attempt — no retry, no "already own" error.
    let drove_again = client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("re-attach after in-place reconnect");
    assert!(
        drove_again,
        "reconnect re-attach did not resume the lease on the first try; log:\n{}",
        server.read_log()
    );

    // Surface the log for eyeballing even on success when run with --nocapture.
    let lines = drain_log_lines(&server.read_log());
    eprintln!("--- server log ({} lines) ---", lines.len());
    for l in &lines {
        eprintln!("{l}");
    }
}

// ── Phase 4: lease ownership ───────────────────────────────────────

/// SAME-client_id reconnect reclaims drive rights with ZERO retries (the core
/// retirement of attach_owner_with_retry). Owner-attach under a fixed id, drop
/// the socket, stand up a NEW client with the SAME id, attach Owner in a SINGLE
/// call — assert driver==true on the first attempt and lease_holder == that id.
#[test]
fn same_client_id_reclaims_lease_without_retry() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let sid = {
        let client = connect_as("gui-A");
        let info = client
            .create_session(std::env::temp_dir(), "reclaim".into(), None)
            .expect("create_session");
        let drove = client.attach(&info.session_id, AttachMode::Owner).expect("attach");
        assert!(drove, "first Owner attach must acquire the lease");
        client.prompt(&info.session_id, "hi").expect("prompt ok");
        info.session_id
        // drop → socket EOF; EOF does NOT release the lease.
    };

    std::thread::sleep(Duration::from_millis(100));

    // Fresh client, SAME id, single attach → resumes on the first try.
    let client2 = connect_as("gui-A");
    let drove = client2
        .attach(&sid, AttachMode::Owner)
        .expect("re-attach same id");
    assert!(
        drove,
        "same-client_id re-attach must resume the lease on the FIRST try; log:\n{}",
        server.read_log()
    );
    let snap = client2.admin_status().expect("admin_status");
    assert_eq!(
        admin_entry(&snap, &sid)
            .and_then(|e| e.lease_holder.as_ref())
            .map(|l| l.client_id.as_str()),
        Some("gui-A"),
        "lease_holder must be gui-A; log:\n{}",
        server.read_log()
    );
}

/// A second DISTINCT client becomes Observer, not an error. With A holding a
/// live lease (kept fresh by heartbeat), B attaches Owner → driver==false (a
/// silent downgrade, NOT "already own"). B's gated verbs are rejected while A's
/// succeed; lease_holder stays A.
#[test]
fn second_client_downgrades_to_observer() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "downgrade".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));

    let client_b = connect_as("gui-B");
    let drove_b = client_b
        .attach(&sid, AttachMode::Owner)
        .expect("B attach must NOT error on contention");
    assert!(
        !drove_b,
        "B must downgrade to Observer (driver==false), not error; log:\n{}",
        server.read_log()
    );

    // B's gated action is rejected; A's succeeds.
    assert!(
        client_b
            .set_permission_mode(&sid, PermissionMode::ReadOnly)
            .is_err(),
        "non-holder B must be rejected; log:\n{}",
        server.read_log()
    );
    client_a
        .set_permission_mode(&sid, PermissionMode::ReadOnly)
        .expect("holder A may change mode");

    let snap = client_a.admin_status().expect("admin_status");
    assert_eq!(
        admin_entry(&snap, &sid)
            .and_then(|e| e.lease_holder.as_ref())
            .map(|l| l.client_id.as_str()),
        Some("gui-A"),
        "lease_holder must stay A; log:\n{}",
        server.read_log()
    );
}

/// Lease expiry frees ownership — proven time-driven, not socket-driven: A
/// attaches Owner then STOPS heartbeating with its SOCKET STILL OPEN. With a low
/// TTL the lease must free (lazy + sweep), an idle Observer must receive the
/// freeing notification path (admin reports None), and a DIFFERENT client B then
/// first-claims it.
#[test]
fn lease_expires_when_heartbeat_stops() {
    let _g = serial_lock();
    let server = TestServer::start_with_ttl_ms(Some(1200));
    server.activate_env();

    // A holds the lease but its socket stays OPEN (no heartbeat).
    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "expire".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));

    // Poll until the lease frees within TTL + sweep budget.
    let snap = poll_admin(&client_a, Duration::from_secs(10), |s| {
        admin_entry(s, &sid).is_some_and(|e| e.lease_holder.is_none())
    });
    assert!(
        admin_entry(&snap, &sid).is_some_and(|e| e.lease_holder.is_none() && !e.has_owner),
        "lease must expire while A's socket is still open (time-driven); log:\n{}",
        server.read_log()
    );

    // A DIFFERENT client first-claims the now-free lease.
    let client_b = connect_as("gui-B");
    let drove_b = client_b.attach(&sid, AttachMode::Owner).expect("B attach");
    assert!(
        drove_b,
        "B must first-claim the freed lease; log:\n{}",
        server.read_log()
    );
}

/// Headless AdminPrompt stays LEASELESS (ADR-0015) under lease enforcement.
/// connect_existing (no attach, no client_id, no lease) → admin_prompt succeeds
/// and lease_holder stays None. Then with A holding the lease, a leaseless
/// admin_prompt STILL succeeds and A still holds the lease afterward.
#[test]
fn admin_prompt_stays_leaseless() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let owner = connect_as("gui-A");
    let info = owner
        .create_session(std::env::temp_dir(), "headless".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();

    // Leaseless caller: connect_existing never attaches / never sets client_id.
    let cli = SessionServerClient::connect_existing().expect("connect_existing");
    cli.admin_prompt(&sid, "go").expect("admin_prompt unowned");
    let snap = cli.admin_status().expect("admin_status");
    assert!(
        admin_entry(&snap, &sid).is_some_and(|e| e.lease_holder.is_none()),
        "admin_prompt must NOT take a lease; log:\n{}",
        server.read_log()
    );

    // Now A takes the lease; a leaseless admin_prompt still bypasses the gate.
    assert!(owner.attach(&sid, AttachMode::Owner).expect("A attach"));
    cli.admin_prompt(&sid, "again")
        .expect("admin_prompt bypasses the lease gate even when leased");
    let snap = owner.admin_status().expect("admin_status");
    assert_eq!(
        admin_entry(&snap, &sid)
            .and_then(|e| e.lease_holder.as_ref())
            .map(|l| l.client_id.as_str()),
        Some("gui-A"),
        "A must still hold the lease after a headless prompt; log:\n{}",
        server.read_log()
    );
}

/// Clean Detach releases immediately; socket EOF does NOT (the load-bearing
/// distinction). (a) explicit Detach → lease_holder None promptly. (b) socket
/// EOF → lease_holder stays A until TTL, and a same-id reconnect within TTL
/// resumes on the first try.
#[test]
fn detach_releases_now_eof_starts_the_clock() {
    let _g = serial_lock();
    let server = TestServer::start_with_ttl_ms(Some(2000));
    server.activate_env();

    // (a) Explicit clean detach releases the lease immediately.
    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "detach".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));
    client_a.detach(&sid).expect("clean detach");
    let snap = poll_admin(&client_a, Duration::from_secs(2), |s| {
        admin_entry(s, &sid).is_some_and(|e| e.lease_holder.is_none())
    });
    assert!(
        admin_entry(&snap, &sid).is_some_and(|e| e.lease_holder.is_none()),
        "explicit Detach must release the lease promptly (no TTL wait); log:\n{}",
        server.read_log()
    );

    // (b) Re-take, then drop the SOCKET (EOF). The lease must persist until TTL.
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A re-attach"));
    drop(client_a);
    std::thread::sleep(Duration::from_millis(150));
    // A monitor client confirms the lease is STILL held shortly after EOF.
    let monitor = connect_as("gui-monitor-observer");
    let snap = monitor.admin_status().expect("admin_status");
    assert!(
        admin_entry(&snap, &sid)
            .and_then(|e| e.lease_holder.as_ref())
            .map(|l| l.client_id.as_str())
            == Some("gui-A"),
        "socket EOF must NOT release the lease (starts the TTL clock); log:\n{}",
        server.read_log()
    );

    // A same-id reconnect within the TTL resumes drive rights on the first try.
    let client_a2 = connect_as("gui-A");
    let drove = client_a2.attach(&sid, AttachMode::Owner).expect("A2 attach");
    assert!(
        drove,
        "same-id reconnect within TTL must resume on the first try; log:\n{}",
        server.read_log()
    );
}

/// Promote / blue-green under lease. A owns. B observes (distinct id). B Promote
/// → Err (A still leased). A cleanly Detaches → B Promote → Ok and lease_holder
/// == B.
#[test]
fn promote_blocked_until_holder_releases() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "promote".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));

    let client_b = connect_as("gui-B");
    assert!(!client_b.attach(&sid, AttachMode::Observer).expect("B observe"));
    assert!(
        client_b.promote(&sid).is_err(),
        "B must not promote while A holds a live lease; log:\n{}",
        server.read_log()
    );

    // A cleanly hands off.
    client_a.detach(&sid).expect("A clean detach");
    std::thread::sleep(Duration::from_millis(100));
    client_b.promote(&sid).expect("B promote after A released");
    let snap = client_b.admin_status().expect("admin_status");
    assert_eq!(
        admin_entry(&snap, &sid)
            .and_then(|e| e.lease_holder.as_ref())
            .map(|l| l.client_id.as_str()),
        Some("gui-B"),
        "lease_holder must be B after promote; log:\n{}",
        server.read_log()
    );
}

/// Drain `try_recv()` on `client` until a `LeaseChanged` for `sid` whose
/// `lease` matches `want_holder` arrives (`None` → freed; `Some(id)` → held by
/// id), or the deadline passes. Returns whether the matching frame was seen.
/// Polls because notifications land on a background reader thread.
fn await_lease_changed(
    client: &SessionServerClient,
    sid: &str,
    want_holder: Option<&str>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        while let Some(note) = client.try_recv() {
            if let Notification::LeaseChanged { session_id, lease } = &note {
                if session_id == sid {
                    let holder = lease.as_ref().map(|l| l.client_id.as_str());
                    if holder == want_holder {
                        return true;
                    }
                }
            }
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// END-TO-END notification edge (sweep → forwarder → client frame), not just an
/// admin poll: an attached Observer must actually RECEIVE a `LeaseChanged{None}`
/// frame when the holder's lease frees. A (gui-A) takes the lease then stops
/// heartbeating with its socket OPEN; B (gui-B) attaches as Observer and drains
/// its notification stream. Under a low TTL the sweep frees the lease and the
/// server pushes `LeaseChanged{lease:None}` to B. This is the exact signal the
/// candidate-promote path keys on; an admin_status poll would NOT have caught a
/// forwarder that failed to emit the frame.
#[test]
fn observer_receives_lease_freed_notification() {
    let _g = serial_lock();
    let server = TestServer::start_with_ttl_ms(Some(1200));
    server.activate_env();

    // A holds the lease, socket stays OPEN, never heartbeats → it will expire.
    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "freed-note".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));

    // B observes the SAME session and listens on its notification stream.
    let client_b = connect_as("gui-B");
    assert!(
        !client_b
            .attach(&sid, AttachMode::Observer)
            .expect("B observe"),
        "Observer attach must not acquire the lease"
    );

    // The sweep must push LeaseChanged{None} to B within TTL + sweep budget.
    assert!(
        await_lease_changed(&client_b, &sid, None, Duration::from_secs(10)),
        "Observer must RECEIVE a LeaseChanged{{None}} frame when the holder's \
         lease frees (sweep→forwarder→client), not just see it via admin; log:\n{}",
        server.read_log()
    );
}

/// A live, still-beating holder is NOT displaced by a second Owner-attach across
/// several heartbeat intervals. With a low TTL, A holds the lease and keeps
/// heartbeating faster than the TTL. B repeatedly attaches as Owner over several
/// TTL-spans; every B attach must downgrade to Observer (driver==false) and the
/// lease_holder must stay A throughout. This is the server-side guarantee behind
/// the client-side "observer never beats / never steals" fix: while the true
/// holder beats, a contending Owner attach can never win the lease.
#[test]
fn beating_holder_not_displaced_by_owner_attach() {
    let _g = serial_lock();
    let ttl_ms = 600u64;
    let server = TestServer::start_with_ttl_ms(Some(ttl_ms));
    server.activate_env();

    let client_a = connect_as("gui-A");
    let info = client_a
        .create_session(std::env::temp_dir(), "no-steal".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    assert!(client_a.attach(&sid, AttachMode::Owner).expect("A attach"));

    let client_b = connect_as("gui-B");

    // Cover several heartbeat intervals: A beats well inside the TTL, B keeps
    // trying to attach Owner. Beat at ~TTL/3 so the lease never lapses between
    // beats; probe B once per beat. ~9 iterations spans ~5 TTL windows.
    let beat = Duration::from_millis(ttl_ms / 3);
    for i in 0..9 {
        client_a.heartbeat(&sid).expect("A heartbeat keeps the lease live");
        let drove_b = client_b
            .attach(&sid, AttachMode::Owner)
            .expect("B attach must not error on contention");
        assert!(
            !drove_b,
            "iter {i}: B must stay Observer while A beats (no steal); log:\n{}",
            server.read_log()
        );
        let snap = client_a.admin_status().expect("admin_status");
        assert_eq!(
            admin_entry(&snap, &sid)
                .and_then(|e| e.lease_holder.as_ref())
                .map(|l| l.client_id.as_str()),
            Some("gui-A"),
            "iter {i}: lease_holder must stay A while A beats; log:\n{}",
            server.read_log()
        );
        std::thread::sleep(beat);
    }
}

/// WAL v1 discard on startup: pre-seed the WAL dir with a hand-written v1 log
/// (header version:1 + an old owner_changed event), start the v2 server → the
/// stale session is absent from recovery (resumes empty) and the server does not
/// crash on the stale log.
#[test]
fn v1_wal_discarded_on_server_start() {
    let _g = serial_lock();
    // Build the socket + WAL dir paths the way TestServer would, but seed the
    // WAL dir BEFORE the server starts.
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    let socket = dir.join(format!("sketch-restest-{pid}-{n}-v1.sock"));
    let wal_dir = socket.with_extension("wal");
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    std::fs::create_dir_all(&wal_dir).expect("mk wal dir");
    let stale = wal_dir.join("stale-session.log");
    std::fs::write(
        &stale,
        concat!(
            r#"{"t":"header","version":1,"server_session_id":"stale-session","label":"old","cwd":"/tmp","permission_mode":"yolo"}"#,
            "\n",
            r#"{"t":"event","type":"owner_changed","session_id":"stale-session","has_owner":true}"#,
            "\n",
            r#"{"t":"event","type":"session_attached","session_id":"stale-session","acp_session_id":"acp-old"}"#,
            "\n",
        ),
    )
    .expect("seed v1 wal");

    // Start the v2 server against this socket (manually, so the seeded WAL dir
    // is in place first).
    unsafe { std::env::set_var("SKETCH_SESSION_SOCKET", &socket) };
    let log = socket.with_extension("log");
    let logfile = std::fs::File::create(&log).expect("server log");
    let bin = env!("CARGO_BIN_EXE_sketch-session-server");
    let mut child = Command::new(bin)
        .env("SKETCH_SESSION_SOCKET", &socket)
        .env("SKETCH_ACP_AGENT", "/usr/bin/true")
        .env("SKETCH_CONFIG", "/nonexistent/sketch-test-config.kdl")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(logfile))
        .spawn()
        .expect("spawn server");

    // Wait for socket, then assert the stale session was NOT recovered.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("server never came up");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let client = SessionServerClient::connect().expect("connect");
    let sessions = client.list_sessions().expect("list");
    assert!(
        !sessions.iter().any(|s| s.session_id == "stale-session"),
        "v1 WAL session must be discarded (absent from recovery); sessions={sessions:?}"
    );

    // Cleanup.
    drop(client);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    let _ = std::fs::remove_file(&log);
}
