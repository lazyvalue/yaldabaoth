# Spec: Session branching (conversation trees)

- **Status:** DRAFT — hardened after one adversarial-review pass (REVISE →
  resolved); the seeding/identity core was reworked. Awaiting human approval.
- **Date:** 2026-06-23
- **Provenance:** Design conversation → approach (A) "tree-over-sessions" chosen
  over (B) "in-log forest" because (A) is additive to the post-refactor
  architecture (strict 1:1 binding + one linear `(session, generation, seq)` log
  per session) while (B) would re-litigate the event-stream and ownership
  contracts. (A) also uniquely enables concurrent/side-by-side branch
  exploration, which (B) structurally cannot under 1:1.

> A conversation can **branch**: from any turn you can start a divergent path,
> producing a tree of paths instead of a single linear chat. This spec models a
> branch as a **forked session** — a new `AgentSession` whose history is the
> parent's prefix **re-stamped under the child's own identity** — and models the
> tree as a **parent-pointer relationship over sessions**, not a new in-log data
> structure. Every event-stream and session-ownership invariant is preserved;
> branching is additive at the ownership/store/UI layer. The one component with a
> genuine backend dependency is **seeding the child agent's context** to the
> shared prefix.

---

## Builds On

- **spec-agent-session-ownership.md** — defines `SessionId`, the `AgentSession`
  record, the `AgentSessions` store, and the strict 1:1 tile↔session invariants
  (INV-1..4). *WHY:* a branch is just another `AgentSession`; the tree is
  metadata over the store. *HOW:* the fork operation creates a child via the
  store's existing `create_local`/`bind_sid`/`show_session` path; each branch is
  bound by at most one tile exactly like any session — no invariant changes.
- **spec-event-stream.md** — defines the canonical `AgentEvent` envelope
  (`session_id`, `generation`, `turn`, `seq`), the durable log as source of
  truth, and turn-numbering for live vs replayed history. *WHY:* `session_id`
  lives **inside** the envelope and is the total-routing key, and `seq` is
  monotonic per `(session_id, generation)` — so a fork must **re-stamp** the
  prefix under the child's identity, not copy it verbatim (§2). *HOW:* the
  child's seeded prefix is the parent's events with `session_id` set to the
  child, `seq` renumbered monotonically within the child, and `turn` preserved;
  it is the child's replayed history (`TurnEnded { ReplayEnd }` marks its end),
  the child's first agent channel emits its own `ChannelOpened` (§4), and the
  live counter resumes at `fork_turn`. No new `AgentEventKind` variant — fork is
  session-creation, not an agent fact.
- **spec-session-server-actor.md** — the Manager actor owns the session map +
  WAL; all mutation arrives as `Command`s. *WHY:* fork is a new server-side
  mutation. *HOW:* `Command::Fork` is applied by the actor: allocate a child
  session with its **own** server session id and a **fresh** `acp_session_id`,
  write the re-stamped prefix to the child's WAL, spawn the child's agent, persist
  the fork edge.
- **spec-jump-panel.md** — the root-level navigator listing workspaces + agent
  sessions. *WHY:* it is where the tree becomes visible and navigable. *HOW:* the
  "Agent sessions" section groups sessions by fork-family (children collapsed
  under their root) so a family reads as one expandable unit, not N flat rows.
- **spec-agent-cwd.md** — a session's working directory is a required typed
  field. *WHY:* a child must have a cwd. *HOW:* a fork inherits the parent's cwd
  verbatim.

---

## Overview

Named entities introduced here, referenced by later sections:

- **Conversation tree** — a set of `AgentSession`s connected by **fork edges**.
  Not a stored object; it is *derived* by walking parent pointers.
- **Fork edge** — `ForkPoint { parent_session_id, fork_turn }`, one optional
  field on a child `AgentSession`. The root of a tree has `fork_parent = None`.
