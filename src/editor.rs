use std::path::PathBuf;

use crate::cursor::CursorPos;
use crate::document::Document;
use crate::tree::{BlockInfo, TreeState};

pub struct Editor {
    document: Document,
    cursor: CursorPos,
    tree_state: TreeState,
    in_insert_mode: bool,
    /// Anchor of an active selection. `None` = no selection (just cursor).
    /// When `Some`, the selection runs from `anchor` to `cursor`.
    selection_anchor: Option<CursorPos>,
    /// When true, motions extend the selection rather than collapsing it.
    extend_mode: bool,
    /// Half-open line ranges marking lines that are wholly frozen — content
    /// the user cannot edit (typically Claude's words in the *claude* buffer).
    /// A line is either entirely frozen or entirely editable; mid-line splits
    /// are not allowed. Sorted, non-overlapping, all entries `s < e`.
    frozen_lines: Vec<(usize, usize)>,
    /// Line index marking the read-only prefix of the buffer. Edits on lines
    /// `< this` are silently rejected. Used by the *claude* buffer to lock
    /// prior turns once a new turn begins.
    lockable_through_line: usize,
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
            selection_anchor: None,
            extend_mode: false,
            frozen_lines: Vec::new(),
            lockable_through_line: 0,
        }
    }

    // --- Frozen lines / locked prefix ---

    /// Backward-compat: returns frozen line ranges projected to char ranges
    /// (covering the line text including its trailing newline) so view code
    /// that highlights frozen content keeps working.
    pub fn frozen_ranges(&self) -> Vec<(usize, usize)> {
        self.frozen_lines
            .iter()
            .map(|&(sl, el)| {
                let s = self.document.line_col_to_char(sl, 0);
                let e = if el >= self.document.line_count() {
                    self.document.rope().len_chars()
                } else {
                    self.document.line_col_to_char(el, 0)
                };
                (s, e)
            })
            .filter(|&(s, e)| s < e)
            .collect()
    }

    pub fn frozen_lines(&self) -> &[(usize, usize)] {
        &self.frozen_lines
    }

    /// Backward-compat: char index of the start of the first editable line.
    pub fn lockable_through_char(&self) -> usize {
        if self.lockable_through_line == 0 {
            0
        } else if self.lockable_through_line >= self.document.line_count() {
            self.document.rope().len_chars()
        } else {
            self.document.line_col_to_char(self.lockable_through_line, 0)
        }
    }

    pub fn lockable_through_line(&self) -> usize {
        self.lockable_through_line
    }

    pub fn set_lockable_through_line(&mut self, line: usize) {
        self.lockable_through_line = line;
    }

    /// Backward-compat shim. Accepts a char index; converts to a line index by
    /// snapping UP — char at the very start of a line locks lines above it
    /// only; any char in the middle/end of a line locks that line too.
    pub fn set_lockable_through_char(&mut self, c: usize) {
        self.lockable_through_line = char_to_line_ceil(&self.document, c);
    }

    /// Mark `[start_line, end_line)` as frozen. Existing ranges within or
    /// touching the new range are merged. Out-of-order or empty ranges are
    /// silently dropped.
    pub fn add_frozen_lines(&mut self, start_line: usize, end_line: usize) {
        if start_line >= end_line {
            return;
        }
        // Insert and merge adjacent/overlapping intervals.
        self.frozen_lines.push((start_line, end_line));
        self.frozen_lines.sort_by_key(|&(s, _)| s);
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(self.frozen_lines.len());
        for (s, e) in self.frozen_lines.drain(..) {
            if let Some(last) = merged.last_mut() {
                if s <= last.1 {
                    last.1 = last.1.max(e);
                    continue;
                }
            }
            merged.push((s, e));
        }
        self.frozen_lines = merged;
    }

    /// Backward-compat shim: accept a char range and convert to a line range
    /// using floor/ceil snapping. Used only by older call sites.
    pub fn add_frozen_range(&mut self, char_start: usize, char_end: usize) {
        let sl = char_to_line_floor(&self.document, char_start);
        let el = char_to_line_ceil(&self.document, char_end);
        self.add_frozen_lines(sl, el);
    }

    pub fn clear_frozen_ranges(&mut self) {
        self.frozen_lines.clear();
    }

    /// True if `line` is in any frozen range.
    pub fn is_frozen_line(&self, line: usize) -> bool {
        self.frozen_lines.iter().any(|&(s, e)| line >= s && line < e)
    }

    /// True if `char_idx` falls within any frozen line. Boundary semantics:
    /// the very first char of a frozen line counts as inside; the trailing
    /// newline of a frozen line is part of that line.
    pub fn is_in_frozen_range(&self, char_idx: usize) -> bool {
        let (line, _) = char_to_line_col(&self.document, char_idx);
        self.is_frozen_line(line)
    }

    /// Insert `ch` at `(line, col)` is allowed if:
    ///   - the line is past the locked prefix, AND
    ///   - the line is editable, OR the insert is `\n` at a line boundary of a
    ///     frozen line (col 0 or end-of-line) — opens a new editable line
    ///     before/after the frozen line without splitting it.
    fn can_insert_char_at(&self, line: usize, col: usize, ch: char) -> bool {
        if line < self.lockable_through_line {
            return false;
        }
        if !self.is_frozen_line(line) {
            return true;
        }
        if ch != '\n' {
            return false;
        }
        let line_len = self.document.line_len_chars(line);
        let line_end = line_len.saturating_sub(
            // Trailing '\n' counts toward line_len_chars; treat the position
            // immediately before it as end-of-line.
            if self.document.line_text(line).ends_with('\n') { 1 } else { 0 },
        );
        col == 0 || col >= line_end
    }

    /// Delete of `[del_s, del_e)` (char indices) is allowed iff:
    ///   - the start is at/past the locked prefix line, AND
    ///   - no character being deleted lives on a frozen line, AND
    ///   - the range does not delete a `\n` that joins an editable line into
    ///     an adjacent frozen line (which would merge them).
    fn can_delete_range(&self, del_s: usize, del_e: usize) -> bool {
        if del_s >= del_e {
            return true;
        }
        let (start_line, _) = char_to_line_col(&self.document, del_s);
        if start_line < self.lockable_through_line {
            return false;
        }
        // Walk every char in [del_s, del_e) and check the line isn't frozen.
        // Also check the boundary newline rule: a delete that ends inside a
        // frozen line would join an editable line above into the frozen line.
        let rope = self.document.rope();
        let mut line = start_line;
        let mut idx = del_s;
        let line_count = self.document.line_count();
        while idx < del_e {
            if self.is_frozen_line(line) {
                return false;
            }
            // Advance to next char; if it's a newline we cross to next line.
            let ch = match rope.get_char(idx) {
                Some(c) => c,
                None => break,
            };
            if ch == '\n' && line + 1 < line_count {
                // Deleting this newline would merge `line` (editable) into
                // `line + 1`. If the next line is frozen, reject.
                if self.is_frozen_line(line + 1) {
                    return false;
                }
                line += 1;
            }
            idx += 1;
        }
        true
    }

    /// Recompute frozen line ranges after inserting `text` at `(line, col)`.
    /// Lines strictly after the insertion shift by the number of inserted
    /// newlines. A line being inserted INTO (mid-line) keeps its identity;
    /// inserting newlines mid-editable-line splits that editable line into
    /// multiple editable lines (frozen ranges below shift down accordingly).
    fn shift_frozen_lines_for_insert(&mut self, line: usize, col: usize, text: &str) {
        let inserted_nl = text.chars().filter(|c| *c == '\n').count();
        if inserted_nl == 0 {
            return;
        }
        // The insertion is at (line, col). Frozen ranges entirely above `line`
        // are unchanged. Ranges starting at line `line+1` or later shift down.
        // A range that contains `line` (the line we're inserting into): only
        // possible when col == 0 with text ending in `\n` (per can_insert_*),
        // i.e., we're inserting whole lines above the frozen range — shift the
        // whole range down.
        for (s, e) in self.frozen_lines.iter_mut() {
            if *s > line {
                *s += inserted_nl;
                *e += inserted_nl;
            } else if *s == line && col == 0 {
                // Insertion is exactly at the start of this frozen range; the
                // new lines come ABOVE the range — shift it down.
                *s += inserted_nl;
                *e += inserted_nl;
            }
            // Else: insertion is mid-line on an editable line (`line` not in
            // any frozen range) — frozen ranges below have already been shifted
            // by the iteration logic above for any `s > line`, so we're done.
        }
        // lockable_through_line shifts the same way.
        if self.lockable_through_line > line
            || (self.lockable_through_line == line && col == 0)
        {
            self.lockable_through_line += inserted_nl;
        }
    }

    /// Recompute frozen line ranges after deleting `[del_s, del_e)`. Caller
    /// must have already verified no frozen line is touched.
    fn shift_frozen_lines_for_delete(&mut self, del_s: usize, del_e: usize) {
        if del_s >= del_e {
            return;
        }
        // Count `\n`s being deleted, and the line where the deletion starts.
        let rope = self.document.rope();
        let mut deleted_nl = 0usize;
        for i in del_s..del_e {
            if rope.get_char(i) == Some('\n') {
                deleted_nl += 1;
            }
        }
        if deleted_nl == 0 {
            return;
        }
        let (start_line, _) = char_to_line_col(&self.document, del_s);
        // Only frozen ranges strictly after `start_line` shift up; ranges
        // starting at `start_line` or earlier are not in the deleted region
        // (per can_delete_range). The first frozen range AFTER the delete
        // begins at some line > start_line + deleted_nl in the pre-delete
        // numbering; in post-delete numbering it begins at (orig - deleted_nl).
        for (s, e) in self.frozen_lines.iter_mut() {
            if *s > start_line {
                *s = s.saturating_sub(deleted_nl);
                *e = e.saturating_sub(deleted_nl);
            }
        }
        if self.lockable_through_line > start_line {
            self.lockable_through_line =
                self.lockable_through_line.saturating_sub(deleted_nl);
        }
    }

    /// Programmatic insert (bypasses lockable guard). Used by app.rs to push
    /// Claude replies into the *claude* buffer. Frozen-line shifts are applied
    /// before the insertion point is invalidated.
    pub fn programmatic_insert(&mut self, char_idx: usize, text: &str) {
        let (line, col) = char_to_line_col(&self.document, char_idx);
        self.shift_frozen_lines_for_insert(line, col, text);
        self.document.insert_str_at_char(char_idx, text);
    }

    /// Programmatic delete (bypasses both lockable AND frozen-overlap checks).
    pub fn programmatic_delete(&mut self, del_s: usize, del_e: usize) {
        let len = self.document.rope().len_chars();
        let s = del_s.min(len);
        let e = del_e.min(len);
        if s >= e {
            return;
        }
        // Compute newline count BEFORE the delete so we can shift correctly.
        self.shift_frozen_lines_for_delete(s, e);
        self.document.delete_range(s, e);
    }

    /// Walk the active region (lines `>= lockable_through_line`) and collect
    /// contiguous runs of editable lines (those NOT in any frozen range).
    /// Joins runs with a blank-line separator so distinct insertions are
    /// clearly delimited when shipped over the channel. Used by `:claude-send`
    /// to extract the user's inline edits without echoing Claude's prose.
    pub fn extract_editable_inserts(&self) -> String {
        let line_count = self.document.line_count();
        if self.lockable_through_line >= line_count {
            return String::new();
        }
        let mut runs: Vec<String> = Vec::new();
        let mut cur: Vec<String> = Vec::new();
        for l in self.lockable_through_line..line_count {
            if self.is_frozen_line(l) {
                if !cur.is_empty() {
                    let joined = cur.join("\n");
                    let trimmed = joined.trim();
                    if !trimmed.is_empty() {
                        runs.push(trimmed.to_string());
                    }
                    cur.clear();
                }
            } else {
                let line_text = self.document.line_text(l);
                let stripped = line_text.trim_end_matches('\n').to_string();
                cur.push(stripped);
            }
        }
        if !cur.is_empty() {
            let joined = cur.join("\n");
            let trimmed = joined.trim();
            if !trimmed.is_empty() {
                runs.push(trimmed.to_string());
            }
        }
        runs.join("\n\n")
    }

    // --- Selection ---

    /// Returns the current selection range as ((start_line, start_col), (end_line, end_col))
    /// where start <= end in document order. Returns None if no selection is active.
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let (a_l, a_c) = (anchor.line, anchor.col);
        let (c_l, c_c) = (self.cursor.line, self.cursor.col);
        if (a_l, a_c) <= (c_l, c_c) {
            Some(((a_l, a_c), (c_l, c_c)))
        } else {
            Some(((c_l, c_c), (a_l, a_c)))
        }
    }

    pub fn selection_anchor(&self) -> Option<CursorPos> {
        self.selection_anchor
    }

    /// Set the anchor to the cursor's current position (begin a selection).
    pub fn anchor_at_cursor(&mut self) {
        self.selection_anchor = Some(self.cursor);
    }

    /// Drop any active selection (cursor position unchanged).
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Collapse the selection to the cursor position.
    pub fn collapse_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Swap cursor and anchor.
    pub fn flip_selection(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.selection_anchor = Some(self.cursor);
            self.cursor = anchor;
        }
    }

    /// Select the entire document.
    pub fn select_all(&mut self) {
        let last_line = self.document.line_count().saturating_sub(1);
        let last_col = self.document.line_len_chars(last_line);
        self.selection_anchor = Some(CursorPos::new());
        self.cursor.line = last_line;
        self.cursor.col = last_col;
    }

    /// Extend the selection to encompass full lines: anchor to start of first line, cursor to
    /// end-of-line (newline) of the last line. If no selection exists, selects the current line.
    /// Calling repeatedly extends the selection downward by one line.
    pub fn extend_by_line(&mut self) {
        let line_count = self.document.line_count();
        if let Some(((sl, _), (el, _))) = self.selection_range() {
            // Was the previous selection already line-aligned? If so, extend down by one line.
            let prev_was_line_aligned = self
                .selection_anchor
                .map(|a| a.col == 0)
                .unwrap_or(false)
                && self.cursor.col == self.document.line_len_chars(el);
            let target_end_line = if prev_was_line_aligned {
                (el + 1).min(line_count.saturating_sub(1))
            } else {
                el
            };
            let mut a = CursorPos::new();
            a.line = sl;
            a.col = 0;
            self.selection_anchor = Some(a);
            self.cursor.line = target_end_line;
            self.cursor.col = self.document.line_len_chars(target_end_line);
        } else {
            let l = self.cursor.line;
            let mut a = CursorPos::new();
            a.line = l;
            a.col = 0;
            self.selection_anchor = Some(a);
            self.cursor.col = self.document.line_len_chars(l);
        }
    }

    pub fn extend_mode(&self) -> bool {
        self.extend_mode
    }

    pub fn set_extend_mode(&mut self, on: bool) {
        self.extend_mode = on;
    }

    pub fn toggle_extend_mode(&mut self) {
        self.extend_mode = !self.extend_mode;
    }

    /// Convert the current selection to a (start_char, end_char) char-offset tuple
    /// in the document's rope.
    fn selection_char_range(&self) -> Option<(usize, usize)> {
        let ((sl, sc), (el, ec)) = self.selection_range()?;
        let start = self.document.line_col_to_char(sl, sc);
        let end = self.document.line_col_to_char(el, ec);
        Some((start, end))
    }

    /// Get the text content of the current selection.
    pub fn selection_text(&self) -> Option<String> {
        let (start, end) = self.selection_char_range()?;
        if start == end {
            // Single-cell selection — yank the character at `start` if present.
            let rope = self.document.rope();
            if start < rope.len_chars() {
                return Some(rope.slice(start..start + 1).to_string());
            }
            return Some(String::new());
        }
        let rope = self.document.rope();
        let end = end.min(rope.len_chars());
        Some(rope.slice(start..end).to_string())
    }

    /// Delete the current selection. Returns true if a selection was deleted.
    /// If the selection is zero-width (just cursor), deletes one character at the cursor.
    pub fn delete_selection(&mut self) -> bool {
        let Some(((sl, sc), (el, ec))) = self.selection_range() else {
            return false;
        };
        let start = self.document.line_col_to_char(sl, sc);
        let mut end = self.document.line_col_to_char(el, ec);
        if start == end {
            let rope_len = self.document.rope().len_chars();
            if start < rope_len {
                end = start + 1;
            }
        }
        if start >= end {
            self.selection_anchor = None;
            return false;
        }
        if !self.can_delete_range(start, end) {
            // Selection covers locked or frozen content; refuse rather than
            // silently mutating Claude's words.
            self.selection_anchor = None;
            return false;
        }
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.shift_frozen_lines_for_delete(start, end);
        self.document.delete_range(start, end);
        self.cursor.line = sl;
        self.cursor.col = sc;
        // Clamp
        let line_count = self.document.line_count();
        if self.cursor.line >= line_count {
            self.cursor.line = line_count.saturating_sub(1);
        }
        let line_len = self.document.line_len_chars(self.cursor.line);
        if self.cursor.col > line_len {
            self.cursor.col = line_len;
        }
        self.selection_anchor = None;
        self.document.end_undo_group(self.cursor.line, self.cursor.col);
        self.reparse();
        true
    }

    /// Yank the current selection to the clipboard. Returns the yanked text.
    pub fn yank_selection(&self) -> Option<String> {
        self.selection_text()
    }

    /// Prepare for a motion. If `creates_selection` is true, a fresh selection
    /// is anchored at the current cursor (Helix-style word motion). If false,
    /// the selection is collapsed unless we're in extend mode.
    pub fn pre_move(&mut self, creates_selection: bool) {
        if self.extend_mode {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else if creates_selection {
            self.selection_anchor = Some(self.cursor);
        } else {
            self.selection_anchor = None;
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
        if !self.can_insert_char_at(self.cursor.line, self.cursor.col, ch) {
            return;
        }
        // Shift first (uses pre-insert document state), then mutate.
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.shift_frozen_lines_for_insert(self.cursor.line, self.cursor.col, s);
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
        let char_idx = self
            .document
            .line_col_to_char(self.cursor.line, self.cursor.col);
        if char_idx == 0 {
            return;
        }
        let del_s = char_idx - 1;
        let del_e = char_idx;
        if !self.can_delete_range(del_s, del_e) {
            return;
        }
        // Shift first (uses pre-delete document state), then mutate.
        self.shift_frozen_lines_for_delete(del_s, del_e);
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
        let char_idx = self
            .document
            .line_col_to_char(self.cursor.line, self.cursor.col);
        let rope_len = self.document.rope().len_chars();
        if char_idx >= rope_len {
            return;
        }
        let del_s = char_idx;
        let del_e = char_idx + 1;
        if !self.can_delete_range(del_s, del_e) {
            return;
        }
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.shift_frozen_lines_for_delete(del_s, del_e);
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
        let line = self.cursor.line;
        let line_start = self.document.line_col_to_char(line, 0);
        let line_end = if line + 1 < self.document.line_count() {
            self.document.line_col_to_char(line + 1, 0)
        } else {
            self.document.rope().len_chars()
        };
        if !self.can_delete_range(line_start, line_end) {
            return;
        }
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.shift_frozen_lines_for_delete(line_start, line_end);
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
        let line = self.cursor.line;
        let insert_col = self.document.line_len_chars(line);
        if !self.can_insert_char_at(line, insert_col, '\n') {
            return;
        }
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.shift_frozen_lines_for_insert(line, insert_col, "\n");
        self.document.insert_char(line, insert_col, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
        self.in_insert_mode = true;
        // Don't close undo group yet — will be closed by end_insert
    }

    /// Open a new line above cursor and enter insert mode.
    pub fn open_line_above(&mut self) {
        if !self.can_insert_char_at(self.cursor.line, 0, '\n') {
            return;
        }
        self.document
            .begin_undo_group(self.cursor.line, self.cursor.col);
        self.shift_frozen_lines_for_insert(self.cursor.line, 0, "\n");
        self.document.insert_char(self.cursor.line, 0, '\n');
        self.cursor.col = 0;
        self.in_insert_mode = true;
    }

    /// Undo last action.
    pub fn undo(&mut self) {
        if let Some((line, col)) = self.document.undo() {
            self.cursor.line = line.min(self.document.line_count().saturating_sub(1));
            self.cursor.col = col;
            self.clamp_cursor_col(false);
            self.reparse();
        }
    }

    /// Redo last undone action.
    pub fn redo(&mut self) {
        if let Some((line, col)) = self.document.redo() {
            self.cursor.line = line.min(self.document.line_count().saturating_sub(1));
            self.cursor.col = col;
            self.clamp_cursor_col(false);
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

    /// f<char> — find next `ch` on current line.
    pub fn find_char_forward(&mut self, ch: char) -> bool {
        self.cursor.find_char_forward(&self.document, ch)
    }

    /// F<char> — find previous `ch` on current line.
    pub fn find_char_backward(&mut self, ch: char) -> bool {
        self.cursor.find_char_backward(&self.document, ch)
    }

    /// t<char> — till next `ch` on current line.
    pub fn till_char_forward(&mut self, ch: char) -> bool {
        self.cursor.till_char_forward(&self.document, ch)
    }

    /// T<char> — till previous `ch` on current line.
    pub fn till_char_backward(&mut self, ch: char) -> bool {
        self.cursor.till_char_backward(&self.document, ch)
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

/// Convert a rope char index to (line, col).
fn char_to_line_col(doc: &Document, char_idx: usize) -> (usize, usize) {
    let rope = doc.rope();
    let len = rope.len_chars();
    let i = char_idx.min(len);
    let line = rope.char_to_line(i);
    let line_start = rope.line_to_char(line);
    (line, i - line_start)
}

/// Convert a char index to a line index, snapping DOWN — the line containing
/// the char.
fn char_to_line_floor(doc: &Document, char_idx: usize) -> usize {
    let (line, _) = char_to_line_col(doc, char_idx);
    line
}

/// Convert a char index to a line index, snapping UP — the next line if the
/// char is mid-line, the same line if it sits exactly on a line start.
fn char_to_line_ceil(doc: &Document, char_idx: usize) -> usize {
    let (line, col) = char_to_line_col(doc, char_idx);
    if col == 0 { line } else { line + 1 }
}
