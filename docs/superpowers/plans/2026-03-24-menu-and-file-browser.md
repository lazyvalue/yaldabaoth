# Menu System & File Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Helix-style which-key menu popup (Space leader) and a file browser side panel with fuzzy filtering, enabling users to discover commands and browse/open files.

**Architecture:** Two new modules (`menu.rs`, `file_browser.rs`) plus modifications to `app.rs` (mode routing), `view.rs` (popup + panel rendering), and `keybind.rs` (new actions). The menu dispatches commands; the file browser is the first command consumer. App gains a mode enum (Normal/Menu/FileBrowser) that routes key events.

**Tech Stack:** Rust, ratatui, crossterm, std::fs (directory listing), tempfile (test)

**Spec:** `docs/superpowers/specs/2026-03-23-menu-and-file-browser-design.md`

---

## File Structure

```
src/
├── menu.rs           # NEW — MenuNode, MenuAction, MenuState, default tree, key processing
├── file_browser.rs   # NEW — FileBrowser, BrowserEntry, directory listing, filter, navigation
├── keybind.rs        # MODIFY — add 8 new Action variants, add Space binding
├── app.rs            # MODIFY — AppMode enum, mode routing, menu/browser state, file loading
├── view.rs           # MODIFY — menu popup rendering, file browser panel rendering
├── lib.rs            # MODIFY — add new module declarations

tests/
├── menu_test.rs      # NEW — menu tree navigation tests
├── file_browser_test.rs  # NEW — directory listing, filter, navigation tests
```

---

### Task 1: Add New Action Variants to Keybind System

**Files:**
- Modify: `src/keybind.rs`
- Modify: `tests/keybind_test.rs`

- [ ] **Step 1: Add test for OpenMenu action**

Add to `tests/keybind_test.rs`:

```rust
#[test]
fn test_space_opens_menu() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::OpenMenu));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test keybind_test test_space_opens_menu`
Expected: FAIL — `Action::OpenMenu` doesn't exist

- [ ] **Step 3: Add new Action variants**

In `src/keybind.rs`, add these variants to the `Action` enum (before `None`):

```rust
    OpenMenu,
    OpenFileBrowser,
    FileBrowserDown,
    FileBrowserUp,
    FileBrowserEnter,
    FileBrowserParentDir,
    FileBrowserFilter,
    FileBrowserClose,
```

- [ ] **Step 4: Add Space binding to defaults**

In the `Default` impl for `KeybindManager`, add:

```rust
single.insert(key(' '), Action::OpenMenu);
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test keybind_test`
Expected: All 7 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/keybind.rs tests/keybind_test.rs
git commit -m "feat: add menu and file browser action variants, bind Space to OpenMenu"
```

---

### Task 2: Menu Module — Data Model and Key Processing

**Files:**
- Create: `src/menu.rs`
- Create: `tests/menu_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write menu tests**

Create `tests/menu_test.rs`:

```rust
use sketch::keybind::Action;
use sketch::menu::{MenuState, default_menu};

#[test]
fn test_menu_starts_inactive() {
    let state = MenuState::new();
    assert!(!state.is_active());
}

#[test]
fn test_menu_open_close() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    assert!(state.is_active());
    state.close();
    assert!(!state.is_active());
}

#[test]
fn test_menu_command_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key('f', &menu);
    assert_eq!(result, Some(Action::OpenFileBrowser));
    assert!(!state.is_active()); // command closes menu
}

#[test]
fn test_menu_submenu_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key('g', &menu);
    assert_eq!(result, None); // submenu opened, no action dispatched
    assert!(state.is_active()); // still in menu
}

#[test]
fn test_menu_submenu_then_command() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu); // enter goto submenu
    let result = state.process_key('g', &menu);
    assert_eq!(result, Some(Action::JumpTop));
    assert!(!state.is_active());
}

#[test]
fn test_menu_escape_from_submenu() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu); // enter goto submenu
    state.handle_escape();
    assert!(state.is_active()); // back to root menu, not closed
    assert!(state.path.is_empty());
}

#[test]
fn test_menu_escape_from_root() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.handle_escape();
    assert!(!state.is_active()); // closed
}

#[test]
fn test_menu_unrecognized_key_ignored() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key('z', &menu);
    assert_eq!(result, None);
    assert!(state.is_active()); // still open
}

#[test]
fn test_menu_current_nodes() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let nodes = state.current_nodes(&menu);
    assert!(nodes.iter().any(|n| n.key == 'f'));
    assert!(nodes.iter().any(|n| n.key == 'g'));
}

#[test]
fn test_menu_submenu_current_nodes() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu);
    let nodes = state.current_nodes(&menu);
    assert!(nodes.iter().any(|n| n.key == 'g')); // goto > g = top
    assert!(nodes.iter().any(|n| n.key == 'e')); // goto > e = bottom
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test menu_test`
Expected: FAIL — module doesn't exist