- **Fork point / `fork_turn`** — the turn at which a child diverges. The child
  **shares the parent's turns strictly before `fork_turn`** (the immutable
  *shared prefix*); `fork_turn` is the child's first *new, divergent* turn.
- **`Command::Fork`** — the server-side mutation that materializes a child from a
  parent at a fork point.
- **Prefix re-stamping** — rewriting the parent's prefix events under the child's
  `session_id` / `seq` before they become the child's log (required by the
  event-stream identity model, §2).
- **Prefix seeding** — making the child *agent process*'s context equal to the
  shared prefix. The one component with a genuine backend dependency (the
  **seeding ladder**, below).

The product model is "a family of related chats": each branch is a first-class
session — independently bound to a tile, independently runnable (two branches can
generate at once), independently closable — grouped visually under its root.

---

## Behaviors

### Forking (DRAFT)
- **`fork(parent_sid, fork_turn) → child_sid`** creates a new session whose
  durable log is the parent's events for turns `[0, fork_turn)`, **re-stamped
  under the child's identity** (NOT copied verbatim — see below). The prefix is
  **immutable** in the child: the child never mutates shared history, it only
  appends its divergent tail from `fork_turn` onward.
- **Re-stamping (resolves the event-stream identity collision).** Each prefix
  event's `session_id` is set to the child's, `seq` is renumbered monotonically
  within the child (`seq` is defined monotonic per `(session_id, generation)`;
  spec-event-stream §2), and `turn` is **preserved** so the transcript reads
  continuously. The prefix is stamped as the child's *seeded* generation; the
  child's first agent channel then spawns at the next generation and emits its
  own `ChannelOpened` (spec-event-stream §4), which the GUI rebaselines on.
  Verbatim copy would leave the parent's `session_id` inside the envelope —
  mis-routing the child's stream to the parent and colliding the per-session
  `seq` cursor that the §6 resume predicate depends on.
- **The child gets its own identity, not the parent's resume id.** The child is
  allocated a fresh server session id and a **cleared** `acp_session_id` (it has
  no agent-side conversation until its first channel spawn). A re-stamped prefix
  carries **no** `SessionAttached` bearing the parent's `acp_session_id`, so WAL
  recovery (`session_wal.rs` re-derives `--resume` from the last `SessionAttached`)
  cannot make the child load the parent's agent session.
- The **parent is untouched** — it keeps its own tail past `fork_turn` and keeps
  running. Parent and child are independent sessions from the instant of forking.
- **Edit-and-rerun is fork-with-a-replaced-prompt.** "Go back to user turn N,
  change it, and re-run" = `fork(parent, N)` where the child's first submitted
  turn is the edited prompt. The headline UX is the same primitive, not a
  separate mechanism.
- A fork **inherits** the parent's `cwd` and derives a `label` marking lineage
  (e.g. `claude-1 ⑂2`); both are ordinary session fields.

### Fork legality — completed turn boundary only (DRAFT)
- A fork is legal **only at a completed turn boundary**: the seeded prefix must
  end on a `TurnEnded` (spec-event-stream §5), never inside a streaming run of
  `Chunk`s. Otherwise the child would carry a dangling partial turn before its
  `TurnEnded { ReplayEnd }`.
- Forking a parent with a **live turn in flight** at the boundary either **waits
  for that turn's `TurnEnded`** or is **refused** — never copies a half-streamed
  turn. This guard applies to the **tip-only rung too**: a session's tip may
  itself be mid-stream, so phase 1 needs it.

### Prefix seeding — the seeding ladder (DRAFT)
The server re-stamps log events cheaply, but the **agent process** must continue
as if the shared prefix were its conversation history. The rung in force is a
stated capability, never a silent degrade:
1. **Tip-only fork (guaranteed floor).** Restrict `fork_turn` to the parent's
   current tip — "duplicate this conversation and continue" — which is ordinary
   `session/load(parent_acp_id)` against the parent's existing agent history and
   needs **zero** backend work. Phase 1 ships on this rung. *(The child still gets
   its own server identity per Forking; it shares only the agent-side resume at
   spawn, then diverges.)*
