# Text Object Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add semantic cursor navigation in Rendered mode — move between links, headings, list items, and code blocks with j/k, with context-dependent Enter actions.

**Architecture:** A `NavMode` enum and `NavObject` list stored per-buffer. Objects are discovered by scanning rendered blocks with `view::render_block_to_lines` (same function the view uses). The view draws a span-wide highlight for the selected object. Three entry methods: cycle key `m`, direct keys `gl`/`gh`/`gi`/`gc`, and commands.

**Tech Stack:** Rust, ratatui, crossterm

---

### Task 1: Add NavMode, NavObject, and Object Discovery to Buffer

**Files:**
- Modify: `src/buffer.rs`

- [ ] **Step 1: Add NavMode enum and NavObject struct**

Add to `src/buffer.rs`:

```rust
use crate::view;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    Character,
    Link,
    Heading,
    ListItem,
    CodeBlock,
}

impl NavMode {
    pub fn next(self) -> Self {
        match self {
            NavMode::Character => NavMode::Link,
            NavMode::Link => NavMode::Heading,
            NavMode::Heading => NavMode::ListItem,
            NavMode::ListItem => NavMode::CodeBlock,
            NavMode::CodeBlock => NavMode::Character,
        }
    }

    pub fn label(&self) -> Option<&'static str> {
        match self {
            NavMode::Character => None,
            NavMode::Link => Some("LINKS"),
            NavMode::Heading => Some("HEADINGS"),
            NavMode::ListItem => Some("LIST ITEMS"),
            NavMode::CodeBlock => Some("CODE BLOCKS"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NavObject {
    pub rendered_row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub kind: NavMode,
    pub action_data: String,
}
```

- [ ] **Step 2: Add nav fields to Buffer struct**

Add to the `Buffer` struct:

```rust
    pub nav_mode: NavMode,
    pub nav_objects: Vec<NavObject>,
    pub nav_object_index: usize,
```

Initialize in `Buffer::new`:

```rust
            nav_mode: NavMode::Character,
            nav_objects: Vec::new(),
            nav_object_index: 0,
```

- [ ] **Step 3: Implement `rebuild_nav_objects`**

Add method to `impl Buffer`:

```rust
    /// Rebuild the list of navigable objects by scanning rendered blocks.
    /// Uses view::render_block_to_lines for correct view-space coordinates.
    pub fn rebuild_nav_objects(&mut self, theme: &Theme) {
        self.nav_objects.clear();
        let content_width = self.viewport.content_width(200);
        let mut rendered_row = 0;

        for block in &self.rendered_cache {
            let lines = view::render_block_to_lines(block, content_width, theme);

            match block {
                RenderedBlock::Heading { .. } => {
                    // First line of the rendered heading is the title
                    if let Some(line) = lines.first() {
                        let text = line.text_content();
                        let char_len = text.chars().count();
                        if char_len > 0 {
                            self.nav_objects.push(NavObject {
                                rendered_row,
                                col_start: 0,
                                col_end: char_len,
                                kind: NavMode::Heading,
                                action_data: String::new(),
                            });
                        }
                    }
                }
                RenderedBlock::CodeBlock { lines: code_lines, .. } => {
                    // Collect full code text for yank action
                    let code_text: String = code_lines
                        .iter()
                        .map(|l| l.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Some(line) = lines.first() {
                        let text = line.text_content();
                        let char_len = text.chars().count();
                        self.nav_objects.push(NavObject {
                            rendered_row,
                            col_start: 0,
                            col_end: char_len.max(1),
                            kind: NavMode::CodeBlock,
                            action_data: code_text,
                        });
                    }
                }
                RenderedBlock::List { .. } => {
                    // Each rendered line that starts with a list marker is a list item
                    for (line_idx, line) in lines.iter().enumerate() {
                        let text = line.text_content();
                        let char_len = text.chars().count();
                        // List items have markers like "- ", "1. ", "- [x] "
                        // Detect by checking if any span uses the list_marker style
                        let has_marker = line.spans.first().map_or(false, |s| {
                            let t = s.text.trim();
                            t.ends_with('.') || t == "-" || t == "*" || t == "+"
                                || t.starts_with('-') || t.starts_with('*')
                                || t.contains('[')
                        });
                        if has_marker && char_len > 0 {
                            self.nav_objects.push(NavObject {
                                rendered_row: rendered_row + line_idx,
                                col_start: 0,
                                col_end: char_len,
                                kind: NavMode::ListItem,
                                action_data: String::new(),
                            });
                        }
                    }
                }
                _ => {}
            }

            // Scan all lines for links (links can appear in any block type)
            for (line_idx, line) in lines.iter().enumerate() {
                let mut col = 0;
                for span in &line.spans {
                    let span_chars = span.text.chars().count();
                    if let Some(ref url) = span.link {
                        if span_chars > 0 {
                            self.nav_objects.push(NavObject {
                                rendered_row: rendered_row + line_idx,
                                col_start: col,
                                col_end: col + span_chars,
                                kind: NavMode::Link,
                                action_data: url.clone(),
                            });
                        }
                    }
                    col += span_chars;
                }
            }

            rendered_row += lines.len();
        }

        // Sort by rendered_row then col_start for consistent ordering
        self.nav_objects.sort_by(|a, b| {
            a.rendered_row.cmp(&b.rendered_row)
                .then(a.col_start.cmp(&b.col_start))
        });
    }

    /// Get objects filtered by the current nav mode.
    pub fn objects_for_current_mode(&self) -> Vec<(usize, &NavObject)> {
        self.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == self.nav_mode)
            .collect()
    }

    /// Find the nearest object of the current mode to a given rendered row.
    pub fn nearest_object_index(&self, rendered_row: usize) -> Option<usize> {
        let filtered: Vec<(usize, &NavObject)> = self.objects_for_current_mode();
        if filtered.is_empty() {
            return None;
        }
        // Find closest by rendered_row distance
        let (best_filtered_idx, _) = filtered
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, o))| {
                (o.rendered_row as isize - rendered_row as isize).unsigned_abs()
            })?;
        // Return the index into the full nav_objects vec
        Some(filtered[best_filtered_idx].0)
    }
```

