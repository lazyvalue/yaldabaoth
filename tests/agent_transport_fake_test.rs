//! Phase 6 (spec-session-server-actor §Rollout) — in-process `AgentTransport`
//! fake tests.
//!
//! These tests drive the `FakeTransport` directly through the `AgentTransport`
//! trait — the SAME object-safe surface the session-server's pump thread uses
//! against the real subprocess-backed `AcpChannelClient`. They prove the fake's
//! framing/ordering fidelity (FIFO `ReplyEvent` delivery, turn-boundary
//! detection, liveness flip) and the `AgentSpawner` constructor seam (real path
//! still boxes a real client; fake path returns an in-process transport and can
//! fail on demand), with NO subprocess spawned.
//!
//! WHAT THIS PROVES vs. WHAT IT DOESN'T: the fake injects at the `ReplyEvent`
//! layer — the post-deserialize currency the pump consumes — so these tests
//! exercise the reducer/forwarder/pump *logic*, not the JSON-RPC/socket wire
//! framing. The real-agent transcript tests (`session_transcript_test.rs`) and
//! crash/WAL/socket tests intentionally stay subprocess-backed to keep that
//! wire-boundary coverage. See the `keep_real` list in the Phase 6 plan.
//!
//! Requires `--features test-support` (enforced via `required-features` on the
//! `[[test]]` target in Cargo.toml).

use std::sync::Arc;

use sketch::acp_channel::{
    AgentSpawner, AgentTransport, DEFAULT_PERMISSION_MODE, FakeAgentSpawner, FakeTransport,
    PermissionMode, RealAgentSpawner, ReplyEvent, SketchFrontend,
};

/// A faithful re-implementation of the session-server pump's CORE drain logic
/// (main.rs `spawn_pump_thread`), pulled inline so these tests can assert the
/// exact ordering/turn-boundary behaviour the real pump produces — without
/// reaching into the bin crate. It drains `try_recv()` to exhaustion, then
/// checks `turn_count()` against `last_turns` to detect a completed turn.
///
/// Returns `(events, turn_ended)` for one drain cycle.
fn pump_cycle(transport: &dyn AgentTransport, last_turns: &mut usize) -> (Vec<ReplyEvent>, bool) {
    let mut events = Vec::new();
    while let Some(ev) = transport.try_recv() {
        events.push(ev);
    }
    let current = transport.turn_count();
    let turn_ended = current > *last_turns;
    if turn_ended {
        *last_turns = current;
    }
    (events, turn_ended)
}

/// FIFO fidelity: events pushed in order arrive in order, exactly once, and the
/// turn-boundary is detected the cycle the counter climbs — mirroring the real
/// notification-handler → pump path. This is the unit-level proof the fake's
/// framing matches the real client's `reply_rx` semantics.
#[test]
fn fake_preserves_event_order_and_turn_boundary() {
    let (transport, controls) = FakeTransport::new();
    assert!(transport.is_connected());
    assert_eq!(transport.turn_count(), 0);
    assert_eq!(transport.session_id().as_deref(), Some("fake-sess-0001"));

    // Push a known sequence: 5 chunks then complete the turn. The DEFAULT worker
    // bumps the turn counter only and pushes NO TurnEnded into the reply stream
    // (that variant is gated behind SKETCH_EMIT_TURN_ENDED=1) — the pump detects
    // the boundary purely via turn_count() > last_turns.
    for i in 0..5 {
        controls.push_chunk(&format!("chunk-{i}"));
    }
    controls.complete_turn();

    let mut last_turns = 0;
    let (events, turn_ended) = pump_cycle(&transport, &mut last_turns);

    // 5 chunks, in submission order, no dupes, no loss — and NO TurnEnded record,
    // matching the real default reply stream.
    assert_eq!(
        events.len(),
        5,
        "all 5 chunks drained in one cycle, no TurnEnded"
    );
    for (i, ev) in events.iter().enumerate() {
        match ev {
            ReplyEvent::Chunk(t) => assert_eq!(t, &format!("chunk-{i}")),
            other => panic!("expected Chunk at {i}, got {other:?}"),
        }
    }
    assert!(
        turn_ended,
        "turn boundary detected via the counter, not a TurnEnded event"
    );
    assert_eq!(transport.turn_count(), 1);

    // A subsequent idle cycle yields nothing and no new boundary.
    let (events, turn_ended) = pump_cycle(&transport, &mut last_turns);
    assert!(events.is_empty());
    assert!(!turn_ended);
}

/// Two back-to-back turns each advance the counter by one — the pump sees two
/// distinct boundaries via `turn_count()`, with no TurnEnded record in the
/// stream (default worker behavior).
#[test]
fn fake_multiple_turns_advance_counter() {
    let (transport, controls) = FakeTransport::new();
    let mut last_turns = 0;

    controls.push_chunk("a");
    controls.complete_turn();
    let (_e, ended1) = pump_cycle(&transport, &mut last_turns);
    assert!(ended1);
    assert_eq!(transport.turn_count(), 1);

    controls.push_chunk("b");
    controls.complete_turn();
    let (events, ended2) = pump_cycle(&transport, &mut last_turns);
    assert!(ended2);
    assert_eq!(transport.turn_count(), 2);
    // Default boundary: the drained stream is just the chunk, no TurnEnded record.
    assert!(matches!(events.last(), Some(ReplyEvent::Chunk(_))));
}

/// Liveness flip: `disconnect()` makes `is_connected()` false — the signal the
/// real pump turns into a `Command::AgentDisconnected`.
#[test]
fn fake_disconnect_flips_liveness() {
    let (transport, controls) = FakeTransport::new();
    assert!(transport.is_connected());
    controls.disconnect();
    assert!(!transport.is_connected());
}

