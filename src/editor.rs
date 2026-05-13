use std::path::PathBuf;

use crate::cursor::CursorPos;
use crate::document::Document;
use crate::tree::{BlockInfo, TreeState};

/// Document substrate for a single file/buffer: rope, tree-sitter state, frozen
/// ranges, and the read-only-prefix bookmark. One `EditorCore` may have many
/// active `EditorView`s when the buffer is shared across windows (GPUI
/// workspace splits). Mutations route through `EditorView` methods, which take
/// `&mut EditorCore` to access this substrate.
pub struct EditorCore {
    document: Document,
    tree_state: TreeState,
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

/// Per-window cursor, selection, and insert-mode state attached to an
/// `EditorCore`. In the TUI a `Buffer` holds exactly one view; in the GPUI
/// workspace a single `EditorCore` may have multiple `EditorView`s when the
/// underlying file is open in more than one window.
pub struct EditorView {
    cursor: CursorPos,
    /// Anchor of an active selection. `None` = no selection (just cursor).
    /// When `Some`, the selection runs from `anchor` to `cursor`.
    selection_anchor: Option<CursorPos>,
    /// When true, motions extend the selection rather than collapsing it.
    extend_mode: bool,
    in_insert_mode: bool,
}

/// Convenience wrapper that pairs one `EditorCore` with one `EditorView`. The
/// TUI uses this as its per-buffer editor handle, preserving the 1:1
/// view-per-buffer relationship the TUI has always had. The GPUI workspace
/// composes `EditorCore` and `EditorView` separately (core lives in the buffer
/// pool, views live in windows).
pub struct Editor {
    core: EditorCore,
    view: EditorView,
}

// =============================================================================
// EditorCore
// =============================================================================

impl EditorCore {
    pub fn new(text: String, file_path: PathBuf) -> Self {
        let mut tree_state = TreeState::new();
        tree_state.parse(text.as_bytes());

        let document = Document::from_text(text, file_path);
        Self {
            document,
            tree_state,
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
        let rope = self.document.rope();
        let mut line = start_line;
        let mut idx = del_s;
        let line_count = self.document.line_count();
        while idx < del_e {
            if self.is_frozen_line(line) {
                return false;
            }
            let ch = match rope.get_char(idx) {
                Some(c) => c,
                None => break,
            };
            if ch == '\n' && line + 1 < line_count {
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
    fn shift_frozen_lines_for_insert(&mut self, line: usize, col: usize, text: &str) {
        let inserted_nl = text.chars().filter(|c| *c == '\n').count();
        if inserted_nl == 0 {
            return;
        }

        // Normalize: inserting at-or-past the visible end of a line is
        // identical (in the rope) to inserting at the start of the next line.
        let line_text = self.document.line_text(line);
        let visible_len = line_text.trim_end_matches('\n').chars().count();
        let (eff_line, eff_col) = if col >= visible_len {
            (line + 1, 0)
        } else {
            (line, col)
        };

        let mut new_ranges: Vec<(usize, usize)> = Vec::with_capacity(self.frozen_lines.len() + 1);
        for &(s, e) in self.frozen_lines.iter() {
            if e <= eff_line {
                new_ranges.push((s, e));
            } else if s >= eff_line {
                new_ranges.push((s + inserted_nl, e + inserted_nl));
            } else if eff_col == 0 {
                if s < eff_line {
                    new_ranges.push((s, eff_line));
                }
                let new_below = (eff_line + inserted_nl, e + inserted_nl);
                if new_below.0 < new_below.1 {
                    new_ranges.push(new_below);
                }
            } else {
                new_ranges.push((s, e));
            }
        }
        self.frozen_lines = new_ranges;

        if self.lockable_through_line > eff_line {
            self.lockable_through_line += inserted_nl;
        }
    }

    /// Recompute frozen line ranges after deleting `[del_s, del_e)`. Caller
    /// must have already verified no frozen line is touched.
    fn shift_frozen_lines_for_delete(&mut self, del_s: usize, del_e: usize) {
        if del_s >= del_e {
            return;
        }
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
    /// Claude replies into the *claude* buffer.
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
        self.shift_frozen_lines_for_delete(s, e);
        self.document.delete_range(s, e);
    }

    /// Walk the active region and collect contiguous runs of editable lines,
    /// joined with blank-line separators. Used by `:claude-send`.
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

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn tree_state(&self) -> &TreeState {
        &self.tree_state
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
    pub fn reparse(&mut self) {
        let text = self.document.full_text();
        self.tree_state.parse(text.as_bytes());
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.document.save()
    }

    pub fn save_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.document.save_to(path)
    }
}

// =============================================================================
// EditorView
// =============================================================================

impl EditorView {
    pub fn new() -> Self {
        Self {
            cursor: CursorPos::new(),
            selection_anchor: None,
            extend_mode: false,
            in_insert_mode: false,
        }
    }

    pub fn cursor(&self) -> &CursorPos {
        &self.cursor
    }

    pub fn cursor_mut(&mut self) -> &mut CursorPos {
        &mut self.cursor
    }

    pub fn is_insert_mode(&self) -> bool {
        self.in_insert_mode
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

    // --- Selection ---

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

    pub fn anchor_at_cursor(&mut self) {
        self.selection_anchor = Some(self.cursor);
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub fn collapse_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub fn flip_selection(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.selection_anchor = Some(self.cursor);
            self.cursor = anchor;
        }
    }

    pub fn select_all(&mut self, core: &EditorCore) {
        let last_line = core.document.line_count().saturating_sub(1);
        let last_col = core.document.line_len_chars(last_line);
        self.selection_anchor = Some(CursorPos::new());
        self.cursor.line = last_line;
        self.cursor.col = last_col;
    }

    pub fn extend_by_line(&mut self, core: &EditorCore) {
        let line_count = core.document.line_count();
        if let Some(((sl, _), (el, _))) = self.selection_range() {
            let prev_was_line_aligned = self
                .selection_anchor
                .map(|a| a.col == 0)
                .unwrap_or(false)
                && self.cursor.col == core.document.line_len_chars(el);
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
            self.cursor.col = core.document.line_len_chars(target_end_line);
        } else {
            let l = self.cursor.line;
            let mut a = CursorPos::new();
            a.line = l;
            a.col = 0;
            self.selection_anchor = Some(a);
            self.cursor.col = core.document.line_len_chars(l);
        }
    }

    fn selection_char_range(&self, core: &EditorCore) -> Option<(usize, usize)> {
        let ((sl, sc), (el, ec)) = self.selection_range()?;
        let start = core.document.line_col_to_char(sl, sc);
        let end = core.document.line_col_to_char(el, ec);
        Some((start, end))
    }

    pub fn selection_text(&self, core: &EditorCore) -> Option<String> {
        let (start, end) = self.selection_char_range(core)?;
        if start == end {
            let rope = core.document.rope();
            if start < rope.len_chars() {
                return Some(rope.slice(start..start + 1).to_string());
            }
            return Some(String::new());
        }
        let rope = core.document.rope();
        let end = end.min(rope.len_chars());
        Some(rope.slice(start..end).to_string())
    }

    pub fn delete_selection(&mut self, core: &mut EditorCore) -> bool {
        let Some(((sl, sc), (el, ec))) = self.selection_range() else {
            return false;
        };
        let start = core.document.line_col_to_char(sl, sc);
        let mut end = core.document.line_col_to_char(el, ec);
        if start == end {
            let rope_len = core.document.rope().len_chars();
            if start < rope_len {
                end = start + 1;
            }
        }
        if start >= end {
            self.selection_anchor = None;
            return false;
        }
        if !core.can_delete_range(start, end) {
            self.selection_anchor = None;
            return false;
        }
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_delete(start, end);
        core.document.delete_range(start, end);
        self.cursor.line = sl;
        self.cursor.col = sc;
        let line_count = core.document.line_count();
        if self.cursor.line >= line_count {
            self.cursor.line = line_count.saturating_sub(1);
        }
        let line_len = core.document.line_len_chars(self.cursor.line);
        if self.cursor.col > line_len {
            self.cursor.col = line_len;
        }
        self.selection_anchor = None;
        core.document.end_undo_group(self.cursor.line, self.cursor.col);
        core.reparse();
        true
    }

    pub fn yank_selection(&self, core: &EditorCore) -> Option<String> {
        self.selection_text(core)
    }

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

    // --- Insert / delete mutations ---

    pub fn begin_insert(&mut self, core: &mut EditorCore) {
        self.in_insert_mode = true;
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
    }

    pub fn end_insert(&mut self, core: &mut EditorCore) {
        self.in_insert_mode = false;
        core.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        core.reparse();
    }

    pub fn insert_char(&mut self, core: &mut EditorCore, ch: char) {
        if !core.can_insert_char_at(self.cursor.line, self.cursor.col, ch) {
            return;
        }
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        core.shift_frozen_lines_for_insert(self.cursor.line, self.cursor.col, s);
        core.document
            .insert_char(self.cursor.line, self.cursor.col, ch);
        if ch == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
        } else {
            self.cursor.col += 1;
        }
    }

    pub fn backspace(&mut self, core: &mut EditorCore) {
        let char_idx = core
            .document
            .line_col_to_char(self.cursor.line, self.cursor.col);
        if char_idx == 0 {
            return;
        }
        let del_s = char_idx - 1;
        let del_e = char_idx;
        if !core.can_delete_range(del_s, del_e) {
            return;
        }
        core.shift_frozen_lines_for_delete(del_s, del_e);
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            core.document.delete_char(self.cursor.line, self.cursor.col);
        } else if self.cursor.line > 0 {
            let prev_line_len = core.document.line_len_chars(self.cursor.line - 1);
            self.cursor.line -= 1;
            self.cursor.col = prev_line_len;
            core.document.delete_char(self.cursor.line, self.cursor.col);
        }
    }

    pub fn delete_char_at_cursor(&mut self, core: &mut EditorCore) {
        let char_idx = core
            .document
            .line_col_to_char(self.cursor.line, self.cursor.col);
        let rope_len = core.document.rope().len_chars();
        if char_idx >= rope_len {
            return;
        }
        let del_s = char_idx;
        let del_e = char_idx + 1;
        if !core.can_delete_range(del_s, del_e) {
            return;
        }
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_delete(del_s, del_e);
        core.document.delete_char(self.cursor.line, self.cursor.col);
        let line_len = core.document.line_len_chars(self.cursor.line);
        if self.cursor.col >= line_len && line_len > 0 {
            self.cursor.col = line_len - 1;
        }
        core.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        core.reparse();
    }

    pub fn delete_current_line(&mut self, core: &mut EditorCore) {
        let line = self.cursor.line;
        let line_start = core.document.line_col_to_char(line, 0);
        let line_end = if line + 1 < core.document.line_count() {
            core.document.line_col_to_char(line + 1, 0)
        } else {
            core.document.rope().len_chars()
        };
        if !core.can_delete_range(line_start, line_end) {
            return;
        }
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_delete(line_start, line_end);
        core.document.delete_line(self.cursor.line);
        if self.cursor.line >= core.document.line_count() {
            self.cursor.line = core.document.line_count().saturating_sub(1);
        }
        self.cursor.col = 0;
        core.document
            .end_undo_group(self.cursor.line, self.cursor.col);
        core.reparse();
    }

    pub fn open_line_below(&mut self, core: &mut EditorCore) {
        let line = self.cursor.line;
        let insert_col = core.document.line_len_chars(line);
        if !core.can_insert_char_at(line, insert_col, '\n') {
            return;
        }
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_insert(line, insert_col, "\n");
        core.document.insert_char(line, insert_col, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
        self.in_insert_mode = true;
    }

    pub fn open_line_above(&mut self, core: &mut EditorCore) {
        if !core.can_insert_char_at(self.cursor.line, 0, '\n') {
            return;
        }
        core.document.begin_undo_group(
            self.cursor.line,
            self.cursor.col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_insert(self.cursor.line, 0, "\n");
        core.document.insert_char(self.cursor.line, 0, '\n');
        self.cursor.col = 0;
        self.in_insert_mode = true;
    }

    pub fn undo(&mut self, core: &mut EditorCore) {
        let cur_frozen = core.frozen_lines.clone();
        let cur_lockable = core.lockable_through_line;
        if let Some((line, col, frozen, lockable)) =
            core.document.undo(&cur_frozen, cur_lockable)
        {
            core.frozen_lines = frozen;
            core.lockable_through_line = lockable;
            self.cursor.line = line.min(core.document.line_count().saturating_sub(1));
            self.cursor.col = col;
            self.clamp_cursor_col(core, false);
            core.reparse();
        }
    }

    pub fn redo(&mut self, core: &mut EditorCore) {
        let cur_frozen = core.frozen_lines.clone();
        let cur_lockable = core.lockable_through_line;
        if let Some((line, col, frozen, lockable)) =
            core.document.redo(&cur_frozen, cur_lockable)
        {
            core.frozen_lines = frozen;
            core.lockable_through_line = lockable;
            self.cursor.line = line.min(core.document.line_count().saturating_sub(1));
            self.cursor.col = col;
            self.clamp_cursor_col(core, false);
            core.reparse();
        }
    }

    pub fn active_block_index(&self, core: &EditorCore) -> Option<usize> {
        let byte_offset = core
            .document
            .line_col_to_byte(self.cursor.line, self.cursor.col);
        core.tree_state.active_block_at_byte(byte_offset)
    }

    // --- Motion delegates (operate on cursor with core's document) ---

    pub fn move_down(&mut self, core: &EditorCore, insert_mode: bool) {
        self.cursor.move_down(&core.document, insert_mode);
    }

    pub fn move_right_clamped(&mut self, core: &EditorCore, insert_mode: bool) {
        self.cursor.move_right(&core.document, insert_mode);
    }

    pub fn clamp_cursor_col(&mut self, core: &EditorCore, insert_mode: bool) {
        self.cursor.clamp_col(&core.document, insert_mode);
    }

    pub fn move_cursor_line_end(&mut self, core: &EditorCore, insert_mode: bool) {
        self.cursor.move_line_end(&core.document, insert_mode);
    }

    pub fn move_cursor_word_forward(&mut self, core: &EditorCore) {
        self.cursor.move_word_forward(&core.document);
    }

    pub fn move_cursor_word_backward(&mut self, core: &EditorCore) {
        self.cursor.move_word_backward(&core.document);
    }

    pub fn move_cursor_word_end(&mut self, core: &EditorCore) {
        self.cursor.move_word_end(&core.document);
    }

    pub fn jump_cursor_bottom(&mut self, core: &EditorCore) {
        self.cursor.jump_bottom(&core.document);
    }

    pub fn find_char_forward(&mut self, core: &EditorCore, ch: char) -> bool {
        self.cursor.find_char_forward(&core.document, ch)
    }

    pub fn find_char_backward(&mut self, core: &EditorCore, ch: char) -> bool {
        self.cursor.find_char_backward(&core.document, ch)
    }

    pub fn till_char_forward(&mut self, core: &EditorCore, ch: char) -> bool {
        self.cursor.till_char_forward(&core.document, ch)
    }

    pub fn till_char_backward(&mut self, core: &EditorCore, ch: char) -> bool {
        self.cursor.till_char_backward(&core.document, ch)
    }
}

impl Default for EditorView {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Editor (thin wrapper preserving the old surface — 1:1 view per buffer)
// =============================================================================

impl Editor {
    pub fn new(text: String, file_path: PathBuf) -> Self {
        Self {
            core: EditorCore::new(text, file_path),
            view: EditorView::new(),
        }
    }

    pub fn core(&self) -> &EditorCore {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut EditorCore {
        &mut self.core
    }

    pub fn view(&self) -> &EditorView {
        &self.view
    }

    pub fn view_mut(&mut self) -> &mut EditorView {
        &mut self.view
    }

    // --- Frozen lines / locked prefix (delegate to core) ---

    pub fn frozen_ranges(&self) -> Vec<(usize, usize)> {
        self.core.frozen_ranges()
    }

    pub fn frozen_lines(&self) -> &[(usize, usize)] {
        self.core.frozen_lines()
    }

    pub fn lockable_through_char(&self) -> usize {
        self.core.lockable_through_char()
    }

    pub fn lockable_through_line(&self) -> usize {
        self.core.lockable_through_line()
    }

    pub fn set_lockable_through_line(&mut self, line: usize) {
        self.core.set_lockable_through_line(line);
    }

    pub fn set_lockable_through_char(&mut self, c: usize) {
        self.core.set_lockable_through_char(c);
    }

    pub fn add_frozen_lines(&mut self, start_line: usize, end_line: usize) {
        self.core.add_frozen_lines(start_line, end_line);
    }

    pub fn add_frozen_range(&mut self, char_start: usize, char_end: usize) {
        self.core.add_frozen_range(char_start, char_end);
    }

    pub fn clear_frozen_ranges(&mut self) {
        self.core.clear_frozen_ranges();
    }

    pub fn is_frozen_line(&self, line: usize) -> bool {
        self.core.is_frozen_line(line)
    }

    pub fn is_in_frozen_range(&self, char_idx: usize) -> bool {
        self.core.is_in_frozen_range(char_idx)
    }

    pub fn programmatic_insert(&mut self, char_idx: usize, text: &str) {
        self.core.programmatic_insert(char_idx, text);
    }

    pub fn programmatic_delete(&mut self, del_s: usize, del_e: usize) {
        self.core.programmatic_delete(del_s, del_e);
    }

    pub fn extract_editable_inserts(&self) -> String {
        self.core.extract_editable_inserts()
    }

    pub fn document(&self) -> &Document {
        self.core.document()
    }

    pub fn document_mut(&mut self) -> &mut Document {
        self.core.document_mut()
    }

    pub fn tree_state(&self) -> &TreeState {
        self.core.tree_state()
    }

    pub fn block_boundaries(&self) -> Vec<BlockInfo> {
        self.core.block_boundaries()
    }

    pub fn block_text(&self, block_index: usize) -> String {
        self.core.block_text(block_index)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.core.save()
    }

    pub fn save_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.core.save_to(path)
    }

    // --- Selection / cursor / mode (delegate to view) ---

    pub fn cursor(&self) -> &CursorPos {
        self.view.cursor()
    }

    pub fn cursor_mut(&mut self) -> &mut CursorPos {
        self.view.cursor_mut()
    }

    pub fn is_insert_mode(&self) -> bool {
        self.view.is_insert_mode()
    }

    pub fn extend_mode(&self) -> bool {
        self.view.extend_mode()
    }

    pub fn set_extend_mode(&mut self, on: bool) {
        self.view.set_extend_mode(on);
    }

    pub fn toggle_extend_mode(&mut self) {
        self.view.toggle_extend_mode();
    }

    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        self.view.selection_range()
    }

    pub fn selection_anchor(&self) -> Option<CursorPos> {
        self.view.selection_anchor()
    }

    pub fn anchor_at_cursor(&mut self) {
        self.view.anchor_at_cursor();
    }

    pub fn clear_selection(&mut self) {
        self.view.clear_selection();
    }

    pub fn collapse_selection(&mut self) {
        self.view.collapse_selection();
    }

    pub fn flip_selection(&mut self) {
        self.view.flip_selection();
    }

    pub fn select_all(&mut self) {
        self.view.select_all(&self.core);
    }

    pub fn extend_by_line(&mut self) {
        self.view.extend_by_line(&self.core);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.view.selection_text(&self.core)
    }

    pub fn delete_selection(&mut self) -> bool {
        self.view.delete_selection(&mut self.core)
    }

    pub fn yank_selection(&self) -> Option<String> {
        self.view.yank_selection(&self.core)
    }

    pub fn pre_move(&mut self, creates_selection: bool) {
        self.view.pre_move(creates_selection);
    }

    // --- Insert / delete mutations (split borrows view + core) ---

    pub fn begin_insert(&mut self) {
        self.view.begin_insert(&mut self.core);
    }

    pub fn end_insert(&mut self) {
        self.view.end_insert(&mut self.core);
    }

    pub fn insert_char(&mut self, ch: char) {
        self.view.insert_char(&mut self.core, ch);
    }

    pub fn backspace(&mut self) {
        self.view.backspace(&mut self.core);
    }

    pub fn delete_char_at_cursor(&mut self) {
        self.view.delete_char_at_cursor(&mut self.core);
    }

    pub fn delete_current_line(&mut self) {
        self.view.delete_current_line(&mut self.core);
    }

    pub fn open_line_below(&mut self) {
        self.view.open_line_below(&mut self.core);
    }

    pub fn open_line_above(&mut self) {
        self.view.open_line_above(&mut self.core);
    }

    pub fn undo(&mut self) {
        self.view.undo(&mut self.core);
    }

    pub fn redo(&mut self) {
        self.view.redo(&mut self.core);
    }

    pub fn active_block_index(&self) -> Option<usize> {
        self.view.active_block_index(&self.core)
    }

    // --- Motion delegates ---

    pub fn move_down(&mut self, insert_mode: bool) {
        self.view.move_down(&self.core, insert_mode);
    }

    pub fn move_right_clamped(&mut self, insert_mode: bool) {
        self.view.move_right_clamped(&self.core, insert_mode);
    }

    pub fn clamp_cursor_col(&mut self, insert_mode: bool) {
        self.view.clamp_cursor_col(&self.core, insert_mode);
    }

    pub fn move_cursor_line_end(&mut self, insert_mode: bool) {
        self.view.move_cursor_line_end(&self.core, insert_mode);
    }

    pub fn move_cursor_word_forward(&mut self) {
        self.view.move_cursor_word_forward(&self.core);
    }

    pub fn move_cursor_word_backward(&mut self) {
        self.view.move_cursor_word_backward(&self.core);
    }

    pub fn move_cursor_word_end(&mut self) {
        self.view.move_cursor_word_end(&self.core);
    }

    pub fn jump_cursor_bottom(&mut self) {
        self.view.jump_cursor_bottom(&self.core);
    }

    pub fn find_char_forward(&mut self, ch: char) -> bool {
        self.view.find_char_forward(&self.core, ch)
    }

    pub fn find_char_backward(&mut self, ch: char) -> bool {
        self.view.find_char_backward(&self.core, ch)
    }

    pub fn till_char_forward(&mut self, ch: char) -> bool {
        self.view.till_char_forward(&self.core, ch)
    }

    pub fn till_char_backward(&mut self, ch: char) -> bool {
        self.view.till_char_backward(&self.core, ch)
    }
}

// =============================================================================
// Helpers (private to this module)
// =============================================================================

fn char_to_line_col(doc: &Document, char_idx: usize) -> (usize, usize) {
    let rope = doc.rope();
    let len = rope.len_chars();
    let i = char_idx.min(len);
    let line = rope.char_to_line(i);
    let line_start = rope.line_to_char(line);
    (line, i - line_start)
}

fn char_to_line_floor(doc: &Document, char_idx: usize) -> usize {
    let (line, _) = char_to_line_col(doc, char_idx);
    line
}

fn char_to_line_ceil(doc: &Document, char_idx: usize) -> usize {
    let (line, col) = char_to_line_col(doc, char_idx);
    if col == 0 { line } else { line + 1 }
}
