# bug-0072: diff-picker-rows-blank

**Status:** FIXED
**First seen:** 2026-09-27
**Component:** `docs/components/diff.md` (UXI-Diff-10); shared primitive `yux/detail.rs::single_line_ellipsis`

## Symptom

Live GUI (Linux/niri, release build of `ab488e2`): open an unbound Diff tile
(`cmd-d` / `ctrl-shift-d`). The worktree picker renders one row per worktree
(12 in Scott's repo, all on named branches), but "none of the worktrees have
names / descriptions" — the rows show the glyph and (for the primary) the badge,
but no branch label and no path text. Expected: branch name + a line saying what
the worktree is.

## Context / root cause

Manifest check: no prior picker-label bug (bug-0011/0042/0044/0048 are picker
focus/placement/close/label-content bugs, unrelated mechanisms).

`diff_picker_body` → `picker_option_row_detailed`, whose label and detail are
`single_line_ellipsis` leaves (`whitespace_nowrap` + `text_ellipsis`) stacked in
a `flex().flex_col().flex_1().min_w_0()` wrapper.

gpui 0.2.2's text leaf (`TextLayout::layout` measure closure) caches its first
taffy measurement whenever `wrap_width` is `None` — which is ALWAYS under
`whitespace_nowrap` — and derives the ellipsis truncation width from that first
call's `available_space.width`. Instrumented measure calls (a throwaway logging
leaf in the real nesting) showed taffy's call order:

- label inside the `flex_col().flex_1()` wrapper: `Definite(0px)` first, then
  `MinContent`, `MaxContent`, finally `Definite(1828px)`. First call wins ⇒ the
  text is truncated at 0px ⇒ shaped to a bare `"…"`, and never re-laid-out. The
  element BOX is full width (1416px in the harness), so every existing
  bounds-only guard (`diff_picker_lists_and_paints_worktrees`) was green.
- label directly as a flex item (`picker_option_row`, other pickers) or inside a
  block wrapper: `MinContent` first ⇒ no truncation, full text cached ⇒ visible
  but never ellipsized (just clipped by `overflow_hidden`). Latent; why other
  pickers "work".

Not fonts (title + other pickers use the same `body_font`/`code_font`), not
colors, not the scroll container.

## Planned solution

1. Guard on the REAL path that reads the SHAPED text, not just the box: new
   `probe_text` seam (`render_blocks.rs`) wraps a `StyledText` leaf and, after
   paint, records `TextLayout::text()` (post-truncation) + the leaf bounds.
2. Fix in the shared primitive: `single_line_ellipsis` uses `whitespace_normal()`
   + `line_clamp(1)` + `text_ellipsis()` instead of `whitespace_nowrap()`. Under
   normal whitespace gpui re-measures whenever the available width changes, so
   the final (real-width) pass decides the truncation; `line_clamp(1)` keeps one
   line.
3. Description line: `<HEAD subject> · <relative age> · <~/path>` via one batched
   `git log --no-walk=unsorted --format=%H%x00%ct%x00%s <heads…>` in the async
   `list_worktrees` (errors-as-values ⇒ path only).

## Approaches already tried (do NOT repeat)

- Replacing the `flex_col` wrapper with a block wrapper: label becomes visible
  (first measure is `MinContent`) but a too-long label is clipped with NO
  ellipsis — it just moves the cache bug. Rejected (guard
  `diff_picker_long_branch_label_ellipsizes_with_visible_prefix` RED).

---

## Log

### 2026-09-27 19:20 — localized on the real path, fixed in the primitive (graph kfa node tch0)

- Guard `verify_harness.rs::diff_picker_rows_paint_label_and_description`: real
  `open_diff_inner` → async `list_worktrees` on a tempdir fixture (primary
  `feature` + linked `topic`), forced repaint with the probe on; asserts per row
  that the label leaf's shaped text contains the branch, the detail's contains
  `<subject> · ` and the worktree dir, both leaves w>0/h>0 and inside the row.
  **RED on unfixed `ab488e2`:** `row 0 label shaped to no visible text: "…"`
  (leaf 1416px wide).
- Companion `diff_picker_long_branch_label_ellipsizes_with_visible_prefix`
  (240-char branch): label must end with `…` AND keep a visible prefix.
- Fix: `yux/detail.rs::single_line_ellipsis_box` → `whitespace_normal()
  .line_clamp(1).text_ellipsis()`; `picker_option_row_detailed` leaves go through
  `probe_text` (production: plain `SharedString`, one branch).
- Description: `diff_git.rs` `WorktreeEntry::head_commit` + `parse_head_commits`
  + batched `attach_head_commits`; `diff_view.rs::{relative_age,
  worktree_row_description}` (pure, `now` passed in) with unit tests.
- Negative controls: (1) fix reverted to `whitespace_nowrap` ⇒ both guards RED
  (`"…"`); (2) description reverted to path-only ⇒ guard RED
  (`row 0 detail must paint "two file change · ", painted "/tmp/.tmp…"`).
  Restored ⇒ GREEN. Full `cargo test --bin yalda-gpui`: 838 passed, 0 failed.
- Side effect (intended): every `single_line_ellipsis` caller (jump panel, diff
  footer, picker rows) now truly ellipsizes at its final width instead of
  clipping. Full suite green.
- Activation: NOT restarted — release binary rebuild + GUI restart is Scott's
  call (`NEEDS-RUNTIME`: final glyph appearance on the real font stack, gap 1).
