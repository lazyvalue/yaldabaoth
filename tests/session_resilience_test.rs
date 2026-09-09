//! Headless resilience harness for the session server.
//!
//! Goal: reproduce — without the GPUI app — the property the user actually
//! wants: *agent sessions keep running on the server across GUI restarts, with
//! no flapping*. The reconnect logic lives in `SessionServerClient` (the GUI's
//! half) and `yalda-session-server` (the daemon), both drivable from a plain
//! test process. So a "GUI restart" here is just dropping one client and
//! standing up another against the same live server.
//!
//! These tests spawn the REAL `yalda-session-server` binary (via
//! `CARGO_BIN_EXE_yalda-session-server`) on a private socket, so they exercise
//! the same connect/attach/reconnect code paths the app uses. No real ACP agent
//! is required: `create_session` returns immediately and the session stays in
//! the manager's map even if the agent never spawns (the agent only fills the
//! event log; it's not in the connect/reconnect loop). We point
//! `YALDA_ACP_AGENT` at a no-op so nothing real is launched.
//!
//! The "storm" we're hunting: the GUI client flapping its connection in a tight
//! loop (one field log had 489 reconnects). If that's a socket-layer bug it
//! shows up here as repeated disconnects in the server log under a stable
//! client; if a single client stays cleanly connected and sessions survive
//! repeated client teardown, the storm is elsewhere (agent/pump timing) and
//! these tests pin down that it is NOT the bare socket path.

use std::path::PathBuf;
use std::os::unix::fs::MetadataExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use yalda::acp_channel::PermissionMode;
use yalda::session_client::SessionServerClient;
use yalda::session_proto::{AdminSnapshot, Notification, socket_path};

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
        // Default agent is a no-op (`/usr/bin/true`): most tests here exercise
        // the socket/attach/reconnect layer, where the agent never spawns a
        // real transcript.
        Self::spawn_with_agent("/usr/bin/true", &[])
    }

    /// Start a server whose spawned agents are the real `yalda-acp-stub`, with
    /// the given `(VAR, value)` env knobs (e.g. `STUB_CHUNKS`) applied to the
    /// server process and inherited by every agent it spawns. Used by the one
    /// test that needs a real streamed transcript to assert reconnect REPLAY,
    /// not just reconnect success.
    fn start_with_stub_agent(knobs: &[(&str, &str)]) -> TestServer {
        Self::spawn_with_agent(env!("CARGO_BIN_EXE_yalda-acp-stub"), knobs)
    }

    fn spawn_with_agent(agent: &str, knobs: &[(&str, &str)]) -> TestServer {
        // Unique socket + log per test instance. pid is stable per process;
        // SEQ disambiguates multiple servers within one test.
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir();
        let socket = dir.join(format!("yalda-restest-{pid}-{n}.sock"));
        let log = dir.join(format!("yalda-restest-{pid}-{n}.log"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&log);

        let logfile = std::fs::File::create(&log).expect("create server log");
        let bin = env!("CARGO_BIN_EXE_yalda-session-server");
        let mut builder = Command::new(bin);
        builder
            .env("YALDA_SESSION_SOCKET", &socket)
            .env("YALDA_ACP_AGENT", agent)
            // Hermetic: force the no-config path so default-mode assertions
            // see the built-in default (Yolo), not whatever ~/.config/yalda/
            // config.kdl the dev box happens to have. config_path() returns
            // this nonexistent path → Config::default().
            .env("YALDA_CONFIG", "/nonexistent/yalda-test-config.kdl");
        for (k, v) in knobs {
            builder.env(k, v);
        }
        // `--force` skips the Highlander single-process guard: test servers run
        // on private sockets, deliberately side-by-side with each other AND
        // with whatever live server the dev box is running. The guard itself is
        // exercised by the dedicated `highlander_*` tests below.
        builder.arg("--force");
        let child = builder
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(logfile))
            .spawn()
            .expect("spawn yalda-session-server");

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
        unsafe { std::env::set_var("YALDA_SESSION_SOCKET", &self.socket) };
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
        let _ = std::fs::remove_file(self.socket.with_extension("pid"));
        let _ = std::fs::remove_dir_all(self.socket.with_extension("wal"));
        // Leave the log on disk for post-mortem if a test failed; temp dir is
        // cleaned by the OS. (Removing here would hide failures.)
    }
}

