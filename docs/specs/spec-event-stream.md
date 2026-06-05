# Spec: Agent event-stream foundation

- **Status:** DRAFT — design synthesized + adversarially reviewed; verdict **needs-revision**. The open issues in §8 MUST be resolved before this becomes the implementation blueprint.
- **Date:** 2026-06-05
- **Provenance:** 4-aspect parallel review (enum/data, emit-forward, subscriber, reducer) → architect synthesis → adversarial critique. Companion to ADR-0006 (the principle) and the D4 durability subsystem (the durable instance of this stream).

> This is the structural foundation for agent interaction as a sourced-once, folded event stream (ADR-0006). It is a **pillar**; it is intentionally captured in full, holes included, so the revision pass has a concrete target.

## 1. Canonical event vocabulary (enum + data)

## Canonical vocabulary: ONE enum, `AgentEvent` (rename of `ReplyEvent`)

There is exactly one agent-fact vocabulary. `WorkerEvent::Reply(_)` collapses (the single-variant wrapper is deleted; the worker→driver hop carries `AgentEvent` directly — if a control channel is later needed, add a SEPARATE typed enum, never re-wrap facts). `session_proto::Notification::ReplyEvent`/`TurnEnded`/`UserPrompt` lifecycle-overlap variants collapse INTO `AgentEvent` so the same fact is never modeled twice across two enums.

```rust
// src/acp_channel.rs — the canonical, durably-logged, forwarded-verbatim vocabulary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]          // BLOCKER FIX (review 1, major serde): explicit tag
#[non_exhaustive]               // BLOCKER FIX: future variants are additive, not breaking
pub enum AgentEventKind {
    #[serde(rename = "chunk")]
    Chunk { text: String, role: ChunkRole },        // minor: un-park AgentThoughtChunk additively
    #[serde(rename = "tool_call_started")]  ToolCallStarted(ToolCall),
    #[serde(rename = "tool_call_updated")]  ToolCallUpdated(ToolCallUpdate),
    #[serde(rename = "plan_updated")]       PlanUpdated(Plan),
    #[serde(rename = "mode_changed")]       ModeChanged(SessionModeId),
    #[serde(rename = "usage_updated")]      UsageUpdated(UsageSnapshot),
    #[serde(rename = "notice")]             Notice { kind: NoticeKind, msg: String }, // split retry/info
    #[serde(rename = "user_message")]       UserMessage { text: String }, // replay echo + live submit, unified
    #[serde(rename = "turn_ended")]         TurnEnded { outcome: TurnOutcome },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkRole { Message, Thought }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind { Retry, Info }   // terminal failure is TurnOutcome::Failed, NOT a Notice

// The ACP StopReason, forwarded verbatim. Replay-end is an OUTCOME, not a sibling variant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome")]
#[non_exhaustive]
pub enum TurnOutcome {
    #[serde(rename = "completed")]  Completed,
    #[serde(rename = "cancelled")]  Cancelled,
    #[serde(rename = "max_tokens")] MaxTokens,
    #[serde(rename = "refusal")]    Refusal,
    #[serde(rename = "failed")]     Failed { msg: String },   // retry-exhausted / agent error
    #[serde(rename = "replay_end")] ReplayEnd,                // subsumes ReplayComplete
}
```

### How TurnEnded fits + the ReplayComplete / TurnEnded merge (review 1 major, review 2 major)
`ReplayComplete` is DELETED. It and the synthesized turn-end both mean "a boundary settled"; they become ONE event differentiated by `TurnOutcome`. Resume emits `TurnEnded { outcome: ReplayEnd, count, generation, .. }` exactly once after the `session/load` response (ordered strictly after the last replayed chunk, as the marker is today). A live turn emits `TurnEnded { outcome: Completed|Cancelled|MaxTokens|Refusal|Failed }` carrying the verbatim `PromptResponse.stopReason` the worker already holds at acp_channel.rs:1388. Consumers stop OR-combining two boundary concepts (kills main.rs:12154); both finalize paths collapse into the single `TurnEnded` arm of the total reducer.

