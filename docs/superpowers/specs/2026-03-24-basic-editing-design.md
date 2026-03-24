# Basic Editing — Design Spec

## Overview

Add vim-style editing to Sketch with a rope-based text buffer, tree-sitter incremental parsing, and Obsidian-style per-block reveal (the block the cursor is on shows raw markdown, all others stay rendered). This is the first editing increment — it covers insert mode, basic normal mode motions, per-action undo, and `:w`/`:q` command mode.

## Text Buffer

The source of truth is a `ropey::Rope` holding the raw markdown text. Raw markdown is no longer discarded after parsing.

```rust
struct Document {
    rope: Rope,
    file_path: PathBuf,
    modified: bool,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
}

struct UndoEntry {
    old_text: String,
    range: (usize, usize),    // byte range in rope that was modified
    cursor_before: CursorPos,
}
```

- `Rope` gives O(log n) inserts/deletes at any position
- `Document` owns the rope, exposes `insert_char`, `delete_range`, `line_count`, `line_text`, `save`
- `modified` tracks whether buffer differs from disk
- Undo/redo stacks store inverse operations per-action (not per-keystroke)
- An "action" boundary is created when entering/leaving insert mode, and for each normal mode operation (`dd`, `x`, etc.)

- `UndoEntry.old_text` copies the replaced text as a `String`. For large deletions this allocates — acceptable for this scope; a more efficient inverse-operation model is a future consideration.

Dependencies: `ropey` crate.

## Cursor Model

Character-level cursor replacing the current line-only model:

```rust
struct CursorPos {
    line: usize,    // 0-indexed line in the rope
    col: usize,     // 0-indexed column (char index within line, NOT byte offset — consistent with ropey's char-based API)
}
```

### Behavior

- Normal mode: cursor sits *on* a character (block cursor). Cannot go past last character of a line.
- Insert mode: cursor sits *between* characters (beam cursor). Can be positioned after the last character.
- `h`/`l`: move within a line, clamped at line boundaries (no wrapping)
- `j`/`k`: move between lines, preserving a "desired column" (sticky column) — if you move from a long line to a short line, the cursor clamps to end-of-line but remembers the original column
- `0`: column 0. `$`: last character.
- `w`/`b`/`e`: word motions per vim's `:help word` — a "word" is a sequence of keyword characters (letters, digits, underscore) or a sequence of non-blank non-keyword characters, separated by whitespace

### Cursor ↔ Viewport

- Viewport `scroll_offset` auto-adjusts to keep cursor visible (scrolloff of ~3 lines)
- The existing `viewport.cursor_line` is replaced by `CursorPos.line` as source of truth

## Tree-sitter Integration

Tree-sitter provides incremental parsing for block boundary detection. pulldown-cmark stays for rendering non-active blocks.

### Why both parsers

- **Tree-sitter**: incremental syntax tree — when you edit one character, re-parses only the affected region. Tells us block boundaries instantly.
- **pulldown-cmark**: produces styled `RenderedBlock` output for beautiful rendering. Tree-sitter's tree is structural, not styled.

### Flow

```
Edit keystroke
  → Rope is modified
  → Tree-sitter incrementally re-parses (µs)
  → Block boundaries updated
  → Active block (cursor's block) shown as raw text
  → Other blocks re-render through pulldown-cmark + syntect (only when block content changes)
```

### Block boundary detection

Tree-sitter's markdown grammar produces nodes like `atx_heading`, `paragraph`, `fenced_code_block`, `block_quote`, `list`, `thematic_break`, `table`, `image`. Each node has a byte range. Given the cursor's byte position in the rope, we find which top-level node contains it — that's the "active block."

```rust
struct TreeState {
    parser: tree_sitter::Parser,
    tree: Option<tree_sitter::Tree>,
}
```

On edit: call `tree.edit()` with the change description, then `parser.parse()` with the old tree — tree-sitter incrementally updates in microseconds.

### Rendering cache

Non-active blocks cache their `RenderedBlock` output. The cache is invalidated when tree-sitter detects a change in that block's byte range. Typing in one block doesn't re-render the entire document.

Dependencies: `tree-sitter` 0.24+ and `tree-sitter-md` (from MDeiml/tree-sitter-markdown — the most maintained grammar).