/// The actor-facing `TransportHandle` derived from the fake shares the same
/// atomics: a prompt sent through the handle is observable on the drive side,
/// and a permission-mode change round-trips. This pins the handle/atomic
/// plumbing the permission-mode and admin-prompt tests rely on.
#[test]
fn fake_handle_shares_state_with_controls() {
    let (transport, controls) = FakeTransport::new();
    let handle = transport.handle();

    // session_id propagates through the handle.
    assert_eq!(handle.session_id().as_deref(), Some("fake-sess-0001"));

    // A prompt enqueued via the handle is observable on the drive side — proves
    // the prompt_tx/prompt_rx pair is wired (needed to assert admin_prompt
    // actually reached the agent).
    handle.send("drive a turn").expect("send on live handle");
    assert_eq!(controls.try_recv_prompt().as_deref(), Some("drive a turn"));

    // permission_mode is a shared atomic: actor writes via handle, agent reads.
    assert_eq!(controls.permission_mode(), DEFAULT_PERMISSION_MODE);
    handle.set_permission_mode(PermissionMode::ReadOnly);
    assert_eq!(controls.permission_mode(), PermissionMode::ReadOnly);

    // Liveness propagates: disconnect makes the handle's send fail.
    controls.disconnect();
    assert!(handle.send("after disconnect").is_err());
}

/// The real spawner still produces a real `AcpChannelClient` (boxed) when an
/// agent binary is available — and surfaces `NotFound` cleanly when not. We use
/// a guaranteed-absent command so this never depends on a Claude agent being
/// installed; the point is that `RealAgentSpawner` forwards to the real spawn
/// path and returns the trait object, with the error contract intact.
#[test]
fn real_spawner_forwards_to_subprocess_path() {
    let spawner: Arc<dyn AgentSpawner> = Arc::new(RealAgentSpawner);
    let result = spawner.spawn(
        "definitely-not-an-agent-binary-xyzzy",
        None,
        None,
        SketchFrontend::Gpui,
    );
    // No such binary on PATH → NotFound, surfaced through the trait unchanged.
    // (Map the Ok arm away first — `Box<dyn AgentTransport>` isn't `Debug`, so
    // `expect_err` can't print it.)
    let err = result
        .map(|_| ())
        .expect_err("absent agent should fail to spawn");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

/// The fake spawner hands out a pre-built in-process transport (no subprocess).
/// A scenario can capture the controls (via the factory) and drive a turn that
/// the consumer drains through the trait — the substrate for headless
/// manager/pump tests.
#[test]
fn fake_spawner_yields_in_process_transport() {
    use std::sync::Mutex;

    // The factory builds a fresh fake per spawn and stashes its controls so the
    // test can drive events after the (faked) spawn returns.
    let captured: Arc<Mutex<Option<sketch::acp_channel::FakeAgentControls>>> =
        Arc::new(Mutex::new(None));
    let captured_for_factory = Arc::clone(&captured);

    let spawner = FakeAgentSpawner::new(move |_cmd, _cwd, _resume| {
        let (transport, controls) = FakeTransport::new();
        *captured_for_factory.lock().unwrap() = Some(controls);
        Ok(Box::new(transport) as Box<dyn AgentTransport>)
    });

    let transport = spawner
        .spawn("", None, None, SketchFrontend::Gpui)
        .expect("fake spawn succeeds");
    assert!(transport.is_connected());

    // Drive a synthetic turn through the captured controls; the consumer drains
    // it through the same trait surface the pump uses.
    let controls_guard = captured.lock().unwrap();
    let controls = controls_guard.as_ref().expect("controls captured on spawn");
    controls.push_chunk("hello");
    controls.complete_turn();

    let mut last_turns = 0;
    let (events, turn_ended) = pump_cycle(transport.as_ref(), &mut last_turns);
    // Default boundary: just the chunk in the stream; the turn is detected via
    // the counter, not a TurnEnded record.
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ReplyEvent::Chunk(_)));
    assert!(turn_ended);
}

/// Opt-in `SKETCH_EMIT_TURN_ENDED=1` mode: `emit_turn_ended_event()` bumps the
/// counter AND pushes a `TurnEnded{count}` into the reply stream, so a scenario
/// exercising the gated forwarded-TurnEnded path sees the record. The deliberate
/// counterpart to the default counter-only `complete_turn()`.
#[test]
fn fake_emit_turn_ended_event_is_opt_in() {
    let (transport, controls) = FakeTransport::new();
    controls.push_chunk("x");
    controls.emit_turn_ended_event();

    let mut last_turns = 0;
    let (events, turn_ended) = pump_cycle(&transport, &mut last_turns);
    assert_eq!(events.len(), 2, "chunk + an explicit TurnEnded record");
    assert!(matches!(events[0], ReplyEvent::Chunk(_)));
    assert!(matches!(events[1], ReplyEvent::TurnEnded { count: 1 }));
    assert!(turn_ended);
    assert_eq!(transport.turn_count(), 1);
}

/// The fake spawner can fail on demand — exercising the `SpawnFailed` branch the
/// session-server takes when a spawn errors, deterministically (hard to hit with
/// a real subprocess).
#[test]
fn fake_spawner_can_fail_on_demand() {
    let spawner = FakeAgentSpawner::new(|_cmd, _cwd, _resume| {
        Err(std::io::Error::other("simulated spawn failure"))
    });
    let err = spawner
        .spawn("", None, None, SketchFrontend::Gpui)
        .map(|_| ())
        .expect_err("fake spawner returns the injected error");
    assert_eq!(err.kind(), std::io::ErrorKind::Other);
    assert!(err.to_string().contains("simulated spawn failure"));
}
