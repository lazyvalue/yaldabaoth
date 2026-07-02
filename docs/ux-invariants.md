# Spec: UX Invariants (canonical, cross-cutting)

**Status:** LIVING — authoritative. This is the canonical contract for how UX
elements behave across the whole app.
**Last updated:** 2026-06-29

## What this is

The single, canonical list of **behavioral invariants every tile and UX element
must honor.** Element-specific specs (`spec-chatbox-caret-containment.md`,
`spec-textbox-compose.md`, `spec-agent-presentation.md`, `spec-yux.md`, …) refine
*how* a given surface satisfies these; this file states *what* is true everywhere,
so a reader (or reviewer, or future change) has one place that says "the cursor is
always visible" without re-deriving it per surface.

## How to use it (mandatory)

- **Every code change must be checked against these invariants.** A change that
  touches a tile, view, editor, scroll, caret, or input surface MUST NOT violate
  an invariant below. If a change appears to require violating one, that is a
  signal to stop and reconcile the spec first — not to ship the violation.
- **This file is updated when new UX is designed.** When a new surface or behavior
  is added, add or extend the relevant invariant here (and link the element spec).
  New invariants get the next `INV-UX-N` id.
- **Each invariant names its enforcement.** Prefer a headless regression test
  (`verify_harness.rs` / `tests.rs`); where GPUI paint can't be driven headlessly,
  say so and name the human runtime check. An invariant with neither is a gap.
- **Conformance is tracked honestly.** Each invariant carries a status:
  `honored` (conformant + guarded), `partial` (conformant on some surfaces),
  or `target` (the contract, NOT yet conformant — a known gap to close).

## Invariants

### INV-UX-1 — The cursor is always visible, and moving it moves the visible text

**Statement.** In any tile or element that has a cursor/caret, the caret is always
within the visible viewport. Moving the caret scrolls the content so the caret
stays visible — both vertically and horizontally. The caret is never stranded
off-screen (above, below, or past the right edge), and the viewport never shows a
region the caret has left.

**Applies to.** Every editable/navigable surface: file read/edit buffers
(`EditView` Code + WordProcessor), the rendered doc view cursor, the agent
transcript navigation caret (transcript-focus), and the agent compose buffer
(worksheet inline + chatbox pinned). Any future surface with a caret.

**Why.** A caret you can't see is a caret you can't use — you don't know where you
are or where your edit will land. This has been the single most-regressed UX
property in the app (the chatbox caret-offscreen bug, "fixed" 15+ times; the
worksheet caret-below-buffer bug; the streaming caret-drift bug).

**How (the discipline that satisfies it).**
- A surface computes its scroll window from the CURRENT caret + the MEASURED
  viewport extent, at ONE chokepoint, and scrolls *to* that window — it never
  reads the scroll offset back or lets a stale offset win
  (`spec-chatbox-caret-containment.md`: `compute_window` for the compose;
  `ListView`/`ScrollAnchoredList` splice-anchoring elsewhere).
- Programmatic edits (streaming, freeze, paste) that shift text shift the caret
  with it, so the caret never drifts out of view (the `splice_insert`/
  `splice_delete` cursor-shift discipline).
- A pending-reveal latch re-renders so the reveal is consumed on the next frame.

**Status:** `honored` — vertical caret containment on all surfaces. The agent
compose no longer needs the horizontal axis at all: it **word-wraps** (INV-UX-2),
so the caret is always on a rendered visual row and there are no off-screen-right
columns. Other monospace surfaces that still scroll horizontally keep the
`compute_window` horizontal half.

**Enforcement.** Headless, two levels:
- **Model:** the caret-containment guards
  (`chatbox_caret_cell_stays_in_window_for_every_edit_path`); the WRAPPED-compose
  vertical-containment guard `compose_wrapped_caret_never_below_the_fold`; the
  worksheet caret-on-tail / streaming-cursor tests.
- **Paint (the real proof):** `compose_caret_row_painted_inside_box_when_wrapped`
  drives a real layout/paint pass (`run_until_parked`) and asserts — via the
  layout probe (`probe_bounds` / `layout_probe_*` in render_blocks.rs) — that the
  caret's row is actually PAINTED inside the compose box. The virtualized list
  never paints an off-screen row, so a caret below the fold fails the test
  (validated by injecting the regression). This closes what a model test can't
  (does the list actually scroll + paint there); the probe is the reusable #3.2
  capability for any painted-geometry assertion.

> **Subtlety (regressed once by INV-UX-2, now pinned):** under word-wrap a logical
> line spans multiple VISUAL rows, so the compose's vertical window MUST be
> computed in visual-row space (`compose_visual_metrics` →
> `compose_first_visible_line` → `compose_item_for_visual_row`). Computing it over
> logical lines strands the caret below the fold — the recurring chatbox-cursor
> bug, reintroduced by the wrap change and re-fixed.

