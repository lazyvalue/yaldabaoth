//! Headless transcript harness for the session server.
//!
//! Sibling to `session_resilience_test.rs`. That file points the server at a
//! NO-OP agent (`/usr/bin/true`), so it only covers the bare socket/ownership
//! layer — the session's durable `event_log` stays empty. This file points the
//! server at `sketch-acp-stub` (a real-protocol stub ACP agent, built as a
//! `[[bin]]`), which streams a controllable transcript. That lets us exercise
//! the parts that need a real transcript:
//!
//!   1. a prompt/turn round-trip (chunks stream, turn completes, transcript
//!      lands in the session),
//!   2. **large-replay reconnect** — the path originally (wrongly) suspected as
//!      the reconnect-storm cause: a big `event_log` must replay in full on a
//!      fresh attach after a simulated GUI restart,
//!   3. mid-turn reconnect — drop + reconnect while the agent is still
//!      streaming, then confirm the session is still usable and the transcript
//!      isn't corrupted.
//!
//! Everything is driven through the real `SessionServerClient` and the real
//! `sketch-session-server` binary, so these are end-to-end against the same
//! code the GUI uses.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use sketch::session_client::SessionServerClient;
use sketch::session_proto::{socket_path, AttachMode, Notification};

/// A running server bound to a private socket, pointed at the stub ACP agent.
/// Per-test env knobs (`STUB_CHUNKS`, `STUB_DELAY_MS`, …) shape the transcript
/// the stub produces.
struct TestServer {
    child: Child,
    socket: PathBuf,
    log: PathBuf,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

impl TestServer {
    /// Start a server whose spawned agents are `sketch-acp-stub`, with the
    /// given `(VAR, value)` env knobs applied to the server process (and thus
    /// inherited by every agent it spawns).
    fn start_with_env(knobs: &[(&str, &str)]) -> TestServer {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir();
        let socket = dir.join(format!("sketch-txtest-{pid}-{n}.sock"));
        let log = dir.join(format!("sketch-txtest-{pid}-{n}.log"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&log);
        // Also clear any persisted state file colocated with the socket so a
        // prior run can't restore stale sessions into this fresh server.
        let _ = std::fs::remove_file(socket.with_extension("state.json"));

        let logfile = std::fs::File::create(&log).expect("create server log");
        let server_bin = env!("CARGO_BIN_EXE_sketch-session-server");
        let stub_bin = env!("CARGO_BIN_EXE_sketch-acp-stub");
        let mut cmd = Command::new(server_bin);
        cmd.env("SKETCH_SESSION_SOCKET", &socket)
            .env("SKETCH_ACP_AGENT", stub_bin);
        for (k, v) in knobs {
            cmd.env(k, v);
        }
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(logfile))
            .spawn()
            .expect("spawn sketch-session-server");

        let server = TestServer { child, socket, log };
        server.wait_for_socket();
        server
    }

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

    fn activate_env(&self) {
        // SAFETY: tests in this file run serially (SERIAL mutex below).
        unsafe { std::env::set_var("SKETCH_SESSION_SOCKET", &self.socket) };
        assert_eq!(socket_path(), self.socket);
    }

    fn read_log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Tests share process-wide env (`SKETCH_SESSION_SOCKET`), so serialize them.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Count `ReplyEvent(Chunk)` notifications carrying agent text in a drained
/// batch. This is the per-turn transcript payload the stub produces.
fn count_agent_chunks(notes: &[Notification]) -> usize {
    notes
        .iter()
        .filter(|n| {
            matches!(
                n,
                Notification::ReplyEvent {
                    event: sketch::acp_channel::ReplyEvent::Chunk(_),
                    ..
                }
            )
        })
        .count()
}

/// Drain notifications from `client` until `done(&collected)` returns true or
/// the deadline passes. Returns everything collected. Uses the non-blocking
/// `try_recv` with a short sleep — bounded wait, no fixed total sleep.
fn drain_until<F>(
    client: &SessionServerClient,
    timeout: Duration,
    mut done: F,
) -> Vec<Notification>
where
    F: FnMut(&[Notification]) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();
    loop {
        let mut got_any = false;
        while let Some(note) = client.try_recv() {
            collected.push(note);
            got_any = true;
        }
        if done(&collected) {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        if !got_any {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    collected
}

/// 1. Prompt/turn round-trip. Create a session, attach as Owner, prompt, and
///    verify the streamed chunks arrive and the turn completes (TurnEnded).
///    Then confirm the transcript is durable by re-attaching a fresh client and
///    seeing the same chunks replay.
#[test]
fn prompt_turn_round_trip() {
    let _g = serial_lock();
    const CHUNKS: usize = 5;
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "5")]);
    server.activate_env();

