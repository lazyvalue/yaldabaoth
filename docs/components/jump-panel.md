# Component: Jump Panel

**Status:** living
**Component token:** `JumpPanel` (⇒ invariants are `UXI-JumpPanel-N`)

## Description

An always-visible root-level **navigator sidebar** (fixed `JUMP_PANEL_WIDTH`,
currently 320px),
laid out outside the workspace content so it stays put across workspace switches
(INV-JP1). Toggle: `toggle_jump_panel` (`cmd-j`). Rendered inline (cheap,
O(workspaces + tiles)), not cached. Primary code home:
`jump_panel_view.rs`.

**Palette.** Two header tiers, distinct hues: top-level section headers
("SYSTEM CONSOLE" / "WORKSPACES" / "UNBOUND") are **red** (`DetailStyle.err`,
`0xff6b6b`, bold uppercase); per-cwd subheaders are **electric blue** (`0x3b9eff`,
real path casing). Operational state uses two literal hues: **orange = working**
and **green = ready for input**. The "you are here" active mark and selected tabs
use the overlay's neutral gray selection palette (UXI-JumpPanel-5).

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

- **System console** — a single global row that summons the system-console
  overlay (`UXI-SystemConsole-1`). It replaces the former empty Pinned
  placeholder.
- **Workspaces** — one collapsible folder per durable workspace, containing one
  row per bound tile (`UXI-JumpPanel-23`).
  - The folder badge shows the **1-based workspace number** — the digit
    `ctrl-<n>` jumps to (INV-UX-11).
  - Folder click folds/unfolds. Bound-tile click selects its workspace and
    focuses that tile.
- **Unbound** — only tiles outside every workspace, retaining the existing
  activity tabs, tag folders, status, provider, ordering, and archive signals
  where the tile is an Agent.
  - **＋ New agent session** creates an unbound Agent tile and session.
  - **Status dot** = what the AGENT is doing (INV-UX-10, UXI-JumpPanel-6) — the
    shape + color are one signal, not binding:
    - **● orange** — working (a reply is in flight).
    - **● green** — connected and idle → **ready for input / your turn**.
    - **○ dim** — disconnected or connecting. The whole row is also dimmed.
  - Click → directly focuses the unbound tile without binding it.
  - The row bound to the **focused tile** carries a left accent bar
    (`UXI-JumpPanel-5`) — "this is where you are."

## References

- `docs/components/README.md` § Terminology — bound/unbound tile ownership and
  direct unbound focus.
- `docs/specs/spec-jump-panel.md` — deeper design doc.
- ADR-0033 — optional workspace ownership; supersedes ADR-0021's ephemeral
  virtual-workspace navigation.
- Migrated from `docs/ux-invariants.md` INV-UX-10, INV-UX-18. Those entries are
  now `→ migrated here`.

**Terminology migration.** Older implemented-invariant evidence below may name
“free sessions,” “bare views,” or ephemeral workspaces. Those are historical
descriptions of the code being replaced, not current product terms or behavior;
ADR-0033 and `UXI-JumpPanel-23` override them.

## UX invariants

### UXI-JumpPanel-1 — The jump-panel agent dot is a per-session status light

**Statement.** Each agent-session row in the jump panel carries a leading dot
whose **shape and color together** are one signal for what the agent is doing —
NOT binding (binding is no longer surfaced in the panel):

- **● orange** (`0xff9e64`) — **working**: a reply is in flight.
- **● green** (`theme.agent.tool_completed`) — **ready for input / your turn**:
  every connected session not currently producing a reply.
- **○ dim** — **unavailable**: disconnected or connecting (which also dims the
  whole row).

The mapping is a pure function of `(connected, awaiting)` —
`AgentRow::dot_status` → `AgentDotStatus::{Working, WaitingForYou, Neutral}` — so
the render just picks the glyph/hue. `unread` is retained as internal attention
state but never makes one Waiting row look different from another.

**Applies to.** `jump_panel_view.rs`: `jump_panel_agent_rows` (reads each opened
session's `state.turn_phase.is_awaiting()` into `AgentRow::awaiting` and
`state.unread` into `AgentRow::unread`) and `render_jump_panel` (glyph + color
from `dot_status`).

**Why.** The user wants to glance at the panel and see which agents are working,
which are ready for them and which are unavailable — without opening each tile.

