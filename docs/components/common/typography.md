# Component: Typography (common)

**Status:** living
**Component token:** `Typography` (⇒ `UXI-Typography-N`)

## Description

The reading typography of rendered markdown: one shared **type scale** (heading
sizes, heading leading and space-above, block gap, reading measure, list gutter)
defined once in `yux/typography.rs` (`TypeScale` / `TYPE_SCALE`), and the block
treatments built on it in `render_blocks.rs` — the reading measure, heading rhythm,
hanging list markers, the blockquote rule, and the code-block header (language
label + copy button). The Doc view (`Buffer::Viewing`) is the primary consumer; the
Word-Processor edit view adopts the same heading scale; the agent transcript shares
`block_inner`, so its parsed blocks get the same heading/list/quote treatment (but
not the reading measure or the copy button, which are Doc-view only).

All tokens scale with the document zoom (`text_scale`, [TextZoom](text-zoom.md));
chrome does not. The inter-block gap keeps [ParagraphSpacing](paragraph-spacing.md)
(`PARAGRAPH_GAP_PX` on top of `TYPE_SCALE.block_gap_px`).

## References

- `docs/components/buffer.md` — the Doc view and the WP editor are Buffer sub-views.
- `docs/components/common/paragraph-spacing.md` — the between-block gap.
- `docs/components/common/text-zoom.md` — the `text_scale` every token multiplies by.
- `docs/components/common/blockquote.md` — quoted text is italic (unchanged here).

## UX invariants

### UXI-Typography-1 — The Doc's text column is a centered reading measure

**Statement.** In the Doc view every markdown block sits in a column whose text is
at most `TYPE_SCALE.measure_ch` (72) `ch` of the body font at the current zoom,
centered in a wider tile; in a tile narrower than that the column takes the full
width. The measure scales with zoom. The cursor bar and selection belong to the
block, so they move with the column. Source-file Docs (one code line per block)
keep the full width.

**Applies to.** `doc_view.rs` — `reading_measure` (body font `ch_advance` ×
`measure_ch`), the per-row centered `max_w` column in `build_doc_body`.

**Status.** `implemented`.

**Enforcement.** `md_harness.rs::doc_reading_measure_caps_centers_and_scales`
(painted column width = measure + 15px chrome, centered, ×1.5 at 1.5× zoom, full
width in a 500px window). NC: drop the `max_w` → 1190 vs 619.8, RED.

### UXI-Typography-2 — One heading scale for reading and writing

**Statement.** A heading of level N is the same size and leading in the Doc view and
the WP editor: both take `TYPE_SCALE.heading(N)` and `heading_leading()` (relative
1.3). Headings get extra space ABOVE them (`heading_space_above`, not on the first
block) and the normal block gap below, so a heading binds to the section it opens.

**Applies to.** `render_blocks.rs` (`block_inner` heading arm, `block_element`
space-above), `edit_view.rs::build_wp` (heading size / leading / top pad).

**Status.** `implemented`.

**Enforcement.** `md_harness.rs::wp_heading_matches_doc_heading` (painted h1/h2 line
boxes equal across Doc and WP). NC: WP h1 back to 26px → 34 vs 36.5, RED.

### UXI-Typography-3 — List markers hang

**Statement.** A list's markers sit right-aligned in ONE gutter sized to the list's
widest marker (`TYPE_SCALE.list_gutter`), so every item's text — and every wrapped
continuation line — starts at the same x, right of the marker, never under it.

**Applies to.** `render_blocks.rs` — `block_inner` List arm (gutter width),
`list_item_element` (gutter + text column).

**Status.** `implemented`.

**Enforcement.** `md_harness.rs::list_markers_hang_in_a_shared_gutter` (`9.` / `10.`
items at 2× zoom: texts align, wrapped item stays right of its marker). NC: per-item
`min_w(24)` marker → 428 vs 445, RED.

### UXI-Typography-4 — A blockquote has a full-height left rule

**Statement.** A blockquote paints a 3px left rule spanning the full height of the
quoted blocks, and the quoted text wraps within the column.

**Applies to.** `render_blocks.rs` — `block_inner` BlockQuote arm (`flex_none`
stretched rule; `flex_1`/`min_w_0` content).

**Status.** `implemented` (the rule previously painted 0×0 and long quotes did not
wrap).

**Enforcement.** `md_harness.rs::blockquote_bar_spans_the_quote`. RED on the old
code: bar `0x0`.

### UXI-Typography-5 — Code blocks show their language and copy their text

**Statement.** A fenced code block in the Doc view has a muted, right-aligned header
with its language (when given) and a **Copy** button. Clicking Copy writes the
block's exact plain text (lines joined by `\n`) to the clipboard and shows "Copied"
for ~1.5s. The click resolves the block by structural path in the tile's current
`DocState` at event time (yux rule 4), works for nested code blocks, and does not
start a text selection.

**Applies to.** `render_blocks.rs` — `code_copy_button`, `block_at_path`,
`code_block_text`, `RenderCtx::code_copy`; `doc_ui.rs::copy_doc_code_block`;
`DocState::code_copied{,_seq}` (in `DocSeqs`).

**Status.** `implemented`.

**Enforcement.** `md_harness.rs::code_block_copy_button_writes_the_code` (real click
at the painted button; top-level and nested block). NC: skip the clipboard write →
clipboard `None`, RED.

Exact look (spacing feel, colors, the label/button glyphs) is harness gap #1 —
`NEEDS-RUNTIME` human eye.