    let client = SessionServerClient::connect().expect("connect");
    let info = client
        .create_session(std::env::temp_dir(), "round-trip".into(), None)
        .expect("create_session");
    client
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");

    // Prompt. The stub streams CHUNKS agent_message_chunks then ends the turn.
    client.prompt(&info.session_id, "hello").expect("prompt");

    // Wait until we've seen a TurnEnded for this session.
    let notes = drain_until(&client, Duration::from_secs(15), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    let chunk_count = count_agent_chunks(&notes);
    assert_eq!(
        chunk_count, CHUNKS,
        "expected {CHUNKS} streamed agent chunks, got {chunk_count}; notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        notes.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == info.session_id && *turn_count >= 1
        )),
        "turn did not complete; notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );

    // Durability: a FRESH client (simulating a second GUI / a reopened panel)
    // attaches and must receive the full transcript replayed from event_log.
    let client2 = SessionServerClient::connect().expect("connect #2");
    client2
        .attach(&info.session_id, AttachMode::Observer)
        .expect("attach observer #2");
    let replay = drain_until(&client2, Duration::from_secs(10), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });
    let replay_chunks = count_agent_chunks(&replay);
    assert_eq!(
        replay_chunks, CHUNKS,
        "replay to a fresh attach must reproduce the full transcript ({CHUNKS} chunks), got {replay_chunks}; \
         replay={replay:#?}\nlog:\n{}",
        server.read_log()
    );
    // The user's prompt is also in the durable log.
    assert!(
        replay.iter().any(|n| matches!(
            n,
            Notification::UserPrompt { text, .. } if text == "hello"
        )),
        "replayed transcript must include the user prompt; replay={replay:#?}",
    );
}

/// 2. Large-replay reconnect. Build a BIG event_log (one turn of many chunks),
///    wait for it to finish, drop the client (simulating a GUI exit), then a
///    fresh client re-attaches and must get the entire transcript replayed.
///    This is the "large attach-replay" path once suspected as the storm cause;
///    here it gets real headless coverage.
#[test]
fn large_replay_reconnect() {
    let _g = serial_lock();
    const CHUNKS: usize = 800;
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "800")]);
    server.activate_env();

    let sid = {
        let client = SessionServerClient::connect().expect("connect #1");
        let info = client
            .create_session(std::env::temp_dir(), "large".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach owner #1");
        client.prompt(&info.session_id, "go big").expect("prompt");

        // Wait until the whole big turn has landed (TurnEnded) AND we've seen
        // all the chunks on this connection. Generous timeout for 800 chunks.
        let notes = drain_until(&client, Duration::from_secs(30), |n| {
            count_agent_chunks(n) >= CHUNKS
                && n.iter().any(|note| {
                    matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
                })
        });
        let got = count_agent_chunks(&notes);
        assert_eq!(
            got, CHUNKS,
            "owner connection should observe all {CHUNKS} chunks live, got {got}\nlog:\n{}",
            server.read_log()
        );
        info.session_id
        // client dropped here → GUI #1 exits. The session + its big event_log
        // live on in the server.
    };

    // Give the server a beat to process the disconnect (release ownership).
    std::thread::sleep(Duration::from_millis(100));

    // GUI #2: fresh client re-attaches. The server replays the full event_log
    // (all 800 chunks + the user prompt + the turn boundary) on attach.
    let client2 = SessionServerClient::connect().expect("connect #2");
    let became_owner = client2
        .attach_owner_with_retry(&sid)
        .expect("re-attach after restart");
    assert!(
        became_owner,
        "fresh attach should reclaim ownership (previous owner released on drop)\nlog:\n{}",
        server.read_log()
    );

    let replay = drain_until(&client2, Duration::from_secs(30), |n| {
        count_agent_chunks(n) >= CHUNKS
            && n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid)
            })
    });
    let replay_chunks = count_agent_chunks(&replay);
    assert_eq!(
        replay_chunks, CHUNKS,
        "large-replay reconnect must replay ALL {CHUNKS} chunks on re-attach, got {replay_chunks}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        replay.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, .. } if *session_id == sid
        )),
        "large replay must include the turn boundary; log:\n{}",
        server.read_log()
    );

    // The session survived and is still usable: prompt again and see a NEW turn
    // complete on top of the replayed history.
    client2.prompt(&sid, "again").expect("prompt after reconnect");
    let after = drain_until(&client2, Duration::from_secs(30), |n| {
        // A second TurnEnded (turn_count >= 2) means the new turn settled.
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2)
        })
    });
    assert!(
        after.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2
        )),
        "session must stay usable after a large-replay reconnect (second turn never completed); \
         after={after:#?}\nlog:\n{}",
        server.read_log()
    );
}