**Status.** `implemented` — the mapping and palette routing are headless-guarded;
the actual alpha/compositing is a paint/human-eye detail (harness gap #1).

**Enforcement.** Headless in `verify_harness.rs`:
`agent_status_dot_reflects_turn_phase` (every connected idle state→
WaitingForYou, mid-turn→Working through the real `jump_panel_agent_rows`) and the
pure `agent_dot_status_mapping` unit test (totality and disconnected-wins).
`jump_panel_state_palette_is_orange_green_and_gray` guards palette routing.

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

4. **The durable custom slot NEVER moves except by a user drag.** The All tab
   presents those slots through Working / Waiting / Unavailable activity
   sections (`UXI-JumpPanel-14`), so a live state transition can move a row
   between sections, but never changes its relative custom rank within a
   section. Async roster refresh, `SessionCreated`/`SessionDeleted`, reconnect,
   and `/clear` likewise never rewrite an existing slot. (The dedicated Waiting
   and Working tabs intentionally use chronological state-entry order.)
   `/clear` is the load-bearing identity case: it
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
element** — "this is where you are" — with a 2px neutral left mark plus the
overlay's gray selected background. Label text remains normal foreground.
Selection is intentionally neutral so orange and green are reserved for
operational state. Two independent marks,
0/1/2 marks total:

1. **The active workspace row** — the row whose index equals
   `workspace.active_workspace` — is boxed while workspace content is active.
   During direct-unbound focus, **no workspace row is marked**. The mark is a
   neutral left bar over the gray selected background.
2. **The focused viewed-session row** — the agent-session row shown by the
   **focused tile** (`AgentTile::session()`) — is marked, including a detached
   direct visit (`UXI-JumpPanel-23`). When the focused tile
   is a **buffer**, or an **empty** Agent tile (selector, no session), there is
   **no session mark**. A roster-only / unopened session is never the focused
   session, so it is never marked. A disconnected-but-focused session **still** gets
   its mark (it means "active," orthogonal to the status-dot color / row dim).

There is a single focused tile, so at most one session mark and one workspace mark.
The System Console row, the "＋ New agent session" affordance, and section
headings never get a mark. The panel renders inline every frame, so the marks track focus
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
Option<Hsla>` param → left `border_l_2` neutral bar + gray selected background),
and `render_jump_panel` (workspace/session rows use `OverlayTheme::border` for the
mark and `OverlayTheme::selected_bg` for selection).

**Why.** With several workspaces and many agent sessions listed, the user loses
track of which one they're currently looking at. A clean accent mark on "where you
are" — workspace and, when applicable, the specific agent session — makes the
panel a map with a "you are here" pin. Neutral gray is reserved for selection so
orange and green retain their single operational meanings.

**Status.** `implemented`.

**Deviation from plan.** `jump_active_session()` does NOT call
`focused_bound_session()` (which `.expect`s a focused window) — the jump panel can
render with no focused window (the `workspace_number_ignores_direct_unbound_focus` path), so it
matches `workspace.focused_content()` directly and returns `(None, None)` when
absent, rather than panicking. The mark is a 2px left `border_l_2` neutral bar
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

### UXI-JumpPanel-6 — Unread is internal attention state, not a third visual state

**Statement.** When an agent session finishes a turn (its phase returns to
`Idle`) while it is **not** the focused tile's session, it is marked **unread**.
The mark clears when the session becomes focused/viewed. A turn that finishes
while you ARE focused never marks unread (you're reading it live).

Unread does not change jump-panel styling. Both read and unread idle sessions are
Waiting and wear the same green ready-for-input wash without a repeated status
word. This keeps the operational model honest: a row cannot appear in Waiting
while looking neutral or unavailable.

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
(`AgentRow::unread`, with `dot_status` deliberately independent of it).

**Why.** Unread remains useful for attention/accounting, but exposing it as a
separate visual treatment contradicted the Waiting tab and made the list noisy.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs`:
`jump_dot_unread_on_background_turn_end_read_on_focused` (drives the REAL
`apply_server_batch` → `ServerNotification::TurnEnded` on a backgrounded vs a
focused session; asserts their distinct unread booleans while both project to
`WaitingForYou`. `jump_session_rows_do_not_paint_redundant_status_words`
proves Waiting rows carry no redundant status hint.

_Note (UXI-JumpPanel-7): the status glyph is a **`✦`** whose color carries the
state (orange working / green ready / dim unavailable), not a separate ●/○ dot._

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
  neutral-dim (`dim`), and neutral overlay selection remain theme-derived.

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
2. **✦ New agent session** → `new_agent_session_in(pid)` (an unbound Agent tile
   and session rooted at the project's cwd).
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
   workspaces, then its All-tab Working / Waiting / Unavailable session
   partition, then the unfiled session groups), so `Cmd-P` → arrows → `Enter`
   is a keyboard navigator with no typing.
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
- `jump_all_tab_groups_activity_with_headers` — empty-query agent candidates
  mirror the All tab's Working / Waiting / Unavailable presentation order.
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

**Statement.** The panel's live status (`UXI-JumpPanel-1`'s glyph and `-10`'s
wash) applies to **every listed session**, including ones
this GUI has never opened — unbound Agent tiles, jump-panel-created ones, and sessions
another GUI instance is driving.

The derivation is **local-then-server**:

1. session open in this GUI ⇒ its live `turn_phase` wins;
2. otherwise ⇒ the server's `SessionInfo.busy` (a turn is in flight) drives
   **working**; connected + not busy is **ready for input**. A **busy→idle**
   broadcast may also raise the internal roster-side unread mark, cleared when
   you jump to it, without changing the visible ready state.

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
never-opened connected roster session goes WaitingForYou → Working →
WaitingForYou through the REAL `apply_server_batch` reducer + row builder.
Reading clears its internal unread mark without changing its visible readiness.
The live server→GUI loop is verification gap 2.

### UXI-JumpPanel-10 — A live session shows state without redundant row text

**Statement.** In the jump panel, the two connected session states are
unmistakable while scanning, but visually quiet: each carries its own glyph
shape and a low-alpha background wash with no bounding box. The tabs and All-tab
section headers already name the state, so individual session rows never repeat
the words `working` or `your turn`:

| State (`AgentDotStatus`) | Glyph | Row |
|---|---|---|
| **Working** — a reply is in flight | `◆` (filled, orange) | orange wash α 0.07 |
| **Ready for input** — connected and not working | `✦` (green) | green wash α 0.08 |
| **Unavailable** — disconnected/connecting | `✦` (dim) | plain dim row |

Both state washes are alpha-derived from existing theme colors; there are no
outlines or rounded chips. The active row (`UXI-JumpPanel-5`, "you are here")
uses the overlay's gray selected background, which wins over the status wash.

The **glyph shapes differ** so working vs waiting is legible with no color
perception at all.

`agent_row_marks(status)` remains the shared glyph/word vocabulary used by the
agent tile (`UXI-AgentTile-28`), but the Jump Panel consumes only its glyph. The
tile's own status pill is outside this invariant and remains unchanged.

**Applies to.** `jump_panel_view.rs`: `agent_row_marks` (new pure fn),
`jump_session_row_el` (glyph + wash, no status hint), and `jump_nav_row`
(workspace accelerator hints remain supported).

**Why.** A colored `✦` alone was too quiet to catch out of the corner of the eye.
Distinct shapes and a restrained wash keep state scannable, while the surrounding
tabs and headers supply the words once instead of repeating them on every row.

**Status.** `implemented` — both live-state rows are paint-guarded against
redundant status words while glyph distinction and palette routing remain
guarded. The alpha/compositing as pixels remains harness gap #1 (human eye).

**Enforcement.**
`verify_harness.rs::jump_session_rows_do_not_paint_redundant_status_words`
drives real Waiting and Working rows and proves neither paints a right-edge
status-word element. Glyph distinction stays pinned by
`agent_row_marks_name_the_live_states`; `dot_status` itself stays pinned by
`agent_dot_status_mapping` + `agent_status_dot_reflects_turn_phase`.

**Negative control observed.** With the paint probe added before removing the
row hints, `jump_session_rows_do_not_paint_redundant_status_words` failed on the
real Working row because its `working` element painted. Passing no session hint
returned the guard to green.

**Deviation from plan.** None material. `agent_row_marks` still supplies the
Agent Tile's status-pill word, while `jump_session_row_el` intentionally consumes
only its glyph. This keeps the request scoped to the Jump Panel.

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
surface) so project/session labels, tab counts, and summaries have room to
breathe.

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

