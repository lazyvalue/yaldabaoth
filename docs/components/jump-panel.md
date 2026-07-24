# Component: Jump Panel

**Status:** living
**Component token:** `JumpPanel` (⇒ invariants are `UXI-JumpPanel-N`)

## Description

An always-visible root-level **navigator sidebar** (fixed `JUMP_PANEL_WIDTH`),
laid out outside the workspace content so it stays put across workspace switches
(INV-JP1). Toggle: `toggle_jump_panel` (`cmd-j`). Rendered inline (cheap,
O(workspaces + sessions)), not cached. Primary code home:
`jump_panel_view.rs`.

**Palette.** Two header tiers, distinct hues: top-level section headers
("PINNED" / "WORKSPACES" / "AGENT SESSIONS") are **red** (`DetailStyle.err`,
`0xff6b6b`, bold uppercase); per-cwd subheaders are **electric blue** (`0x3b9eff`,
real path casing). The "you are here" active mark + selection tint are the theme's
cyan `frozen_bar` (UXI-JumpPanel-5). **Italic carries exactly one meaning** — the
"waiting on you" session state (UXI-JumpPanel-6); nothing else in the panel is
italic.

Sections:

- **Pinned** — *placeholder* (pinning mechanics land later).
- **Workspaces** — one row per non-ephemeral tab, active marked (accent label +
  left accent bar, `UXI-JumpPanel-5`).
  - Each row's badge shows the **1-based workspace number** (`idx + 1`) — the
    digit `ctrl-<n>` jumps to (INV-UX-11).
  - Click → `select_tab`.
- **Agent sessions** — a **＋ New agent session** create-affordance
  (`UXI-JumpPanel-3`) followed by the universal roster (every server session) ∪
  local-only mid-create sessions (`jump_panel_agent_rows`).
  - **＋ New agent session** → asks for a cwd (UXI-JumpPanel-4) then
    `spawn_free_agent_session_at`: creates a session bound to no tile and no
    workspace; it appears in the rows below as a new unbound (○) row, never
    auto-bound.
  - **Status dot** = what the AGENT is doing (INV-UX-10, UXI-JumpPanel-6) — the
    shape + color are one signal, not binding:
    - **● orange** — working (a reply is in flight).
    - **● green + italic label** — idle with unread output → **waiting on you**
      (UXI-JumpPanel-6).
    - **○ dim** — idle and already read, or disconnected, or a roster-only
      session whose phase we can't know. Disconnected also dims the whole row.
  - Click → bound session focuses its tile; **free** session opens in an
    ephemeral virtual workspace (torn down on switch-away).
  - The row bound to the **focused tile** carries a left accent bar
    (`UXI-JumpPanel-5`) — "this is where you are."

## References

- `docs/specs/spec-jump-panel.md` — deeper design doc.
- ADR-0021 — the ephemeral virtual-workspace decision that shaped free-session
  jump.
- Migrated from `docs/ux-invariants.md` INV-UX-10, INV-UX-18. Those entries are
  now `→ migrated here`.

## UX invariants

### UXI-JumpPanel-1 — The jump-panel agent dot is a per-session status light

**Statement.** Each agent-session row in the jump panel carries a leading dot
whose **shape and color together** are one signal for what the agent is doing —
NOT binding (binding is no longer surfaced in the panel):

- **● orange** (`0xff9e64`) — **working**: a reply is in flight.
- **● green** (`theme.agent.tool_completed`) + **italic label** — **waiting on
  you**: the session finished a turn whose output you haven't read yet
  (UXI-JumpPanel-6).
- **○ dim** — **neutral**: idle and already read, OR the phase is unknown (a
  roster-only session running on the server but never opened here), OR the agent
  is disconnected (which also dims the whole row).

The mapping is a pure function of `(connected, awaiting, unread)` —
`AgentRow::dot_status` → `AgentDotStatus::{Working, WaitingForYou, Neutral}` — so
the render just picks the glyph/hue. Working wins over unread; disconnected wins
over any phase.

