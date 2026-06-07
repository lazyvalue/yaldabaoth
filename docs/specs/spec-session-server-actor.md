# Spec: Session-server actor architecture

- **Status:** DRAFT — north-star. Frames the concurrency, ownership, lifecycle,
  and hardening model the session server should converge on; complements the
  already-specified event-stream and durable-log layers. Individually-verifiable
  migration phases in § Rollout.
- **Date:** 2026-06-07
- **Provenance:** Design review prompted by the reconnect-storm incident (root-
  caused + fixed, `81ae216`, branch `session-resilience` merged to `master`). The
  fix closed the immediate bug (client never `shutdown` its socket → server never
  released ownership); this spec addresses the architectural soil that grew it.
- **Theme:** *make the implicit explicit* — lifetimes, sequence numbers, leases,
  durability, and authorization should be first-class objects, not properties
  that emerge from thread scheduling, `Vec` indices, ephemeral connection ids,
  process-exit timing, and "local = trusted."

---

## Builds On

- **spec-event-stream.md** — defines the canonical `AgentEvent` vocabulary, the
  `(generation, turn, seq)` envelope, the single `emit()` chokepoint, and the
  subscriber/forwarder + compaction contract. *WHY:* it is the "what flows"
  layer; this spec is the "who owns and mutates it" layer beneath it. *HOW:* the
  actor here is the single component that holds the `emit()` chokepoint lock-free
  (it *is* the single writer), assigns `seq`, and appends to the log; the resume
  predicate `epoch=(generation, log_base)` is evaluated inside the actor.
- **ADR-0009 (durable session log, D4)** — decides the append-only WAL + periodic
  snapshot durability substrate. *WHY:* the actor's state must survive crashes,
  not just clean shutdown. *HOW:* the actor owns the WAL writer; every state
  mutation that must survive is an actor-applied, then-logged command.
- **spec-multi-session.md** — current session lifecycle, the session ring, the
  observer/promote ("blue-green") model, and the persisted-session format. *WHY:*
  this spec replaces the *mechanism* (shared map + `conn_id` ownership) while
  preserving the *behavior* (multi-attach, promote). *HOW:* observer/promote
  become lease operations; the ring is unchanged.
- **spec-state-architecture.md** — the D1–D6 state-first decomposition; D4 is the
  durable log. *WHY:* the GUI half of "make state derived, not hand-synced" is the
  same disease this spec cures on the server. *HOW:* unidirectional GUI data flow
  (§ Behaviors) is the client-side corollary of the server being the single
  source of truth.

---

## Overview

The session server today is **shared mutable state**: a `Mutex<HashMap<ServerSessionId, ManagedSession>>`
mutated from connection-handler tasks, one pump *OS thread* per session, and
per-session spawn threads, coordinated by ad-hoc tokens (`channel_generation`,
`replay_fence`, `owner: Option<conn_id>`) and guarded by poison-tolerant lock
access. That shape is the root soil of the race the storm fix patched: ownership
is *set* in one place, *cleared* in another (disconnect cleanup), and *read* in a
third (attach), and they race.

This spec proposes the server converge on six explicit entities:

1. **The Manager actor** (DRAFT) — a single async task that exclusively owns the
   session map. All mutations arrive as messages on an `mpsc` command channel and
   are applied by this one task. There is no shared lock, therefore no lock to
   poison and no cross-task mutation race. (Mechanical replacement for the
   `Mutex<HashMap>`.)
2. **The Session record** (DRAFT) — owned solely by the actor; never `Arc`-shared
   for mutation. Holds the live `AgentTransport`, the durable log handle, the seq
   tip, and the current lease.
3. **The Lease** (DRAFT) — an explicit, time-bounded grant of *drive rights*
   (prompt/cancel/close) over a session to one client identity, replacing
   `owner: Option<conn_id>`. Has a stable `client_id`, an expiry, and is renewed
   by heartbeat. Reconnect *resumes* a lease; it never races to reclaim it.