### UXI-JumpPanel-14 — Every project has Waiting / Working / All ordinary agent tabs

> **Panel-superseded in part by `UXI-JumpPanel-20`.** In the sidebar, the **All**
> tab no longer paints the Working/Waiting/Unavailable activity partition described
> in clause 3 — sessions there group under **tag folders** and sort by label. The
> activity partition (`agent_row_groups_for_tab`) is retained and still drives the
> **`Cmd-P`** empty-query order (`UXI-JumpPanel-9`), which is unchanged. Waiting and
> Working tabs are otherwise as below (now with tag folders layered in).

**Statement.** Directly below each expanded project's workspace rows, the jump
panel renders three independently selectable ordinary agent tabs (the
orthogonal fourth Archived tab is specified by `UXI-JumpPanel-16`):

1. **Waiting** contains every connected session that is not currently producing
   a reply (`AgentActivity::Waiting`). Every row is consistently green, with no
   repeated status word, whether its latest output is read or unread.
2. **Working** contains connected sessions with a reply in flight
   (`AgentDotStatus::Working`).
3. **All** contains every non-archived session in the project and is the
   default. It is visually partitioned into headed activity sections:
   **Working** first, **Waiting** second, then a subdued **Unavailable** section
   only when disconnected or connecting sessions exist. Empty sections are
   omitted.

Waiting and Working are chronological live queues: rows sort by when they
entered that state, oldest first and **most recent last**. Their order is not
draggable. State-entry time is owned beside the operational state itself: local
sessions use `TurnPhase::turn_started` / `AgentState::waiting_since`;
roster-only sessions use `AgentRoster::state_since`. Unread remains internal
attention state, not an activity or visual state.

Selecting, focusing, attaching, or otherwise **viewing** a session is not a
state transition and cannot change this timestamp or its queue position. When a
roster-backed row becomes locally attached, its roster activity timestamp
remains authoritative; the act of constructing a local view must not replace it
with the new `AgentState` construction time. A Waiting row moves to the bottom
only after it actually enters Working and later re-enters Waiting.

A connected agent therefore always belongs to exactly one of Waiting or
Working. Disconnected/connecting sessions are `AgentActivity::Unavailable` and
remain visible in All under the conditional exceptional section; Unavailable is
not a selectable tab.

All is the durable curated roster. Its sections are a stable partition of
`jump_session_order`: custom order is preserved within each section, while
Working always precedes Waiting and Unavailable. It exposes the existing
within-project drag reorder, and a newly discovered server sid is appended to
the order's bottom. A state transition can move a row to another section but
never changes its durable slot. The first roster seed freezes the previous
by-label default into the order; later `SessionCreated` events only append.

**Applies to.** `jump_panel_view.rs` (`JumpAgentTab`,
`agent_rows_for_tab`, `agent_row_groups_for_tab`, `jump_panel_sections`,
`render_jump_panel`,
`append_new_jump_sessions`), `agent.rs` (`AgentState::waiting_since`),
`agent_roster.rs` (`state_since`), `agent_ui.rs` (state-transition timestamp and
new-session append sites), and `yux/detail.rs` (`compact_tab`,
`compact_list_group_heading`).

**Status.** `implemented` — filtering, chronology, view/attach timestamp
continuity, independent project selection, stable durable order, append
semantics, painted tabs, and the headed All partition are headless-guarded.
Exact heading density and contrast remain harness gap #1.

**Enforcement.**
`verify_harness.rs::jump_agent_state_tabs_filter_and_sort_without_moving_all`
guards state filtering, oldest→newest order, and All identity.
The view/attach continuity clause is guarded through the real
`jump_to_agent` roster attach path by
`viewing_a_waiting_agent_does_not_change_waiting_order`.
`jump_project_agent_tabs_are_independent_and_all_appends` drives the real
per-project section projection and append method, probes all four painted tab
controls, and includes a state flip that must not rewrite the All projection's
durable order. `jump_session_rows_do_not_paint_redundant_status_words` proves
Waiting and Working rows do not repeat the surrounding state labels.
`jump_all_tab_groups_activity_with_headers` drives the real All render and
proves nonempty headings and rows paint in Working → Waiting → Unavailable
order, rows retain their durable relative order within each group, and empty
groups paint no heading.

