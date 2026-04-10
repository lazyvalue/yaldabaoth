# Full-Screen File Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a full-screen file browser mode that reuses the existing `FileBrowser` state and provides a richer view with metadata columns, breadcrumbs, and keybind hints.

**Architecture:** New `AppScreen` enum on `App` gates whether draw/input goes to the editor or the full-screen browser. The existing dropdown browser is untouched. Both share one `FileBrowser` instance. A new `draw_full_file_browser()` in `view.rs` owns the full-screen layout.

**Tech Stack:** Rust, ratatui, crossterm, std::fs metadata

---

### Task 1: Add metadata fields to BrowserEntry

**Files:**
- Modify: `src/file_browser.rs:10-15` (BrowserEntry struct)
- Modify: `src/file_browser.rs:149-204` (list_directory)
- Modify: `src/file_browser.rs:248-302` (search_recursive)
- Modify: `tests/file_browser_test.rs`

- [ ] **Step 1: Write a failing test for metadata fields**

Add to `tests/file_browser_test.rs`:

```rust
#[test]
fn test_entries_have_metadata() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    let file_entry = browser.entries().iter().find(|e| e.name == "README.md").unwrap();
    assert!(file_entry.size.is_some());
    assert!(file_entry.modified.is_some());
    // Dirs should also have modified time
    let dir_entry = browser.entries().iter().find(|e| e.name == "src").unwrap();
    assert!(dir_entry.modified.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test file_browser_test test_entries_have_metadata`
Expected: FAIL — `size` and `modified` fields don't exist on `BrowserEntry`

- [ ] **Step 3: Add fields to BrowserEntry**

In `src/file_browser.rs`, update the struct:

```rust
#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}
```

- [ ] **Step 4: Populate metadata in `list_directory`**

In `src/file_browser.rs`, in `list_directory`, after `let metadata = match fs::metadata(&path) { ... };`, capture size and mtime, then pass them into the `BrowserEntry` constructors:

```rust
let size = if metadata.is_file() { Some(metadata.len()) } else { None };
let modified = metadata.modified().ok();

let browser_entry = BrowserEntry { name, is_dir, path, size, modified };
```

Also update the `..` parent entry at the bottom of `list_directory`:

```rust
result.push(BrowserEntry {
    name: "..".to_string(),
    is_dir: true,
    path: parent.to_path_buf(),
    size: None,
    modified: None,
});
```

- [ ] **Step 5: Populate metadata in `search_recursive`**

In `search_recursive`, the `BrowserEntry` constructor (around line 291) should also include:

```rust
let size = if metadata.is_file() { Some(metadata.len()) } else { None };
let modified = metadata.modified().ok();

results.push(BrowserEntry {
    name: relative,
    is_dir,
    path: path.clone(),
    size,
    modified,
});
```

- [ ] **Step 6: Run all file_browser tests**

Run: `cargo test --test file_browser_test`
Expected: All pass, including `test_entries_have_metadata`

- [ ] **Step 7: Run full build to check nothing else broke**

Run: `cargo build 2>&1`
Expected: Clean build. The dropdown renderer in `view.rs` constructs tuples `(name, is_dir, is_selected)` from entries — it doesn't touch `size` or `modified`, so no breakage.

- [ ] **Step 8: Commit**

```bash
git add src/file_browser.rs tests/file_browser_test.rs
git commit -m "feat: add size and modified metadata to BrowserEntry"
```

---

### Task 2: Add `AppScreen` enum and `OpenFileBrowserFull` action

**Files:**
- Modify: `src/app.rs:18-27` (AppMode enum area — add AppScreen above it)
- Modify: `src/app.rs:29-60` (App struct — add `screen` field)
- Modify: `src/app.rs:63-124` (App::new — init `screen`)
- Modify: `src/keybind.rs:10-75` (Action enum — add variant)
- Modify: `src/command.rs:39-385` (default_registry — add command)
- Modify: `src/menu.rs:198-220` (default_menu — add entry)

- [ ] **Step 1: Add `OpenFileBrowserFull` to the Action enum**

