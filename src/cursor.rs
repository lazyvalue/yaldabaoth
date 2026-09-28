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

    /// Absolute column placement (undo/redo restore, selection anchor, click):
    /// sets the column and CLEARS the sticky vertical column, so a following
    /// clamp / j / k uses THIS column rather than a stale `desired_col` left over
    /// from an earlier vertical run. Writing `cursor.col` directly skips this and
    /// lets the old sticky column win — the undo-lands-at-the-wrong-column bug.
    pub fn set_col(&mut self, col: usize) {
        self.col = col;
        self.desired_col = None;
    }

    /// Absolute `(line, col)` placement, clearing the sticky column. See
    /// [`set_col`](Self::set_col).
    pub fn set_pos(&mut self, line: usize, col: usize) {
        self.line = line;
        self.col = col;
        self.desired_col = None;
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
        self.desired_col = None;
    }

    pub fn move_right(&mut self, doc: &Document, insert_mode: bool) {
        if self.col < max_col(doc, self.line, insert_mode) {
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
        let target = self.desired_col.unwrap_or(self.col);
        self.col = target.min(max_col(doc, self.line, insert_mode));
    }

    pub fn move_line_start(&mut self) {
        self.col = 0;
        self.desired_col = None;
    }

    /// Move to the first non-blank character of the current line (vim `^`).
    /// Skips leading whitespace; a blank or whitespace-only line lands on
    /// column 0.
    pub fn move_first_non_blank(&mut self, doc: &Document) {
        let text = doc.line_text(self.line);
        // `is_whitespace()` already excludes the trailing '\n', so the search
        // never matches the line terminator.
        self.col = text.chars().position(|c| !c.is_whitespace()).unwrap_or(0);
        self.desired_col = None;
    }

    pub fn move_line_end(&mut self, doc: &Document, insert_mode: bool) {
        self.col = max_col(doc, self.line, insert_mode);
        self.desired_col = None;
    }

    pub fn move_word_forward(&mut self, doc: &Document) {
        let line = doc.rope().line(self.line.min(doc.line_count().saturating_sub(1)));
        let len = line.len_chars();
        let mut i = self.col.min(len);
        let mut chars = line.chars_at(i).peekable();

        // Skip the current word / punctuation run.
        if let Some(&c) = chars.peek() {
            let cls = char_class(c);
            if cls != CharClass::Space {
                while chars.peek().is_some_and(|&c| char_class(c) == cls) {
                    chars.next();
                    i += 1;
                }
            }
        }
        // Skip whitespace up to (not across) the line break.
        while chars.peek().is_some_and(|&c| c.is_whitespace() && c != '\n') {
            chars.next();
            i += 1;
        }

        if chars.peek().is_none_or(|&c| c == '\n') {
            // Move to next line, onto its first non-blank.
            if self.line + 1 < doc.line_count() {
                self.line += 1;
                self.col = doc
                    .rope()
                    .line(self.line)
                    .chars()
                    .take_while(|&c| c.is_whitespace() && c != '\n')
                    .count();
            }
        } else {
            self.col = i;
        }
        self.desired_col = None;
    }

    /// Vim `b`: back to the start of the previous word, crossing line breaks.
    /// Whitespace (including newlines) before the caret is skipped, except that
    /// an empty line counts as a word and stops the motion; then the caret
    /// walks back over one same-class run (B7: at col 0 this lands on the start
    /// of the previous line's last word, not its last char).
    pub fn move_word_backward(&mut self, doc: &Document) {
        let rope = doc.rope();
        let start = doc.line_col_to_char(self.line, self.col);
        let mut i = start;
        let mut back = rope.chars_at(i);
        let mut prev = |i: usize| -> Option<char> { if i == 0 { None } else { back.prev() } };
        // `pending` holds the char at i-1 once read.
        let mut pending = prev(i);
        while let Some(c) = pending {
            if char_class(c) != CharClass::Space {
                break;
            }
            i -= 1;
            pending = prev(i);
            // Empty line: the char just stepped over is a '\n' that starts its
            // line (the one before it is also a '\n', or it is the doc start).
            if c == '\n' && i < start && (pending.is_none() || pending == Some('\n')) {
                let (l, col) = doc.line_col_of_char(i);
                self.line = l;
                self.col = col;
                self.desired_col = None;
                return;
            }
        }
        if let Some(c) = pending {
            let cls = char_class(c);
            while pending.is_some_and(|c| char_class(c) == cls) {
                i -= 1;
                pending = prev(i);
            }
        }
        let (l, col) = doc.line_col_of_char(i);
        self.line = l;
        self.col = col;
        self.desired_col = None;
    }

    /// Vim `e`: forward to the end of the next word, crossing line breaks
    /// (B7: from a line's last word it continues to the next line's first word
    /// end instead of resting on the '\n' column). With nothing ahead the caret
    /// stays put.
    pub fn move_word_end(&mut self, doc: &Document) {
        let rope = doc.rope();
        let len = rope.len_chars();
        let mut i = doc.line_col_to_char(self.line, self.col) + 1;
        if i >= len {
            return;
        }
        let mut chars = rope.chars_at(i).peekable();
        while chars.peek().is_some_and(|&c| char_class(c) == CharClass::Space) {
            chars.next();
            i += 1;
        }
        let Some(first) = chars.next() else {
            return;
        };
        let cls = char_class(first);
        while chars.peek().is_some_and(|&c| char_class(c) == cls) {
            chars.next();
            i += 1;
        }
        let (l, col) = doc.line_col_of_char(i);
        self.line = l;
        self.col = col;
        self.desired_col = None;
    }

    /// Column of the nearest `ch` on the current line strictly after the caret
    /// (`forward`) or strictly before it, without leaving the line.
    fn find_on_line(&self, doc: &Document, ch: char, forward: bool) -> Option<usize> {
        if self.line >= doc.line_count() {
            return None;
        }
        let line = doc.rope().line(self.line);
        let col = self.col.min(line.len_chars());
        if forward {
            line.chars_at(col)
                .enumerate()
                .skip(1)
                .take_while(|&(_, c)| c != '\n')
                .find(|&(_, c)| c == ch)
                .map(|(k, _)| col + k)
        } else {
            let mut it = line.chars_at(col);
            (0..col).rev().find(|_| it.prev() == Some(ch))
        }
    }

    /// Find next occurrence of `ch` on the current line after the cursor.
    /// Returns true if found and cursor moved.
    pub fn find_char_forward(&mut self, doc: &Document, ch: char) -> bool {
        self.jump_col(self.find_on_line(doc, ch, true))
    }

    /// Find previous occurrence of `ch` on the current line before the cursor.
    pub fn find_char_backward(&mut self, doc: &Document, ch: char) -> bool {
        self.jump_col(self.find_on_line(doc, ch, false))
    }

    /// Move forward to the position just before the next occurrence of `ch`
    /// on the current line. No movement if the immediately-next char is `ch`.
    pub fn till_char_forward(&mut self, doc: &Document, ch: char) -> bool {
        self.jump_col(self.find_on_line(doc, ch, true).map(|c| c.saturating_sub(1)))
    }

    /// Move backward to the position just after the previous occurrence of
    /// `ch` on the current line.
    pub fn till_char_backward(&mut self, doc: &Document, ch: char) -> bool {
        self.jump_col(self.find_on_line(doc, ch, false).map(|c| c + 1))
    }

    fn jump_col(&mut self, col: Option<usize>) -> bool {
        match col {
            Some(c) => {
                self.col = c;
                self.desired_col = None;
                true
            }
            None => false,
        }
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

/// B15: the one character classification every word motion (`w` / `b` / `e`)
/// uses — whitespace (incl. '\n'), word chars (alphanumeric + `_`), and
/// everything else (punctuation runs are their own words, like vim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Space,
    Word,
    Punct,
}

pub fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// B15: the one "furthest caret column" rule — Insert may rest one past the
/// last char (at EOL); Normal sits ON the last char.
fn max_col(doc: &Document, line: usize, insert_mode: bool) -> usize {
    let line_len = doc.line_len_chars(line);
    if insert_mode {
        line_len
    } else {
        line_len.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::path::PathBuf;

    fn doc(text: &str) -> Document {
        Document::from_text(text.to_string(), PathBuf::from("test.md"))
    }

    #[test]
    fn first_non_blank_skips_leading_whitespace() {
        // lines: 0="    foo", 1="bar", 2="   " (whitespace-only)
        let d = doc("    foo\nbar\n   \n");
        let mut c = CursorPos::new();

        // indented line: `^` lands on the first non-blank char (col 4)
        c.line = 0;
        c.col = 6;
        c.move_first_non_blank(&d);
        assert_eq!(c.col, 4);

        // no indent: `^` lands on col 0 (same as `0`)
        c.line = 1;
        c.col = 2;
        c.move_first_non_blank(&d);
        assert_eq!(c.col, 0);

        // whitespace-only line: no non-blank char → col 0
        c.line = 2;
        c.col = 1;
        c.move_first_non_blank(&d);
        assert_eq!(c.col, 0);
    }
}
