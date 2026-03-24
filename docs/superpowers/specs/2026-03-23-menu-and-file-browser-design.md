# Menu System & File Browser — Design Spec

## Overview

Add a Helix-style "which-key" menu popup and a file browser side panel to Sketch. The menu system is a generic, reusable command dispatcher — the file browser is the first command that uses it. Pressing the leader key (Space) opens a top-anchored popup showing available commands. Pressing `f` opens a file browser in a left side panel.

## Which-Key Menu System

### Data Model

The menu is a tree of nodes. Each node is a key-labeled entry that either dispatches a command or opens a submenu:

```rust
struct MenuNode {
    key: char,
    label: String,
    action: MenuAction,
}

enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(Action),  // reuses the existing Action enum from keybind.rs
}
```

### Default Menu Tree

```
Space (leader)
├── f  "file browser"  → OpenFileBrowser
├── /  "search"        → SearchForward
├── q  "quit"          → Quit
└── g  "goto" ▸
    ├── g  "top"          → JumpTop
    ├── e  "bottom"       → JumpBottom
    └── h  "next heading" → NextHeading
```

### Menu State

```rust
struct MenuState {
    active: bool,
    path: Vec<usize>,  // breadcrumb trail of indices — each entry is the index of the
                        // MenuNode we descended into at that level of the tree.
                        // To resolve the current submenu: walk the root menu tree
                        // following path[0], then path[1], etc.
                        // Escape pops the last entry (go up one level).
                        // Empty path = root menu.
}
```

### Behavior

- Space (leader key) enters menu mode — popup appears top-anchored below the top bar
- Popup shows available keys and labels in a compact horizontal grid
- Keys in purple, labels in gray, submenus marked with `▸`
- Pressing a valid key either executes a command (closes popup) or opens a submenu (popup updates)
- Escape closes the popup at any depth
- Unrecognized keys are ignored — popup stays open
- Content below the popup is NOT dimmed
- Bottom bar is NOT updated — the popup is self-contained
- The popup takes 2-3 terminal rows

### Submenu Navigation

When entering a submenu, the popup header updates to show the path (e.g., "goto") and displays the submenu's entries. Escape from a submenu goes back one level. Escape from the root menu closes it entirely.

## File Browser Side Panel

### Data Model

```rust
struct FileBrowser {
    root: PathBuf,              // starting directory — the dir of the currently viewed file, or cwd
    current_dir: PathBuf,       // directory being displayed — initialized to root
    entries: Vec<BrowserEntry>,
    selected: usize,
    filter_mode: bool,
    filter_text: String,
    filtered_entries: Vec<usize>,  // indices into entries matching filter
    width_percent: u16,            // panel width as % of terminal, default 30
}

struct BrowserEntry {
    name: String,
    is_dir: bool,
    path: PathBuf,
}
```

### Layout

- Left side panel, ~30% of terminal width (clamped to min 20, max 60 columns)
- Border separator between panel and content
- Panel header shows current directory path
- File list with `▸` selection marker
- Directories shown in cyan, files in default text color
- Directories sorted first, then files, both alphabetical
- Panel footer shows context-sensitive key hints

### Navigation

- `j`/`k` — move selection up/down, wrapping at edges
- `Space` — open file (loads in viewer, closes browser) or descend into directory
- `Backspace` — go to parent directory (no-op at filesystem root)
- `/` — enter filter mode
- `Escape` — when filtering: clear filter; when not filtering: close panel
- `q` — close panel

All file browser keys are designed to be configurable. KDL config parsing for file browser keys is deferred — MVP uses hardcoded defaults.

### Fuzzy Filter

- `/` enters filter mode — a filter input appears below the panel header
- Typed characters do case-insensitive substring matching against filenames
- List updates live as you type, showing only matching entries
- Matched text is highlighted in the filename
- `Enter` or `Space` opens the selected match
- `Escape` clears the filter and returns to the full list
- Filter searches the current directory only (not recursive)

### File Opening

When a file is selected:
- The file is read into memory (same as startup)
- The viewer re-renders with the new file content
- The file browser panel closes
- The top bar updates to show the new filename
- Non-UTF-8 files show an error in the content area, browser stays open

### Error Handling

