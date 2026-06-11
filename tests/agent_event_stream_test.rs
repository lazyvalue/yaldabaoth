//! Phase 8 Stage A (spec-event-stream §1/§2/§9) — headless AgentEvent-stream
//! tests, fake-backed (no subprocess).
//!
//! These prove the ADDITIVE producer collapse: the canonical `AgentEvent` stream
//! the server forwards (`Notification::Agent { event }`) AGREES with the legacy
//! `ReplyEvent` / turn-inference path it ships alongside (spec §9), and the
//! forward-compat `Unknown` catch-all round-trips a newer variant byte-faithfully
//! (spec §8 — the load-bearing cross-version-forwarding guard).
//!
//! The server bin's `Manager`/`record_agent` isn't reachable from an integration
//! test, so — exactly like `agent_transport_fake_test.rs` re-implements the
//! pump's drain logic inline — this re-implements the server's "emit alongside"
//! mapping inline against the SAME `agent_event` lib seam the bin calls
//! (`agent_kind_from_reply` / `replay_end_kind` / `turn_ended_kind`). When the
//! server bin and this model both route through that one lib function, agreement
//! here pins agreement there.
//!
//! Requires `--features test-support`.

use yalda::acp_channel::{AgentTransport, FakeTransport, ReplyEvent};
use yalda::agent_event::{
    AgentEvent, AgentEventKind, ChunkRole, TurnOutcome, agent_kind_from_reply, replay_end_kind,
    turn_ended_kind,
};

/// Model of the server's additive emit path (main.rs `Command::Record` +
/// `Command::TurnCount`): drain the worker's `ReplyEvent` stream and, for each
/// event, ALSO produce the canonical `AgentEvent` the server would `record_agent`
/// — assigning the envelope identity `(session_id, generation, turn, seq)` the
/// same way the server does (monotonic `seq`; `turn` = current settled count for
/// in-flight events, `turns - 1` for a settled boundary).
struct ServerModel {
    session_id: String,
    generation: u64,
    turns: u64,
    seq: u64,
    agent_stream: Vec<AgentEvent>,
}

impl ServerModel {
    fn new(session_id: &str, generation: u64) -> Self {
        Self {
            session_id: session_id.into(),
            generation,
            turns: 0,
            seq: 0,
            agent_stream: Vec::new(),
        }
    }

    fn emit(&mut self, turn: u64, kind: AgentEventKind) {
        let ev = AgentEvent::new(
            self.session_id.clone(),
            self.generation,
            turn,
            self.seq,
            kind,
        );
        self.seq += 1;
        self.agent_stream.push(ev);
    }

    /// Mirror `Command::Record`: a streamed `ReplyEvent` becomes its
    /// `AgentEventKind` (chunk/tool/etc) at the IN-FLIGHT turn; `ReplayComplete`
    /// becomes `ReplayEnd`. `TurnEnded { count }` is envelope-authoritative and
    /// handled by `settle_turn` instead.
    fn ingest_reply(&mut self, reply: &ReplyEvent) {
        if let Some(kind) = agent_kind_from_reply(reply) {
            let turn = self.turns; // in-flight turn (0-based)
            self.emit(turn, kind);
        } else if matches!(reply, ReplyEvent::ReplayComplete) {
            let turn = self.turns;
            self.emit(turn, replay_end_kind());
        }
        // ReplyEvent::TurnEnded → no-op here (settled via settle_turn).
    }

    /// Mirror `Command::TurnCount`: a settled live turn. `new_count` is the
    /// 1-based settled count; the boundary's envelope turn is `new_count - 1`.
    fn settle_turn(&mut self, new_count: u64) {
        self.turns = new_count;
        let completed = new_count.saturating_sub(1);
        self.emit(completed, turn_ended_kind(TurnOutcome::Completed));
    }
}

/// Drain the fake transport to exhaustion and detect the inference boundary the
/// same way the real pump does (`turn_count() > last_turns`).
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

/// §9 AGREEMENT: for a normal turn (a few chunks + a tool call, then a settled
/// boundary), the forwarded `AgentEvent` stream describes exactly the same facts
/// as the legacy `ReplyEvent` stream + the inference — no duplicated chunks, and
/// the forwarded `TurnEnded`'s envelope `turn` equals what the inference's
/// counter-climb implies.
#[test]
fn agent_stream_agrees_with_reply_inference_for_one_turn() {
    let (transport, controls) = FakeTransport::new();
    let mut model = ServerModel::new("sess-A", 0);

    // The worker streams a few chunks + a tool call, then settles the turn
    // (DEFAULT worker: counter bump only, no TurnEnded record in the stream).
    controls.push_chunk("hello ");
    controls.push_chunk("world");
    controls.complete_turn();

    let mut last_turns = 0;
    let (events, inferred_turn_ended) = pump_cycle(&transport, &mut last_turns);

    // The legacy stream the GUI applies: just the two chunks.
    assert_eq!(
        events.len(),
        2,
        "two chunks, no TurnEnded record (default worker)"
    );
    for ev in &events {
        model.ingest_reply(ev);
    }
    // The inference fires the boundary; the server records the settled count.
    assert!(
        inferred_turn_ended,
        "inference detects the boundary via the counter"
    );
    let inferred_count = transport.turn_count() as u64; // == 1
    model.settle_turn(inferred_count);

    // AGREEMENT 1 — no duplicated chunks: exactly two Chunk events in the agent
    // stream, matching the two legacy chunks (NOT four).
    let chunk_count = model
        .agent_stream
        .iter()
        .filter(|e| matches!(e.kind, AgentEventKind::Chunk { .. }))
        .count();
    assert_eq!(chunk_count, 2, "agent stream must not double-apply chunks");

    // AGREEMENT 2 — the forwarded TurnEnded's envelope turn == inferred boundary.
    let ended: Vec<&AgentEvent> = model
        .agent_stream
        .iter()
        .filter(|e| matches!(e.kind, AgentEventKind::TurnEnded { .. }))
        .collect();
    assert_eq!(ended.len(), 1, "exactly one TurnEnded forwarded");
    assert_eq!(
        ended[0].turn,
        inferred_count - 1,
        "forwarded TurnEnded turn must equal (inferred settled count - 1)"
    );

    // Envelope invariants: seq is monotonic and dense, generation is stable.
    for (i, e) in model.agent_stream.iter().enumerate() {
        assert_eq!(
            e.seq, i as u64,
            "seq monotonic + dense per (session,generation)"
        );
        assert_eq!(e.generation, 0);
        assert_eq!(e.session_id, "sess-A");
    }
}