In `src/keybind.rs`, add after `NavActivate,`:

```rust
OpenFileBrowserFull,
```

- [ ] **Step 2: Add `AppScreen` enum to `app.rs`**

In `src/app.rs`, add before the existing `AppMode` enum (around line 18):

```rust
#[derive(Debug, PartialEq)]
enum AppScreen {
    Editor,
    FileBrowser { came_from_dropdown: bool },
}
```

- [ ] **Step 3: Add `screen` field to `App` struct**

In `src/app.rs`, add to the `App` struct after `should_quit`:

```rust
screen: AppScreen,
```

And in `App::new`, initialize it:

```rust
screen: AppScreen::Editor,
```

- [ ] **Step 4: Register `file-browser-full` command**

In `src/command.rs`, add a new `CommandDef` inside `default_registry()`, after the `nav-activate` entry:

```rust
CommandDef {
    name: "file-browser-full".into(),
    aliases: vec![],
    action: Action::OpenFileBrowserFull,
    description: "Open full-screen file browser".into(),
},
```

Also add `"file-browser-full"` to the `test_all_actions_have_commands` expected list, and add `"reload"` and `"outline"` if not already present (check the list — the existing test enumerates expected commands).

- [ ] **Step 5: Add menu entry**

In `src/menu.rs`, in `default_menu()`, add after the `MenuNode::entry("f", "file browser", "file-browser")` line:

```rust
MenuNode::entry("F", "file browser (full)", "file-browser-full"),
```

- [ ] **Step 6: Handle action in `execute_action`**

In `src/app.rs`, in the `execute_action` method, add the `OpenFileBrowserFull` arm in the `Action::None | Action::FileBrowser*` block. Replace that block so it includes the new action:

```rust
Action::OpenFileBrowserFull => {
    self.open_file_browser_full(false);
}
```

And add to the no-op arm:

```rust
Action::None
| Action::FileBrowserDown
| Action::FileBrowserUp
| Action::FileBrowserEnter
| Action::FileBrowserParentDir
| Action::FileBrowserFilter
| Action::FileBrowserClose
| Action::OpenFileBrowserFull => {}
```

Wait — that's contradictory. The `OpenFileBrowserFull` should be its own arm *before* the no-op arm. Add it right before `Action::None`:

```rust
Action::OpenFileBrowserFull => {
    self.open_file_browser_full(false);
}
```

- [ ] **Step 7: Add `open_file_browser_full` method stub**

In `src/app.rs`, add after `open_file_browser`:

```rust
fn open_file_browser_full(&mut self, came_from_dropdown: bool) {
    if self.file_browser.is_none() {
        let dir = std::env::current_dir().unwrap_or_default();
        self.file_browser = Some(FileBrowser::new(dir));
    }
    self.screen = AppScreen::FileBrowser { came_from_dropdown };
}
```

- [ ] **Step 8: Build and verify**

Run: `cargo build 2>&1`
Expected: Clean build. The `screen` field exists but isn't used in routing yet — that's Task 3.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs src/keybind.rs src/command.rs src/menu.rs
git commit -m "feat: add AppScreen enum, OpenFileBrowserFull action and command"
```

---

### Task 3: Route draw and input through AppScreen

**Files:**
- Modify: `src/app.rs:137-313` (run loop and handle_key)
- Modify: `src/view.rs:19-63` (ViewState)
- Modify: `src/view.rs:65-142` (draw function)

- [ ] **Step 1: Add `full_browser` field to ViewState**

In `src/view.rs`, add these structs before `ViewState`:

```rust
pub struct FullBrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_selected: bool,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

