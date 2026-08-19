# Agent Session Ownership — project session, normalized state, viewport references

Status: DRAFT (supersedes the binding/lifecycle parts of `spec-multi-session.md`
and the multi-subscriber/lease parts of `spec-session-server-actor.md`).

> **Placement superseded by ADR-0033 (2026-08-18).** The normalized,
> project-owned session store and one-session-entity rules below remain
> authoritative. The “free session,” ephemeral viewport, and durable-reference
> placement model does not. Every navigable session is represented by a stable
> Agent tile whose workspace membership is **bound** or **unbound**; an Agent
> tile with no selected session is **empty**. Direct navigation focuses an
> unbound tile without creating another viewport.

> **Extended by ADR-0028 / `docs/components/project.md` (Projects primitive):** a
> session now also has a **project membership** — `Assigned` (a stored
> `ProjectId`) or `Inferred` (`Projects::by_cwd(session.cwd)`) or `Unfiled`. The
> durable tile placement (INV-2 below) is additionally **gated to intra-project**:
> a session may be placed only in a workspace sharing the session's project
> (`UXI-Project-6`). A session keeps its immutable spawn `cwd` (server-side ground
> truth); only its *project* is derived. The store patterns here are the model the
> `Projects` store mirrors.

## Why we keep breaking this

Every agent-session bug for months — duplicate attaches, two tiles mirroring
each other's input/output, "attached ×4", stuck-reconnecting ghosts — is one
defect wearing different hats:

> **There is no enforced invariant binding a session to a tile, and the binding
> state is raw public fields (`AgentSlot.server_session_id`, `AgentRing.slots`)
> that ~11 independent code paths mutate directly.**

Bind points that each create/attach a session with **no** coordination:
`restore_agent_leaves`, `open_agent_inner`, `new_agent_session`,
`bootstrap_fresh_agent_session`, `apply_open_agent_resolution` (×2 arms),
`picker_attach_existing`, `picker_start_new`, reconnect, lease handoff,
`change_agent_cwd`. The only dedup (`open_sids`) lives in 2 of them and is racy
(snapshots at list-time, binds later).

The amplifier: `for_each_server_session_slot` *fans a session's events out to
every tile that holds it*. So the moment two paths bind the same session, the
bug becomes visible mirroring instead of failing loudly.

**Why the big refactors didn't fix it:** they were *layer* refactors (transcript
reconciler, render pipeline, session-server actor, lease). Each made its layer
internally correct. This defect lives in the **seam** between the server's
"session" and the GUI's "tile" — no layer owns it, so the symptom relocates to
whichever bind path wasn't hardened that round.

The multi-subscriber machinery (forwarders fan-out, leases, owner/observer,
candidate/promote) exists for **one** feature: the self-hosting blue-green
`:promote` loop. We are dropping that feature. With it gone, the entire reason
for many-to-many binding evaporates.

## Decision

1. **Project owns the session; workspaces own viewports.** Each session belongs
   logically to exactly one project. `AgentSessions` is the normalized runtime
   store for session identity and state, keyed by `SessionId`; it is not layout.
   Each workspace owns its `AgentTile`s, and every bound tile holds an ordinary
   reference to a session. Multiple viewports may reference the same session.
2. **One session entity per server sid.** The private `sid → session` index is
   maintained *only* by `AgentSessions`. Illegal duplicate runtime entities
   (and therefore duplicate transports/reducers) are unrepresentable because
   the only way to obtain a session is through an API that returns the existing
   one.
3. **Ownership inversion.** Session *state* (`AgentState`, channel, label, cwd,
   resume id) moves **out** of the layout tree and into `AgentSessions`. Tiles
   hold lightweight **keys** (`SessionId`), not state. Routing becomes an O(1)
   map lookup, not a tree walk; there is nothing to keep in sync.
4. **`:promote` mirror is dormant, then deleted.** The client stops using the
   server's multi-subscriber/lease surface (one attach per session, ever). The
   server keeps working unchanged; its now-unused lease/forwarder-fan/owner code
   is deleted in a later, separate pass (no protocol change needed now).

