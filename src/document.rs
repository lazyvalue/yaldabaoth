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
        let temp_path = dir.join(format!(
            ".{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::write(&temp_path, self.rope.to_string())?;
        fs::rename(&temp_path, path)?;
        self.file_path = path.to_path_buf();
        self.modified = false;
        Ok(())
    }
}