### TurnFailed merge (review 1 minor)
A terminal failure is NOT a `Notice` — it is a turn boundary. The retry-exhausted/agent-error arms at acp_channel.rs:1422 emit `TurnEnded { outcome: Failed { msg } }` (which also bumps the counter, which already happens at :1435), NOT `Notice`. `Notice` keeps ONLY transient lines (retry-in-progress = `NoticeKind::Retry`). This makes terminal failure a drivable state transition (force Idle, mark turn errored) instead of a status string.

### Split/merge summary
- MERGE: `ReplayComplete` → `TurnEnded{ReplayEnd}`; `Notification::TurnEnded` → `AgentEventKind::TurnEnded`; `Notification::UserPrompt` → `AgentEventKind::UserMessage`; terminal `Notice` → `TurnEnded{Failed}`; `WorkerEvent::Reply` → bare `AgentEvent`.
- SPLIT: `Chunk(String)` → `Chunk{text, role}`; `Notice(String)` → `Notice{kind, msg}`.
- KEEP in `Notification` (server-scope, NOT agent facts): the genuine transport/lifecycle variants — `SessionAttached/Detached`, `OwnerChanged`, `SessionCreated/Closed/Renamed`. These are server→GUI control, not worker-sourced facts, and stay in `Notification` wrapping `AgentEvent` for agent facts: `Notification::Agent { event: AgentEvent }`.

## 2. Self-attributing identity (envelope)

## Every agent fact is a self-attributing envelope. Consumers NEVER infer identity.