### INV-UX-2 — The agent compose (chatbox / worksheet) always word-wraps

**Statement.** The agent tile's compose buffer wraps long lines to the available
width. Text never runs off the right edge of the box requiring horizontal scroll
to read it; a long line flows onto the next visual row.

**Applies to.** The agent compose buffer in BOTH placements
(`InputModeKind::Chatbox` pinned box, `InputModeKind::Worksheet` inline).

**Why.** A compose box is for composing prose; horizontally-scrolled input is
unreadable and you lose sight of what you wrote. Wrapping keeps the whole draft
visible.

**Status:** `honored` (runtime-unverified for paint, per the GPUI headless gap).
The compose **word-wraps**: `wrap_line_cols` (agent.rs) partitions each logical
line into ≤width visual rows at space boundaries (over-long words hard-break),
covering every char; `build_chatbox_wrapped_line` renders one visual row per
segment via `build_chatbox_line` (each row sliced to exactly its segment ⇒ no
clip, no horizontal scroll), with the caret on the row `caret_visual_row` picks.
The small/virtualized decision keys on TOTAL VISUAL rows so a long wrapped line
can't overflow the un-scrolled small box. This **retired the compose's
horizontal-scroll window** (`spec-chatbox-caret-containment.md` horizontal axis);
the vertical caret-containment is kept.

**Enforcement.** Headless: `wrap_line_cols_word_wraps_and_covers_every_char`
(wraps, hard-breaks, covers every char, ≥1 row, makes progress) +
`caret_visual_row_places_caret_on_a_rendered_row` (caret always on a rendered
row). Runtime (GPUI paint not headless): type a line wider than the box in both
placements and confirm it wraps with the caret visible.

### INV-UX-3 — Agent text uses the normal tile/desktop background

**Statement.** Agent transcript text sits on the SAME background as the normal
yalda desktop / tile — there is no per-turn "card" background tint behind agent
or user turns. Turns are distinguished by the gutter label, the foreground
author tint, and the left bar — never by a different background color.

**Applies to.** The agent transcript (`TranscriptView`). The transient
focused-row highlight (a dim band on the cursor row, shown ONLY while the
transcript is focused for navigation) is NOT a violation — it's a focus/nav cue,
not a resting background. Code blocks keep their own background (code styling, not
a turn card). The compose box keeps its pinned-control affordance.

**Why.** A tinted card per turn makes the transcript read as a separate surface
floating on the desktop; the agent's text should blend into the tile like every
other surface, so the workspace looks like one continuous space.

**Status:** `honored` (runtime-unverified for paint). `transcript_view.rs` sets
`row_bg` to transparent for every committed turn; the per-turn `claude_turn_bg`/
`user_turn_bg` (theme `agent_turn_bg`/`user_turn_bg`) tints are no longer applied.
The cursor-row dim highlight remains, gated on transcript focus
(`cursor_line == usize::MAX` when composing, so no row matches).

**Enforcement.** Runtime (GPUI paint not headless): open an agent tile and
confirm agent/user turns show no background tint distinct from the tile; the
focused row highlights only during transcript (`f`) navigation. (A headless guard
awaits the element-tree-snapshot harness — `docs/projects/headless-e2e/` #3.2.)

### INV-UX-4 — No empty turn header

**Statement.** A `You` / `Claude` turn divider is rendered ONLY for a turn that
has visible content — a prose line, a rendered block, a tool group, or the
in-flight thinking indicator. The transcript never shows a turn header with
nothing under it, and never a stack of empty alternating `You`/`Claude`
dividers.

**Applies to.** The agent transcript (`rebuild_agent_view_model` →
`FlatItem::TurnHeader`).

**Why.** Empty turns are visual noise that make the conversation unreadable and
imply exchanges that didn't happen (the reported "blank turns" — a screenful of
empty `You`/`Claude` dividers between the real turns). They arise when a turn's
only lines are blank (stripped by the blank-collapse pass) or when blank
separator / resume-artifact lines carry their own escalating turn numbers.

**Status:** `honored`. After the flat-item build (blank-collapse, tool-group
merge, thinking indicator), `rebuild_agent_view_model` runs a right→left pass
that drops any `TurnHeader` with no non-header item before the next header.

**Enforcement.** Headless: `rebuild_drops_empty_turn_headers` builds a transcript
with empty turns (blank lines carrying escalating turn numbers) interleaved with
real turns and asserts no header is orphaned (every header is followed by content;
header count == content-bearing-turn count). Validated by disabling the pass →
the test fails.

### INV-UX-5 — Subagents are detected from the harness, shown as a one-per-line list above the compose