- [ ] **Step 3: Create the menu module**

Create `src/menu.rs`:

```rust
use crate::keybind::Action;

#[derive(Debug, Clone)]
pub struct MenuNode {
    pub key: char,
    pub label: String,
    pub action: MenuAction,
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(Action),
}

pub struct MenuState {
    active: bool,
    pub path: Vec<usize>,
}

impl MenuState {
    pub fn new() -> Self {
        Self {
            active: false,
            path: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn open(&mut self) {
        self.active = true;
        self.path.clear();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.path.clear();
    }

    /// Process a key press in the menu. Returns Some(Action) if a command was selected.
    /// Returns None if a submenu was entered or key was unrecognized.
    /// Closes the menu when a command is executed.
    pub fn process_key(&mut self, key: char, menu: &[MenuNode]) -> Option<Action> {
        let nodes = self.current_nodes(menu);
        for (i, node) in nodes.iter().enumerate() {
            if node.key == key {
                match &node.action {
                    MenuAction::Command(action) => {
                        let action = *action;
                        self.close();
                        return Some(action);
                    }
                    MenuAction::Submenu(_) => {
                        // Find the index in the actual menu tree (not the slice)
                        let idx = self.resolve_node_index(menu, key);
                        if let Some(idx) = idx {
                            self.path.push(idx);
                        }
                        return None;
                    }
                }
            }
        }
        None // unrecognized key — ignored, menu stays open
    }

    /// Handle escape: go up one level, or close if at root.
    pub fn handle_escape(&mut self) {
        if self.path.is_empty() {
            self.close();
        } else {
            self.path.pop();
        }
    }

    /// Get the menu nodes for the current depth.
    pub fn current_nodes<'a>(&self, menu: &'a [MenuNode]) -> &'a [MenuNode] {
        let mut nodes = menu;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                } else {
                    return &[];
                }
            } else {
                return &[];
            }
        }
        nodes
    }

    /// Get the current submenu label for display (e.g., "goto").
    pub fn current_label(&self, menu: &[MenuNode]) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        let mut nodes = menu;
        let mut label = None;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                label = Some(node.label.clone());
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                }
            }
        }
        label
    }

    fn resolve_node_index(&self, menu: &[MenuNode], key: char) -> Option<usize> {
        let nodes = self.current_nodes(menu);
        // We need the index in the current level's node list
        // But current_nodes returns a slice — we need to find the index
        // Walk the tree to the current level and find the matching key
        let mut target = menu;
        for &idx in &self.path {
            if let Some(node) = target.get(idx) {
                if let MenuAction::Submenu(children) = &node.action {
                    target = children;
                }
            }
        }
        target.iter().position(|n| n.key == key)
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default menu tree.
pub fn default_menu() -> Vec<MenuNode> {
    vec![
        MenuNode {
            key: 'f',
            label: "file browser".into(),
            action: MenuAction::Command(Action::OpenFileBrowser),
        },
        MenuNode {
            key: '/',
            label: "search".into(),
            action: MenuAction::Command(Action::SearchForward),
        },
        MenuNode {
            key: 'q',
            label: "quit".into(),
            action: MenuAction::Command(Action::Quit),
        },
        MenuNode {
            key: 'g',
            label: "goto".into(),
            action: MenuAction::Submenu(vec![
                MenuNode {
                    key: 'g',
                    label: "top".into(),
                    action: MenuAction::Command(Action::JumpTop),
                },
                MenuNode {
                    key: 'e',
                    label: "bottom".into(),
                    action: MenuAction::Command(Action::JumpBottom),
                },
                MenuNode {
                    key: 'h',
                    label: "next heading".into(),
                    action: MenuAction::Command(Action::NextHeading),
                },
            ]),
        },
    ]
}
```

