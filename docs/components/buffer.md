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
  with `Ctrl-W`: **Code (RAW)** — monospace, line-number gutter, `md_highlight`
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
