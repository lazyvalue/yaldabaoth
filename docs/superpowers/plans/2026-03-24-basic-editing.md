# Basic Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add vim-style editing with rope buffer, tree-sitter incremental parsing, Obsidian-style per-block reveal, insert/normal mode, basic motions, undo/redo, and `:w`/`:q` command mode.

**Architecture:** Four new modules (document, cursor, tree, editor) layered beneath the existing app. `Editor` owns `Document` (rope + undo), `CursorPos`, and `TreeState`. App owns Editor and routes keys by mode. View renders active block as raw text, others as styled RenderedBlocks. Cache invalidation uses tree-sitter's `changed_ranges`.

**Tech Stack:** Rust, ropey, tree-sitter 0.24+, tree-sitter-md (MDeiml), ratatui, crossterm, pulldown-cmark, syntect

**Spec:** `docs/superpowers/specs/2026-03-24-basic-editing-design.md`

---

## File Structure

```
src/
├── document.rs      # NEW — Rope wrapper, insert/delete, undo/redo, save
├── cursor.rs        # NEW — CursorPos, movement (hjkl, wb e, 0$), sticky column
├── tree.rs          # NEW — TreeState, tree-sitter parser, block boundaries, incremental parse
├── editor.rs        # NEW — Editor orchestrator, owns Document+Cursor+Tree, edit operations
├── keybind.rs       # MODIFY — add 18 new Action variants, rebind o/q/gx
├── app.rs           # MODIFY — Insert/Command modes, Editor integration, command bar
├── view.rs          # MODIFY — active block raw rendering, cursor display, [+] indicator, command bar
├── viewport.rs      # MODIFY — cursor-following scroll with scrolloff
├── menu.rs          # MODIFY — remove q->Quit from default menu tree
├── lib.rs           # MODIFY — add new module declarations

tests/
├── document_test.rs     # NEW — rope operations, undo/redo, save
├── cursor_test.rs       # NEW — movement, clamping, sticky column, word motions
├── tree_test.rs         # NEW — block boundary detection, incremental re-parse
├── editor_test.rs       # NEW — insert mode, dd, x, undo integration
```

---

### Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add ropey and tree-sitter dependencies**

Add to `[dependencies]` in `Cargo.toml`:

```toml
ropey = "1"
tree-sitter = "0.24"
tree-sitter-md = { git = "https://github.com/MDeiml/tree-sitter-markdown", tag = "v0.3.2" }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat: add ropey, tree-sitter, and tree-sitter-md dependencies"
```

---

### Task 2: Document Module (Rope Buffer + Undo)

**Files:**
- Create: `src/document.rs`
- Create: `tests/document_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write document tests**

Create `tests/document_test.rs`:

```rust
use sketch::document::Document;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_new_document_from_string() {
    let doc = Document::from_text("Hello\nWorld".to_string(), PathBuf::from("test.md"));
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(0), "Hello\n");
    assert_eq!(doc.line_text(1), "World");
    assert!(!doc.is_modified());
}

#[test]
fn test_insert_char() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.insert_char(0, 5, 'X'); // line 0, char 5
    assert_eq!(doc.line_text(0), "HelloX");
    assert!(doc.is_modified());
}

#[test]
fn test_insert_newline() {
    let mut doc = Document::from_text("Hello World".to_string(), PathBuf::from("test.md"));
    doc.insert_char(0, 5, '\n');
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(0), "Hello\n");
    assert_eq!(doc.line_text(1), " World");
}

#[test]
fn test_delete_char() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.delete_char(0, 4); // delete 'o'
    assert_eq!(doc.line_text(0), "Hell");
    assert!(doc.is_modified());
}

#[test]
fn test_delete_line() {
    let mut doc = Document::from_text("Line1\nLine2\nLine3".to_string(), PathBuf::from("test.md"));
    doc.delete_line(1);
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(1), "Line3");
}

#[test]
fn test_undo_insert() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.begin_undo_group(0, 5);
    doc.insert_char(0, 5, 'X');
    let cursor = doc.end_undo_group(0, 6);
    doc.undo();
    assert_eq!(doc.line_text(0), "Hello");
    assert!(!doc.is_modified());
}

#[test]
fn test_undo_redo() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.begin_undo_group(0, 5);
    doc.insert_char(0, 5, 'X');
    doc.end_undo_group(0, 6);
    doc.undo();
    assert_eq!(doc.line_text(0), "Hello");
    doc.redo();
    assert_eq!(doc.line_text(0), "HelloX");
}

#[test]
fn test_save() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.md");
    std::fs::write(&path, "Original").unwrap();
    let mut doc = Document::from_text("Modified".to_string(), path.clone());
    doc.insert_char(0, 8, '!');
    assert!(doc.is_modified());
    doc.save().unwrap();
    assert!(!doc.is_modified());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Modified!");
}

