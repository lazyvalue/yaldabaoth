# Buffer System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-file buffer support so users can open, switch, close, and browse multiple files with preserved per-buffer state.

**Architecture:** A new `Buffer` struct encapsulates per-file state (editor, viewport, view mode, render cache). `App` holds a `Vec<Buffer>` and an active index. Buffer operations (open, close, switch, cycle) live in `App`. A buffer list UI panel appears below the top bar with fuzzy search.

**Tech Stack:** Rust, ratatui, crossterm

---

### Task 1: Create Buffer Struct and Move Per-File State

**Files:**
- Create: `src/buffer.rs`
- Modify: `src/lib.rs`
- Modify: `src/app.rs`

This is the core refactor. Extract per-file fields from `App` into `Buffer`, then change `App` to hold `Vec<Buffer>` + `active_buffer`.

- [ ] **Step 1: Create `src/buffer.rs` with the Buffer struct**

```rust
use sketch::blocks::RenderedBlock;
use sketch::config::Config;
use sketch::editor::Editor;
use sketch::highlight::Highlighter;
use sketch::render;
use sketch::theme::Theme;
use sketch::view::ViewMode;
use sketch::viewport::Viewport;

pub struct Buffer {
    pub editor: Editor,
    pub viewport: Viewport,
    pub view_mode: ViewMode,
    pub highlighter: Highlighter,
    pub rendered_cache: Vec<RenderedBlock>,
    pub view_cache_dirty: bool,
}

impl Buffer {
    pub fn new(filename: String, content: String, config: &Config, theme: &Theme) -> Self {
        let editor = Editor::new(content, std::path::PathBuf::from(&filename));
        let viewport = Viewport::new(config.max_line_width);
        let syntect_theme = theme.name.syntect_theme();
        Self {
            editor,
            viewport,
            view_mode: ViewMode::Rendered,
            highlighter: Highlighter::with_syntect_theme(syntect_theme),
            rendered_cache: Vec::new(),
            view_cache_dirty: true,
        }
    }

    pub fn rebuild_render_cache(&mut self, theme: &Theme) {
        let text = self.editor.document().full_text();
        self.rendered_cache = render::render_with_highlighter(&text, theme, &self.highlighter);
    }

    pub fn update_total_lines(&mut self, content_width: usize) {
        match self.view_mode {
            ViewMode::Rendered => {
                self.viewport.total_lines = self
                    .rendered_cache
                    .iter()
                    .map(|b| self.viewport.block_height(b, content_width))
                    .sum();
            }
            ViewMode::Raw => {
                self.viewport.total_lines = self.editor.document().line_count();
            }
        }
    }

    /// The canonical file path for deduplication.
    pub fn file_path(&self) -> &std::path::Path {
        &self.editor.document().file_path
    }
}
```

Note: this is in the library crate (`src/buffer.rs`), so use `crate::` imports, not `sketch::`. The code above shows the intent — adjust imports to `use crate::blocks::RenderedBlock;` etc.

- [ ] **Step 2: Add `pub mod buffer;` to `src/lib.rs`**

Add after `pub mod blocks;`:
```rust
pub mod buffer;
```

- [ ] **Step 3: Refactor `App` to use `Vec<Buffer>` + `active_buffer`**

This is the big change. In `src/app.rs`:

Remove these fields from `App`:
```
editor, viewport, view_mode, highlighter, rendered_cache, view_cache_dirty
```

Add these fields:
```rust
    buffers: Vec<Buffer>,
    active_buffer: usize,
```

Add helper methods:
```rust
    fn active(&self) -> &Buffer {
        &self.buffers[self.active_buffer]
    }

    fn active_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_buffer]
    }
```

Update `App::new`:
```rust
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::from_name(config.theme);
        // ... keybind/menu config setup stays the same ...
        let registry = CommandRegistry::default_registry();

        let buffer = Buffer::new(filename, markdown, config, &theme);

        Self {
            buffers: vec![buffer],
            active_buffer: 0,
            theme,
            keybinds,
            registry,
            should_quit: false,
            search_query: String::new(),
            search_input_mode: false,
            search_input_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            mode: AppMode::Normal,
            menu_state: MenuState::new(),
            menu_tree,
            file_browser: None,
            command_buffer: String::new(),
            command_error: String::new(),
        }
    }
```

