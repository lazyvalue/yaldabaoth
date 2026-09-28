# Text-editing code review — 2026-09-27

Graph `exa` (text-editing-code-review), branch `text-edit-review`.

Scope: every text-input surface — the core engine (`document.rs`, `editor.rs`,
`cursor.rs`, `keybind.rs`), the buffer Edit view (Code + WP), the agent compose
(chatbox + worksheet You-block), the diff comment compose + send picker, and the
14 ad-hoc single-line inputs (palettes, filters, rename/tag/cwd overlays).

Severity: **bug** · **hot** (per keystroke / per frame) · **cold** · **dup** · nit.
Status column is filled in by the implementation packages (`fixed` / `deferred`).
IDs are stable so commits and the worklog can cite them.

## A. Single-line inputs (14 ad-hoc `String` push/pop sites)

Sites: `jump_palette.rs` query · `SendPicker.query` (diff) · `TagEditorOverlay.input`
· `KeymapView.filter` · `LinearTile.input` · `CogView.graph_filter` ·
`RenameState.input` + `FileBrowser.filter_text` (lib `file_browser.rs`, used by the
browser screen AND the rail) · `NewProjectOverlay.cwd` · `RenameOverlay.text` ·
`TagInputOverlay.text` · `BufferSwitcher.filter_text`.

| ID | Sev | Finding |
|----|-----|---------|
| A1 | dup | Same push/pop editing re-implemented 14×; no caret movement, no word/line delete anywhere. |
| A2 | bug | Six sites (`NewProject`, `Rename`, `TagInput`, `BufferSwitcher`, both browser filters) have **no modifier guard**: an unbound Cmd/Ctrl chord types its bare letter (`cmd-v` in rename types `v`). File rename rejects Ctrl only. |
| A3 | bug | Three sites reject **all** Alt chars, so Option-composed symbols (`@ { [ |` on non-US Mac layouts) can't be typed; three others accept Alt even with no composed char (Linux `alt-b` types `b`). |
| A4 | bug | Prefilled paths/names (rename, new-project cwd) editable only by one-at-a-time backspace from the end. |
| A5 | dup | Selection-reset rule differs per site (reset / clamp — Cog's doc comment says reset but code clamps). |
| A6 | nit | The browser rail filter draws no caret. |
| A7 | cold | `SendPicker::ranked()` re-runs the fuzzy rank on every call (render, move, activate). |

## B. Core engine (`document.rs` / `editor.rs` / `cursor.rs`)

| ID | Sev | Finding |
|----|-----|---------|
| B1 | bug | Insert-mode Delete calls `begin_undo_group`, which **overwrites** the open `pending_undo`: the insert session's undo history is lost and later undo offsets are wrong. Also clamps caret with Normal-mode rule in Insert. |
| B2 | bug | Undo/redo restores a stale `frozen_lines` / `lockable` snapshot taken at group start, discarding agent lines streamed meanwhile. |
| B3 | bug | `can_insert_char_at` subtracts the `\n` twice: Enter can split a frozen line one char before its end. |
| B4 | bug | `dd` on an empty last line deletes the previous line's `\n` without running the frozen/lock guard or the shifts. |
| B5 | bug | `backspace` validates `[idx-1, idx)` with the clamped column but deletes with the unclamped one. |
| B6 | bug | `modified = !undo_stack.is_empty()` ignores the save point. |
| B7 | bug | `e` on a line's last word lands on the `\n` column; `b` at col 0 goes to prev line's last char, not its last word start. |
| B8 | bug | ropey default features treat `\r`, U+2028 etc. as line breaks while the engine counts only `\n` (anchor/frozen drift, tree-sitter edit mismatch). |
| B9 | hot | `shift_recorded_splices` walks the entire (never-trimmed) undo+redo history on every streamed chunk. |
| B10 | cold | One heap `Splice` per typed char; paste/seed/restore insert char-by-char (N guards, N splices, N rope ops). |
| B11 | hot | `shift_for_delete` rebuilds both anchor BTreeMaps (O(A log A)) on any newline-deleting edit. |
| B12 | cold | Every undo group clones all of `frozen_lines`. |
| B13 | hot | `is_frozen_line` is a linear scan; `can_delete_range` calls it per char → O(k·F). |
| B14 | hot | `reparse()` copies the whole rope (`full_text()`) for a tree-sitter tree nothing in the app reads. |
| B15 | dup | `char_to_line_col` ≡ `Document::line_col_of_char`; "visible line length" computed 4 ways; `max_col` ×3; word/find/till motions re-implement char classification on a fresh `Vec<char>` each. |
| B16 | dup | `apply_inverse`/`apply_forward` and `EditorView::undo`/`redo` are mirror copies. |
| B17 | nit | Failed multi-key prefix drops the first key's own single binding. |

## C. Buffer Edit view + highlight pipeline

| ID | Sev | Finding |
|----|-----|---------|
| C1 | hot | `HighlightCache` matches lines **by index**: one Enter near the top misses and re-highlights (twice: raw + stripped) every line below. |
| C2 | hot | Every edit rebuilds a `Vec<String>` of the whole doc (+ tab replace) and re-hashes + re-classifies (WP) every line. The same "snapshot lines" code exists 4× (edit view, compose render, transcript, You-block). |
| C3 | hot | Every root render calls `refresh_blocks` for every Doc tile (incl. hidden) — a sibling Edit keystroke triggers a full-text copy + full markdown parse + deep clone per Doc. |
| C4 | bug | Tabs expanded to 4 spaces for display but caret/selection columns stay raw: caret off by 3 cols per preceding tab (Code, WP, compose). |
| C5 | bug | WP view passes `selection_bg = None`, so selected prose is misread as inline code and reflows into the monospace font. |
| C6 | hot+bug | `x`/`d`/`y`/`p` shell out to `wl-copy`/`xclip`/`pbcopy` synchronously on the UI thread; counted `5x` spawns 5 processes, 5 undo groups, 5 reparses, and leaves only the last char in the clipboard. A second clipboard path (`cx.write_to_clipboard`) is used elsewhere. |
| C7 | bug | Cmd-V can't paste into a Diff review comment (`paste_from_clipboard` doesn't route to it). |
| C8 | bug | Sibling-tile edits reset this tile's scroll to its own caret (`edit_seq` in the reveal key). |
| C9 | dup | Bullet/ordered/quote marker detection implemented 3× (list continuation, WP classify, highlighter). |
| C10 | hot | Edit body isn't a cached yux view (rebuilt every root notify). |
| C11 | nit | Gutter fixed `px(40)` — doesn't scale with zoom; 5-digit line numbers overflow. |

