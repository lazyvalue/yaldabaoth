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
`render_browser`, `edit_ui.rs`, `browser_ui.rs`, `render_blocks.rs`.

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