The defect (review 1 blocker): `ReplyEvent` carries zero identity, so attribution (session id), turn number `k`, and generation are reconstructed out-of-band by three different mechanisms. Fix: identity rides ON the event as a flat envelope, sourced once at the worker (the only place that authoritatively knows it), forwarded verbatim. The server stops GRAFTING `session_id` at its hop (session_proto.rs:167); the worker stamps it.

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentEvent {
    /// Which session. Sourced at the worker, not grafted at the server hop.
    pub session_id: ServerSessionId,
    /// Channel-respawn token. Monotonic per session; bumped on force-restart.
    /// Every consumer rebaselines by comparing this, never by watching a
    /// counter appear to go backwards. (Review 1/2/3 major: generation.)
    pub generation: u64,
    /// The turn this fact belongs to. Sourced once; consumers stop re-deriving
    /// `k` via per-consumer ReplayTurns state. `seq` is the monotonic per-
    /// (session,generation) event index — the durable cursor base (see log bounding).
    pub turn: u64,
    pub seq: u64,
    pub kind: AgentEventKind,
}
```

### Field semantics (STATED, so the reducer never infers)
- `session_id`: total attribution. A consumer holding a raw `AgentEvent` can route it with zero out-of-band state. Kills the "id grafted at server hop only" finding.
- `generation`: promotes the server-internal `channel_generation` (main.rs:38) onto the wire. On force-restart the worker stamps `generation+1` on its first event. **Rebaseline rule:** any consumer seeing a `generation` strictly greater than its current one MUST run `reset_for_replay` for that session before applying. This is the SINGLE uniform rebaseline mechanism, replacing the server pump's local `last_turns=0` reset (main.rs:699-702) AND the GUI direct path which has no generation guard today. Generation closes the post-respawn wedge on every path identically.
- `turn`: the authoritative `k`. The worker owns `k` because it owns the boundary. `TurnEnded` carries the count of the turn that just closed; subsequent events carry `count+1`. This collapses `ReplayTurns`/`current_turn()` from a per-consumer inference (main.rs:12255) into a forwarded fact. `ReplayTurns` survives ONLY as a replay-cursor helper for the legacy direct path during the additive phase, then is deleted.
- `seq`: monotonic per `(session_id, generation)`, assigned at the worker. This is the ordering key AND the basis for a stable, compaction-safe cursor (see subscriber/log-bounding contracts). It is NOT a Vec index.

### Unify the user-turn fact by identity, not content (review 1/2 major)
`UserMessage` and `UserPrompt` model the same fact in two enums and dedupe by content string (the order-sensitive bug at main.rs:12359). Unified: there is ONE `AgentEventKind::UserMessage{text}` carried in ONE `AgentEvent` with `turn`/`seq`. Dedupe becomes BY IDENTITY (`(session_id, generation, turn)`) — the live optimistic insert and the replayed/forwarded echo share the same `turn`, so the reconciler suppresses by key, not by trimmed-suffix content match. The server stops emitting a separate `UserPrompt`; it forwards the worker's `UserMessage` like every other event, and ALSO emits a wake (review 4 minor: observers see the prompt immediately).

## 3. Emit-once + forward-verbatim contract

## One authoritative emitter per fact; every hop forwards verbatim.

### The rule (ADR-0006, made mechanical)
For each fact, exactly ONE site constructs the `AgentEvent`. Every downstream hop re-wraps it without reconstructing or re-deriving any field. No layer SYNTHESIZES a fact it did not author.

### Authoritative sources (the only legal emit sites)
- All ACP `SessionUpdate`-derived facts (Chunk/ToolCall*/Plan/Mode/Usage): the worker notification handler, acp_channel.rs:1011-1071. One `SessionUpdate` → one `AgentEvent`, stamped with the live `session_id`/`generation`/`turn`/`seq`.
- `TurnEnded`: the worker driver loop at acp_channel.rs:1435, where `session/prompt` resolves. The `PromptResponse.stopReason` (today discarded at :1389) is mapped to `TurnOutcome` and carried. The cancel arm (:1397) → `Cancelled`; retry-exhausted/error (:1422) → `Failed{msg}`. Resume emits `TurnEnded{ReplayEnd}` once after `session/load` (replaces the `ReplayComplete` emit at :1286).
- `UserMessage` (live submit): the OWNER's optimistic local insert is the source for its own display, but the durable/forwarded copy is sourced ONCE at the worker when it observes the submit, stamped with the assigned `turn`. (Replaces the server-side `UserPrompt` emit at main.rs:519-526.)

### What gets DELETED (the re-synthesis sites)
- Server pump turn-end inference + synthesized `Notification::TurnEnded` (main.rs:747, 799-818) → server forwards the worker's `TurnEnded`.
- GUI direct-path inference (main.rs:12138) and TUI inference (app/claude.rs:326) → consume the forwarded `TurnEnded`.
- Server `replay_fence` turn-count heuristic (main.rs:749-779) → de-dup keys off forwarded `(generation, turn, seq)`, not a re-synthesized counter.
- Straggler-drain passes (main.rs:800-810, 12143-12151) → become dead code once the boundary is an in-band event ordered after the last chunk; remove them.

### Mechanical enforcement (review 4 major: convention → invariant)
1. The server funnels EVERY logged notification through one `record(session, note)` helper that does `event_log.push` + broadcast wake atomically under the lock (ADR-0006 consequence #3). Hand-scattered `make_mut(event_log).push` sites (main.rs:282/299/393/407/523/608/621/715/795/808/817) collapse into this one mutator. The `OwnerChanged` broadcast-only path is the single NAMED exception, asserted in a test.
2. A `debug_assert` / test: any `Notification` the forwarder receives on the broadcast is either in the log by next tail OR in the known broadcast-only set — so a future variant can't be silently dropped.
3. `#[non_exhaustive]` on `AgentEventKind`/`TurnOutcome` makes additions non-breaking on the wire; in-crate consumers stay catch-all-free (see reducer contract) so additions are a COMPILE error there.

## 4. Subscriber / forwarder semantics

## Stated guarantees (not conventions) for the forwarder + client seam.

The core design — broadcast is a wake signal, `event_log` is source of truth, re-tail on every wake — is SOUND and kept (it is the one hop already doing source-once correctly). Hardening makes its guarantees explicit and fixes the unbounded-log blocker.

### Ordering
Per-session, per-generation TOTAL order by `seq`. The forwarder delivers `event_log[cursor..]` in log order; `seq` is the contractual key, not the Vec index. Cross-KIND ordering is guaranteed ONLY for log-backed agent facts. `OwnerChanged` rides the broadcast-only path and is explicitly NOT ordered relative to the transcript (review 4 minor) — documented as a transient UI flag; nothing order-sensitive may ever use the broadcast-only path.