## Model

```rust
/// Stable local identity, monotonic, never reused. Independent of the server
/// sid (which is absent pre-attach and can change on resume-fallback).
struct SessionId(u64);

/// One project-owned agent conversation. State that used to live in
/// AgentSlot.state + the slot's binding fields, now normalized centrally.
struct AgentSession {
    state: AgentState,              // editor / transcript / tools / turn_phase / channel
    label: String,
    cwd: PathBuf,
    resume_id: Option<String>,
    server_session_id: Option<String>,   // bound once, on attach
    // DELETED: is_driver, lease/owner/candidate fields.
}

/// The normalized runtime store. Private fields — the rest of the app touches
/// session entities ONLY through this API. Project membership is resolved from
/// the session's immutable spawn cwd through `Projects` (UXI-Project-2).
struct AgentSessions {
    sessions: BTreeMap<SessionId, AgentSession>,
    by_sid: HashMap<String, SessionId>,   // private; maintained internally
    next_id: u64,
}
```

```rust
enum Bind { Created(SessionId), AlreadyOpen(SessionId) }

impl AgentSessions {
    /// Idempotent server-session entry — the ONLY way to bind a sid.
    /// If a session already carries this sid, returns AlreadyOpen(id) and
    /// mutates nothing. Else creates one and returns Created(id).
    fn open_or_focus(&mut self, sid: &str, label: String, cwd: PathBuf,
                     resume_id: Option<String>) -> Bind;

    /// A fresh local session with no sid yet (pre-attach placeholder).
    fn create_local(&mut self, label: String, cwd: PathBuf) -> SessionId;

    /// Bind a sid to an existing local session once attach resolves.
    /// Errors if that sid is already bound elsewhere (caller drops the dup).
    fn bind_sid(&mut self, id: SessionId, sid: String) -> Result<(), AlreadyBound>;

    fn locate(&self, sid: &str) -> Option<SessionId>;   // O(1) routing
    fn get(&self, id: SessionId) -> Option<&AgentSession>;
    fn get_mut(&mut self, id: SessionId) -> Option<&mut AgentSession>;
    fn close(&mut self, id: SessionId) -> Option<AgentSession>;  // drops channel/forwarder
    fn iter(&self) -> impl Iterator<Item = (SessionId, &AgentSession)>;
}
```

Tiles change from owning state to holding *one reference key*:

```rust
enum App { Buffer(BufferApp), Agent(AgentTile) }

struct AgentTile {                 // a VIEW onto one session, not a store
    bound: Option<SessionId>,      // reference to the shown session; None ⇒ picker
    underlying: Option<Box<BufferApp>>,
    picker: Option<SessionPicker>, // lists FREE sessions + "new"
}
```

A tile shows **exactly one** session at a time. There is no in-tile session
ring. The same `SessionId` may be referenced by a durable workspace tile and a
direct ephemeral viewport at once; both render the one shared session entity.

## Placement, free sessions, and rebind — superseded by ADR-0033

The remainder of this section is retained as design history. Its current
replacement is ADR-0033 plus `UXI-Workspace-16`,
`UXI-JumpPanel-23`, and `UXI-AgentTile-34`.

Sessions exist in the project/session domain independently of tiles — a session
can run with no viewport displaying it. Placement is the dynamic map from
**non-ephemeral workspace** tiles to the session each shows. Ephemeral tiles are
viewports but not placement.

- **Free session** — a `SessionId` with no durable workspace-tile reference.
  Computed as `store.ids() − {bound ids in non-ephemeral workspaces}`. A bare
  direct view does not change this classification.
- **Bind/reference** — point a tile at a session: `tile.bound = Some(id)`.
  Normal workspace picker placement accepts only a free session (or focuses its
  existing placement), while direct navigation creates an ephemeral reference
  regardless of whether durable placement exists.