**Applies to.** `jump_panel_view.rs`: `jump_panel_agent_rows` (reads each opened
session's `state.turn_phase.is_awaiting()` into `AgentRow::awaiting` and
`state.unread` into `AgentRow::unread`; roster-only rows stay `None`/`false`) and
`render_jump_panel` (glyph + color + italic from `dot_status`).

**Why.** The user wants to glance at the panel and see which agents are working,
which are waiting on them (unread), and which are done — without opening each tile.

**Status.** `partial` — the **mapping** is headless-guarded; the actual
**hue** is a paint/human-eye detail (harness gap #1). Roster-only sessions can't
show working/waiting until the server reports turn state in `SessionInfo` (today
`Neutral`).

**Enforcement.** Headless in `verify_harness.rs`:
`agent_status_dot_reflects_turn_phase` (idle+read→Neutral, idle+unread→
WaitingForYou, mid-turn→Working through the real `jump_panel_agent_rows`) and the
pure `agent_dot_status_mapping` unit test (totality, working-wins,
disconnected-wins). The hue itself is a runtime check.

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
tile bind); the roster→row projection in `jump_panel_agent_rows`. **Two entry
points invoke the same method:** the jump-panel row AND the `?` global menu's
"new agent session" entry (`main.rs` `global_menu()` → `dispatch_menu_command`
→ `"new-free-agent-session"`). Neither is privileged — the jump-panel row just
makes it discoverable where free sessions live; the menu makes it keyboard-
reachable (`?` then `a`).

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
binds it). The `?`-menu entry point is pinned by
`global_menu_offers_and_dispatches_free_agent_session` (the entry is present in the
REAL `global_menu()` and `dispatch_menu_command("new-free-agent-session")` — the
call a menu selection makes — opens the create flow; NC observed RED by dropping
the entry). The daemon round-trip and the row's paint are the named runtime gaps.

_Note: as of UXI-JumpPanel-4 both entry points open a cwd overlay first; the create
(`spawn_free_agent_session_at`) runs on the overlay's commit, not on the click/key._

### UXI-JumpPanel-4 — Creating a free agent session lets you choose its CWD

**Statement.** Both free-session entry points (the jump-panel **＋ New agent
session** row and the `?` menu "new agent session") do NOT spawn immediately.
They first open a **path-input overlay** ("NEW AGENT SESSION AT…") pre-filled
with the default cwd (`agent_base_cwd()` — the active workspace's cwd, else the
process cwd):

1. **Enter accepts the default** — one keystroke keeps the old zero-friction
   behavior (create at the sensible default).
2. **Typing/editing a path** roots the new free session there. On commit the path
   is resolved and validated by `resolve_agent_cwd_arg` (tilde expansion,
   canonicalize, `.`/`..` collapse — spec-agent-cwd.md §2), then
   `spawn_free_agent_session_at(resolved)` creates the session at that cwd.
3. **An invalid path** surfaces a transient status error and creates **nothing**
   — the overlay closes, no session is spawned.
