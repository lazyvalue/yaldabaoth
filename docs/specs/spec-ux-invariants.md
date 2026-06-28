# Spec: UX Invariants (canonical, cross-cutting)

**Status:** LIVING — authoritative. This is the canonical contract for how UX
elements behave across the whole app.
**Last updated:** 2026-06-25

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

### INV-UX-7 — A submit is delivered immediately (even mid-turn); failed sends queue, never drop; Esc interrupts

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
status so the user can retry — never silently moved or dropped. In the agent view,
bare **`Esc` interrupts an in-flight turn** (`session/cancel`); with no turn in
flight, `Esc` keeps its existing meaning.

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
