//! Headless transcript harness for the session server.
//!
//! Sibling to `session_resilience_test.rs`. That file points the server at a
//! NO-OP agent (`/usr/bin/true`), so it only covers the bare socket/ownership
//! layer — the session's durable `event_log` stays empty. This file points the
//! server at `yalda-acp-stub` (a real-protocol stub ACP agent, built as a
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
//! `yalda-session-server` binary, so these are end-to-end against the same
//! code the GUI uses.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use yalda::session_client::SessionServerClient;
use yalda::session_proto::{AttachMode, Notification, socket_path};

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
    /// Start a server whose spawned agents are `yalda-acp-stub`, with the
    /// given `(VAR, value)` env knobs applied to the server process (and thus
    /// inherited by every agent it spawns).
    fn start_with_env(knobs: &[(&str, &str)]) -> TestServer {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir();
        let socket = dir.join(format!("yalda-txtest-{pid}-{n}.sock"));
        let log = dir.join(format!("yalda-txtest-{pid}-{n}.log"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&log);
        // Clear any durable state colocated with this socket so a prior run
        // can't restore stale sessions into this fresh server.
        let _ = std::fs::remove_file(socket.with_extension("state.json"));
        let _ = std::fs::remove_dir_all(socket.with_extension("wal"));
        Self::spawn_on(socket, log, knobs)
    }

    /// Spawn a server process bound to `socket`, stderr → `log`, agents =
    /// `yalda-acp-stub`, with the given env knobs. Does NOT clear durable
    /// state — callers that want a fresh start clear it first.
    fn spawn_on(socket: PathBuf, log: PathBuf, knobs: &[(&str, &str)]) -> TestServer {
        let logfile = std::fs::File::create(&log).expect("create server log");
        let server_bin = env!("CARGO_BIN_EXE_yalda-session-server");
        let stub_bin = env!("CARGO_BIN_EXE_yalda-acp-stub");
        let mut cmd = Command::new(server_bin);
        cmd.env("YALDA_SESSION_SOCKET", &socket)
            .env("YALDA_ACP_AGENT", stub_bin);
        for (k, v) in knobs {
            cmd.env(k, v);
        }
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(logfile))
            .spawn()
            .expect("spawn yalda-session-server");

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
        unsafe { std::env::set_var("YALDA_SESSION_SOCKET", &self.socket) };
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

/// Tests share process-wide env (`YALDA_SESSION_SOCKET`), so serialize them.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Connect a client carrying a stable lease `client_id` (phase 4). A client
/// that wants drive rights (Owner attach + prompt) MUST present one. Owner
/// clients across a "GUI restart" reuse the SAME id so the lease resumes.
fn connect_as(client_id: &str) -> SessionServerClient {
    let c = SessionServerClient::connect().expect("connect");
    c.set_client_id(client_id.to_string());
    c
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
                    event: yalda::acp_channel::ReplyEvent::Chunk(_),
                    ..
                }
            )
        })
        .count()
}