4. **The durable log substrate** (ACTIVE per ADR-0009) — the append-only WAL +
   snapshots that make the actor's state crash-recoverable. The actor is its only
   writer.
5. **The `AgentTransport` seam** (DRAFT) — a trait abstracting "talk to one
   agent," with a real ACP-subprocess impl and an in-process fake. Makes the
   server drivable in tests without spawning processes.
6. **The admin/observability surface** (DRAFT) — structured `tracing` spans per
   connection/session/lease plus an admin query verb that dumps live state
   (sessions, leases, connection count, per-subscriber seq cursors).

**What the current design already gets right (SHIPPED — keep):** event-log as
source of truth with broadcast-as-*wake* (the self-healing forwarder that re-tails
`event_log[cursor..]` and recovers on `Lagged`); explicit turn boundaries with a
`generation` token; the detached daemon that survives GUI exit; the
observer/promote blue-green co-attach; and (as of `81ae216`) logging disconnect
*reasons* and a single-instance guard. This spec does not undo any of these — it
removes the shared-mutable substrate underneath them.

---

## Behaviors

### Single-writer serialization (DRAFT)
- All session-state mutations (`create`, `attach`, `lease`/`renew`/`release`,
  `prompt`, `cancel`, `close`, `set_permission`, `record(event)`, channel
  (re)spawn publish) are **commands sent to the Manager actor** and applied by it
  alone, in arrival order. Reads that must be consistent are commands that carry a
  oneshot reply channel; cheap eventually-consistent reads may use a snapshot
  published by the actor.