**View/attach reconciliation (2026-07-28).** The initial wording suggested a
binary local-versus-roster timestamp owner. The shipped projection is
phase-aware: while local and roster activity agree, the roster timestamp is
authoritative and survives view attachment; while they disagree because the
local reducer has started a real transition ahead of `SessionBusy`, the local
transition timestamp leads until the roster catches up. This preserves queue
identity without making status chronology lag behind real local activity.

**Negative control observed.** With the guard added before the renderer changed,
`jump_all_tab_groups_activity_with_headers` failed specifically because the
nonempty Working heading was absent. Adding the stable partition and its real
heading render returned it to green.

**Deviation from plan.** None material. The repeated glyph / label / count /
hairline shape was promoted to `yux::compact_list_group_heading`. The underlying
All projection remains in durable custom order for persistence; its headed
presentation order is also applied to empty-query `Cmd-P`, preserving
`UXI-JumpPanel-9`'s existing panel-order promise.

### UXI-JumpPanel-15 — Agent tabs are a separated neutral two-row control

**Statement.** Waiting / Working / All / Archived render as a single bounded
2×2 segmented control, not four floating words or one crowded strip:

- one rounded neutral hairline encloses the group;
- **Waiting / Working** occupy the first row and **All / Archived** occupy the
  second row;
- internal hairlines separate its four equal-width targets;
- the selected segment uses the overlay's gray selected background;
- every label uses normal foreground contrast;
- 10px of breathing room separates the workspace rows from the control;
- agent summaries use `AgentTheme::agent_tint`, never `warm_accent` or the
  structural low-contrast `dim` color.

The everyday themes are explicitly art-directed: Folio supporting text is deep
steel (`#2d3d4e`), never gold/tan; Nightfox is pale blue (`#9abed0`), never its
nearly-background `dim` blue.

**Applies to.** `jump_panel_view.rs` (`render_jump_panel`,
`jump_supporting_text_color`, `jump_session_row_el`), and `yux/detail.rs`
(`compact_tab`).

**Status.** `implemented` — the two-row geometry, aligned columns, containment,
and palette selection are headless-guarded. Exact antialiasing and subjective
visual balance remain harness gap #1.

**Enforcement.** `jump_agent_tabs_paint_as_two_by_two_grid` probes the
enclosing control and all four painted tab targets, proving Waiting / Working
share the first row, All / Archived share a lower second row, and every target
is inside the control.
`tests.rs::jump_supporting_text_is_cool_and_readable_in_everyday_themes`
locks the Folio/Nightfox supporting colors and rejects their warm accents.
`jump_panel_state_palette_is_orange_green_and_gray` guards neutral selection.

**Negative control observed.** Against the unchanged one-row renderer,
`jump_agent_tabs_paint_as_two_by_two_grid` failed specifically because
All / Archived did not paint below Waiting / Working. Reflowing the enclosing
control into two aligned rows returned it to green.

**Deviation from plan.** None material. The existing `compact_tab` primitive
remains the tab target; the Jump Panel owns the requested 2×2 group layout and
its internal vertical and horizontal hairlines.

### UXI-JumpPanel-16 — Archived sessions enter durable cold storage

**Statement.** Archiving is a durable server-owned lifecycle state on a
server-backed agent session, not a third operational activity:

- an archived session is absent from Waiting, Working, All, and the `Cmd-P`
  jump palette;
- it appears only in the project's fourth **Archived** tab;
- the Archived tab preserves the same durable `jump_session_order` used by All;
- archiving, unarchiving, and live activity changes never move that durable
  slot, so unarchiving restores the row to All and to whichever one of Waiting
  or Working currently applies;
- archiving immediately unbinds every workspace tile showing that session and
  leaves each tile on its ordinary live session picker;
- after fsyncing the archive marker, the session server cancels any in-flight
  turn, fences and drops the ACP pump/transport, clears queued prompts and its
  live forwarder, and closes the WAL file descriptor. The WAL file and rebuilt
  in-memory transcript remain intact;
- server restart recovers archived metadata and transcript without spawning an
  ACP adapter or opening that session's WAL for append;
- unarchiving reopens the same WAL, fsyncs the live marker, and resumes the last
  ACP session id. Prompt, restart, and model-change requests are rejected while
  the session remains archived;
- selecting the session explicitly from the **Archived** jump-panel tab opens
  the preserved transcript read-only in a bare ephemeral agent view. This
  direct visit does not unarchive it or add it to any tile picker;
- the active agent's `<space>` menu offers exactly one contextual action:
  **archive session** or **unarchive session**;
- right-clicking a jump-panel session row opens a cursor-anchored context menu
  with that same contextual action. `Esc` or click-away dismisses it.

The state is keyed by stable server sid and persisted in that session's WAL.
`Preferences::jump_archived_sessions` remains a GUI projection/order aid; at
upgrade, its legacy GUI-only flags are migrated into server state before the
first roster snapshot is adopted. A session that does not yet have a sid cannot
be archived; its local menu action is disabled and it has no archivable
jump-row identity. `/clear` succession migrates the projected flag from the
predecessor sid to the replacement sid alongside the existing durable
order-slot migration.

**Applies to.** `jump_panel_view.rs` (`JumpAgentTab`, session projections,
session-row right click, archive request and acknowledged local projection),
`jump_palette.rs` (candidate projection), `main.rs` (projected archive set,
local-menu command, session context-menu overlay), `agent_ui.rs` (roster
migration, `SessionArchived` reduction, `show_pickers_for_session`, direct
`jump_to_session`, `/clear` identity succession), `session_proto.rs`,
`session_client.rs`, `session_wal.rs`, and `yalda-session-server/main.rs`.

