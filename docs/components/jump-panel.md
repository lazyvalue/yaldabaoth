# Component: Jump Panel

**Status:** living
**Component token:** `JumpPanel` (⇒ invariants are `UXI-JumpPanel-N`)

## Description

An always-visible root-level **navigator sidebar** (fixed `JUMP_PANEL_WIDTH`,
currently 320px),
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

> **Current chrome (`UXI-JumpPanel-7/-8`).** The Sections list below predates the
> declutter: the inline **＋ create rows**, the **✕** close glyph, and the **cwd
> subtext** are GONE. Create/delete now live in menus (global "new project"; a
> per-project **context menu** on the name). Rows carry **icons** (`⊞`
> workspaces / status-colored `✦` agent sessions — the ●/○ dot is folded into the
> star's color), the workspace `ctrl-<n>` number is a **dim right-edge hint**,
> labels are **SEMIBOLD**, sections are split by a **hairline rule above** each
> header, and the panel background is a **recessed shade** of the editor bg
> (`jump_panel_bg`). Read UXI-JumpPanel-7/-8 for the authoritative behavior.

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

- `docs/components/README.md` § Terminology — **free** (a session no tile binds) and
  **bare agent view** (the ephemeral workspace a free session opens in). The panel's
  agent list is where free sessions live.
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

4. **In the All tab, a row NEVER moves except by a user drag.** Nothing else —
   an agent-state transition, async roster refresh, a
   `SessionCreated`/`SessionDeleted` broadcast, a reconnect, or a `/clear` — may
   change an existing session's slot. (Waiting and Working intentionally use
   chronological state-entry order per `UXI-JumpPanel-14`.) `/clear` is the
   load-bearing All-tab case: it
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