**Statement.** Sub-agents (the agent's `Task` spawns) are detected from the
STRUCTURE the harness emits over ACP — not a name heuristic — and surfaced as a
compact **one-per-line list ABOVE the compose box** (status glyph + label + spawn
**prompt** snippet on each line; not cards). Clicking a line focuses the subagent
(the transcript shows its output).

**Applies to.** The agent tile (`classify_subagent` / `AgentState::subagents`;
the subagent-panes strip in `render_agent`).

**Why.** The user must see what each subagent was asked (prompt) and how it's
doing (status) + its output. The previous detector keyed on `kind == ToolKind::Other`,
which a real `Task` never has (claude-code-acp maps `Task` → `ToolKind::Think`),
so subagents were invisible; and they were tucked in a right sidebar.

**Status:** `honored` (detection AND pane layout headless-tested). `classify_subagent`
keys on `Think` + a `prompt`/`subagent_type` raw-input (excluding `TodoWrite`'s
`todos`), with a name fallback, and captures the prompt. `render_agent` renders the
list one-per-line above the compose (auto when subagents exist; `Cmd-2`/`ToggleSubagents`
collapses).

**Enforcement.** Headless: `classify_subagent_detects_the_harness_task_shape`
(Think+prompt detected, prompt captured; TodoWrite/Read excluded; name fallback) +
`subagents_surfaces_registered_task_with_prompt` (end-to-end through the real
`ToolCall`) + **PAINT proof** `subagent_panes_paint_above_the_compose` (the layout
probe asserts the list strip is painted with its bottom at/above the compose box's
top). Runtime: only click-to-show-output is left for a human eyeball.

### INV-UX-7 — A submit is delivered immediately (even mid-turn); failed sends queue, never drop (stop is ⌘., not Esc)

> **Numbering note:** `INV-UX-6` is reserved for the parallel `toolgroup-expand-key`
> branch (tool-group collapse). This invariant is `INV-UX-7` to avoid a collision
> at integrate time.

**Statement.** Submitting a message **sends it to the agent immediately, even
while a turn is in flight**, and commits it as a user turn — it does **not** start
a duplicate competing local turn (a mid-turn steer rides the in-flight turn; the
running clocks are not reset). When the agent advertises the `promptQueueing`
capability the worker forwards the prompt concurrently, so the agent receives the
steer mid-turn and processes it the instant the current turn finishes. If the send
**fails** (offline / reconnecting) the draft is **left in the compose** with a
status so the user can retry — never silently moved or dropped. The stop gesture is
**`⌘.`** (`stop_agent` → `session/cancel`; a second press force-restarts). **`Esc`
is NOT a stop** — it is the worksheet mode key (Insert→Normal, leave-block), and
binding it to stop conflicted with mode switching, so it was unbound (2026-06-29).

**Applies to.** The agent tile: `submit_compose` / `send_prompt_to_session`
(`agent_ui.rs`) and the worker's concurrent driver (`acp_channel.rs`, gated on
`promptQueueing`). There is no client-side steering queue — delivery is immediate.

**Why.** Over ACP **v1 a prompt is a turn** and there is no mid-turn input
message — but the live agent (`claude-agent-acp`) advertises a vendor capability
`_meta.claudeCode.promptQueueing` and (verified by live probe) **accepts a
`session/prompt` while a turn is in flight**, queueing it without interrupting.
yalda's worker previously *serialized* (awaited each turn before sending the next),
so a steer couldn't reach the agent until the boundary; the concurrent driver
fixes that. ACP v2 (the `unstable_protocol_v2` draft) is NOT yet honored by the
agent — it negotiates down to v1 — so promptQueueing is the real mechanism, and
this design is the v2-ready shape (`spec-turn-steering.md`).

**Status:** `honored` — state, ordering, and transport verified.

**Enforcement.** Headless in `verify_harness.rs`:
`steering_submit_while_awaiting_sends_immediately` (mid-turn submit sends + commits
+ doesn't reset the turn), `steering_midturn_ordering_and_dedup` (steer lands after
prior agent content, committed once, agent echo deduped — via the real reducer),
and `esc_interrupts_in_flight_turn` / `stop_interrupts_only_when_in_flight`.
Transport (live, not headless): `tests/steering_midturn_live.rs` drives the REAL
worker + REAL `claude-agent-acp` and proves a mid-turn steer is delivered and
processed; the v2-refused / promptQueueing facts were confirmed by a live probe.
The only thing left for a human is the subjective visual feel.

### INV-UX-8 — Worksheet renders inline-flush; chatbox renders as a pinned box