2. **Prefix replay (the real arbitrary-turn mechanism).** Spawn a fresh agent
   session and feed the prefix turns back as synthetic prior context up to
   `fork_turn`. This is the path for forking *before* the tip, because
   `session/load` in this codebase replays the **agent's own** persisted
   conversation keyed by acp session id (`acp_channel.rs` — load takes a
   `SessionId`, not a transcript) and offers no truncation parameter. Whether the
   agent accepts injected synthetic history is **runtime-unverified against
   `claude-agent-acp`** and gates phase 2 "done."
3. **Truncated resume (only if the protocol gains it).** If a future ACP
   capability lets `session/load` accept a transcript truncated at a turn
   boundary, it supersedes rung 2. **Not assumed to exist** — the load protocol
   as implemented does not support it, so no phase is planned around it.

### Navigating the tree (DRAFT)
- The **jump panel** groups the "Agent sessions" section by fork-family: a root
  and its descendants render as one collapsible unit; a row shows lineage (depth
  / fork-turn) and the existing bound-vs-free indicator. The roster has **no**
  fork concept today (`agent_sessions.rs` / `AgentRoster`), so this is **net-new
  projection logic** (group-by-`fork_root`) plus a collapsible-unit render — not
  reuse of an existing grouping. Selecting any branch row uses the panel's
  existing bound→focus / free→ephemeral-workspace semantics (spec-jump-panel.md)
  unchanged.
- **Two branches open at once** is the natural side-by-side case: because each
  branch is a distinct session, two tiles may bind two siblings (INV-2 holds —
  one session, one tile each). No special "compare" mode is required.

### Orphaned children — missing parent is inert, never fatal (DRAFT)
- A child's `fork_parent.parent` may name a session that no longer exists (the
  parent was **killed** — `store.close` + server close, distinct from closing its
  tile per spec-agent-session-ownership.md). The edge then becomes **inert**: the
  child is treated as a **detached root** for tree purposes — it keeps its own
  full history (the prefix was re-stamped into its log, so nothing is lost) and
  renders ungrouped.
- `fork_root` / `children` walks **must tolerate a dangling `parent`** (stop at a
  missing id; never panic or loop). The jump-panel family grouping renders an
  orphan as its own root.

### Persistence & restore (DRAFT)
- The fork edge is durable in **both** places it must survive: the child's
  server-side **WAL** (so the server reconstructs the tree on restart) and the
  GUI's `PersistedSlot` (so the GUI regroups the family on reopen).
- **The edge is keyed by stable session identity, not the resume string.** It
  stores the parent's **server session id** (and the GUI's local `SessionId` on
  restore), never `parent_resume_id` — a `resume_id` is only the id a session
  *tried* to load and can diverge from the live session on `session/load →
  session/new` fallback (spec-multi-session §15), so it is not a reliable join key.
- On restore, the tree reassembles by reading each session's `fork_parent`; a
  child whose parent slot is **absent** restores as a **detached root** (per
  Orphaned children). No separate tree document exists to drift.

### Turn numbering (SHIPPED mechanism, reused)
- The child's shared prefix **preserves** the parent's `turn` numbers
  `[0, fork_turn)` (re-stamping rewrites `session_id`/`seq`, not `turn`;
  spec-event-stream §5). `TurnEnded { ReplayEnd }` marks the end of the seeded
  prefix; the child's live counter resumes at `fork_turn`. The
  `UserTurnReconciler` and `ReplayTurns` are **unchanged** — each session still
  owns one linear turn line; the child's line merely *starts* from the seeded
  prefix.

---

## Data Model

Client-side additions (spec-agent-session-ownership.md model):

```rust
/// A child's divergence from its parent. None ⇒ this session is a tree root.
struct ForkPoint {
    parent: SessionId,   // the session this branched from
    fork_turn: u64,      // first divergent turn; shared prefix is [0, fork_turn)
}

struct AgentSession {
    // ... existing: state, label, cwd, resume_id, server_session_id ...
    fork_parent: Option<ForkPoint>,   // NEW — the only new field
}
```

