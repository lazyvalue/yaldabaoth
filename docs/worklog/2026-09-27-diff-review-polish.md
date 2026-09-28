# Worklog: diff-review-polish

**Date:** 2026-09-27 · **Graph:** `kfa` · **Branch:** `kfa` (+ `picker-labels`, `code-font`) → `main` (`dc75539`)

## Shipped
- Worktree picker rows paint branch name + "subject · age · ~/path" (bug-0072: gpui nowrap text measured at width 0 was permanently ellipsized; `single_line_ellipsis` now line-clamps) — `diff_picker_rows_paint_label_and_description`, observed RED on `ab488e2`.
- Code font chosen from installed fonts, JetBrains Mono first (was macOS-only SF Mono/Menlo); optional `code_font` preference — `choose_code_font_*` unit tests.
- GitHub-style inline comment compose under the anchor line (moves with scroll, render-flat typing); boxed comment cards, no emoji — `diff_compose_inline_tracks_scroll`, `diff_comment_card_paints_bordered_box_without_emoji`.
- Expand context above/between/below hunks (`↑/↓ 20 more lines`, show all, `+` around the cursor's hunk); revealed lines numbered, commentable — UXI-Diff-18 guards.
- Syntax highlighting for Rust, TS/TSX (via `two-face`), Markdown; whole-file, async, cached per (path, side, file_hash, theme); re-highlights on theme switch — UXI-Diff-19 guards.
- Final: `cargo test --lib` 259 ✓, `--bin yalda-gpui` 889 ✓, `--features test-support` 901 ✓; release build ✓. Every node's guards observed RED with the fix reverted (Cog node outputs).

## Caveats
- NEEDS-RUNTIME gap 1: colors of highlights, card box, expander rows, picker description on real Linux fonts are layout-verified only.
- Running GUI not restarted; activation is Scott's restart.
- `cargo mutants` not run (not installed locally); CI mutation-gate covers.
- Diff lines still clip at row width (no wrap / h-scroll); draft compose wraps at 88 cols.

## Decisions
- None new (ADR-0040 stands; TS grammar via `two-face` noted in UXI-Diff-19).

## Cog
- Status: `complete`

```text
graph diff-picker-names-and-code-font (frontiers)
frontier 0: code-font [done], picker-labels [done]
frontier 1: comment-inline [done]
frontier 2: context-expand [done]
frontier 3: syntax-highlight [done]
frontier 4: ship [done]
frontier 5: omega [done] (omega)
```