### Per-block rendering strategy

To render a non-active block: extract the block's text from the rope using tree-sitter's byte range for that node, feed the substring to pulldown-cmark independently. Each block is rendered in isolation. This works for most blocks; edge cases (e.g., a list item needing surrounding context) are handled by extracting the entire parent list node's text.

### Cache invalidation

After each edit:
1. Call `tree.edit()` then `parser.parse()` to get the new tree
2. Call `new_tree.changed_ranges(&old_tree)` to identify changed byte ranges
3. Compare changed ranges against cached block byte ranges — invalidate any block whose range overlaps a changed range
4. Re-render only invalidated blocks via pulldown-cmark

## Editing Modes

Two new modes added to the existing `AppMode` enum:

```rust
enum AppMode {
    Normal,       // cursor movement + vim commands
    Insert,       // typing text
    Command,      // :w, :q, :q!, :wq
    Menu,         // existing
    FileBrowser,  // existing
}
```

### Normal mode

Cursor movement (replaces the current scroll-only j/k behavior):
- `h`/`j`/`k`/`l` — character/line movement
- `w`/`b`/`e` — word motions
- `0`/`$` — line start/end
- `gg`/`G` — top/bottom of document (existing)

Editing commands:
- `i` — enter insert mode at cursor
- `a` — enter insert mode after cursor
- `o` — open line below, enter insert (note: `o` was previously bound to OpenLink in the viewer MVP; OpenLink moves to `gx`)
- `O` — open line above, enter insert
- `x` — delete character under cursor
- `dd` — delete current line
- `u` — undo
- `Ctrl+r` — redo
- `:` — enter command mode

Viewport scrolling (still works):
- `Ctrl+d`/`Ctrl+u` — half page
- `Ctrl+f`/`Ctrl+b` — full page

**Key conflict resolution:** `j`/`k` now move the cursor (which scrolls the viewport to follow). The old scroll-only behavior is covered by `Ctrl+d`/`Ctrl+u`. `q` no longer quits — quit is `:q` only (frees `q` for future use like macros). `o` moves from OpenLink to open-line-below; OpenLink reassigned to `gx` (vim convention). `gg`/`G` now move the cursor to line 0 / last line (not just scroll). The default menu tree's `q -> Quit` entry is removed (quit is `:q` only).

### Insert mode

- All printable characters insert at cursor position
- Enter inserts a newline
- Backspace deletes character before cursor
- Delete key deletes character at cursor
- Arrow keys move cursor
- Esc returns to normal mode and creates an undo boundary

### Command mode