- **Rebind** — change `tile.bound` from A to a free B; A becomes free (it keeps
  running in the store / on the server — rebinding never kills a session).
- **Close the tile** — frees its session (the session keeps running); the tile
  falls back to its `underlying` buffer.
- **Kill the session** — an explicit, separate action: `store.close(id)` +
  server-side close. Removes it from the store and detaches.

The picker/switcher is the UI for this: it lists the **free** sessions plus a
"start a new session" row. Opening an agent over a buffer, or invoking the
switcher on an existing agent tile, both go through it.

## Invariants (enforced by construction)

- **INV-1 — one session per sid.** `by_sid` is a map; `open_or_focus`/`bind_sid`
  are the only writers. Two `AgentSession`s for one sid cannot exist.
- **INV-2 — viewport references do not own session state.** A `SessionId` has at
  most one durable real-workspace placement under the current picker policy, but
  may have additional ephemeral viewport references. Every tile uses the same
  `AgentTile::Bound { session }` shape. Free-session and persistence calculations
  scan only non-ephemeral workspaces; ordinary picker attach still focuses an
  existing durable placement.
- **INV-3 — one channel per session.** The channel/forwarder lives on the single
  `AgentSession`; closing it is the only detach. No second attach can occur
  because `open_or_focus` short-circuits on an existing sid.
- **INV-4 — routing is total and unique.** `route(sid)` resolves to 0 or 1
  sessions via `locate`. Fan-out is deleted; finding >1 is now impossible.

## The single placement choke

All ~11 bind paths collapse into one entry point on the view:

```rust
/// Resolve `sid` to its one session entity, creating it if needed. Placement
/// surfaces may focus its durable tile; direct-navigation surfaces may create
/// an additional ephemeral viewport reference.
fn show_session(&mut self, sid: Option<&str>, want_new_tile: bool, cx) -> SessionId
```

`restore_agent_leaves`, `open_agent`, the picker, reconnect, and cwd-respawn all
call this. The racy `open_sids` dedup is deleted — uniqueness is structural now.

## What gets deleted (this pass, client-side)

- `for_each_server_session_slot` (fan-out) and every caller.
- `AgentSlot` (folded into `AgentSession` + `AgentRing.order`).
- `is_driver`, `pending_open_token`, the `open_sids` snapshots, lease heartbeat
  calls, owner/observer/candidate/promote branches in `agent_ui.rs`.

Server-side lease/forwarder-fan/promote code is left **dormant** and removed in a
separate follow-up (`spec-session-server-actor.md` cleanup), since no protocol
change is required for the client to behave 1:1.

## Migration (each stage builds + tests green before the next)

1. **Types.** Add `SessionId`, `AgentSession`, `AgentSessions`; unit-test the
   invariants in isolation (open_or_focus idempotency, bind_sid rejection).
2. **Own the state.** Move `AgentState` ownership into `AgentSessions`; change
   `AgentRing` to `Vec<SessionId>`. Update `agent_mut()`, `render_agent`,
   persistence to go through keys. (Largest mechanical step.)
3. **One choke.** Route every bind path through `show_session`; delete the racy
   dedup and the duplicate placeholder/attach flows.
4. **Kill fan-out.** Replace `for_each_server_session_slot` with `locate`-based
   single-session routing; assert-or-log if >1 ever appears (it can't).
5. **Dormant promote.** Stop calling lease/owner APIs from the client.

## Resolved decisions

- **One session per tile, rebindable** (not an in-tile ring). A tile shows one
  session; the switcher rebinds it to any free session. This replaces Ctrl-]/[
  cycling with explicit rebind and is strictly simpler.
- **Closing a tile frees its session; it is not killed.** Killing is an explicit
  separate action.
- **`AgentRing` → `AgentTile`.** The `Vec<AgentSlot>` ring collapses to a single
  optional `SessionId` binding + the picker. Ring-cycling code (`next`/`prev`,
  `active`, `slot_by_index`, the Ctrl-]/[ handlers) is deleted.