Add `use sketch::buffer::Buffer;` to imports. Remove `use sketch::editor::Editor;`, `use sketch::highlight::Highlighter;`, `use sketch::viewport::Viewport;`, `use sketch::blocks::RenderedBlock;` (unless still needed elsewhere in app.rs — `RenderedBlock` is used in `find_next_heading` etc., so keep that one).

Now do a systematic find-replace throughout `app.rs`. Every occurrence needs updating:

| Old | New |
|---|---|
| `self.editor` | `self.active().editor` or `self.active_mut().editor` |
| `self.viewport` | `self.active().viewport` or `self.active_mut().viewport` |
| `self.view_mode` | `self.active().view_mode` or `self.active_mut().view_mode` |
| `self.rendered_cache` | `self.active().rendered_cache` |
| `self.view_cache_dirty` | `self.active_mut().view_cache_dirty` |
| `self.rebuild_render_cache()` | `self.active_mut().rebuild_render_cache(&self.theme)` |
| `self.update_total_lines(cw)` | `self.active_mut().update_total_lines(cw)` |

Key methods that need the substitution:
- `run()` — cache rebuild, raw_lines, draw closure (all the `self.editor.document()` calls, `self.viewport`, `self.view_mode`, `self.rendered_cache`)
- `handle_key()` — `self.effective_content_width` needs adjustment
- `handle_normal_key()` — search uses `self.editor`
- `handle_insert_key()` — all `self.editor` calls
- `handle_command_key()` — no editor access
- `handle_menu_key()` — no editor access
- `handle_file_browser_key()` — `self.load_file` (will become `self.open_buffer`)
- `execute_action()` — heavy use of `self.editor`, `self.viewport`, `self.view_mode`
- `execute_command()` — `self.editor.save_to`
- `ensure_cursor_visible()` — `self.editor`, `self.viewport`
- `doc_line_to_rendered_y()` — `self.editor`, `self.viewport`, `self.rendered_cache`
- `open_file_browser()` — `self.editor.document()`
- `load_file()` — will become `open_buffer()`
- `effective_content_width()` — `self.viewport`
- `perform_search()` — `self.editor`
- `jump_to_match()` — `self.editor`
- `find_next_heading()`, `find_prev_heading()`, `heading_level_at_offset()` — `self.viewport`, `self.rendered_cache`

**Borrow checker challenges:** Methods like `execute_action` that call `self.active_mut().editor` AND `self.active_mut().viewport` need care. Since `active_mut()` borrows all of `self`, you may need to access `self.buffers[self.active_buffer].editor` directly in places where multiple mutable borrows are needed. The `ensure_raw_for_editing` method accesses `self.view_mode` which becomes `self.buffers[self.active_buffer].view_mode`.

For the `run()` draw closure, you'll need to extract data from the active buffer before the closure:
```rust
let buf = &self.buffers[self.active_buffer];
let view_cache_dirty = buf.view_cache_dirty;
// ... etc
```

Then inside the closure, borrow `self` carefully.

- [ ] **Step 4: Move `effective_content_width` to `Buffer` or keep in `App`**

The method uses `self.file_browser` (App state) and `self.viewport` (Buffer state). Keep it in `App` but access viewport through the active buffer:

```rust
    fn effective_content_width(&self, terminal_width: usize) -> usize {
        let available = if let Some(browser) = &self.file_browser {
            terminal_width.saturating_sub(browser.panel_width(terminal_width as u16) as usize + 1)
        } else {
            terminal_width
        };
        self.active().viewport.content_width(available)
    }
```

- [ ] **Step 5: Replace `load_file` with `open_buffer`**

```rust
    fn open_buffer(&mut self, path: std::path::PathBuf) -> bool {
        // Check if already open — switch to it
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        for (i, buf) in self.buffers.iter().enumerate() {
            let buf_path = buf.file_path().canonicalize()
                .unwrap_or_else(|_| buf.file_path().to_path_buf());
            if buf_path == canonical {
                self.active_buffer = i;
                return true;
            }
        }

        // Open new buffer
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    let config_stub = sketch::config::Config::default();
                    let buffer = Buffer::new(
                        canonical.display().to_string(),
                        content,
                        &config_stub,
                        &self.theme,
                    );
                    self.buffers.push(buffer);
                    self.active_buffer = self.buffers.len() - 1;
                    true
                }
                Err(_) => false,
            },
            Err(_) => false,
        }
    }
```

