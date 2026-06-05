# Spec: Agent event-stream foundation

- **Status:** Revised — hardening pass 2. Pass 1 resolved the 9 original holes;
  an adversarial re-validation found them holding at 7/9 but flagged two
  under-specified seams (server-restart authority, ownerless compaction) plus
  plumbing blast-radius. Pass 2 closes those (§5, §6, §10 pass-2 table, §12).
  **Design-level solid**; the remaining items are implementation-time plumbing,
  enumerated in §12.
- **Date:** 2026-06-05
- **Provenance:** 4-aspect parallel review → architect synthesis → adversarial
  critique (verdict: needs-revision) → this hardening pass.
- **Related:** ADR-0006 (the principle), D4 durable-log subsystem (the durable
  instance of this stream), the self-hosting candidate/promote flow (two GUI
  versions co-attach one server session — makes cross-version forwarding real).

> Agent interaction is a **sourced-once, forwarded-verbatim, folded event
> stream** (ADR-0006). This spec pins the vocabulary, the identity envelope, the
> emit/forward/subscriber/reducer contracts, and evolution — to the level of
> rigor a load-bearing pillar needs.

---

## 1. Canonical vocabulary — one enum, `AgentEvent`

There is exactly one agent-fact vocabulary. The `WorkerEvent::Reply` wrapper is
deleted (worker→driver carries `AgentEvent` directly; a control channel, if ever
needed, is a *separate* typed enum — facts are never re-wrapped). The duplicate
lifecycle variants in `session_proto::Notification` (`ReplyEvent`, `TurnEnded`,
`UserPrompt`) collapse **into** `AgentEvent` so no fact is modeled in two enums.
`Notification` keeps only genuine server→GUI control that is *not* a worker fact
(`SessionAttached/Detached`, `OwnerChanged`, `SessionCreated/Closed/Renamed`),
wrapping agent facts as `Notification::Agent { event: AgentEvent }`.

```rust
// src/<lib>/agent_event.rs — canonical, durably-logged, forwarded-verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub session_id: ServerSessionId,  // sourced at the worker, not grafted downstream
    pub generation: u64,              // channel-respawn token; rides the FIRST event of a channel
    pub turn: u64,                    // authoritative k (see §5)
    pub seq: u64,                     // monotonic per (session, generation); ordering + cursor base
    pub kind: AgentEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEventKind {
    ChannelOpened { resumed: bool },         // FIRST event of every (re)spawned channel — §4
    Chunk { text: String, role: ChunkRole }, // role: Message | Thought (un-parks AgentThoughtChunk)
    ToolCallStarted(ToolCall),
    ToolCallUpdated(ToolCallUpdate),
    PlanUpdated(Plan),
    ModeChanged(SessionModeId),
    UsageUpdated(UsageSnapshot),
    Notice { kind: NoticeKind, msg: String },// transient ONLY (Retry | Info); terminal failure is an outcome
    UserMessage { text: String },            // live submit + replay echo, unified; dedup by identity (§5)
    TurnEnded { outcome: TurnOutcome },       // subsumes ReplayComplete; terminal failure lives here
    CompactedSummary { through_turn: u64, summary: String }, // §7 — compaction is expressible in the stream
    // Forward-compatibility: an older decoder lands unknown variants HERE,
    // preserving bytes so a forwarding node never corrupts the durable log (§8).
    Unknown { tag: String, raw: serde_json::Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed, Cancelled, MaxTokens, Refusal,
    Failed { msg: String },  // retry-exhausted / agent error — a boundary, not a Notice string
    ReplayEnd,               // end of the replayed history prefix (§5)
}
```

`ChunkRole`, `NoticeKind` are small snake_case enums. `TurnEnded` carries the
verbatim ACP `PromptResponse.stopReason` for live turns.

---

## 2. Identity envelope — consumers never infer identity

Every fact is a self-attributing envelope. The four envelope fields are sourced
**once, at the worker** (the only place that authoritatively knows them) and
forwarded verbatim; the server stops grafting `session_id` at its hop.

- `session_id` — total attribution; a consumer holding a raw `AgentEvent` routes
  it with zero out-of-band state.
- `generation` — the channel-respawn token; the **single uniform rebaseline
  signal** (§4). Replaces the server pump's local `last_turns=0` reset and the
  GUI direct path's (absent) guard.
- `turn` — the authoritative `k` (§5). Collapses per-consumer `ReplayTurns`
  inference into a forwarded fact in the common case.
- `seq` — monotonic per `(session_id, generation)`, assigned at the emit
  chokepoint (§3). The ordering key **and** the compaction-safe cursor base — a
  logical offset, never a `Vec` index.

