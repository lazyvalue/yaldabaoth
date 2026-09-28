# Component: Buffer

**Status:** living
**Component token:** `Buffer` (⇒ `UXI-Buffer-N`)

## Description

`App::Buffer(BufferApp)` — a tile that is a view onto the shared file-buffer pool
(ADR-0007), always in exactly one `BufferMode`. `Viewing ⇄ Editing` toggle over the
same pooled `SharedCore`; `Picking` is reached via `Cmd+O` (Buffer-scoped). The
three modes:

- **`Picking` (Browser view, `BrowserView`)** — file/buffer browser: directory
  navigation, parent, hidden-file toggle, sort cycle, worktree mode, filter input.
- **`Viewing` (Doc view, `YaldaView`)** — rendered markdown, block-by-block: a left
  orange cursor-bar on the focused block; `j/k`/arrows move block focus; `g`/`G`
  top/bottom; page scroll; wiki-links; marks. Built from `RenderedBlock`s.
- **`Editing` (Edit view, `EditView`)** — raw markdown source in two submodes toggled
  from the Buffer tile menu: **Code (RAW)** — monospace, line-number gutter, `md_highlight`
  source colors; **WordProcessor (WP)** — proportional font with per-line
  typographic classification (`classify_wp_line`). Vim-style Normal/Insert submodes
  (`AppMode`).

Buffer and Agent are orthogonal — a Buffer tile never nests an agent, and vice
versa. Primary code home: `screens.rs::render_doc` / `render_edit` /
`render_browser`, `doc.rs` / `doc_ui.rs` / `doc_view.rs` (Doc state, methods,
cached body `DocView`), `edit_ui.rs` / `edit_view.rs`, `browser_ui.rs`,
`render_blocks.rs`.

## References

- `docs/specs/spec-tiles-and-apps.md` — `App::Buffer` and the tile/app model (ADR-0019).
- `docs/components/common/text-editing.md` — the Edit view obeys `TextEditing`.
- `docs/components/common/text-zoom.md` — the Doc/Edit views obey `TextZoom`.
- `docs/components/common/paragraph-spacing.md` — the Doc view + WP obey
  `ParagraphSpacing` (`UXI-ParagraphSpacing-1`).
- `docs/components/common/diagram.md` — a `mermaid` fenced block renders inline as
  its diagram image in the `Viewing` (Doc) view (`UXI-Diagram-1`).

## UX invariants

- **`UXI-Buffer-1` (fuzzy find is name-scoped and prunes build output).** The
  `Picking` view's filter is a recursive fuzzy find rooted at the browser's
  `current_dir`. It matches a **subsequence of the filename** (or the whole
  relative path only when the query contains `/`), never a substring of the full
  path — so a match reflects the file's name, not every ancestor directory it
  sits under. It **never descends** into build-output / dependency-cache / VCS
  directories (`target`, `node_modules`, `.git`, `dist`, `build`, `vendor`,
  `__pycache__`, … — see `IGNORED_DIRS`). Results rank by fuzzy score (boundary /
  contiguous matches first), then shorter path, then name. Rationale: matching
  the full path and walking `target/` made the finder slow and swamped with
  irrelevant hits. Guard: `file_browser.rs`
  `search_skips_ignored_dirs_and_matches_filename_not_path`,
  `fuzzy_score_requires_subsequence_and_ranks_boundaries`.
- **`UXI-Buffer-2` (the picker remembers where you were).** The `Picking` view's
  cursor lands on the entry you just left, not the top of the list: (a) opening
  the picker from a file-backed buffer (`Cmd+O` → `open_browser_inner`) selects
  that file's row; (b) going to the parent directory (`FileBrowser::go_parent`,
  `h`) selects the child directory you came from. Both go through
  `FileBrowser::select_path` (a no-op while filtering or if the path is not a row
  in the listing). Rationale: in-and-out navigation should keep your place.
  Guards: `file_browser.rs` `go_parent_lands_on_the_child_dir_just_left`,
  `select_path_lands_on_the_named_file`; `verify_harness.rs`
  `open_picker_lands_on_the_file_just_left`.