- Consequence: every "set X here, clear X there, read X over there" invariant
  (the ownership race; the `seq`/enqueue/log/watermark coupling from
  spec-event-stream §3) is serialized by construction. No poison-tolerant lock,
  no `.lock().unwrap()` cascade (ADR-0009's named crash vector) — there is no
  shared lock.

### Lease-based ownership & deterministic reconnect (DRAFT)
- A client presents a **stable `client_id`** (not a per-connection id) at
  handshake. Drive rights are a **lease**: `Leased { client_id, expires_at }`.
- `attach` as driver = *acquire or resume* the lease. If the lease is unheld or
  already held by this `client_id`, it is granted immediately. If held by a
  *different, still-live* `client_id`, the caller attaches as **observer** (the
  blue-green path) — no retry loop, no fallback dance.
- A lease is kept alive by **heartbeat**; a missed-heartbeat lease **expires** and
  becomes acquirable. Expiry is driven two ways so it is deterministic in a
  message-driven actor: (a) **lazily** — any command that inspects a lease
  evaluates `expires_at` against `now` at apply time, so an `attach` never acts on
  a stale grant; and (b) a **periodic sweep tick** in the actor's `select` loop
  releases expired leases even with no inbound traffic (and emits `LeaseChanged`
  so idle observers can promote). Without (a), an `attach` could wrongly see a
  not-yet-swept expired lease; without (b), an expired lease on an idle session
  would never free.
- **Same-client reconnect never contends with itself.** The acquire check is
  "held by a *different* `client_id` whose lease is not expired" → caller becomes
  Observer. A returning **same**-`client_id` always resumes its own lease
  regardless of expiry, so the `ConnectionGone`→re-attach window cannot make a
  client lose its own session — closing the race that `attach_with_owner_retry`
  (`session_client.rs:460`) papers over today with a ~1s retry.
- This retires the `attach_with_owner_retry` / observer-fallback choreography and
  the teardown-vs-reattach race the storm fix mitigates with a retry.

### Incremental, cursor-based reconnect (DRAFT)
- A reconnecting client sends its last-acked `(generation, seq)`. The actor
  applies the spec-event-stream §6 resume predicate and streams **only events
  after the cursor** when the epoch matches; a full from-0 rebuild happens only
  on epoch mismatch (compaction-past-cursor or generation change).
- Retires today's unconditional "`reset_for_replay` + replay entire log from
  index 0" on every reconnect — the behavior that makes a large transcript
  re-stream wholesale and visibly reset the GUI.

### Explicit resource lifetime (SHIPPED partial → DRAFT target)
- SHIPPED (`81ae216`): `SessionServerClient::Drop` shuts the socket down, so the
  server observes disconnect promptly instead of at process exit.
- DRAFT target: connection teardown is owned by an async end-to-end path (no
  detached sync reader thread blocked on `lines()` bridging to a sync GUI). The
  sync↔async bridge in `session_client.rs` and `acp_channel.rs` is where these
  "cleanup deferred to process exit" traps breed; eliminating it removes the class.

### Durability & recovery (SHIPPED — `session_wal.rs`; snapshot/compaction deferred)
- ✅ The session state is reconstructable from the durable WAL after any crash
  cause, not just clean shutdown. Guarantee (per ADR-0009): never lose a
  completed turn or a sent prompt; worst case is an in-flight stream tail
  truncating on power loss. The WAL carries a schema `version` from day one.
  Shipped as a per-session append-only NDJSON log (not yet snapshot+tail —
  compaction is deferred per ADR-0009); recovery replays the full log and
  tolerates a torn final line. Verified by `session_recovered_after_server_crash`
  (SIGKILL → restart → full transcript recovers).
- **fsync is in-actor at turn boundaries, not delegated (resolves the durability-
  vs-await-free tension).** Per-event `Record` does a buffered `write()` only — a
  *process* crash loses nothing (the OS flushes), so this stays await-free. The
  stronger `fsync` (power-loss durability) happens **inside the actor** on the
  low-frequency turn-boundary records (`UserPrompt`, `TurnEnded`) — never
  per-token — and the client ack for a prompt is returned **after** that fsync, so
  "never lose a sent prompt" holds at ack time. The cost: the inlet serializes on
  disk latency at turn boundaries only. This is the contention spec-event-stream
  §11 leaves open; it is carried forward here as a measured risk, not resolved
  away. (Truly long/blocking work — agent spawn — is still delegated to a child
  task that reports back via a `Command`.)
- **Agent re-adoption across server restart:** on recovery the actor re-spawns
  each agent via `session/load(acp_session_id)` and fences the agent's `--resume`
  replay to the persisted `(generation, seq)` watermark (spec-event-stream §5), so
  the reloaded log is authoritative and the two replays can't fight.

### Single-instance by construction (SHIPPED guard → DRAFT target)
- SHIPPED (`81ae216`): a TOCTOU-windowed guard — a second server probes the
  socket and exits if a live one answers.
- DRAFT target: OS-level exclusivity — **launchd socket activation** on macOS
  (the kernel owns the socket; exactly one server by construction) or a `flock`ed
  lockfile — so single-instance is a property, not a check with a race window.

### Authorization & safe defaults (DRAFT — hardening)
- **The primary control is the permission mode, not socket auth.** Today any
  process running as the user can connect and drive an agent whose **default
  permission mode is `Yolo`** (auto-approve tool calls — file writes, shell;
  `acp_channel.rs:542`). The load-bearing fix is to **default to a safe
  permission mode**, escalating to `Yolo` only on explicit user action, and to
  **assert the socket is mode `0600`**.
- **A capability token is NOT pursued for the single-user model.** Against the
  only in-scope adversary (a same-uid process), a token on a `0600` socket is
  theater — that process can already read it from the handshake. A token is
  justified *only if* a genuinely lower-trust local component is later given
  socket access (e.g. a sandboxed helper or the agent subprocess itself reaching
  back in); if that boundary ever becomes real, scope tokens to capabilities
  then. Until then, default-safe permission mode + `0600` is the whole story.

### Bounded resources & backpressure (DRAFT — hardening)
- Every queue is bounded with an explicit overflow policy. The event log is
  compacted/snapshotted (spec-event-stream §6 watermark; deferred per ADR-0009
  until it measurably hurts). A **slow subscriber past the high-water backlog is
  disconnected** (forced clean reconnect) rather than allowed to pin unbounded
  growth — the owner/lease-holder is a hard ceiling and is never gapped.

### Protocol versioning & capability negotiation (DRAFT)
- `initialize` negotiates a protocol version and capability set, so the GUI and
  server evolve independently. This is load-bearing for the self-hosting
  candidate/promote flow, which co-attaches a *new* and an *old* GUI to one
  server session. (The `#[serde(default)]` already added to a wire field is the
  canary that ad-hoc compat has begun.)

### Testability as a first-class contract (SHIPPED foundation → DRAFT seam)
- SHIPPED: `tests/session_resilience_test.rs` drives the real server binary
  headlessly on a private socket; invariant *accepts == closes* (no zombie
  connections).
- DRAFT: the `AgentTransport` trait lets most tests run with an in-process fake
  agent (no subprocess). The `SKETCH_ACP_AGENT` env injection is the current,
  cruder seam.

### Unidirectional GUI data flow (DRAFT — client corollary)
- The GUI is a **projection** of server events: `server events → reducer → view
  state`, with only ephemeral UI state held locally. Retires the
  `reset_for_replay` + manual re-attach + status-reconciliation choreography,
  which is the same hand-synced-cache disease spec-state-architecture targets.

---

## Data Model

Owned exclusively by the Manager actor (illustrative, not prescriptive):

```rust
// The actor task owns this directly — never behind a shared Mutex, never Arc'd
// for mutation.
struct Manager {
    sessions: HashMap<ServerSessionId, Session>,
    cmd_rx: mpsc::Receiver<Command>,        // the single mutation inlet
    snapshot_tx: watch::Sender<DirSummary>, // cheap eventually-consistent reads
    wal: WalWriter,                         // the only writer of the durable log
}

struct Session {
    id: ServerSessionId,
    acp_session_id: Option<String>,     // for agent re-adoption on restart
    transport: TransportHandle,         // Send handle to the agent — see "Transport bridge" below
    generation: u64,                    // channel-respawn token; actor-allocated, persisted
    seq_tip: u64,                       // monotonic per (session, generation)
    log: SessionLog,                    // durable handle; subscribers tail [cursor..]
    lease: Lease,
    subscribers: Vec<Subscriber>,       // owner/observers, each with its own cursor (N ≤ ~10)
    permission_mode: PermissionMode,    // default SAFE, not Yolo
}

enum Lease {
    Unowned,
    Leased { client_id: ClientId, expires_at: Instant },
}

// Commands carry a oneshot for replies that need consistency.
enum Command {
    Create { cwd, label, resume: Option<String>, reply: oneshot<SessionInfo> },
    Attach { sid, client_id, cursor: Option<(u64,u64)>, want_drive: bool,
             reply: oneshot<AttachOutcome> },     // AttachOutcome = Driver | Observer
    Heartbeat { sid, client_id },
    Prompt { sid, client_id, text, reply: oneshot<Result<()>> },
    Cancel { sid, client_id, reply: oneshot<Result<()>> },
    Close  { sid, client_id, reply: oneshot<Result<()>> },
    SetPermission { sid, client_id, mode, reply: oneshot<Result<()>> },
    Record { sid, event: AgentEvent },            // from transports; appended + broadcast
    PublishChannel { sid, transport, generation },// (re)spawn completed
    ConnectionGone { client_id },                 // starts lease-expiry clock
    AdminQuery { reply: oneshot<AdminSnapshot> },
}
```

**Transport bridge (resolves the sync↔async seam).** The actor task is async,
but the real `AcpChannelClient` owns a `std::sync::mpsc::Receiver` that is **not
`Sync`** — which is exactly why pumps run on dedicated OS threads today
(`main.rs:722`). The actor therefore never holds the agent's receiver. The
`AgentTransport` seam is defined async at its boundary: each transport owns its
own reader (the real impl keeps its OS thread draining the non-`Sync` receiver;
the fake yields a stream) and **forwards every agent event as a `Command::Record`
into the actor's `mpsc` inlet** via a cloned `Sender` (which *is* `Send`). The
actor holds only a `TransportHandle` — a `Send` outbound handle exposing
`prompt`/`cancel` and the agent's `acp_session_id`. So "the actor owns the
transport" means it owns the *handle and lifecycle*, not the blocking receiver;
the bridge thread/task is the one place the non-`Sync` boundary lives, and it has
exactly one job (receiver → `Record`). This is the actor-model generalization of
today's per-session pump thread.

**Generation is actor-allocated and persisted (resolves the re-adoption-fence
authority).** `generation` is allocated monotonically by the actor on every
(re)spawn and written to the WAL/snapshot. On recovery the actor **loads the
persisted generation** rather than resetting to `0` (today's restore path resets
to `0`, per spec-event-stream §4 — this spec requires that be corrected, since
re-adoption fencing keys off the persisted `(generation, seq)` watermark). `u64`
monotonic; never reused, never reset on restore.

On-disk shape (WAL + snapshot) is owned by ADR-0009 / spec-event-stream §12; this
spec only requires that the WAL carry a `version` tag and that recovery be
`snapshot + tail`.

---

## Interfaces

**API surface (external — GUI ↔ server over the socket):** the existing
`session_proto` request/response set, extended so `initialize` negotiates
`{protocol_version, capabilities}`; `attach` carries
`{client_id, cursor:(generation,seq), want_drive}` and returns
`Driver | Observer`; a `heartbeat` keeps a lease live; an `admin_status` verb
returns the `AdminSnapshot`. The wire envelope carries a schema `version`.

**Events / messages (internal):** the `Command` enum above is the *only* inlet to
session-state mutation. Outbound to clients: `AgentEvent`-bearing notifications
(spec-event-stream) plus control notifications (`SessionAttached/Detached`,
`LeaseChanged` — replacing `OwnerChanged`, `SessionCreated/Closed/Renamed`).
`AgentTransport` is the inbound seam from agents: it yields `AgentEvent`s and
accepts `prompt`/`cancel`.

**Migration coupling:** the `OwnerChanged → LeaseChanged` rename is a breaking
wire+WAL change. It MUST ride the **same** one-time schema-`version` bump and
old-shape reader that spec-event-stream §12 already defines for collapsing
`Notification::{ReplyEvent,TurnEnded,UserPrompt}` into `AgentEvent` — not a second
independent breaking change. "Version from day one" (§Constraints) means both
land under one migration.

**Data ownership:** the Manager actor owns the session map and the WAL writer
exclusively — no other task reads or writes them except via `Command`. Each
`Subscriber` owns its own log cursor. The durable log is owned by D4 (ADR-0009);
the actor is its sole writer. Clients own only ephemeral UI/projection state.

---

## State Machine — Lease (DRAFT)

```
            attach(want_drive), lease Unowned or same client_id
 Unowned ───────────────────────────────────────────────► Leased(client_id)
    ▲                                                          │  │
    │  expiry (missed heartbeat)  OR  explicit release         │  │ heartbeat
    └──────────────────────────────────────────────────────────┘  │ (renew expires_at)
                                                                ◄───┘
 attach(want_drive) by a *different* live client_id while Leased
        → caller becomes Observer (no state change to the lease)
```

- Connection drop → `ConnectionGone` → lease keeps its grant but the expiry clock
  runs; a returning same-`client_id` attach resumes with zero contention. A
  different client may acquire only after expiry or explicit release.
- This is the deterministic replacement for `owner: Option<conn_id>` + the attach
  retry/observer-fallback.

---

## Constraints

- **No shared mutable session state.** If a code path needs to mutate a session
  outside the actor, that is a design violation — add a `Command`.
- **The actor's per-message work is `await`-free** where it holds the coupled
  `(seq, enqueue, log-append, watermark)` invariant (spec-event-stream §3),
  including the buffered WAL `write()`. The two exceptions are explicit: agent
  spawn (truly blocking) is delegated to a child task that reports back via a
  `Command`; turn-boundary `fsync` runs in-actor before ack (§Behaviors,
  Durability) and is the one accepted inlet-serialization point.
- **Behavior parity:** multi-attach, observer/promote, and resume-by-identity
  must be preserved; this spec changes mechanism, not user-visible capability.
- **Durability guarantee** is exactly ADR-0009's (never lose a completed turn or
  sent prompt; no fsync-per-token).
