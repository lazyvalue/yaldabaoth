# bug-0017: cannot-select-code-blocks-in-worksheet

**Status:** FIXED
**First seen:** 2026-07-22 ("STILL CANNOT SELECT WITH MOUSE IN CODE BLOCKS IN AGENT WORKSHEET MODE")
**Component:** `docs/components/common/selection.md` (`UXI-Selection-3`) + agent-tile transcript

Sibling of [bug-0008](bug-0008-cannot-select-parsed-blocks-in-transcript.md)
(parsed blocks registered zero hit tokens) and
[bug-0015](bug-0015-code-block-shifts-under-the-pointer-on-click.md) (the block
reflowed +25px on click). The user stated all three are **the same bug**. They
are three *distinct* faces of one surface — code blocks render as a single
`FlatItem::Block`, which bypassed every prose selection mechanism. bug-0008 and
bug-0015 each fixed an INVISIBLE layer (hit geometry, reflow), so the user's
experience never changed — hence the recurrence.

## Symptom

Dragging the mouse across a multiline code block in the agent transcript
(Worksheet mode — "I ALWAYS USE WORKSHEET MODE") does nothing the user can see:
no highlight appears, and it feels like selection is broken. Prose selects fine.

## Context / root cause

Two independent defects, both from the block being one `FlatItem::Block` instead
of per-line `FlatItem::Line`s:

1. **No selection highlight is ever painted inside a block.** The `FlatItem::Block`
   render arm built its `RenderCtx` with `doc_selection: None` (and
   `current_block`/`line_layouts` `None`), and nothing else paints a selection
   background inside a block. The editor selection AND the copy-on-release both
   worked — so every headless band/clipboard probe (bug-0008, bug-0015) passed
   GREEN while the user saw *nothing*. (Localized by Fable.)
2. **The hit bands were misaligned with the glyphs.** `register_block_hits_on_paint`
   split the block's full outer height into N EQUAL bands over the raw line range —
   but `detect_block_ranges` ranges are **fence-inclusive** (```` ``` ````…```` ``` ````),
   while `block_inner` paints only the *content* lines plus a `p_2` pad and an
   optional `[lang]` header. So bands were divided by too many lines AND offset by
   the pad/header; a click on a glyph mapped to the wrong raw line.

The existing bug-0015 guard passed anyway because it drove its drag from the
REGISTERED band centers (not real glyph positions) and asserted clipboard, not
paint — so a uniform offset was invisible to it (anti-circling rule 3).

Secondary latent bug (fixed same commit): `block_ranges_by_item` paired
`FlatItem::Block` ordinals against `c.block_ranges` (ALL detected ranges), but a
Block is emitted only for a range that PARSED — an unparsed range above a code
block shifted every later block's hit range.

## Planned solution

Route transcript code-block content lines through the proven prose machinery:
per-line REAL-bounds hit registration (`register_token_on_paint`) + run-background
selection painting (`apply_selection_bg_to_runs`), keyed by the correct raw doc
line (`raw_base = range.start + 1`, skipping the opening fence). One mechanism
fixes both alignment and visibility and is immune to the block's padding/header, so
it can't recur. Tables keep the even-split path (not reported).

## Approaches already tried (do NOT repeat)

- **bug-0008** per-raw-line even-split hit bands (`register_block_hits_on_paint`):
  necessary for tables but WRONG for code — fence-inclusive + padding/header offset,
  and it never painted a highlight. Superseded for code blocks by the per-line
  real-bounds path.
- **bug-0015** `drag_protect_line` reflow freeze: correct and still live, but it
  only froze the item count — it was never the whole symptom. Re-doing it won't help.

---

## Log

### 2026-07-22 — per-line real-bounds hits + run-bg selection for code blocks (FIXED)

**Root-caused** with Fable (the missing-paint defect) + direct code reading (the
fence-inclusive/padding misalignment). Confirmed the passing bug-0015 guard was
vacuous to both (drives band-center clicks, asserts clipboard not paint).

**What changed.**
- `render_blocks.rs`: new `RenderCtx::block_hits: Option<BlockHits>`
  (`{ sink, raw_base, selection, sel_bg }`). `doc_styled_line_element` now honors
  it: for a transcript code line it registers a `TokenHit` from the line's OWN
  painted bounds and applies the selection bg to the run range from
  `line_selection_range(raw_base + li)`. Early-return-to-plain now also checks
  `block_hits`.
- `transcript_view.rs`: the `FlatItem::Block` arm sets `block_hits` for a fenced
  `CodeBlock` (`raw_base = s + 1`) and SKIPS `register_block_hits_on_paint` for it;
  tables keep the even-split path. `block_ranges_snap` now snapshots **parsed-only**
  ranges from `resolved_blocks` (was `c.block_ranges`, all detected).
- `main.rs`: `DocRenderTap.block_selection: Vec<(raw_line, s_char, e_char)>` — the
  paint tap the guard asserts against.

**How verified.** `verify_harness.rs::code_block_selection_is_painted_and_aligned`
(REAL `transcript_mouse_down`/`_move`/`_up`, worksheet transcript, ```` ```rust ````
block with a language header): asserts (a) hit bands cover content lines 1..=3 and
NOT the fence lines 0/4, (b) the selection highlight PAINTED on the dragged lines
(via `DocRenderTap.block_selection`), (c) release copies the code.

**Negative controls, each observed RED for the right reason:**
- Set `block_hits: None` at the block arm ⇒ bands `{0,1,2,3,4,5}` (fence lines
  reappear) ⇒ assert (a) fires.
- Disable `apply_selection_bg_to_runs` ⇒ "no selection highlight was painted inside
  the code block (bug-0017)" ⇒ assert (b) fires.

Updated bug-0015's `code_block_does_not_shift_when_clicked` non-vacuity assert
(content-line bands, not fence-inclusive four). Suites: **405 bin + 157 lib green.**

**Shipped:** committed to `main` (anti-circling rule 5) + release binary rebuilt.
Promoted to **`UXI-Selection-3`**.

**Unverified.** Live macOS drag delivery (gaps #1/#4) — needs the user's eye on the
rebuilt release app. Tables (the other `FlatItem::Block`) still have no in-cell
highlight — not reported; tracked as follow-up.