The tree is **derived**, not stored:

```rust
impl AgentSessions {
    /// Sessions whose fork_parent.parent == id (cheap scan; sessions are few).
    fn children(&self, id: SessionId) -> impl Iterator<Item = SessionId>;
    /// Walk fork_parent to the root (fork_parent == None).
    fn fork_root(&self, id: SessionId) -> SessionId;
}
```

No `by_parent` index unless the scan proves hot (it won't at session counts of
order ~tens). There is one source of truth for existence (the store) and one for
the edge (`fork_parent`); nothing to keep in sync.

Server-side (spec-session-server-actor.md `Session`): the child `Session` gets its
own server session id and a cleared `acp_session_id`, and records
`{ parent_server_sid, fork_turn }` in its WAL header so the edge survives restart.
The shared prefix lives as **re-stamped log events** (child `session_id`,
renumbered `seq`, preserved `turn`) in the child's own log — storage is
**duplicated** per branch in phases 1–3; structural sharing is deferred (phase 4,
Constraints).

Persistence (`PersistedSlot`, spec-multi-session.md format): add
`fork_parent: Option<{ parent_server_sid, fork_turn }>`, keyed by the parent's
**server session id** (the stable identity), not its resume string — the latter
can diverge from the live session on resume fallback. On restore the GUI joins the
edge back to the parent's local `SessionId`; an absent parent → detached root.

---

## Interfaces

**Server command (module-internal, actor inlet):**
```rust
Command::Fork {
    parent: ServerSessionId,
    fork_turn: u64,                       // clamped to parent tip on the tip-only rung
    reply: oneshot<Result<SessionInfo>>,  // the child's SessionInfo (new sid)
}
```
Applied by the Manager actor, in order: (1) verify `fork_turn` lands on a
completed `TurnEnded` boundary (wait or refuse otherwise — Fork legality);
(2) allocate a child session with a **fresh** server sid + **cleared**
`acp_session_id`; (3) **re-stamp** parent log `[0, fork_turn)` under the child's
`session_id` with renumbered `seq` (preserved `turn`) and write it to the child's
WAL with the `{ parent_server_sid, fork_turn }` edge; (4) spawn the child agent
via the seeding ladder — the child's first channel emits its **own**
`ChannelOpened` (spec-event-stream §4) before any live event; (5) reply with the
child `SessionInfo`. Emits the existing `SessionCreated` control notification —
**no new `AgentEvent` variant**.

**GUI entry point (the single fork choke, on the view):**
```rust
/// Fork `parent` at `at_turn`, bind the child via show_session, return it.
fn fork_session(&mut self, parent: SessionId, at_turn: u64,
                want_new_tile: bool, cx) -> SessionId
```
Both affordances route through it: the transcript "branch from here" action, and
the edit-a-past-user-turn-and-resubmit path (which calls it with `at_turn` = the
edited turn, then submits the edited prompt to the child).

**Store API (module-internal):** `children`, `fork_root` (above); `fork_parent`
is read through `get`/`get_mut`. No new public mutator beyond setting
`fork_parent` at child creation.

---

## Constraints

- **No invariant changes.** INV-1..4 (one session per sid; ≤1 tile per session;
  one channel per session; total/unique routing) and the linear
  `(session, generation, seq)` log per session hold unchanged. A branch is a
  session; the tree is metadata. If a change here requires touching those, the
  design is wrong.
- **Shared prefix is immutable.** A child never edits history before `fork_turn`;
  it only appends. Editing shared history is not a supported operation — to
  "change the past" you fork.
- **Storage duplication is accepted for phases 1–3.** Each branch holds its own
  copy of the shared prefix. **Structural prefix-sharing** (immutable prefix
  nodes referenced by multiple branches) is the **phase-4 deferred** optimization
  and must not be designed-in early at the cost of the linear-log model.