- `:` opens a command input bar at the bottom (like `/` does for search)
- `:w` — save file
- `:w filename` — save to new path (save-as, updates document's file path)
- `:q` — quit (warns if modified, requires `:q!` to force)
- `:wq` — save and quit
- `:q!` — force quit without saving
- Esc cancels and returns to normal mode

## Obsidian-style Per-Block Reveal

The block containing the cursor shows raw markdown. All other blocks show styled rendered output.

### How it works

- Tree-sitter provides the byte range of each top-level markdown block
- The cursor's byte position in the rope is computed from `CursorPos`
- The block containing the cursor is the "active block"
- Active block renders as raw markdown lines from the rope, with the cursor visible
- All other blocks render as styled `RenderedBlock`s (same as current viewer)

### Visual treatment

- Active block shows raw markdown text in paragraph style (monochrome, no markdown rendering)
- Subtle left-border or background tint distinguishes the editing boundary
- Block cursor in normal mode, beam cursor in insert mode
- Transition is instant — no animation

### Edge cases

- Cursor on a horizontal rule (`---`): shows the raw `---` text
- Cursor on an image (`![alt](url)`): shows the raw markdown
- Multi-line blocks (code blocks, lists, blockquotes): entire block reveals when cursor enters any line
- If an edit changes block boundaries (e.g., adding a blank line splits a paragraph), tree-sitter detects this and the active block updates
- Tab characters in raw text display as 4 spaces
- Search highlights (`/` search) appear in rendered blocks only, not in the raw active block. Search remains a substate of Normal mode (existing `search_input_mode: bool` approach)

### Cursor movement across blocks

Free movement — `h`/`j`/`k`/`l` move through the entire document. As the cursor enters a new block, the old block re-renders and the new block reveals. The document feels like one continuous text buffer.

## File Saving

- `:w` writes rope contents to file path via `Document.save()`
- Save writes to a temporary file first, then atomically renames (prevents data loss on crash)
- On successful save, `modified` set to false, title bar `[+]` clears
- `:w filename` saves to new path and updates the document's file path
- File permissions preserved from original file

### Quit behavior

- `:q` checks `modified` — if true, shows "No write since last change (add ! to override)" in the command bar
- `:q!` quits without saving
- `:wq` saves then quits
- `q` key no longer quits in normal mode — quit is `:q` only

### Unsaved change indicator

- Title bar shows `[+]` after filename when `modified` is true
- Warns on `:q` if unsaved

## Architecture Changes

### New Modules

- `src/document.rs` — `Document` struct (rope, file path, modified flag, undo/redo), text mutation methods, save
- `src/cursor.rs` — `CursorPos`, movement logic (h/j/k/l, w/b/e, 0/$), sticky column
- `src/tree.rs` — `TreeState`, tree-sitter parser setup, incremental parse, block boundary queries
- `src/editor.rs` — Orchestrates editing: handles insert/normal mode editing operations, creates undo boundaries. `Editor` owns `Document`, `CursorPos`, and `TreeState` — `App` owns `Editor`. This gives clean borrow checker ergonomics.

### Modified Modules

- `src/app.rs` — Add `Insert` and `Command` modes. Normal mode now moves cursor instead of scrolling. Holds `Document`, `CursorPos`, `TreeState`, `Editor`. Command mode input handling.
- `src/view.rs` — Active block detection and raw text rendering. Cursor rendering (block/beam). Modified indicator `[+]` in top bar. Command bar rendering.
- `src/keybind.rs` — New actions: `MoveLeft`, `MoveRight`, `MoveUp`, `MoveDown`, `MoveWordForward`, `MoveWordBackward`, `MoveWordEnd`, `MoveLineStart`, `MoveLineEnd`, `InsertMode`, `InsertAfter`, `OpenLineBelow`, `OpenLineAbove`, `DeleteChar`, `DeleteLine`, `Undo`, `Redo`, `EnterCommand`.
- `src/viewport.rs` — Cursor-following scroll (auto-adjust `scroll_offset` to keep cursor visible with scrolloff margin).

### Removed/Changed

- `App.blocks` (`Vec<RenderedBlock>`) becomes a render cache, not the source of truth. Source of truth is `Document.rope`.
- The `render::render()` function is still used but called per-block for non-active blocks, not for the whole document at once.
- `q` removed from normal mode keybindings (quit is `:q` only). `q -> Quit` removed from default menu tree.
- `o` reassigned from OpenLink to OpenLineBelow. OpenLink moves to `gx` (multi-key sequence).
- Opening a new file via the file browser with unsaved edits: warns like `:q` ("No write since last change"). User must save first or the open is cancelled.

### New Dependencies

- `ropey` — rope data structure
- `tree-sitter` — incremental parser
- `tree-sitter-md` — markdown grammar

## Testing

- **Unit tests for `document.rs`**: Insert/delete at various positions, undo/redo cycles, save to temp file, modified flag tracking.
- **Unit tests for `cursor.rs`**: Movement clamping, sticky column behavior, word motion boundaries, line start/end.
- **Unit tests for `tree.rs`**: Block boundary detection from markdown, incremental re-parse after edits, correct block type identification.
- **Unit tests for `editor.rs`**: Insert mode text entry + undo boundary, `dd` + undo, `x` + undo, combined operation sequences.
- **Integration test**: Load a markdown file, make edits, save, reload, verify content matches.
- **No TUI tests** — cursor rendering and block reveal tested manually.

## Explicitly Out of Scope

- New file creation (`:e newfile`, `sketch` without arguments opening an empty buffer)
- `:e` command to open a different file

## Future Considerations (Not in This Spec)

- Visual mode (character/line selection, text objects)
- Additional vim motions (f/t/F/T, %, etc.)
- Operators with motions (d + w, c + $, y + ip, etc.)
- Multiple buffers / split views
- Raw/rendered toggle (full document switch)
- Code editing with language-specific tree-sitter grammars
- `.` repeat last command
- Macros