**Why.** Long-lived session histories need to leave everyday navigation without
being deleted or losing their curated position, but hidden sessions must not
retain a subprocess and file descriptor indefinitely. Keeping archive
orthogonal to activity prevents it from becoming a misleading replacement for
Waiting or Working while giving it an explicit resource boundary.

**Status.** `implemented` — server-owned cold archive, durable migration,
resource release, filtering, immediate tile-to-picker transition, read-only
transcript revisit, both command surfaces, and painted-row interaction are
guarded. Exact visual balance of the four-tab strip and cursor-anchored popup
remains harness gap #1.

**Enforcement.**
`verify_harness.rs::jump_session_archive_filters_tabs_palette_and_persists`
drives all four real projections, proves `Cmd-P` remains the non-archived All
projection even while Archived is selected, and round-trips the preference
snapshot.
`verify_harness.rs::jump_session_archive_controls_toggle_the_same_durable_flag`
drives the dynamic `<space>` menu command plus actual right-click and click
events against the painted session row and context-menu item. It also proves a
sid-less local session cannot be archived.
`archive_unbinds_tiles_but_direct_jump_reopens_the_transcript` drives the shared
archive mutator on a real bound session, proves its tile immediately becomes a
picker without dropping the session or transcript, then drives the real local
jump-panel dispatch and proves the transcript opens normally in an ephemeral
view.
`clear_keeps_the_sessions_jump_panel_slot` proves `/clear` migrates both the
durable order slot and archive identity.

**Negative control observed.** Temporarily removing the All projection's archive
filter made `jump_session_archive_filters_tabs_palette_and_persists` fail with
the archived sid present beside the live sid. Restoring the filter returned the
guard to green.

**Deviation from plan.** None material. `Cmd-P` explicitly requests the All
projection rather than inheriting the currently visible per-project tab; this
is the necessary expression of the requirement that the palette remain ordinary
navigation and never expose Archived.

### UXI-JumpPanel-17 — Waiting and Working tabs show their live session totals

**Statement.** Every expanded project's Waiting and Working tab carries an
always-visible number indicator, including when the total is **0**. Each number
is derived from that project's current, deduplicated session projection:

- Waiting counts non-archived rows whose activity is
  `AgentActivity::Waiting`;
- Working counts non-archived rows whose activity is
  `AgentActivity::Working`.

Unavailable and archived rows contribute to neither total. Archive/unarchive
and live activity transitions update the numbers on the next projection. All
and Archived stay unnumbered. The Waiting indicator uses the existing semantic
green, the Working indicator uses the existing semantic orange, and the
selected tab retains the neutral gray treatment from `UXI-JumpPanel-15`.

**Applies to.** `jump_panel_view.rs` (`JumpProjectSection`,
`jump_panel_sections_with_tab`, `render_jump_panel`) and `yux/detail.rs`
(`compact_tab`, `compact_count_indicator`).

**Why.** The filtered tabs need to communicate queue size before selection,
without adding another row of chrome or allowing a stale independently-managed
counter to drift from the sessions the tab actually contains.

**Status.** `implemented` — the indicators derive from the same project
projection as their tabs and their geometry is headless-guarded. Exact
antialiasing and subjective balance in each theme remain harness gap #1.

**Enforcement.**
`verify_harness.rs::jump_waiting_working_tabs_paint_live_counts` drives the real
project projection, proves archived and unavailable sessions are excluded,
observes a Working→Waiting transition, and layout-probes both indicators inside
their tab targets to ensure the zero indicator remains painted.

**Negative control observed.** With the guard added before the renderer changed,
it failed specifically because the Waiting count indicator did not paint.
Adding the derived totals and compact indicators returned the guard to green.

**Deviation from plan.** None material. The visual number shape was promoted to
the reusable `yux::compact_count_indicator` primitive, while `compact_tab`
learned to accept optional inline content so equal-width tab geometry remains
centralized.

### UXI-JumpPanel-18 — Archiving a session announces itself

**Statement.** Toggling a session's durable archive flag is an observable event,
not a silent state edit:

- **System console.** Every archive and every unarchive writes one
  `ConsoleLevel::Info` line naming the agent — `archived agent session "<label>"`
  / `unarchived agent session "<label>"`. The label is the session's live store
  name when this GUI has it open, otherwise the roster's name for that sid.
- **Agent transcript.** When the session is open in this GUI, the same event
  appends a yalda-local `TurnId::System` notice — `session archived` /
  `session unarchived` — to that session's transcript. It carries no turn
  number, emits no `TurnHeader`, and is excluded from agent-turn numbering
  exactly like every other lifecycle notice.
- **Roster-only sessions.** A session this GUI has not opened has no in-memory
  transcript, so it gets the console line only. This is a deliberate scope
  boundary: the notice is a local view event, not durable server transcript.
- **No-op toggles are silent.** Archiving an already-archived session (or
  unarchiving an unarchived one) changes nothing and therefore announces
  nothing. A sid-less local session still cannot be archived at all
  (`UXI-JumpPanel-16`).

Both command surfaces from `UXI-JumpPanel-16` — the agent tile's `<space>`
**archive session** / **unarchive session** command and the jump-panel session
row's right-click context menu — announce identically, because both already
route through the single durable-flag mutator.

**Applies to.** `jump_panel_view.rs::set_session_archived` (the one choke point)
and its `announce_session_archived` helper, `agent_ui.rs::append_system_notice`
(the `TurnId::System` transcript lane), and
`system_console.rs::append_system_console`.

**Why.** Archive is the one session command whose entire visible effect is a row
disappearing from the lists you were looking at. Without an announcement it is
indistinguishable from a session vanishing for some other reason, and there is
no record of who left when. Naming the agent in the console line makes the
console a usable audit trail; the transcript notice puts the event where the
session's own history lives.

