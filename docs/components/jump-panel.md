# Component: Jump Panel

**Status:** living
**Component token:** `JumpPanel` (⇒ invariants are `UXI-JumpPanel-N`)

## Description

An always-visible root-level **navigator sidebar** (fixed `JUMP_PANEL_WIDTH`),
laid out outside the workspace content so it stays put across workspace switches
(INV-JP1). Toggle: `toggle_jump_panel` (`cmd-j`). Rendered inline (cheap,
O(workspaces + sessions)), not cached. Primary code home:
`jump_panel_view.rs`. Sections:

- **Pinned** — *placeholder* (pinning mechanics land later).
- **Workspaces** — one row per non-ephemeral tab, active marked (accent label).
  - Each row's badge shows the **1-based workspace number** (`idx + 1`) — the
    digit `ctrl-<n>` jumps to (INV-UX-11).
  - Click → `select_tab`.
- **Agent sessions** — a **＋ New agent session** create-affordance
  (`UXI-JumpPanel-3`) followed by the universal roster (every server session) ∪
  local-only mid-create sessions (`jump_panel_agent_rows`).
  - **＋ New agent session** → `spawn_free_agent_session`: creates a session bound
    to no tile and no workspace; it appears in the rows below as a new unbound
    (○) row, never auto-bound.
  - **Dot shape** = binding: `●` in-use / `○` free.
  - **Dot color** = per-session status light (INV-UX-10): **working** (reply in
    flight) = warm accent, **waiting for you** (turn finished) = green,
    **neutral/disconnected** = dim. Disconnected also dims the whole row.
  - Click → bound session focuses its tile; **free** session opens in an
    ephemeral virtual workspace (torn down on switch-away).

## References

- `docs/specs/spec-jump-panel.md` — deeper design doc.
- ADR-0021 — the ephemeral virtual-workspace decision that shaped free-session
  jump.
- Migrated from `docs/ux-invariants.md` INV-UX-10, INV-UX-18. Those entries are
  now `→ migrated here`.

## UX invariants

### UXI-JumpPanel-1 — The jump-panel agent dot is a per-session status light

**Statement.** Each agent-session row in the jump panel carries a leading dot
whose **shape** encodes binding (`●` in-use / `○` free) and whose **color** is a
status light for the session's turn phase:

- **working** (a reply is in flight) → warm accent (`theme.agent.warm_accent`),
- **waiting for you** (the turn finished; it's the user's move) → green
  (`theme.agent.tool_completed`),
- **neutral** (dim) when the phase is unknown (a roster-only session running on
  the server but never opened in this GUI, so no local `TurnPhase`) **or** the
  agent is disconnected (which also dims the whole row).

The mapping is a pure function of `(connected, awaiting)` — `AgentRow::dot_status`
→ `AgentDotStatus::{Working, WaitingForYou, Neutral}` — so the render just picks
the hue. Disconnected wins over any prior phase.

**Applies to.** `jump_panel_view.rs`: `jump_panel_agent_rows` (reads each opened
session's `state.turn_phase.is_awaiting()` into `AgentRow::awaiting`; roster-only
rows stay `None`) and `render_jump_panel` (shape from `bound`, color from
`dot_status`).

**Why.** The user wants to glance at the panel and see which agents need them
versus which are still working — without opening each tile.