- [ ] **Step 4: Add module to lib.rs**

Add `pub mod menu;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test menu_test`
Expected: All 10 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/menu.rs src/lib.rs tests/menu_test.rs
git commit -m "feat: menu module with tree navigation, submenu support, and defaults"
```

---

### Task 3: File Browser Module — Core Logic

**Files:**
- Create: `src/file_browser.rs`
- Create: `tests/file_browser_test.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml` (add tempfile dev-dependency)

- [ ] **Step 1: Add tempfile dev-dependency**

Add to `[dev-dependencies]` in `Cargo.toml`:

```toml
tempfile = "3"
```

- [ ] **Step 2: Write file browser tests**

Create `tests/file_browser_test.rs`:

```rust
use sketch::file_browser::FileBrowser;
use std::fs;
use tempfile::TempDir;

fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("README.md"), "hello").unwrap();
    fs::write(dir.path().join("Cargo.toml"), "cargo").unwrap();
    fs::write(dir.path().join(".hidden"), "secret").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::create_dir(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("src").join("main.rs"), "fn main(){}").unwrap();
    dir
}

#[test]
fn test_lists_directory() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    assert!(!browser.entries().is_empty());
}

#[test]
fn test_directories_sorted_first() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    let entries = browser.entries();
    // Directories should come before files
    let first_file_idx = entries.iter().position(|e| !e.is_dir);
    let last_dir_idx = entries.iter().rposition(|e| e.is_dir);
    if let (Some(first_file), Some(last_dir)) = (first_file_idx, last_dir_idx) {
        assert!(last_dir < first_file, "Directories should sort before files");
    }
}

#[test]
fn test_hidden_files_excluded() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    assert!(!browser.entries().iter().any(|e| e.name.starts_with('.')));
}

#[test]
fn test_selection_movement() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    assert_eq!(browser.selected(), 0);
    browser.move_down();
    assert_eq!(browser.selected(), 1);
    browser.move_up();
    assert_eq!(browser.selected(), 0);
}

#[test]
fn test_selection_wraps() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.move_up(); // at 0, should wrap to last
    assert_eq!(browser.selected(), browser.visible_entries().len() - 1);
}

#[test]
fn test_enter_directory() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    // Find "src" directory
    let src_idx = browser.entries().iter().position(|e| e.name == "src").unwrap();
    browser.set_selected(src_idx);
    let result = browser.enter_selected();
    assert!(result.is_none()); // entered dir, no file to open
    assert!(browser.current_dir().ends_with("src"));
    assert!(browser.entries().iter().any(|e| e.name == "main.rs"));
}

#[test]
fn test_enter_file() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let file_idx = browser.entries().iter().position(|e| e.name == "README.md").unwrap();
    browser.set_selected(file_idx);
    let result = browser.enter_selected();
    assert!(result.is_some()); // returns file path to open
}

#[test]
fn test_go_parent() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let src_idx = browser.entries().iter().position(|e| e.name == "src").unwrap();
    browser.set_selected(src_idx);
    browser.enter_selected();
    browser.go_parent();
    assert_eq!(browser.current_dir(), dir.path());
}

#[test]
fn test_filter() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.set_filter("read");
    let visible = browser.visible_entries();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "README.md");
}

#[test]
fn test_filter_case_insensitive() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.set_filter("readme");
    let visible = browser.visible_entries();
    assert_eq!(visible.len(), 1);
}

#[test]
fn test_clear_filter() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let all_count = browser.visible_entries().len();
    browser.set_filter("read");
    assert_eq!(browser.visible_entries().len(), 1);
    browser.clear_filter();
    assert_eq!(browser.visible_entries().len(), all_count);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test file_browser_test`
Expected: FAIL — module doesn't exist

- [ ] **Step 4: Implement file_browser module**

Create `src/file_browser.rs`:

```rust
use std::path::{Path, PathBuf};
use std::fs;

#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
}

pub struct FileBrowser {
    root: PathBuf,
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    selected: usize,
    filter_text: String,
    filtered_indices: Vec<usize>,
    pub filter_mode: bool,
}

