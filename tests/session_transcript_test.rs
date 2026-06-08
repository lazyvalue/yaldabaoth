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
        // Clear any durable state colocated with this socket so a prior run
        // can't restore stale sessions into this fresh server.
        let _ = std::fs::remove_file(socket.with_extension("state.json"));
        let _ = std::fs::remove_dir_all(socket.with_extension("wal"));
        Self::spawn_on(socket, log, knobs)
    }

    /// Spawn a server process bound to `socket`, stderr → `log`, agents =
    /// `sketch-acp-stub`, with the given env knobs. Does NOT clear durable
    /// state — callers that want a fresh start clear it first.
    fn spawn_on(socket: PathBuf, log: PathBuf, knobs: &[(&str, &str)]) -> TestServer {
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

    /// SIGKILL the server and reap it, **leaving the socket + WAL on disk** so a
    /// successor can recover — simulates a hard crash (no graceful shutdown).
    fn crash(&mut self) {
        let _ = self.child.kill(); // std Child::kill = SIGKILL on Unix
        let _ = self.child.wait();
    }

    /// Start a fresh server process on the SAME socket + WAL dir (call after
    /// `crash`). The new server's single-instance guard sees the stale,
    /// unconnectable socket, clears it, and recovers sessions from the WAL.
    fn respawn(&self, knobs: &[(&str, &str)]) -> TestServer {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let log = self.log.with_extension(format!("respawn-{n}.log"));
        Self::spawn_on(self.socket.clone(), log, knobs)
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
        // Clean up durable state so repeated runs don't accumulate. (For a
        // crash/respawn pair both TestServers share these paths; double-remove
        // is harmless.)
        let _ = std::fs::remove_file(self.socket.with_extension("state.json"));
        let _ = std::fs::remove_dir_all(self.socket.with_extension("wal"));
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

/// 4. The literal feature: an agent turn RUNS TO COMPLETION with NO GUI attached.
///    The owner prompts and immediately leaves; for the entire turn there are
///    zero connections to the server. The turn must still complete and the full
///    transcript must be durable, so a GUI that attaches later sees the finished
///    work. This is "agents keep running when no GUI is attached," proven end to
///    end (not just "the session record survives a restart").
#[test]
fn turn_completes_with_no_subscriber_attached() {
    let _g = serial_lock();
    const CHUNKS: usize = 30;
    // ~20ms/chunk → ~600ms turn. We wait far longer than that with NOBODY
    // connected, so completion provably happens with no subscriber.
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "30"), ("STUB_DELAY_MS", "20")]);
    server.activate_env();

    let sid = {
        let client = SessionServerClient::connect().expect("connect owner");
        let info = client
            .create_session(std::env::temp_dir(), "headless".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach owner");
        // Send the prompt, then leave IMMEDIATELY — before the turn can finish.
        // `prompt` is a round-trip, so on return the agent already has the work.
        client.prompt(&info.session_id, "run with nobody watching").expect("prompt");
        info.session_id
        // client dropped here → the GUI is gone. From now until the attach far
        // below, there are ZERO connections to the server.
    };

    // No GUI attached. Wait comfortably past the turn duration (~600ms) so the
    // turn must complete with no subscriber. The per-session pump drains the
    // agent and appends to event_log regardless of attach state.
    std::thread::sleep(Duration::from_secs(3));

    // A GUI shows up only now. It must see the COMPLETED turn — every chunk plus
    // the turn boundary — replayed from the durable log.
    let gui = SessionServerClient::connect().expect("late connect");
    let sessions = gui.list_sessions().expect("list");
    assert!(
        sessions.iter().any(|s| s.session_id == sid),
        "session vanished while no GUI was attached; log:\n{}",
        server.read_log()
    );
    gui.attach(&sid, AttachMode::Owner).expect("attach after the fact");

    let replay = drain_until(&gui, Duration::from_secs(10), |n| {
        count_agent_chunks(n) >= CHUNKS
            && n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid)
            })
    });
    let chunks = count_agent_chunks(&replay);
    assert_eq!(
        chunks, CHUNKS,
        "turn must complete with no GUI attached: expected {CHUNKS} chunks in the durable log, \
         got {chunks}; replay={replay:#?}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        replay.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 1
        )),
        "the turn must have ENDED while unattended (no TurnEnded in the durable log); \
         replay={replay:#?}\nlog:\n{}",
        server.read_log()
    );
}