- **Prefix is re-stamped, never copied verbatim.** Prefix events must carry the
  child's `session_id` and a `seq` renumbered within the child before entering the
  child's log; verbatim copy mis-routes the stream and collides the `seq` cursor
  (spec-event-stream §2/§6). The child also gets a fresh server sid and a cleared
  `acp_session_id` so WAL recovery cannot resume it onto the parent's agent.
- **Fork only at a completed turn boundary** (Fork legality) — including the
  tip-only rung.
- **Seeding rung must be explicit.** The active rung of the seeding ladder is a
  surfaced capability; a fork that silently degrades from arbitrary-turn to
  tip-only is a violation — degrade loudly or refuse.
- **Backend dependency is runtime-checked.** Rung 2 (prefix replay as synthetic
  history) is unverified against `claude-agent-acp` — `session/load` resumes the
  agent's own conversation by id and takes no transcript, so arbitrary-turn
  seeding rests on the agent accepting injected history. Phase 2 is not "done"
  until a human runtime check confirms the seeded child continues coherently (the
  GPUI app is not headless-drivable; dev-system definition of done).

---

## Rollout — phased, each independently shippable

1. **Tip-only fork (rung 1).** `Command::Fork` clamped to the parent tip, with the
   turn-boundary guard; the full identity story lands here — **prefix re-stamping,
   fresh child sid, cleared `acp_session_id`** (B1/B2 bite the first fork, so they
   are not deferrable). `fork_parent` field + `fork_session` choke + edge
   persistence (keyed by server sid) + jump-panel family grouping + orphan
   tolerance. No new-context backend risk (shares the parent's resume at spawn).
   Validates the tree UI, persistence, and roster grouping end-to-end.
   Headless-testable on the server harness.
2. **Arbitrary-turn fork (rung 2).** Lift the tip clamp; implement prefix seeding
   by replaying the prefix as synthetic agent context. The backend runtime check
   (does `claude-agent-acp` accept injected history?) gates "done."
3. **Edit-and-rerun UX.** The transcript affordance to edit a past user turn and
   resubmit, built on (2): `fork_session(parent, turn)` + submit edited prompt.
4. **Deferred.** Structural prefix-sharing to eliminate storage duplication;
   side-by-side branch-tile polish (sibling indicators, an explicit compare
   layout) if the side-by-side-via-two-tiles default proves insufficient.

---

## Revision History

- **2026-06-23** — DRAFT created. Models session branching as approach (A)
  tree-over-sessions: a branch is a forked `AgentSession`, the tree is a
  `fork_parent` pointer relationship, seeding the child agent's context is the
  one backend-dependent piece (resolved by a 3-rung ladder with a zero-risk
  tip-only floor). Additive to spec-agent-session-ownership, spec-event-stream,
  and spec-session-server-actor; surfaced in spec-jump-panel via family grouping.
- **2026-06-23** — Adversarial-review pass (verdict REVISE → resolved). B1: the
  prefix is **re-stamped** under the child's `session_id`/`seq` (verbatim copy
  broke the event-stream identity/cursor model, §2/§6), `turn` preserved. B2: the
  child gets a **fresh server sid + cleared `acp_session_id`** so WAL recovery
  can't resume it onto the parent's agent. B3: the seeding ladder is reordered —
  tip-only is the floor (rung 1), **prefix replay** is the real arbitrary-turn
  mechanism (rung 2), truncated resume demoted to "only if the protocol gains it"
  (the implemented `session/load` takes no transcript). N1: added the
  **completed-turn-boundary** fork guard (applies to tip-only too). N2: the edge
  is keyed by **server session id**, not the resume string. N3: added
  **orphaned-child** handling (missing parent → inert edge, detached root; walks
  tolerate dangling pointers). V1/V2: noted family grouping is net-new roster
  projection; `Command::Fork` emits the child's own `ChannelOpened` first.