#[test]
fn test_full_text() {
    let doc = Document::from_text("Hello\nWorld".to_string(), PathBuf::from("test.md"));
    assert_eq!(doc.full_text(), "Hello\nWorld");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test document_test`
Expected: FAIL

- [ ] **Step 3: Implement document module**

Create `src/document.rs`:

```rust
use ropey::Rope;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct UndoEntry {
    /// Snapshot of the rope text before this undo group
    before_text: String,
    cursor_before_line: usize,
    cursor_before_col: usize,
    cursor_after_line: usize,
    cursor_after_col: usize,
}

pub struct Document {
    rope: Rope,
    pub file_path: PathBuf,
    modified: bool,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    /// Pending undo group: snapshot taken at begin_undo_group
    pending_undo: Option<UndoEntry>,
}

impl Document {
    pub fn from_text(text: String, file_path: PathBuf) -> Self {
        Self {
            rope: Rope::from_str(&text),
            file_path,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_undo: None,
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    pub fn line_len_chars(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let line_slice = self.rope.line(line);
        let len = line_slice.len_chars();
        // Exclude trailing newline from length for cursor purposes
        if len > 0 && line_slice.char(len - 1) == '\n' {
            len - 1
        } else {
            len
        }
    }

    pub fn full_text(&self) -> String {
        self.rope.to_string()
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Convert (line, char_col) to a byte offset in the rope.
    pub fn line_col_to_byte(&self, line: usize, col: usize) -> usize {
        let line_start = self.rope.line_to_byte(line);
        let line_slice = self.rope.line(line);
        // Convert char offset to byte offset within the line
        let byte_in_line = if col >= line_slice.len_chars() {
            line_slice.len_bytes()
        } else {
            line_slice.char_to_byte(col)
        };
        line_start + byte_in_line
    }

    /// Convert (line, char_col) to a char offset in the rope.
    pub fn line_col_to_char(&self, line: usize, col: usize) -> usize {
        let line_start = self.rope.line_to_char(line);
        line_start + col
    }

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) {
        let char_idx = self.line_col_to_char(line, col);
        self.rope.insert_char(char_idx, ch);
        self.modified = true;
        self.redo_stack.clear();
    }

    pub fn delete_char(&mut self, line: usize, col: usize) {
        let char_idx = self.line_col_to_char(line, col);
        if char_idx < self.rope.len_chars() {
            self.rope.remove(char_idx..char_idx + 1);
            self.modified = true;
            self.redo_stack.clear();
        }
    }

    pub fn delete_line(&mut self, line: usize) {
        if line >= self.rope.len_lines() {
            return;
        }
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        if start < end {
            self.rope.remove(start..end);
        } else if line > 0 {
            // Last line with no trailing newline — remove the newline before it
            let prev_end = self.rope.line_to_char(line);
            if prev_end > 0 {
                self.rope.remove(prev_end - 1..prev_end);
            }
        }
        self.modified = true;
        self.redo_stack.clear();
    }

    /// Begin an undo group. Call before a sequence of edits that should undo as one.
    pub fn begin_undo_group(&mut self, cursor_line: usize, cursor_col: usize) {
        self.pending_undo = Some(UndoEntry {
            before_text: self.rope.to_string(),
            cursor_before_line: cursor_line,
            cursor_before_col: cursor_col,
            cursor_after_line: 0,
            cursor_after_col: 0,
        });
    }

    /// End an undo group. Pushes it to the undo stack.
    pub fn end_undo_group(&mut self, cursor_line: usize, cursor_col: usize) {
        if let Some(mut entry) = self.pending_undo.take() {
            entry.cursor_after_line = cursor_line;
            entry.cursor_after_col = cursor_col;
            self.undo_stack.push(entry);
        }
    }

    /// Undo the last action. Returns the cursor position to restore, if any.
    pub fn undo(&mut self) -> Option<(usize, usize)> {
        let entry = self.undo_stack.pop()?;
        // Save current state for redo
        let redo_entry = UndoEntry {
            before_text: self.rope.to_string(),
            cursor_before_line: entry.cursor_after_line,
            cursor_before_col: entry.cursor_after_col,
            cursor_after_line: entry.cursor_before_line,
            cursor_after_col: entry.cursor_before_col,
        };
        self.redo_stack.push(redo_entry);
        // Restore previous text
        self.rope = Rope::from_str(&entry.before_text);
        self.modified = !self.undo_stack.is_empty();
        Some((entry.cursor_before_line, entry.cursor_before_col))
    }

    /// Redo the last undone action. Returns cursor position to restore.
    pub fn redo(&mut self) -> Option<(usize, usize)> {
        let entry = self.redo_stack.pop()?;
        let undo_entry = UndoEntry {
            before_text: self.rope.to_string(),
            cursor_before_line: entry.cursor_after_line,
            cursor_before_col: entry.cursor_after_col,
            cursor_after_line: entry.cursor_before_line,
            cursor_after_col: entry.cursor_before_col,
        };
        self.undo_stack.push(undo_entry);
        self.rope = Rope::from_str(&entry.before_text);
        self.modified = true;
        Some((entry.cursor_before_line, entry.cursor_before_col))
    }

    /// Save the document to disk atomically.
    pub fn save(&mut self) -> io::Result<()> {
        self.save_to(&self.file_path.clone())
    }

    /// Save to a specific path.
    pub fn save_to(&mut self, path: &Path) -> io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let temp_path = dir.join(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
        fs::write(&temp_path, self.rope.to_string())?;
        fs::rename(&temp_path, path)?;
        self.file_path = path.to_path_buf();
        self.modified = false;
        Ok(())
    }
}
```

- [ ] **Step 4: Add module to lib.rs**

Add `pub mod document;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test document_test`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/document.rs src/lib.rs tests/document_test.rs
git commit -m "feat: document module with rope buffer, undo/redo, and atomic save"
```

---

### Task 3: Cursor Module

**Files:**
- Create: `src/cursor.rs`
- Create: `tests/cursor_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write cursor tests**

Create `tests/cursor_test.rs`:

```rust
use sketch::cursor::CursorPos;
use sketch::document::Document;
use std::path::PathBuf;

fn doc(text: &str) -> Document {
    Document::from_text(text.to_string(), PathBuf::from("test.md"))
}

#[test]
fn test_move_right() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_right(&d, false);
    assert_eq!(c.col, 1);
}

#[test]
fn test_move_right_clamps_normal() {
    let d = doc("Hi");
    let mut c = CursorPos::new();
    c.col = 1; // on 'i', last char
    c.move_right(&d, false); // normal mode: can't go past last char
    assert_eq!(c.col, 1);
}

#[test]
fn test_move_right_insert_mode() {
    let d = doc("Hi");
    let mut c = CursorPos::new();
    c.col = 1;
    c.move_right(&d, true); // insert mode: can go one past
    assert_eq!(c.col, 2);
}

#[test]
fn test_move_left() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.col = 3;
    c.move_left();
    assert_eq!(c.col, 2);
}

#[test]
fn test_move_left_clamps() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_left();
    assert_eq!(c.col, 0);
}