/// 4b. HEADLESS START-WORK (ADR-0015). A non-GUI caller enqueues a prompt to an
///     EXISTING session it does NOT own, via the ungated `admin_prompt` path,
///     and the agent runs the turn to completion with no owner ever attached.
///     This proves the owner-gate-free enqueue actually drives the agent — the
///     literal "start work headlessly" feature, distinct from "finish a turn the
///     owner started" (test #4).
#[test]
fn admin_prompt_drives_turn_without_owner() {
    let _g = serial_lock();
    const CHUNKS: usize = 4;
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", &CHUNKS.to_string())]);
    server.activate_env();

    // Create the session but do NOT attach as owner — it stays unowned.
    let client = SessionServerClient::connect().expect("connect");
    let info = client
        .create_session(std::env::temp_dir(), "headless-start".into(), None)
        .expect("create_session");

    // Sanity: nobody owns it (admin_status reports owner=None).
    let snap = client.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    assert!(
        !s.has_owner && s.owner_conn_id.is_none(),
        "precondition: session must be UNOWNED before the headless prompt; snapshot={snap:#?}"
    );

    // The ungated enqueue. A normal `prompt` here would be rejected ("only the
    // session owner can send prompts") because this client never attached as
    // owner — `admin_prompt` skips that gate.
    client
        .admin_prompt(&info.session_id, "hello")
        .expect("admin_prompt drives an unowned session");

    // Prove the turn actually ran: attach a FRESH observer (read-only, takes no
    // ownership) and drain the replayed durable transcript for the user prompt
    // we enqueued plus the agent reply and the turn boundary.
    let observer = SessionServerClient::connect().expect("connect observer");
    observer
        .attach(&info.session_id, AttachMode::Observer)
        .expect("attach observer");
    let notes = drain_until(&observer, Duration::from_secs(15), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    assert!(
        notes.iter().any(|n| matches!(
            n,
            Notification::UserPrompt { text, .. } if text == "hello"
        )),
        "the headless-enqueued user prompt must be in the durable transcript; notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );
    let chunks = count_agent_chunks(&notes);
    assert_eq!(
        chunks, CHUNKS,
        "the agent must have streamed its reply ({CHUNKS} chunks) for the headless prompt, got {chunks}; \
         notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        notes.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == info.session_id && *turn_count >= 1
        )),
        "the headless prompt must have driven a turn to completion (turns >= 1); notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );

    // Cross-check the actor's own turn count too (independent of the replay).
    let snap2 = client.admin_status().expect("admin_status after turn");
    let s2 = snap2
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session still present");
    assert!(
        s2.turns >= 1,
        "server-side turn count must be >= 1 after the headless prompt drove a turn; snapshot={snap2:#?}"
    );
}

/// 5. CRASH RECOVERY (ADR-0009). The decisive "agents run with no GUI" test:
///    a turn completes, the server is SIGKILL'd with NO graceful shutdown (the
///    old clean-shutdown-only JSON snapshot would lose everything here), a fresh
///    server starts on the same socket + WAL dir, and the session plus its full
///    transcript must be recovered from the durable write-ahead log.
#[test]
fn session_recovered_after_server_crash() {
    let _g = serial_lock();
    const CHUNKS: usize = 6;
    let mut server = TestServer::start_with_env(&[("STUB_CHUNKS", "6")]);
    server.activate_env();

    let sid = {
        let client = SessionServerClient::connect().expect("connect");
        let info = client
            .create_session(std::env::temp_dir(), "crashtest".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach");
        client
            .prompt(&info.session_id, "survive a crash")
            .expect("prompt");
        // Wait for the turn to fully complete — TurnEnded is a WAL fsync
        // boundary, so after this the transcript is durably on disk.
        let notes = drain_until(&client, Duration::from_secs(15), |n| {
            n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
            })
        });
        assert_eq!(
            count_agent_chunks(&notes),
            CHUNKS,
            "turn must complete before the crash; log:\n{}",
            server.read_log()
        );
        info.session_id
    };

    // HARD CRASH — SIGKILL, no graceful shutdown, no chance to persist.
    server.crash();

    // Fresh server on the SAME socket + WAL dir. Its single-instance guard
    // clears the stale socket and recovery replays the WAL.
    let server2 = server.respawn(&[("STUB_CHUNKS", "6")]);

    // The recovered session must reappear (recovery runs at startup before the
    // accept loop, but be tolerant of scheduling).
    let client2 = SessionServerClient::connect().expect("connect after crash");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut present = false;
    while Instant::now() < deadline {
        if let Ok(sessions) = client2.list_sessions() {
            if sessions.iter().any(|s| s.session_id == sid) {
                present = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        present,
        "session was not recovered from the WAL after a hard crash; log:\n{}",
        server2.read_log()
    );

    // Attach and confirm the FULL pre-crash transcript replays from the WAL.
    client2
        .attach_owner_with_retry(&sid)
        .expect("re-attach recovered session");
    let replay = drain_until(&client2, Duration::from_secs(10), |n| {
        count_agent_chunks(n) >= CHUNKS
            && n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid)
            })
    });
    assert_eq!(
        count_agent_chunks(&replay),
        CHUNKS,
        "WAL must recover all {CHUNKS} agent chunks after a hard crash, got {}; log:\n{}",
        count_agent_chunks(&replay),
        server2.read_log()
    );
    assert!(
        replay.iter().any(|n| matches!(
            n,
            Notification::UserPrompt { text, .. } if text == "survive a crash"
        )),
        "the user's prompt must survive the crash; replay={replay:#?}"
    );
    assert!(
        replay.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, .. } if *session_id == sid
        )),
        "the completed turn boundary must survive the crash"
    );
}