- [ ] **Step 4: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles (nav fields unused for now)

- [ ] **Step 5: Commit**

```bash
git add src/buffer.rs
git commit -m "feat: add NavMode, NavObject, and object discovery to Buffer"
```

---

### Task 2: Add Actions, Keybindings, Commands, and Menu

**Files:**
- Modify: `src/keybind.rs`
- Modify: `src/command.rs`
- Modify: `src/menu.rs`

- [ ] **Step 1: Add Action variants in `src/keybind.rs`**

Add before `None`:

```rust
    NavCycle,
    NavCharacter,
    NavLinks,
    NavHeadings,
    NavListItems,
    NavCodeBlocks,
```

- [ ] **Step 2: Add default keybindings**

In the `Default` impl for `KeybindManager`, add:

```rust
        single.insert(key('m'), "nav-cycle".into());
```

And add multi-key sequences:

```rust
        multi.insert(vec![key('g'), key('l')], "nav-links".into());
        multi.insert(vec![key('g'), key('h')], "nav-headings".into());
        multi.insert(vec![key('g'), key('i')], "nav-list-items".into());
        multi.insert(vec![key('g'), key('c')], "nav-code-blocks".into());
```

- [ ] **Step 3: Register commands in `src/command.rs`**

Add to `default_registry()`:

```rust
            CommandDef {
                name: "nav-cycle".into(),
                aliases: vec![],
                action: Action::NavCycle,
                description: "Cycle navigation mode".into(),
            },
            CommandDef {
                name: "nav-character".into(),
                aliases: vec![],
                action: Action::NavCharacter,
                description: "Character navigation mode".into(),
            },
            CommandDef {
                name: "nav-links".into(),
                aliases: vec![],
                action: Action::NavLinks,
                description: "Link navigation mode".into(),
            },
            CommandDef {
                name: "nav-headings".into(),
                aliases: vec![],
                action: Action::NavHeadings,
                description: "Heading navigation mode".into(),
            },
            CommandDef {
                name: "nav-list-items".into(),
                aliases: vec![],
                action: Action::NavListItems,
                description: "List item navigation mode".into(),
            },
            CommandDef {
                name: "nav-code-blocks".into(),
                aliases: vec![],
                action: Action::NavCodeBlocks,
                description: "Code block navigation mode".into(),
            },
```

- [ ] **Step 4: Add navigate submenu in `src/menu.rs`**

In `default_menu()`, add before the "goto" submenu:

```rust
        MenuNode::submenu("n", "navigate", vec![
            MenuNode::entry("l", "links", "nav-links"),
            MenuNode::entry("h", "headings", "nav-headings"),
            MenuNode::entry("i", "list items", "nav-list-items"),
            MenuNode::entry("c", "code blocks", "nav-code-blocks"),
            MenuNode::entry("m", "cycle mode", "nav-cycle"),
        ]),
```

- [ ] **Step 5: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles, tests pass