- **`UXI-Buffer-3` (reload invalidates every derived view).** Reloading a
  file-backed Buffer replaces the shared core's text for every tile viewing or
  editing that file, and advances the core's monotonic content generation. No
  derived snapshot keyed by that generation—rendered blocks, extracted source
  lines, RAW/WP syntax spans, or WP line kinds—may survive the replacement.
  Rationale: rebuilding the core with a fresh generation of zero could alias an
  existing cache key, leaving old text and an old open-fence style painted after
  reload. Guard: `verify_harness.rs`
  `buffer_reload_does_not_reuse_old_syntax_state`; generation seam:
  `editor.rs::replace_text_preserves_monotonic_content_generation`.
- **`UXI-Buffer-4` (open buffers track the disk; a clean buffer follows external
  edits).** Every file in the buffer pool is watched (its parent directory,
  non-recursively, so an external editor's temp-file + rename write is seen);
  closing the last view of a clean buffer drops it from the pool and unwatches
  it. When a watched file changes on disk and its buffer is **clean**, the buffer
  reloads silently: every Edit view of the file keeps its caret line/col
  (clamped to the new text) and its scroll position, and the reload advances the
  content generation so every derived view repaints (`UXI-Buffer-3`). Watcher
  events are debounced (~100 ms) and the file is read on the background executor
  — the watcher never blocks paint. Status: implemented (`file_sync.rs`). Guards:
  `verify_harness.rs` `file_sync_external_write_reloads_clean_buffer_preserving_caret`,
  `file_sync_unwatches_closed_buffer`; `file_sync.rs`
  `os_watcher_forwards_only_tracked_files`.
- **`UXI-Buffer-5` (an external change never clobbers unsaved work).** When a
  watched file changes on disk while its buffer is **dirty**, the buffer text is
  left untouched and the buffer enters a *disk conflict*: the tile status bar
  (Doc and Edit) shows "changed on disk — space k keep mine · space R reload
  theirs". While conflicted, neither autosave nor `Ctrl-S` writes the file
  (`Ctrl-S` reports "not saved: changed on disk …"). The Buffer tile menu
  resolves it: **`k` keep mine** clears the conflict so the next save (manual or
  the autosave it re-arms) overwrites the disk version; **`R` reload theirs**
  replaces the buffer with the disk text (carets clamped, buffer clean). An
  explicit `r` reload from disk also clears the conflict. Status: implemented.
  Guard: `verify_harness.rs`
  `file_sync_dirty_buffer_external_change_conflicts_and_is_not_clobbered`.
- **`UXI-Buffer-6` (autosave).** A dirty, non-conflicted file buffer is written
  ~1 s after its **last** edit (each edit restarts the clock), immediately when
  its tile loses focus, and when the OS window deactivates. Writes are atomic —
  a temp file in the same directory renamed over the target — so no reader ever
  sees a half-written file and no temp file is left behind. Status: implemented.
  Guard: `verify_harness.rs` `file_sync_autosave_writes_after_idle_and_on_focus_loss`.
  (OS-window deactivation is wired in `main()` via `observe_window_activation`
  and is not headlessly exercised.)
- **`UXI-Buffer-7` (our own writes are not external changes).** Every save
  (manual or autosave) records the hash of the content it wrote; a watcher event
  whose disk content matches the last loaded/written content is our own echo
  (or a no-op touch) and is ignored — it neither reloads the buffer nor raises a
  conflict, even if the user kept typing after the save. Status: implemented.
  Guard: `verify_harness.rs` `file_sync_own_write_echo_does_not_reload_or_conflict`.
- **`UXI-Buffer-8` (Viewing → Editing keeps your place).** Toggling a
  source-mapped Doc into Edit (`Ctrl-E` / `Ctrl-Shift-E` / the `enter-edit` /
  `enter-wp` menu entries — all `enter_edit_with`) lands the caret at column 0 of
  the **focused block's first source line** (`spans[cursor_block].lines.start`),
  and the edit list starts with the **top visible block's first source line** as
  its top row. The caret is then guaranteed painted inside the edit viewport
  (UXI-TextEditing-1 wins): raw rows are usually taller than the rendered blocks
  (blank separator lines, monospace wrap), so when the caret would fall below the
  fold the list settles down only as far as needed to show it. Unmapped Docs
  (`spans` empty — string-backed help/welcome) keep the old top-of-file landing.
  Mechanism: `ScrollAnchoredList::land(top, focus)` + `settle()` (a fresh list's
  rows are unmeasured until laid out once; the follow-up frame is scheduled via
  `cx.defer`, never a notify in render). Status: implemented. Guards:
  `md_harness.rs` `doc_to_edit_lands_caret_on_focused_block`,
  `doc_to_edit_keeps_top_block_on_top_when_caret_fits`.
- **`UXI-Buffer-9` (Editing → Viewing keeps your place).** Leaving Edit
  (`Ctrl-V` / the `back-to-doc` menu entry — `back_to_doc`) sets the Doc cursor
  to the block holding the caret line (`Rendered::block_at_line`; blank lines
  between blocks map to the preceding block) and makes the block holding the top
  visible edit line the Doc's top block; the cursor block is guaranteed painted
  inside the Doc viewport (same land/settle). Status: implemented. Guard:
  `md_harness.rs` `edit_to_doc_lands_cursor_on_caret_block`.
- **`UXI-Buffer-10` (a no-op round trip is exact).** Doc → Edit → Doc with no
  edit and no caret motion in between returns to the same cursor block **and**
  the same Doc scroll offset (top block + in-block offset), even when the Edit
  landing had to settle its top to show the caret. The Doc position is stashed on
  the `EditState` (`doc_return`, keyed on `edit_seq` + caret) and restored
  verbatim; any edit or caret move falls back to UXI-Buffer-9's mapping. Status:
  implemented. Guard: `md_harness.rs` `doc_edit_doc_round_trip_keeps_place`.
- **`UXI-Buffer-11` (the Doc view renders GFM constructs, not their source
  residue).** In `Viewing` (and wherever `block_inner` renders markdown, e.g. the
  agent transcript): a task item (`- [ ]` / `- [x]`) shows a checkbox in place of
  its bullet (done items dimmed; in the Doc view the box is addressable as
  `md-task-<block>.<item…>` and toggles — UXI-Buffer-12); an image alone in its
  paragraph paints the picture — relative paths from the document's directory,
  `http(s)` via the app HTTP client — never wider than the column, aspect kept,
  alt text while loading or on failure (an image inside prose is its alt text as
  a link); a hard break (two trailing spaces / `\`) starts a new line within the
  paragraph; table columns honor `:--` / `:-:` / `--:`; `[^n]` footnote
  references paint as link-styled superscript markers and their definitions as a
  de-emphasized block. Status: implemented (graph 4f1 `render-fixes`). Guards:
  `md_harness.rs` `task_list_items_paint_checkboxes`,
  `images_paint_fitted_to_the_column`, `hard_breaks_paint_separate_lines`,
  `table_columns_honor_alignment`, `footnote_definitions_paint_as_blocks`;
  `render.rs` `render_fixes_tests::*`.
- **`UXI-Buffer-12` (task checkboxes toggle from the Doc view).** In `Viewing`
  on a file-backed Doc, a task item's state flips `[ ]` → `[x]`, `[x]`/`[X]` →
  `[ ]` in the **source** by: (a) a left **click on its painted checkbox** —
  exactly that item, at any nesting depth; or (b) **`x`** (`ToggleTask`,
  YaldaView) on the focused block — the block's **first open** task (document
  order, nested items included), or, when every task in it is done, its **last**
  task (so repeated `x` checks a list off top-down and one more `x` walks it
  back; `x` on a block with no task does nothing). Space is NOT the gesture —
  it is the App leader (ADR-0032). The edit goes through the shared pooled
  buffer as **one undo step** (`u` in any Edit view of the file undoes it), marks
  the buffer dirty (autosave, UXI-Buffer-6), leaves every other byte untouched,
  moves the Doc cursor to the toggled block, and the Doc repaints the new state.
  A `[ ]` inside code / a table / non-task text is never a target (markers come
  from the same parser the renderer uses). String-backed Docs (help/welcome)
  are read-only: a status line says so. Mechanism: `yalda::task_list`
  (`task_markers_in` over the block's `SourceSpan`, `task_item_paths`,
  `key_toggle_target`) + `EditorCore::replace_char_undoable`; the checkbox
  listener captures only its structural path and resolves the Doc and marker at
  event time. Status: implemented (graph 4f1 `checkbox-toggle`). Guards:
  `md_harness.rs` `x_toggles_the_focused_blocks_first_open_task`,
  `clicking_a_checkbox_toggles_that_item`; `task_list.rs` tests;
  `editor.rs` `replace_char_undoable_is_one_undo_step`.
- **`UXI-Buffer-13` (heading jumps).** In `Viewing`, `]]` puts the Doc cursor
  on the next painted heading block (any level) and `[[` on the previous one —
  from inside a section, `[[` goes to that section's own heading. The target
  heading is scrolled to the **top** of the view (the outline rail's jump,
  UXI-Rail-4), not merely revealed. No wrap: past the last / before the first
  heading the cursor stays and the status line says "no next / previous
  heading". Headings inside a folded section are skipped (UXI-Buffer-14).
  Headings nested in a container block (e.g. a blockquote) are not jump
  targets — the Doc cursor addresses top-level blocks. Keys are keymap-registry
  entries (`NextHeading` / `PrevHeading`, YaldaView), listed + rebindable in the
  keybindings tile. Status: implemented (graph 4f1 `heading-nav`). Guard:
  `md_harness.rs` `heading_jumps_move_cursor_and_scroll_heading_to_top`.
- **`UXI-Buffer-14` (heading folds).** In `Viewing`, vim-style folding by
  heading: `za` toggles the fold of the section under the cursor (the cursor's
  heading block, or the nearest heading above it); `zM` folds every heading that
  has a section; `zR` unfolds everything. A heading's section is every block
  after it up to the next heading of the same or a higher level, so a folded
  `#` hides its `##` subsections. A folded section paints **only its heading**,
  followed by a muted `… N hidden blocks` marker; the hidden blocks are not
  painted (zero-height rows) and are not copied by a selection that spans them.
  `j`/`k`/page/`G` and `]]`/`[[` skip hidden blocks; folding from inside a
  section lands the cursor on its heading (it stays visible, UXP-1); a jump that
  targets a hidden block (outline rail, local-menu goto, Edit→Doc landing) opens
  the folds hiding it. Fold state is **per Doc tile** (`DocState::folds`), keyed
  by the heading's first source line and re-keyed on every re-parse by heading
  identity (level + text, nearest line) — so folds survive edits (in this or a
  sibling Edit tile of the same file), theme re-renders and a Doc → Edit → Doc
  round trip; a fold whose heading vanished is dropped. The outline rail marks
  a folded heading with `▸`. Fold changes are a `DocSeqs` input (`fold_seq`) of
  the cached `DocView`. Status: implemented (graph 4f1 `heading-nav`). Guards:
  `md_harness.rs` `za_folds_the_section_out_of_paint_and_j_skips_it`,
  `zm_folds_all_and_zr_unfolds_all`,
  `fold_survives_an_edit_that_moves_its_heading`,
  `fold_toggle_rerenders_the_cached_doc_body`,
  `outline_row_label_marks_folded_headings`. Glyph/colour of the marker is a
  human check (genuine gap 1).