/// Connect a client. Under the strict 1:1 model there is no client_id / lease,
/// so this is just a thin wrapper over `connect()`. The `_label` arg is ignored
/// (kept so call sites that documented "which GUI" don't have to change).
fn connect_as(_label: &str) -> SessionServerClient {
    SessionServerClient::connect().expect("connect")
}

/// Serialize tests: they share process-wide env (`YALDA_SESSION_SOCKET`).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lock the serial guard, recovering from a prior test's panic-poisoning so
/// one failing test doesn't cascade into spurious `PoisonError`s in the rest.
fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn drain_log_lines(log: &str) -> Vec<String> {
    log.lines().map(|l| l.to_string()).collect()
}

#[test]
fn competing_server_cannot_steal_socket_or_touch_owner_state() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let socket_before = std::fs::metadata(&server.socket)
        .expect("owner socket metadata")
        .ino();
    let bin = env!("CARGO_BIN_EXE_yalda-session-server");
    // `--force` gets past the Highlander process scan so this test still
    // exercises the LEASE layer specifically.
    let output = Command::new(bin)
        .arg("--force")
        .env("YALDA_SESSION_SOCKET", &server.socket)
        .env("YALDA_ACP_AGENT", "/usr/bin/false")
        .env("YALDA_CONFIG", "/nonexistent/yalda-test-config.kdl")
        .output()
        .expect("launch competing server");
    assert!(output.status.success(), "loser should exit cleanly: {output:?}");
    let loser_log = String::from_utf8_lossy(&output.stderr);
    assert!(
        loser_log.contains("owns the lifetime lease"),
        "loser did not refuse at the lifetime lease: {loser_log}"
    );

    let socket_after = std::fs::metadata(&server.socket)
        .expect("competing server must not unlink owner socket")
        .ino();
    assert_eq!(socket_after, socket_before, "owner socket inode was replaced");
    let client = connect_as("still-owner");
    client.list_sessions().expect("original owner still serves requests");
}