- **Compat:** the wire and WAL both version from day one; no renumber/repurpose of
  existing variants (additive only, per spec-event-stream §8).
- **Non-goals:** networked/multi-host operation, multi-user sharing, and a
  general plugin agent registry are out of scope; the trust model is single local
  user with capability-token defense-in-depth.

---

## Rollout — phased, individually-verifiable

Each phase is independently shippable behind the headless harness and CI; none
requires the GPUI app to verify the server-side change.

1. **Lifetime hygiene (SHIPPED, `81ae216`):** socket `shutdown` on drop;
   single-instance guard; socket-scoped pid/state paths; the resilience harness.
2. **Durable WAL (D4):** ✅ SHIPPED (`session_wal.rs`). Per-session append-only
   NDJSON WAL: write-immediately (process-crash safe) + fsync at turn boundaries
   (power-loss safe), versioned header, recovery by full-log replay (torn final
   line tolerated), agent re-adoption via `session/load`; replaces the
   clean-shutdown-only JSON snapshot. Verified: `session_recovered_after_server_crash`
   SIGKILLs the server and asserts the completed turn's transcript recovers.
   **snapshot+tail compaction deferred** (ADR-0009) until it measurably matters.
3. **Actor extraction:** move the `Mutex<HashMap>` behind a single Manager task +
   `Command` inlet, mechanically, preserving today's `conn_id` ownership.
   **Hand-rolled `tokio::mpsc` + `oneshot`, no actor framework** (ADR-0012).
   Verify: existing resilience + transcript-replay harness unchanged; delete
   poison-tolerant lock access and the `.lock().unwrap()` vector.
