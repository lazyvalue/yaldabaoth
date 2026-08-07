# bug-0031 — char-select caret doesn't line up with the selection

**Status:** FIXED
**First seen:** 2026-08-07
**Component:** AgentTile / transcript (`UXI-AgentTile-34`), caret rendering.

## Symptom

In the agent worksheet transcript, char-select mode (`v` + motion) shows the caret
visually **mismatched** with the selection: extending rightward, the highlight ends
one cell before the caret, and the caret block sits one cell *past* the highlighted
span. "The caret and the selection highlight end don't line up."

## Context / root cause

The transcript selection model is uniformly **exclusive of the head cell**:

- Highlight: `line_selection_range` (`render_blocks.rs`) returns `[start, cursor_col)`.
- Copy: `selection_char_range` (`editor.rs`) slices `[start, cursor_col)`.

Highlight and copy AGREE. But on the cursor line, `build_wrapped_line` (`agent.rs`)
draws the Normal-mode caret as a **block over the cell at `cursor_col`** — which is
NOT part of the selection. So during a rightward `v` selection the orange caret block
is always one cell to the right of the blue highlight's trailing edge. (Leftward, the
cursor is the selection START, so the block sits on the first highlighted cell — no
visible mismatch.)

## Planned solution

Purely-visual, no change to copy/selection semantics: while a **non-empty selection**
is active on the cursor line, render the caret as a **beam** (Insert-style, a
zero-width mark before `cursor_col`) instead of a Normal-mode block. The beam sits at
the left edge of `cursor_col` = the highlight's trailing edge, so caret and highlight
line up. Copy/selection ranges are untouched (still `[start, cursor_col)`), so `V`
line-wise select, mouse drag, and the `r` quote are unaffected.

Scope: the transcript `build_wrapped_line` call in `transcript_view.rs` picks the
caret mode via a pure `caret_mode_during_selection` helper (`agent.rs`). Line-wise
`V` (cursor at EOL) and no-selection nav are unchanged.

## Approaches already tried (do NOT repeat)

- **Inclusive-of-head selection (vim-style: cursor cell is the last selected
  cell).** Rejected — it changes copy/`selection_text` semantics (shared by mouse
  drag-select, copy-on-select, and the `r` quote), overshoots on line-wise `V` (its
  cursor rests at EOL, so +1 grabs the newline), and would break the existing
  `UXI-Selection` copied-span tests. The beam fix touches zero copy semantics.

## Log

### 2026-08-07 — FIXED (beam caret during selection)

- **Change.** `agent.rs`: new pure `caret_mode_during_selection(mode,
  active_selection)` → `Insert` (beam) when a selection is active, else `mode`.
  `transcript_view.rs`: the `build_wrapped_line` call computes `active_sel` (cursor
  line + a non-empty `sel_snap`) and passes `caret_mode` instead of `mode_snap`.
  No change to `line_selection_range` / `selection_char_range` — copy + highlight
  ranges are identical to before.
- **Verified.** `verify_harness.rs::worksheet_char_select_caret_is_beam` — real
  `v`+`l`… keystrokes, then asserts the render actually painted a beam via a new
  `DocRenderTap.caret_beam_on_cursor_line` tap (recorded in `build_wrapped_line`'s
  cursor-line path). **NC observed RED:** revert the helper to `mode` always →
  `caret_beam_on_cursor_line == Some(false)` (block). Pure unit:
  `tests.rs::caret_mode_during_selection_beams_only_with_selection`. Full suite 537
  green.
- **Outcome.** The caret now sits flush at the selection's trailing edge; no orange
  block one cell past the highlight. The exact painted glyph (beam vs block shape)
  is harness gap #1 — the tap proves which mode the render CHOSE on the real path.