Note: For `open_buffer`, we need `Config` to get `max_line_width`. The simplest approach: store `max_line_width` in `App` (it's already available during `App::new`). Or use a default. Since viewport width comes from config, store it:

Add `max_line_width: usize` field to `App`, set it from `config.max_line_width` in `new()`. Then `Buffer::new` can take `max_line_width: usize` directly instead of a full `Config`.

Actually, let's simplify `Buffer::new` to take `max_line_width`:

```rust
impl Buffer {
    pub fn new(filename: String, content: String, max_line_width: usize, theme: &Theme) -> Self {
        let editor = Editor::new(content, std::path::PathBuf::from(&filename));
        let viewport = Viewport::new(max_line_width);
        let syntect_theme = theme.name.syntect_theme();
        Self {
            editor,
            viewport,
            view_mode: ViewMode::Rendered,
            highlighter: Highlighter::with_syntect_theme(syntect_theme),
            rendered_cache: Vec::new(),
            view_cache_dirty: true,
        }
    }
}
```

- [ ] **Step 6: Update all file browser references from `load_file` to `open_buffer`**

In `handle_file_browser_key`, change both occurrences:
```rust
// Old:
if let Some(path) = browser.enter_selected()
    && self.load_file(path, content_width)

// New:
if let Some(path) = browser.enter_selected()
    && self.open_buffer(path)
```

- [ ] **Step 7: Update `open_file_browser` to no longer require save check**

With buffers, we don't need to warn about unsaved changes when browsing — the current buffer is preserved. Remove the modified check:

```rust
    fn open_file_browser(&mut self) {
        let dir = self
            .active()
            .editor
            .document()
            .file_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }
```

- [ ] **Step 8: Build and verify compilation**

Run: `cargo build 2>&1 | tail -10`
Expected: compiles (possibly with warnings)

- [ ] **Step 9: Run tests**

Run: `cargo test 2>&1 | tail -10`
Expected: all existing tests pass (buffer.rs has no tests yet, menu/keybind/config tests don't touch App)

- [ ] **Step 10: Commit**

```bash
git add src/buffer.rs src/lib.rs src/app.rs
git commit -m "refactor: extract Buffer struct, App holds Vec<Buffer>"
```

---

### Task 2: Add Buffer Commands, Keybindings, and Menu Entry

**Files:**
- Modify: `src/keybind.rs`
- Modify: `src/command.rs`
- Modify: `src/menu.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Add Action variants to `src/keybind.rs`**

Add to the `Action` enum (in `src/keybind.rs` — that's where it lives currently, check if it was moved to `command.rs`):

```rust
    NextBuffer,
    PrevBuffer,
    BufferList,
    CloseBuffer,
```

- [ ] **Step 2: Add default keybindings in `src/keybind.rs`**

In the `Default` impl for `KeybindManager`, add:

```rust
        single.insert(
            KeyPress::new(KeyCode::Tab, KeyModifiers::NONE),
            "next-buffer".into(),
        );
        single.insert(
            KeyPress::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            "prev-buffer".into(),
        );
```

Note: crossterm represents Shift+Tab as `KeyCode::BackTab`. Check the exact representation — it might be `KeyCode::BackTab` with no modifiers, or `KeyCode::Tab` with `KeyModifiers::SHIFT`. Test both. The safest is:

```rust
        single.insert(
            KeyPress::new(KeyCode::Tab, KeyModifiers::NONE),
            "next-buffer".into(),
        );
        single.insert(
            KeyPress::new(KeyCode::BackTab, KeyModifiers::NONE),
            "prev-buffer".into(),
        );
```

- [ ] **Step 3: Register commands in `src/command.rs`**

Add to `default_registry()`:

```rust
            CommandDef {
                name: "next-buffer".into(),
                aliases: vec!["bn".into()],
                action: Action::NextBuffer,
                description: "Switch to next buffer".into(),
            },
            CommandDef {
                name: "prev-buffer".into(),
                aliases: vec!["bp".into()],
                action: Action::PrevBuffer,
                description: "Switch to previous buffer".into(),
            },
            CommandDef {
                name: "buffer-list".into(),
                aliases: vec!["buffers".into(), "ls".into()],
                action: Action::BufferList,
                description: "Show buffer list".into(),
            },
            CommandDef {
                name: "close-buffer".into(),
                aliases: vec!["bd".into()],
                action: Action::CloseBuffer,
                description: "Close current buffer".into(),
            },
```

- [ ] **Step 4: Add menu entry in `src/menu.rs`**

In `default_menu()`, add before the "goto" submenu entry:

```rust
        MenuNode::entry("b", "buffers", "buffer-list"),
```

- [ ] **Step 5: Handle new actions in `App::execute_action`**

In `src/app.rs`, add to the `execute_action` match:

```rust
            Action::NextBuffer => {
                if self.buffers.len() > 1 {
                    self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
                }
            }
            Action::PrevBuffer => {
                if self.buffers.len() > 1 {
                    self.active_buffer = if self.active_buffer == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.active_buffer - 1
                    };
                }
            }
            Action::BufferList => {
                self.buffer_list_selected = self.active_buffer;
                self.buffer_list_filter_mode = false;
                self.buffer_list_filter_text.clear();
                self.mode = AppMode::BufferList;
            }
            Action::CloseBuffer => {
                self.close_current_buffer();
            }
```

Add `close_current_buffer` method:

```rust
    fn close_current_buffer(&mut self) {
        if self.active().editor.document().is_modified() {
            self.command_error =
                "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(self.active_buffer);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
    }
```

Add new fields to `App`:
```rust
    buffer_list_selected: usize,
    buffer_list_filter_mode: bool,
    buffer_list_filter_text: String,
```

And initialize them in `App::new`:
```rust
    buffer_list_selected: 0,
    buffer_list_filter_mode: false,
    buffer_list_filter_text: String::new(),
```

Add `BufferList` to `AppMode`:
```rust
enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
    BufferList,
}
```

- [ ] **Step 6: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles and tests pass

- [ ] **Step 7: Commit**

```bash
git add src/keybind.rs src/command.rs src/menu.rs src/app.rs
git commit -m "feat: add buffer commands, keybindings, and menu entry"
```

---

### Task 3: Buffer List Key Handling

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add buffer list key handler**

Add to the `handle_key` match in `App`:

```rust
            AppMode::BufferList => self.handle_buffer_list_key(key, viewport_height, content_width),
```

Implement the handler:

```rust
    fn handle_buffer_list_key(&mut self, key: KeyEvent, _viewport_height: usize, _content_width: usize) {
        if self.buffer_list_filter_mode {
            match key.code {
                KeyCode::Esc => {
                    self.buffer_list_filter_mode = false;
                    self.buffer_list_filter_text.clear();
                    self.buffer_list_selected = 0;
                }
                KeyCode::Enter => {
                    let filtered = self.filtered_buffer_indices();
                    if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                        self.active_buffer = buf_idx;
                        self.mode = AppMode::Normal;
                    }
                }
                KeyCode::Backspace => {
                    self.buffer_list_filter_text.pop();
                    self.buffer_list_selected = 0;
                }
                KeyCode::Char(c) => {
                    self.buffer_list_filter_text.push(c);
                    self.buffer_list_selected = 0;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.visible_buffer_count();
                if count > 0 {
                    self.buffer_list_selected = (self.buffer_list_selected + 1) % count;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = self.visible_buffer_count();
                if count > 0 {
                    self.buffer_list_selected = if self.buffer_list_selected == 0 {
                        count - 1
                    } else {
                        self.buffer_list_selected - 1
                    };
                }
            }
            KeyCode::Enter => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.active_buffer = buf_idx;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('d') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.close_buffer_at(buf_idx);
                }
            }
            KeyCode::Char('/') => {
                self.buffer_list_filter_mode = true;
                self.buffer_list_filter_text.clear();
                self.buffer_list_selected = 0;
            }
            _ => {}
        }
    }