impl FileBrowser {
    pub fn new(start_dir: PathBuf) -> Self {
        let mut browser = Self {
            root: start_dir.clone(),
            current_dir: start_dir,
            entries: Vec::new(),
            selected: 0,
            filter_text: String::new(),
            filtered_indices: Vec::new(),
            filter_mode: false,
        };
        browser.refresh();
        browser
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn entries(&self) -> &[BrowserEntry] {
        &self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        let max = self.visible_entries().len().saturating_sub(1);
        self.selected = idx.min(max);
    }

    /// Get entries visible after filtering.
    pub fn visible_entries(&self) -> Vec<&BrowserEntry> {
        if self.filter_text.is_empty() {
            self.entries.iter().collect()
        } else {
            self.filtered_indices.iter()
                .filter_map(|&i| self.entries.get(i))
                .collect()
        }
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        let visible = self.visible_entries();
        visible.get(self.selected).copied()
    }

    pub fn move_down(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 { return; }
        self.selected = (self.selected + 1) % len;
    }

    pub fn move_up(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 { return; }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
    }

    /// Enter the selected entry. Returns Some(path) if a file was selected (to open),
    /// or None if a directory was entered.
    pub fn enter_selected(&mut self) -> Option<PathBuf> {
        let entry = self.selected_entry()?.clone();
        if entry.is_dir {
            self.current_dir = entry.path;
            self.selected = 0;
            self.clear_filter();
            self.refresh();
            None
        } else {
            Some(entry.path)
        }
    }

    /// Navigate to parent directory. No-op at filesystem root.
    pub fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.selected = 0;
            self.clear_filter();
            self.refresh();
        }
    }

    pub fn set_filter(&mut self, text: &str) {
        self.filter_text = text.to_string();
        self.update_filtered();
        // Reset selection to 0 when filter changes
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        self.filtered_indices.clear();
        self.filter_mode = false;
        self.selected = 0;
    }

    pub fn filter_text(&self) -> &str {
        &self.filter_text
    }

    /// Calculate panel width in columns.
    pub fn panel_width(&self, terminal_width: u16) -> u16 {
        let raw = (terminal_width as u32 * 30 / 100) as u16;
        raw.clamp(20, 60).min(terminal_width / 2)
    }

    fn refresh(&mut self) {
        self.entries = Self::list_directory(&self.current_dir);
        self.update_filtered();
    }

    fn list_directory(dir: &Path) -> Vec<BrowserEntry> {
        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            let path = entry.path();

            // Follow symlinks — check the resolved metadata
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue, // broken symlink — skip
            };

            let is_dir = metadata.is_dir();
            let browser_entry = BrowserEntry { name, is_dir, path };

            if is_dir {
                dirs.push(browser_entry);
            } else {
                files.push(browser_entry);
            }
        }

        // Sort each group alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Directories first, then files
        dirs.extend(files);
        dirs
    }

    fn update_filtered(&mut self) {
        if self.filter_text.is_empty() {
            self.filtered_indices.clear();
            return;
        }
        let query = self.filter_text.to_lowercase();
        self.filtered_indices = self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
    }
}
```

- [ ] **Step 5: Add module to lib.rs**

Add `pub mod file_browser;` to `src/lib.rs`.

- [ ] **Step 6: Run tests**

Run: `cargo test --test file_browser_test`
Expected: All 11 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/file_browser.rs src/lib.rs Cargo.toml Cargo.lock tests/file_browser_test.rs
git commit -m "feat: file browser module with directory listing, filtering, and navigation"
```

---

### Task 4: App Mode Routing and Menu Integration

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add AppMode enum and menu state to App**

At the top of `src/app.rs`, add:

```rust
use sketch::menu::{self, MenuNode, MenuState};
use sketch::file_browser::FileBrowser;
```

Add an `AppMode` enum:

```rust
#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Menu,
    FileBrowser,
}
```

Add fields to `App`:

```rust
    mode: AppMode,
    menu_state: MenuState,
    menu_tree: Vec<MenuNode>,
    file_browser: Option<FileBrowser>,
```

Initialize in `App::new`:

```rust
    mode: AppMode::Normal,
    menu_state: MenuState::new(),
    menu_tree: menu::default_menu(),
    file_browser: None,
```