**Status.** `implemented` — both announcements, the roster-only console-only
case, and the silent no-op toggle are headless-guarded. Nothing here is a
runtime gap: the console line is asserted from its persisted file and the
transcript notice from real editor metadata.

**Enforcement.**
`verify_harness.rs::archive_toggle_announces_in_console_and_transcript` drives
the real `set_session_archived` — the single mutator both the `<space>` command
and the row context menu route through — against an open session (`S1`, bound
via `install_agent_slot`) and a roster-only session (`S2`). It asserts the
`INFO\tarchived agent session "<label>"` line in the console log under a
tempdir override, the `session archived` transcript tail with an actual
`TurnId::System` tag read from `editor.metadata::<TurnId>()`, that a repeated
archive writes neither a console line nor a transcript line, that archiving
`S2` leaves `S1`'s transcript untouched, and that unarchive announces
symmetrically.

**Negative control observed.** Commenting out the `announce_session_archived`
call in `set_session_archived` failed the guard at the first assertion with an
empty console log. Restoring the call returned it to green.

**Deviation from plan.** One: the console line is written through
`YaldaGpuiView::append_system_console` rather than the free
`record_system_message`. The free function only appends to the persisted file —
it exists for lifecycle messages recorded *before* GPUI builds the console view
— so using it would have left an already-open console overlay showing nothing
until the next launch. `append_system_console` pushes into the live
`SystemConsoleView` and persists, which is what "write a log message to the
system console" actually requires.

### UXI-JumpPanel-19 — SUPERSEDED: direct session visits use ephemeral views

**Superseded by `UXI-JumpPanel-23` / ADR-0033.** Direct navigation now focuses
an existing unbound tile; it neither duplicates the tile nor fabricates an
ephemeral workspace. Historical behavior remains below until the code is
replaced.

**Statement.** Activating an agent session directly from either the jump panel
or `Cmd-P` opens it in a bare ephemeral agent view, even when that session is
already referenced by a tile in a real workspace. The direct tile is an ordinary
second viewport reference to the same project session: it never moves, unbinds,
or duplicates the session's durable workspace placement. Leaving the bare view
tears down only that reference; returning to the workspace finds the session in
its original tile. A free session stays free while viewed directly because the
ephemeral reference is not placement.

Session identity, ACP transport, transcript, and reducer state are owned once by
the project/session domain (`AgentSessions` is the normalized runtime store).
Workspaces own their tiles, and tiles hold only `SessionId` references. Whether a
reference is durable is derived from its containing `Workspace::ephemeral` flag;
there is no second "detached tile" state to keep synchronized.

**Applies to.** `agent.rs::AgentTile::{Bound,session}`;
`agent_ui.rs::{jump_to_agent,jump_to_session,agent_tile_id_bound_to,
bound_sid_set,save_agent_ring}`; `workspace.rs::Workspace::ephemeral`; and
`jump_palette.rs::activate_jump_palette_selection`, which deliberately shares
the jump-panel dispatcher.

**Why.** Choosing a session directly means "show me the session," not "take me
to whichever workspace currently contains it." Workspace placement is durable
spatial context and must remain present while the user makes a temporary direct
visit.

**Status.** `implemented` (headless).

**Enforcement.** `verify_harness.rs::bound_session_jumps_focus_single_owner_workspace`
drives the shared jump-panel dispatcher and the real `Cmd-P` key/activation path
against a session already bound in a real workspace. It proves both entries open
an ephemeral tile holding the same `SessionId`, the original durable placement
remains unique, and switch-away reveals it unchanged. The free-session guard also
proves an ephemeral reference does not count as placement. Negative control:
restoring the former focus-existing-workspace branch fails on the missing
ephemeral view.

### UXI-JumpPanel-20 — SUPERSEDED: sessions group under tag folders

**Superseded by `UXI-JumpPanel-23` / ADR-0033.** The folder behavior survives,
but groups unbound **tiles** by tile-local tags rather than grouping all
sessions by session-sidecar metadata.

**Statement.** A session carries a set of user-assigned **tags** (`UXI-AgentTile-33`),
keyed by server sid in the id-keyed `session_tags.json` sidecar. Within a project's
agent-session tabs the jump panel groups sessions under **tag "folders"** —
collapsible headers, one per tag — nested a level below the Waiting / Working / All
tabs (`UXI-JumpPanel-14`):

1. **Tags are project-scoped.** A session belongs to one project (its cwd), so its
   tags only ever group it among that project's sessions. Two projects that both
   have a tag named `urgent` are independent folders with independent order.
2. **Only non-empty folders show, per category.** In the **Waiting** tab a tag
   folder appears only if it holds ≥1 session currently Waiting; likewise Working.
   In **All** a folder appears if it holds ≥1 non-archived session. A tag with no
   session in the active tab does not render there.
3. **A session appears once per tag it carries.** A session tagged `alpha` and
   `urgent` renders under BOTH folders (multi-appearance). Its per-appearance
   element ids are disambiguated by a folder-ordinal suffix so GPUI ids never
   collide.
4. **Untagged sessions are flat rows**, rendered **below** the folders within the
   tab — never inside an "Untagged" folder, never duplicated at a folder level.
5. **Waiting / Working keep their chronological order** within a folder (oldest
   first, `UXI-JumpPanel-14`). **All drops the Working/Waiting/Unavailable activity
   sub-headers entirely and sorts** — folders in the user's manual tag order
   (`UXI-JumpPanel-21`), and sessions **alphabetically by label** within each folder
   and within the untagged residual. This **supersedes** `UXI-JumpPanel-14`'s All
   activity partition and the within-All session drag-order presentation
   (`UXI-JumpPanel-2` clause 2): the manual axis in the tagged view is the tag
   folder, not the individual session, so session rows no longer drag.