### Replay & cursor (review 4 blocker — unbounded log + absolute cursor)
The cursor is encoded RELATIVE to a base, never a raw `Vec` index, the moment the log can shrink. `Session` gains `log_base: u64` (count of compacted-from-front entries) alongside `event_log`. The forwarder holds a private `acked_seq: u64` and tails `event_log[(acked_seq - log_base)..]`, clamping up to `log_base` on underflow and emitting a one-time `TranscriptTruncated{ from_seq }` marker so the GUI shows a gap rather than silently mis-indexing.

### Attach is O(unseen), not O(whole transcript) (review 4 major)
`Attach` accepts an optional `acked_seq` (last-seen seq) plus the `generation`/transcript-epoch the client last saw. The forwarder starts at that seq instead of 0. The server returns the current epoch; if it differs (server restart, compaction past the client's seq) the client MUST do a from-0 rebuild — signaled explicitly, not inferred. This eliminates the full re-send + full GUI rebuild on every transient reconnect (today's most user-visible cost, session_client.rs:256-262).

### Lag / backpressure
The broadcast is wake-only, so `Lagged(n)` is benign — the next tail self-heals (kept). The producer's `send` never blocks. Backpressure to a slow socket parks the forwarder in `write_all().await`; data is never lost (next tail catches up). NEW: a per-forwarder high-water policy — track unacked backlog (current `seq` minus `acked_seq`); past a threshold, log a warning and optionally disconnect the wedged observer so one stuck client can't indirectly inflate shared-log memory.

### Multi-subscriber
Owner + N observers are independent: each has its own forwarder, own `acked_seq`, own broadcast `Receiver` subscribed under the lock. A slow observer cannot stall the owner or producer (kept, correct). NEW server-side guard against duplicate Attach (review 4 major): the accept loop aborts and removes any existing `subscribed[session_id]` handle before inserting the new one (mirror the Detach path at main.rs:946-947), closing the double-delivery / leaked-task hole that today is only prevented by GUI convention.

### Log bounding (review 4 blocker)
The log is bounded by turn-granular compaction: keep the last N turns verbatim; older turns collapse to a compacted summary form, advancing `log_base`. Because the cursor is seq-relative, compaction is now a tuning knob, not a breaking change. Persistence stops deep-copying + pretty-serializing the entire log on every save (main.rs:176-182): persist incrementally (append-only journal) or only the compacted snapshot + tail.

### Client seam (review 4 minor/nit)
The note-before-wake two-channel ordering (session_client.rs:194-202) is load-bearing; documented as a contract with a test that reversing it breaks. On reconnect the consumer MUST reset local transcript state before the replay burst (now an explicit epoch-driven decision, not implicit). With seq-based attach, reconnect normally replays only unseen events.

## 5. Reducer contract (total + idempotent + total-reset)

## Total reducers, idempotent-on-(reset+replay), one total-reset.