/// Drain server notifications into a `Vec` until `done(accumulated)` holds or
/// the deadline passes. Returns everything drained. Mirrors the transcript
/// harness so reconnect-replay can be content-asserted here too.
fn drain_until<F>(client: &SessionServerClient, timeout: Duration, mut done: F) -> Vec<Notification>
where
    F: FnMut(&[Notification]) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut out: Vec<Notification> = Vec::new();
    loop {
        while let Some(n) = client.try_recv() {
            out.push(n);
        }
        if done(&out) || Instant::now() > deadline {
            return out;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Like [`drain_until`] but over a raw notification receiver handed back by
/// `SessionServerClient::reconnect()` (which TAKES the client's internal
/// receiver and returns it to the caller — the GUI's pump re-installs it; a test
/// must read from this returned handle, since `client.try_recv()` no longer has
/// a receiver after reconnect).
fn drain_rx_until<F>(
    rx: &std::sync::mpsc::Receiver<Notification>,
    timeout: Duration,
    mut done: F,
) -> Vec<Notification>
where
    F: FnMut(&[Notification]) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut out: Vec<Notification> = Vec::new();
    loop {
        while let Ok(n) = rx.try_recv() {
            out.push(n);
        }
        if done(&out) || Instant::now() > deadline {
            return out;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Count `ReplyEvent(Chunk)` notifications carrying agent text in a drained
/// batch — the per-turn transcript payload the stub produces.
fn count_agent_chunks(notes: &[Notification]) -> usize {
    notes
        .iter()
        .filter(|n| {
            matches!(
                n,
                Notification::ReplyEvent {
                    event: yalda::acp_channel::ReplyEvent::Chunk(_),
                    ..
                }
            )
        })
        .count()
}

/// Whether a drained batch contains a `TurnEnded` for `sid`.
fn saw_turn_ended(notes: &[Notification], sid: &str) -> bool {
    notes
        .iter()
        .any(|n| matches!(n, Notification::TurnEnded { session_id, .. } if session_id == sid))
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
        client.attach(&info.session_id).expect("attach #1");
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
    client2.attach(&sid).expect("re-attach after restart");

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
        closes,
        accepts,
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
    // Each restart models a SINGLE GUI reconnecting: connect → attach → drop.
    // Under the strict 1:1 model a successful attach is just `()`; the property
    // under test is the absence of a reconnect storm, not lease semantics.
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
        // Deterministic single-shot attach — exactly what the GUI now does on
        // restart.
        client
            .attach(&sid)
            .unwrap_or_else(|e| panic!("attach on restart {i} failed: {e}"));
        // client drops → socket EOF; the next iteration re-attaches cleanly.
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
    let bin = env!("CARGO_BIN_EXE_yalda-session-server");
    // `--force` gets past the Highlander process scan so this test still
    // exercises the LEASE layer specifically.
    let mut intruder = Command::new(bin)
        .arg("--force")
        .env("YALDA_SESSION_SOCKET", &server.socket)
        .env("YALDA_ACP_AGENT", "/usr/bin/true")
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
    assert!(
        status.success(),
        "intruder server exited non-zero: {status:?}"
    );

    // The intruder's stderr should say it failed at the lifetime lease, before
    // it could inspect/unlink the socket or touch WAL/agent state.
    let mut err = String::new();
    if let Some(mut e) = intruder.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    assert!(
        err.contains("owns the lifetime lease"),
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
/// run without YALDA_CONFIG), a freshly created session comes back in the
/// no-config default mode, which is now `Yolo` (the default was reverted from
/// the safe modes pending an inline-approval UI; see ADR-0014 addendum). The
/// attached client can then explicitly change the mode and that change must be
/// reflected in the session's reported mode — pinning the mode contract end to
/// end.
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

    // Attach, then flip to a safe mode.
    client.attach(&info.session_id).expect("attach");
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
        mode,
        0o600,
        "session-server socket must be 0600 (owner-only), got {mode:o}; log:\n{}",
        server.read_log()
    );
}

/// The diagnostic `admin_status` verb: a read-only snapshot of every managed
/// session's live server-side state. Asserts the snapshot is empty before any
/// session exists, tracks created sessions (with the safe default permission
/// mode), and reflects the subscriber count once a client attaches.
#[test]
fn admin_status_reports_live_sessions() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client = connect_as("gui-admin");

    // Empty to start.
    let snap: AdminSnapshot = client.admin_status().expect("admin_status (empty)");
    assert_eq!(
        snap.session_count,
        0,
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
        snap.session_count,
        2,
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
            entry.event_log_len,
            0,
            "fresh session should have an empty event log; log:\n{}",
            server.read_log()
        );
    }

    // Attach to one session, then re-snapshot: that entry must report the
    // single attached client's forwarder as a subscriber.
    client.attach(&s1.session_id).expect("attach");
    // Give the server a beat to register the subscriber/forwarder.
    std::thread::sleep(Duration::from_millis(100));

    let snap = client.admin_status().expect("admin_status (after attach)");
    let attached = snap
        .sessions
        .iter()
        .find(|e| e.session_id == s1.session_id)
        .expect("attached session present after attach");
    assert!(
        attached.subscriber_count >= 1,
        "attached session must report >= 1 subscriber after attach, got {}; log:\n{}",
        attached.subscriber_count,
        server.read_log()
    );
}

/// Archive is a server lifecycle transition, not a GUI filter: the WAL file is
/// retained while its append descriptor and ACP runtime are released, the state
/// appears in both list/admin responses, and unarchive reopens the durable log.
#[test]
fn archive_is_cold_and_reversible_over_the_real_wire() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();
    let client = connect_as("gui-archive");
    let created = client
        .create_session(std::env::temp_dir(), "cold-wire".into(), None)
        .expect("create archive fixture");
    let wal_dir = server.socket.with_extension("wal");
    assert_eq!(
        std::fs::read_dir(&wal_dir).expect("WAL dir").count(),
        1,
        "created session owns one durable WAL"
    );

    client
        .set_archived(&created.session_id, true)
        .expect("archive request");
    let listed = client.list_sessions().expect("list after archive");
    let cold = listed
        .iter()
        .find(|info| info.session_id == created.session_id)
        .expect("archived session remains listed");
    assert!(cold.archived);
    assert!(!cold.connected);
    let admin = client.admin_status().expect("admin after archive");
    let cold_admin = admin
        .sessions
        .iter()
        .find(|info| info.session_id == created.session_id)
        .expect("archived session remains managed");
    assert!(cold_admin.archived);
    assert!(!cold_admin.wal_open, "archive closes the append descriptor");
    assert_eq!(
        std::fs::read_dir(&wal_dir).expect("WAL dir").count(),
        1,
        "archive retains the transcript file"
    );

    client
        .set_archived(&created.session_id, false)
        .expect("unarchive request");
    let admin = client.admin_status().expect("admin after unarchive");
    let live = admin
        .sessions
        .iter()
        .find(|info| info.session_id == created.session_id)
        .expect("unarchived session remains managed");
    assert!(!live.archived);
    assert!(live.wal_open, "unarchive reopens the append descriptor");

    client
        .set_archived(&created.session_id, true)
        .expect("rearchive before close");
    client
        .close_session(&created.session_id)
        .expect("close archived session");
    assert_eq!(
        std::fs::read_dir(&wal_dir).expect("WAL dir").count(),
        0,
        "explicit close deletes a cold session's retained WAL"
    );
}

/// The in-place `reconnect()` path the GUI pump uses (not a brand-new client).
/// This is the exact code that runs when the server reader thread sees EOF and
/// the pump rebuilds the connection — the LIVE GUI's actual reconnect mechanism.
///
/// It must not only succeed: after the reconnect + re-attach it must REPLAY the
/// full durable transcript without gap or duplication (the property a GUI
/// depends on to rebuild its panel after a flap). So we drive a real streamed
/// turn first, then reconnect-in-place and assert the entire event_log replays
/// — mirroring what `large_replay_reconnect` / `prompt_turn_round_trip` assert
/// for the drop-and-fresh-connect path.
#[test]
fn in_place_reconnect_reattaches() {
    let _g = serial_lock();
    const CHUNKS: usize = 6;
    let server = TestServer::start_with_stub_agent(&[("STUB_CHUNKS", "6")]);
    server.activate_env();

    let mut client = connect_as("gui-reconn");
    let info = client
        .create_session(std::env::temp_dir(), "reconn".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    client.attach(&sid).expect("attach");

    // Drive a real turn: the stub streams CHUNKS chunks then ends the turn.
    client.prompt(&sid, "hello").expect("prompt");
    let live = drain_until(&client, Duration::from_secs(15), |n| {
        saw_turn_ended(n, &sid)
    });
    let live_chunks = count_agent_chunks(&live);
    assert_eq!(
        live_chunks,
        CHUNKS,
        "live turn must stream all {CHUNKS} chunks before reconnect, got {live_chunks}; log:\n{}",
        server.read_log()
    );

    // Drive the in-place reconnect the pump uses. Against a live server this
    // must rebuild cleanly and HAND BACK the fresh notification receiver — the
    // GUI's pump re-takes it; here the test drains it directly (the client's
    // internal `try_recv` receiver is the one handed out by `reconnect`, so we
    // must read from the returned `note_rx`, exactly like the pump does).
    let (note_rx, _wake_rx) = client.reconnect().unwrap_or_else(|e| {
        panic!(
            "in-place reconnect failed: {e}; log:\n{}",
            server.read_log()
        )
    });
    assert!(
        client.is_connected(),
        "client not connected after reconnect"
    );

    // Re-attach on the rebuilt connection. The server replays the full event_log
    // from index 0, so the rebuilt pump rebuilds the whole transcript.
    client
        .attach(&sid)
        .expect("re-attach after in-place reconnect");

    // The replay must reproduce the ENTIRE transcript: exactly CHUNKS chunks
    // (no gap, no duplication) plus the turn boundary. Drain the receiver the
    // reconnect handed back.
    let replay = drain_rx_until(&note_rx, Duration::from_secs(15), |n| {
        saw_turn_ended(n, &sid)
    });
    let replay_chunks = count_agent_chunks(&replay);
    assert_eq!(
        replay_chunks,
        CHUNKS,
        "in-place reconnect replay must reproduce the full transcript ({CHUNKS} chunks, \
         no gap/dup), got {replay_chunks}; replay={replay:#?}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        saw_turn_ended(&replay, &sid),
        "replay must include the turn boundary; log:\n{}",
        server.read_log()
    );

    // Surface the log for eyeballing even on success when run with --nocapture.
    let lines = drain_log_lines(&server.read_log());
    eprintln!("--- server log ({} lines) ---", lines.len());
    for l in &lines {
        eprintln!("{l}");
    }
}

/// Headless AdminPrompt works with NO client attached (ADR-0015). A non-GUI
/// caller (`connect_existing`, never attaches) drives a session via
/// `admin_prompt` both before AND after a normal client attaches — the headless
/// enqueue path has no attach/ownership precondition under the strict 1:1 model.
#[test]
fn admin_prompt_works_with_no_client_attached() {
    let _g = serial_lock();
    let server = TestServer::start();
    server.activate_env();

    let client = connect_as("gui-A");
    let info = client
        .create_session(std::env::temp_dir(), "headless".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();

    // Headless caller: connect_existing never attaches.
    let cli = SessionServerClient::connect_existing().expect("connect_existing");
    cli.admin_prompt(&sid, "go")
        .expect("admin_prompt with nobody attached");

    // Now a normal client attaches; a headless admin_prompt still succeeds.
    client.attach(&sid).expect("attach");
    cli.admin_prompt(&sid, "again")
        .expect("admin_prompt still works after a client attaches");
}

/// Upgrade boundary: pre-seed the WAL dir with a hand-written v1 log, start the
/// current server, and prove the production startup path retains the durable
/// session identity instead of silently discarding it.
#[test]
fn v1_wal_session_survives_server_upgrade() {
    let _g = serial_lock();
    // Build the socket + WAL dir paths the way TestServer would, but seed the
    // WAL dir BEFORE the server starts.
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    let socket = dir.join(format!("yalda-restest-{pid}-{n}-v1.sock"));
    let wal_dir = socket.with_extension("wal");
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    std::fs::create_dir_all(&wal_dir).expect("mk wal dir");
    let stale = wal_dir.join("stale-session.log");
    std::fs::write(
        &stale,
        concat!(
            r#"{"t":"header","version":1,"server_session_id":"stale-session","label":"old","cwd":"/tmp","permission_mode":"Yolo"}"#,
            "\n",
            r#"{"t":"event","type":"session_attached","session_id":"stale-session","acp_session_id":"acp-old"}"#,
            "\n",
        ),
    )
    .expect("seed v1 wal");

    // Start the v2 server against this socket (manually, so the seeded WAL dir
    // is in place first).
    unsafe { std::env::set_var("YALDA_SESSION_SOCKET", &socket) };
    let log = socket.with_extension("log");
    let logfile = std::fs::File::create(&log).expect("server log");
    let bin = env!("CARGO_BIN_EXE_yalda-session-server");
    let mut child = Command::new(bin)
        .arg("--force")
        .env("YALDA_SESSION_SOCKET", &socket)
        .env("YALDA_ACP_AGENT", "/usr/bin/true")
        .env("YALDA_CONFIG", "/nonexistent/yalda-test-config.kdl")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(logfile))
        .spawn()
        .expect("spawn server");

    // Wait for socket, then assert the historical session was recovered.
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
    let recovered = sessions
        .iter()
        .find(|s| s.session_id == "stale-session")
        .unwrap_or_else(|| panic!("v1 WAL session vanished during upgrade; sessions={sessions:?}"));
    assert_eq!(recovered.acp_session_id.as_deref(), Some("acp-old"));

    // Cleanup.
    drop(client);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    let _ = std::fs::remove_file(&log);
}

/// bug-0064 (2026-09-09): one WAL carrying durable interior corruption — the
/// production shape left by the Sep 1–2 twin-writer split-brain: a complete
/// record, then a newline-terminated orphan tail fragment, then more complete
/// records — must NOT crash-loop the whole server. This drives the REAL
/// startup path (`restore_seed_from_disk` → `try_recover_each`) of the real
/// binary: the server must come up, publish every other session, skip the
/// corrupt one loudly, and leave its bytes untouched.
#[test]
fn interior_corrupt_wal_is_skipped_and_the_server_still_boots() {
    let _g = serial_lock();
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    let socket = dir.join(format!("yalda-restest-{pid}-{n}-corrupt.sock"));
    let wal_dir = socket.with_extension("wal");
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    std::fs::create_dir_all(&wal_dir).expect("mk wal dir");

    let header = |sid: &str| {
        format!(
            r#"{{"t":"header","version":3,"server_session_id":"{sid}","label":"{sid}","cwd":"/tmp","permission_mode":"Yolo","provider":"claude"}}"#
        )
    };
    let attached = |sid: &str| {
        format!(
            r#"{{"t":"event","type":"session_attached","session_id":"{sid}","acp_session_id":"acp-{sid}"}}"#
        )
    };
    // Sorted FIRST so the corrupt file is met before the valid one — exactly
    // the production order (0b319323 failed with the rest of the roster
    // still unread).
    let corrupt_path = wal_dir.join("00-corrupt-session.log");
    let corrupt_bytes = format!(
        "{}\n{}\n{}\n{}\n",
        header("00-corrupt-session"),
        attached("00-corrupt-session"),
        // Verbatim a1204e30 line 11811: the surviving tail of a record whose
        // head the concurrent appender clobbered.
        r#"pected":false},"toolName":"Bash"}}}}}"#,
        attached("00-corrupt-session"),
    );
    std::fs::write(&corrupt_path, &corrupt_bytes).expect("seed corrupt wal");
    let valid_path = wal_dir.join("99-valid-session.log");
    std::fs::write(
        &valid_path,
        format!(
            "{}\n{}\n",
            header("99-valid-session"),
            attached("99-valid-session")
        ),
    )
    .expect("seed valid wal");

    unsafe { std::env::set_var("YALDA_SESSION_SOCKET", &socket) };
    let log = socket.with_extension("log");
    let logfile = std::fs::File::create(&log).expect("server log");
    let bin = env!("CARGO_BIN_EXE_yalda-session-server");
    let mut child = Command::new(bin)
        .arg("--force")
        .env("YALDA_SESSION_SOCKET", &socket)
        .env("YALDA_ACP_AGENT", "/usr/bin/true")
        .env("YALDA_CONFIG", "/nonexistent/yalda-test-config.kdl")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(logfile))
        .spawn()
        .expect("spawn server");

    // The bug's exact symptom is the process exiting during recovery, so a
    // dead child is reported as such (with its log) rather than as a timeout.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "server exited during recovery ({status}) instead of skipping the corrupt WAL; log:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!(
                "server never came up; log:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let client = SessionServerClient::connect().expect("connect");
    let sessions = client.list_sessions().expect("list");
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert!(
        ids.contains(&"99-valid-session"),
        "the healthy session must be recovered alongside a corrupt neighbour; got {ids:?}"
    );
    assert!(
        !ids.contains(&"00-corrupt-session"),
        "a corrupt WAL must not be advertised as a recovered session; got {ids:?}"
    );
    // Retained byte-for-byte for manual repair, and the skip is loud.
    assert_eq!(
        std::fs::read(&corrupt_path).unwrap(),
        corrupt_bytes.as_bytes()
    );
    let server_log = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        server_log.contains("SKIPPING CORRUPT WAL")
            && server_log.contains("00-corrupt-session.log")
            && server_log.contains("malformed WAL record at line 3"),
        "the skip must name the file and the line; log:\n{server_log}"
    );

    drop(client);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&wal_dir);
    let _ = std::fs::remove_file(&log);
}