**Statement.** The two compose **placements** are visually distinct, not
cosmetically identical. In **Worksheet** placement the compose renders **inline
flush in the transcript column** — no box chrome (no panel background, border, or
horizontal margins), with an accent **left bar** as the `›` draft gutter and the
**You** label — so the draft reads as a continuation of the conversation. In
**Chatbox** placement the compose renders as a **pinned, bordered box** inset from
the column edge by a horizontal margin. Toggling placement (`Ctrl-Alt-Enter` /
space → w) therefore produces a **visible** change, not a near-no-op.

**Applies to.** The agent tile compose panel (`compose_panel` in `render_agent`,
`screens.rs`), both the small-draft and virtualized render paths.

**Why.** Model C (ADR-0024) made worksheet and chatbox **one model at two
placements**, but v1 shipped the placement axis with near-identical rendering
(both a boxed panel; worksheet differed only by an accent border + label), so
switching "felt like nothing happened" and worksheet read as broken. The flush
inline rendering is the deferred §7/v1 styling from `design-c.md` that makes the
worksheet placement actually distinct and usable. The inner compose body (wrap,
caret-containment window, virtualization) is **unchanged** — only the outer chrome
differs — so INV-UX-1/INV-UX-2 are untouched.

**Status:** `honored` — placement chrome implemented; the visible distinction is
headless-tested via the captured compose bounds. Exact glyphs/colors are the one
human-eye item (harness gap #1).

**Enforcement.** Headless in `verify_harness.rs`:
`worksheet_renders_flush_chatbox_renders_boxed` — boots in chatbox, captures the
compose box's painted bounds via the `compose-box` layout probe, toggles to
worksheet, re-captures, and asserts the worksheet box paints **~8px further left**
(the chatbox `mx_2` margin is gone) — i.e. flush in the column. The border/bg/
accent-bar differences are color-level (harness gap #1, a human eye). INV-UX-1's
`compose_caret_row_painted_inside_box_when_wrapped` still passes in both
placements (caret math unchanged).

> **SUPERSEDED by INV-UX-9 (2026-06-28).** INV-UX-8 described the two compose
> *placements* of the Model C UX (always-present worksheet compose vs pinned
> chatbox box). INV-UX-9 replaces that UX: the worksheet has **no always-present
> compose** — you edit **inline in the transcript buffer** (a You-block), and the
> chatbox is **mid-turn-only**. The flush-vs-boxed distinction is retained only for
> the mid-turn chatbox. The data substrate (Model C, ADR-0024) is unchanged.

### INV-UX-9 — The worksheet is an inline-editable conversation buffer (chatbox is mid-turn only)

**Statement.** New agent sessions **default to Worksheet** (the canonical mode);
the chatbox is the mid-turn surface plus an optional persistent placement
(`Ctrl-Alt-Enter`). Worksheet mode behaves as an **editable conversation buffer**,
per `spec-worksheet.md` (AUTHORITATIVE). Concretely:

1. **Free navigation.** In Normal mode the caret moves freely over the whole
   transcript (read navigation; nothing editable until Insert). While navigating,
   the element under the cursor is focus-highlighted — including **You-blocks** (a
   block highlights when the cursor is on its anchor line), so your insertion points
   read like every other transcript element. (The tint color is a paint/human-eye
   check — harness gap #1.)
2. **Insert opens a You-block at the caret** — a `You` delimiter + editable region
   where typed text lives.
3. **Empty insert is a no-op** — leaving Insert with only whitespace removes the
   You-block; the transcript is byte-identical to before (no phantom turn, cf.
   INV-UX-4).
4. **Non-empty You-block persists and is sent** — real text stays in place as the
   pending reply; the next Submit sends it and freezes it as a committed user turn.
5. **Insert is bounded** — a You-block can be opened **only within the most-recent
   agent turn, only after an agent newline**; frozen content is not editable.
6. **Multiple insertion points** — Insert at a new legal point opens another block
   (the previous parks in place, text kept); one is active, the rest render inline
   read-only; Submit sends them all combined + freezes each in place. A fresh
   (empty) session opens one tail block so there's always a visible input.
   - **The You-block is a DOCUMENT, not a text box (intent, 2026-07-01).** It renders
     EVERY line and GROWS with its content — it must never become a fixed-height
     region that scrolls its own text out of view (you are co-authoring a doc, not
     typing in a little box). Keeping the caret visible is the TRANSCRIPT scroll's
     job: the reveal scrolls to the caret's visual ROW *within* the block (parked ~2
     rows above the viewport bottom, so earlier lines flow up the page), never by
     truncating the block. This upholds INV-UX-1 for a block of any height. (Was
     windowed to 10 logical lines — the "You div scrolls after a while" bug.) Pinned
     by the PAINTED guard `worksheet_tall_you_block_grows_caret_painted_in_viewport`
     (a block taller than the viewport, caret proven inside it) in
     `transcript_view.rs` (render: all lines) + the reveal in `TranscriptView`.
7. **Mid-turn → chatbox.** While the agent is mid-turn the transcript is read-only
   and a chatbox appears **pinned at the bottom**; input goes there (steers/queues,
   INV-UX-7). The chatbox is **not visible when the agent is idle**.
   - **The leaders stay universal mid-turn (revised 2026-07-01).** Suppressing all
     mid-turn keystrokes into the chatbox also killed the `<space>`/`.`/`?` leader
     menus — the reported "leaders don't work mid-turn" bug. The rule now keys off
     the **steering draft**: with an EMPTY draft the worksheet is resting in nav, so
     the leaders FIRE (open the tile/workspace/global menu); once the draft is
     non-empty the keystrokes belong to the chatbox (so spaces in a steer stay
     spaces) and the leaders are suppressed. The bare `m`/`'` **mark chord** never
     fires mid-turn — `m` always types (it routes to the chatbox), independent of
     the draft. Governed by `focused_in_insert_mode` (the `midturn_steer` term) and
     the mark-chord `transcript_nav` guard (`agent_ui.rs`). Pinned real-state (via
     the in-process channel seam, `AcpChannelClient::test_connected`) by
     `real_midturn_worksheet_empty_draft_space_opens_menu`,
     `real_midturn_worksheet_typed_draft_space_is_suppressed`, and
     `real_midturn_worksheet_m_types_not_marks` (run with `--features test-support`).

**Substrate.** This is layered on Model C (ADR-0024), which stays the durable
implementation: transcript is the single ordered source of truth, agent content
appends/streams at a clean EOF, only user *text* enters the buffer. Rules 5–7 make
that safe — editing happens only while idle, so streaming never lands in a buffer
being edited (the ordering-corruption class Model C killed stays unrepresentable).

**Applies to.** The agent tile: the worksheet key dispatch (`handle_claude_key`,
`agent_ui.rs`), the You-block lifecycle (insert-enter opens / empty-exit discards /
submit freezes), the insertable-point guard (`agent.rs`), and the mid-turn chatbox
visibility (`screens.rs` `render_agent`).

**Why.** The Model C *UX* (read-only transcript + always-present separate compose)
made the worksheet "functionally useless — can't place the cursor anywhere to
respond." The inline-edit behavior is what the user actually wants and what
`spec-agent-window.md` §9–§15 originally specified; INV-UX-9 restores it on the
durable Model C substrate.

**Status:** `partially implemented` — **stages 1 + 2** (tickets 001/002) landed:
rules 1–7. The You-block opens at the caret (Insert from worksheet navigation),
renders INLINE in the transcript at its anchor (`FlatItem::YouBlock`), is discarded
on empty Esc (transcript byte-identical), persists on non-empty Esc (one block),
freezes IN PLACE at the anchor on Submit (`freeze_as_user_turn_at`), and is gated to
the latest agent turn / tail (the legal-point guard). The bottom chatbox shows only
mid-turn. **Deferred (ticket 003):** retiring the user-selected Worksheet⇄Chatbox
toggle + defaulting new sessions to Worksheet. Do NOT mark `honored` until 003 +
a human runtime check (the inline caret/colours are harness gap #1). See
`docs/projects/worksheet-inline-edit/`.

**Enforcement.** Headless in `verify_harness.rs`:
`worksheet_insert_opens_and_empty_esc_discards_you_block` (open + byte-identical
discard, INV-1), `worksheet_nonempty_you_block_persists_after_esc` (rules 4/6),
`worksheet_compose_visibility_tracks_block_and_turn` + `worksheet_renders_flush_chatbox_renders_boxed`
(painted: inline block vs bottom chatbox vs hidden — rules 2/6/7). In `tests.rs`:
`you_block_anchor_guard_restricts_to_latest_turn` (rule 5). In `editor.rs`:
`freeze_as_user_turn_at_inserts_between_agent_lines` +
`freeze_as_user_turn_at_tail_degrades_to_eof_append` (the in-place freeze + its
metadata auto-shift). The render-count proxy `transcript_021_*` still passes —
chatbox typing leaves the transcript flat (the compose fingerprint is live only for
the inline block).

### INV-UX-10 — The jump-panel agent dot is a per-session status light

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

**Status:** `partially honored` — the **mapping** is headless-guarded; the actual
**hue** is a paint/human-eye detail (harness gap #1). Roster-only sessions can't
show working/waiting until the server reports turn state in `SessionInfo` (today
`Neutral`).

**Enforcement.** Headless in `verify_harness.rs`:
`agent_status_dot_reflects_turn_phase` (idle→WaitingForYou, mid-turn→Working
through the real `jump_panel_agent_rows`) and the pure `agent_dot_status_mapping`
unit test (totality + disconnected-wins). The hue itself is a runtime check.

### INV-UX-11 — `ctrl-<n>` jumps to the n-th workspace (the number the panel shows)

**Statement.** The jump panel numbers **non-ephemeral** workspaces `1..N` (the
`idx + 1` badge), and `ctrl-1`…`ctrl-9` / `ctrl-0` (the 10th) jump straight to
that workspace. The displayed digit and the keystroke target always agree because
both skip ephemeral virtual workspaces (ADR-0021) — `goto_workspace_number(n)`
selects the n-th non-ephemeral tab. A digit past the last workspace is a no-op.

**Applies to.** `main.rs`: the `GotoWorkspace1..10` actions + `ctrl-<n>`
bindings (app-global, `None` context), `goto_workspace_number`, and the
`WorkspaceNavExt::workspace_nav` helper wired onto every screen root (the action
needs a handler in the focused element's ancestry — same discipline as
`toggle_jump_panel`). `jump_panel_view.rs`: the workspace-row number badge.

**Edge.** An **empty-layout** workspace renders a bare div with no action
handlers (chrome.rs), so global keys (incl. `ctrl-<n>`, `ctrl-tab`, `cmd-t`)
don't dispatch while sitting on one — a pre-existing, transient edge state, not
specific to this binding.

**Why.** Direct numeric workspace switching, matching the visible numbering.

**Status:** `honored` (headless).

**Enforcement.** `verify_harness.rs`: `ctrl_digit_switches_workspace` (full
keymap→action→handler dispatch: `ctrl-3` then `ctrl-1`, plus past-the-end no-op)
and `workspace_number_skips_ephemeral` (numbering skips the ephemeral tab).

### INV-UX-12 — `Cmd-0` focuses the agent bottom panels; vim selects (2-D), Esc restores

**Statement.** The agent bottom panels render as **two side-by-side columns** above
the compose — **Plan / Tasklist on the LEFT, Subagents on the RIGHT** (each shown
only when open; Subagents only when non-empty; one open fills the width). In an
agent tile `Cmd-0` enters **panel focus**: the region enlarges and one row in one
column is selected. Selection is **2-D** — `panel_col` (which column) + `panel_sel`
(the row WITHIN that column). `h`/`←` and `l`/`→` switch the active column to the
adjacent **open** column (clamping the row into it); `j`/`k`/`↑`/`↓` move the row
within the active column (clamped); `g`/`G` jump to its ends. `Enter` activates the
selected row (a Subagent row focuses its output and leaves panel focus; a Plan row
has no target yet). `Esc` leaves panel focus, **restoring the focus captured on
entry** (`panel_return_focus`). The mode is **modal** — while focused, other keys
are inert (no leaders, no compose typing). You can never be panel-focused with no
focusable column: entering requires a column with ≥1 row (lands on the leftmost
such), and closing the active column **re-seats** to another open column or exits if
none remain. In an agent tile `Cmd-0` is panel-focus, **not** zoom-reset (the
`AgentView`-scoped binding is registered after the global `cmd-0 ZoomReset` so
GPUI's most-recent-first match prefers it); elsewhere `Cmd-0` still resets zoom.

**Applies to.** `agent.rs`: `AgentFocus::Panel`, `PanelColumn`, `PanelItem`,
`panel_column_rows` / `panel_open_columns` / `reseat_panel_focus`, the `panel_col` /
`panel_sel` / `panel_return_focus` state. `agent_ui.rs`: `focus_agent_panel` /
`exit_agent_panel` / `panel_move_selection` / `panel_switch_column` /
`panel_select_end` / `panel_activate_selection`, the modal interception at the top
of `handle_claude_key`, and the re-seat in `toggle_tasklist` / `toggle_subagents`.
`main.rs`: the `FocusAgentPanel` action + `cmd-0` `AgentView` binding.
`screens.rs::render_agent`: the two-column layout + enlarge + per-column selection
highlight + `FocusAgentPanel` `on_action`.

**Why.** Keyboard-drive the bottom panels (jump to a subagent's output) without a
mouse, with a clear enter/navigate/exit gesture that always returns you where you
were; the column split keeps Plan and Subagents side by side.

**Status:** `honored` (headless; exact enlarge px / highlight color are gap-1).

**Enforcement.** `verify_harness.rs`: `agent_panel_cmd0_enters_and_esc_restores`,
`agent_panel_vim_moves_selection`, `agent_panel_hl_switches_columns`,
`agent_panel_enter_focuses_subagent`, `agent_panel_cmd0_binding_enters_panel` (real
keymap dispatch proves the AgentView binding shadows zoom-reset),
`agent_panel_closing_last_panel_exits_focus`, plus the state-machine fuzzer ops +
oracle (`focus ∈ {Compose,Transcript,Panel}`, panel-focused ⇒ a panel is open).

### INV-UX-13 — Document text zoom scales the agent transcript, like a buffer

**Statement.** `Cmd-=` / `Cmd-+` (in) and `Cmd--` (out) — the document text-zoom
`text_scale` — scale the **agent transcript** the same way they scale the buffer
doc view: the conversation **prose** and the transcript's **markdown blocks**
(headings / code / tables) multiply by `text_scale`. Zoom is GLOBAL (not session
state): its action handler pushes `notify_transcript_views(TextStyle)` so every live
`TranscriptView` re-renders and re-reads `text_scale` off the root (via
`RootSnapshot`) — it is NOT a per-session `TranscriptSeqs` seq. As with buffers,
**chrome stays at native size**: the turn/tool gutter labels, tool-card status
glyphs, the bottom panels, the status footer, and the **compose input** (its caret
and line-box are pixel-pinned for caret-containment — INV-UX-1 — so its font is held
fixed; scaling it would require scaling the caret + `CHATBOX_CHAR_W` in lockstep, a
separate change). `Cmd-0` resets zoom everywhere EXCEPT agent tiles, where it is
panel-focus (INV-UX-12) — zoom-out then back is the reset there.

**Applies to.** `transcript_view.rs`: `RootSnapshot.text_scale` (read from
`root.text_scale`), the per-line `text_size(px(13.0 * text_scale))` on the
`FlatItem::Line` **row wrapper** (NOT just `claude-body` — `gpui::list` items do
not inherit the list's ambient text size, so the size must live on the item, the
same way the doc/WP views set it on each line wrapper), and the `FlatItem::Block`
`RenderCtx { text_scale }`. `main.rs`: `set_text_scale` → `notify_transcript_views`.

**Why.** Reading the agent conversation at a comfortable size should work exactly
like reading a document — the transcript is the agent tile's primary reading
surface.

**Status:** `honored` (headless — the painted line height is probed).

**Enforcement.** `verify_harness.rs`: `transcript_prose_scales_with_zoom` (probes
a prose line's PAINTED height at 1× vs 2× — it must grow, so the font actually
scales, not just the cache busting) + `transcript_021_theme_and_zoom_bust_cache`
(zoom re-renders the transcript once — the invalidation path). Exact glyph shape
remains the harness's pixel gap (#1), but the size change is now guarded.

## Cross-references

- `spec-turn-steering.md` — the full steering design (queue, delivery modes, the
  ACP v1/v2 constraint) this invariant summarizes.
- `spec-chatbox-caret-containment.md` — the compose caret window. Its VERTICAL
  axis still governs the compose; its HORIZONTAL axis is RETIRED for the compose
  (superseded by INV-UX-2 word-wrap).
- `spec-agent-presentation.md` / `spec-agent-render-pipeline.md` — the agent
  render path + `TranscriptSeqs` fingerprint discipline (every render input
  covered, never notify in render) that keeps caret state from going stale.
- `spec-yux.md` — `ScrollAnchoredList` / `ListView` splice-anchoring (the scroll
  primitive INV-UX-1 leans on).
- ADR-0024 — Model C (read-only transcript + separate compose); the compose is
  the surface INV-UX-2 governs.

## Revision history

- 2026-06-29 (5) — Added INV-UX-13: document text zoom (`Cmd-±`) now scales the
  agent **transcript** (conversation prose + markdown blocks) by `text_scale`, like
  the buffer doc view; chrome + the pixel-pinned compose input stay native. The
  size must live on each `FlatItem::Line` wrapper — `gpui::list` items don't inherit
  the list's ambient text size, which is why an initial `claude-body`-only attempt
  left the prose unscaled. Guard: `transcript_prose_scales_with_zoom` (probes the
  painted line height grows 1×→2×).
- 2026-06-29 (4) — Reworked the agent bottom panels into **two side-by-side
  columns** above the compose — Plan/Tasklist (left) + Subagents (right) — and added
  INV-UX-12 (`Cmd-0` focuses + enlarges them; **2-D** vim selection: `h`/`l` switch
  column, `j`/`k` move the row within it, `g`/`G` ends; `Enter` activates, `Esc`
  restores prior focus; modal; re-seats/auto-exits when the active column closes;
  `Cmd-0` is panel-focus, not zoom-reset, in agent tiles). Guards:
  `agent_panel_cmd0_enters_and_esc_restores`, `agent_panel_vim_moves_selection`,
  `agent_panel_hl_switches_columns`, `agent_panel_enter_focuses_subagent`,
  `agent_panel_cmd0_binding_enters_panel`, `agent_panel_closing_last_panel_exits_focus`,
  + fuzzer ops/oracle.
- 2026-06-29 (3) — Added INV-UX-10 (jump-panel agent dot is a per-session status
  light: working=warm accent, waiting-for-you=green, neutral=dim/disconnected;
  mapping `AgentRow::dot_status`) and INV-UX-11 (`ctrl-<n>` jumps to the n-th
  workspace, the number the panel shows; `goto_workspace_number` skips ephemeral
  workspaces). Guards: `agent_status_dot_reflects_turn_phase`,
  `agent_dot_status_mapping`, `ctrl_digit_switches_workspace`,
  `workspace_number_skips_ephemeral`.
- 2026-07-01 — **Worksheet rests TYPEABLE, not in nav (supersedes 2026-06-29 (2)).**
  Resting in nav made `/clear` (and fresh open) require pressing `i` before anything
  you typed appeared — the "can't see anything I'm typing after clear" bug. A fresh /
  cleared / restored worksheet now rests focused + Insert (`settle_input_focus` →
  focus=Compose), so typing lands and is visible immediately. The tile-menu leaders
  are NOT lost: the `<space>`/`.`/`?` still fire on an EMPTY block via the empty-draft
  heuristic in `focused_in_insert_mode` (bare space on a blank block opens the menu;
  once you've typed, space types — the same rule as mid-turn steer). Both prior
  complaints now hold. Guards: `worksheet_typing_after_clear_is_visible_without_pressing_i`,
  `clear_resets_worksheet_to_a_typeable_block`, `fresh_worksheet_space_opens_the_tile_menu`,
  `focused_in_insert_mode_agent_tile_gate`.
- 2026-06-29 (2) — Worksheet rests in transcript NAV (not auto-Insert) after fresh
  open / `/clear` / restore: the input block is VISIBLE but the `space`/`.`/`?`
  leaders open the tile/app menus (they fire only outside text entry) — fixing "can't
  use the tile menu when an agent tile is focused"; press `i` to type. Tool-group
  fold headers clamped to one line (`fold_header_line`) so a multi-line command no
  longer renders its body in the header ("tool use not folded"). Guards:
  `fresh_worksheet_space_opens_the_tile_menu`, `fold_header_line_is_single_short_line`.
- 2026-06-29 — Worksheet runtime fixes: `/clear` settles to a typeable block;
  multiple insertion points editable deterministically (caret on an existing block
  resumes it); you-div scoped-Normal indicator (`You · NORMAL` + accent tint) and
  nav-cursor highlight on you-divs; **`Esc` unbound from stopping a turn** (INV-UX-7
  updated — stop is `⌘.`; Esc is the worksheet mode key); bare-`m`/`'` mark chord
  gated to transcript-nav only so `m`/`'` stay typeable in the compose.
- 2026-06-28 — Added INV-UX-8 (worksheet renders inline-flush in the transcript
  column; chatbox stays a pinned box — the two placements are now visibly
  distinct, closing the "toggling worksheet does nothing" gap). The deferred
  `design-c.md` §7/v1 inline-flush styling. Guards
  `worksheet_renders_flush_chatbox_renders_boxed` in `verify_harness.rs`.
- 2026-06-26 — Added INV-UX-7 (mid-turn submits steer/queue instead of starting a
  competing turn; no optimistic transcript echo; Esc interrupts an in-flight
  turn). See `spec-turn-steering.md`. Guards `steering_*` +
  `esc_interrupts_in_flight_turn` in `verify_harness.rs`. (INV-UX-6 reserved for
  the parallel tool-group branch.)

- 2026-06-25 (5) — Added INV-UX-5 (subagents detected structurally from the
  harness + shown as panes below the compose, with the prompt). Fixes the
  `kind==Other` detector that never matched a real `Task`. Guards
  `classify_subagent_detects_the_harness_task_shape` +
  `subagents_surfaces_registered_task_with_prompt`.

- 2026-06-25 (4) — Added INV-UX-4 (no empty turn header) → `honored`: a right→left
  pass in `rebuild_agent_view_model` drops `TurnHeader`s with no content before the
  next header (the "blank turns" bug). Guard `rebuild_drops_empty_turn_headers`.
- 2026-06-25 (3) — Added INV-UX-3 (agent text uses the tile/desktop background;
  no per-turn card tint) → `honored`: `transcript_view.rs` `row_bg` transparent
  for all turns; focus-row highlight retained, gated on transcript focus.
- 2026-06-25 (2) — INV-UX-2 implemented → `honored`: the compose word-wraps
  (`wrap_line_cols` / `build_chatbox_wrapped_line`), retiring its horizontal-scroll
  window; INV-UX-1's compose horizontal half is now moot. Tests
  `wrap_line_cols_word_wraps_and_covers_every_char` +
  `caret_visual_row_places_caret_on_a_rendered_row`.
- 2026-06-25 — Created. INV-UX-1 (cursor always visible + tracks text;
  `partial`/`honored`), INV-UX-2 (agent compose word-wraps; `target` — chatbox
  currently horizontal-scrolls, a known gap).