pub struct FullBrowserViewState {
    pub dir: String,
    pub entries: Vec<FullBrowserEntry>,
    pub filter_mode: bool,
    pub filter_text: String,
    pub came_from_dropdown: bool,
}
```

Add to `ViewState`:

```rust
pub full_browser: Option<FullBrowserViewState>,
```

- [ ] **Step 2: Early-return in `draw()` for full-screen browser**

In `src/view.rs`, at the top of `draw()`, after the terminal-too-small check, add:

```rust
if let Some(ref fb_state) = state.full_browser {
    draw_full_file_browser(frame, area, fb_state, state.theme);
    return;
}
```

Add a stub for the draw function at the bottom of `view.rs`:

```rust
fn draw_full_file_browser(frame: &mut Frame, area: Rect, fb: &FullBrowserViewState, theme: &Theme) {
    // Placeholder — filled in Task 4
    let bg = Paragraph::new("FILE BROWSER").style(Style::default().fg(Color::White));
    frame.render_widget(bg, area);
}
```

- [ ] **Step 3: Build ViewState with `full_browser` in app.rs**

In `src/app.rs`, inside the `terminal.draw` closure, construct `full_browser`. Before the `let state = ViewState { ... }` block, add:

```rust
let full_browser_state = if let AppScreen::FileBrowser { came_from_dropdown } = self.screen {
    if let Some(browser) = &self.file_browser {
        let entries: Vec<view::FullBrowserEntry> = browser
            .visible_entries()
            .iter()
            .enumerate()
            .map(|(i, e)| view::FullBrowserEntry {
                name: e.name.clone(),
                is_dir: e.is_dir,
                is_selected: i == browser.selected(),
                size: e.size,
                modified: e.modified,
            })
            .collect();
        Some(view::FullBrowserViewState {
            dir: browser.current_dir().display().to_string(),
            entries,
            filter_mode: browser.filter_mode,
            filter_text: browser.filter_text().to_string(),
            came_from_dropdown,
        })
    } else {
        None
    }
} else {
    None
};
```

Then add to the `ViewState` initializer:

```rust
full_browser: full_browser_state,
```

- [ ] **Step 4: Route input through screen in `handle_key`**

In `src/app.rs`, modify `handle_key`. At the top, after the `command_error` clearing, add an early return for the full-screen browser:

```rust
if let AppScreen::FileBrowser { .. } = self.screen {
    self.handle_full_browser_key(key, size.width, viewport_height, content_width);
    self.buffers[self.active_buffer].update_total_lines(content_width);
    return Ok(());
}
```

Add a stub method:

```rust
fn handle_full_browser_key(
    &mut self,
    key: KeyEvent,
    _term_width: u16,
    _viewport_height: usize,
    _content_width: usize,
) {
    // Close on q/Esc for now — full implementation in Task 5
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            self.close_full_browser();
        }
        _ => {}
    }
}

fn close_full_browser(&mut self) {
    match self.screen {
        AppScreen::FileBrowser { came_from_dropdown: true } => {
            self.screen = AppScreen::Editor;
            // file_browser stays Some, mode stays FileBrowser for dropdown
            self.mode = AppMode::FileBrowser;
        }
        AppScreen::FileBrowser { came_from_dropdown: false } => {
            self.screen = AppScreen::Editor;
            self.mode = AppMode::Normal;
        }
        _ => {}
    }
}
```

- [ ] **Step 5: Add Tab-expand from dropdown**

In `src/app.rs`, in `handle_file_browser_key`, in the non-filter-mode match block (around line 513), add a `KeyCode::Tab` arm:

```rust
KeyCode::Tab => {
    self.open_file_browser_full(true);
}
```

- [ ] **Step 6: Build and verify**

Run: `cargo build 2>&1`
Expected: Clean build. You can now enter full-screen browser via menu/command/tab and exit with q/Esc.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/view.rs
git commit -m "feat: route draw and input through AppScreen for full-screen browser"
```

---

### Task 4: Implement the full-screen browser renderer

**Files:**
- Modify: `src/view.rs` (replace `draw_full_file_browser` stub)

- [ ] **Step 1: Add helper for human-readable file size**

In `src/view.rs`, add a helper function:

```rust
fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
```

- [ ] **Step 2: Add helper for short date format**

