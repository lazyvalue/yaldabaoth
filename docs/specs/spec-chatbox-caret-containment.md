# Chatbox Caret Containment

**Status:** PARTIALLY SUPERSEDED — the **horizontal axis is RETIRED**; the
**vertical axis remains in force.**

> **Horizontal axis superseded (2026-06-25).** The compose now **word-wraps**
> (`spec-ux-invariants.md` INV-UX-2: `wrap_line_cols` /
> `build_chatbox_wrapped_line`), so there is no off-screen-right text to scroll
> to and **no horizontal window**. The `left_col` / horizontal `compute_window`
> half described below is no longer used by the compose render — long lines flow
> onto the next visual row instead of scrolling sideways. Treat every
> "horizontal" / `left_col` / `visible_cols`-as-scroll passage below as
> HISTORICAL. The **vertical** caret-containment (`compute_window`'s `top_line`,
> the splice-anchored list, no `reset()`) is unchanged and still load-bearing,
> and `visible_cols` survives only as the **wrap width**.
>
> The VERTICAL design below is IMPLEMENTED, runtime-unverified (GPUI paint can't
> be driven headlessly); per the 15× prior regressions it is not "fixed" until a
> human runtime check confirms it live.

**Last updated:** 2026-06-25

## Builds On

- **spec-yux.md** (`yux/list.rs`) — `compose_first_visible_line` (the vertical
  window function) and `ScrollAnchoredList` (the splice-anchored `gpui::list`
  the compose box paints into). This spec promotes that vertical function to one
  of a *pair* of axis-window functions and forbids any other source of scroll
  offset. WHY/HOW: the existing function is half the fix already shipped; the
  recurrence is the *other* half (horizontal) plus a line model that invalidates
  the vertical half.
- **spec-agent-presentation.md** — the agent-tile render path and the
  `TranscriptSeqs` fingerprint discipline (every render input has a covering
  seq, never notify in render). The compose surface obeys the same rule: its
  caret-containment inputs are all in its fingerprint. WHY/HOW: a missed seq is
  exactly how a stale caret paints at an offset that no longer matches the
  scroll.
- **spec-textbox-compose.md** (HISTORICAL, TUI era) — the original compose
  feature on the crossterm surface. Superseded as a *surface* by the GPUI
  `Chatbox`; referenced only so a reader knows this spec is the GPUI successor,
  not a second compose box.

## Overview

The **compose surface** (the chatbox: the message-input box of an agent tile,
`Chatbox` in `agent.rs`, rendered in `screens.rs`) lets the **caret and typed
text scroll outside the visible box** — horizontally and/or vertically — so the
user types into a region they cannot see. This has been "fixed" and regressed
**15+ times**; it makes the app practically unusable.

**Root cause (the thing every prior fix missed).** The surface uses two
*contradictory* line models on the two axes:

- **Horizontal** overflow is "prevented" by **soft-wrap** — `build_chatbox_line`
  word-wraps the cursor line (`flex_wrap` over tokens) and other lines
  (`StyledText` native wrap). Wrapping makes a logical line occupy a *variable*
  number of visual rows, and a single unbreakable token still overflows the clip.
- **Vertical** scroll and virtualization assume the **opposite**: each logical
  line is exactly **one fixed-height (18px), non-wrapping row**
  (`compose_first_visible_line`, `ScrollAnchoredList` default item height 18px,
  `scroll_to_item`).

These cannot both be true. The instant any visible line wraps, the vertical math
under-counts the caret's true pixel-Y (pushing it below the window) and the long
token escapes the horizontal clip. There is **no single owner** of "the caret
cell is inside the visible box," so each fix corrected one model for one input
path (typing) and left the others (paste, newline-at-EOL, long token, resize,
zoom, resume) free to re-break it.

**Target model.** One coherent model — the canonical code-editor model the Edit
view already uses: **fixed-height, non-wrapping rows + horizontal scroll** — with
a **single caret-containment invariant** owned in one place. The named entities:

- **Compose grid** — the line/column grid: row height `line_h` (uniform),
  column advance `char_w` (uniform; the compose font is monospace, fixed 13px,
  zoom-invariant). No soft-wrap.