- [ ] **Step 2: Route key events by mode**

Refactor `handle_key` to dispatch based on `self.mode`:

```rust
fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
    let size = terminal.size()?;
    let viewport_height = (size.height as usize).saturating_sub(2);
    let content_width = self.effective_content_width(size.width as usize);

    match self.mode {
        AppMode::Normal => self.handle_normal_key(key, viewport_height, content_width),
        AppMode::Menu => self.handle_menu_key(key, viewport_height, content_width),
        AppMode::FileBrowser => self.handle_file_browser_key(key, size.width, viewport_height, content_width),
    }

    Ok(())
}
```

- [ ] **Step 3: Extract handle_normal_key**

Move the existing `handle_key` body (search input mode check + keybind processing) into `handle_normal_key`. Add handling for the new `OpenMenu` action:

```rust
Action::OpenMenu => {
    self.menu_state.open();
    self.mode = AppMode::Menu;
}
Action::OpenFileBrowser => {
    self.open_file_browser();
}
```

- [ ] **Step 4: Implement handle_menu_key**

```rust
fn handle_menu_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
    match key.code {
        KeyCode::Esc => {
            self.menu_state.handle_escape();
            if !self.menu_state.is_active() {
                self.mode = AppMode::Normal;
            }
        }
        KeyCode::Char(c) => {
            if let Some(action) = self.menu_state.process_key(c, &self.menu_tree) {
                self.mode = AppMode::Normal;
                self.execute_action(action, viewport_height, content_width);
            }
        }
        _ => {} // ignore unrecognized keys
    }
}
```

- [ ] **Step 5: Extract execute_action**

Move the action match from `handle_normal_key` into a shared `execute_action` method so both normal keybinds and menu commands use the same dispatch.

- [ ] **Step 6: Implement handle_file_browser_key**

```rust
fn handle_file_browser_key(&mut self, key: KeyEvent, term_width: u16, viewport_height: usize, content_width: usize) {
    let browser = match &mut self.file_browser {
        Some(b) => b,
        None => { self.mode = AppMode::Normal; return; }
    };

    if browser.filter_mode {
        match key.code {
            KeyCode::Esc => browser.clear_filter(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(path) = browser.enter_selected() {
                    if self.load_file(path, content_width) {
                        self.file_browser = None;
                        self.mode = AppMode::Normal;
                    }
                }
            }
            KeyCode::Backspace => {
                let mut text = browser.filter_text().to_string();
                text.pop();
                browser.set_filter(&text);
            }
            KeyCode::Char(c) => {
                let mut text = browser.filter_text().to_string();
                text.push(c);
                browser.set_filter(&text);
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('j') => browser.move_down(),
        KeyCode::Char('k') => browser.move_up(),
        KeyCode::Char(' ') => {
            if let Some(path) = browser.enter_selected() {
                if self.load_file(path, content_width) {
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                }
            }
        }
        KeyCode::Backspace => browser.go_parent(),
        KeyCode::Char('/') => browser.filter_mode = true,
        KeyCode::Char('q') | KeyCode::Esc => {
            self.file_browser = None;
            self.mode = AppMode::Normal;
        }
        _ => {}
    }
}
```

- [ ] **Step 7: Implement open_file_browser and load_file helpers**

```rust
fn open_file_browser(&mut self) {
    let dir = std::path::Path::new(&self.filename)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    self.file_browser = Some(FileBrowser::new(dir));
    self.mode = AppMode::FileBrowser;
}

/// Load a file into the viewer. Returns true on success, false on error
/// (browser should stay open on failure).
fn load_file(&mut self, path: std::path::PathBuf, content_width: usize) -> bool {
    match std::fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(content) => {
                self.filename = path.display().to_string();
                self.blocks = render::render(&content, &self.theme);
                self.viewport.scroll_offset = 0;
                self.viewport.cursor_line = 0;
                self.viewport.calculate_total_lines(&self.blocks, content_width);
                true
            }
            Err(_) => false, // Non-UTF-8: keep browser open
        },
        Err(_) => false, // Read error: keep browser open
    }
}

fn effective_content_width(&self, terminal_width: usize) -> usize {
    let available = if let Some(browser) = &self.file_browser {
        terminal_width.saturating_sub(browser.panel_width(terminal_width as u16) as usize + 1)
    } else {
        terminal_width
    };
    self.viewport.content_width(available)
}
```