- [ ] **Step 6: Commit**

```bash
git add src/keybind.rs src/command.rs src/menu.rs
git commit -m "feat: add nav mode actions, keybindings, commands, and menu"
```

---

### Task 3: Handle Nav Actions in App

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add `use sketch::buffer::NavMode;` import**

Add to the imports at the top of `src/app.rs`:

```rust
use sketch::buffer::NavMode;
```

- [ ] **Step 2: Add helper method to enter a nav mode**

Add to `impl App`:

```rust
    fn enter_nav_mode(&mut self, mode: NavMode) {
        let buf = &mut self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return; // Nav modes only work in Rendered mode
        }
        buf.nav_mode = mode;
        if mode == NavMode::Character {
            return;
        }
        // Rebuild objects and jump to nearest
        buf.rebuild_nav_objects(&self.theme);
        let current_row = buf.rendered_cursor_row;
        if let Some(idx) = buf.nearest_object_index(current_row) {
            buf.nav_object_index = idx;
            let obj = &buf.nav_objects[idx];
            buf.rendered_cursor_row = obj.rendered_row;
            buf.rendered_cursor_col = obj.col_start;
        }
    }
```

- [ ] **Step 3: Handle nav actions in `execute_action`**

Add to the `execute_action` match, before the `Action::None` arm:

```rust
            Action::NavCycle => {
                let current = self.buffers[self.active_buffer].nav_mode;
                let next = current.next();
                self.enter_nav_mode(next);
                // Ensure cursor is visible after mode change
                let viewport_height = _content_width; // need actual viewport height
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCharacter => {
                self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            }
            Action::NavLinks => {
                self.enter_nav_mode(NavMode::Link);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavHeadings => {
                self.enter_nav_mode(NavMode::Heading);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavListItems => {
                self.enter_nav_mode(NavMode::ListItem);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCodeBlocks => {
                self.enter_nav_mode(NavMode::CodeBlock);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
```

