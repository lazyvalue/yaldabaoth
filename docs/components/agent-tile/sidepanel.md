# Agent Tile — Sidepanel (Plan + Subagents)

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-1..3`.

## Description

A segmented, fixed-width **sidepanel on the RIGHT** of the agent tile holding two
stacked segments: **Plan / Tasklist** on top, **Subagents** below — both visible at
once. The main column (transcript + recap + compose) takes the remaining width. Each
segment shows only when its `*_open` flag is set (Subagents also only when
non-empty); with one open it fills the sidepanel height. Built in
`screens.rs::render_agent`: the `agent-sidepanel` container holds the
`tasklist-panel` and `subagent-panes` segments.

## References

- Migrated from `docs/ux-invariants.md` INV-UX-12 (panel focus) + INV-UX-5
  (subagents one-per-line). Those entries are now `→ migrated here`.
- `docs/components/agent-tile/README.md` — parent component.

## UX invariants

### UXI-AgentTile-1 — Plan + Subagents live in a segmented right sidepanel

**Statement.** The Plan and Subagents panels render as a segmented, fixed-width
sidepanel on the RIGHT of the tile — Plan on top, Subagents below — beside the main
column, not above the compose. When both are open, both are visible, stacked, each
scrolling independently. The sidepanel appears only when at least one segment is
open.

**Applies to.** `screens.rs::render_agent`: the `agent-sidepanel` container +
`tasklist-panel` / `subagent-panes` segments; `content_row` (flex_row: main column +
sidepanel).

**Why.** Keep both lists visible at once beside the conversation instead of stealing
height above the compose; a taller reading/writing column.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::subagent_panes_paint_right_of_compose` (PAINT:
panes' left edge at/right-of the compose right edge) +
`plan_and_subagents_share_the_sidepanel` (both segments painted, stacked, inside one
`agent-sidepanel`). Negative-controlled (row→col revert fires RED).

### UXI-AgentTile-2 — A subagent is detected structurally and shown one-per-line

**Statement.** Subagents are derived from the tool-call stream structurally (a
`Task`/Think tool call carrying a spawn prompt; `TodoWrite`/`Read` excluded), not
from a label heuristic, and rendered one row per subagent (status glyph + label +
prompt snippet). Clicking a subagent row focuses its output (swaps the main view —
see the transcript facet / INV-UX-15).

**Applies to.** `agent.rs::classify_subagent` / `AgentState::subagents`; the
`subagent-panes` rows in `screens.rs::render_agent`.

**Why.** The user wants to see WHICH subagents ran and what each was asked, reliably,
regardless of tool naming.

**Status.** `implemented`.

**Enforcement.** `tests.rs::classify_subagent_detects_the_harness_task_shape`,
`tests.rs::subagents_surfaces_registered_task_with_prompt`, and the PAINT proof in
UXI-AgentTile-1.

### UXI-AgentTile-3 — `Cmd-0` focuses the sidepanel; vim selects (2-D), Esc restores

**Statement.** In an agent tile `Cmd-0` enters **panel focus**: the sidepanel widens
and one row in one segment is selected. Selection is **2-D** — `panel_col` (which
segment) + `panel_sel` (row within it). `h`/`l` (or `←`/`→`) switch the active
segment to the adjacent open one (clamping the row in); `j`/`k` (or `↑`/`↓`) move the
row; `g`/`G` jump to its ends. Highlighting a Subagent swaps the main view to its
context; highlighting a Plan row clears the swap. `Enter` commits + leaves focus;
`Esc` leaves and restores the focus captured on entry. The mode is modal (other keys
inert). You can never be panel-focused with no focusable segment. In an agent tile
`Cmd-0` is panel-focus, not zoom-reset.

**Applies to.** `agent.rs`: `AgentFocus::Panel`, `PanelColumn`, `panel_col` /
`panel_sel` / `panel_return_focus`. `agent_ui.rs`: `focus_agent_panel` /
`exit_agent_panel` / `panel_move_selection` / `panel_switch_column` /
`panel_activate_selection` / `reveal_panel_selection`; the modal interception in
`handle_claude_key`. `main.rs`: `FocusAgentPanel` + the `cmd-0` `AgentView` binding.

**Why.** Keyboard-drive the panels (view a subagent's context) without a mouse, with
a clear enter/navigate/exit gesture that always returns you where you were.

**Status.** `implemented` (headless; exact widen px / highlight color are a paint gap).

**Enforcement.** `verify_harness.rs`: `agent_panel_cmd0_enters_and_esc_restores`,
`agent_panel_vim_moves_selection`, `agent_panel_hl_switches_columns`,
`agent_panel_enter_focuses_subagent`, `agent_panel_cmd0_binding_enters_panel`,
`agent_panel_closing_last_panel_exits_focus`, `panel_highlight_swaps_to_subagent`,
`panel_enter_reveals_and_exits`, plus the state-machine fuzzer + oracle.

### UXI-AgentTile-17 — A subagent row stacks label over prompt (two lines, not side-by-side columns)

**Statement.** Each subagent in the Subagents segment renders as a **two-line row**:
line 1 is the status glyph + the subagent label in a single foreground color (warm
accent when that subagent is focused, else the normal editor foreground); line 2 is
the spawn-prompt snippet, **dimmed and indented under the label**, on a **single
ellipsized line** so rows stay short. The label and prompt are NEVER placed
side-by-side on one line — the old two-tone "black label + brown prompt column"
layout is removed. A subagent with no prompt renders line 1 only.

**Applies to.** `screens.rs::render_agent` — the `subagent-panes` rows
(`subagent-pane-{i}`): a `flex_col` row with a glyph+label line and an indented
dimmed prompt line, each `.truncate()`d to a single line.

**Why.** In the ~280px sidepanel the single-line glyph + label + prompt layout read
as two cramped, mismatched-color columns. Stacking gives a clean primary/secondary
hierarchy that reads in a narrow column.

**Status.** `implemented` (headless — stacking is proven by the layout probe; exact
indent px / dim color are a paint gap).

**Enforcement.** `verify_harness.rs::subagent_row_stacks_label_over_prompt` — layout
probe: with a subagent carrying a prompt, the prompt line's painted top is at/below
the label line's painted bottom (stacked, not side-by-side), both non-empty.
Negative-controlled (reverting the row to `flex_row` fired it RED: prompt top 90 vs
label bottom 107.5). No deviation from plan.