- [ ] **Step 8: Update the draw call to pass menu and browser state**

Update the `ViewState` construction and `terminal.draw` call to include the new state. This will require updating `ViewState` (done in Task 5).

For now, add placeholder fields and update the draw call to pass `mode`, `menu_state`, `menu_tree`, and `file_browser`.

- [ ] **Step 9: Update the resize handler**

In the existing `Event::Resize` handler in `run()`, change it to use `effective_content_width`:

```rust
Event::Resize(w, _h) => {
    let cw = self.effective_content_width(w as usize);
    self.viewport.calculate_total_lines(&self.blocks, cw);
}
```

- [ ] **Step 10: Verify it compiles**

Run: `cargo build`
Expected: Compiles (may have warnings about unused fields until view.rs is updated)

- [ ] **Step 11: Commit**

```bash
git add src/app.rs
git commit -m "feat: app mode routing with menu and file browser integration"
```

---

### Task 5: View Layer — Menu Popup Rendering

**Files:**
- Modify: `src/view.rs`

- [ ] **Step 1: Add menu state to ViewState**

Add to `ViewState`:

```rust
    pub menu_active: bool,
    pub menu_nodes: Vec<(char, String, bool)>,  // (key, label, is_submenu)
    pub menu_label: Option<String>,              // submenu breadcrumb label
```

- [ ] **Step 2: Implement menu popup drawing**

Add a `draw_menu_popup` function that renders top-anchored below the top bar:

```rust
fn draw_menu_popup(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Menu takes 2 rows: label row + entries row
    let popup_height = 2u16;
    let popup_area = Rect::new(area.x, area.y, area.width, popup_height.min(area.height));

    // Opaque background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 58)));
    frame.render_widget(bg, popup_area);

    // Label row
    let label_text = state.menu_label.as_deref().unwrap_or("Commands");
    let label_line = Line::from(Span::styled(
        format!("  {}", label_text.to_uppercase()),
        Style::default().fg(Color::Rgb(98, 114, 164)),
    ));
    if popup_area.height >= 1 {
        frame.render_widget(
            Paragraph::new(label_line),
            Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
        );
    }

    // Entries row
    if popup_area.height >= 2 {
        let mut spans = vec![Span::raw("  ")];
        for (i, (key, label, is_submenu)) in state.menu_nodes.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(Color::Rgb(189, 147, 249)).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            if *is_submenu {
                spans.push(Span::styled(
                    format!("{} ▸", label),
                    Style::default().fg(Color::Rgb(139, 233, 253)),
                ));
            } else {
                spans.push(Span::styled(
                    label.clone(),
                    Style::default().fg(Color::Rgb(204, 204, 204)),
                ));
            }
        }
        let entries_line = Line::from(spans);
        frame.render_widget(
            Paragraph::new(entries_line),
            Rect::new(popup_area.x, popup_area.y + 1, popup_area.width, 1),
        );
    }
}
```

- [ ] **Step 3: Call draw_menu_popup from draw()**

After `draw_content`, if `state.menu_active`, call `draw_menu_popup` with the content area (so it overlays the top of the content):

```rust
if state.menu_active {
    draw_menu_popup(frame, content_area, state);
}
```

- [ ] **Step 4: Add necessary imports**

Add `use ratatui::style::{Color, Modifier};` to the imports in `view.rs`.

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`

- [ ] **Step 6: Commit**

```bash
git add src/view.rs
git commit -m "feat: menu popup rendering with top-anchored overlay"
```

---

### Task 6: View Layer — File Browser Panel Rendering

**Files:**
- Modify: `src/view.rs`

- [ ] **Step 1: Add file browser state to ViewState**

Add to `ViewState`:

```rust
    pub file_browser_open: bool,
    pub file_browser_dir: String,
    pub file_browser_entries: Vec<(String, bool, bool)>,  // (name, is_dir, is_selected)
    pub file_browser_filter_mode: bool,
    pub file_browser_filter_text: String,
    pub file_browser_panel_width: u16,
    pub file_browser_hint: String,