- **Box content extent** — `(box_w, box_h)`: the compose panel's *inner* size
  in pixels after padding/border, measured from the live panel bounds — NOT the
  window viewport. `visible_cols = floor(box_w / char_w)`,
  `visible_rows = floor(box_h / line_h)`.
- **Visible window** — `ComposeWindow { top_line, left_col }`: the top-left grid
  cell of the visible box. The single, *authoritative* source of truth for
  scroll on both axes (it replaces reading the top line back from the list
  anchor).
- **`compose_first_visible_line`** (SHIPPED) and **`compose_first_visible_col`**
  (DRAFT) — the two pure, axis-symmetric functions that compute the window.
- **`compose_window`** (DRAFT) — the one chokepoint that calls both and is the
  *only* code that decides scroll offset.
- **Caret-containment invariant** — after every mutation, the caret cell —
  including the caret glyph's full width — is inside the box on both axes
  (Behavior 2).

This spec keeps the compose panel rendered **inline** in `render_agent`
(`screens.rs`), as today — it does NOT depend on the (still-unbuilt) promotion of
the compose box to a yux `CachedView`. Correctness rests on GPUI re-rendering the
root every frame, so the window is recomputed from the current caret + measured
extent on every frame (Behavior 7). When the compose box is later promoted to a
`CachedView`, that migration MUST carry every containment input (Behavior 2's
cell coordinates + the extent) into its fingerprint, or the caret goes stale —
that obligation is recorded here but is out of this spec's scope.

## Behaviors

### 1 · One grid model [vertical SHIPPED · non-wrap-horizontal DRAFT]

Each logical line renders as exactly one row of height `line_h`. Columns advance
by a uniform `char_w` (monospace). The compose box **does not soft-wrap**: a line
longer than the box extends off the right edge and is reached by horizontal
scroll, never by wrapping. This makes "which cell is at the top-left of the box"
exact integer arithmetic on both axes — zero text measurement, the property that
makes containment hold *by construction* (the same reason the vertical half was
made measurement-free). Today the cursor line and `StyledText` lines wrap; this
behavior removes that.

### 2 · The caret-containment invariant [DRAFT]

After **any** mutation that can move the caret or change the visible extent, the
window satisfies, for the caret cell `(cursor_line, cursor_col)`:

```
top_line ≤ cursor_line  <  top_line + visible_rows
left_col ≤ cursor_col   ≤  left_col + visible_cols - 1
```

Both bounds are **exclusive at the far edge** (`cursor_col` may equal at most
`left_col + visible_cols - 1`). The column axis reserves the rightmost column for
the caret glyph: the caret is painted as a fixed-width block (`make_caret`,
`px(8.0)`) that is slightly *wider* than one 13px-monospace column advance
(~7.8px), so a caret allowed to sit in the last column would overflow the clip by
`caret_w - char_w`. Reserving one column keeps the whole caret block inside
`[0, box_w)`. The caret at EOL (one past the last char) is the binding case for
this reservation. The invariant is checked on the *cell* (line AND column), not
merely the line.

### 3 · Single owner of scroll offset [DRAFT]

The window is computed **solely** by `compose_window` (which calls the two pure
functions) from `(cursor, previous window, line/column counts, visible extent)`.
`ComposeWindow.top_line` is **authoritative**: the virtualized list is scrolled
*to* it (`scroll_to(ListOffset { item_ix: top_line, .. })`), and `top_line` is NOT
read back from the list's own anchor (`logical_scroll_top().item_ix`) — that
prior read is *replaced*, so there is exactly one owner of the vertical offset
(keeping both would re-introduce the two-owners disagreement after a splice).
Scroll offset is **never** set by GPUI's measurement-based
`scroll_to_reveal_item` (it derives offsets from cached/estimated row heights and
misfires on freshly-spliced unmeasured rows — the documented strand-the-caret
bug), nor by any per-input-path ad-hoc code. Both render paths (the small ≤8-line
direct path and the virtualized `gpui::list` path) read the **same**
`ComposeWindow` — no model divergence between them.

### 4 · Minimal, stable scroll [vertical SHIPPED · horizontal DRAFT]