### Total reducer (review 3 minor: incidental → structural)
`apply_agent_events` (the renamed `apply_reply_events`) and `apply_server_batch` stay catch-all-free exhaustive matches. To make totality STRUCTURAL not incidental:
- Add an exhaustiveness test that constructs EVERY `AgentEventKind` / `Notification` variant and asserts the reducer handles it (constructed via a `#[derive]`d enumerate helper or an explicit `all_variants()` list guarded by a `match` so a new variant forces the test to update).
- Doc-contract forbidding a `_ => {}` arm in the in-crate reducers. The wire enums are `#[non_exhaustive]` (so cross-version additions don't break deserialize), but in the OWNING crate exhaustiveness still forces an arm — the `#[non_exhaustive]` only relaxes OTHER crates; keep the reducer in-crate so additions remain a compile error.
- The worker-side upstream `SessionUpdate` catch-all (acp_channel.rs:1070) is replaced with a diagnostic-emitting arm (emit a `Notice{Info}` / counter) so a new ACP variant surfaces instead of vanishing.

### Idempotency contract (review 3 major: state explicit)
Idempotency is "reset-then-replay rebuilds identical state", NOT "apply is a fixpoint". `apply_agent_events` appends unconditionally; re-feeding a log onto non-empty state double-appends. This is made EXPLICIT and ENFORCED:
- Doc the contract at the reducer: the ONLY legal way to re-feed the log is `reset_for_replay` first.
- `debug_assert` at the replay entry path that the transcript is empty before a replay burst, so replay-without-reset is caught loudly.
- Dedupe of the live/replayed user-turn overlap is BY IDENTITY (`(session_id, generation, turn)`) via the reconciler, replacing content-string dedupe — now order-insensitive.
- A replay is byte-identical-rebuilding ONLY if the log is lossless; `ToolCallUpdated` with no prior `ToolCallStarted` is still synthesized (main.rs:12313) but the reducer should surface a diagnostic so a lossy log is observable, not silent.

### Total reset (review 3 major: agent_mode omission)
`reset_for_replay` (main.rs:5408) hand-enumerating ~22 fields is the structural defect that dropped `agent_mode`. Fix structurally: split `AgentState` into `ConnectionPrefs` (preserved across replay) and a nested `TranscriptState` struct holding EVERY transcript-derived field (transcript, current_plan, usage, subagents, agent_mode, tool_calls, ReplayTurns/reconciler state, status). `reset_for_replay` replaces `TranscriptState` wholesale via `TranscriptState::default()` (or `::new(generation)`), so a struct-literal forces every field to be considered — the compiler would have caught `agent_mode`. Reset is triggered by the generation-bump rule (one uniform path), not per-caller.

### Generation = the reset trigger
Seeing `event.generation > current` runs the total reset for that session before applying the event. This unifies "respawn rebaseline" and "reconnect replay" under one mechanism: both bump/observe generation, both reset, both replay from the (seq-based) log.

## 6. Vocabulary evolution rules

## How the vocabulary evolves without breaking consumers.

1. **Additive-only on the wire.** `AgentEventKind`, `TurnOutcome`, `NoticeKind`, `ChunkRole` are `#[non_exhaustive]` and serde-tagged with EXPLICIT `#[serde(rename)]` on every variant/struct (the gap at acp_channel.rs:146 vs the safe pattern in session_proto.rs:54). A renamed Rust identifier therefore can NOT silently break the durable `event_log` or a cross-version wire payload. New variants deserialize on old code as an unknown tag → handled by the wire layer's tolerant decode (skip-with-diagnostic), never a hard error.

2. **One fact, one variant, one source — forever.** A new transition is added as a new `AgentEventKind` variant emitted at ONE authoritative source and forwarded verbatim. It is NEVER re-synthesized at a downstream hop, and NEVER modeled simultaneously in `Notification` and `AgentEventKind` (the original sin this design removes). If a fact is server-scoped (lifecycle/ownership) it lives in `Notification`; if it is a worker-observed agent fact it lives in `AgentEventKind`. The boundary is: "does the worker authoritatively observe it?"

3. **Prefer payload growth over new variants for refinements.** Refining an existing fact (e.g. more `TurnOutcome` cases, a new `ChunkRole`) adds an enum case to the payload, not a sibling top-level variant — keeping the reducer's arm count stable and the fact's identity singular. This is exactly why `ReplayEnd`/`Failed` are `TurnOutcome` cases, not new `AgentEventKind` variants beside `TurnEnded`.

4. **In-crate exhaustiveness is the enforcement mechanism.** Despite `#[non_exhaustive]` on the wire, the OWNING-crate reducers stay catch-all-free, so any new variant is a compile error that forces every consumer to handle it. The exhaustiveness test (constructs all variants) is the regression guard against a future `_ => {}`.

5. **Identity fields are append-only and defaulted.** New envelope fields (`session_id`/`generation`/`turn`/`seq` and any future addition) use `#[serde(default)]` so old logs deserialize. Identity semantics, once shipped, are frozen — `seq` is forever the per-(session,generation) ordering key; `generation` is forever the reset trigger.

6. **Compaction format is versioned.** The compacted-summary form (log bounding) carries its own schema version so the persisted journal can evolve independently of the live vocabulary.

7. **ADR amendments, not silent drift.** Any change to the emit-once source of an existing fact, the reset-then-replay contract, or the seq/generation semantics requires a follow-on ADR amending this one — these are the load-bearing invariants the whole pillar rests on.

## 7. Open risks (author-flagged)

- seq/turn must be assigned at the worker under the SAME ordering as the events are sent, or the cursor/ordering guarantee breaks. The worker has a pump_task draining WorkerEvent into an mpsc (acp_channel.rs:963-978); seq must be stamped at construction in the notification handler / driver, before the mpsc, and the mpsc must preserve order (it does, single producer per channel) — verify no reordering across the handler vs driver-loop emit paths (both emit on the same event_tx, but TurnEnded comes from the driver while chunks come from the handler; their relative order must match wall-clock, which ACP guarantees since updates arrive while the prompt RPC is pending).
- Generation must be assigned at the worker, but today it lives ONLY in the server (channel_generation, main.rs:38). The worker channel is respawned BY the server, so the server knows the new generation before the worker emits — the generation value must be injected INTO the worker at spawn (constructor arg) so the worker can stamp it, rather than the server stamping on forward (which would reintroduce a graft-at-hop). Needs a plumbing decision: pass generation into AcpChannelClient::new.
- Turn-granular compaction (log bounding) must not compact a turn that an attached observer has not yet acked, or it forces a gap-marker for a live-but-slow client. Compaction watermark = min(acked_seq) across attached forwarders, OR accept the gap-marker for laggards. Pick a policy explicitly; min-across-subscribers reintroduces a slow-client-stalls-compaction coupling.
- The additive rollout (ADR-0006) requires emitting TurnEnded AND keeping the inference to assert agreement. During that window, generation/turn/seq envelope fields and the old counter-based path coexist — define the cutover precisely so the direct path (server_managed=false, main.rs:11486) doesn't double-finalize (forwarded TurnEnded + inferred turn_ended).
- Merging UserPrompt into UserMessage changes the live-submit delivery path: today the owner never receives its own UserPrompt (logged-only). Unifying to one forwarded UserMessage means the owner WILL receive an echo of its own optimistic insert — the identity-based reconciler MUST suppress it by (session_id,generation,turn); verify the optimistic insert is stamped with the same turn the worker will assign, which requires the turn number to be known at submit time or reconciled when the echo arrives.
- ReplayEnd-as-TurnOutcome means the reducer's TurnEnded arm must branch on outcome to decide finalize-vs-replay-finish semantics (today split across finish_replay at acp_channel.rs:245 and finalize_agent_turn). Folding both into one arm risks the mid-replay premature-Idle edge (review 3 minor, main.rs:11889-11893) — the arm must NOT flip to Idle on ReplayEnd if more generations/turns are pending; needs an explicit replay-vs-live state check.
- Persisted event_log format changes (incremental journal + compaction) is a migration: existing on-disk session snapshots (main.rs:204) are full pretty-JSON logs. Need a one-time loader that ingests the old format and re-bases it into the new seq/log_base model, or a version gate.

## 8. Adversarial critique — MUST resolve before implementation

Verdict: **needs-revision**

### Holes

- GENERATION/REBASELINE RACE (the design's own keystone, underspecified to the point of being wrong as written). The design says 'generation is assigned at the worker' and 'any consumer seeing generation > current MUST reset_for_replay'. But the worker is RESPAWNED BY the server (main.rs:580-627): the server bumps channel_generation AFTER spawn_with_resume_in returns (main.rs:603), and the brand-new worker has ALREADY started emitting its session/load replay chunks (acp_channel.rs:1011 handler fires on the first session/update) BEFORE the server's bump line runs and before any generation value could be injected. open_risk #2 admits the value must be injected at constructor time, but the design never resolves the ordering: which generation do the resume chunks carry? Concrete break: GUI sees generation-N replay chunks, then a generation-N SessionAttached, never resets, and appends the resumed transcript onto the live one. This is the exact post-respawn wedge the design claims to close, relocated. The SessionAttached note (main.rs:604-609) is pushed at bump time, AFTER replay chunks may already be queued on the worker's std mpsc.
- SEQ/TURN ORDERING ACROSS THE TWO WORKER EMIT PATHS (open_risk #1 hand-waves the actual hazard). seq must be 'assigned in the notification handler/driver before the mpsc.' But there are TWO senders on event_tx: the SDK notification handler (acp_channel.rs:1016-1056, async callback context) and the driver loop (acp_channel.rs:1408/1422 Notice, plus the new TurnEnded at :1435). A shared monotonic seq requires an atomic fetch_add at each emit. The design's 'single producer per channel' refers to the pump DRAIN side; the SEND side has two producers. If the handler emits chunk(seq=5) and the driver concurrently emits TurnEnded(seq=6) but the executor lands the driver's send into the mpsc first, seq order and mpsc/log order disagree, violating the forwarder's 'total order by seq' contract. The design needs seq assigned under the same synchronization as the mpsc enqueue and never specifies it. ACP guarantees updates arrive while the RPC is pending but does NOT order the driver's post-resolution TurnEnded send after the last handler send — separate tasks racing on event_tx.
- REPLAY-ON-ATTACH INTERLEAVING / epoch is undefined. The new Attach{acked_seq, epoch} starts the forwarder at acked_seq if epoch matches, else from-0 rebuild. But 'epoch' is given no concrete definition. Is epoch == generation, a server-boot nonce, or log_base? They invalidate differently. If epoch is a server-boot id, a force-restart (bumps generation but not server boot, main.rs:603) lets the client resume at a stale acked_seq into a generation-N+1 log where seq reset — duplicate/skipped events. Epoch must be the (generation, log_base) pair with the predicate 'acked_seq resolvable in current generation AND >= log_base', which the design never states. Also a TOCTOU window: between the client computing acked_seq and the server processing Attach, compaction can advance log_base past acked_seq; the design's clamp-on-underflow + TranscriptTruncated handles indexing but the epoch comparison that decides full-vs-incremental is unspecified.
- TWO SUBSCRIBERS + COMPACTION WATERMARK left as 'pick a policy' (open_risk #3) with both options broken for the stated goals. Option A (compact only up to min(acked_seq) across forwarders) reintroduces the slow-client-stalls-shared-resource coupling the multi-subscriber section claims to eliminate — a wedged observer pins unbounded log growth, only partially mitigated by the 'optionally disconnect' policy. Option B (compact regardless, emit TranscriptTruncated) means a merely-slow-but-healthy OWNER gets a gap in its own authoritative transcript mid-session, forcing an unneeded from-0 rebuild — data loss from the owner's perspective. The design ships both the high-water-disconnect AND the gap-marker without specifying which fires first, so a slow owner could be either disconnected or gapped depending on unspecified ordering.
- RECONNECT MID-TURN double-finalize during additive rollout (open_risk #4 names it; no mechanical cutover given). During the additive phase the worker emits TurnEnded AND the counter inference is kept 'to assert agreement.' On the direct path (server_managed=false, main.rs:11486/12138) the GUI infers turn_ended from turn_count() climbing. If a forwarded TurnEnded{Completed} arrives AND the same cycle's turn_count() climbed, finalize_agent_turn runs twice (forwarded-TurnEnded arm + main.rs:12155). finalize is not specified idempotent — it inserts an editable line below frozen content — so two finalizes = two trailing editable lines or a flipped turn_phase. 'Define the cutover precisely' is asserted but never done.
- REPLAYEND-AS-OUTCOME: the worker does NOT authoritatively observe historical turn boundaries during session/load, so 'the worker owns k because it owns the boundary' is FALSE for replayed turns. ACP replays history as session/update chunks with NO per-turn RPC resolution — only the single final session/load response. The design says TurnEnded{ReplayEnd} fires 'exactly once after session/load' yet also says 'TurnEnded carries the count of the turn that just closed; subsequent events carry count+1', implying a per-replayed-turn boundary the worker cannot observe. So the 'turn' field on replayed chunks must be reconstructed by the worker from the replay stream — the very inference the design forbids. open_risk #6's 'must NOT flip to Idle on ReplayEnd if more turns pending' has no mechanism because the single folded TurnEnded arm now needs external replay-vs-live state that the design removed.
- UNBOUNDED/COMPACTED LOG vs IDEMPOTENCY contradiction. The reducer contract states reset+replay rebuilds identical state ONLY if the log is lossless (ToolCallUpdated-without-Started is synthesized, main.rs:12313). Turn-granular compaction 'collapses older turns to a compacted summary form' — lossy by definition. So any reconnect replaying from a compacted base CANNOT rebuild identical state, violating the idempotency the design relies on after every reconnect. Treating compaction as 'a tuning knob, not a breaking change' because the cursor is seq-relative only fixes INDEXING, not the semantic fact that the replayed prefix is now a summary the total reducer has NO arm for — there is no AgentEventKind::CompactedSummary in the vocabulary. Compaction is not actually expressible in the stream as designed.
- NEW EVENT VARIANT A YEAR FROM NOW: the #[non_exhaustive] + 'tolerant decode (skip-with-diagnostic)' claim is not free with serde internally-tagged enums. #[serde(tag="kind")] ERRORS on an unknown tag ('data did not match any variant'); it does NOT skip. #[serde(other)] is unit-variant-only and cannot capture payload, so it would DROP the bytes from the durable source-of-truth log entirely — not just skip-render. For a durably-logged, cross-version-FORWARDED vocabulary, an older node forwarding a newer variant it can't decode would corrupt the log it passes on. The design asserts tolerant decode as a property of #[non_exhaustive] but it requires a custom Deserialize or an explicit Unknown{raw: Value, tag} catch-all that PRESERVES bytes — neither is in the design. Additionally, an older GUI replaying a newer durable log would silently produce a DIFFERENT transcript than intended, while the idempotency contract asserts identical-rebuild; 'identical to what' becomes version-dependent.
- OUT-OF-ORDER / racy backpressure read. record() (mechanical-enforcement #1) fuses push+wake under the lock for logged events — good. But the design never states that the high-water backlog computation (seq - acked_seq) that decides whether to disconnect a wedged observer is read under the SAME lock as record(). If not, a forwarder can observe a stale acked_seq concurrent with a push and either spuriously disconnect a healthy client or fail to disconnect a wedged one. The backpressure threshold is specified as a policy but not as a synchronized read, leaving the slow-subscriber guard non-deterministic.

### Recommendations

- Resolve generation-assignment ordering concretely: inject generation into AcpChannelClient at spawn (open_risk #2) AND have the worker stamp it on a synthetic 'generation opened' marker (or the session/load's first emission) emitted BEFORE any replay chunk — not pushed by the server at main.rs:603 after the worker has started. The rebaseline-on-increase rule only works if generation rides the first event of the new channel.
- Define epoch explicitly as the pair (generation, log_base) with one resume-validity predicate: 'acked_seq >= log_base AND client_generation == server_generation', else from-0. State it once in the subscriber contract so server-restart, compaction-past-cursor, and force-restart all funnel through one comparison.
- Make finalize_agent_turn idempotent (no-op if already finalized for this (generation,turn)). This removes the need for a precise additive-rollout cutover entirely and neutralizes any duplicate forwarded TurnEnded — cheaper and safer than 'define the cutover precisely.'
- Confront the replayed-turn boundary problem: during session/load the worker observes no per-turn RPC, so it cannot authoritatively stamp 'turn' per replayed chunk. Either (a) document that replay turn-numbering IS a deterministic inference from the lossless log (an explicit exception to 'worker owns k'), or (b) persist per-event turn/seq IN the log so resume re-emits them verbatim. Option (b) aligns with 'transcript is a projection of the log' and is the only choice consistent with the design's own emit-once principle.
- Add an explicit CompactedSummary/TranscriptTruncated variant to the wire vocabulary with a reducer arm that renders a gap placeholder, and restate idempotency as 'reset+replay rebuilds identical state from the UNCOMPACTED tail; the compacted prefix rebuilds to a deterministic summary placeholder.' Without a vocabulary variant, turn-granular compaction is not expressible in the stream.
- Replace assumed serde 'tolerant decode' with a concrete mechanism: a custom Deserialize or an explicit Unknown{raw: serde_json::Value, tag: String} catch-all that PRESERVES bytes, so a forwarding-but-not-rendering older node never corrupts the durable log it passes on. #[serde(other)] alone drops payload from the source-of-truth log.
- Pick the compaction watermark policy explicitly and tie it to subscriber role: never gap the OWNER (compact no further than owner acked_seq, or force the owner to a clean reconnect); allow gapping/disconnecting observers past the high-water threshold; state that high-water disconnect fires before any gap-marker. Compute (seq - acked_seq) under the same lock as record() so the threshold read is not racy.
- Implement the additive rollout as a single feature gate over all three inference sites ('!has_forwarded_turn_ended_in_stream') so the moment a real TurnEnded variant appears every inference dies in the same commit, minimizing the dual-path window where double-finalize lives.