6. **Archived is unchanged** — a flat list, no tag folders.
7. **The nesting is visible.** A tag folder header is a `🏷 tag` in the grouping
   blue + a quiet count + a trailing hairline rule (so it reads as a header, not a
   row). Its session rows are wrapped in an **indented container with a left guide
   line**, so they clearly read as children *of* the tag. When both folders and
   untagged rows exist, a **labeled `untagged` hairline separator** divides the
   last folder from the loose rows below.

**Applies to.** `jump_panel_view.rs`: `AgentRow.tags` (populated in
`jump_panel_agent_rows` from `self.session_tags` by sid), the pure
`partition_rows_by_tag(rows, tag_order) -> (folders, untagged)`, `render_jump_panel`
(the per-tab folder/untagged render replacing the `agent_row_groups_for_tab` call
for non-Archived tabs; All sorts its rows by label first), and `jump_session_row_el`
(new `id_suffix` param). `agent_roster.rs`: the `session_tags.json` sidecar
(`session_tags_path`, `load_session_tags`, `save_session_tags`, `add_session_tag`,
`remove_session_tag`). `main.rs`: `self.session_tags` load/hydrate.

**Why.** Grouping by cwd/project alone doesn't match how the user thinks about
their agents; a tag folder ("frontend", "urgent") is manual curation, and letting a
session sit in several folders lets one worker be both "urgent" and "frontend"
without choosing.

**Status.** `implemented`.

**Deviation from plan.** Two. (1) `agent_row_groups_for_tab` (the All activity
partition) is **retained, not removed** — `jump_palette.rs` still uses it so the
`Cmd-P` empty-query order keeps the Working/Waiting/Unavailable presentation
(`UXI-JumpPanel-9` unchanged); only the sidebar's All *render* dropped the activity
headings. So the supersession of `UXI-JumpPanel-14`'s All partition is **panel-only**.
(2) Untagged rows render **below** the folders (chosen placement); Waiting/Working
keep their chronological within-folder order, only All sorts by label.

**Enforcement.** `verify_harness.rs`:
`session_tags_partition_folders_and_untagged` (pure: multi-appearance, untagged
residual, folder order = tag_order then alpha; **built-in NC**: empty order = alpha,
the opposite folder order) and `jump_panel_groups_sessions_under_tag_folders`
(drives the REAL section projection + render: a tagged session paints under its
folder header with the `-tg0` id suffix, an untagged one paints flat below, and the
`untagged` separator paints **between** them and vanishes when no folders exist;
`jump_all_tab_groups_activity_with_headers` proves All paints **no** activity
heading and sorts untagged rows by label. **NCs observed RED**: forcing the partition
to treat every row as untagged → the folder header never paints; disabling the
separator render → the "separator paints" assert fires). The literal
folder glyph/indent is harness gap #1.

### UXI-JumpPanel-21 — SUPERSEDED: session-tag folders reorder and fold

**Superseded by `UXI-JumpPanel-23` / ADR-0033.** Order and fold persistence
carry forward for the Unbound tile tag folders.

**Statement.** Tag folders are user-curated like the project sections
(`UXI-JumpPanel-2`, `-13`):

1. **Drag to reorder.** Dragging a tag-folder header onto another reorders the
   folders **within that project**, persisted in
   `Preferences::jump_tag_order` (a `project name → [tag]` map). The order is applied
   as a **stable** sort by rank in the list, so an absent/empty order is a total
   no-op (folders stay alphabetical) and a newly-seen tag sorts after the listed
   ones. The order is keyed by durable **project name** (ProjectId is runtime-local),
   the same key `UXI-JumpPanel-13` folds by.
2. **Fold to collapse.** A tag folder's chevron toggles its collapse; folded hides
   its session rows and unfolding restores them. Folded state persists per
   project+tag in `Preferences::jump_folded_tags` (composite `"{project}\u{1f}{tag}"`
   keys).
3. **A tag folder never crosses projects.** A folder drag is scoped to its project
   (the `TagDrag` payload carries the project name; the reorder re-checks it).

**Applies to.** `jump_panel_view.rs`: `TagDrag` payload, `partition_rows_by_tag`
(consumes `jump_tag_order[project]`), `reorder_tag`, `toggle_tag_fold`, the folder
header `on_drag`/`drag_over`/`on_drop` + chevron `on_click` in `render_jump_panel`.
`main.rs`: `self.jump_tag_order` (`HashMap<String, Vec<String>>`),
`self.jump_folded_tags` (`HashSet<String>`), hydrated on boot and written by
`save_settings`. `persist.rs`: `Preferences::{jump_tag_order, jump_folded_tags}`.

**Why.** Alphabetical folders aren't how the user prioritizes; dragging the active
tag to the top and remembering it is. Folding keeps a large project's panel
scannable. Both are per-project because tags are project-scoped
(`UXI-JumpPanel-20`).

**Status.** `implemented`.

**Deviation from plan.** The project-scope guard is expressed as "both the dragged
and target tag must be present in the project" (a tag absent from the project — a
foreign-project or ghost tag — is refused), rather than a separate payload re-check;
the `TagDrag` payload still carries the project name for the `can_drop` gesture gate.

**Enforcement.** `verify_harness.rs`:
`jump_reorder_tag_folders_persists` (drives the REAL `reorder_tag`: folders reorder,
`jump_tag_order[project]` is written, a ghost/foreign tag is refused; **NC observed
RED**: early-return the reorder → "the manual order is stored per project" fires) and
`jump_tag_folder_fold_hides_and_restores` (REAL `toggle_tag_fold` + render probe: the
folded folder's row is absent while its header stays, unfolding returns the row;
**NC observed RED**: drop the `!folded` render gate → "folded folder hides its row"
fires). Preference round-trip is covered by
`tests.rs::preferences_round_trip_with_text_scale` (the `jump_tag_order` +
`jump_folded_tags` fields). The GPUI mouse-drag gesture itself is harness gap #2
(no headless drag-dispatch seam).