/// §9 AGREEMENT across two turns: each counter climb maps to exactly one
/// forwarded `TurnEnded`, with increasing envelope `turn` (0 then 1).
#[test]
fn agent_stream_agrees_across_two_turns() {
    let (transport, controls) = FakeTransport::new();
    let mut model = ServerModel::new("sess-B", 0);
    let mut last_turns = 0;

    // Turn 1
    controls.push_chunk("a");
    controls.complete_turn();
    let (events, ended) = pump_cycle(&transport, &mut last_turns);
    for ev in &events {
        model.ingest_reply(ev);
    }
    assert!(ended);
    model.settle_turn(transport.turn_count() as u64);

    // Turn 2
    controls.push_chunk("b");
    controls.complete_turn();
    let (events, ended) = pump_cycle(&transport, &mut last_turns);
    for ev in &events {
        model.ingest_reply(ev);
    }
    assert!(ended);
    model.settle_turn(transport.turn_count() as u64);

    let ended_turns: Vec<u64> = model
        .agent_stream
        .iter()
        .filter_map(|e| match e.kind {
            AgentEventKind::TurnEnded { .. } => Some(e.turn),
            _ => None,
        })
        .collect();
    assert_eq!(ended_turns, vec![0, 1], "two boundaries at turns 0 then 1");
}

/// ReplayComplete folds into a `TurnEnded { ReplayEnd }` — and ReplayEnd is NOT a
/// live boundary, so it does not advance the settled turn count.
#[test]
fn replay_complete_folds_into_replay_end() {
    let (transport, controls) = FakeTransport::new();
    let mut model = ServerModel::new("sess-C", 0);
    let mut last_turns = 0;

    // A resume burst: a replayed user message + chunk, then ReplayComplete.
    controls.push(ReplyEvent::UserMessage("prior prompt".into()));
    controls.push_chunk("prior reply");
    controls.push(ReplyEvent::ReplayComplete);
    let (events, _ended) = pump_cycle(&transport, &mut last_turns);
    for ev in &events {
        model.ingest_reply(ev);
    }

    let kinds: Vec<&AgentEventKind> = model.agent_stream.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], AgentEventKind::UserMessage { .. }));
    assert!(matches!(
        kinds[1],
        AgentEventKind::Chunk {
            role: ChunkRole::Message,
            ..
        }
    ));
    assert!(matches!(
        kinds[2],
        AgentEventKind::TurnEnded {
            outcome: TurnOutcome::ReplayEnd
        }
    ));
    assert_eq!(model.turns, 0, "ReplayEnd is not a settled live turn");
}

/// Spec §8 — the load-bearing cross-version guard, end-to-end through a
/// `Notification::Agent` wrapper: an older decoder lands a newer `kind` in
/// `Unknown` and re-emits it under its ORIGINAL tag, byte-faithful, so a
/// forwarding node never corrupts the durable WAL.
#[test]
fn unknown_agent_event_round_trips_through_notification() {
    use yalda::session_proto::Notification;

    // A future server wrote this `Notification::Agent` with a kind this build
    // doesn't know.
    let wire = r#"{"type":"agent","event":{"session_id":"s1","generation":2,"turn":4,"seq":9,"kind":"speculative_decode","draft_tokens":7}}"#;
    let note: Notification = serde_json::from_str(wire).expect("forwarding node must decode");
    let event = match &note {
        Notification::Agent { event } => event,
        other => panic!("expected Agent, got {other:?}"),
    };
    match &event.kind {
        AgentEventKind::Unknown { tag, raw } => {
            assert_eq!(tag, "speculative_decode");
            assert_eq!(raw.get("draft_tokens").and_then(|v| v.as_u64()), Some(7));
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
    // Envelope identity is still readable even for an unknown kind.
    assert_eq!(event.generation, 2);
    assert_eq!(event.turn, 4);
    assert_eq!(event.seq, 9);

    // Re-serialize (forward verbatim): the kind tag is the ORIGINAL, the payload
    // survives, and a second decode is stable.
    let reser = serde_json::to_string(&note).unwrap();
    assert!(reser.contains("\"kind\":\"speculative_decode\""));
    assert!(!reser.contains("\"kind\":\"unknown\""));
    assert!(reser.contains("\"draft_tokens\":7"));
    let again: Notification = serde_json::from_str(&reser).unwrap();
    match again {
        Notification::Agent { event: e2 } => assert_eq!(e2, event.clone()),
        other => panic!("expected Agent, got {other:?}"),
    }
}