On each axis the window moves **only when the caret would otherwise leave it**
(caret above/left → that line/col becomes the edge; caret below/right → it
becomes the far edge), and is clamped so it never scrolls past content into blank
space while still containing the caret. A caret already inside the window does
not move it (no jitter). This is the existing vertical behavior, mirrored to the
horizontal axis.

### 5 · Horizontal rendering [DRAFT]

This is a **rewrite** of `build_chatbox_line`, not a tweak: the existing
whitespace/non-whitespace tokenizer and `flex_wrap` exist *to make wrapping
work*, and they are removed. Each row becomes a single non-wrapping line that
clips to the box content width (`overflow_x_hidden`) and offsets its inner
content by `-left_col * char_w`, so the visible columns `[left_col, left_col +
visible_cols)` land in `[0, box_w)`. A long unbreakable token scrolls past the
edge rather than overflowing it. The caret glyph is still injected at the cursor
column, and its **Normal vs Insert semantics are preserved** — Normal paints the
block over the character at the cursor (that char is consumed by the caret cell),
Insert paints a zero-width beam before it (the char stays in the after-stream).
Selection highlight still rides the line as run backgrounds.

### 6 · Visible extent is itself an input [DRAFT]

`visible_cols = floor(box_w / char_w)` and `visible_rows = floor(box_h / line_h)`
are recomputed **each frame from the same-frame measured bounds of the compose
panel** — its inner content rect after `mx_2` / `px_4` / border, captured the way
the desktop canvas captures its bounds (a `Cell<Rect>` written during paint), NOT
from `viewport_width_px` (the whole-window width, which over-counts columns by the
split tree + sidebars + padding and would itself strand the caret). The extent
must be read from the measured bounds, not from a value updated only on a resize
*event* — otherwise the frame right after a resize computes the window against a
stale extent (a one-frame off-screen caret on exactly the resize path). The
extent-moving inputs are **window resize, tile resize, and rail (sidebar)
open/close**; text zoom and font changes do NOT affect the compose box (it renders
at a fixed 13px monospace, exempt from `text_scale` — chrome stays zoom-fixed), so
they are not extent inputs.

### 7 · Every input path is covered [DRAFT]

Because the window is *recomputed from the current caret and measured extent every
frame* (not maintained incrementally), the invariant holds after every path with
no per-path code: type at EOL, paste (multi-line and single long line), newline
(incl. at EOL of a full window), delete/backspace, cursor motion
(arrow/home/end/word/page), selection extension, programmatic set (resume / draft
restore), and window/tile/rail extent changes. The headless test (Constraint 4)
enumerates these explicitly.

### 8 · No notify-in-render [DRAFT]

The compose panel is rendered inline (see Overview), so it has no per-surface
fingerprint of its own today — the root re-renders every frame, which is what
makes Behavior 7's "recompute every frame" hold. The one yux rule that still
binds: the compose render path never calls `cx.notify()` (a notify mid-draw is
parked — the parked-notify bug). The future `CachedView` promotion inherits the
fingerprint obligation stated in the Overview.

## Data Model

```rust
/// The top-left visible grid cell of the compose box. The single source of
/// truth for scroll on both axes; recomputed every frame by `compose_window`,
/// read by both the list scroll (top_line) and the per-row horizontal offset
/// (left_col). Replaces "read top back from the list anchor" (vertical) and the
/// absence of any horizontal state.
struct ComposeWindow {
    top_line: usize,
    left_col: usize,
}
```

Stored on `Chatbox` (`agent.rs`), authoritative and owned by the compose surface.
`Chatbox` keeps its `editor`, `mode`, and `ScrollAnchoredList` (the list is still
the virtualized painter for long drafts; it is scrolled *to* `top_line`, it does
not *decide* it). `char_w` / `line_h` are constants of the monospace compose font
(`line_h = 18.0`); `char_w` is the font's advance, measured once.

## Interfaces

All module-internal (the compose render path in `agent.rs` / `screens.rs` calls
into `yux/list.rs`):

- **`compose_first_visible_line(cursor_line, prev_top, line_count, visible_rows) -> usize`**
  (SHIPPED) — minimal vertical window keeping `cursor_line` in
  `[top, top+visible_rows)`, clamped to content.