Note: `execute_action` already has `viewport_height` as a parameter. The `NavCycle` arm above has a bug — it uses `_content_width` instead. Fix it to use `viewport_height` (check the actual parameter name in the method signature — it's `viewport_height: usize`).

Corrected NavCycle:
```rust
            Action::NavCycle => {
                let current = self.buffers[self.active_buffer].nav_mode;
                let next = current.next();
                self.enter_nav_mode(next);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
```

- [ ] **Step 4: Override MoveDown/MoveUp for object modes**

In the `Action::MoveDown` arm, update the Rendered mode branch:

```rust
            Action::MoveDown => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_next();
                    } else {
                        let total = self.buffers[self.active_buffer].viewport.total_lines;
                        if self.buffers[self.active_buffer].rendered_cursor_row + 1 < total {
                            self.buffers[self.active_buffer].rendered_cursor_row += 1;
                        }
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.move_down(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
```

And `Action::MoveUp`:

```rust
            Action::MoveUp => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_prev();
                    } else {
                        self.buffers[self.active_buffer].rendered_cursor_row =
                            self.buffers[self.active_buffer].rendered_cursor_row.saturating_sub(1);
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.cursor_mut().move_up();
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
```

- [ ] **Step 5: Override MoveLeft/MoveRight for lateral object movement**

In the Rendered mode branch of `MoveLeft | MoveRight | ...`, add nav mode handling:

```rust
            Action::MoveLeft | Action::MoveRight
            | Action::MoveWordForward | Action::MoveWordBackward | Action::MoveWordEnd
            | Action::MoveLineStart | Action::MoveLineEnd => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let nav = self.buffers[self.active_buffer].nav_mode;
                    if nav == NavMode::Link || nav == NavMode::ListItem {
                        match action {
                            Action::MoveLeft => self.nav_move_prev(),
                            Action::MoveRight => self.nav_move_next(),
                            _ => {}
                        }
                        self.ensure_rendered_cursor_visible(viewport_height);
                    } else if nav == NavMode::Character {
                        match action {
                            Action::MoveLeft => {
                                self.buffers[self.active_buffer].rendered_cursor_col =
                                    self.buffers[self.active_buffer].rendered_cursor_col.saturating_sub(1);
                            }
                            Action::MoveRight => {
                                self.buffers[self.active_buffer].rendered_cursor_col += 1;
                            }
                            Action::MoveLineStart => {
                                self.buffers[self.active_buffer].rendered_cursor_col = 0;
                            }
                            _ => {}
                        }
                    }
                    // Heading and CodeBlock modes: h/l are no-ops
                } else {
                    // ... raw mode handling unchanged ...
```

- [ ] **Step 6: Add nav_move_next and nav_move_prev helpers**

```rust
    fn nav_move_next(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        // Find current position in filtered list
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let next_pos = (pos + 1) % filtered.len();
        let next_idx = filtered[next_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = next_idx;
        let obj = &buf.nav_objects[next_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    fn nav_move_prev(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let prev_pos = if pos == 0 { filtered.len() - 1 } else { pos - 1 };
        let prev_idx = filtered[prev_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = prev_idx;
        let obj = &buf.nav_objects[prev_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }
```

- [ ] **Step 7: Handle Enter for context-dependent actions**

Find where `KeyCode::Enter` is processed in normal mode. Currently in `handle_normal_key`, Enter is only handled during search input mode. Add a check after the keybind processing, or handle it as a new action. Simplest approach: add a `NavActivate` action... Actually, let's keep it simple. In `handle_normal_key`, after the search check and before the keybind dispatch, add Enter handling for nav modes:

Actually, Enter in normal mode goes through the keybind system. It's not currently bound to anything in normal mode. Let's add a binding and action:

Add `NavActivate` to the Action enum in `keybind.rs`:
```rust
    NavActivate,
```

Add command in `command.rs`:
```rust
            CommandDef {
                name: "nav-activate".into(),
                aliases: vec![],
                action: Action::NavActivate,
                description: "Activate selected nav object".into(),
            },
```

Add keybinding — bind Enter in normal mode:
```rust
        single.insert(
            KeyPress::new(KeyCode::Enter, KeyModifiers::NONE),
            "nav-activate".into(),
        );
```

Handle in `execute_action`:
```rust
            Action::NavActivate => {
                let buf = &self.buffers[self.active_buffer];
                if buf.view_mode == ViewMode::Rendered && buf.nav_mode != NavMode::Character {
                    let obj = buf.nav_objects.get(buf.nav_object_index).cloned();
                    if let Some(obj) = obj {
                        match obj.kind {
                            NavMode::Link => {
                                self.open_link(&obj.action_data);
                            }
                            NavMode::Heading => {
                                // Scroll past heading, return to character mode
                                let buf = &mut self.buffers[self.active_buffer];
                                buf.rendered_cursor_row = obj.rendered_row;
                                buf.nav_mode = NavMode::Character;
                            }
                            NavMode::CodeBlock => {
                                // Yank code block to clipboard
                                use std::io::Write;
                                use std::process::{Command, Stdio};
                                if let Ok(mut child) = Command::new("pbcopy")
                                    .stdin(Stdio::piped())
                                    .spawn()
                                {
                                    if let Some(mut stdin) = child.stdin.take() {
                                        let _ = stdin.write_all(obj.action_data.as_bytes());
                                    }
                                }
                            }
                            NavMode::ListItem => {
                                // TODO: toggle checkbox — for now, no-op
                            }
                            NavMode::Character => {}
                        }
                    }
                }
            }
```

- [ ] **Step 8: Handle Esc to exit nav mode**

In `handle_normal_key`, the Esc key goes through the keybind system. It's not currently bound. We need Esc to exit nav mode. Add to `handle_normal_key`, before the keybind dispatch:

```rust
        // Check Esc for nav mode exit
        if key.code == KeyCode::Esc
            && self.buffers[self.active_buffer].nav_mode != NavMode::Character
            && self.buffers[self.active_buffer].view_mode == ViewMode::Rendered
        {
            self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            return;
        }
```

Add this right after the search_input_mode check block and before the keybind dispatch.

- [ ] **Step 9: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles, tests pass

- [ ] **Step 10: Commit**

```bash
git add src/app.rs src/keybind.rs src/command.rs
git commit -m "feat: handle nav mode actions — move, activate, exit"
```

---

### Task 4: View — Object Highlight and Top Bar Indicator

**Files:**
- Modify: `src/view.rs`
- Modify: `src/app.rs` (ViewState population)

- [ ] **Step 1: Add nav fields to ViewState**

In `src/view.rs`, add to `ViewState`:

```rust
    pub nav_mode_label: Option<String>,
    pub nav_highlight: Option<(usize, usize, usize)>, // (rendered_row, col_start, col_end)
```

- [ ] **Step 2: Populate fields in app.rs draw closure**

In the ViewState construction in `App::run`, add:

```rust
                    nav_mode_label: self.buffers[self.active_buffer].nav_mode.label()
                        .map(|s| s.to_string()),
                    nav_highlight: {
                        let buf = &self.buffers[self.active_buffer];
                        if buf.nav_mode != NavMode::Character {
                            buf.nav_objects.get(buf.nav_object_index).map(|obj| {
                                (obj.rendered_row, obj.col_start, obj.col_end)
                            })
                        } else {
                            None
                        }
                    },
```

- [ ] **Step 3: Show nav mode in top bar**

In `draw_top_bar`, after the `buffer_info` line, add:

```rust
    let nav_info = state.nav_mode_label.as_deref().unwrap_or("");
    let nav_display = if !nav_info.is_empty() {
        format!(" [{}]", nav_info)
    } else {
        String::new()
    };
```

Append to the position string:
```rust
    let position = format!("line {}/{} {}%{}{}", current_line, total, percent, buffer_info, nav_display);
```

- [ ] **Step 4: Draw object highlight in `draw_content_rendered`**

Replace the existing single-character rendered cursor block with one that handles both character mode (single char) and object mode (full span):

In `draw_content_rendered`, where the cursor is currently drawn (the `if state.view_mode == ViewMode::Rendered && state.show_block_cursor && render_y == state.rendered_cursor_row` block), replace it with:

```rust
                    // Draw rendered-mode cursor / nav object highlight
                    if state.view_mode == ViewMode::Rendered && state.show_block_cursor {
                        // Object mode highlight (full span)
                        if let Some((obj_row, obj_col_start, obj_col_end)) = state.nav_highlight {
                            if render_y == obj_row {
                                let line_text = line.text_content();
                                let line_chars: Vec<char> = line_text.chars().collect();
                                let start = obj_col_start.min(line_chars.len());
                                let end = obj_col_end.min(line_chars.len());
                                if start < end {
                                    let highlight_text: String = line_chars[start..end].iter().collect();
                                    let highlight_x = area.x + x_offset as u16 + start as u16;
                                    let w = (end - start) as u16;
                                    if highlight_x < area.x + area.width {
                                        let clamped_w = w.min(area.x + area.width - highlight_x);
                                        let highlight_style = Style::default()
                                            .fg(Color::Rgb(40, 42, 54))
                                            .bg(Color::Rgb(248, 248, 242));
                                        let highlight_area = Rect::new(
                                            highlight_x,
                                            area.y + screen_y as u16,
                                            clamped_w,
                                            1,
                                        );
                                        frame.render_widget(
                                            Paragraph::new(Span::styled(
                                                highlight_text[..clamped_w as usize].to_string(),
                                                highlight_style,
                                            )),
                                            highlight_area,
                                        );
                                    }
                                }
                            }
                        }
                        // Character mode cursor (single char) — only when no nav highlight on this line
                        else if render_y == state.rendered_cursor_row {
                            let line_text = line.text_content();
                            let line_chars: Vec<char> = line_text.chars().collect();
                            let col = state.rendered_cursor_col.min(
                                line_chars.len().saturating_sub(1),
                            );
                            let cursor_char = line_chars.get(col).copied().unwrap_or(' ');
                            let cursor_x = area.x + x_offset as u16 + col as u16;
                            if cursor_x < area.x + area.width {
                                let mut span_col = 0;
                                let mut on_link = false;
                                for span in &line.spans {
                                    let span_len = span.text.chars().count();
                                    if col >= span_col && col < span_col + span_len {
                                        on_link = span.link.is_some();
                                        break;
                                    }
                                    span_col += span_len;
                                }
                                let cursor_style = if on_link {
                                    Style::default()
                                        .fg(Color::Rgb(40, 42, 54))
                                        .bg(Color::Rgb(139, 233, 253))
                                        .add_modifier(Modifier::UNDERLINED)
                                } else {
                                    Style::default()
                                        .fg(Color::Rgb(40, 42, 54))
                                        .bg(Color::Rgb(248, 248, 242))
                                };
                                let cursor_area = Rect::new(
                                    cursor_x,
                                    area.y + screen_y as u16,
                                    1,
                                    1,
                                );
                                frame.render_widget(
                                    Paragraph::new(Span::styled(
                                        cursor_char.to_string(),
                                        cursor_style,
                                    )),
                                    cursor_area,
                                );
                            }
                        }
                    }
```

- [ ] **Step 5: Build and test**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5`
Expected: compiles, tests pass

- [ ] **Step 6: Commit**

```bash
git add src/view.rs src/app.rs
git commit -m "feat: nav object highlight and top bar mode indicator"
```

---

### Task 5: Final Verification and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy 2>&1`
Expected: no errors

- [ ] **Step 3: Fix any issues**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: clippy fixes and cleanup"
```
