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
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
    }

    /// End insert mode — closes the undo boundary.
    pub fn end_insert(&mut self) {
        self.in_insert_mode = false;
        self.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Insert a character at the cursor position (insert mode).
    pub fn insert_char(&mut self, ch: char) {
        self.document
            .insert_char(self.cursor.line, self.cursor.col, ch);
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
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.document.delete_char(self.cursor.line, self.cursor.col);
        // Clamp cursor if line got shorter
        let line_len = self.document.line_len_chars(self.cursor.line);
        if self.cursor.col >= line_len && line_len > 0 {
            self.cursor.col = line_len - 1;
        }
        self.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Delete current line (normal mode 'dd').
    pub fn delete_current_line(&mut self) {
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.document.delete_line(self.cursor.line);
        // Clamp cursor
        if self.cursor.line >= self.document.line_count() {
            self.cursor.line = self.document.line_count().saturating_sub(1);
        }
        self.cursor.col = 0;
        self.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
    }

    /// Open a new line below cursor and enter insert mode.
    pub fn open_line_below(&mut self) {
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        // Insert newline at end of current line content
        let insert_col = self.document.line_len_chars(self.cursor.line);
        self.document
            .insert_char(self.cursor.line, insert_col, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
        self.in_insert_mode = true;
        // Don't close undo group yet — will be closed by end_insert
    }

    /// Open a new line above cursor and enter insert mode.
    pub fn open_line_above(&mut self) {
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
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
        let byte_offset = self
            .document
            .line_col_to_byte(self.cursor.line, self.cursor.col);
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

    // --- Combined cursor+document operations to avoid borrow conflicts ---

    /// Move cursor down and clamp column.
    pub fn move_down(&mut self, insert_mode: bool) {
        self.cursor.move_down(&self.document, insert_mode);
    }

    /// Move cursor right, respecting mode.
    pub fn move_right_clamped(&mut self, insert_mode: bool) {
        self.cursor.move_right(&self.document, insert_mode);
    }

    /// Clamp cursor column to current line.
    pub fn clamp_cursor_col(&mut self, insert_mode: bool) {
        self.cursor.clamp_col(&self.document, insert_mode);
    }

    /// Move cursor to end of line.
    pub fn move_cursor_line_end(&mut self, insert_mode: bool) {
        self.cursor.move_line_end(&self.document, insert_mode);
    }

    /// Move cursor word forward.
    pub fn move_cursor_word_forward(&mut self) {
        self.cursor.move_word_forward(&self.document);
    }

    /// Move cursor word backward.
    pub fn move_cursor_word_backward(&mut self) {
        self.cursor.move_word_backward(&self.document);
    }

    /// Move cursor to word end.
    pub fn move_cursor_word_end(&mut self) {
        self.cursor.move_word_end(&self.document);
    }

    /// Jump cursor to bottom of document.
    pub fn jump_cursor_bottom(&mut self) {
        self.cursor.jump_bottom(&self.document);
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