#[test]
fn test_move_down() {
    let d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.move_down(&d, false);
    assert_eq!(c.line, 1);
}

#[test]
fn test_move_down_clamps() {
    let d = doc("Only");
    let mut c = CursorPos::new();
    c.move_down(&d, false);
    assert_eq!(c.line, 0);
}

#[test]
fn test_move_up() {
    let d = doc("Line1\nLine2");
    let mut c = CursorPos::new();
    c.line = 1;
    c.move_up();
    assert_eq!(c.line, 0);
}

#[test]
fn test_sticky_column() {
    let d = doc("LongLine\nHi\nLongLine");
    let mut c = CursorPos::new();
    c.col = 7; // end of "LongLine"
    c.move_down(&d, false); // "Hi" — clamps to col 1
    assert_eq!(c.col, 1);
    c.move_down(&d, false); // "LongLine" — restores to col 7
    assert_eq!(c.col, 7);
}

#[test]
fn test_move_line_start() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.col = 3;
    c.move_line_start();
    assert_eq!(c.col, 0);
}

#[test]
fn test_move_line_end() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_line_end(&d, false);
    assert_eq!(c.col, 4); // on 'o', the last char
}

#[test]
fn test_move_word_forward() {
    let d = doc("hello world");
    let mut c = CursorPos::new();
    c.move_word_forward(&d);
    assert_eq!(c.col, 6); // start of "world"
}

#[test]
fn test_move_word_backward() {
    let d = doc("hello world");
    let mut c = CursorPos::new();
    c.col = 8;
    c.move_word_backward(&d);
    assert_eq!(c.col, 6); // start of "world"
}

#[test]
fn test_jump_top() {
    let d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.line = 2;
    c.col = 3;
    c.jump_top();
    assert_eq!(c.line, 0);
    assert_eq!(c.col, 0);
}

