# Buffer System Design

## Overview

Add multi-file buffer support to sketch. Users can open multiple files, switch between them, view a buffer list, close buffers, and cycle through them. Each buffer preserves its own scroll position, view mode, cursor position, and undo history.

## Buffer Struct

New file `src/buffer.rs`. A `Buffer` encapsulates all per-file state:

```rust
pub struct Buffer {
    pub editor: Editor,
    pub viewport: Viewport,
    pub view_mode: ViewMode,
    pub highlighter: Highlighter,
    pub rendered_cache: Vec<RenderedBlock>,
    pub view_cache_dirty: bool,
}
```

`Buffer` provides methods:
- `new(filename, content, config)` — create a buffer from file content
- `rebuild_render_cache(theme)` — re-render markdown blocks
- `effective_content_width(terminal_width, browser_width)` — compute content width

`App` changes from holding editor/viewport/view_mode/highlighter/rendered_cache/view_cache_dirty directly to:

```rust
buffers: Vec<Buffer>,
active_buffer: usize,
```

Helper methods `active(&self) -> &Buffer` and `active_mut(&mut self) -> &mut Buffer` provide access. All existing `self.editor`, `self.viewport`, `self.view_mode` references become `self.active().editor`, etc.

## Buffer Operations

**Open:** `App::open_buffer(path)` reads the file, creates a `Buffer`, pushes to `buffers`, sets `active_buffer` to the new index. If the file is already open (matched by canonical path), switches to the existing buffer instead of opening a duplicate.

**Close:** `App::close_buffer(index)` — if the buffer is modified, sets `command_error` with "No write since last change (add ! to override)" and refuses. Otherwise removes from `buffers`. If the closed buffer was active, `active_buffer` moves to the previous buffer (or 0). If it was the last buffer, quit the app.

**Switch:** `App::switch_buffer(index)` sets `active_buffer = index`. No other work needed since each buffer holds its own state.

**Cycle:** `Tab` cycles to next buffer (wrapping). `shift-Tab` cycles to previous.

The current `load_file` method becomes `open_buffer` — the file browser calls it, and instead of replacing the editor, it adds/switches a buffer.

## Buffer List UI

The buffer list is a panel that appears below the top bar, taking up to 1/3 of the screen height, capped by buffer count.

**Layout when visible:**
```
[top bar]           — 1 row
[buffer list]       — up to height/3 rows, min(buffer_count, height/3)
[content]           — remaining space
[bottom bar]        — 1 row
```

**Each row:** `full/path/to/file.md [+]` — full path with `[+]` for modified buffers. The active buffer is highlighted. The buffer currently being viewed in the content area has a distinct indicator.

**Navigation:**
- `j`/`k` — move selection up/down
- `Enter` — switch to selected buffer, dismiss list
- `d` — close selected buffer (with modified guard)
- `Esc` — dismiss buffer list
- `/` — enter filter mode

**Filter mode:** Pressing `/` shows a text input at the top of the buffer list. Typing fuzzy-filters the list against full paths (case-insensitive, non-contiguous character matching — e.g., "aprs" matches `src/app.rs`). `Esc` clears the filter back to full list. `Enter` switches to the selected filtered result.

**App mode:** `AppMode::BufferList` with a `buffer_list_selected: usize` field tracking cursor position within the list, plus `buffer_list_filter_mode: bool` and `buffer_list_filter_text: String`.

## Commands and Keybindings

New `Action` variants: `NextBuffer`, `PrevBuffer`, `BufferList`, `CloseBuffer`.

New commands:

| Command | Aliases | Action |
|---|---|---|
| `next-buffer` | | `NextBuffer` |
| `prev-buffer` | | `PrevBuffer` |
| `buffer-list` | `buffers`, `ls` | `BufferList` |
| `close-buffer` | `bd` | `CloseBuffer` |

Default keybindings:
- `Tab` — `next-buffer`
- `shift-Tab` — `prev-buffer`

Default menu: add `b` entry "buffers" pointing to `buffer-list` in the root menu.

## Module Structure

### New Files
- `src/buffer.rs` — `Buffer` struct and per-buffer methods

### Modified Files
- `src/app.rs` — Replace single-editor fields with `Vec<Buffer>` + `active_buffer`. Add buffer operations (open, close, switch, cycle). Add buffer list mode handling. Move per-buffer logic (cache rebuild, content width) into `Buffer`.
- `src/view.rs` — New `draw_buffer_list` function, layout adjustment when buffer list is visible. `ViewState` gains buffer list fields (entries, selected index, filter state).
- `src/command.rs` — New `Action` variants and command registrations.
- `src/keybind.rs` — Tab/shift-Tab default bindings.
- `src/menu.rs` — Add "buffers" entry to default menu.
- `src/lib.rs` — Add `pub mod buffer`.

### Unchanged
`keys.rs`, `config.rs`, `blocks.rs`, `cursor.rs`, `document.rs`, `editor.rs`, `file_browser.rs`, `highlight.rs`, `render.rs`, `theme.rs`, `viewport.rs`, `main.rs`.

### Note on app.rs size
Moving per-buffer logic (cache rebuilding, content width calculation) into `buffer.rs` offsets the new buffer management code. Net effect should keep `app.rs` roughly the same size.

## Future Work

The file browser will be refactored from its current side-panel style to use the same top-bar list pattern as the buffer list. This is a follow-up task, not part of this spec.
