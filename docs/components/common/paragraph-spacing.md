# Component: Paragraph Spacing (common)

**Status:** implemented
**Component token:** `ParagraphSpacing` (⇒ `UXI-ParagraphSpacing-N`)

## Description

The reading surfaces insert a small extra amount of **vertical space between
stacked blocks / paragraphs / list items** so paragraphs read as distinct chunks
and bullet lists get air between items. This is *option B* (per the `/new-ux`
interrogation): the gap grows **between** blocks and between list items — the
leading **within** a paragraph (soft-wrapped lines of one block) is unchanged, so
prose stays dense but paragraphs are broken up.

Applies to the three reading surfaces:

- **Doc view** (`Buffer::Viewing`, rendered markdown) — the gap between top-level
  `RenderedBlock`s, and between items of a `RenderedBlock::List`.
- **Agent transcript** (`AgentTile` `TranscriptView`) — three places: the gap
  between `FlatItem::Block`s (markdown blocks), between list items inside them (the
  transcript shares `block_inner`, so the list gap is the same one), and — for plain
  **prose paragraphs** — top padding on the line that STARTS a new paragraph. The
  transcript's **blank-collapse pass drops blank lines from the flat items**, so
  `\n\n`-separated agent/user paragraphs would otherwise render *adjacent*; the
  paragraph-start line is detected by reading the raw source (`lines_snap`), which
  still holds the collapsed blank, so the gap survives the collapse.
- **Edit view — Word Processor** (`Buffer::Editing`, `EditView::WordProcessor`) —
  the paragraph-break blank line and the space above bullet/ordered list items.

Explicitly **out**: the **Code / RAW** edit sub-view (source lines behind a
line-number gutter — spacing them fights the gutter model), the **compose box**
(an input, not presented prose), and all **chrome** (tab strip, status bars,
gutters, rails). Within-paragraph wrapped lines stay tight on every surface (a
soft-wrapped line is a single `build_wrapped_line` item, so wrapping never inserts
the gap).

The gap **scales with the document text-zoom** (`text_scale`, see
[TextZoom](text-zoom.md)) so it stays proportional as you zoom.

## References

- `docs/components/buffer.md` — Doc view + WP are Buffer sub-views.
- `docs/components/agent-tile/transcript.md` — the transcript consuming this.
- `docs/components/common/text-zoom.md` — the `text_scale` this multiplies by.

## UX invariants

### UXI-ParagraphSpacing-1 — Extra vertical space breaks up paragraphs and list items

**Statement.** On the reading surfaces (Doc view, agent transcript, WP edit),
consecutive **blocks / paragraphs** and consecutive **list items (bullets)** are
separated by a readability gap that is **strictly larger** than the leading
between two soft-wrapped lines *within* one paragraph, and strictly larger than
the pre-change block gap. The extra space is a shared constant
(`PARAGRAPH_GAP_PX`, `render_blocks.rs`) added at each between-block / between-item
site and **multiplied by `text_scale`** so it scales with zoom. Leading within a
paragraph is untouched; the Code/RAW sub-view, the compose box, and all chrome
render at native spacing.

**Applies to.**

- `render_blocks.rs`: `PARAGRAPH_GAP_PX` + `paragraph_gap(text_scale)` (the shared
  helper); `block_element` (Doc-view inter-block bottom **padding** `pb`); the
  `RenderedBlock::List` arm of `block_inner` (per-item flex `gap`, shared by Doc
  view + transcript).
- `transcript_view.rs`: the `FlatItem::Block` wrapper **padding** `pt`/`pb`; AND the
  `FlatItem::Line` prose **paragraph-start** top padding (`is_paragraph_break` = a
  frozen non-blank line whose previous `lines_snap` line is blank), added as
  `pt(2 + gap)` over the row's base `py(2)`.
- `screens.rs` `build_edit_body_wp`: the `WpLineKind::Empty` paragraph-break row
  height, and a `top_pad` on `WpLineKind::BulletItem` / `OrderedItem` (all scaled).