### UXI-JumpPanel-22 — Every session row identifies its owning agent provider

**Statement.** Every agent-session row carries a compact, fixed-width provider
mark at its right edge: **`✳` for Claude** and **`⌬` for Codex**. The two shapes
are deliberately distinct because both provider names begin with `C`; a single
initial would be ambiguous. The mark uses the panel's cool supporting-text color
and never an operational state hue.

Provider identity and operational state remain independent signals. The existing
leading `◆` / `✦` status mark and orange / green / dim state treatment continue
to answer *what the agent is doing* (`UXI-JumpPanel-10`); the trailing provider
mark answers *which agent owns the ACP session*. Active-row selection, archive
membership, tag folders, ordering, and click/drag behavior are unchanged.

The mark is present for every row source. Roster-backed rows use
`SessionInfo::provider`, which is server-authoritative. Local-only rows use the
opened `AgentState::provider`, including the mid-create window before the roster
catches up. A row never infers provider identity from its editable label.

**Applies to.** `jump_panel_view.rs`: `AgentRow::provider`,
`jump_panel_agent_rows` (roster and local-only projections), the pure
`agent_provider_mark`, and `jump_session_row_el` (right-edge painted mark).

**Why.** Mixed Claude and Codex rosters can contain renamed sessions whose labels
carry no provider clue. The jump panel is the universal session navigator, so it
must expose the durable provider identity without requiring the user to open the
conversation and inspect a turn header.

**Status.** `implemented` — provider projection and real-row paint are headless-
guarded. Exact glyph rasterization and subjective visual balance remain harness
gap #1.

**Enforcement.** A mixed-provider `verify_harness.rs` guard must drive the real
`jump_panel_agent_rows` projection for roster-backed Claude and Codex sessions,
cover a local-only provider row, and use layout probes to prove that the matching
provider marks paint on the real jump-panel rows while the leading status marks
remain intact. Exact glyph rasterization and subjective visual balance are
harness gap #1.

**Negative control observed.** With provider identity present in `AgentRow` but
before `jump_session_row_el` painted it, the mixed-provider guard failed on the
first roster row: `alpha claude must paint its Claude ownership mark`. Adding
the trailing mark returned the guard to green.

### UXI-JumpPanel-23 — Workspaces are tile folders; Unbound is the out-of-workspace list

**Statement.** The jump panel and Cmd-P project the frame's exclusive tile
ownership:

1. Every workspace is an independently collapsible folder. Its children are
   exactly its bound tiles, in workspace reading order.
2. **Unbound** contains exactly the frame's unbound tiles. No bound tile appears
   there, and no unbound tile appears under a workspace.
3. Unbound tiles are organized by their tile tags using the existing
   project-scoped tag folder order and fold behavior. Untagged tiles remain flat.
4. Selecting a bound tile selects its workspace and focuses it. Selecting an
   unbound tile directly focuses it and leaves it unbound.
5. Cmd-P (“Jump to…”) flattens the same destinations and dispatches the same
   bound/unbound activation paths. The period shell menu is not a tile picker.
6. Agent tiles retain the existing activity, provider, summary, archive, and
   ordering signals. A roster session without a bound tile materializes as one
   unbound Agent tile instead of a session-only navigation row.
7. Workspace-folder folds persist independently by stable workspace identity;
   tile membership changes update both projections immediately.
8. Workspace folder headers use the same base typography as ordinary jump
   navigation rows; folder hierarchy must not enlarge the workspace label.

**Applies to.** `jump_panel_view.rs`: project/workspace/tile view models and
renderer; `jump_palette.rs`: flattened entries and activation;
`agent_ui.rs`: roster-to-unbound materialization; `persist.rs`: folder folds
and tile ownership.

**Why.** The navigation tree should reveal where durable tile state lives.
Workspace folders make placement visible, while Unbound is the one predictable
place for state that is not currently laid out.

**Status.** `implemented (headless)`.

**Enforcement.** `jump_panel_workspace_folders_and_unbound_rows_are_tile_native`
drives production paint and click targets and proves exclusivity, folding, tag
grouping, direct focus, and Agent metadata. Cmd-P has a real keystroke/activation
guard. Inverting the Unbound project predicate was observed RED; 15 targeted
ownership/Cmd-P/jump-panel mutants were caught (Cog graph `9k2`).

### UXI-JumpPanel-24 — Tagged navigation keeps fixed chrome typography

**Statement.** Tag folders and the tile rows nested beneath them use explicit,
fixed jump-panel typography. A tag-folder header uses the panel's compact
monospace subheader size; tagged tile rows use the same 13px monospace navigation
row as untagged tiles. Neither surface may inherit the document font, a GPUI
default size, or document zoom. A tag folder therefore never becomes taller than
an ordinary navigation row merely because the tile carries a tag.

**Applies to.** `jump_panel_view.rs`: the production Unbound tag-folder header
inside `render_jump_panel`, and `jump_tile_row_el` / `jump_nav_row` for its child
rows.

**Why.** The tile-native Unbound renderer introduced a new tag-folder element
without an explicit font family or size. It fell back to GPUI's larger default,
so tagged groups intermittently looked oversized beside explicitly styled jump
rows.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::jump_panel_tagged_items_keep_fixed_chrome_size`
drives the production tagged and untagged Unbound paint paths, compares their
real bounds to the standard jump navigation row, then changes document zoom and
proves all of those chrome heights remain fixed.
