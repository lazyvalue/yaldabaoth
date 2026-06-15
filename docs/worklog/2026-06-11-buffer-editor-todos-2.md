# Worklog: buffer-editor-todos-2

**Date:** 2026-06-11
**Branch:** buffer-todos-2 (commit 10ef289) — three more Buffer TODO items from untitled.md

Follow-on to `2026-06-11-buffer-editor-todos.md` (the `p`/`P`, `<num>g`, paging,
file-rename batch). This session cleared three of the six remaining unchecked
Buffer TODOs.

## Built (with status)

- **`r{char}` replace-char (normal mode)** — new
  `EditorView::replace_char_at_cursor` deletes the char under the cursor and
  inserts the replacement in **one undo group** (respects the same frozen-line /
  lockable guards as delete+insert), leaving the cursor on the replaced char.
  No-op on empty line / past end-of-line. Stateful 2-key chord handled in the
  Edit screen's `dispatch_normal` via a new `EditState.pending_replace` flag:
  `r` arms it, the next keypress is consumed as the replacement (Esc / non-char
  cancels). Kept off the shared `dispatch_normal_core` path so the Agent/Claude
  surfaces are untouched. 3 editor unit tests (swap+position, single-undo,
  empty-line no-op). Builds + lib tests pass.

- **Vim default-register deletes (delete → yank buffer)** — `d` (delete-
  selection), `c` (change-selection), `x`/`delete-char`, and `dd`/`delete-line`
  now copy the about-to-be-deleted text to the clipboard (yalda's yank buffer)
  **before** deleting, so a subsequent `p`/`P` puts it back — matching vim's
  unnamed register. New `yank_then_delete_selection` + `char_under_cursor`
  helpers in edit_ui.rs; the four dispatch arms route through them. Builds +
  tests pass.

- **Visual-selection highlight on blank / whitespace-only lines** — the syntax
  highlighter yields **no segments** for whitespace-only lines, and a blank line
  fully inside a multi-line selection projects to a zero-width column range, so
  `apply_selection_bg` painted nothing and the selection read as an
  un-highlighted gap. New pure `apply_line_selection` (render_blocks.rs) emits a
  highlighted placeholder space when the line is blank **and** any part of it —
  or its trailing newline (selection continuing onto a later line) — is
  selected; a blank line that's merely the zero-width *end* of a selection stays
  un-highlighted (matches vim). Both the Code and WP render closures route
  through it, replacing the inline `apply_selection_bg` guard. 4 bin unit tests
  (interior blank, whitespace-only, end-of-selection, outside-selection).

## Verification

- `cargo build --bin yalda-gpui` clean (only pre-existing dead-code warnings).
- `cargo test --lib` → 134 passed; `cargo test --bin yalda-gpui` → 174 passed.
- **Runtime check still needed** (GUI can't be driven headlessly): eyeball the
  blank-line selection highlight, the `r` chord, and that `p` after a delete
  pastes the deleted text. Logic is unit-covered; the on-screen behavior is not.

## Wordwrap + cursor-offscreen (integrated, branch buffer-wordwrap)

The sibling `buffer-wordwrap` worktree held uncommitted WIP (from earlier the
same day) implementing soft-wrap. Committed it (`d865e3e`), then merged
buffer-todos-2 into it (clean auto-merge) so the render-path rewrite and the
blank-line selection-highlight fix land together rather than colliding.

- **Wordwrap in both edit views.** Code + WP body builders now route through the
  chatbox's `build_wrapped_line` (generalized with a `line_font` param so WP
  uses its proportional font while keeping the monospace code-bg fallback).
  Tokens break at whitespace and stack below the gutter; the caret is an inline
  flex child so it wraps with the text. `overflow_x_hidden` on the body clips
  the rare unbroken token. Deletes the now-unused `build_line_content`.
- **Both cursor-offscreen bugs subsumed.** Their shared root cause was the
  non-wrapping line pushing the inline caret off the right edge with no
  horizontal scroll ("visible cursor position is not the same as edit cursor").
  With the caret as an inline run that wraps + `overflow_x_hidden`, the caret
  stays on-screen. The earlier `list_state.reset()`-scroll hypothesis was a red
  herring — the complaint was horizontal, not vertical.
- **Compatibility check:** `apply_line_selection`'s blank-line placeholder
  (`[(" ", bg)]`) survives `build_wrapped_line`'s tokenizer as a single
  whitespace token (it only skips truly-empty segments), so the whitespace-line
  highlight renders through the new wrapped path.

## Delivered

- Fast-forward-merged `buffer-wordwrap` (which now contains all six items) into
  **main** (`abfcb0d`). Build clean; **134 lib + 174 bin tests pass** on main.
- **Every Buffer TODO in untitled.md is now ticked.**
- **Runtime check still owed** (no headless GUI): soft-wrap of long lines, caret
  staying visible at line ends / after `o`/`i`, blank-line selection highlight,
  the `r` chord, and post-delete `p`.

## Artifacts

- untitled.md: all Buffer TODO boxes ticked (the three logic items + wordwrap +
  both cursor-offscreen bugs).
- Worktrees `buffer-todos-2` and `buffer-wordwrap` are now merged into main and
  can be removed (`git worktree remove`).