/// 3. Mid-turn reconnect. With a per-chunk delay the turn streams slowly, so we
///    can drop the owner WHILE the agent is still emitting chunks, then a fresh
///    client re-attaches. The turn keeps running server-side; the fresh client
///    must end up with a clean, non-duplicated transcript and a usable session.
#[test]
fn mid_turn_reconnect_no_corruption() {
    let _g = serial_lock();
    const CHUNKS: usize = 40;
    // 25ms/chunk → ~1s turn, plenty of window to disconnect mid-stream.
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "40"), ("STUB_DELAY_MS", "25")]);
    server.activate_env();

    let sid = {
        let client = SessionServerClient::connect().expect("connect #1");
        let info = client
            .create_session(std::env::temp_dir(), "midturn".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach owner #1");
        client.prompt(&info.session_id, "stream slowly").expect("prompt");

        // Wait until SOME chunks have arrived but the turn is NOT done yet, then
        // bail — this drops the client mid-turn.
        let _partial = drain_until(&client, Duration::from_secs(10), |n| {
            count_agent_chunks(n) >= 3
        });
        let saw = count_agent_chunks(&_partial);
        assert!(
            saw >= 3 && saw < CHUNKS,
            "should have dropped mid-turn (saw {saw} of {CHUNKS}); the turn streamed too fast — \
             increase STUB_DELAY_MS\nlog:\n{}",
            server.read_log()
        );
        info.session_id
        // client dropped here → mid-turn disconnect.
    };

    // Fresh client re-attaches while the turn may still be streaming on the
    // server. The server keeps pumping the agent regardless of attach state.
    let client2 = SessionServerClient::connect().expect("connect #2");
    client2
        .attach_owner_with_retry(&sid)
        .expect("re-attach mid-turn");

    // The forwarder tails event_log from index 0 on attach, so this fresh
    // client gets the WHOLE turn (everything streamed before AND after the
    // reconnect), ending with the turn boundary.
    let notes = drain_until(&client2, Duration::from_secs(30), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid)
        })
    });

    // No corruption / no duplication: exactly CHUNKS distinct agent chunks, and
    // the per-chunk index sequence is 0..CHUNKS with no repeats. The stub emits
    // "chunk N", so we can verify ordering/uniqueness precisely.
    let chunk_texts: Vec<String> = notes
        .iter()
        .filter_map(|n| match n {
            Notification::ReplyEvent {
                event: sketch::acp_channel::ReplyEvent::Chunk(t),
                ..
            } => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        chunk_texts.len(),
        CHUNKS,
        "mid-turn reconnect must deliver exactly {CHUNKS} chunks with no loss/dup, got {}; \
         texts={chunk_texts:?}\nlog:\n{}",
        chunk_texts.len(),
        server.read_log()
    );
    let expected: Vec<String> = (0..CHUNKS).map(|i| format!("chunk {i}")).collect();
    assert_eq!(
        chunk_texts, expected,
        "chunk sequence must be in-order and unique after a mid-turn reconnect (no dup/reorder)\nlog:\n{}",
        server.read_log()
    );

    // Session still usable: a follow-up prompt completes a fresh turn.
    client2.prompt(&sid, "still there?").expect("prompt after midturn reconnect");
    let after = drain_until(&client2, Duration::from_secs(30), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2)
        })
    });
    assert!(
        after.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2
        )),
        "session must remain usable after a mid-turn reconnect; after={after:#?}\nlog:\n{}",
        server.read_log()
    );
}