- **`compose_first_visible_col(cursor_col, prev_left, line_len, visible_cols) -> usize`**
  (DRAFT) — the horizontal mirror, keeping `cursor_col` in
  `[left, left + visible_cols - 1]` (the rightmost column reserved for the caret
  glyph, Behavior 2), clamped to `line_len` and floored at 0 via `saturating_sub`.
  Note the deliberate asymmetry vs the vertical function: vertical uses
  `< top + visible_rows` (exclusive), horizontal uses `≤ left + visible_cols - 1`
  (one extra column reserved). Copying the vertical impl blindly is an off-by-one
  trap; the difference is the caret-width reservation.
- **`compose_window(cursor, prev: ComposeWindow, counts, extent) -> ComposeWindow`**
  (DRAFT) — the **only** function that decides scroll offset; calls the two
  above and returns the new window. Every render builds the box from its result.

## Constraints

1. **Monospace, no soft-wrap.** Column→pixel is uniform `char_w`; the compose
   font is monospace by constraint. Soft-wrap is forbidden in the compose box —
   wrap (variable height) and uniform-row virtualization are mutually exclusive,
   and holding both is the root cause this spec eliminates. A future wrapping
   compose surface would be a *different* model and a *new* spec, not a patch
   here.
2. **No measurement-based caret reveal.** `scroll_to_reveal_item` /
   `scroll_to_item` must not be used to keep the caret visible (Behavior 3).
3. **Definition of done is the test, not the eyeball.** This bug has been
   declared fixed on "looks right when I type" 15+ times and regressed. This spec
   is not done until Constraint 4's test exists and is green, plus a human
   runtime check.
4. **Headless invariant test (the permanent guard).** A test in
   `verify_harness.rs` / `tests.rs` drives a real `Editor` through every
   Behavior-7 path and, after each, asserts the caret cell is inside
   `compose_window`'s result on **both** axes — for a range of `visible_rows` /
   `visible_cols` including the degenerate `1`, AND the boundary inputs
   `line_len = 0` (empty line, `cursor_col = 0` → `left_col = 0`) and
   `line_len < visible_cols` (→ `left_col = 0`, no underflow; the col function
   must `saturating_sub` like the line function). The math is headless even
   though GPUI paint is not; this pins the property directly, for every path, the
   way the existing vertical test pins `compose_first_visible_line`.
5. **Render-count guard (yux rule 5).** Since the compose panel is inline (not a
   cached child), the guard is the *converse*: a render-count test asserts that
   typing in the compose box does NOT re-render the cached transcript
   (`TranscriptView`) — its `perf_render_count` stays flat — so the rewrite
   doesn't accidentally couple the two surfaces.

## Revision History

- 2026-06-24 — Initial DRAFT. Names the root cause (two contradictory line
  models — soft-wrap horizontal vs uniform-row vertical — with no single owner of
  caret containment) behind the 15×-recurring "chatbox caret/text off-screen"
  bug, and specifies the fix: one fixed-row + horizontal-scroll grid model, one
  containment invariant on both axes, one scroll-offset owner (`compose_window`),
  and a headless test over every input path as the definition of done.
- 2026-06-24 — Revised per adversarial review. (B1) Box extent comes from the
  compose panel's *measured inner bounds*, not `viewport_width_px`; `char_w` is
  the measured monospace advance. (B2) Compose stays **inline** — dropped the
  cached-fingerprint language, recorded the fingerprint obligation as a future
  `CachedView`-migration constraint instead. (N1) Right column reserved for the
  8px caret block (`≤ left + visible_cols - 1`). (N2) Extent read from same-frame
  measured bounds, not an event-updated cache. (N3) Behavior 5 marked as a
  `build_chatbox_line` rewrite that preserves Normal/Insert caret semantics. (V1)
  Trimmed zoom/font from extent inputs (compose is zoom-fixed). (V2) Test covers
  `line_len = 0` and `line_len < visible_cols`; documented the inclusive/exclusive
  asymmetry. (V3) `ComposeWindow.top_line` is authoritative and *replaces* the
  list-anchor read.
