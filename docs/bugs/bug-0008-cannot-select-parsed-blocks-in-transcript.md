# bug-0008: cannot-select-parsed-blocks-in-transcript

**Status:** FIXED
**First seen:** 2026-07-16
**Component:** docs/components/common/selection.md (`UXI-Selection-1`) + agent-tile transcript

## Symptom

Mouse selection in the agent transcript doesn't work on **any parsed block** — you
can't select a markdown **table**, a **bullet list**, a **code block**, or a
**heading**. Plain prose lines select fine, but agent output is mostly blocks, so it
reads as "selecting with the mouse simply does not work." Reported as tables first,
then bullet points, then "it needs to work everywhere."

## Context / root cause

Transcript selection works via a paint-time **token-hit sink**: `build_wrapped_line`
wraps each painted prose token in `register_token_on_paint`, pushing a `TokenHit
{line, start_char, count, bounds}`; `hit_test_tokens` maps a mouse point → `(line,
char)`. But a **`FlatItem::Block`** (parsed table/list/code/heading) renders through a
DIFFERENT path — `block_inner(&RenderCtx, block)` (transcript_view.rs:1184) — and
`RenderCtx` has **no token sink field**. So blocks register **zero** `TokenHit`s: the
hit-test has nothing to grab inside them, a mouse-down there snaps to the nearest
prose token elsewhere, and their content is unselectable. This is orthogonal to
bug-0003/0006 (which were about prose lines / stripped columns).

## Planned solution

Register per-**raw-line** hit-test bands for every parsed block. A block knows its raw
`(start,end)` line range (`AgentState.block_ranges`); at paint, split the block's
painted height into one horizontal band per source line and push a `TokenHit` mapping
that band to raw line `start+i`. A drag over a block then selects its raw markdown
source lines (the table / list / code text). Precise per-cell selection would need the
parser to retain per-cell source spans (it doesn't) — out of scope; raw-line
granularity delivers "you can select + copy the block."

## Approaches already tried (do NOT repeat)

- <none — first attempt held>

---

## Log

### 2026-07-16 — per-raw-line hit bands for parsed blocks (FIXED)

**What changed.**
- `render_blocks.rs`: new `register_block_lines_on_paint(inner, sink, start_line,
  raw_lines)` element wrapper — at paint it splits its bounds into `raw_lines.len()`
  horizontal bands and pushes one `TokenHit` per raw line (`start_line + i`, the raw
  line's char count, the band bounds).
- `transcript_view.rs`: snapshot `c.block_ranges` into `TranscriptPrep`; build a
  per-flat-item block-range map (paired to `FlatItem::Block` render order); the
  `FlatItem::Block` arm now wraps its element in `register_block_lines_on_paint` with
  the block's raw line range + texts.

**How verified.** Guard `transcript_block_table_is_mouse_selectable`: freezes a
markdown table so it renders as a `FlatItem::Block`, asserts (1) the paint sink now
has `TokenHit`s covering the table's raw lines, and (2) a real `simulate_mouse_*` drag
across it copies the table content. **Negative control (observed RED):** drop the
`register_block_lines_on_paint` wrapper → the block registers zero hits → "registered
NO hit-test tokens" fails. Full suite: 380 pass.

**Granularity.** Non-table blocks (lists / code / headings) select at raw-line
granularity (copy = the raw markdown line). **Tables select PER CELL** (added same
day, on user request): the generalized `register_block_hits_on_paint` takes per-
painted-row `(raw_line, cells)`; for a table each rendered row is split into equal
columns (cells render `flex_1` = equal width, so the split lands on the real cells)
and each column registers a hit keyed to that cell's exact raw char span
(`parse_table_cell_ranges`). The non-rendered `---` separator row is skipped
(`is_table_separator_line`) so vertical bands align to painted rows. Selecting the
`scott@x.com` cell hit-tests to raw line 2, chars 10..21 — exactly the cell.

**Guard update.** `transcript_block_table_is_mouse_selectable` now asserts the data
row registers a DISTINCT hit per cell (`Scott` at char 2, `scott@x.com` at char 10)
and drives the REAL `hit_test_tokens`: the email cell's center → `(line 2, col in
10..=21)`, its left edge → `(2, 10)` (the cell start). Two negative controls, each
observed RED: (a) drop the wrapper → zero hits; (b) force the table onto whole-line
bands → the per-cell `start_char==10` hit disappears.

**Note on the harness.** A full `simulate_mouse_*` drag across a single cell produced
a cross-line selection (a `simulate_mouse` dispatch-timing artifact — gap #4, focus-
accurate not OS-accurate), so the per-cell guard drives `hit_test_tokens` (the exact
function the real mouse path calls) on the painted sink rather than the simulated
drag. **Still runtime-unverified end-to-end** — real macOS drag delivery is gap #1/#4;
needs a rebuild + human check.