- Permission denied on a directory: show inline error message, stay in current directory
- Empty directory: show "empty" placeholder text
- File read errors: show error in content area, browser stays open for another selection
- Hidden files (dotfiles): hidden by default
- Symlinks: followed; broken symlinks are silently excluded from the listing

## Architecture Changes

### New Modules

- `src/menu.rs` — `MenuNode`, `MenuAction`, `MenuState`, default menu tree, key processing
- `src/file_browser.rs` — `FileBrowser` struct, directory listing, filter state, navigation

### Modified Modules

- `src/app.rs` — New app modes: `Normal`, `Menu`, `FileBrowser`. Event loop routes key events based on current mode. Menu mode handles popup. FileBrowser mode handles side panel. File loading extracted into a reusable method.
- `src/view.rs` — `draw()` gains menu popup rendering (top-anchored overlay with opaque background) and file browser panel rendering (left split layout). Menu popup is rendered after content so it overlays. When the file browser panel is open, the content area shrinks to the remaining width; `max_line_width` and content centering apply within the reduced area; `total_lines` is recalculated for the new width on panel open/close.
- `src/keybind.rs` — New actions: `OpenMenu`, `OpenFileBrowser`, `FileBrowserDown`, `FileBrowserUp`, `FileBrowserEnter`, `FileBrowserParentDir`, `FileBrowserFilter`, `FileBrowserClose`.

### New Action Variants (Complete List)

All new variants added to the existing `Action` enum in `keybind.rs`:

- `OpenMenu` — enters menu mode (bound to Space in normal mode)
- `OpenFileBrowser` — opens the file browser side panel
- `FileBrowserDown` — move selection down in file list
- `FileBrowserUp` — move selection up in file list
- `FileBrowserEnter` — open selected file or descend into directory
- `FileBrowserParentDir` — navigate to parent directory
- `FileBrowserFilter` — enter filter mode
- `FileBrowserClose` — close the file browser panel

Note: In file browser mode, Space maps to `FileBrowserEnter`, NOT `OpenMenu`. Each mode has independent key bindings.

### Mode Flow

```
Normal → Space → Menu → f → FileBrowser
                      → Esc → Normal
FileBrowser → Esc → Normal
FileBrowser → Space (on file) → Normal (with new file loaded)
FileBrowser → Space (on dir) → FileBrowser (new directory)
```

The menu popup and file browser are independent. The menu is a transient overlay that dispatches a command and disappears. The file browser is a persistent panel with its own input handling.

## Configuration

### Menu Tree (KDL)

```kdl
menu {
    key "f" label="file browser" action="OpenFileBrowser"
    key "/" label="search" action="SearchForward"
    key "q" label="quit" action="Quit"
    group "g" label="goto" {
        key "g" label="top" action="JumpTop"
        key "e" label="bottom" action="JumpBottom"
        key "h" label="next heading" action="NextHeading"
    }
}
```

User config extends defaults. `action="None"` removes an entry.

### Leader Key

Configured in the keybinding section:

```kdl
mode "normal" {
    key "Space" action="OpenMenu"
}
```

### File Browser Keys

Separate mode in keybinding config:

```kdl
mode "file_browser" {
    key "j" action="FileBrowserDown"
    key "k" action="FileBrowserUp"
    key "Space" action="FileBrowserEnter"
    key "Backspace" action="FileBrowserParentDir"
    key "/" action="FileBrowserFilter"
    key "q" action="FileBrowserClose"
    key "Escape" action="FileBrowserClose"
}
```

MVP ships with hardcoded defaults. KDL parsing for menu/file browser config is a future enhancement.

## Testing

- **Unit tests for `menu.rs`**: Menu tree navigation — key sequence → correct action or submenu. Unrecognized keys ignored. Escape closes.
- **Unit tests for `file_browser.rs`**: Directory listing from temp directories, selection movement with wrapping, filter matching, parent directory navigation. Use `tempfile` crate.
- **No TUI integration tests** — popup rendering and side panel layout tested manually.

## Future Considerations (Not in This Spec)

- KDL config parsing for menu tree and file browser keys
- Hidden file toggle in file browser
- File preview in browser (show first few lines of selected file)
- Multiple open files / buffer list
- Recursive fuzzy search across entire project