```rust
fn format_mtime(time: std::time::SystemTime) -> String {
    let duration = time.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    // Convert to simple date — use libc-free approach
    // Seconds per day: 86400
    let days = secs / 86400;
    // Approximate month/day from days since epoch (Jan 1 1970)
    // This is a rough approach; for a TUI display it's good enough
    let (year, month, day) = days_to_ymd(days);
    let months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    let mon = months.get(month as usize).unwrap_or(&"???");
    format!("{} {:2}", mon, day)
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m - 1, d) // month 0-indexed for array lookup
}
```

- [ ] **Step 3: Implement `draw_full_file_browser`**

Replace the stub with the full implementation:

```rust
fn draw_full_file_browser(frame: &mut Frame, area: Rect, fb: &FullBrowserViewState, theme: &Theme) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    // Layout: header(1) + entries(fill) + filter(0 or 1) + hints(1)
    let filter_height = if fb.filter_mode { 1u16 } else { 0 };
    let chunks = Layout::vertical([
        Constraint::Length(1),                // header
        Constraint::Min(1),                   // entry list
        Constraint::Length(filter_height),     // filter input
        Constraint::Length(1),                // hint bar
    ])
    .split(area);

    let header_area = chunks[0];
    let list_area = chunks[1];
    let filter_area = chunks[2];
    let hint_area = chunks[3];

    // --- Header: breadcrumb path ---
    let max_dir_width = header_area.width as usize - 3;
    let dir_display = if fb.dir.len() > max_dir_width {
        let start = fb.dir.len() - max_dir_width;
        format!(" \u{25b8} \u{2026}{}", &fb.dir[start..])
    } else {
        format!(" \u{25b8} {}", fb.dir)
    };
    let header_line = Line::from(Span::styled(
        dir_display,
        Style::default().fg(Color::Rgb(139, 233, 253)).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(Color::Rgb(40, 42, 54))),
        header_area,
    );

    // --- Entry list ---
    let visible_rows = list_area.height as usize;
    let selected_idx = fb.entries.iter().position(|e| e.is_selected).unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    // Column widths: 2 marker + name(fill) + 2 pad + 6 size + 2 pad + 7 mtime
    let size_col_width: u16 = 6;
    let mtime_col_width: u16 = 7;
    let padding: u16 = 2;
    let metadata_width = padding + size_col_width + padding + mtime_col_width;

    if fb.entries.is_empty() {
        let empty_line = Line::from(Span::styled(
            "  (empty)",
            Style::default().fg(Color::Rgb(102, 102, 102)),
        ));
        frame.render_widget(
            Paragraph::new(empty_line),
            Rect::new(list_area.x, list_area.y, list_area.width, 1),
        );
    } else {
        let mut y = 0u16;
        for (i, entry) in fb.entries.iter().enumerate() {
            if i < scroll_offset {
                continue;
            }
            if y >= list_area.height {
                break;
            }

            let row_area = Rect::new(list_area.x, list_area.y + y, list_area.width, 1);

            let marker = if entry.is_selected { "\u{25b8} " } else { "  " };
            let bg_style = if entry.is_selected {
                Style::default().bg(Color::Rgb(50, 52, 68))
            } else {
                Style::default().bg(Color::Rgb(30, 30, 48))
            };

            // Fill row background
            let bg_fill = Paragraph::new("").style(bg_style);
            frame.render_widget(bg_fill, row_area);

            let name_style = if entry.is_dir {
                bg_style.fg(Color::Rgb(139, 233, 253))
            } else {
                bg_style.fg(Color::Rgb(204, 204, 204))
            };
            let suffix = if entry.is_dir { "/" } else { "" };

            let name_max = (list_area.width - metadata_width - 2) as usize; // 2 for marker
            let name_text = format!("{}{}", entry.name, suffix);
            let name_display = if name_text.len() > name_max {
                format!("\u{2026}{}", &name_text[name_text.len() - name_max + 1..])
            } else {
                name_text.clone()
            };

            let size_str = match entry.size {
                Some(s) => format_file_size(s),
                None => "\u{2014}".to_string(),
            };
            let mtime_str = match entry.modified {
                Some(t) => format_mtime(t),
                None => "\u{2014}".to_string(),
            };

            let name_padding = name_max.saturating_sub(name_display.len());

            let spans = vec![
                Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
                Span::styled(name_display, name_style),
                Span::styled(" ".repeat(name_padding), bg_style),
                Span::styled("  ", bg_style),
                Span::styled(format!("{:>width$}", size_str, width = size_col_width as usize), bg_style.fg(Color::Rgb(98, 114, 164))),
                Span::styled("  ", bg_style),
                Span::styled(format!("{:>width$}", mtime_str, width = mtime_col_width as usize), bg_style.fg(Color::Rgb(98, 114, 164))),
            ];

            frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
            y += 1;
        }
    }

    // --- Filter input ---
    if fb.filter_mode {
        let filter_line = Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Rgb(255, 184, 108)).bg(Color::Rgb(30, 30, 48))),
            Span::styled(
                &fb.filter_text,
                Style::default().fg(Color::Rgb(241, 250, 140)).bg(Color::Rgb(30, 30, 48)),
            ),
            Span::styled("\u{258e}", Style::default().fg(Color::Rgb(102, 102, 102)).bg(Color::Rgb(30, 30, 48))),
        ]);
        frame.render_widget(Paragraph::new(filter_line), filter_area);
    }

    // --- Hint bar ---
    let mut hints = String::from(" enter:open  o:open+stay  -:parent  .:hidden  /:filter");
    if fb.came_from_dropdown {
        hints.push_str("  tab:collapse");
    }
    hints.push_str("  q:close");
    let hint_line = Line::from(Span::styled(
        hints,
        Style::default().fg(Color::Rgb(98, 114, 164)).bg(Color::Rgb(25, 25, 40)),
    ));
    frame.render_widget(
        Paragraph::new(hint_line).style(Style::default().bg(Color::Rgb(25, 25, 40))),
        hint_area,
    );
}
```

