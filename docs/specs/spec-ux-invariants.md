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

**Enforcement.** Headless: the caret-containment guards
(`chatbox_caret_cell_stays_in_window_for_every_edit_path`), the worksheet
caret-on-tail / streaming-cursor tests. Runtime (GPUI paint not headless): type
past the bottom/right edge in each surface and confirm the caret stays visible.

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

## Cross-references

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