#[test]
fn test_jump_bottom() {
    let d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.jump_bottom(&d);
    assert_eq!(c.line, 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test cursor_test`
Expected: FAIL

- [ ] **Step 3: Implement cursor module**

Create `src/cursor.rs`:

```rust
use crate::document::Document;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPos {
    pub line: usize,
    pub col: usize,
    /// Remembered column for vertical movement (sticky column)
    desired_col: Option<usize>,
}

impl CursorPos {
    pub fn new() -> Self {
        Self { line: 0, col: 0, desired_col: None }
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
        self.desired_col = None;
    }

    pub fn move_right(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        let max_col = if insert_mode { line_len } else { line_len.saturating_sub(1) };
        if self.col < max_col {
            self.col += 1;
        }
        self.desired_col = None;
    }

    pub fn move_up(&mut self) {
        if self.line > 0 {
            if self.desired_col.is_none() {
                self.desired_col = Some(self.col);
            }
            self.line -= 1;
        }
    }

    pub fn move_down(&mut self, doc: &Document, insert_mode: bool) {
        if self.line + 1 < doc.line_count() {
            if self.desired_col.is_none() {
                self.desired_col = Some(self.col);
            }
            self.line += 1;
        }
    }

    /// Clamp column to valid range for current line. Call after vertical movement.
    pub fn clamp_col(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        let max_col = if insert_mode { line_len } else { line_len.saturating_sub(1) };
        let target = self.desired_col.unwrap_or(self.col);
        self.col = target.min(max_col);
    }

    pub fn move_line_start(&mut self) {
        self.col = 0;
        self.desired_col = None;
    }

    pub fn move_line_end(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        self.col = if insert_mode { line_len } else { line_len.saturating_sub(1) };
        self.desired_col = None;
    }

    pub fn move_word_forward(&mut self, doc: &Document) {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut i = self.col;

        // Skip current word
        if i < len && is_word_char(chars[i]) {
            while i < len && is_word_char(chars[i]) { i += 1; }
        } else if i < len && !chars[i].is_whitespace() {
            while i < len && !chars[i].is_whitespace() && !is_word_char(chars[i]) { i += 1; }
        }
        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            if chars[i] == '\n' { break; }
            i += 1;
        }

        if i >= len || chars[i] == '\n' {
            // Move to next line
            if self.line + 1 < doc.line_count() {
                self.line += 1;
                self.col = 0;
                // Skip leading whitespace on next line
                let next_text = doc.line_text(self.line);
                let next_chars: Vec<char> = next_text.chars().collect();
                let mut j = 0;
                while j < next_chars.len() && next_chars[j].is_whitespace() && next_chars[j] != '\n' { j += 1; }
                self.col = j;
            }
        } else {
            self.col = i;
        }
        self.desired_col = None;
    }

    pub fn move_word_backward(&mut self, doc: &Document) {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();

        if self.col == 0 {
            // Move to end of previous line
            if self.line > 0 {
                self.line -= 1;
                let prev_len = doc.line_len_chars(self.line);
                self.col = prev_len.saturating_sub(1);
            }
            self.desired_col = None;
            return;
        }

        let mut i = self.col;
        // Skip whitespace backwards
        while i > 0 && chars[i - 1].is_whitespace() { i -= 1; }
        // Skip word backwards
        if i > 0 && is_word_char(chars[i - 1]) {
            while i > 0 && is_word_char(chars[i - 1]) { i -= 1; }
        } else if i > 0 {
            while i > 0 && !chars[i - 1].is_whitespace() && !is_word_char(chars[i - 1]) { i -= 1; }
        }

        self.col = i;
        self.desired_col = None;
    }

    pub fn move_word_end(&mut self, doc: &Document) {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut i = self.col + 1;

        // Skip whitespace
        while i < len && chars[i].is_whitespace() && chars[i] != '\n' { i += 1; }
        // Move to end of word
        if i < len && is_word_char(chars[i]) {
            while i + 1 < len && is_word_char(chars[i + 1]) { i += 1; }
        } else if i < len && !chars[i].is_whitespace() {
            while i + 1 < len && !chars[i + 1].is_whitespace() && !is_word_char(chars[i + 1]) { i += 1; }
        }

        self.col = i.min(len.saturating_sub(1));
        self.desired_col = None;
    }

    pub fn jump_top(&mut self) {
        self.line = 0;
        self.col = 0;
        self.desired_col = None;
    }

    pub fn jump_bottom(&mut self, doc: &Document) {
        self.line = doc.line_count().saturating_sub(1);
        self.col = 0;
        self.desired_col = None;
    }
}

impl Default for CursorPos {
    fn default() -> Self {
        Self::new()
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
```

- [ ] **Step 4: Add module to lib.rs**

Add `pub mod cursor;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test cursor_test`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/cursor.rs src/lib.rs tests/cursor_test.rs
git commit -m "feat: cursor module with hjkl, word motions, sticky column"
```

---

### Task 4: Tree-sitter Integration Module

**Files:**
- Create: `src/tree.rs`
- Create: `tests/tree_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write tree tests**

Create `tests/tree_test.rs`:

```rust
use sketch::tree::TreeState;

#[test]
fn test_parse_markdown() {
    let md = "# Hello\n\nA paragraph.\n\n```rust\ncode\n```\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    assert!(ts.tree().is_some());
}

#[test]
fn test_block_boundaries() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    let blocks = ts.block_boundaries();
    // Should find: heading, paragraph, thematic_break
    assert!(blocks.len() >= 3);
}

#[test]
fn test_active_block_index() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    // Byte offset 0 should be in the heading block
    let idx = ts.active_block_at_byte(0);
    assert_eq!(idx, Some(0));
    // Byte offset in the paragraph
    let idx = ts.active_block_at_byte(10);
    assert!(idx.is_some());
}

#[test]
fn test_incremental_reparse() {
    let md = "# Hello\n\nWorld\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    let blocks_before = ts.block_boundaries().len();

    // Simulate editing "World" to "World!"
    let new_md = "# Hello\n\nWorld!\n";
    ts.edit(9, 14, 15); // start_byte, old_end_byte, new_end_byte
    ts.parse(new_md.as_bytes());
    assert!(ts.tree().is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tree_test`
Expected: FAIL

- [ ] **Step 3: Implement tree module**

Create `src/tree.rs`:

```rust
use tree_sitter::{InputEdit, Language, Parser, Point, Tree};

/// Block boundary info from tree-sitter.
#[derive(Debug, Clone)]
pub struct BlockInfo {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: String,
}

pub struct TreeState {
    parser: Parser,
    tree: Option<Tree>,
}

impl TreeState {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let language = tree_sitter_md::LANGUAGE;
        parser.set_language(&language.into()).expect("Failed to set tree-sitter markdown language");
        Self { parser, tree: None }
    }

    pub fn parse(&mut self, source: &[u8]) {
        self.tree = self.parser.parse(source, self.tree.as_ref());
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// Notify tree-sitter of an edit before re-parsing.
    pub fn edit(&mut self, start_byte: usize, old_end_byte: usize, new_end_byte: usize) {
        if let Some(tree) = &mut self.tree {
            // Simplified: we don't track exact line/col for the edit points
            // tree-sitter uses these for optimization but works without precise values
            let edit = InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position: Point::new(0, 0),
                old_end_position: Point::new(0, 0),
                new_end_position: Point::new(0, 0),
            };
            tree.edit(&edit);
        }
    }

    /// Get top-level block boundaries from the syntax tree.
    pub fn block_boundaries(&self) -> Vec<BlockInfo> {
        let tree = match &self.tree {
            Some(t) => t,
            None => return Vec::new(),
        };

        let root = tree.root_node();
        let mut blocks = Vec::new();

        // The markdown tree-sitter grammar uses a "document" root node
        // with top-level block children
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            // Skip whitespace-only nodes
            if child.kind() == "\n" || child.is_extra() {
                continue;
            }
            blocks.push(BlockInfo {
                start_byte: child.start_byte(),
                end_byte: child.end_byte(),
                start_line: child.start_position().row,
                end_line: child.end_position().row,
                kind: child.kind().to_string(),
            });
        }

        blocks
    }

    /// Find which block index contains the given byte offset.
    pub fn active_block_at_byte(&self, byte_offset: usize) -> Option<usize> {
        let blocks = self.block_boundaries();
        for (i, block) in blocks.iter().enumerate() {
            if byte_offset >= block.start_byte && byte_offset < block.end_byte {
                return Some(i);
            }
        }
        // If past last block, return last block
        if !blocks.is_empty() {
            Some(blocks.len() - 1)
        } else {
            None
        }
    }

    /// Get changed byte ranges between the old and new tree.
    /// Call after edit() + parse() to find what blocks need re-rendering.
    pub fn changed_ranges(&self, old_tree: &Tree) -> Vec<std::ops::Range<usize>> {
        match &self.tree {
            Some(new_tree) => {
                let ranges = old_tree.changed_ranges(new_tree);
                ranges.map(|r| r.start_byte..r.end_byte).collect()
            }
            None => Vec::new(),
        }
    }
}

impl Default for TreeState {
    fn default() -> Self {
        Self::new()
    }
}
```

Note to implementer: The `tree_sitter_md::LANGUAGE` constant may need adjustment based on the actual crate API. The MDeiml tree-sitter-markdown crate exports the language differently — you may need `tree_sitter_md::language()` or `tree_sitter_md::LANGUAGE.into()`. Check the crate docs and adapt.

- [ ] **Step 4: Add module to lib.rs**

Add `pub mod tree;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test tree_test`
Expected: All PASS (may need to adjust tree-sitter API calls based on crate version)

- [ ] **Step 6: Commit**

```bash
git add src/tree.rs src/lib.rs tests/tree_test.rs
git commit -m "feat: tree-sitter integration for markdown block boundary detection"
```

---

### Task 5: Editor Module (Orchestrator)

**Files:**
- Create: `src/editor.rs`
- Create: `tests/editor_test.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write editor tests**

Create `tests/editor_test.rs`:

```rust
use sketch::editor::Editor;
use std::path::PathBuf;

fn editor(text: &str) -> Editor {
    Editor::new(text.to_string(), PathBuf::from("test.md"))
}

#[test]
fn test_insert_char() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 5;
    ed.begin_insert();
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Hello!");
}

#[test]
fn test_insert_mode_undo() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 5;
    ed.begin_insert();
    ed.insert_char('!');
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Hello!!");
    ed.undo();
    assert_eq!(ed.document().line_text(0), "Hello");
}

#[test]
fn test_delete_char() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 4; // on 'o'
    ed.delete_char_at_cursor();
    assert_eq!(ed.document().line_text(0), "Hell");
}

#[test]
fn test_delete_line() {
    let mut ed = editor("Line1\nLine2\nLine3");
    ed.cursor_mut().line = 1;
    ed.delete_current_line();
    assert_eq!(ed.document().line_count(), 2);
    assert_eq!(ed.document().line_text(1), "Line3");
}

#[test]
fn test_delete_line_undo() {
    let mut ed = editor("Line1\nLine2\nLine3");
    ed.cursor_mut().line = 1;
    ed.delete_current_line();
    ed.undo();
    assert_eq!(ed.document().line_count(), 3);
    assert_eq!(ed.document().line_text(1), "Line2\n");
}

#[test]
fn test_open_line_below() {
    let mut ed = editor("Line1\nLine2");
    ed.open_line_below();
    assert_eq!(ed.document().line_count(), 3);
    assert_eq!(ed.cursor().line, 1);
    assert_eq!(ed.document().line_text(1), "\n");
}

#[test]
fn test_active_block_index() {
    let mut ed = editor("# Heading\n\nParagraph\n");
    ed.cursor_mut().line = 2; // on "Paragraph"
    let idx = ed.active_block_index();
    assert!(idx.is_some());
}

#[test]
fn test_block_text() {
    let mut ed = editor("# Heading\n\nParagraph\n");
    let blocks = ed.block_boundaries();
    assert!(blocks.len() >= 2);
    let first_block_text = ed.block_text(0);
    assert!(first_block_text.contains("Heading"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test editor_test`
Expected: FAIL

- [ ] **Step 3: Implement editor module**

Create `src/editor.rs`:

```rust
use std::path::PathBuf;

use crate::cursor::CursorPos;
use crate::document::Document;
use crate::tree::{BlockInfo, TreeState};

pub struct Editor {
    document: Document,
    cursor: CursorPos,
    tree_state: TreeState,
    in_insert_mode: bool,
}

impl Editor {
    pub fn new(text: String, file_path: PathBuf) -> Self {
        let mut tree_state = TreeState::new();
        tree_state.parse(text.as_bytes());

        let document = Document::from_text(text, file_path);
        Self {
            document,
            cursor: CursorPos::new(),
            tree_state,
            in_insert_mode: false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn cursor(&self) -> &CursorPos {
        &self.cursor
    }

    pub fn cursor_mut(&mut self) -> &mut CursorPos {
        &mut self.cursor
    }

    pub fn tree_state(&self) -> &TreeState {
        &self.tree_state
    }

    pub fn is_insert_mode(&self) -> bool {
        self.in_insert_mode
    }

    /// Begin insert mode — creates an undo boundary.
    pub fn begin_insert(&mut self) {
        self.in_insert_mode = true;
        self.document.begin_undo_group(self.cursor.line, self.cursor.col);
    }

    /// End insert mode — closes the undo boundary.
    pub fn end_insert(&mut self) {
        self.in_insert_mode = false;
        self.document.end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Insert a character at the cursor position (insert mode).
    pub fn insert_char(&mut self, ch: char) {
        self.document.insert_char(self.cursor.line, self.cursor.col, ch);
        if ch == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
        } else {
            self.cursor.col += 1;
        }
        // Don't reparse on every keystroke in insert mode — defer to end_insert
    }

    /// Delete character before cursor (backspace in insert mode).
    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            self.document.delete_char(self.cursor.line, self.cursor.col);
        } else if self.cursor.line > 0 {
            // Join with previous line
            let prev_line_len = self.document.line_len_chars(self.cursor.line - 1);
            self.cursor.line -= 1;
            self.cursor.col = prev_line_len;
            // Delete the newline at end of previous line
            self.document.delete_char(self.cursor.line, self.cursor.col);
        }
    }

    /// Delete character at cursor (normal mode 'x').
    pub fn delete_char_at_cursor(&mut self) {
        self.document.begin_undo_group(self.cursor.line, self.cursor.col);
        self.document.delete_char(self.cursor.line, self.cursor.col);
        // Clamp cursor if line got shorter
        let line_len = self.document.line_len_chars(self.cursor.line);
        if self.cursor.col >= line_len && line_len > 0 {
            self.cursor.col = line_len - 1;
        }
        self.document.end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Delete current line (normal mode 'dd').
    pub fn delete_current_line(&mut self) {
        self.document.begin_undo_group(self.cursor.line, self.cursor.col);
        self.document.delete_line(self.cursor.line);
        // Clamp cursor
        if self.cursor.line >= self.document.line_count() {
            self.cursor.line = self.document.line_count().saturating_sub(1);
        }
        self.cursor.col = 0;
        self.document.end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Open a new line below cursor and enter insert mode.
    pub fn open_line_below(&mut self) {
        let line_end_char = self.document.line_col_to_char(
            self.cursor.line,
            self.document.line_len_chars(self.cursor.line),
        );
        // If the current line has a trailing newline, insert before it
        self.document.begin_undo_group(self.cursor.line, self.cursor.col);
        // Insert newline at end of current line content
        let insert_col = self.document.line_len_chars(self.cursor.line);
        self.document.insert_char(self.cursor.line, insert_col, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
        self.in_insert_mode = true;
        // Don't close undo group yet — will be closed by end_insert
    }

    /// Open a new line above cursor and enter insert mode.
    pub fn open_line_above(&mut self) {
        self.document.begin_undo_group(self.cursor.line, self.cursor.col);
        let insert_char_idx = self.document.line_col_to_char(self.cursor.line, 0);
        // Insert newline at the start of current line, then move cursor up
        self.document.insert_char(self.cursor.line, 0, '\n');
        // cursor.line stays the same (now points to the empty line we created)
        self.cursor.col = 0;
        self.in_insert_mode = true;
    }

    /// Undo last action.
    pub fn undo(&mut self) {
        if let Some((line, col)) = self.document.undo() {
            self.cursor.line = line;
            self.cursor.col = col;
            self.reparse();
        }
    }

    /// Redo last undone action.
    pub fn redo(&mut self) {
        if let Some((line, col)) = self.document.redo() {
            self.cursor.line = line;
            self.cursor.col = col;
            self.reparse();
        }
    }

    /// Get the index of the active block (block containing cursor).
    pub fn active_block_index(&self) -> Option<usize> {
        let byte_offset = self.document.line_col_to_byte(self.cursor.line, self.cursor.col);
        self.tree_state.active_block_at_byte(byte_offset)
    }

    /// Get block boundary info.
    pub fn block_boundaries(&self) -> Vec<BlockInfo> {
        self.tree_state.block_boundaries()
    }

    /// Get the text for a specific block by index.
    pub fn block_text(&self, block_index: usize) -> String {
        let blocks = self.block_boundaries();
        if let Some(block) = blocks.get(block_index) {
            let text = self.document.full_text();
            let start = block.start_byte.min(text.len());
            let end = block.end_byte.min(text.len());
            text[start..end].to_string()
        } else {
            String::new()
        }
    }

    /// Re-parse the document with tree-sitter.
    fn reparse(&mut self) {
        let text = self.document.full_text();
        self.tree_state.parse(text.as_bytes());
    }

    /// Save the document.
    pub fn save(&mut self) -> std::io::Result<()> {
        self.document.save()
    }

    /// Save to a specific path.
    pub fn save_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.document.save_to(path)
    }
}
```

- [ ] **Step 4: Add module to lib.rs**

Add `pub mod editor;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test editor_test`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/editor.rs src/lib.rs tests/editor_test.rs
git commit -m "feat: editor module orchestrating document, cursor, and tree-sitter"
```

---

### Task 6: Update Keybindings

**Files:**
- Modify: `src/keybind.rs`
- Modify: `src/menu.rs`

- [ ] **Step 1: Add new Action variants**

Add to `Action` enum in `src/keybind.rs` (before `None`):

```rust
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordForward,
    MoveWordBackward,
    MoveWordEnd,
    MoveLineStart,
    MoveLineEnd,
    InsertMode,
    InsertAfter,
    OpenLineBelow,
    OpenLineAbove,
    DeleteChar,
    DeleteLine,
    Undo,
    Redo,
    EnterCommand,
```

- [ ] **Step 2: Update default keybindings**

Replace the default bindings. Key changes:
- `j`/`k` → `MoveDown`/`MoveUp` (was `ScrollDown`/`ScrollUp`)
- Add `h` → `MoveLeft`, `l` → `MoveRight`
- Remove `q` → `Quit`
- Change `o` from `OpenLink` to `OpenLineBelow`
- Add `gx` multi-key for `OpenLink`
- Add all new editing bindings

```rust
// Replace existing j/k bindings
single.insert(key('h'), Action::MoveLeft);
single.insert(key('j'), Action::MoveDown);
single.insert(key('k'), Action::MoveUp);
single.insert(key('l'), Action::MoveRight);
// Remove: single.insert(key('q'), Action::Quit);
// Remove: single.insert(key('o'), Action::OpenLink);
single.insert(key('w'), Action::MoveWordForward);
single.insert(key('b'), Action::MoveWordBackward);
single.insert(key('e'), Action::MoveWordEnd);
single.insert(key('0'), Action::MoveLineStart);
single.insert(key('$'), Action::MoveLineEnd);
single.insert(key('i'), Action::InsertMode);
single.insert(key('a'), Action::InsertAfter);
single.insert(key('o'), Action::OpenLineBelow);
single.insert(key('O'), Action::OpenLineAbove);
single.insert(key('x'), Action::DeleteChar);
single.insert(key('u'), Action::Undo);
single.insert(key(':'), Action::EnterCommand);
single.insert(ctrl('r'), Action::Redo);

// Multi-key
multi.insert(vec![key('d'), key('d')], Action::DeleteLine);
multi.insert(vec![key('g'), key('x')], Action::OpenLink);
```

- [ ] **Step 3: Remove q->Quit from default menu tree**

In `src/menu.rs`, remove the `q`/`quit` entry from `default_menu()`.

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: Keybind tests may need updating for changed defaults. Update tests for the new bindings.

- [ ] **Step 5: Commit**

```bash
git add src/keybind.rs src/menu.rs tests/keybind_test.rs
git commit -m "feat: update keybindings for editing mode (hjkl motion, insert, commands)"
```

---

### Task 7: App Integration — Insert, Command Modes, Editor Wiring

**Files:**
- Modify: `src/app.rs`

This is the largest task. It rewires the app to use Editor and adds Insert/Command mode handling.

- [ ] **Step 1: Add Insert and Command to AppMode**

```rust
enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
}
```

- [ ] **Step 2: Replace blocks/filename with Editor**

Replace `App.blocks`, `App.filename` with `App.editor` (`Editor`). Keep `App.viewport`, `App.theme`, `App.keybinds`. Add `command_buffer: String` for command mode.

Update `App::new` to create an `Editor` instead of calling `render::render()` directly.

- [ ] **Step 3: Update the render pipeline**

The draw call now needs to:
1. Get block boundaries from `editor.block_boundaries()`
2. Get the active block index from `editor.active_block_index()`
3. For each block: if it's the active block, render as raw text from `editor.block_text(i)`; otherwise render via pulldown-cmark as `RenderedBlock`

Create a helper method `render_blocks_for_view()` that produces a mixed list of rendered and raw blocks for the view.

- [ ] **Step 4: Handle Insert mode keys**

When `mode == AppMode::Insert`:
- Printable chars → `editor.insert_char(c)`
- Enter → `editor.insert_char('\n')`
- Backspace → `editor.backspace()`
- Esc → `editor.end_insert()`, switch to Normal mode
- Arrow keys → cursor movement

- [ ] **Step 5: Handle Command mode**

When `mode == AppMode::Command`:
- Characters append to `command_buffer`
- Enter executes the command
- Esc cancels, returns to Normal
- Backspace edits the buffer

Command execution:
```rust
fn execute_command(&mut self, cmd: &str) -> bool {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    match parts[0] {
        "w" => {
            if parts.len() > 1 {
                let path = std::path::Path::new(parts[1]);
                self.editor.save_to(path).is_ok()
            } else {
                self.editor.save().is_ok()
            }
        }
        "q" => {
            if self.editor.document().is_modified() {
                // Set error message: "No write since last change (add ! to override)"
                false
            } else {
                self.should_quit = true;
                true
            }
        }
        "q!" => {
            self.should_quit = true;
            true
        }
        "wq" => {
            if self.editor.save().is_ok() {
                self.should_quit = true;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}
```

- [ ] **Step 6: Update Normal mode to use cursor motions**

Replace `ScrollDown`/`ScrollUp` with `MoveDown`/`MoveUp` that move the cursor via `editor.cursor_mut()`. The viewport follows the cursor.

Add the `ensure_cursor_visible` method:
```rust
fn ensure_cursor_visible(&mut self, viewport_height: usize) {
    let scrolloff = 3usize;
    let cursor_line = self.editor.cursor().line;
    // Scroll up if cursor is above viewport
    if cursor_line < self.viewport.scroll_offset + scrolloff {
        self.viewport.scroll_offset = cursor_line.saturating_sub(scrolloff);
    }
    // Scroll down if cursor is below viewport
    if cursor_line >= self.viewport.scroll_offset + viewport_height - scrolloff {
        self.viewport.scroll_offset = cursor_line + scrolloff + 1 - viewport_height;
    }
}
```

- [ ] **Step 7: Update file browser integration**

`load_file` now creates a new `Editor` instead of calling `render::render()`.

`open_file_browser` checks `editor.document().is_modified()` and warns if unsaved.

- [ ] **Step 8: Verify it compiles**

Run: `cargo build`

- [ ] **Step 9: Commit**

```bash
git add src/app.rs
git commit -m "feat: integrate editor with insert/command modes and cursor-following scroll"
```

---

### Task 8: View Layer — Active Block Reveal, Cursor, Command Bar

**Files:**
- Modify: `src/view.rs`

- [ ] **Step 1: Update ViewState**

Add/change fields:

```rust
pub struct ViewState<'a> {
    pub filename: &'a str,
    pub modified: bool,                         // NEW
    pub rendered_blocks: &'a [ViewBlock],        // CHANGED — mixed rendered/raw blocks
    pub viewport: &'a Viewport,
    pub theme: &'a Theme,
    pub mode_label: &'a str,
    pub cursor_line: usize,                      // NEW — line in document
    pub cursor_col: usize,                       // NEW — column in document
    pub show_block_cursor: bool,                 // NEW — block vs beam cursor
    // ... existing search, menu, file_browser fields ...
    pub command_mode: bool,                      // NEW
    pub command_buffer: &'a str,                 // NEW
    pub command_error: &'a str,                  // NEW
}
```

Add a `ViewBlock` enum that the app constructs:
```rust
pub enum ViewBlock {
    Rendered(RenderedBlock),
    Raw { lines: Vec<String>, start_line: usize }, // raw markdown lines + their document line offset
}
```

- [ ] **Step 2: Update top bar for modified indicator**

In `draw_top_bar`, if `state.modified`, append `[+]` to the filename.

- [ ] **Step 3: Render raw blocks (active block reveal)**

In `render_block_to_lines`, handle `ViewBlock::Raw` by producing `StyledLine`s with the raw text in a monochrome style with a subtle left-border tint.

- [ ] **Step 4: Render cursor**

In `draw_content`, when rendering the line that contains the cursor:
- Calculate the cursor's screen position (x, y)
- If `show_block_cursor` (normal mode): highlight the character under the cursor
- If beam cursor (insert mode): show a thin bar between characters

Use ratatui's `Paragraph` with cursor positioning.

- [ ] **Step 5: Render command bar**

In `draw_bottom_bar`, when `state.command_mode`, show `:buffer` input. When `state.command_error` is non-empty, show the error message.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build`

- [ ] **Step 7: Commit**

```bash
git add src/view.rs
git commit -m "feat: active block reveal, cursor rendering, modified indicator, command bar"
```

---

### Task 9: Viewport Cursor-Following Scroll

**Files:**
- Modify: `src/viewport.rs`

- [ ] **Step 1: Add scrolloff constant and cursor-following method**

The viewport already has the core scrolling methods. Add:

```rust
pub const SCROLLOFF: usize = 3;

/// Ensure the cursor line is visible with scrolloff margin.
pub fn ensure_cursor_visible(&mut self, cursor_line: usize, viewport_height: usize) {
    if viewport_height == 0 { return; }
    let top = self.scroll_offset;
    let bottom = self.scroll_offset + viewport_height;

    if cursor_line < top + SCROLLOFF {
        self.scroll_offset = cursor_line.saturating_sub(SCROLLOFF);
    } else if cursor_line >= bottom.saturating_sub(SCROLLOFF) {
        self.scroll_offset = cursor_line + SCROLLOFF + 1 - viewport_height;
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`

- [ ] **Step 3: Commit**

```bash
git add src/viewport.rs
git commit -m "feat: viewport cursor-following scroll with scrolloff margin"
```

---

### Task 10: Wire Everything Together and Test

**Files:**
- Modify: `src/app.rs` (final wiring)
- Modify: various files for bug fixes

- [ ] **Step 1: Update the draw call in app.rs**

The `terminal.draw` closure needs to build `ViewBlock` list from the editor's block boundaries and active block index, construct the full `ViewState`, and call `view::draw`.

- [ ] **Step 2: Manual end-to-end test**

Run: `cargo run -- tests/fixtures/showcase.md`

Verify:
- [ ] h/j/k/l move cursor through document
- [ ] Cursor block shows raw markdown, others show rendered
- [ ] `i` enters insert mode, typing inserts characters
- [ ] Esc returns to normal mode
- [ ] `o` opens line below
- [ ] `x` deletes character
- [ ] `dd` deletes line
- [ ] `u` undoes, `Ctrl+r` redoes
- [ ] `:w` saves file
- [ ] `:q` warns if modified
- [ ] `:q!` force quits
- [ ] `:wq` saves and quits
- [ ] `[+]` shows in title bar when modified
- [ ] Space opens menu, `f` opens file browser
- [ ] w/b/e word motions work
- [ ] 0/$ line start/end work

- [ ] **Step 3: Fix any bugs found**

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat: wire editing into view layer with active block reveal"
```

---

### Task 11: Final Cleanup

**Files:**
- Various

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -- -W clippy::all`
Fix warnings.

- [ ] **Step 2: Run fmt**

Run: `cargo fmt`

- [ ] **Step 3: Update snapshots**

Run: `cargo insta test --accept`

- [ ] **Step 4: Run all tests**

Run: `cargo test`

- [ ] **Step 5: Build release**

Run: `cargo build --release`

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "chore: clippy fixes, formatting, snapshot updates for editing feature"
```