- [ ] **Step 4: Build and verify**

Run: `cargo build 2>&1`
Expected: Clean build.

- [ ] **Step 5: Commit**

```bash
git add src/view.rs
git commit -m "feat: implement full-screen file browser renderer with metadata columns"
```

---

### Task 5: Implement full-screen browser input handling

**Files:**
- Modify: `src/app.rs` (replace `handle_full_browser_key` stub)

- [ ] **Step 1: Implement full input handler**

Replace the `handle_full_browser_key` stub with:

```rust
fn handle_full_browser_key(
    &mut self,
    key: KeyEvent,
    _term_width: u16,
    _viewport_height: usize,
    _content_width: usize,
) {
    let browser = match &mut self.file_browser {
        Some(b) => b,
        None => {
            self.screen = AppScreen::Editor;
            self.mode = AppMode::Normal;
            return;
        }
    };

    if browser.filter_mode {
        match key.code {
            KeyCode::Esc => {
                self.close_full_browser();
                return;
            }
            KeyCode::Enter => {
                let count = browser.visible_entries().len();
                if count == 1 {
                    if let Some(path) = browser.enter_selected() {
                        if self.open_buffer(path) {
                            self.screen = AppScreen::Editor;
                            self.mode = AppMode::Normal;
                        }
                    }
                } else if count > 0 {
                    browser.filter_mode = false;
                }
                return;
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
        KeyCode::Char('j') | KeyCode::Down => browser.move_down(),
        KeyCode::Char('k') | KeyCode::Up => browser.move_up(),
        KeyCode::Char('l') | KeyCode::Enter => {
            if let Some(path) = browser.enter_selected() {
                if self.open_buffer(path) {
                    self.screen = AppScreen::Editor;
                    self.mode = AppMode::Normal;
                }
            }
        }
        KeyCode::Char('o') => {
            // Open file but stay in browser
            if let Some(path) = browser.enter_selected() {
                let _ = self.open_buffer(path);
                // Stay in full browser screen
            }
        }
        KeyCode::Char('h') | KeyCode::Char('-') | KeyCode::Backspace => browser.go_parent(),
        KeyCode::Char('.') => browser.toggle_hidden(),
        KeyCode::Char('/') => browser.filter_mode = true,
        KeyCode::Char('G') => {
            let len = browser.visible_entries().len();
            if len > 0 {
                browser.set_selected(len - 1);
            }
        }
        KeyCode::Tab => {
            if let AppScreen::FileBrowser { came_from_dropdown: true } = self.screen {
                self.close_full_browser();
            }
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            self.close_full_browser();
        }
        _ => {}
    }
}
```