4. **Lease ownership:** replace `owner: conn_id` with `Lease{client_id,expiry}` +
   heartbeat; retire `attach_with_owner_retry`/observer-fallback. Verify: a
   headless "client_id reconnect" reclaims its lease with zero retries; a second
   client_id becomes observer.
5. **Cursor reconnect:** carry `(generation, seq)` on attach; incremental resume.
   Verify: large-transcript reconnect re-streams only the tail, not from 0.
6. **`AgentTransport` seam:** extract the trait + in-process fake; migrate most
   harness tests off subprocess spawning.
7. **Hardening:** safe-default permission mode + `0600` socket assertion (the
   real control; capability token only if a lower-trust local component later
   needs socket access); bounded queues + slow-subscriber disconnect (per-
   backlog, not subscriber-count); OS-level single-instance (launchd/flock);
   structured `tracing` + `admin_status`.
8. **GUI projection (client corollary):** unidirectional `events → reducer → view`;
   retire `reset_for_replay` choreography. Verify: human runtime check (GPUI not
   headless-drivable) — the one phase that needs it.

---

## Revision History

- **2026-06-07** — DRAFT created. North-star for the session-server concurrency,
  ownership, lifecycle, and hardening model, prompted by the reconnect-storm
  incident (`81ae216`). Scoped to complement spec-event-stream.md (event
  vocabulary/stream) and ADR-0009 (durable log); contributes the actor/single-
  writer model, lease-based ownership, the `AgentTransport` seam, and the
  hardening + phased-rollout plan.
- **2026-06-07** — Folded in adversarial-review findings (verdict REVISE):
  named the sync↔async **transport bridge** (actor holds a `Send` handle; a
  per-session reader forwards `Record`), made **turn-boundary fsync in-actor
  before ack** explicit (vs the await-free inlet), gave **lease expiry** a
  lazy-check + sweep-tick clock and a same-`client_id` resume guarantee,
  required **generation** to be actor-allocated and persisted (not reset on
  restore), **dropped the capability token** as theater for the single-user
  threat model (safe-default permission mode is the real control), and
  cross-linked the `OwnerChanged→LeaseChanged` rename to spec-event-stream §12's
  one-time migration.