## D. Agent compose

| ID | Sev | Finding |
|----|-----|---------|
| D1 | bug | Cmd-V in Insert uses vim `p` rules: pastes one char right of the caret and leaves the caret before the last pasted char. |
| D2 | bug | Frozen transcript lines: caret column is raw but segments are markdown-stripped → caret drifts on `**bold**`/links. |
| D3 | bug | Draft typed since the last ring save is lost on Quit/Restart; sessions file written non-atomically. |
| D4 | bug | Rejected prompt restored only in Chatbox mode (`chatbox_mut()`), not Worksheet. |
| D5 | hot | `build_chatbox_wrapped_line` re-collects the full line per visual row → O(L²/cols) per line per frame (a pasted 50 KB line is ~500×50K char ops/frame). |
| D6 | hot | Compose render rebuilds all lines, wraps every line twice, and hands `reconcile` a fresh `Rc` (defeating its `ptr_eq` fast path) on every root render. |
| D7 | hot | ~9 full-draft `to_string()` copies per unmodified keystroke in `handle_claude_key` (`text().trim().is_empty()`, `slash_query`, `topic_query`, before/after compare…). |
| D8 | hot | Topic catalog `Vec` cloned on every compose keystroke. |
| D9 | hot | "TEMP" `clear_log` diagnostics build their arguments (full-draft copy, O(transcript) scans) even when logging is off. |
| D10 | hot | Slash/topic popup rows computed for every agent tile on every render, before the visibility gate. |
| D11 | hot | Worksheet keystroke re-renders the whole cached `TranscriptView` (You-block not its own entity). |
| D12 | bug? | Virtualized long draft: wrap-width change doesn't respline items → stale row heights. |
| D13 | bug | Caret at EOL on a full wrapped row renders one column past the box. |
| D14 | dup | Three text-input renderers/caret models (agent compose, diff comment `U+2758` splice, `█` suffix inputs); three "reset editor + caret to end" helpers; dead horizontal-scroll window code (`compute_window`). |
| D15 | nit | Grapheme/wide-char/CRLF assumptions (1 char = 1 column). |

## E. Diff tile

| ID | Sev | Finding |
|----|-----|---------|
| E1 | hot | Body render deep-clones all review comments; `cursor` is in `DiffSeqs`, so every j/k pays it. |
| E2 | hot | `comment_card_lines` re-wraps a comment's whole body for every visible part-row, every frame. |
| E3 | cold | `visible_rows` filters all comments per file (O(files×comments)); `place_comment` rebuilds per-side vectors per comment. |
| E4 | nit | Diff comment compose has no max height/scroll — a long comment squeezes the diff to nothing. |
| — | ok | Review JSON is not rewritten per keystroke (writes on save/viewed/delete/send, background, gen-guarded). |

## Implementation packages

- **P1 line-input** (A*): shared `yalda::line_input::LineInput` + one typed-char policy; migrate all 14 sites.
- **P2 engine** (B1, B3–B7, B10, B11, B13, B15, B16 + D1 bulk paste).
- **P3 render pipeline** (C1, C2, C4, C5, D2, D5, D6).
- **P4 compose hot path** (D3, D4, D7–D10).
- **P5 clipboard + diff** (C6, C7, E1, E2).
- **Deferred** (documented, not done here): B2/B12 (frozen snapshot in undo — needs replaying shifts), B8 (ropey features + CRLF normalization touches file save), B9, B14 (removing tree-sitter is a product decision), B17, C3, C8, C9, C10, C11, D11–D15, E3, E4.