```

- [ ] **Step 2: Add helper methods**

```rust
    fn close_buffer_at(&mut self, index: usize) {
        if self.buffers[index].editor.document().is_modified() {
            self.command_error =
                "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(index);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
        // Adjust selection
        let count = self.visible_buffer_count();
        if self.buffer_list_selected >= count && count > 0 {
            self.buffer_list_selected = count - 1;
        }
        if self.buffers.len() == 0 {
            self.mode = AppMode::Normal;
        }
    }

    fn visible_buffer_count(&self) -> usize {
        self.filtered_buffer_indices().len()
    }

    fn filtered_buffer_indices(&self) -> Vec<usize> {
        if self.buffer_list_filter_text.is_empty() {
            return (0..self.buffers.len()).collect();
        }
        let query = self.buffer_list_filter_text.to_lowercase();
        (0..self.buffers.len())
            .filter(|&i| {
                let path = self.buffers[i].file_path().display().to_string().to_lowercase();
                fuzzy_match(&path, &query)
            })
            .collect()
    }
```

- [ ] **Step 3: Add fuzzy match function**

Add at the bottom of `src/app.rs` (outside `impl App`):

```rust
/// Simple fuzzy match: all chars in `query` appear in `text` in order.
fn fuzzy_match(text: &str, query: &str) -> bool {
    let mut text_chars = text.chars();
    for qc in query.chars() {
        loop {
            match text_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}
```

- [ ] **Step 4: Update mode_label for BufferList**

In the `run()` draw closure where `mode_label` is set, add:
```rust
                        AppMode::BufferList => "NORMAL",
```

- [ ] **Step 5: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles and tests pass

- [ ] **Step 6: Commit**

```bash
git add src/app.rs
git commit -m "feat: buffer list key handling with fuzzy search"
```

---

### Task 4: Buffer List UI Rendering

**Files:**
- Modify: `src/view.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Add buffer list fields to `ViewState`**

In `src/view.rs`, add to `ViewState`:

```rust
    pub buffer_list_open: bool,
    pub buffer_list_entries: Vec<(String, bool, bool)>, // (path, is_modified, is_selected)
    pub buffer_list_active_index: usize, // which buffer is currently active (viewed)
    pub buffer_list_filter_mode: bool,
    pub buffer_list_filter_text: String,
    pub buffer_count: usize,
```

- [ ] **Step 2: Populate buffer list fields in `App::run` draw closure**

In `src/app.rs`, inside the draw closure, add before the `ViewState` construction:

```rust
                let buffer_list_entries: Vec<(String, bool, bool)> = if self.mode == AppMode::BufferList {
                    let filtered = self.filtered_buffer_indices();
                    filtered.iter().enumerate().map(|(i, &buf_idx)| {
                        let path = self.buffers[buf_idx].file_path().display().to_string();
                        let modified = self.buffers[buf_idx].editor.document().is_modified();
                        let selected = i == self.buffer_list_selected;
                        (path, modified, selected)
                    }).collect()
                } else {
                    Vec::new()
                };
```

Add to the `ViewState` initialization:

```rust
                    buffer_list_open: self.mode == AppMode::BufferList,
                    buffer_list_entries,
                    buffer_list_active_index: self.active_buffer,
                    buffer_list_filter_mode: self.buffer_list_filter_mode,
                    buffer_list_filter_text: self.buffer_list_filter_text.clone(),
                    buffer_count: self.buffers.len(),
```

- [ ] **Step 3: Update the `draw` function layout in `src/view.rs`**

Change the main layout in `draw()` to account for the buffer list panel:

```rust
pub fn draw(frame: &mut Frame, state: &ViewState) {
    let area = frame.area();

    if area.width < 40 || area.height < 5 {
        let msg = Paragraph::new("Terminal too small (min 40x5)");
        frame.render_widget(msg, area);
        return;
    }

    // Calculate buffer list height
    let buffer_list_height = if state.buffer_list_open {
        let max_height = (area.height as usize) / 3;
        let entry_rows = state.buffer_list_entries.len() + if state.buffer_list_filter_mode { 1 } else { 0 };
        (entry_rows.min(max_height).max(1)) as u16
    } else {
        0
    };

    let layout = if buffer_list_height > 0 {
        Layout::vertical([
            Constraint::Length(1),                    // top bar
            Constraint::Length(buffer_list_height),   // buffer list
            Constraint::Min(1),                       // content
            Constraint::Length(1),                     // bottom bar
        ]).split(area)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Min(1),
            Constraint::Length(1),
        ]).split(area)
    };

    let top_bar = layout[0];
    let buffer_list_area = layout[1];
    let content_area = layout[2];
    let bottom_bar = layout[3];

    draw_top_bar(frame, top_bar, state);

    if state.buffer_list_open {
        draw_buffer_list(frame, buffer_list_area, state);
    }

    if state.file_browser_open {
        let [browser_area, doc_area] = Layout::horizontal([
            Constraint::Length(state.file_browser_panel_width),
            Constraint::Min(1),
        ])
        .areas(content_area);

        draw_file_browser_panel(frame, browser_area, state);
        draw_content(frame, doc_area, state);
    } else {
        draw_content(frame, content_area, state);
    }
    if state.menu_active {
        draw_menu_popup(frame, content_area, state);
    }
    draw_bottom_bar(frame, bottom_bar, state);
}
```

- [ ] **Step 4: Implement `draw_buffer_list`**

```rust
fn draw_buffer_list(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    let mut y = 0u16;

    // Filter input row
    if state.buffer_list_filter_mode {
        if y < area.height {
            let filter_line = Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::Rgb(255, 184, 108))),
                Span::styled(
                    &state.buffer_list_filter_text,
                    Style::default().fg(Color::Rgb(241, 250, 140)),
                ),
                Span::styled("\u{258e}", Style::default().fg(Color::Rgb(102, 102, 102))),
            ]);
            frame.render_widget(
                Paragraph::new(filter_line),
                Rect::new(area.x + 1, area.y + y, area.width.saturating_sub(1), 1),
            );
            y += 1;
        }
    }

    // Buffer entries
    for (i, (path, is_modified, is_selected)) in state.buffer_list_entries.iter().enumerate() {
        if y >= area.height {
            break;
        }

        let is_active = i == state.buffer_list_active_index;
        let marker = if *is_selected { "\u{25b8} " } else { "  " };
        let modified_indicator = if *is_modified { " [+]" } else { "" };
        let active_indicator = if is_active { " *" } else { "" };

        let style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };

        let path_style = if is_active {
            style.fg(Color::Rgb(139, 233, 253))
        } else {
            style.fg(Color::Rgb(204, 204, 204))
        };

        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(path.clone(), path_style),
            Span::styled(modified_indicator, style.fg(Color::Rgb(255, 184, 108))),
            Span::styled(active_indicator, style.fg(Color::Rgb(98, 114, 164))),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x + 1, area.y + y, area.width.saturating_sub(1), 1),
        );
        y += 1;
    }
}
```

Note: `buffer_list_active_index` in the draw function needs to map to the filtered list position, not the raw buffer index. Adjust: pass the active buffer's position within the filtered list from `app.rs`:

Actually, let's simplify: the `buffer_list_entries` already contains `(path, is_modified, is_selected)` — where `is_selected` marks the cursor. We need one more flag: `is_active` (the buffer being viewed). Change the tuple to include 4 elements:

```rust
    pub buffer_list_entries: Vec<(String, bool, bool, bool)>, // (path, is_modified, is_active, is_selected)
```

Update app.rs to build entries with the active flag:
```rust
                    filtered.iter().enumerate().map(|(i, &buf_idx)| {
                        let path = self.buffers[buf_idx].file_path().display().to_string();
                        let modified = self.buffers[buf_idx].editor.document().is_modified();
                        let is_active = buf_idx == self.active_buffer;
                        let selected = i == self.buffer_list_selected;
                        (path, modified, is_active, selected)
                    }).collect()
```

And update `draw_buffer_list` to destructure `(path, is_modified, is_active, is_selected)`.

- [ ] **Step 5: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles and tests pass

- [ ] **Step 6: Commit**

```bash
git add src/view.rs src/app.rs
git commit -m "feat: buffer list UI panel with fuzzy search"
```

---

### Task 5: Update Top Bar with Buffer Count

**Files:**
- Modify: `src/view.rs`

- [ ] **Step 1: Show buffer count in top bar when multiple buffers are open**

In `draw_top_bar`, add a buffer indicator when `state.buffer_count > 1`. After the position string, add something like `[2/5]` showing active buffer number / total:

Find the `draw_top_bar` function and update the position string:

```rust
    let buffer_info = if state.buffer_count > 1 {
        format!(" [{}/{}]", state.buffer_list_active_index + 1, state.buffer_count)
    } else {
        String::new()
    };
    let position = format!("line {}/{} {}%{}", current_line, total, percent, buffer_info);
```

Note: `buffer_list_active_index` is the active buffer index from the `App`. It needs to be passed properly. Use the existing field we already added to `ViewState`.

Actually we already have `buffer_count` and `buffer_list_active_index` — but `buffer_list_active_index` might only be populated when the list is open. Let's add a dedicated field:

Add to ViewState:
```rust
    pub active_buffer_index: usize,
```

Set it in app.rs:
```rust
    active_buffer_index: self.active_buffer,
```

Then in `draw_top_bar`:
```rust
    let buffer_info = if state.buffer_count > 1 {
        format!(" [{}/{}]", state.active_buffer_index + 1, state.buffer_count)
    } else {
        String::new()
    };
```

- [ ] **Step 2: Build and test**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add src/view.rs src/app.rs
git commit -m "feat: show buffer count in top bar"
```

---

### Task 6: Final Verification and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy 2>&1`
Expected: no errors

- [ ] **Step 3: Fix any issues found**

Address clippy warnings if any.

- [ ] **Step 4: Commit fixes**

```bash
git add -A
git commit -m "chore: clippy fixes and cleanup"
```