// ── Highlander guard (bug-0064 recurrence, graph f5x) ──────────────────────
//
// The 2026-09-01 split-brain: a frozen GUI auto-launched a second server from
// a stale pre-lease binary, which stole the socket pathname. The lease only
// excludes servers that cooperate in taking it — the Highlander guard is the
// process-scan backstop that refuses to boot while ANY other
// yalda-session-server process exists, regardless of build, socket, or lease.

/// A server started WITHOUT `--force` while another server process exists must
/// refuse to boot: clean exit(0), the refusal logged, and its socket never
/// created. The other server here is on a DIFFERENT private socket, proving
/// the refusal comes from the process scan — not the socket or lease guards.
#[test]
fn highlander_refuses_boot_while_another_server_process_exists() {
    let _g = serial_lock();
    // Guarantee at least one live server process exists (hermetic on CI; on a
    // dev box the user's real server would match too).
    let other = TestServer::start();

    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let socket = std::env::temp_dir().join(format!("yalda-restest-{pid}-{n}-highlander.sock"));
    let _ = std::fs::remove_file(&socket);

    let bin = env!("CARGO_BIN_EXE_yalda-session-server");
    // Bounded wait, NOT `.output()`: a regressed (guard-less) server BOOTS and
    // runs forever, and this test must fail cleanly instead of hanging.
    let mut guarded = Command::new(bin)
        .env("YALDA_SESSION_SOCKET", &socket)
        .env("YALDA_ACP_AGENT", "/usr/bin/true")
        .env("YALDA_CONFIG", "/nonexistent/yalda-test-config.kdl")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch guarded server");
    let status = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match guarded.try_wait().expect("try_wait") {
                Some(s) => break s,
                None if Instant::now() > deadline => {
                    let _ = guarded.kill();
                    let _ = guarded.wait();
                    panic!(
                        "guarded server did not exit — the Highlander guard \
                         let it boot alongside a live server"
                    );
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    };
    assert!(
        status.success(),
        "Highlander refusal must be a CLEAN exit (systemd Restart=on-failure \
         must not respawn-loop it): {status:?}"
    );
    let mut log = String::new();
    if let Some(mut e) = guarded.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut log);
    }
    assert!(
        log.contains("Highlander"),
        "expected the Highlander refusal in stderr; got:\n{log}"
    );
    assert!(
        !socket.exists(),
        "refusing server must never create its socket"
    );
    drop(other);
}