**Status.** `partial` — the **mapping** is headless-guarded; the actual
**hue** is a paint/human-eye detail (harness gap #1). Roster-only sessions can't
show working/waiting until the server reports turn state in `SessionInfo` (today
`Neutral`).

**Enforcement.** Headless in `verify_harness.rs`:
`agent_status_dot_reflects_turn_phase` (idle→WaitingForYou, mid-turn→Working
through the real `jump_panel_agent_rows`) and the pure `agent_dot_status_mapping`
unit test (totality + disconnected-wins). The hue itself is a runtime check.

### UXI-JumpPanel-2 — Jump-panel items reorder by drag, at two levels, cwd-bounded

**Statement.** The jump panel's "Agent sessions" section is user-reorderable by
drag-and-drop at two levels, and the ordering survives restart:

1. **cwd groups reorder.** Dragging a cwd subheader onto another moves that whole
   group; the header order is the user's, persisted in
   `Preferences::jump_cwd_order` (a list of cwd display keys).
2. **sessions reorder within their group.** Dragging a session row onto another
   in the SAME group reorders it there, persisted in
   `Preferences::jump_session_order` (a list of server sids).
3. **A session never crosses cwd groups.** A session drag is accepted only by a
   drop target in its own cwd group (`can_drop` gates on the drag payload's
   `cwd_key`), and `reorder_session` defensively re-checks and refuses a
   cross-cwd move. So a session can never be dragged into a cwd it doesn't belong
   in — it is a hard gate at the gesture AND a guard in the state change.

Both orders are applied as a **stable** sort by "rank in the order list" over the
cwd-grouped rows (`order_grouped_rows`), so an empty/absent order list is a total
no-op: the panel stays alphabetical (groups) / by-label (sessions) until the user
drags. Items not yet in an order list sort after the listed ones, keeping their
default position. Only roster-backed sessions (stable sid) drag; local mid-create
placeholders don't.

4. **A row NEVER moves except by a user drag.** Nothing else — an async roster
   refresh, a `SessionCreated`/`SessionDeleted` broadcast, a reconnect, or a
   `/clear` — may change a session's slot. `/clear` is the load-bearing case: it
   kills the server session and creates a new one with a NEW sid, so the row is
   the *same session* to the user but a different key to the order list. Its
   continuity is carried by an explicit **order succession**
   (`jump_order_succession`: placeholder `SessionId` → predecessor sid): the
   mid-open placeholder ranks by `AgentRow::order_sid` = the predecessor's sid,
   and at bind time `inherit_order_slot` substitutes the fresh sid for the
   predecessor's IN PLACE in `jump_session_order`. Any future
   "kill and re-create the same session" flow must record a succession the same
   way, or the row will sink to the bottom of its group (bug-0007).

**Applies to.** `jump_panel_view.rs` (`SessionDrag`/`CwdDrag` payloads,
`JumpDragPreview`, `group_agent_rows_by_cwd` + `order_grouped_rows` +
`reorder_move`, `reorder_cwd_group`/`reorder_session`, and the `on_drag` /
`drag_over` / `can_drop` / `on_drop` wiring in `render_jump_panel`), and the
`Preferences::{jump_cwd_order, jump_session_order}` persistence (`persist.rs`,
loaded/saved in `main.rs`).

**Why.** Alphabetical-by-label is not how a user thinks about their projects and
agents; manual curation (drag the active project to the top, order its agents by
task) is. Grouping stays cwd-anchored because the cwd is the project axis — so a
session moving to a foreign project's group would be a lie about where it runs,
hence the hard cwd gate.

**Status.** `implemented` — headless for the ordering + reorder state change; the GPUI
mouse-drag GESTURE that dispatches the drop is `NEEDS-RUNTIME` — gap #2, there is
no headless drag-dispatch seam, so the on_drag/on_drop wiring itself is
human-verified.

**Enforcement.** `verify_harness.rs`:
`jump_reorder_ordering_applies_and_defaults_to_alpha` (the pure order +
negative-control default), `jump_reorder_move_semantics` (list surgery), and
`jump_reorder_methods_reorder_and_gate_by_cwd` (drives the REAL
`reorder_cwd_group` / `reorder_session` the drop handlers call: headers reorder,
sessions reorder within group, cross-cwd refused).
Clause 4 (slot stability across `/clear`) is pinned by
`clear_keeps_the_sessions_jump_panel_slot`, which drives the REAL
`clear_agent_session` + the REAL async `Created` resolution and asserts the slot
both mid-open and post-bind (negative controls observed RED on each arm).

### UXI-JumpPanel-3 — The jump panel can create a free (tile-less) agent session

**Statement.** The jump panel's "Agent sessions" section leads with a **＋ New
agent session** action row. Clicking it creates an agent session bound to **no
tile and no workspace** — a *free* session (spec-agent-session-ownership.md) —
via `spawn_free_agent_session`:

1. The new session lands in the universal roster and appears as a row in the
   list below, **unbound** (`○`), exactly like any other free session — it is
   **never auto-bound** to a tile by the create, so nothing the user is looking
   at changes except a new row appearing.
2. It is **bindable later** the ordinary way — selecting the row binds it (a
   bound tile focuses; a free row opens an ephemeral virtual workspace,
   ADR-0021).
3. With **no session server** there is no roster to host a free session, so the
   action is a graceful no-op that sets a transient status note and creates
   nothing — it never panics and never auto-binds a phantom.

**Applies to.** `jump_panel_view.rs` `render_jump_panel` (the `jump-new-agent`
row → `on_click` → `spawn_free_agent_session`); `agent_ui.rs`
`spawn_free_agent_session` (no-server guard + create + `refresh_roster`, never a
tile bind); the roster→row projection in `jump_panel_agent_rows`. The `?` global
menu's "new agent session" entry (`main.rs`, `"new-free-agent-session"`) invokes
the same method — the jump-panel row just makes it discoverable where free
sessions live.

**Why.** The user wants to spin up an agent without first placing a tile for it
— create the worker, bind it to a viewport (or not) whenever. The capability
existed only as a buried `?`-menu item; surfacing it in the jump panel makes
"create an agent that isn't attached to a tile" a one-click, discoverable act.

**Status.** `implemented` — the no-server contract and the free-then-bindable
projection are headless; the live server `create_session` round-trip is
`NEEDS-RUNTIME` (harness gap #2 — needs the daemon) and the ＋ row's click paint
is gap #1.

**Enforcement.** `verify_harness.rs`:
`free_agent_session_no_server_is_graceful_noop` (drives the REAL
`spawn_free_agent_session` with no session server: a status note is set, the
store gains no session and no tile binds — negative control: the method's
no-server guard) and `free_agent_row_is_unbound_and_bindable` (a
`SessionCreated` roster broadcast — the end state the create produces — surfaces
as an unbound `○` row through the real `jump_panel_agent_rows`, then `jump_to_agent`
binds it). The daemon round-trip and the row's paint are the named runtime gaps.
