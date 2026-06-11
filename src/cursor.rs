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
        Self {
            line: 0,
            col: 0,
            desired_col: None,
        }
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
        self.desired_col = None;
    }

    pub fn move_right(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        let max_col = if insert_mode {
            line_len
        } else {
            line_len.saturating_sub(1)
        };
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
            self.clamp_col(doc, insert_mode);
        }
    }

    /// Clamp column to valid range for current line. Call after vertical movement.
    pub fn clamp_col(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        let max_col = if insert_mode {
            line_len
        } else {
            line_len.saturating_sub(1)
        };
        let target = self.desired_col.unwrap_or(self.col);
        self.col = target.min(max_col);
    }

    pub fn move_line_start(&mut self) {
        self.col = 0;
        self.desired_col = None;
    }

    pub fn move_line_end(&mut self, doc: &Document, insert_mode: bool) {
        let line_len = doc.line_len_chars(self.line);
        self.col = if insert_mode {
            line_len
        } else {
            line_len.saturating_sub(1)
        };
        self.desired_col = None;
    }

    pub fn move_word_forward(&mut self, doc: &Document) {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut i = self.col;

        // Skip current word
        if i < len && is_word_char(chars[i]) {
            while i < len && is_word_char(chars[i]) {
                i += 1;
            }
        } else if i < len && !chars[i].is_whitespace() {
            while i < len && !chars[i].is_whitespace() && !is_word_char(chars[i]) {
                i += 1;
            }
        }
        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            if chars[i] == '\n' {
                break;
            }
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
                while j < next_chars.len() && next_chars[j].is_whitespace() && next_chars[j] != '\n'
                {
                    j += 1;
                }
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
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        // Skip word backwards
        if i > 0 && is_word_char(chars[i - 1]) {
            while i > 0 && is_word_char(chars[i - 1]) {
                i -= 1;
            }
        } else if i > 0 {
            while i > 0 && !chars[i - 1].is_whitespace() && !is_word_char(chars[i - 1]) {
                i -= 1;
            }
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
        while i < len && chars[i].is_whitespace() && chars[i] != '\n' {
            i += 1;
        }
        // Move to end of word
        if i < len && is_word_char(chars[i]) {
            while i + 1 < len && is_word_char(chars[i + 1]) {
                i += 1;
            }
        } else if i < len && !chars[i].is_whitespace() {
            while i + 1 < len && !chars[i + 1].is_whitespace() && !is_word_char(chars[i + 1]) {
                i += 1;
            }
        }

        self.col = i.min(len.saturating_sub(1));
        self.desired_col = None;
    }

    /// Find next occurrence of `ch` on the current line after the cursor.
    /// Returns true if found and cursor moved.
    pub fn find_char_forward(&mut self, doc: &Document, ch: char) -> bool {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        let start = self.col + 1;
        for (offset, &c) in chars.iter().enumerate().skip(start) {
            if c == '\n' {
                break;
            }
            if c == ch {
                self.col = offset;
                self.desired_col = None;
                return true;
            }
        }
        false
    }

    /// Find previous occurrence of `ch` on the current line before the cursor.
    pub fn find_char_backward(&mut self, doc: &Document, ch: char) -> bool {
        if self.col == 0 {
            return false;
        }
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        for i in (0..self.col).rev() {
            if chars[i] == ch {
                self.col = i;
                self.desired_col = None;
                return true;
            }
        }
        false
    }

    /// Move forward to the position just before the next occurrence of `ch`
    /// on the current line. No movement if the immediately-next char is `ch`.
    pub fn till_char_forward(&mut self, doc: &Document, ch: char) -> bool {
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        let start = self.col + 1;
        for (offset, &c) in chars.iter().enumerate().skip(start) {
            if c == '\n' {
                break;
            }
            if c == ch {
                self.col = offset.saturating_sub(1);
                self.desired_col = None;
                return true;
            }
        }
        false
    }

    /// Move backward to the position just after the previous occurrence of
    /// `ch` on the current line.
    pub fn till_char_backward(&mut self, doc: &Document, ch: char) -> bool {
        if self.col == 0 {
            return false;
        }
        let text = doc.line_text(self.line);
        let chars: Vec<char> = text.chars().collect();
        for i in (0..self.col).rev() {
            if chars[i] == ch {
                self.col = i + 1;
                self.desired_col = None;
                return true;
            }
        }
        false
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

    /// Jump to a specific line (0-indexed), clamped to the document's last
    /// line. Column resets to 0. Used by `<num>g`/`<num>G` and (via
    /// repeated stepping at the dispatch layer) half-page paging.
    pub fn jump_to_line(&mut self, doc: &Document, line: usize) {
        self.line = line.min(doc.line_count().saturating_sub(1));
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