/// `--force` really overrides the guard: a forced server boots to a
/// connectable socket while another server process is running.
#[test]
fn highlander_force_boots_side_by_side() {
    let _g = serial_lock();
    let a = TestServer::start();
    let b = TestServer::start(); // start() passes --force and waits for the socket
    assert!(
        std::os::unix::net::UnixStream::connect(&b.socket).is_ok(),
        "forced second server must be connectable"
    );
    drop(b);
    drop(a);
}

/// ADR-0037: `SessionServerClient::connect()` must NEVER launch a server. With
/// no server on the socket it fails fast with a connect error, the socket file
/// stays absent, and no yalda-session-server process appears. (Before the
/// bug-0064 recurrence fix, this call spawned a detached server from PATH —
/// the exact mechanism that booted a stale pre-lease binary into a
/// split-brain.)
#[test]
fn client_connect_never_launches_a_server() {
    let _g = serial_lock();
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let socket = std::env::temp_dir().join(format!("yalda-restest-{pid}-{n}-nolaunch.sock"));
    let _ = std::fs::remove_file(&socket);
    unsafe { std::env::set_var("YALDA_SESSION_SOCKET", &socket) };

    let before = Instant::now();
    let result = SessionServerClient::connect();
    let elapsed = before.elapsed();

    assert!(result.is_err(), "connect() with no server must fail");
    assert!(
        elapsed < Duration::from_secs(2),
        "connect() must fail FAST, not wait out a launch-retry backoff \
         (took {elapsed:?})"
    );
    // The old auto-launch path spawned a server that would create the socket
    // within its startup window. Give any such regression time to manifest.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !socket.exists(),
        "a server appeared on the socket — connect() launched one"
    );
}