/// Drain notifications from `client` until `done(&collected)` returns true or
/// the deadline passes. Returns everything collected. Uses the non-blocking
/// `try_recv` with a short sleep — bounded wait, no fixed total sleep.
fn drain_until<F>(client: &SessionServerClient, timeout: Duration, mut done: F) -> Vec<Notification>
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

    let client = connect_as("gui-rt");
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
        chunk_count,
        CHUNKS,
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
        replay_chunks,
        CHUNKS,
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
        let client = connect_as("gui-large");
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
            got,
            CHUNKS,
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
    let client2 = connect_as("gui-large");
    let became_owner = client2
        .attach(&sid, AttachMode::Owner)
        .expect("re-attach after restart");
    assert!(
        became_owner,
        "fresh same-id attach should resume the lease on the first try\nlog:\n{}",
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
        replay_chunks,
        CHUNKS,
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
    client2
        .prompt(&sid, "again")
        .expect("prompt after reconnect");
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
        let client = connect_as("gui-midturn");
        let info = client
            .create_session(std::env::temp_dir(), "midturn".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach owner #1");
        client
            .prompt(&info.session_id, "stream slowly")
            .expect("prompt");

        // Wait until SOME chunks have arrived but the turn is NOT done yet, then
        // bail — this drops the client mid-turn.
        let _partial = drain_until(&client, Duration::from_secs(10), |n| {
            count_agent_chunks(n) >= 3
        });
        let saw = count_agent_chunks(&_partial);
        assert!(
            (3..CHUNKS).contains(&saw),
            "should have dropped mid-turn (saw {saw} of {CHUNKS}); the turn streamed too fast — \
             increase STUB_DELAY_MS\nlog:\n{}",
            server.read_log()
        );
        info.session_id
        // client dropped here → mid-turn disconnect.
    };

    // Fresh client re-attaches while the turn may still be streaming on the
    // server. The server keeps pumping the agent regardless of attach state.
    let client2 = connect_as("gui-midturn");
    client2
        .attach(&sid, AttachMode::Owner)
        .expect("re-attach mid-turn");

    // The forwarder tails event_log from index 0 on attach, so this fresh
    // client gets the WHOLE turn (everything streamed before AND after the
    // reconnect), ending with the turn boundary.
    let notes = drain_until(&client2, Duration::from_secs(30), |n| {
        n.iter().any(
            |note| matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid),
        )
    });

    // No corruption / no duplication: exactly CHUNKS distinct agent chunks, and
    // the per-chunk index sequence is 0..CHUNKS with no repeats. The stub emits
    // "chunk N", so we can verify ordering/uniqueness precisely.
    let chunk_texts: Vec<String> = notes
        .iter()
        .filter_map(|n| match n {
            Notification::ReplyEvent {
                event: yalda::acp_channel::ReplyEvent::Chunk(t),
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
        chunk_texts,
        expected,
        "chunk sequence must be in-order and unique after a mid-turn reconnect (no dup/reorder)\nlog:\n{}",
        server.read_log()
    );

    // Session still usable: a follow-up prompt completes a fresh turn.
    client2
        .prompt(&sid, "still there?")
        .expect("prompt after midturn reconnect");
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

/// 3b. MID-TURN PROMPT QUEUEING. A prompt sent while a turn is still streaming
///     must not be dropped: the channel driver is serial (one session/prompt in
///     flight; later prompts wait in an unbounded queue), so prompt B runs as
///     the next turn after A completes. Pins the server+channel half of the
///     "messages I send mid-turn are dropped" report — if this passes, a drop
///     is client-side (GUI gating / lease rejection invisible to the user).
#[test]
fn midturn_prompt_queues_and_runs_next_turn() {
    let _g = serial_lock();
    const CHUNKS: usize = 40;
    // 25ms/chunk → ~1s turn: a wide window to land a second prompt mid-stream.
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "40"), ("STUB_DELAY_MS", "25")]);
    server.activate_env();

    let client = connect_as("gui-midturn-queue");
    let info = client
        .create_session(std::env::temp_dir(), "midturn-queue".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    client
        .attach(&sid, AttachMode::Owner)
        .expect("attach owner");

    client.prompt(&sid, "turn A").expect("prompt A");
    // Wait until turn A is verifiably mid-stream (some chunks, not all).
    let partial = drain_until(&client, Duration::from_secs(10), |n| {
        count_agent_chunks(n) >= 3
    });
    let saw = count_agent_chunks(&partial);
    assert!(
        (3..CHUNKS).contains(&saw),
        "prompt B must land mid-turn (saw {saw} of {CHUNKS} chunks); increase STUB_DELAY_MS\nlog:\n{}",
        server.read_log()
    );

    // The mid-turn send under test.
    client.prompt(&sid, "turn B").expect("prompt B mid-turn");

    // Both turns must complete: drain to TurnEnded(2).
    let rest = drain_until(&client, Duration::from_secs(30), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2)
        })
    });
    assert!(
        rest.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count == 2
        )),
        "turn B (sent mid-turn) must run as the next turn — a mid-turn prompt \
         must never be dropped; got TurnEndeds: {:#?}\nlog:\n{}",
        rest.iter()
            .filter(|n| matches!(n, Notification::TurnEnded { .. }))
            .collect::<Vec<_>>(),
        server.read_log()
    );
    // Turn B's payload arrived too: A's remaining chunks + B's full set.
    let total_after = count_agent_chunks(&rest);
    assert_eq!(
        saw + total_after,
        2 * CHUNKS,
        "expected the rest of A plus all of B ({} chunks), got {total_after} after {saw} partial\nlog:\n{}",
        2 * CHUNKS - saw,
        server.read_log()
    );
    // And the durable log holds BOTH user prompts.
    for want in ["turn A", "turn B"] {
        assert!(
            partial.iter().chain(rest.iter()).any(|n| matches!(
                n,
                Notification::UserPrompt { text, .. } if text == want
            )),
            "user prompt {want:?} missing from the stream\nlog:\n{}",
            server.read_log()
        );
    }
}

/// 3c. ACTION-AS-LIVENESS: a prompt from the lease holder whose lease has
///     EXPIRED (no heartbeats — e.g. an App-Napped window) must re-grant the
///     lease and deliver, not be refused. Pre-fix, `do_prompt` used the strict
///     `holds_lease` gate, so the first post-wake prompt raced the 5s
///     heartbeat reclaim and lost — silently, because `prompt()` is
///     fire-and-forget (the other half of the "messages sent mid-turn are
///     dropped" report).
#[test]
fn prompt_from_expired_same_client_regrants_lease_and_delivers() {
    let _g = serial_lock();
    // 300ms TTL so the lease verifiably lapses between turns; the sweep
    // (5s cadence) may or may not have cleared it to None — the gate must
    // handle both (expired-same-id renew AND free-claim).
    let server =
        TestServer::start_with_env(&[("STUB_CHUNKS", "3"), ("YALDA_LEASE_TTL_MS", "300")]);
    server.activate_env();

    let client = connect_as("gui-napped");
    let info = client
        .create_session(std::env::temp_dir(), "naptest".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    client
        .attach(&sid, AttachMode::Owner)
        .expect("attach owner");

    client.prompt(&sid, "turn 1").expect("prompt 1");
    drain_until(&client, Duration::from_secs(15), |n| {
        n.iter().any(
            |note| matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid),
        )
    });

    // Let the lease lapse with NO heartbeat (the napped-window simulation).
    std::thread::sleep(Duration::from_millis(700));

    // The post-wake prompt itself must reclaim the lease and drive a turn.
    client.prompt(&sid, "turn 2 after nap").expect("prompt 2");
    let after = drain_until(&client, Duration::from_secs(15), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count >= 2)
        })
    });
    assert!(
        after.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count == 2
        )),
        "a prompt from the same client with a lapsed lease must re-grant and \
         deliver (action-as-liveness), not be silently refused; log:\n{}",
        server.read_log()
    );
}

