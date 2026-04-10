# Full-Screen File Browser Design

## Overview

Add a full-screen file browser mode as a new top-level `AppScreen` state. The existing top-bar dropdown file browser stays untouched for quick navigation. The full-screen browser reuses the same `FileBrowser` instance (shared state: cwd, filter, selection) and provides a richer single-pane view with breadcrumbs, file metadata columns, and keybind hints.

## AppScreen Enum

```rust
#[derive(Debug, PartialEq)]
enum AppScreen {
    Editor,
    FileBrowser { came_from_dropdown: bool },
}
```

Added to `App` as `screen: AppScreen`, defaulting to `Editor`. When `screen` is `FileBrowser`, the entire terminal is given to the full-screen browser renderer — the normal editor layout (top bar, content, bottom bar) is not drawn.

On close:
- If `came_from_dropdown: true`, set `screen = Editor` and reopen the dropdown (`mode = AppMode::FileBrowser`, `file_browser` stays `Some`).
- If `came_from_dropdown: false`, set `screen = Editor` and `mode = AppMode::Normal`. The `FileBrowser` instance stays alive so state persists for next open.

## Entry Points

Three ways to enter the full-screen browser:

1. **Menu entry**: `F` (capital) in the space menu → command `"file-browser-full"`. Opens with `came_from_dropdown: false`.
2. **Command**: `:file-browser-full` from command mode. Opens with `came_from_dropdown: false`.
3. **Expand from dropdown**: While the dropdown file browser is open, press `Tab` to expand into full-screen. Opens with `came_from_dropdown: true`.

All three ensure `self.file_browser` is `Some` (create if `None`, using cwd) before setting `screen = AppScreen::FileBrowser { ... }`.

## Full-Screen Layout

The full-screen view uses the entire terminal area, divided into three zones:

```
┌─────────────────────────────────────────────┐
│ ▸ ~/projects/sketch/src                     │  ← breadcrumb header (1 row)
├─────────────────────────────────────────────┤
│ ▸ ..                                        │
│   src/                          —    —      │
│   Cargo.toml                 1.2K  Apr 07   │
│   README.md                  4.5K  Mar 30   │
│   ...                                       │  ← entry list (fills remaining)
├─────────────────────────────────────────────┤
│ / filter text█                              │  ← filter input (1 row, only when active)
│ enter:open  -:parent  .:hidden  /:filter    │  ← hint bar (1 row)
└─────────────────────────────────────────────┘
```

### Header (1 row)
Breadcrumb showing the full current directory path, truncated from the left with `…` if too wide. Uses the same style as the dropdown header.

### Entry List (fills remaining space)
Each row shows:
- Selection marker: `▸ ` if selected, `  ` otherwise
- Icon: directory entries get a `/` suffix, files get no suffix (same as dropdown)
- Name: left-aligned, colored by type (cyan for dirs, light gray for files)
- Size: right-aligned column, human-readable (e.g. `1.2K`, `3.4M`), dirs show `—`
- Modified time: right-aligned column, short format (e.g. `Apr 07`, `Mar 30`), dirs show `—`

Column layout: name takes remaining space; size gets 6 chars; mtime gets 7 chars; 2 chars padding between columns. The list scrolls to keep the selected entry visible (reuse `scroll_to_keep_visible`).

### Filter Input (1 row, conditional)
Shown only when `filter_mode` is active. Same `/ ` prefix + input text + cursor as the dropdown.

### Hint Bar (1 row)
Static keybinding hints: `enter:open  o:open+stay  -:parent  .:hidden  /:filter  q:close`
If `came_from_dropdown`, also show `tab:collapse` (returns to dropdown).

## Entry Data

`BrowserEntry` gains two optional metadata fields:

```rust
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}
```

These are populated during `list_directory` and `search_recursive` by reading `fs::metadata`. The dropdown renderer ignores them (only uses name/is_dir). The full-screen renderer formats them for display.

## Input Handling

When `screen == AppScreen::FileBrowser`, `handle_key_event` routes to a new `handle_full_browser_key()` method (separate from the dropdown's `handle_file_browser_key()`). Both methods operate on the same `self.file_browser` instance.

**Filter mode** (same behavior as dropdown):
- `Esc` → close full-screen browser entirely
- `Enter` → if single result, open it; if multiple, exit filter mode to navigate
- `Backspace` → delete last char
- `Char(c)` → append to filter

**Normal mode:**
- `j` / `k` → move down/up
- `l` / `Enter` → enter dir or open file (open file: switch to Editor; if shift/alt held, open file but stay in browser)
- `h` / `-` / `Backspace` → go to parent directory
- `.` → toggle hidden files
- `/` → enter filter mode
- `Tab` → if `came_from_dropdown`, collapse back to dropdown; otherwise no-op
- `q` / `Esc` → close (respecting `came_from_dropdown` logic)
- `g g` → jump to first entry
- `G` → jump to last entry

**Opening files — default vs. stay-in-browser:**
- `Enter` / `l` → open file and switch to Editor screen (default)
- `o` → open file in a new buffer but stay in the browser (for batch-opening)

## View Integration

A new function `draw_full_file_browser(frame: &mut Frame, state: &FullBrowserViewState)` in `view.rs` handles the full-screen layout. It is called from the main `draw()` function when a new `full_browser` field on `ViewState` is `Some`.

```rust
pub struct FullBrowserViewState {
    pub dir: String,
    pub entries: Vec<FullBrowserEntry>,
    pub filter_mode: bool,
    pub filter_text: String,
    pub came_from_dropdown: bool,
}

pub struct FullBrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_selected: bool,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}
```

When `state.full_browser.is_some()`, `draw()` skips the normal editor layout entirely and calls `draw_full_file_browser` with the full frame area.

## Module Changes

**Modified files:**
- `src/file_browser.rs` — Add `size` and `modified` fields to `BrowserEntry`, populate from metadata in `list_directory` and `search_recursive`
- `src/app.rs` — Add `AppScreen` enum, `screen` field, `handle_full_browser_key()` method, entry points (menu/command/tab-expand), `FullBrowserViewState` construction in draw closure
- `src/view.rs` — Add `FullBrowserViewState`, `FullBrowserEntry`, `full_browser: Option<FullBrowserViewState>` to `ViewState`, `draw_full_file_browser()` function, early-return in `draw()` when full browser is active
- `src/menu.rs` — Add `F` entry for `file-browser-full`
- `src/command.rs` — Register `file-browser-full` command
- `src/keybind.rs` — Add `OpenFileBrowserFull` action variant

**New files:** None.

**Unchanged:** `buffer.rs`, `theme.rs`, `viewport.rs`, `highlight.rs`, `editor.rs`, `render.rs`