Note: `gg` (jump to first) requires multi-key handling. Since the full-screen browser has its own input handler and doesn't use the `KeybindManager`, handle `g` as a simple prefix:

- [ ] **Step 2: Add `g` prefix support for `gg`**

Add a field to `App`:

```rust
full_browser_pending_g: bool,
```

Initialize to `false` in `App::new`. Then modify `handle_full_browser_key` — in the normal-mode match, replace the `_ => {}` catch-all:

```rust
KeyCode::Char('g') => {
    if self.full_browser_pending_g {
        // gg — jump to first entry
        browser.set_selected(0);
        self.full_browser_pending_g = false;
    } else {
        self.full_browser_pending_g = true;
        return; // wait for next key
    }
}
_ => {
    self.full_browser_pending_g = false;
}
```

And at the top of the normal-mode section, before the match, add:

```rust
// If we had a pending 'g' and got something other than 'g', clear it
// (this is handled in the match _ arm)
```

Actually, this is already handled by the `_ =>` arm clearing the flag. The `return` in the `g` first-press ensures we don't clear it immediately.

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Clean build.

- [ ] **Step 4: Manual test**

Run: `cargo run -- README.md`
- Press `space` then `F` to enter full-screen browser
- `j`/`k` to navigate, `Enter` to open, `-` to go to parent, `.` for hidden files, `/` to filter
- `gg` to jump to top, `G` to jump to bottom
- `o` to open file and stay in browser
- `q` to close

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: implement full-screen file browser input handling"
```

---

### Task 6: Wire up all entry points and close behavior

**Files:**
- Modify: `src/app.rs`

This task ensures all three entry points work and close behavior respects `came_from_dropdown`.

- [ ] **Step 1: Verify menu entry works**

The menu entry `F` → `file-browser-full` was added in Task 2. Verify it dispatches through `execute_action` → `open_file_browser_full(false)`. If `Action::OpenFileBrowserFull` ended up in the no-op arm accidentally, fix it by ensuring it has its own arm *before* the `Action::None | ...` block.

- [ ] **Step 2: Verify command works**

Run the app, press `:`, type `file-browser-full`, press Enter. Should open full-screen browser.

- [ ] **Step 3: Verify Tab expand from dropdown works**

Run the app, press `space f` to open dropdown, then press `Tab`. Should expand to full-screen with `came_from_dropdown: true`.

- [ ] **Step 4: Verify close behavior**

- From Tab-expanded: press `q` → should return to dropdown (verify dropdown is visible, not the editor).
- From menu/command: press `q` → should return to editor in Normal mode.

- [ ] **Step 5: Verify shared state**

- Open dropdown, navigate to a subdirectory, press Tab to expand, verify full-screen shows the same directory.
- In full-screen, navigate to another directory, press `q` to collapse back to dropdown, verify dropdown shows the new directory.

- [ ] **Step 6: Commit (if any fixes were needed)**

```bash
git add src/app.rs
git commit -m "fix: ensure all entry points and close behavior work correctly"
```

---

### Task 7: Update command test coverage

**Files:**
- Modify: `src/command.rs` (tests)

- [ ] **Step 1: Update `test_all_actions_have_commands`**

In `src/command.rs`, find the `test_all_actions_have_commands` test. Add `"file-browser-full"` to the expected list. Also verify `"reload"` and `"outline"` are in the list (they should be from prior work, but confirm).

- [ ] **Step 2: Run the command tests**

Run: `cargo test --lib command`
Expected: All pass.

- [ ] **Step 3: Run full test suite**

Run: `cargo test 2>&1`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/command.rs
git commit -m "test: add file-browser-full to command coverage test"
```