/// 6. SLOW-SUBSCRIBER DISCONNECT (phase-7 liveness hardening). A subscriber
///    whose socket stops draining must NOT be able to park its forwarder task +
///    fd forever. The forwarder bounds every socket write by
///    `SKETCH_SLOW_SUB_TIMEOUT_MS`; when a non-draining peer's OS send buffer
///    fills, the write stalls past the timeout and the server drops that
///    subscriber — while the healthy OWNER is completely unaffected.
///
///    Setup: a LARGE burst (STUB_CHUNKS=2000 × a long STUB_CHUNK_TEXT) so the
///    forwarder must push enough bytes to overflow a non-draining socket's
///    kernel send buffer. The owner is a normal client (reads continuously).
///    The stuck subscriber is a RAW `UnixStream` that attaches as Observer and
///    then NEVER reads. With the timeout dropped to 500ms the test is fast.
#[test]
fn slow_subscriber_is_disconnected_owner_unaffected() {
    use std::io::Write as _;
    use std::os::unix::net::UnixStream as StdUnixStream;

    let _g = serial_lock();
    const CHUNKS: usize = 2000;
    // A long per-chunk text: 2000 chunks × ~200 chars each is ~400KB+ of frames,
    // comfortably more than a default socket send buffer, so a non-draining peer
    // fills up and the forwarder's write_all blocks → trips the 500ms timeout.
    let long_text = "x".repeat(200);
    let server = TestServer::start_with_env(&[
        ("STUB_CHUNKS", "2000"),
        ("STUB_CHUNK_TEXT", &long_text),
        ("SKETCH_SLOW_SUB_TIMEOUT_MS", "500"),
    ]);
    server.activate_env();

    // Owner: a normal client that reads continuously.
    let owner = SessionServerClient::connect().expect("connect owner");
    let info = owner
        .create_session(std::env::temp_dir(), "slow-sub".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");

    // Stuck subscriber: a RAW UnixStream that attaches as Observer over the wire
    // (mirroring `Request::Attach` / `Frame::Request`), then NEVER reads. Its
    // forwarder's writes will pile up in the kernel send buffer until full.
    let mut stuck = StdUnixStream::connect(&server.socket).expect("raw connect");
    let attach_frame = format!(
        "{{\"kind\":\"request\",\"id\":1,\"req\":{{\"method\":\"attach\",\"session_id\":{sid},\"mode\":\"observer\"}}}}\n",
        sid = serde_json::to_string(&info.session_id).unwrap()
    );
    stuck
        .write_all(attach_frame.as_bytes())
        .expect("send raw attach");
    stuck.flush().expect("flush raw attach");
    // Deliberately do NOT read from `stuck` from here on.

    // Drive a turn. The stub streams the big burst → the stuck observer's send
    // buffer fills → its forwarder write times out at 500ms → server drops it.
    owner.prompt(&info.session_id, "flood the slow subscriber").expect("prompt");

    // (a) The stuck subscriber is reaped: the server logs the slow-subscriber
    // warning. Bounded wait — the timeout is 500ms; allow generous slack for the
    // burst to fill the buffer and the write to stall.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw_warning = false;
    while Instant::now() < deadline {
        if server.read_log().contains("slow subscriber: write stalled") {
            saw_warning = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        saw_warning,
        "expected the server to log a 'slow subscriber: write stalled' disconnect for the \
         non-draining observer; the buffer-fill never tripped the {timeout}ms write timeout.\n\
         If this is flaky, the kernel send buffer may be larger than the burst — increase \
         STUB_CHUNKS / STUB_CHUNK_TEXT length.\nlog:\n{log}",
        timeout = 500,
        log = server.read_log()
    );

    // (b) The OWNER is unaffected: it still receives every chunk and the turn
    // completes normally (TurnEnded). The slow subscriber's reaping must not gap
    // or stall the healthy owner.
    let notes = drain_until(&owner, Duration::from_secs(30), |n| {
        count_agent_chunks(n) >= CHUNKS
            && n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
            })
    });
    let got = count_agent_chunks(&notes);
    assert_eq!(
        got, CHUNKS,
        "owner must receive ALL {CHUNKS} chunks even while a slow subscriber is reaped, got {got}\nlog:\n{}",
        server.read_log()
    );
    assert!(
        notes.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == info.session_id && *turn_count >= 1
        )),
        "owner's turn must complete normally despite the slow-subscriber disconnect; \
         notes={notes:#?}\nlog:\n{}",
        server.read_log()
    );

    // Keep the stuck stream alive until the very end so it stays connected for
    // the whole turn (drop closes it; we want the timeout, not a clean EOF).
    drop(stuck);
}