User-turn dedup is **by identity** `(session_id, generation, turn)`, not by
content string — the live optimistic insert and the forwarded/replayed echo
share the same `turn`, so the reconciler suppresses the duplicate deterministically.

---

## 3. The emit chokepoint — one critical section (resolves H2, H9)

There are two *send-side* producers on the worker: the SDK notification handler
and the driver loop (`TurnEnded`, `Notice`). A naive shared `seq` raced. So all
emission funnels through **one `emit(kind)` chokepoint** that, under a single
mutex, atomically: (a) assigns `seq = next_seq++` and the current `turn`, (b)
enqueues the `AgentEvent`, and (c) on the server, appends to `event_log` +
broadcasts (the fused `record()`, already shipped as quick win #4) **and** reads
the backlog watermark `seq - min(acked_seq)`.

Because seq-assignment, enqueue, log-append, and watermark-read share one lock,
**seq order == channel/log order == watermark view** by construction — no task
scheduling can make them disagree. This is principle "one atomic mutator per
coupled invariant" applied to `(seq, enqueue, log, watermark)`. The slow-
subscriber guard (§6) is therefore a synchronized, deterministic read, not racy.

**The critical section is `await`-free** — a std `Mutex` held across an `.await`
is the documented pump footgun. `emit()` does only seq/turn-assign + enqueue
(and, on the server, log-append + broadcast + watermark-read) and returns. On the
server this *lengthens* the existing `sessions`-lock section that `record()`
already holds, so it must stay short and await-free (§11).

---

## 4. Generation lifecycle — ride the first event (resolves H1)

The bug in the draft: the server bumped `channel_generation` *after* spawn
returned, but the respawned worker had already emitted `session/load` replay
chunks — so they carried the wrong generation and the rebaseline never fired.

Hardened: **generation is allocated before spawn and injected into the worker at
construction** (`spawn_with_resume_in(cmd, cwd, resume_id, generation)`). The
worker's **first emission is always `ChannelOpened { resumed }`**, stamped with
that generation, emitted *before any replay chunk*. So the generation bump is
literally the first event of the new channel.

**Rebaseline rule (uniform, every path):** a consumer that sees an `AgentEvent`
whose `generation` is strictly greater than its current one MUST run
`reset_for_replay` for that session *before* applying it. `ChannelOpened`
guarantees that bump arrives first; the rule still holds if any later event is
the first observed (idempotent reset). This single rule replaces the server-pump
reset and closes the post-respawn wedge identically on the server and direct paths.

**Emission site (precise — resolves N2):** `ChannelOpened` is emitted from the
connection closure *before* the worker sends `session/load`/`session/new` (~`acp_channel.rs:1135`,
before `:1268`) — **not** from the notification handler, which can fire the
instant the load RPC is in flight and would race the agent's first replay chunk.
**Generation allocation:** the *server* allocates `generation` (monotonic per
session, persisted with it) and passes it to `spawn_with_resume_in(.., generation)`
at **all three** spawn sites — create (`main.rs:374`), restore (`:280`), and
force-restart (`:601`) — bumping on every (re)spawn (a new worker process is a
new channel). Today only force-restart bumps and restore sets `0`; both must allocate.

---

## 5. Turn ownership — live vs replay (resolves H6)

"The worker owns `k` because it owns the boundary" is true for **live** turns
(the `session/prompt` RPC resolves once, at the boundary, `acp_channel.rs:1388`)
but **false for replayed history**: ACP replays history as `session/update`
chunks with no per-turn RPC — only the single final `session/load` response.

Hardened, per the emit-once principle (Rec 4b): **`turn` and `seq` are persisted
per-event in the durable log (D4) and re-emitted verbatim on resume.** So in the
normal case (our server has the durable log) there is *no* reconstruction — the
worker/server forward the stored `turn`/`seq`. `TurnEnded { ReplayEnd }` marks
the end of the replayed prefix; the live counter resumes at `max(turn)+1`.

The **one bounded exception**, documented explicitly: a *logless* external
`session/load` (resuming an ACP session the server never logged — no D4 record
exists). There, and only there, turn-numbering is a deterministic inference from
the replay stream via the surviving `ReplayTurns` helper. `ReplayTurns` is thus
demoted to "the logless-resume fallback," not a per-consumer mechanism.

**Two replay directions, distinct authority (resolves N4 — the server-restart
double-ingest).** "Forward verbatim" applies to exactly one direction:
1. *server → GUI* (re-attach): the server forwards its durable log **verbatim**
   (it already carries `gen`/`turn`/`seq`). This is the common resume.
2. *agent → worker* (`--resume`): happens **only** when the server itself
   restarts and re-spawns the worker. There the **reloaded durable log is
   authoritative**; the worker **suppresses** its agent-replayed events up to the
   log's persisted tip and emits/append only genuinely-new (post-tip) events.
   This is today's `replay_fence` (server `main.rs:781-804`) generalized from a
   turn count to a `(generation, seq)` **watermark** handed to the worker at
   spawn. The worker's `--resume` stream is fenced, never re-ingested — so the
   reloaded log and the agent's replay can't fight (the unresolved tension in
   pass 1). The fence is suppression-at-source, not a content/identity dedup.

`ReplayEnd` flips `turn_phase` to `Idle` **only if no live turn is in flight** —
which, during a pure replay burst, it never is; with verbatim turns there is no
ambiguity.

---

## 6. Subscriber / forwarder contract (resolves H3, H4, H7)

Source of truth is the durable `event_log` keyed by `(session, generation)`;
broadcast is a wake signal; a subscriber re-tails `event_log[cursor..]` on each
wake. Guarantees are **stated, not conventional**:

- **Ordering:** total by `seq` within `(session, generation)` (guaranteed by §3).
- **Epoch & resume predicate (resolves H3):** `epoch := (generation, log_base)`
  where `log_base` is the lowest `seq` still present after compaction. On
  `Attach { acked_seq, client_generation }`, the server resumes incrementally
  **iff `client_generation == server_generation AND acked_seq >= log_base`**;
  otherwise it sends a from-0 rebuild. This one predicate funnels server-restart,
  compaction-past-cursor, and force-restart through a single comparison,
  evaluated server-side under the §3 lock (no TOCTOU — `log_base` can't advance
  during the decision).
- **Compaction watermark (resolves H4, H7, and the ownerless case N3):** the
  floor is `min(acked_seq)` over all **live** subscribers, computed **under the
  §3 lock** so an ownership handoff/`promote` can't change the floor authority
  mid-pass. An **owner, if present, is a hard ceiling** — never compact past
  `owner.acked_seq`, so the owner is never gapped. A subscriber past the
  high-water backlog threshold is **disconnected** (forced clean from-0
  reconnect) and thereby dropped from the `min`, so a wedged consumer can't pin
  unbounded growth; high-water disconnect fires *before* any gap-marker.
  - **No owner** (a `detach`, or the entire candidate→promote handoff window —
    the *normal* steady state during a promote): floor = `min` over the live
    observers; ownership handoff is atomic under the §3 lock.
  - **No live subscribers** (e.g. a server-restored session nobody has attached
    yet): the session produces no events (prompting requires an owner), so there
    is nothing to compact — floor = tip.
- **Multi-subscriber:** owner + N observers each hold their own cursor; all fan
  from the one `event_log`. (Real today via the candidate/promote co-attach.)
- **TranscriptTruncated** is surfaced as the `CompactedSummary` reducer arm (§7),
  not a silent gap.

---

## 7. Reducer contract (resolves H5, H7)

- **Total reducer:** the consumer matches `AgentEventKind` exhaustively — a new
  variant forces a compile error until handled. `Unknown` and `CompactedSummary`
  have explicit arms (Unknown → render nothing + diagnostic; CompactedSummary →
  a deterministic "history compacted" placeholder block).
- **Idempotent finalize (resolves H5):** `finalize_agent_turn` is a **no-op if
  already finalized for `(generation, turn)`** (tracked per session). This
  neutralizes a duplicate `TurnEnded` (e.g. forwarded event + a lingering
  inference during the additive rollout) — no double trailing line, no phase
  flip — and removes the need for a delicate cutover.
- **Idempotency, stated precisely (resolves H7):** reset + replay rebuilds
  **identical** state from the **uncompacted tail**; a compacted prefix rebuilds
  to a **deterministic summary placeholder** (`CompactedSummary`); for unknown
  variants, rebuild is identical **for a given decoder version** (an older
  decoder renders newer variants as `Unknown`).

---

## 8. Evolution — durable + forwarded across versions (resolves H8)

`#[serde(tag="kind")]` *errors* on an unknown tag, and `#[serde(other)]` is
unit-only (drops payload) — both unacceptable for a **durably-logged,
cross-version-forwarded** vocabulary (the candidate/promote flow co-attaches a
new and an old GUI to one server session). So:

- A concrete **`Unknown { tag, raw: serde_json::Value }`** catch-all **preserves
  the bytes**, via BOTH a custom `Deserialize` (lands unknown tags here) **and a
  matching custom `Serialize` that re-emits `raw` under its original `tag`** —
  NOT as `{"kind":"unknown",..}` (Nit-1). Deserialize-only would re-wrap on the
  way out and corrupt the forwarded log; the pair makes a node that can't *render*
  a newer variant still *round-trip* it verbatim.
- Variants are **additive only**; never renumber/repurpose. `serde(rename)`
  pins wire tags. Payload fields are added as `#[serde(default)]`.
- Rendering is best-effort across versions; the durable log is byte-faithful
  across versions. "Identical rebuild" is therefore explicitly *decoder-version-
  relative*, not absolute.

---

## 9. Additive rollout of the explicit `TurnEnded` (resolves H5)

Emit the forwarded `TurnEnded` alongside the existing inference, behind **one
feature gate over all three inference sites** (`has_forwarded_turn_ended_in_stream`):
the moment a real `TurnEnded` variant appears in a session's stream, every
inference site is disabled in the *same* commit — no long dual-path window. The
idempotent finalize (§7) is the backstop for any residual overlap. Assert/log
agreement between the forwarded event and the inference for a few real sessions
(a resume + a tool-only turn), then delete the inference and the gate.

---

## 10. Resolved holes

| # | Hole | Resolution |
|---|---|---|
| H1 | generation bumped after spawn → replay chunks carry wrong gen | §4: allocate before spawn, inject at construction, ride on `ChannelOpened` first event |
| H2 | `seq` raced across two send-side producers | §3: single `emit()` chokepoint assigns seq+enqueues under one lock |
| H3 | `epoch` undefined | §6: `epoch=(generation, log_base)` + one resume predicate, evaluated under the lock |
| H4 | two-subscriber compaction watermark ambiguous | §6: role-based — never gap the owner; disconnect slow observers before gapping |
| H5 | reconnect mid-turn double-finalize in additive phase | §7 idempotent finalize keyed on `(generation,turn)` + §9 single gate |
| H6 | worker can't own `k` for replayed turns | §5: persist `turn`/`seq` in the log, forward verbatim; `ReplayTurns` only for logless external resume |
| H7 | compaction lossy vs idempotency; no vocab arm | §7 + `CompactedSummary` variant; idempotency restated (uncompacted tail identical, prefix → placeholder) |
| H8 | serde unknown-tag errors / drops payload | §8 `Unknown{tag,raw}` byte-preserving catch-all; additive-only |
| H9 | racy backpressure read | §3: watermark read under the same lock as `record()` |

### Pass 2 — re-validation findings resolved

| # | Finding | Resolution |
|---|---|---|
| N1 | `emit()` lock across the async handler / server-lock lengthening | §3: critical section stated `await`-free; §11 server-lock note |
| N2 | `ChannelOpened` ordering + restore-path generation undefined | §4: emit before the load RPC; server allocates generation at all 3 spawn sites |
| N3 | compaction floor undefined when `owner == None` | §6: `min` over live subscribers, owner hard-ceiling, no-subscriber→tip, under lock |
| N4 | server-restart double-ingest vs the existing `replay_fence` | §5: reloaded log authoritative; worker `--resume` fenced to a `(gen,seq)` watermark |
| N5 + nits | finalize plumbing, custom `Serialize`, `Notification`-collapse migration | §8 + §12 |

---

## 11. Open / deferred

- **`emit()` lock contention** under very high chunk rates — worker-side (single
  session, low rate; likely fine) and, more importantly, **server-side**: `emit()`
  lengthens the `sessions`-lock critical section every pump and every
  `forward_notifications` tail already contend. Measure; a lock-free seq with a
  single-drainer reorder buffer is the fallback if it bites.
- This spec implements the **full D1 / event-stream refactor** — it is *not* a
  quick win; it lands behind CI in its own phased steps. The merged quick wins
  1–5 stand independently of it.

---

## 12. Required plumbing & migration (implementation-time)

The design is solid; these are the concrete blast-radius items it requires
(surfaced by the re-validation) — not open design questions:

- **Wire:** `Request::Attach` (`session_proto`) + `SessionServerClient::attach`
  grow `acked_seq` + `client_generation` (today every attach is an unconditional
  from-0 replay; the §6 predicate needs these).
- **Finalize:** `finalize_agent_turn(&mut Editor)` grows `(generation, turn)`;
  `AgentState` gains a `generation` field (it has none today); all **four**
  finalize/inference sites route through the idempotent guard (§7) and the §9
  single gate — server `ReplyEvent` (~`main.rs:11925`), server `TurnEnded`
  (~`:11938`), direct pump (~`:12169`), worker settle (`acp_channel.rs:1435`).
- **Spawn:** `spawn_with_resume_in` grows `generation`, wired at create
  (`main.rs:374`), restore (`:280`), restart (`:601`).
- **Migration (breaking):** collapsing `Notification::{ReplyEvent,TurnEnded,UserPrompt}`
  + `WorkerEvent::Reply` into `AgentEvent` changes the durable `session_server.json`
  shape. Gate it behind a schema `version` field with a one-time reader for the
  old shape (or explicitly discard old logs). **The durable WAL (ADR-0009) must
  carry a version tag from day one.**