> **SUPERSEDED by `UXI-Project-3/4/7` (T005).** The global, tile-less "＋ New
> agent session" that asked for a cwd is gone: sessions are now created only
> **inside a project** via the jump panel's per-project ＋New agent session row
> (`new_agent_session_in`, cwd = the project's, no prompt). The `?`-menu entry and
> its cwd overlay were removed. The free-then-bindable *projection* below still
> holds; only the create *entry point* moved into a project. The removed guards
> `global_menu_offers_and_dispatches_free_agent_session` /
> `free_agent_cwd_overlay_*` are replaced by `global_cwd_session_overlay_is_gone`
> (entry point removed) and `jump_panel_renders_per_project_sections` (per-project
> ＋ rows).

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

> **SUPERSEDED by `UXI-Project-4` (T005).** There is no longer a cwd to choose at
> session-create time — a session inherits its **project's** cwd. The
> "NEW AGENT SESSION AT…" / "NEW SESSION AT…" path-input overlays
> (`RenameTarget::FreeAgentSessionCwd` / `AgentNewSessionCwd`) and their commit
> arms are deleted. The cwd is chosen once, when the **project** is created
> (`UXI-Project-4`'s NEW PROJECT overlay).

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

_Note (UXI-JumpPanel-7): the status glyph is now a **`✦`** whose COLOR carries the
state (orange working / green waiting / dim neutral), not a separate ●/○ dot. The
`dot_status` mapping and its enforcement are unchanged — only the rendered glyph._

### UXI-JumpPanel-7 — The jump panel is a decluttered navigator; create/delete live in menus, and it wears a recessed shade

**Statement.** The jump panel is a **pure navigator** — it carries no inline
create/delete chrome and no per-project cwd subtext. Specifically, all of the
following are **gone** from the panel body:

1. the **cwd subtext line** under each project header (the project name stands
   alone);
2. the top-level **＋ New project** row — project creation moved to the **GLOBAL
   menu** (`?` → "new project", command `new-project` → `open_new_project_overlay`);
3. the per-project **＋ New workspace** and **＋ New agent session** rows — these
   moved into the project **context menu** (`UXI-JumpPanel-8`);
4. the **✕ close-project** glyph on the header — delete also moved into the
   context menu.

The panel's **look** is:

- **Inter-section rule.** A thin inset **hairline above each project header**
  (`jump_divider`, 1px, the dim border color at α 0.4, inset `mx_3`, 14px above /
  6px below) separates sections — replacing the old under-name underline. Pinned
  always precedes the first project, so every project section gets a top rule.
- **Icons.** A leading **`⊞`** marks a workspace row (dim); a leading **`✦`** marks
  an agent-session row, colored by status (`UXI-JumpPanel-1`). The workspace's
  `ctrl-<n>` number moves from the leading badge to a **dim right-edge hint**.
- **Weight.** Row labels render at **`SEMIBOLD`** (they read too thin at normal
  weight; still a step under the `BOLD` uppercase project headers, so the header >
  row hierarchy holds). The project-name red is softened to α 0.9.
- **Recessed background.** _(SUPERSEDED by `UXI-JumpPanel-11` — the panel now wears
  the command-menu surface, `menu_panel_bg`, and `agent.jump_panel_bg` is gone.
  Kept here for the record.)_ The panel background is `agent.jump_panel_bg` when a
  theme art-directs one (e.g. **Nightfox** `#0d1119`), else `jump_panel_bg(editor_bg())`
  — a touch darker in **lightness** than the editor at the **same hue + saturation**
  (dropping saturation is what muddies a tinted bg), with a **lighten-flip** on
  near-black themes so the seam never vanishes. A fixed ΔL (−0.035 dark / −0.045
  light / +0.04 near-black), not a multiply or black-composite (those vanish at
  low L and overshoot at high L). The darker shade also makes the cyan selection
  tint pop.

- **Theme-owned accents.** The header red, the "Unfiled" subheader blue, and the
  "working" star color are no longer fixed constants — they are `AgentTheme`
  fields (`jump_header` / `jump_subheader` / `jump_working`), defaulting to the old
  theme-neutral values for every theme, with **Nightfox** art-directed to its
  palette (`#c94f6d` / `#719cd6` / `#f4a261`). The waiting-green (`tool_completed`),
  neutral-dim (`dim`), and selection cyan (`frozen_bar`) were already theme-derived.

The navigation/marks/status behavior (`UXI-JumpPanel-1/-2/-5/-6`, `UXI-Project-3`)
is otherwise unchanged; the section view-model (`jump_panel_sections`) is
untouched — only `render_jump_panel` and `jump_nav_row` changed.

**Applies to.** `jump_panel_view.rs`: `render_jump_panel` (removed rows, header
without cwd/✕, per-section `jump_divider`, `⊞` workspace badge + right-edge number
hint, `✦` session badge via `jump_session_row_el`, `jump_panel_bg` background),
`jump_nav_row` (new `hint` param + `SEMIBOLD` label), `jump_panel_bg` +
`jump_divider` (new free fns). `main.rs`: `global_menu` (+ "new project" entry),
`dispatch_menu_command` (+ `"new-project"` arm), `render_project_menu` (delete color
from `jump_header`). `theme.rs`: `AgentTheme.{jump_panel_bg, jump_header,
jump_subheader, jump_working}` (all 8 themes; Nightfox art-directed).

**Why.** With projects, workspaces, and sessions all listed, the panel had grown a
thicket of ＋/✕ affordances and cwd lines that competed with the navigation it
exists for. Moving create/delete into menus and giving the panel a clean, shaded,
iconified look makes it scannable — a map, not a control panel.

**Status.** `implemented` — the panel-background derivation and the create
relocation are headless-guarded; the literal glyphs (`⊞`/`✦`), the SEMIBOLD/red
weights, the hairline, and the recessed shade as *pixels* are harness gap #1
(human eye).

**Enforcement.** `verify_harness.rs`:
`jump_panel_bg_shades_by_theme_and_preserves_hue` (the pure `jump_panel_bg`:
dark → darker, near-black → lighter, light → darker, hue+sat+alpha preserved; NC:
return `editor` unchanged → the dark/near-black asserts fail) and
`new_project_relocated_to_global_menu` (the global menu offers "new project" and
`dispatch_menu_command("new-project")` opens the REAL New Project overlay; NC:
drop the `"new-project"` dispatch arm → the overlay never opens). The theme-owned
accents are pinned by `tests.rs::nightfox_jump_panel_colors_are_art_directed`
(Nightfox carries its palette-native jump colors, Dracula preserves the legacy
constants + `jump_panel_bg: None`; NC: revert Nightfox's fields to the constants →
RED). The removed rows' absence + the literal glyphs/colors are gap #1.

### UXI-JumpPanel-8 — Clicking a project name opens a context menu of project-scoped actions

**Statement.** Clicking a project's **name** in the jump panel opens a small
**context menu** anchored at the cursor, offering the project-scoped actions:

1. **⊞ New workspace** → `new_workspace_in(pid)` (a workspace in that project, cwd
   = the project's, no prompt).
2. **✦ New agent session** → `new_agent_session_in(pid)` (a free session rooted at
   the project's cwd).
3. **✕ Delete project** → `request_delete_project(pid)` (confirm-then-cascade for a
   non-empty project, direct delete for an empty one — `UXI-Project-5`).

The menu is a lightweight popup layered over the still-visible panel (not an opaque
body swap): a transparent full-window **click-away backdrop** under a positioned
popup, so a click on the popup hits an item and a click anywhere else dismisses.
The popup **must `.occlude()`** (bug-0019): GPUI's hit test collects *every* hitbox
under the pointer and stops only at a `BlockMouse` one, so a non-occluding popup
leaves the backdrop hovered underneath — pressing an item dismissed the menu on
mouse **down**, and `on_click` (down-then-up on the same element) never fired, i.e.
every item was inert to the mouse while the `w`/`a`/`d` accelerators still worked.
**Esc** closes; single-key accelerators (`w`/`a`/`d`) fire the items. Choosing an
item **dismisses the menu first**, then runs the action (so each action's
`has_overlay()` guard passes). The menu opens at the click position, nudged a hair
down-right and clamped to the viewport (flipping above the anchor near the bottom
edge). The header stays a `CwdDrag` source, so a **click** opens the menu while a
**drag** still reorders the section (`UXI-JumpPanel-2`).

**Applies to.** `jump_panel_view.rs` `render_jump_panel` (the header `on_click` →
`open_project_menu`, coexisting with `on_drag`). `main.rs`:
`ActiveOverlay::ProjectMenu { pid, x, y }`, `open_project_menu` (anchor + clamp),
`project_menu_action` (dismiss-then-act), `handle_project_menu_key` (Esc + `w`/`a`/
`d`), `render_project_menu` (backdrop + positioned popup, overlay-styled with cyan
α 0.15 inset hover pills), and the `overlay_is_project_menu` render branch.

**Why.** The per-project ＋ rows and the ✕ glyph cluttered the panel and scaled
badly (three affordances per project). A single click-to-open menu on the name is
discoverable, precise (scoped to the clicked project), and keeps the panel body
clean (`UXI-JumpPanel-7`).

**Status.** `implemented` — open, action-dispatch AND the real mouse click (press
→ release on the item's painted rect, through the window's mouse dispatch) are all
headless. Only the popup's placement/hover **pixels** remain harness gap #1.

**Enforcement.** `verify_harness.rs::project_menu_opens_on_name_click_and_actions_dispatch`
(drives the REAL `open_project_menu` → the overlay is `ProjectMenu` for that pid →
`project_menu_action(NewWorkspace)` creates a workspace in the project and closes
the menu → re-open + `DeleteProject` arms the confirm overlay; NCs: skip
`open_overlay(ProjectMenu…)` → menu never opens; no-op `new_workspace_in` in the
action → count assert fails). Plus
`verify_harness.rs::project_menu_item_click_runs_the_action` (bug-0019 — the MOUSE
path: probe the item's painted bounds, `simulate_click` at its centre through the
real window dispatch → a workspace is created and the menu dismisses; a click far
outside still dismisses. NC: drop `.occlude()` → the press is swallowed by the
backdrop and the count assert fails).

**Deviation from plan.** The requested menu named only "New workspace / New agent
session"; **Delete project** was added as a third item (separated by a rule) so
removing the header ✕ (`UXI-JumpPanel-7`) doesn't strand the delete capability —
the per-project menu is its natural, precise home. Placement is stored as a clamped
`(x, y)` computed from `viewport_{width,height}_px` at open time (no live `Window`
needed in the render).

### UXI-JumpPanel-9 — `Cmd-P` opens a fuzzy jump palette over the same list the panel shows

**Statement.** `Cmd-P` opens a centered **jump palette** — a type-to-filter dialog
over the *same* navigable set the sidebar projects: every **non-ephemeral
workspace** and every **agent session** (`Local` ∪ `Roster`). It is a pure
alternate *input* onto that list — it introduces **no new jump semantics**; every
activation runs the panel's existing dispatchers (`select_workspace` for a
workspace, `jump_to_agent` for a session), so 1:1 binding, ephemeral-workspace
teardown, and read-marking stay owned where they already are.

**Projects are not candidates** — a project is a container, not a view target
(clicking one opens a menu, `UXI-JumpPanel-8`). **Ephemeral workspaces are not
candidates** — one is created on demand by `jump_to_session` and torn down on
navigate-away, so it is never a thing you name and type.

Behavior:

1. **Empty query** ⇒ the **full list in panel order** (each project section's
   workspaces then its sessions, then the unfiled session groups), so
   `Cmd-P` → arrows → `Enter` is a keyboard navigator with no typing.
2. **Typing** ⇒ candidates are **filtered by subsequence match and ordered by
   match score**, best first. Score rewards contiguous runs, word-start hits, a
   whole-prefix hit and an exact hit, and prefers shorter labels on a tie; panel
   order is the final tiebreak (stable). The **top row is the best match**, not
   merely the first list member that matched.
3. **Selection** — exactly one row is highlighted, defaulting to the top match
   and **reset to the top on every query edit**. `Up`/`Down` move the highlight
   (wrapping); moving the highlight does **not** navigate.
4. **`Enter`** activates the **highlighted** row (which is the top match unless
   you moved), closes the palette, then jumps.
5. **No matches** ⇒ a dim "No matches" line; `Enter` is a **no-op** and the
   palette stays open.
6. **`Esc`** closes with no navigation.
7. **`Cmd-P` while the palette (or any other overlay) is open is a no-op** — it
   does not toggle, re-open, or clobber a sibling overlay, and the chord never
   leaks through as a typed `p`.
8. **Query is cleared on every open** — no sticky state.
9. **Mouse** — clicking a row activates it; hovering moves the highlight.
10. The **sidebar panel is unchanged** by all of this (`UXI-JumpPanel-1..8` hold);
    the palette works whether the panel is visible or hidden.

**Applies to.** Every screen (`YaldaView` / `EditView` / `BrowserView` /
`AgentView` / rail), including while typing in the agent compose or edit insert
mode — it is a global `Cmd` chord (`None` context), wired on every screen root
alongside `toggle_jump_panel`.

**Why.** The sidebar is a *browsing* navigator: it scales with the number of
workspaces and sessions, and reaching a specific one means finding it by eye and
clicking. A keystroke-addressable palette makes the same set reachable in O(a few
characters) without leaving the keyboard, and — because it projects from the same
source and dispatches through the same activators — it cannot drift from the panel
or grow a second, divergent notion of "jump".

**Status.** `implemented` — code home `jump_palette.rs` (`PaletteItem` /
`PaletteTarget` / `JumpPaletteOverlay`, the pure `fuzzy_score` +
`rank_palette_items`, `jump_palette_items`, `open_jump_palette_impl`,
`handle_jump_palette_key`, `activate_jump_palette_selection`,
`render_jump_palette`); `main.rs` holds `ActiveOverlay::JumpPalette`, the
`OpenJumpPalette` action + `overlay_is_jump_palette` render/capture branch;
`keymap_registry.rs` binds `cmd-p` (GLOBAL); `screens.rs`/`chrome.rs` wire
`open_jump_palette` on all 8 screen roots beside `toggle_jump_panel`. Exact
glyphs/colors are harness gap #1.

**Enforcement.** `verify_harness.rs`, all nine green, each observed RED under its
own reverted-fix mutation:

- `jump_palette_cmd_p_opens_over_any_screen` — real keymap + `simulate_keystrokes("cmd-p")`;
  also pins that a second `Cmd-P` neither toggles nor types a `p`.
  (NC: drop the modifier guard in the `Key::Char` arm → query becomes `"p"`.)
- `jump_palette_lists_workspaces_and_sessions_in_panel_order` — both kinds present,
  workspaces before sessions within a section.
- `jump_palette_ranks_best_match_first` — exact > prefix > scattered, non-matches
  dropped, empty query = panel order, word-start beats mid-word.
  (NC: remove the `sort_by` → order stays as listed.)
- `jump_palette_enter_jumps_to_top_match` — types `gam`, `Enter`, lands on `gamma`.
- `jump_palette_arrows_select_and_enter_activates_the_selection` — `Down` moves the
  highlight and navigates nowhere; `Enter` activates the highlighted row.
  (NC: activate `ranked[0]` instead of `ranked[selected]` → lands on `alpha`.)
- `jump_palette_no_match_enter_is_noop` — palette stays open, no navigation;
  backspacing back to a match re-ranks and re-highlights the top.
  (NC: close the overlay in the no-selection branch → palette gone.)
- `jump_palette_escape_closes_without_navigating`.
- `jump_palette_does_not_open_over_another_overlay`.
  (NC: drop the `has_overlay()` guard → the palette steals the slot.)
- `jump_palette_paints_over_the_screen` — layout probe `"jump-palette"`: absent while
  closed, non-zero box while open. (NC: render an empty `div()` in the branch →
  "the open palette did not paint".)

**Deviation from plan.** Three, all small:

1. **`detail` is not matched against.** The plan said "match against label only" and
   that shipped — but each row also *renders* a dim `detail` (owning project name, or
   the cwd for an unfiled session) purely to disambiguate same-named rows. You read
   it; you can't type against it.
2. **The ranked list is windowed to 12 rows** (`PALETTE_VISIBLE_ROWS`), scrolled to
   keep the highlight visible, rather than rendering an unbounded list. Wrapping
   `Up`/`Down` still traverses the whole ranked set.
3. **`JumpTarget` gained `Debug, PartialEq, Eq` derives** (`jump_panel_view.rs`) so
   `PaletteTarget` could be compared and printed in test failures — the only edit
   outside the palette's own files and the wiring.

### UXI-JumpPanel-12 — Status is known for EVERY session, not just the ones open here

**Statement.** The panel's live status (`UXI-JumpPanel-1`'s dot, `-10`'s word +
chip, `-6`'s waiting-on-you) applies to **every listed session**, including ones
this GUI has never opened — free sessions, jump-panel-created ones, and sessions
another GUI instance is driving.

The derivation is **local-then-server**:

1. session open in this GUI ⇒ its live `turn_phase` / `unread` win (unchanged);
2. otherwise ⇒ the server's `SessionInfo.busy` (a turn is in flight) drives
   **working**, and a **busy→idle** `SessionBusy` broadcast that arrives while you
   are not on that session raises a roster-side unread mark
   (`YaldaGpuiView.roster_unread`) ⇒ **your turn**, cleared when you jump to it.

The server owns `busy`: set when a prompt is accepted or queued, cleared when the
turn settles or the channel is (re)spawned. `SessionInfo.busy` is
`#[serde(default)]` and the broadcast is additive, so a GUI running against an
OLDER daemon degrades to "never busy" instead of failing to parse the session list.

**Applies to.** `session_proto.rs` (`SessionInfo.busy`, `Notification::SessionBusy`);
`yalda-session-server/main.rs` (`ManagedSession.busy`, `set_busy`/`broadcast_busy`,
`enqueue_prompt`, the `TurnCount` arm, `apply_channel_state`); `agent_roster.rs`
(`set_busy`); `agent_ui.rs` (the `SessionBusy` arm, `note_roster_turn_finished`,
`mark_roster_session_read`); `jump_panel_view.rs::jump_panel_agent_rows`.

**Why.** Before this, `awaiting`/`unread` were readable only off a live in-store
session, so most rows were structurally incapable of showing status — which reads
to the user as "the marks appear inconsistently" (bug-0022). A per-session event
subscription for every listed session is not an option (it would spawn replay for
sessions the user isn't using), so the server — which owns every channel — publishes
the one bit.

**Status.** `implemented` — **requires a running server built from this commit**;
an old daemon never sends `SessionBusy`.

**Enforcement.** `verify_harness.rs::roster_only_session_shows_live_status` — a
never-opened roster session goes Neutral → Working → WaitingForYou through the REAL
`apply_server_batch` reducer + the REAL row builder, and clears on read. **NC
observed RED**: drop the `.or(Some(info.busy))` fallback → `Some(Neutral)` while
working. The live server→GUI loop is verification gap 2.

### UXI-JumpPanel-10 — A live session says what it is doing, in words and in shape

**Statement.** In the jump panel, the two **live** session states are unmistakable
while scanning — each one carries a **status word**, its own **glyph shape**, and a
**tinted, outlined chip** around the row, not a hue on a single tiny glyph:

| State (`AgentDotStatus`) | Glyph | Right-edge word | Row |
|---|---|---|---|
| **Working** — a reply is in flight | `◆` (filled) | `working`, in the working hue | tint α 0.12 + hairline outline α 0.55, both in `agent.jump_working` |
| **Waiting for you** — a backgrounded turn finished, unread (`UXI-JumpPanel-6`) | `✦` | `your turn`, in the ready hue | tint + outline in `agent.tool_completed`, label **italic** |
| **Neutral** — idle+read, disconnected, or roster-only | `✦` (dim) | *(nothing)* | plain row |

Both live tints/outlines are **alpha-derived from the existing theme colors** — no
new theme fields, so a re-themed palette carries through. The **active row**
(`UXI-JumpPanel-5`, "you are here") keeps its own accent background — the you-are-
here mark wins over the status tint; the status outline still draws.

The **glyph shapes differ** so working vs waiting is legible with no color
perception at all, and italic still carries exactly one meaning
(`UXI-JumpPanel-6`): waiting on you.

The `(glyph, word)` mapping is the pure `agent_row_marks(status)`, and the **agent
tile reuses it** (`UXI-AgentTile-28`) so both surfaces speak one vocabulary.

**Applies to.** `jump_panel_view.rs`: `agent_row_marks` (new pure fn),
`jump_session_row_el` (chip + hinted row), `jump_nav_row_hinted` (`jump_nav_row`
plus a hint color, so a workspace's `ctrl-<n>` digit stays dim while a session's
status word takes the status hue).

**Why.** A colored `✦` was the *only* signal, at ~10px, in a list of otherwise
identical rows — too quiet to catch out of the corner of the eye, and useless if
you don't remember which hue means which. Words plus shapes plus a chip make "this
one is running" and "this one needs me" readable at a glance.

**Status.** `implemented` — the mapping is headless-guarded; the tint/outline/
italic as *pixels* are harness gap #1 (human eye).

**Enforcement.** `verify_harness.rs::agent_row_marks_name_the_live_states` (each
status' glyph + word, and that the two live glyphs differ; NC: return `("✦", None)`
for every status → RED). `dot_status` itself stays pinned by
`agent_dot_status_mapping` + `jump_panel_agent_dot_reflects_turn_phase`.

### UXI-JumpPanel-11 — The panel wears the command-menu surface (reverses -7's recessed shade)

**Statement.** The jump panel's background is **exactly the command menu's
surface** — `jump_panel_surface(editor_bg) == menu_panel_bg(editor_bg)`, the
elevated card the `?` / `.` / space menus are painted on: a fixed ΔL **lighter**
than the editor at the same hue + saturation. Its right border is the theme's
`overlay.border`.

This **reverses** two `UXI-JumpPanel-7` decisions:

1. the derived **recessed** shade (a ΔL *darken* of the editor bg, lighten-flipped
   on near-black themes) — gone; and
2. the per-theme art-direction hook `AgentTheme.jump_panel_bg` (Nightfox's
   `#0d1119`) — the **field is removed**, so panel shade is no longer theme-owned.
   The jump panel's *accent* colors (`jump_header` / `jump_subheader` /
   `jump_working`) stay theme-owned and unchanged.

`UXI-Menu-5`'s "the card goes the opposite direction from the recessed jump bar"
clause is retired with it — the two now share a surface **on purpose**.

**Applies to.** `jump_panel_view.rs`: `jump_panel_surface` (replaces
`jump_panel_bg`), `render_jump_panel` (bg + border). `theme.rs`:
`AgentTheme.jump_panel_bg` removed from the struct and all 8 themes.

**Why.** The recessed derivation read **muddy** on paper-toned themes (Folio) and
made the sidebar a *third* material next to the editor and the menus. One shared
chrome surface — sidebar, command menu, palette — reads cleaner, is lighter (the
ask), and needs no per-theme tuning.

**Status.** `implemented` — the surface derivation is headless-guarded; the shade
as *pixels* is harness gap #1.

**Enforcement.** `verify_harness.rs::jump_panel_surface_matches_the_command_menu`
(on a Folio-ish paper bg and a dark bg: panel == `menu_panel_bg`, lighter than the
editor, hue+saturation preserved; **NC observed RED**: restore the old
`editor.l - 0.035` recessed derivation → `left: L 0.905` vs `right: L 0.98`).
`menu_panel_bg_is_elevated_above_the_editor` keeps the menu card's own contract.

### UXI-JumpPanel-13 — Projects fold without losing their project menu

**Statement.** Every project header has a disclosure chevron. Folding hides all
workspace and agent-session rows belonging to that project; unfolding restores
them in the same order. The chevron is a distinct click target: clicking the
project name still opens its context menu, and dragging the name still reorders
the project section. Folded state persists by project name, because `ProjectId`
is runtime-local. The panel is 320px wide (100px wider than the former 220px
surface) so project/session labels and status words have room to breathe.

**Applies to.** `jump_panel_view.rs` (`JUMP_PANEL_WIDTH`,
`toggle_project_fold`, the split chevron/name header, and the folded render
gate), `main.rs` (`jump_folded_projects`), and `persist.rs`
(`Preferences::jump_folded_projects`).

**Status.** `implemented`.

**Enforcement.**
`verify_harness.rs::jump_panel_project_fold_hides_and_restores_children` paints
an expanded workspace row, folds its project through the real toggle, proves
the row is absent, then unfolds and proves it returns. Preference serialization
is covered by `tests.rs::preferences_round_trip_with_text_scale`.

### UXI-JumpPanel-14 — Every project has Waiting / Working / All agent tabs

**Statement.** Directly below each expanded project's workspace rows, the jump
panel renders three independently selectable agent tabs:

1. **Waiting** contains connected sessions whose last completed output is unread
   (`AgentDotStatus::WaitingForYou`).
2. **Working** contains connected sessions with a reply in flight
   (`AgentDotStatus::Working`).
3. **All** contains every session in the project and is the default.

Waiting and Working are chronological live queues: rows sort by when they
entered that state, oldest first and **most recent last**. Their order is not
draggable. State-entry time is owned beside the state itself: local sessions use
`TurnPhase::turn_started` / `AgentState::unread_since`; roster-only sessions use
`AgentRoster::state_since` / `roster_unread`.

All is the durable curated roster. It follows `jump_session_order`, exposes the
existing within-project drag reorder, and a newly discovered server sid is
appended to the order's bottom. A state transition never changes an All slot.
The first roster seed freezes the previous by-label default into the order; later
`SessionCreated` events only append.

**Applies to.** `jump_panel_view.rs` (`JumpAgentTab`,
`agent_rows_for_tab`, `jump_panel_sections`, `render_jump_panel`,
`append_new_jump_sessions`), `agent.rs` (`AgentState::unread_since`),
`agent_roster.rs` (`state_since`), `agent_ui.rs` (state-transition timestamp and
new-session append sites), and `yux/detail.rs` (`compact_tab`).

**Status.** `implemented` — filtering, chronology, independent project
selection, stable All order, append semantics, and the three tabs' painted
presence are headless-guarded. Exact colors remain harness gap #1.

**Enforcement.**
`verify_harness.rs::jump_agent_state_tabs_filter_and_sort_without_moving_all`
guards state filtering, oldest→newest order, and All identity.
`jump_project_agent_tabs_are_independent_and_all_appends` drives the real
per-project section projection and append method, probes all three painted tab
controls, and includes a state flip that must not move All rows.
