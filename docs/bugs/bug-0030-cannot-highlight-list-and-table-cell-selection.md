# bug-0030: cannot-highlight-table-cell-selection

**Status:** FIXED
**First seen:** 2026-08-06 ("Can't highlight agent text on bulletpoints or in table cells")
**Component:** `docs/components/common/selection.md` (`UXI-Selection-N`) + agent-tile transcript

Third face of the parsed-`FlatItem::Block` selection family, after
[bug-0008](bug-0008-cannot-select-parsed-blocks-in-transcript.md) (blocks
registered zero hit tokens) and
[bug-0017](bug-0017-cannot-select-code-blocks-in-worksheet.md) (code blocks
painted no highlight). bug-0017 fixed the missing-highlight defect for CODE
blocks only and explicitly logged the follow-up: *"Tables (the other
`FlatItem::Block`) still have no in-cell highlight — not reported; tracked as
follow-up."* This is that follow-up, now reported.

## Symptom

Dragging the mouse across a **table cell** in the agent transcript shows no
highlight — the user sees nothing and reads it as "selection is broken." The
model selects and the clipboard copies correctly; only the visible highlight is
absent (exactly the bug-0017 shape, one block type over).

Reported bundled with "bulletpoints." **Bullet lists were investigated and are
NOT affected**: in the transcript, `detect_block_ranges` only makes a
`FlatItem::Block` for fenced code and tables — a bullet list renders as prose
`FlatItem::Line`s, which already register hit tokens (5/line, verified) and
paint selection via `apply_selection_bg` in `build_wrapped_line`. A prose bullet
drag copies its text (verified headlessly). So the fixable Block defect was the
table cell.

## Context / root cause

The `FlatItem::Block` render arm (`transcript_view.rs`) splits by block type:

- **Fenced code block** → the bug-0017 `block_hits` path: each content line
  self-registers a `TokenHit` from its OWN painted bounds AND paints the
  selection background via `apply_selection_bg_to_runs`. Highlight visible.
- **Table (and any other non-code Block)** → the bug-0008 even-split band path
  (`register_block_hits_on_paint`): registers per-cell `TokenHit` bands so the
  mouse can *select + copy*, but paints **NO** highlight. The block's `RenderCtx`
  has `doc_selection: None` and the band wrapper never drew a selection quad, so
  the user saw nothing — the identical "model selects, clipboard copies, screen
  shows nothing" defect bug-0017 fixed for code.

## Planned solution

Paint the selection highlight in the SAME band geometry the hits are registered
from, inside `register_block_hits_on_paint`. Each band/cell already knows its
raw line, char span `(start_char, count)`, and painted rect; the hit test maps a
mouse x to a char with a uniform monospace-width model
(`col = start_char + round((x-left)/width * count)`). Paint a selection quad over
`[x(sel_start) .. x(sel_end)] × band` using the SAME model, so the highlight
lands exactly where a click/drag selects — consistent by construction, immune to
the block's padding/header, and one mechanism for tables and any future non-code
Block. Different from bug-0008 (registered hits only, no paint) and bug-0017
(per-line real-bounds path for code only — never touched the band path).

## Approaches already tried (do NOT repeat)

- **bug-0008** even-split hit bands: registers hits (copy works) but paints no
  highlight — the exact gap this bug is.
- **bug-0017** per-line real-bounds `block_hits` path: fixed code blocks, does
  NOT apply to the table band path. Re-routing tables through it would need a
  per-cell raw-line + char-offset mapping the parser doesn't retain — the band
  path already has that geometry, so paint there instead.

---

## Log

### 2026-08-06 — selection quad in the band paint (FIXED)

**Root-caused** by reading the `FlatItem::Block` arm: code took the painting
`block_hits` path; tables took the non-painting band path. Confirmed empirically
that transcript bullet lists are prose `FlatItem::Line`s (4 Lines for a 3-item
list; 5 hit tokens/line; a drag copies their text) — so the fixable Block defect
was table cells only.

**What changed.**
- `render_blocks.rs`: `register_block_hits_on_paint` gained `selection` +
  `sel_bg` params. In `RegisterBlockHitsOnPaint::paint`, for each cell, project
  the active selection onto the cell's raw line, intersect with the cell's char
  span, and `window.paint_quad(gpui::fill(...))` a highlight over
  `[x(cs)..x(ce)] × band` using the SAME uniform-width model `hit_test_tokens`
  uses. Painted BEFORE `inner.paint` so it sits behind the glyphs.
- `transcript_view.rs`: the non-code `FlatItem::Block` band call passes
  `sel_snap` + `nc(at_snap.selection_bg)` (was hits-only).
- `main.rs`: `DocRenderTap.band_selection: Vec<(raw_line, s_char, e_char)>` — the
  paint tap the guard asserts against (distinct from `block_selection`, the
  code-block run-bg path).

**How verified.** `verify_harness.rs::transcript_block_table_selection_is_painted`
(REAL `transcript_mouse_down/move` over a frozen markdown table's email cell,
transcript focused): asserts `band_selection` is non-empty, covers the dragged
data row (raw line 2), and the painted span is real-width AND cell-bounded
(within chars 10..21 — a whole-row smear would exceed it); release copies
`scott@x.com`.

**Negative control (observed RED):** set the band call's `selection` arg to
`None` → `band_selection` stays empty → "no selection highlight was painted over
the table cell (bug-0030)" fires. Hit registration is untouched by the revert, so
the copy still works — proving the guard tests the PAINT, not the copy. Restored.

Full suite: **524 bin pass.**

**Shipped:** committed to `main` + release binary rebuilt.

**Unverified.** Live macOS drag delivery (harness gaps #1/#4) — needs the user's
eye on the rebuilt release app. Nested/wrapped bullets and the doc-view (rendered
`.md`) list-item selection paint were NOT in scope (doc-view list items recurse
into `block_inner` with `doc_selection: None` — a latent sibling gap, not the
reported transcript case).