```

- [ ] **Step 2: Split layout when file browser is open**

In `draw()`, when `state.file_browser_open`, split the content area horizontally:

```rust
if state.file_browser_open {
    let [browser_area, doc_area] = Layout::horizontal([
        Constraint::Length(state.file_browser_panel_width),
        Constraint::Min(1),
    ]).areas(content_area);

    draw_file_browser_panel(frame, browser_area, state);
    draw_content(frame, doc_area, state);
} else {
    draw_content(frame, content_area, state);
}
```

- [ ] **Step 3: Implement draw_file_browser_panel**

```rust
fn draw_file_browser_panel(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Split panel into: header (1), optional filter (1), file list (fill), footer (1)
    let has_filter = state.file_browser_filter_mode;
    let constraints = if has_filter {
        vec![
            Constraint::Length(1),  // header
            Constraint::Length(1),  // filter input
            Constraint::Min(1),    // file list
            Constraint::Length(1),  // footer
        ]
    } else {
        vec![
            Constraint::Length(1),  // header
            Constraint::Min(1),    // file list
            Constraint::Length(1),  // footer
        ]
    };
    let areas = Layout::vertical(constraints).split(area);

    let (header_area, filter_area, list_area, footer_area) = if has_filter {
        (areas[0], Some(areas[1]), areas[2], areas[3])
    } else {
        (areas[0], None, areas[1], areas[2])
    };

    // Border separator on right edge
    for y in area.y..area.y + area.height {
        let sep_area = Rect::new(area.x + area.width - 1, y, 1, 1);
        frame.render_widget(
            Paragraph::new("│").style(Style::default().fg(Color::Rgb(98, 114, 164))),
            sep_area,
        );
    }

    let panel_width = area.width.saturating_sub(1); // exclude border

    // Header
    let dir_display = if state.file_browser_dir.len() > panel_width as usize - 2 {
        let start = state.file_browser_dir.len() - (panel_width as usize - 2);
        format!(" …{}", &state.file_browser_dir[start..])
    } else {
        format!(" {}", state.file_browser_dir)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            dir_display,
            Style::default().fg(Color::Rgb(98, 114, 164)),
        ))),
        Rect::new(header_area.x, header_area.y, panel_width, 1),
    );

    // Filter input
    if let Some(filter_area) = filter_area {
        let filter_line = Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Rgb(255, 184, 108))),
            Span::styled(&state.file_browser_filter_text, Style::default().fg(Color::Rgb(241, 250, 140))),
            Span::styled("▎", Style::default().fg(Color::Rgb(102, 102, 102))),
        ]);
        frame.render_widget(
            Paragraph::new(filter_line),
            Rect::new(filter_area.x + 1, filter_area.y, panel_width - 1, 1),
        );
    }

    // File list
    let list_height = list_area.height as usize;
    if state.file_browser_entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (empty)",
                Style::default().fg(Color::Rgb(102, 102, 102)),
            ))),
            Rect::new(list_area.x + 1, list_area.y, panel_width - 1, 1),
        );
    }
    for (i, (name, is_dir, is_selected)) in state.file_browser_entries.iter().enumerate() {
        if i >= list_height { break; }

        let marker = if *is_selected { "▸ " } else { "  " };
        let style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };
        let name_style = if *is_dir {
            style.fg(Color::Rgb(139, 233, 253))
        } else {
            style.fg(Color::Rgb(204, 204, 204))
        };

        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(name.clone(), name_style),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(list_area.x + 1, list_area.y + i as u16, panel_width - 1, 1),
        );
    }

    // Footer
    let hint = &state.file_browser_hint;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", hint),
            Style::default().fg(Color::Rgb(102, 102, 102)),
        ))),
        Rect::new(footer_area.x, footer_area.y, panel_width, 1),
    );
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`

- [ ] **Step 5: Commit**

```bash
git add src/view.rs
git commit -m "feat: file browser side panel rendering with filter and selection"
```

---

### Task 7: Wire App State to View State

**Files:**
- Modify: `src/app.rs`
- Modify: `src/view.rs`

- [ ] **Step 1: Update ViewState construction in app.rs**

Update the `terminal.draw` closure to populate all new ViewState fields from the App's state:

```rust
terminal.draw(|frame| {
    let menu_nodes: Vec<(char, String, bool)> = if self.menu_state.is_active() {
        self.menu_state.current_nodes(&self.menu_tree).iter().map(|n| {
            let is_sub = matches!(n.action, menu::MenuAction::Submenu(_));
            (n.key, n.label.clone(), is_sub)
        }).collect()
    } else {
        Vec::new()
    };

    let (fb_open, fb_dir, fb_entries, fb_filter_mode, fb_filter_text, fb_panel_width, fb_hint) =
        if let Some(browser) = &self.file_browser {
            let entries: Vec<(String, bool, bool)> = browser.visible_entries().iter().enumerate().map(|(i, e)| {
                (e.name.clone(), e.is_dir, i == browser.selected())
            }).collect();
            let hint = if browser.filter_mode {
                format!("{} matches · Space open · Esc clear", entries.len())
            } else {
                "j/k nav · Space open · / filter · Esc close".to_string()
            };
            (
                true,
                browser.current_dir().display().to_string(),
                entries,
                browser.filter_mode,
                browser.filter_text().to_string(),
                browser.panel_width(frame.area().width),
                hint,
            )
        } else {
            (false, String::new(), Vec::new(), false, String::new(), 0, String::new())
        };

    let state = ViewState {
        filename: &self.filename,
        blocks: &self.blocks,
        viewport: &self.viewport,
        theme: &self.theme,
        mode_label: match self.mode {
            AppMode::Normal => "NORMAL",
            AppMode::Menu => "NORMAL",
            AppMode::FileBrowser => "NORMAL",
        },
        search_query: &self.search_query,
        search_input_mode: self.search_input_mode,
        search_input_buffer: &self.search_input_buffer,
        search_match_count: self.search_matches.len(),
        menu_active: self.menu_state.is_active(),
        menu_nodes,
        menu_label: self.menu_state.current_label(&self.menu_tree),
        file_browser_open: fb_open,
        file_browser_dir: fb_dir,
        file_browser_entries: fb_entries,
        file_browser_filter_mode: fb_filter_mode,
        file_browser_filter_text: fb_filter_text,
        file_browser_panel_width: fb_panel_width,
        file_browser_hint: fb_hint,
    };
    view::draw(frame, &state);
})?;
```

- [ ] **Step 2: Recalculate total_lines when browser opens/closes**

In `open_file_browser` and after closing the browser, recalculate:

```rust
// After browser state changes that affect content width:
let new_width = self.effective_content_width(terminal_width);
self.viewport.calculate_total_lines(&self.blocks, new_width);
```

The terminal width is available from `terminal.size()` in the event loop. Pass it through or store it as a field on App.

- [ ] **Step 3: Verify everything compiles and runs**

Run: `cargo build && cargo run -- tests/fixtures/showcase.md`
Test: Press Space (menu appears), press f (file browser opens), navigate with j/k, press Space to open a file, press Esc to close.

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/view.rs
git commit -m "feat: wire menu and file browser state to view rendering"
```

---

### Task 8: Polish and Cleanup

**Files:**
- Various

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -- -W clippy::all`
Fix any warnings.

- [ ] **Step 2: Run fmt**

Run: `cargo fmt`

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: All PASS

- [ ] **Step 4: Manual end-to-end test**

Run: `cargo run -- tests/fixtures/showcase.md`

Verify:
- [ ] Space opens menu popup (top-anchored, compact grid)
- [ ] Pressing `f` opens file browser side panel
- [ ] j/k navigates file list
- [ ] Space on directory descends
- [ ] Space on file opens it in viewer
- [ ] Backspace goes to parent dir
- [ ] `/` enters filter mode, typing filters entries
- [ ] Escape in filter clears filter
- [ ] Escape when not filtering closes browser
- [ ] `q` closes browser
- [ ] Menu `g` opens goto submenu
- [ ] `g` then `g` jumps to top
- [ ] Escape in submenu goes back to root menu
- [ ] Escape in root menu closes it
- [ ] Unrecognized keys in menu do nothing
- [ ] Content re-wraps when browser panel opens/closes

- [ ] **Step 5: Update snapshots if needed**

Run: `cargo insta test --accept`

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "chore: clippy fixes, formatting, and snapshot updates"
```