/// 3d. REJECTED PROMPTS ARE VISIBLE: a prompt refused because ANOTHER window
///     holds a live lease must come back as a `PromptRejected` notification on
///     the submitter's own stream (carrying the text so the GUI can restore
///     it). `prompt()` is fire-and-forget, so without this the rejection had
///     no observable effect anywhere.
#[test]
fn rejected_prompt_surfaces_prompt_rejected_notification() {
    let _g = serial_lock();
    let server = TestServer::start_with_env(&[("STUB_CHUNKS", "2")]);
    server.activate_env();

    // A: the live lease holder (default 15s TTL — stays live for the test).
    let owner = connect_as("gui-owner");
    let info = owner
        .create_session(std::env::temp_dir(), "rejecttest".into(), None)
        .expect("create_session");
    let sid = info.session_id.clone();
    owner.attach(&sid, AttachMode::Owner).expect("attach owner");

    // B: different client_id; Owner attach silently downgrades to observer
    // while A's lease is live.
    let interloper = connect_as("gui-interloper");
    interloper
        .attach(&sid, AttachMode::Owner)
        .expect("attach interloper");

    interloper
        .prompt(&sid, "should be refused")
        .expect("prompt write");
    let notes = drain_until(&interloper, Duration::from_secs(10), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::PromptRejected { session_id, .. } if *session_id == sid)
        })
    });
    let rejected = notes.iter().find_map(|n| match n {
        Notification::PromptRejected {
            session_id,
            reason,
            text,
        } if *session_id == sid => Some((reason.clone(), text.clone())),
        _ => None,
    });
    let (reason, text) = rejected.unwrap_or_else(|| {
        panic!(
            "a refused prompt must surface PromptRejected on the submitter's stream; \
             got: {notes:#?}\nlog:\n{}",
            server.read_log()
        )
    });
    assert!(
        reason.contains("lease"),
        "rejection reason should name the lease: {reason}"
    );
    assert_eq!(
        text, "should be refused",
        "the rejected text rides the notification so the GUI can restore it"
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
        let client = connect_as("gui-headless");
        let info = client
            .create_session(std::env::temp_dir(), "headless".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach owner");
        // Send the prompt, then leave IMMEDIATELY — before the turn can finish.
        // `prompt` is a round-trip, so on return the agent already has the work.
        client
            .prompt(&info.session_id, "run with nobody watching")
            .expect("prompt");
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
    let gui = connect_as("gui-headless");
    let sessions = gui.list_sessions().expect("list");
    assert!(
        sessions.iter().any(|s| s.session_id == sid),
        "session vanished while no GUI was attached; log:\n{}",
        server.read_log()
    );
    gui.attach(&sid, AttachMode::Owner)
        .expect("attach after the fact");

    let replay = drain_until(&gui, Duration::from_secs(10), |n| {
        count_agent_chunks(n) >= CHUNKS
            && n.iter().any(|note| {
                matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid)
            })
    });
    let chunks = count_agent_chunks(&replay);
    assert_eq!(
        chunks,
        CHUNKS,
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

    // Sanity: nobody holds the lease (admin_status reports lease_holder=None).
    let snap = client.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    assert!(
        !s.has_owner && s.lease_holder.is_none(),
        "precondition: session must be UNLEASED before the headless prompt; snapshot={snap:#?}"
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
        chunks,
        CHUNKS,
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
        let client = connect_as("gui-crash");
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
    let client2 = connect_as("gui-crash");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut present = false;
    while Instant::now() < deadline {
        if let Ok(sessions) = client2.list_sessions()
            && sessions.iter().any(|s| s.session_id == sid)
        {
            present = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        present,
        "session was not recovered from the WAL after a hard crash; log:\n{}",
        server2.read_log()
    );

    // Attach and confirm the FULL pre-crash transcript replays from the WAL.
    // After a crash every lease is dead (no heartbeats reached the dead server),
    // so this same-id Owner attach first-claims a free lease.
    client2
        .attach(&sid, AttachMode::Owner)
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

/// 5b. RESUME LIVENESS — the "resume hangs" regression. Recovery alone (test 5)
///     is not enough: the recovered session must remain DRIVABLE. After a crash
///     + respawn the server re-spawns the agent with a resume id; the agent
///     replays the prior history (which the recovered event_log already holds)
///     and the pump's replay fence must discard exactly that burst — then let
///     everything after the worker's end-of-replay marker through.
///
///     The bug this pins down: the fence used to wait for the channel's turn
///     counter to reach the restored turn count, but the counter restarts at 0
///     on every spawn and only moves on LIVE turns (092c218 replaced the
///     post-load bump with the `ReplayComplete` marker), so the fence never
///     cleared and every post-resume event was silently discarded — prompts
///     looked hung forever while the agent worked invisibly (and in yolo mode,
///     invisibly was not hypothetically).
#[test]
fn recovered_session_is_drivable_after_resume() {
    let _g = serial_lock();
    const CHUNKS: usize = 4;
    // STUB_REPLAY_USER makes the stub's session/load re-emit the user's prior
    // prompt before its agent chunks — a realistic replay burst the fence must
    // swallow whole.
    let knobs: &[(&str, &str)] = &[
        ("STUB_CHUNKS", "4"),
        ("STUB_REPLAY_USER", "pre-crash prompt"),
    ];
    let mut server = TestServer::start_with_env(knobs);
    server.activate_env();

    // One completed turn before the crash, so recovery restores turns=1 and
    // arms the replay fence.
    let sid = {
        let client = connect_as("gui-resume");
        let info = client
            .create_session(std::env::temp_dir(), "resumetest".into(), None)
            .expect("create_session");
        client
            .attach(&info.session_id, AttachMode::Owner)
            .expect("attach");
        client
            .prompt(&info.session_id, "pre-crash prompt")
            .expect("prompt");
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

    server.crash();
    let server2 = server.respawn(knobs);

    // Wait for recovery, then attach. The attach tail replays the recovered
    // WAL; the resume's end-of-replay marker is recorded strictly after it
    // (the pump records the marker when it clears the fence), so seeing the
    // marker means the resume settled AND everything before it was delivered.
    let client2 = connect_as("gui-resume");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(sessions) = client2.list_sessions()
            && sessions.iter().any(|s| s.session_id == sid)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    client2
        .attach(&sid, AttachMode::Owner)
        .expect("re-attach recovered session");
    let replay = drain_until(&client2, Duration::from_secs(15), |n| {
        n.iter().any(|note| {
            matches!(
                note,
                Notification::ReplyEvent {
                    event: yalda::acp_channel::ReplyEvent::ReplayComplete,
                    ..
                }
            )
        })
    });
    assert_eq!(
        count_agent_chunks(&replay),
        CHUNKS,
        "the attach tail must hold exactly the recovered transcript — more \
         means the fence leaked the resume's replay burst into the log \
         (double-record), fewer means recovery lost chunks; log:\n{}",
        server2.read_log()
    );
    // The stub's replayed user echo (ReplyEvent::UserMessage) is pre-marker
    // replay and must have been discarded by the fence; the durable
    // UserPrompt record is the only copy of the user's prompt.
    assert!(
        !replay.iter().any(|n| matches!(
            n,
            Notification::ReplyEvent {
                event: yalda::acp_channel::ReplyEvent::UserMessage(_),
                ..
            }
        )),
        "the resume's replayed user echo leaked past the fence; replay={replay:#?}"
    );

    // THE REGRESSION: drive a NEW turn on the recovered session. Pre-fix, the
    // wedged fence discarded the agent's entire response and this drain came
    // back empty (the user-visible "resume hangs").
    client2
        .prompt(&sid, "post-resume prompt")
        .expect("prompt recovered session");
    let live = drain_until(&client2, Duration::from_secs(15), |n| {
        n.iter().any(
            |note| matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == sid),
        )
    });
    assert_eq!(
        count_agent_chunks(&live),
        CHUNKS,
        "the post-resume turn's chunks must reach the client — a recovered \
         session must stay drivable; log:\n{}",
        server2.read_log()
    );
    // Turn numbering continues from the restored count: the channel's own
    // counter restarted at 0, so without the pump's turn_base offset this
    // would regress to 1 (and the WAL's max(turn)+1 recovery would corrupt).
    assert!(
        live.iter().any(|n| matches!(
            n,
            Notification::TurnEnded { session_id, turn_count, .. }
                if *session_id == sid && *turn_count == 2
        )),
        "post-resume TurnEnded must carry turn_count=2 (continuing the \
         restored count of 1), got: {:#?}\nlog:\n{}",
        live.iter()
            .filter(|n| matches!(n, Notification::TurnEnded { .. }))
            .collect::<Vec<_>>(),
        server2.read_log()
    );
}

/// 6. SLOW-SUBSCRIBER DISCONNECT (phase-7 liveness hardening). A subscriber
///    whose socket stops draining must NOT be able to park its forwarder task +
///    fd forever. The forwarder bounds every socket write by
///    `YALDA_SLOW_SUB_TIMEOUT_MS`; when a non-draining peer's OS send buffer
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
        ("YALDA_SLOW_SUB_TIMEOUT_MS", "500"),
    ]);
    server.activate_env();

    // Owner: a normal client that reads continuously.
    let owner = connect_as("gui-slowsub");
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
    owner
        .prompt(&info.session_id, "flood the slow subscriber")
        .expect("prompt");

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
        got,
        CHUNKS,
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

/// 6b. HIGH-WATER BACKLOG BOUND (spec §6, MAJOR — disconnect-before-gap).
///
///     The owner hard-ceiling (`floor = min(sent_seq)` over live forwarders,
///     never compact past it) means a slow/PAUSED forwarder — concretely a
///     backgrounded GUI under macOS App Nap that stops draining its socket —
///     pins `min(sent_seq)`, blocks the trim, and grows the in-memory `event_log`
///     `Vec`. The 60s slow-sub write timeout is the only other reaper, so a
///     reader that pauses can pin growth for up to 60s (or unbounded if it drains
///     just enough to keep resetting the write timer). Spec §6 requires a
///     subscriber past the high-water backlog threshold to be DISCONNECTED
///     (forced clean from-0 reconnect) and thereby dropped from the `min`, so the
///     trim resumes — high-water disconnect fires BEFORE any gap-marker.
///
///     Setup mirrors the slow-subscriber test but isolates the HIGH-WATER reaper
///     from the WRITE-TIMEOUT reaper: the write timeout is set HUGE (60s) so it
///     can NEVER fire within the test, leaving the high-water disconnect as the
///     ONLY mechanism that can reap the wedged consumer. A tiny CAP (16) and a
///     low HIGH_WATER (= cap×2 = 32) make a long turn (CHUNKS=600) blow past the
///     bound quickly. A healthy draining owner drives the turn; a raw observer
///     attaches and then NEVER reads, pinning the floor.
///
///     Assertions:
///       (1) the wedged observer is DISCONNECTED — the server logs the
///           "high-water disconnect" warning AND the raw stream sees EOF;
///       (2) the in-memory log is BOUNDED — `event_log_len` settles far below the
///           full CHUNKS transcript (near the cap), and `log_base` advanced past
///           HIGH_WATER, proving the trim RESUMED after the disconnect.
///
///     FAIL-BEFORE (without the high-water bound): the wedged observer is never
///     disconnected (the 60s write timeout can't fire in-test), so the floor
///     stays pinned at the wedged observer's `sent_seq ≈ 0`, the trim never
///     advances `log_base`, and `event_log_len` grows to the full transcript.
#[test]
fn slow_owner_past_high_water_is_disconnected_log_bounded() {
    use std::io::Read as _;
    use std::io::Write as _;
    use std::os::unix::net::UnixStream as StdUnixStream;

    let _g = serial_lock();
    const CHUNKS: usize = 600;
    const CAP: usize = 16;
    // HIGH_WATER must be (a) comfortably ABOVE any transient lag a HEALTHY
    // draining owner exhibits (so the owner is never falsely reaped), and (b)
    // well BELOW the full transcript (~2×CHUNKS events, since each chunk is
    // logged twice — legacy ReplyEvent + additive Agent) so a WEDGED consumer
    // pinned near seq 0 crosses it long before the turn ends. 150 sits in that
    // window for CHUNKS=600 (full ≈ 1200).
    const HIGH_WATER: usize = 150;
    // A modest per-chunk text so the wedged observer's send buffer fills (pinning
    // the floor).
    let chunk_text = "y".repeat(64);
    let server = TestServer::start_with_env(&[
        ("STUB_CHUNKS", &CHUNKS.to_string()),
        ("STUB_CHUNK_TEXT", &chunk_text),
        // Pace the stream so the HEALTHY owner's reader keeps up and its forwarder
        // never lags past HIGH_WATER (only the never-draining wedged observer
        // does). Fast enough that the whole turn still finishes promptly.
        ("STUB_DELAY_MS", "3"),
        ("YALDA_EVENT_LOG_CAP", &CAP.to_string()),
        ("YALDA_EVENT_LOG_HIGH_WATER", &HIGH_WATER.to_string()),
        // HUGE write timeout: the high-water disconnect must be the ONLY reaper.
        ("YALDA_SLOW_SUB_TIMEOUT_MS", "60000"),
    ]);
    server.activate_env();

    // Healthy owner: a normal client that reads continuously and drives the turn.
    let owner = connect_as("gui-highwater");
    let info = owner
        .create_session(std::env::temp_dir(), "high-water".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");

    // Wedged observer: a RAW UnixStream that attaches as Observer over the wire,
    // then NEVER reads. Its forwarder's writes pile up in the kernel send buffer
    // and its `sent_seq` stays pinned near 0, holding the trim floor down.
    let mut wedged = StdUnixStream::connect(&server.socket).expect("raw connect");
    let attach_frame = format!(
        "{{\"kind\":\"request\",\"id\":1,\"req\":{{\"method\":\"attach\",\"session_id\":{sid},\"mode\":\"observer\"}}}}\n",
        sid = serde_json::to_string(&info.session_id).unwrap()
    );
    wedged
        .write_all(attach_frame.as_bytes())
        .expect("send raw attach");
    wedged.flush().expect("flush raw attach");
    // Deliberately do NOT read from `wedged` from here on (paused-reader sim).

    // Drive a long turn. The stub streams 600 chunks; the wedged observer's send
    // buffer fills and pins the floor; the backlog crosses HIGH_WATER → the
    // server force-disconnects the wedged observer and the trim resumes.
    owner
        .prompt(&info.session_id, "flood past the high-water mark")
        .expect("prompt");

    // (1a) The server logs a high-water disconnect for the wedged observer —
    // and NOT a write-timeout disconnect (the 60s timeout can't fire in-test).
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut saw_high_water = false;
    while Instant::now() < deadline {
        let log = server.read_log();
        if log.contains("high-water disconnect") {
            saw_high_water = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = server.read_log();
    assert!(
        saw_high_water,
        "expected a 'high-water disconnect' for the wedged observer (CAP={CAP}, \
         HIGH_WATER={HIGH_WATER}, CHUNKS={CHUNKS}); the high-water bound never fired.\nlog:\n{log}"
    );
    assert!(
        !log.contains("write stalled"),
        "the high-water disconnect — NOT the 60s write timeout — must reap the wedged \
         observer; the write-timeout path fired unexpectedly.\nlog:\n{log}"
    );

    // (1b) The wedged observer's connection is closed: a read returns EOF (0
    // bytes) or an error once the server drops the forwarder and closes the
    // write half. (Its OS buffer may hold queued frames we never read; we read
    // until EOF/error, bounded.)
    wedged
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let mut buf = [0u8; 4096];
    let mut saw_eof = false;
    let eof_deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < eof_deadline {
        match wedged.read(&mut buf) {
            Ok(0) => {
                saw_eof = true;
                break;
            }
            Ok(_) => continue, // drain queued frames until EOF
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => {
                saw_eof = true;
                break;
            }
        }
    }
    assert!(
        saw_eof,
        "the wedged observer's connection must be CLOSED by the high-water disconnect \
         (read should hit EOF), but it stayed open.\nlog:\n{}",
        server.read_log()
    );

    // (2) The in-memory log is BOUNDED. Probe via admin_status from a FRESH
    // client (decoupled from the owner's liveness). Poll until the trim has
    // RESUMED — `log_base` advanced past HIGH_WATER, which it could NOT have done
    // had the wedged observer kept pinning the floor near seq 0. Throughout, the
    // resident `Vec` must stay BOUNDED: far below the full transcript (~2×CHUNKS
    // entries — each chunk is logged twice), settling near the high-water + cap
    // window. A wedged consumer pinning unbounded growth is the bug under test.
    let admin = connect_as("admin-probe");
    let bound = HIGH_WATER + CAP + 8;
    let probe_deadline = Instant::now() + Duration::from_secs(60);
    let mut resumed = false;
    let mut last = None;
    while Instant::now() < probe_deadline {
        let snap = admin.admin_status().expect("admin_status");
        if let Some(s) = snap
            .sessions
            .iter()
            .find(|s| s.session_id == info.session_id)
        {
            // The in-memory Vec must NEVER blow past the bound — assert on every
            // poll so a transient overshoot is caught, not just the final state.
            assert!(
                s.event_log_len <= bound,
                "in-memory event_log must stay BOUNDED (≤ {bound}) while a wedged consumer \
                 is reaped — got {} (full transcript ≈ {}); admin={s:#?}\nlog:\n{}",
                s.event_log_len,
                2 * CHUNKS,
                server.read_log()
            );
            last = Some((s.log_base, s.event_log_len));
            if s.log_base as usize > HIGH_WATER {
                resumed = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        resumed,
        "the trim must RESUME after the high-water disconnect — log_base should advance \
         past HIGH_WATER ({HIGH_WATER}); last observed (log_base, event_log_len)={last:?}\nlog:\n{}",
        server.read_log()
    );

    drop(wedged);
}

/// Is this a TRANSCRIPT note — i.e. an `event_log` entry the forwarder tails —
/// as opposed to a per-connection control note (`LeaseChanged`) synthesized by
/// the forwarder and never stored in the log? Used to compare what a cursor
/// reconnect streams against the durable log's tail.
fn is_transcript_note(n: &Notification) -> bool {
    !matches!(n, Notification::LeaseChanged { .. })
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
    let owner = connect_as("gui-cursor");
    let info = owner
        .create_session(std::env::temp_dir(), "cursor".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    owner
        .prompt(&info.session_id, "hello cursor")
        .expect("prompt");

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
    let full_transcript: Vec<Notification> =
        full_notes.into_iter().filter(is_transcript_note).collect();
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
    let tail_transcript: Vec<Notification> =
        tail_notes.into_iter().filter(is_transcript_note).collect();
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
    let stale_transcript: Vec<Notification> =
        stale_notes.into_iter().filter(is_transcript_note).collect();
    assert_eq!(
        stale_transcript.len(),
        m,
        "a generation-mismatch cursor (999, 0) must fall back to FULL replay ({m} notes), got {}; \
         transcript={stale_transcript:#?}\nlog:\n{}",
        stale_transcript.len(),
        server.read_log()
    );
}

/// Is this note the Stage B ringbuffer-trim marker (a `CompactedSummary`)?
fn is_compacted_summary(n: &Notification) -> bool {
    matches!(
        n,
        Notification::Agent { event }
            if matches!(
                event.kind,
                yalda::agent_event::AgentEventKind::CompactedSummary { .. }
            )
    )
}

/// 8. RINGBUFFER COMPACTION (spec-event-stream §6, phase-8 Stage B).
///
///    With a TINY in-memory `event_log` cap and a turn of many chunks, the
///    server trims the front of the in-memory log and advances `log_base`. Two
///    guarantees, end-to-end against a real server + stub turn:
///
///    (a) The trim is SURFACED, not silent: a full-replay (from-base) observer
///    receives a `CompactedSummary` marker as its FIRST transcript note, and
///    the resident log stays bounded near the cap (NOT the full chunk count).
///    `admin_status` reports a non-zero `log_base`.
///
///    (b) A client whose acked `seq` fell BELOW `log_base` (it was trimmed past)
///    gets a clean from-base rebuild — it sees the `CompactedSummary` marker
///    too, never a silent gap (spec §6 fast-disconnect-before-gap).
#[test]
fn ringbuffer_compaction_trims_and_surfaces_marker() {
    let _g = serial_lock();
    // A turn of CHUNKS chunks. CAP is well below the total transcript length
    // (SessionAttached + ChannelOpened + UserPrompt + CHUNKS chunks + TurnEnded),
    // so the front MUST be trimmed mid-stream.
    const CHUNKS: usize = 40;
    const CAP: usize = 8;
    // STUB_DELAY_MS paces the stream so the LIVE owner's forwarder keeps up with
    // production. This matters since the owner hard-ceiling (Bug 1b): the trim
    // floor is `min(live forwarder sent_seq)`, so a trim can never drop below the
    // owner's forwarded position. With an un-paced burst the owner's socket would
    // lag the in-memory log and the floor would (correctly) BLOCK trimming until
    // it catches up — the spec §6 "never gap the owner" guarantee, but it would
    // let the resident `Vec` grow past the cap mid-burst. Pacing keeps the owner
    // at the tip so the floor tracks the tip and the log stays bounded near the
    // cap while STILL never gapping the owner. (The high-water disconnect that
    // bounds a genuinely-wedged owner is a separate spec §6 follow-up.)
    let server = TestServer::start_with_env(&[
        ("STUB_CHUNKS", &CHUNKS.to_string()),
        ("STUB_DELAY_MS", "2"),
        ("YALDA_EVENT_LOG_CAP", &CAP.to_string()),
    ]);
    server.activate_env();

    let owner = connect_as("gui-compact");
    let info = owner
        .create_session(std::env::temp_dir(), "compact".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    owner
        .prompt(&info.session_id, "stream a lot")
        .expect("prompt");

    // Drain the owner CONTINUOUSLY so its forwarder progress keeps advancing
    // (the floor follows it to the tip), letting trims keep the log bounded.
    let _ = drain_until(&owner, Duration::from_secs(20), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    // (a) The in-memory log is bounded near the cap, and log_base advanced.
    let snap = owner.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    assert!(
        s.log_base > 0,
        "a trim must have advanced log_base above 0; admin={s:#?}\nlog:\n{}",
        server.read_log()
    );
    // Resident length is bounded near the cap. NOTE (Bug 1b owner hard-ceiling):
    // the bound is the low-water `target` PLUS a bounded tail of entries that
    // were in-flight to the live owner when the final push happened — the trim
    // floor can't drop below the owner's not-yet-forwarded position, so the last
    // handful of streamed entries settle above the cap rather than being trimmed
    // out from under the owner. That tail is small and bounded (it clears on the
    // next push once the owner forwards them), so the log stays a SMALL MULTIPLE
    // of the cap, never the full transcript. The load-bearing guarantee is the
    // second assertion: compaction happened, so the resident log is far below the
    // full ~2*CHUNKS-entry transcript.
    assert!(
        s.event_log_len <= CAP * 3,
        "in-memory log must stay bounded near the cap (owner-ceiling tail included), \
         got {} (cap {CAP}); admin={s:#?}\nlog:\n{}",
        s.event_log_len,
        server.read_log()
    );
    assert!(
        s.event_log_len < CHUNKS,
        "in-memory log must be far below the {CHUNKS}-chunk transcript (compaction \
         must have happened), got {}; admin={s:#?}",
        s.event_log_len
    );

    // (a, cont.) A from-base observer (cursor None) sees the CompactedSummary
    // marker as its FIRST transcript note — the trim is surfaced, not silent.
    let from_base = SessionServerClient::connect().expect("connect from-base observer");
    from_base
        .attach(&info.session_id, AttachMode::Observer)
        .expect("attach from-base observer");
    let notes = drain_until(&from_base, Duration::from_secs(10), |n| {
        n.iter().filter(|x| is_transcript_note(x)).count() >= 1
    });
    let transcript: Vec<Notification> = notes.into_iter().filter(is_transcript_note).collect();
    assert!(
        is_compacted_summary(&transcript[0]),
        "a from-base rebuild must begin with the CompactedSummary trim marker, got {:#?}\nlog:\n{}",
        transcript[0],
        server.read_log()
    );

    // (b) A client whose acked_seq fell BELOW log_base must get a from-base
    // rebuild (sees the marker), NOT a gap. Cursor (gen 0, seq 1) is below the
    // advanced base.
    assert_eq!(
        s.channel_generation, 0,
        "fresh session generation must be 0; admin={s:#?}"
    );
    let stale_seq = 1u64;
    assert!(
        stale_seq < s.log_base,
        "test precondition: acked_seq {stale_seq} must be below log_base {}",
        s.log_base
    );
    let fell_off = SessionServerClient::connect().expect("connect fell-off observer");
    fell_off
        .attach_with_cursor(&info.session_id, AttachMode::Observer, Some((0, stale_seq)))
        .expect("attach fell-off observer");
    let fo_notes = drain_until(&fell_off, Duration::from_secs(10), |n| {
        n.iter().filter(|x| is_transcript_note(x)).count() >= 1
    });
    let fo_transcript: Vec<Notification> =
        fo_notes.into_iter().filter(is_transcript_note).collect();
    assert!(
        is_compacted_summary(&fo_transcript[0]),
        "a cursor below log_base must fall back to a from-base rebuild (marker first), \
         got {:#?}\nlog:\n{}",
        fo_transcript[0],
        server.read_log()
    );
}

/// 9. CURSOR RECONNECT STILL TAILS WHEN IN-RANGE (spec §6 back-compat).
///
///    Even after compaction has advanced `log_base`, a cursor whose acked `seq`
///    is at/above `log_base` (still resident) must stream ONLY the tail
///    `[acked_seq..]` — NOT a full from-base rebuild. This proves the
///    `seq ↔ Vec-offset` translation via `log_base` keeps phase-5 incremental
///    reconnect working through a trim (the load-bearing interaction).
#[test]
fn cursor_reconnect_tails_after_compaction_when_in_range() {
    let _g = serial_lock();
    const CHUNKS: usize = 40;
    const CAP: usize = 8;
    // Pace the stream so the live owner's forwarder keeps up and the owner
    // hard-ceiling floor (Bug 1b) tracks the tip — see the note in
    // `ringbuffer_compaction_trims_and_surfaces_marker`. Without pacing a fast
    // burst would (correctly) block trimming below the lagging owner and
    // `log_base` might not advance, voiding this test's compaction precondition.
    let server = TestServer::start_with_env(&[
        ("STUB_CHUNKS", &CHUNKS.to_string()),
        ("STUB_DELAY_MS", "2"),
        ("YALDA_EVENT_LOG_CAP", &CAP.to_string()),
    ]);
    server.activate_env();

    let owner = connect_as("gui-compact-tail");
    let info = owner
        .create_session(std::env::temp_dir(), "compact-tail".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner");
    owner
        .prompt(&info.session_id, "stream a lot")
        .expect("prompt");
    let _ = drain_until(&owner, Duration::from_secs(20), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    let snap = owner.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    let base = s.log_base;
    let len = s.event_log_len as u64;
    let tip = base + len; // exclusive logical tip
    assert!(
        base > 0,
        "compaction must have advanced log_base; admin={s:#?}"
    );

    // CONTROL: a from-base observer's transcript IS the resident log, in order —
    // ground truth for the suffix comparison.
    let full = SessionServerClient::connect().expect("connect full observer");
    full.attach(&info.session_id, AttachMode::Observer)
        .expect("attach full observer");
    let full_notes = drain_until(&full, Duration::from_secs(10), |n| {
        n.iter().filter(|x| is_transcript_note(x)).count() >= len as usize
    });
    let full_transcript: Vec<Notification> =
        full_notes.into_iter().filter(is_transcript_note).collect();
    assert_eq!(
        full_transcript.len(),
        len as usize,
        "from-base observer must receive exactly the {len} resident notes; \
         got {}\nlog:\n{}",
        full_transcript.len(),
        server.read_log()
    );

    // INCREMENTAL: a cursor at a resident seq K in (base, tip) must stream ONLY
    // the tail [K..] — exactly (tip - K) notes — and that tail must equal the
    // resident-log suffix at Vec offset (K - base).
    let k = base + (tip - base) / 2; // strictly inside the resident range
    assert!(
        k > base && k < tip,
        "K={k} must be inside (base {base}, tip {tip})"
    );
    let tail = SessionServerClient::connect().expect("connect tail observer");
    tail.attach_with_cursor(&info.session_id, AttachMode::Observer, Some((0, k)))
        .expect("attach tail observer with cursor");
    let expected_tail_len = (tip - k) as usize;
    let tail_notes = drain_until(&tail, Duration::from_secs(10), |n| {
        n.iter().filter(|x| is_transcript_note(x)).count() >= expected_tail_len
    });
    let tail_transcript: Vec<Notification> =
        tail_notes.into_iter().filter(is_transcript_note).collect();
    assert_eq!(
        tail_transcript.len(),
        expected_tail_len,
        "cursor (0, {k}) must stream ONLY the {expected_tail_len} tail notes (NOT a from-base \
         rebuild of {len}), got {}; tail={tail_transcript:#?}\nlog:\n{}",
        tail_transcript.len(),
        server.read_log()
    );
    // The tail must equal the resident suffix at Vec offset (K - base).
    let off = (k - base) as usize;
    let expected = &full_transcript[off..];
    for (i, (got, want)) in tail_transcript.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            serde_json::to_string(got).unwrap(),
            serde_json::to_string(want).unwrap(),
            "tail note {i} must equal resident log[{}] (suffix after the cursor seq)",
            off + i
        );
    }
}

/// Extract the 0-based chunk index `i` from a stub chunk's text (`"chunk {i}"`).
/// Matches ONLY the legacy `ReplyEvent::Chunk` (the GUI-driving variant counted
/// by `count_agent_chunks`) — during the additive rollout each chunk is ALSO
/// recorded as an `Agent { Chunk }`, so matching both would double every index.
/// `None` for non-chunk notes or chunks that don't match the stub format.
fn stub_chunk_index(n: &Notification) -> Option<usize> {
    let text = match n {
        Notification::ReplyEvent {
            event: yalda::acp_channel::ReplyEvent::Chunk(t),
            ..
        } => t.as_str(),
        _ => return None,
    };
    text.strip_prefix("chunk ")?.trim().parse::<usize>().ok()
}

/// 10. LIVE OWNER STREAMING ACROSS A TRIM (Bug 1, spec §6 owner hard-ceiling).
///
///     This is the bug the prior tests MISSED: they attached observers AFTER a
///     trim, so the corruption window — a LIVE forwarder whose `sent` position is
///     invalidated by a front-trim that shortens the published `Vec` — never
///     opened. Here an owner attaches BEFORE the turn and streams a long turn
///     whose event count crosses `YALDA_EVENT_LOG_CAP`, so a trim fires
///     mid-stream while the owner is live.
///
///     Two guarantees:
///     (a) The owner receives EVERY chunk exactly once, in order — no gap, no
///     dup. Before Bug 1a's fix, the forwarder tracked `sent` as a raw
///     `Vec` index; a front-trim made `snapshot.len() > sent` go false
///     (stall), then a re-slice at a stale offset gapped/duped the stream.
///     (b) The trim NEVER drops below the owner's forwarded position (Bug 1b's
///     owner hard-ceiling): the durable log stays AHEAD of the cap while the
///     live owner is mid-stream, because the floor = min(live sent_seq)
///     protects everything the owner hasn't yet been sent.
///
///     `STUB_DELAY_MS` paces the stream so the owner forwards incrementally
///     across several trims rather than after the whole turn is already logged.
#[test]
fn live_owner_streams_across_trim_no_gap_or_dup() {
    let _g = serial_lock();
    const CHUNKS: usize = 300;
    const CAP: usize = 16;
    let server = TestServer::start_with_env(&[
        ("STUB_CHUNKS", &CHUNKS.to_string()),
        ("STUB_DELAY_MS", "2"),
        ("YALDA_EVENT_LOG_CAP", &CAP.to_string()),
    ]);
    server.activate_env();

    // Owner attaches BEFORE prompting — it is a LIVE forwarder for the whole
    // turn, so the mid-stream trim hits its live `sent` position.
    let owner = connect_as("gui-live-trim");
    let info = owner
        .create_session(std::env::temp_dir(), "live-trim".into(), None)
        .expect("create_session");
    owner
        .attach(&info.session_id, AttachMode::Owner)
        .expect("attach owner before turn");
    owner
        .prompt(&info.session_id, "stream a lot")
        .expect("prompt");

    // Collect everything the live owner receives until the turn ends.
    let notes = drain_until(&owner, Duration::from_secs(60), |n| {
        n.iter().any(|note| {
            matches!(note, Notification::TurnEnded { session_id, .. } if *session_id == info.session_id)
        })
    });

    // A trim MUST have fired mid-stream (proves the corruption window opened).
    let snap = owner.admin_status().expect("admin_status");
    let s = snap
        .sessions
        .iter()
        .find(|s| s.session_id == info.session_id)
        .expect("session in admin snapshot");
    assert!(
        s.log_base > 0,
        "test precondition: a trim must have advanced log_base mid-stream (CAP={CAP}, \
         CHUNKS={CHUNKS}); admin={s:#?}\nlog:\n{}",
        server.read_log()
    );

    // (a) Every chunk index 0..CHUNKS arrives EXACTLY ONCE, IN ORDER. The live
    // owner is the one stream that must never gap/dup across a trim.
    let indices: Vec<usize> = notes.iter().filter_map(stub_chunk_index).collect();
    assert_eq!(
        indices,
        (0..CHUNKS).collect::<Vec<_>>(),
        "live owner must receive every chunk exactly once, in order, across the trim \
         (no gap/dup); got {} chunks (deduped {}), log_base={}\nlog:\n{}",
        indices.len(),
        {
            let mut u = indices.clone();
            u.sort_unstable();
            u.dedup();
            u.len()
        },
        s.log_base,
        server.read_log()
    );

    // (b) The owner hard-ceiling held: the durable in-memory log was NOT allowed
    // to compact past the live owner's position while it was mid-stream. By the
    // time the turn ends the owner has caught up, so the log has now settled near
    // the cap; the load-bearing assertion is (a) — that the owner saw everything
    // despite the floor protecting it during the stream.
    assert!(
        s.event_log_len <= CAP + 1,
        "after the owner caught up the log should settle near the cap ({CAP}+marker), \
         got {}; admin={s:#?}",
        s.event_log_len
    );
}