4. **An empty input cancels** (the shared `RenameOverlay` "all-whitespace acts
   like Esc" rule), so hammering Enter on a cleared field never mis-creates.

This is the same overlay machinery as `AgentNewSessionCwd` / `WorkspaceCwd` /
`AgentChangeCwd`; the free-session create just gains its own
`RenameTarget::FreeAgentSessionCwd`. The tile-bound "new session at…" flow
(`AgentNewSessionCwd` → `new_agent_session`) is untouched.

**Applies to.** `main.rs`: `RenameTarget::FreeAgentSessionCwd`,
`open_free_agent_session_cwd_overlay` (prefill), the `FreeAgentSessionCwd` arm of
`commit_rename_overlay` (resolve → spawn / error), the overlay header text, and
the `"new-free-agent-session"` dispatch (now opens the overlay); `jump_panel_view.rs`
`render_jump_panel` (the ＋ row → `open_free_agent_session_cwd_overlay`);
`agent_ui.rs` `spawn_free_agent_session_at` (cwd-parameterized core) +
`spawn_free_agent_session` (default-cwd wrapper).

**Why.** A free agent is "outside a workspace," so it has no workspace cwd to
inherit — the user must be able to say where it runs (which repo/dir the agent's
tools operate in). Without this, every free session silently rooted at the active
workspace's dir, which is often the wrong project for an ad-hoc agent.

**Status.** `implemented`.

**Deviation from plan.** None material. The `?`-menu-opens-overlay assertion was
folded into the existing `global_menu_offers_and_dispatches_free_agent_session`
(retargeted to assert the overlay now opens) rather than a separate
`free_agent_entry_points_open_cwd_overlay` test. `spawn_free_agent_session`'s
no-server note now includes the chosen cwd (`… at <cwd>…`), which is what makes the
commit-routing test observable headlessly.

**Enforcement.** `verify_harness.rs`:
`free_agent_cwd_overlay_opens_prefilled_with_default` (the real
`open_free_agent_session_cwd_overlay` sets a `Rename` overlay targeting
`FreeAgentSessionCwd`, text = `agent_base_cwd()`; NC: change the prefill),
`global_menu_offers_and_dispatches_free_agent_session` (the `?`-menu dispatch opens
the overlay; NC: no-op the dispatch arm), and
`free_agent_cwd_overlay_commit_routes_or_errors` (drives the REAL
`commit_rename_overlay`: a valid path closes the overlay and routes to the spawn —
proven by the no-server note; an invalid path surfaces `"not a directory"` and
spawns nothing; NC: no-op both commit arms). The session actually created AT the
typed cwd needs the daemon (harness gap #2).

### UXI-JumpPanel-5 — The active screen element wears an accent mark in the jump panel

**Statement.** The jump panel marks the row(s) representing the **active screen UX
element** — "this is where you are" — with a **left accent bar** (2px, the theme's
cool primary `AgentTheme.frozen_bar`) plus a matching low-alpha selection tint and
an accent-colored label. (Superseded the original bright-red `0xff6b6b` bounding
box, which read as an alarm and clashed with the warm selection tint — restyled to
the cool accent scheme; the *predicate* is unchanged.) Two independent marks,
0/1/2 marks total:

1. **The active workspace row** — the row whose tab index equals
   `workspace.active_tab` — is always boxed **when that tab is listed** (i.e. it
   is non-ephemeral). If the active tab is an **ephemeral virtual workspace** (a
   free session opened via ADR-0021), it isn't in the Workspaces list, so **no
   workspace row is marked**. The mark is a left accent bar + accent label over the
   tinted selection background.
2. **The focused bound-session row** — the agent-session row bound to the
   **focused tile** (`focused_bound_session()`) — is marked. When the focused tile
   is a **buffer**, or an **unbound** agent tile (selector, no session), there is
   **no session mark**. A roster-only / unopened session is never the focused bound
   session, so it is never marked. A disconnected-but-focused session **still** gets
   its mark (it means "active," orthogonal to the status-dot color / row dim).

There is a single focused tile, so at most one session mark and one workspace mark.
The Pinned section, the "＋ New agent session" affordance, and section headings
never get a mark. The panel renders inline every frame, so the marks track focus
changes, workspace switches, and tile close/rebind with no extra plumbing.

Row-activeness is a pure predicate: `jump_target_is_active(target, active_local,
active_sid)` matches a row's `JumpTarget` against the focused session's local
`SessionId` (Local rows) or its server sid (Roster rows), where `(active_local,
active_sid)` come from `YaldaGpuiView::jump_active_session()` (matches the focused
`App::Agent` tile's `session()` + its `sid_of`). The workspace mark reuses the
existing per-row `active` (= `idx == active_tab`), which is naturally `false` for
every listed row when the active tab is ephemeral.

**Applies to.** `jump_panel_view.rs`: `jump_active_session` +
`jump_target_is_active` (the pure derivation), `jump_nav_row` (`active:
Option<Hsla>` param → left `border_l_2` accent bar + tint + accent label), and
`render_jump_panel` (workspace rows pass `active.then_some(active_accent)`; session
rows pass the same over `jump_target_is_active(...)`; `active_accent` =
`AgentTheme.frozen_bar`). `focused_bound_session` (`main.rs`).

**Why.** With several workspaces and many agent sessions listed, the user loses
track of which one they're currently looking at. A clean accent mark on "where you
are" — workspace and, when applicable, the specific agent session — makes the panel
a map with a "you are here" pin. The mark uses the theme's cool primary accent (a
left bar + low-alpha tint), not a bright red border, so it reads as "current"
rather than "alarm" and stays harmonious with the rest of the chrome.

**Status.** `implemented`.

**Deviation from plan.** `jump_active_session()` does NOT call
`focused_bound_session()` (which `.expect`s a focused window) — the jump panel can
render with no focused window (the `workspace_number_skips_ephemeral` path), so it
matches `workspace.focused_content()` directly and returns `(None, None)` when
absent, rather than panicking. The mark is a 2px left `border_l_2` accent bar
(every row reserves the same-width transparent bar when inactive) so it never
shifts row geometry — no inset margin, which on a `w_full` row would overflow the
panel.

**Enforcement.** `verify_harness.rs`:
`jump_active_box_marks_focused_workspace_and_session` (drives the REAL view:
boots a bound agent tile, focuses it, and asserts through the REAL
`jump_active_session` + `jump_target_is_active` over REAL `jump_panel_agent_rows`
that exactly the focused session's row is active and its workspace tab is active;
switching focus to a buffer tile clears the session mark but keeps the workspace
mark; NC: revert the predicate to `false` and observe RED). The literal accent-bar
pixels are harness gap #1 (human eye).

### UXI-JumpPanel-6 — A backgrounded session that finishes a turn is "waiting on you"

**Statement.** When an agent session finishes a turn (its phase returns to
`Idle`) while it is **not** the focused tile's session, it is marked **unread** —
its jump-panel row shows the ● green dot and an **italic** label ("waiting on
you", UXI-JumpPanel-1). The mark **clears** the moment the session becomes the
focused/viewed session. A turn that finishes while you ARE focused on the session
never marks unread (you're reading it live). Italic in the panel means this and
only this.

State lives on `AgentState.unread`. It is **set** in the one idempotent turn-end
chokepoint `finalize_agent_turn_idem` (so every turn-end path — the pump
inference, the legacy `ServerNotification::TurnEnded`, and the forwarded
`AgentEvent` boundary — converges there). It is **cleared** for the focused
session at three points: `jump_to_session` (eager, same-frame on a jump-panel
click, via `mark_session_read`), the tail of `pump_session` (overrides the set on
the same tick for the focused session, no flicker), and the tail of
`apply_server_batch` (clears the focused session after a forwarded turn-end).

**Applies to.** `agent.rs` (`AgentState.unread`, set in `finalize_agent_turn_idem`);
`agent_ui.rs` (`pump_session` focused-clear, `apply_server_batch` focused-clear,
`jump_to_session` + `mark_session_read`); `jump_panel_view.rs`
(`AgentRow::unread`, `dot_status`, the italic row + ● green in `render_jump_panel`).

**Why.** With many sessions, the user needs to see at a glance which agents have
produced output that's waiting for them — distinct from ones still working and
ones they've already read. Clearing on focus makes the mark mean "you haven't
looked," not merely "a turn ended."

**Status.** `implemented`. Roster-only sessions (never opened here) have no local
`unread` and stay neutral until opened.

**Enforcement.** `verify_harness.rs`:
`jump_dot_unread_on_background_turn_end_read_on_focused` (drives the REAL
`apply_server_batch` → `ServerNotification::TurnEnded` on a backgrounded vs a
focused session; asserts through REAL `jump_panel_agent_rows` + `dot_status` that
the backgrounded one is `WaitingForYou` and the focused one is `Neutral`; NC:
remove `self.unread = true` in `finalize_agent_turn_idem` → the backgrounded row
reads `Neutral`, observed RED). The green/italic pixels are harness gap #1.