**Padding, not margin (load-bearing).** The Doc view and the transcript stack their
blocks in a virtualized `gpui::list`, which **ignores item margins** — it measures
and stacks each row by its box size, so a `.mb(...)` produces *no* visible gap (the
pre-existing doc `mb_2` and transcript `mt(4)/mb(4)` were dead spacing). The gap is
therefore applied as vertical **padding**, which is part of the measured box. List
items (bullets) are the exception: they live in a real flex column inside one block,
so a flex `gap` works there.

**Why.** The user asked for "a few extra pixels between every newline … to break
up paragraphs" and, explicitly, space between bullet points — for readability.
Doc-view paragraphs are distinct `RenderedBlock`s but the 8px gap read as too
tight, and list items had **no** vertical gap at all; the transcript's markdown
blocks were 4px apart. A first pass assumed transcript prose *paragraphs* already
carried a blank-line gap and left them alone — but the user's screenshot showed
them running together, and inspection confirmed the **blank-collapse pass**
(`agent.rs`) removes the blank `FlatItem::Line`, so paragraphs render adjacent.
Prose paragraphs therefore get an explicit paragraph-start gap. Within-paragraph
soft breaks (single `\n`) stay tight, so it is still option B, not option A.

**Bounds.** Transcript **prose** bullets only get the item gap when the agent's
markdown is parsed into a `RenderedBlock::List` (rendered as a `FlatItem::Block`);
a list left as raw source `FlatItem::Line`s is unaffected. The Doc view — the
primary reading surface — always parses lists into blocks. The prose paragraph-start
gap is gated on **frozen** (committed) lines, so the live draft/compose is excluded.

**Status.** `implemented` (headless — the painted inter-block gap is probed).

**Deviation from plan.** (1) Planned as `margin`; shipped as `padding` — testing
revealed `gpui::list` ignores item margins (see "Padding, not margin" above), so the
original `mb`/`mt` approach produced 0px and the code + test both moved to padding.
(2) The enforcement test recovers the gap as `(row-slot height − content height)` on
a single block rather than as the distance between two blocks' bounds, because the
list absorbs the padding into the row's slot height (adjacent rows paint back-to-back
at 0px separation). (3) WP heading `top_pad` now also multiplies by `text_scale` (it
was fixed px) — a small consistency win folded in so all WP vertical spacing scales
with zoom together. (4) Transcript prose paragraphs were initially left out (bad
assumption that blank lines survive); added after the user's screenshot, via
paragraph-start detection over `lines_snap` because the blank-collapse pass removes
the blank line. Magnitude (`PARAGRAPH_GAP_PX = 6.0`) is a first cut, tunable.

**Enforcement.** Two `verify_harness.rs` tests, each probing PAINTED bounds:
1. `paragraph_gap_between_doc_blocks_exceeds_within_paragraph_leading` (Doc blocks) —
   at 1× zoom, probes `doc-block-0` (block row, slot includes the bottom padding) and
   `doc-block-inner-0` (content column, excludes it) and asserts the recovered gap
   `slot_h − content_h ∈ [12, 24]px` (shipped 14 = `8 + PARAGRAPH_GAP_PX`). NC (RED):
   swap the `pb` back to `mb_2` → the list drops the margin → recovered gap `0px`.
2. `transcript_paragraph_start_row_is_taller_than_within_paragraph_row` (transcript
   prose) — commits a two-line paragraph α, a blank line, then paragraph β; probes
   `transcript-row-1` (within-paragraph soft break, no gap) vs `transcript-row-3`
   (paragraph start) and asserts the start row is taller by ~the gap. NC (RED):
   remove the paragraph-start `.pt` → both rows are 25px and the delta assert fails.

The exact pixel feel on WP / transcript is the paint gap (#1, human eye); the shared
constant is tunable in `render_blocks.rs`.