/// Is this a TRANSCRIPT note — i.e. an `event_log` entry the forwarder tails —
/// as opposed to a per-connection control note (`OwnerChanged`) synthesized by
/// the forwarder and never stored in the log? Used to compare what a cursor
/// reconnect streams against the durable log's tail.
fn is_transcript_note(n: &Notification) -> bool {
    !matches!(n, Notification::OwnerChanged { .. })
}

/// 7. CURSOR-BASED INCREMENTAL RECONNECT (spec phase 5, additive).
///
///    A reconnecting client that supplies a cursor `(generation, index)` must
///    receive ONLY the transcript tail after `index` — not the full replay from
///    0. A client that supplies NO cursor (today's every-client behavior, incl.
///    the GUI) still gets the full replay. A stale-generation cursor falls back
///    to full replay too. All proven against one real server + stub turn.
#[test]
fn cursor_reconnect_streams_only_tail() {
    let _g = serial_lock();
    const CHUNKS: usize = 8;
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", &CHUNKS.to_string())]);
    server.activate_env();

    // Owner drives one turn so the session has a known, non-trivial event_log.
    let owner = SessionServerClient::connect().expect("connect owner");
    let info = owner
        .create_session(std::env::temp_dir(), "cursor".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    owner.prompt(&info.session_id, "hello cursor").expect("prompt");

    let _ = drain_until(&owner, Duration::from_secs(15), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    // M = the authoritative durable transcript length, and generation must be 0
    // for a fresh, never-force-restarted session.
    let snap = owner.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    let m = s.event_log_len;
    assert_eq!(
        s.channel_generation, 0,
        "a fresh session's channel_generation must be 0; snapshot={snap:#?}"
    );
    assert!(
        m >= 4,
        "expected a non-trivial event_log (SessionAttached + UserPrompt + {CHUNKS} chunks + \
         TurnEnded), got len {m}; log:\n{}",
        server.read_log()
    );

    // CONTROL: a full-replay observer (cursor None) receives all M transcript
    // notes. We also use its transcript as ground truth for the log's contents,
    // so we can assert the cursor tail is an exact suffix.
    let full = SessionServerClient::connect().expect("connect full-replay observer");
    full.attach(&info.session_id, AttachMode::Observer)
        .expect("attach full observer");
    let full_notes = drain_until(&full, Duration::from_secs(10), |n| {
        n.iter()
            .filter(|x| is_transcript_note(x))
            .filter(|x| matches!(x, Notification::TurnEnded { .. }))
            .count()
            >= 1
    });
    let full_transcript: Vec<Notification> = full_notes
        .into_iter()
        .filter(is_transcript_note)
        .collect();
    assert_eq!(
        full_transcript.len(),
        m,
        "cursor None (full replay) must deliver all {m} transcript notes, got {}; \
         transcript={full_transcript:#?}\nlog:\n{}",
        full_transcript.len(),
        server.read_log()
    );

    // INCREMENTAL: a second observer attaches WITH cursor (0, K) for K in
    // (0, M). It must receive ONLY the tail [K..] — exactly M-K notes — and that
    // tail must equal the suffix of the full replay starting at K.
    let k = m / 2;
    assert!(k > 0 && k < m, "K={k} must be strictly inside (0, {m})");
    let tail = SessionServerClient::connect().expect("connect tail observer");
    tail.attach_with_cursor(&info.session_id, AttachMode::Observer, Some((0, k as u64)))
        .expect("attach tail observer with cursor");
    // Drain until we've seen the turn boundary (the last logged note for this
    // turn), then filter to transcript notes.
    let tail_notes = drain_until(&tail, Duration::from_secs(10), |n| {
        n.iter()
            .filter(|x| is_transcript_note(x))
            .filter(|x| matches!(x, Notification::TurnEnded { .. }))
            .count()
            >= 1
    });
    let tail_transcript: Vec<Notification> = tail_notes
        .into_iter()
        .filter(is_transcript_note)
        .collect();
    assert_eq!(
        tail_transcript.len(),
        m - k,
        "cursor (0, {k}) must stream ONLY the {} tail notes, got {}; tail={tail_transcript:#?}\nlog:\n{}",
        m - k,
        tail_transcript.len(),
        server.read_log()
    );
    // The tail must be the exact suffix full[K..] — first tail note == log[K],
    // no first K notes replayed.
    let expected_tail = &full_transcript[k..];
    for (i, (got, want)) in tail_transcript.iter().zip(expected_tail.iter()).enumerate() {
        assert_eq!(
            serde_json::to_string(got).unwrap(),
            serde_json::to_string(want).unwrap(),
            "tail note {i} must match event_log[{}] (the suffix after the cursor)",
            k + i
        );
    }

    // EDGE: a stale-generation cursor (999, 0) must FALL BACK to full replay —
    // the safe behavior for an epoch mismatch (force-restart / server restart).
    let stale = SessionServerClient::connect().expect("connect stale-cursor observer");
    stale
        .attach_with_cursor(&info.session_id, AttachMode::Observer, Some((999, 0)))
        .expect("attach stale-cursor observer");
    let stale_notes = drain_until(&stale, Duration::from_secs(10), |n| {
        n.iter()
            .filter(|x| is_transcript_note(x))
            .filter(|x| matches!(x, Notification::TurnEnded { .. }))
            .count()
            >= 1
    });
    let stale_transcript: Vec<Notification> = stale_notes
        .into_iter()
        .filter(is_transcript_note)
        .collect();
    assert_eq!(
        stale_transcript.len(),
        m,
        "a generation-mismatch cursor (999, 0) must fall back to FULL replay ({m} notes), got {}; \
         transcript={stale_transcript:#?}\nlog:\n{}",
        stale_transcript.len(),
        server.read_log()
    );
}
