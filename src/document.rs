use ropey::Rope;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One primitive text edit recorded for undo: at `start` (a rope char index in
/// the state *before* this splice applied) the run of `removed` text was
/// replaced by the `inserted` text.
///
/// This is the delta that finding #4 replaces the old whole-rope `before_text`
/// snapshot with: storing only the affected region keeps every recorded edit
/// O(edit) rather than O(document), so typing one character into a buffer that
/// also holds a multi-thousand-line frozen transcript no longer snapshots the
/// whole transcript per keystroke.
#[derive(Debug, Clone)]
struct Splice {
    /// Char index where the edit began (in the pre-splice rope).
    start: usize,
    /// Text that occupied `[start, start + removed.chars().count())` before the
    /// edit — what undo must put back.
    removed: String,
    /// Text the edit inserted at `start`. Undo removes
    /// `[start, start + inserted.chars().count())`; redo re-inserts it. Stored
    /// in full (still O(edit), never O(document)) so redo is exactly symmetric.
    inserted: String,
}

/// Ordered deltas for one undo group, recorded in application order. Undo
/// inverts them in reverse; redo re-applies them in forward order. Replaces the
/// old `before_text: String` whole-rope snapshot — the snapshot state is now
/// unrepresentable.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    splices: Vec<Splice>,
    cursor_before_line: usize,
    cursor_before_col: usize,
    cursor_after_line: usize,
    cursor_after_col: usize,
    /// Snapshot of the editor's frozen-line ranges + lockable-through-line
    /// taken at begin_undo_group; restored on undo so frozen state stays in
    /// sync with the rope content. Empty + 0 if the editor never set them.
    frozen_lines_before: Vec<(usize, usize)>,
    lockable_through_line_before: usize,
}

/// Advance a tree-sitter `Point` by appending `text`. Columns are byte
/// offsets within the row: a text with no newline extends the column by its
/// byte length; a text with `k` newlines moves down `k` rows and the column
/// becomes the byte length of the trailing segment after the last `\n`.
fn advance_point(start: tree_sitter::Point, text: &str) -> tree_sitter::Point {
    let newlines = text.bytes().filter(|&b| b == b'\n').count();
    if newlines == 0 {
        tree_sitter::Point {
            row: start.row,
            column: start.column + text.len(),
        }
    } else {
        let trailing = text.rsplit('\n').next().unwrap_or("");
        tree_sitter::Point {
            row: start.row + newlines,
            column: trailing.len(),
        }
    }
}

pub struct Document {
    rope: Rope,
    pub file_path: PathBuf,
    modified: bool,
    /// Monotonic counter bumped on every content mutation (inserts, deletes,
    /// undo, redo). Cheap O(1) signal that lets readers — notably the GUI
    /// highlight cache — skip work when the text is unchanged. Never reset.
    edit_seq: u64,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    /// Pending undo group: snapshot taken at begin_undo_group
    pending_undo: Option<UndoEntry>,
    /// One-clean-splice incremental-reparse tracking. `record_splice` computes
    /// the tree-sitter `InputEdit` for each primitive splice (against the OLD
    /// rope, before the mutation), and `take_pending_edit` hands it to the next
    /// `reparse` ONLY when exactly one splice happened since the last reparse
    /// (the typing hot path). Zero or multiple splices reset it to `None`, so
    /// reparse falls back to a full parse — making a wrong `InputEdit` the only
    /// possible incremental hazard, confined to `note_pending_edit`.
    pending_edit: Option<tree_sitter::InputEdit>,
    pending_splice_count: u32,
}

impl Document {
    pub fn from_text(text: String, file_path: PathBuf) -> Self {
        Self {
            rope: Rope::from_str(&text),
            file_path,
            modified: false,
            edit_seq: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_undo: None,
            pending_edit: None,
            pending_splice_count: 0,
        }
    }

    /// Current edit generation. Increases on every content mutation; equal
    /// values across two observations guarantee the text is byte-identical.
    pub fn edit_seq(&self) -> u64 {
        self.edit_seq
    }

    /// Mark the document mutated: flips `modified` and bumps `edit_seq`.
    /// Every rope-mutating method funnels through here so no edit can change
    /// the text without advancing the generation counter.
    fn touch(&mut self) {
        self.modified = true;
        self.edit_seq = self.edit_seq.wrapping_add(1);
    }

    /// Record one primitive splice into the pending undo group, if a group is
    /// open. `start` is the char index, `removed_chars` the count of chars that
    /// lived in `[start, start + removed_chars)` *before* the edit, and
    /// `inserted` the text the edit places at `start`. Cost is
    /// O(removed + inserted), never O(document) — the point of finding #4.
    ///
    /// Callers must invoke this *before* mutating the rope, so the removed text
    /// can be read from the current (pre-edit) rope.
    fn record_splice(&mut self, start: usize, removed_chars: usize, inserted: &str) {
        // Compute the incremental-reparse edit for EVERY splice (independent of
        // undo grouping), using the OLD rope (this runs before the mutation).
        self.note_pending_edit(start, removed_chars, inserted);
        if self.pending_undo.is_none() {
            return;
        }
        let len = self.rope.len_chars();
        let s = start.min(len);
        let e = (start + removed_chars).min(len);
        let removed = if s < e {
            self.rope.slice(s..e).to_string()
        } else {
            String::new()
        };
        // Borrow after the immutable rope reads above are done.
        if let Some(entry) = self.pending_undo.as_mut() {
            entry.splices.push(Splice {
                start: s,
                removed,
                inserted: inserted.to_string(),
            });
        }
    }

    /// tree-sitter `Point` (row, BYTE-column within the row) for a char index
    /// in the CURRENT rope. tree-sitter columns are byte offsets, not chars.
    fn char_point(&self, char_idx: usize) -> tree_sitter::Point {
        let ci = char_idx.min(self.rope.len_chars());
        let row = self.rope.char_to_line(ci);
        let line_start_byte = self.rope.line_to_byte(row);
        let byte = self.rope.char_to_byte(ci);
        tree_sitter::Point {
            row,
            column: byte - line_start_byte,
        }
    }

    /// Compute + accumulate the tree-sitter `InputEdit` for one primitive
    /// splice. MUST be called BEFORE the rope mutates (it reads the OLD rope to
    /// resolve the start / old-end byte+point). The new-end is the start
    /// advanced by `inserted`. First splice since the last `take_pending_edit`
    /// is stored; a 2nd marks the window multi-splice → `None` (full reparse).
    fn note_pending_edit(&mut self, start: usize, removed_chars: usize, inserted: &str) {
        let len = self.rope.len_chars();
        let s = start.min(len);
        let e = (start + removed_chars).min(len);
        let start_byte = self.rope.char_to_byte(s);
        let start_point = self.char_point(s);
        let old_end_byte = self.rope.char_to_byte(e);
        let old_end_point = self.char_point(e);
        let new_end_byte = start_byte + inserted.len();
        let new_end_point = advance_point(start_point, inserted);
        let edit = tree_sitter::InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: start_point,
            old_end_position: old_end_point,
            new_end_position: new_end_point,
        };
        self.pending_splice_count += 1;
        self.pending_edit = if self.pending_splice_count == 1 {
            Some(edit)
        } else {
            None
        };
    }

    /// Consume the pending incremental `InputEdit` for the next reparse,
    /// resetting the per-reparse window. `Some` ONLY when exactly one clean
    /// splice happened since the last call (the typing hot path); `None`
    /// otherwise (zero or multiple splices → full reparse).
    pub fn take_pending_edit(&mut self) -> Option<tree_sitter::InputEdit> {
        let edit = if self.pending_splice_count == 1 {
            self.pending_edit.take()
        } else {
            None
        };
        self.pending_edit = None;
        self.pending_splice_count = 0;
        edit
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

    /// O(1) tail probe: the last char of the document, or `None` if empty.
    /// Lets callers test trailing-newline / emptiness without cloning the whole
    /// rope to a String (`full_text`), which is O(n) in the transcript length.
    pub fn last_char(&self) -> Option<char> {
        let len = self.rope.len_chars();
        if len == 0 {
            None
        } else {
            self.rope.get_char(len - 1)
        }
    }

    /// O(1): true if the document holds no characters.
    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
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
        let clamped_line = line.min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(clamped_line);
        let line_len = self.line_len_chars(clamped_line);
        line_start + col.min(line_len)
    }

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) {
        let char_idx = self.line_col_to_char(line, col);
        let mut buf = [0u8; 4];
        self.record_splice(char_idx, 0, ch.encode_utf8(&mut buf));
        self.rope.insert_char(char_idx, ch);
        self.touch();
        self.redo_stack.clear();
    }

    pub fn delete_char(&mut self, line: usize, col: usize) {
        let char_idx = self.line_col_to_char(line, col);
        if char_idx < self.rope.len_chars() {
            self.record_splice(char_idx, 1, "");
            self.rope.remove(char_idx..char_idx + 1);
            self.touch();
            self.redo_stack.clear();
        }
    }

    /// Delete the character range `[start_char, end_char)` (rope char indices).
    pub fn delete_range(&mut self, start_char: usize, end_char: usize) {
        let len = self.rope.len_chars();
        let s = start_char.min(len);
        let e = end_char.min(len);
        if s < e {
            self.record_splice(s, e - s, "");
            self.rope.remove(s..e);
            self.touch();
            self.redo_stack.clear();
        }
    }

    /// Insert a string at a (line, col) position.
    pub fn insert_str(&mut self, line: usize, col: usize, text: &str) {
        let char_idx = self.line_col_to_char(line, col);
        self.record_splice(char_idx, 0, text);
        self.rope.insert(char_idx, text);
        self.touch();
        self.redo_stack.clear();
    }

    /// Insert a string at a rope char index. Used when splicing a precomputed
    /// region — see `app.rs::append_to_claude_buffer`.
    pub fn insert_str_at_char(&mut self, char_idx: usize, text: &str) {
        let len = self.rope.len_chars();
        let idx = char_idx.min(len);
        self.record_splice(idx, 0, text);
        self.rope.insert(idx, text);
        self.touch();
        self.redo_stack.clear();
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
            self.record_splice(start, end - start, "");
            self.rope.remove(start..end);
        } else if line > 0 {
            // Last line with no trailing newline — remove the newline before it
            let prev_end = self.rope.line_to_char(line);
            if prev_end > 0 {
                self.record_splice(prev_end - 1, 1, "");
                self.rope.remove(prev_end - 1..prev_end);
            }
        }
        self.touch();
        self.redo_stack.clear();
    }

    /// Replace the text of `line` (excluding its trailing newline) with `new_text`.
    pub fn replace_line_text(&mut self, line: usize, new_text: &str) {
        if line >= self.rope.len_lines() {
            return;
        }
        let start = self.rope.line_to_char(line);
        let line_slice = self.rope.line(line);
        let mut end_char = start + line_slice.len_chars();
        if line_slice.len_chars() > 0 && line_slice.char(line_slice.len_chars() - 1) == '\n' {
            end_char -= 1;
        }
        if end_char > start {
            self.record_splice(start, end_char - start, "");
            self.rope.remove(start..end_char);
        }
        self.record_splice(start, 0, new_text);
        self.rope.insert(start, new_text);
        self.touch();
        self.redo_stack.clear();
    }

    /// Begin an undo group. Call before a sequence of edits that should undo
    /// as one. `frozen_lines` and `lockable_through_line` are snapshotted so
    /// the editor's frozen-region state is restored on undo alongside the
    /// rope text — otherwise undo can desynchronize them, leaving stale
    /// indices that misclassify frozen vs. editable lines.
    pub fn begin_undo_group(
        &mut self,
        cursor_line: usize,
        cursor_col: usize,
        frozen_lines: &[(usize, usize)],
        lockable_through_line: usize,
    ) {
        self.pending_undo = Some(UndoEntry {
            splices: Vec::new(),
            cursor_before_line: cursor_line,
            cursor_before_col: cursor_col,
            cursor_after_line: 0,
            cursor_after_col: 0,
            frozen_lines_before: frozen_lines.to_vec(),
            lockable_through_line_before: lockable_through_line,
        });
    }

    /// End an undo group. Pushes it to the undo stack. A group that recorded no
    /// splices (no actual text change) is dropped, matching the old behavior
    /// where an identical before/after snapshot was a no-op on undo.
    pub fn end_undo_group(&mut self, cursor_line: usize, cursor_col: usize) {
        if let Some(mut entry) = self.pending_undo.take() {
            if entry.splices.is_empty() {
                return;
            }
            entry.cursor_after_line = cursor_line;
            entry.cursor_after_col = cursor_col;
            self.undo_stack.push(entry);
        }
    }

    /// Invert one group's splices in reverse application order, mutating the
    /// rope back to its pre-group state. Cost is O(sum of edit sizes), never
    /// O(document). Used by `undo`.
    fn apply_inverse(&mut self, entry: &UndoEntry) {
        for sp in entry.splices.iter().rev() {
            let rm_end = (sp.start + sp.inserted.chars().count()).min(self.rope.len_chars());
            let rm_start = sp.start.min(rm_end);
            if rm_start < rm_end {
                self.rope.remove(rm_start..rm_end);
            }
            if !sp.removed.is_empty() {
                let at = sp.start.min(self.rope.len_chars());
                self.rope.insert(at, &sp.removed);
            }
        }
    }

    /// Re-apply one group's splices in forward application order, mutating the
    /// rope back to its post-group state. Cost is O(sum of edit sizes). Used by
    /// `redo`.
    fn apply_forward(&mut self, entry: &UndoEntry) {
        for sp in entry.splices.iter() {
            let rm_end = (sp.start + sp.removed.chars().count()).min(self.rope.len_chars());
            let rm_start = sp.start.min(rm_end);
            if rm_start < rm_end {
                self.rope.remove(rm_start..rm_end);
            }
            if !sp.inserted.is_empty() {
                let at = sp.start.min(self.rope.len_chars());
                self.rope.insert(at, &sp.inserted);
            }
        }
    }

    /// Undo the last action. Returns the cursor position to restore, plus the
    /// frozen-line snapshot and lockable-through-line value to restore.
    pub fn undo(
        &mut self,
        current_frozen_lines: &[(usize, usize)],
        current_lockable_through_line: usize,
    ) -> Option<(usize, usize, Vec<(usize, usize)>, usize)> {
        let entry = self.undo_stack.pop()?;
        // Invert the group's splices to walk the rope back to its pre-group
        // state — O(edit), not O(document) (finding #4).
        self.apply_inverse(&entry);
        // Push a redo record. The same splices replay forward on redo; we only
        // swap in the editor's CURRENT (post-group) frozen state as the state a
        // future redo should restore, mirroring the old snapshot behavior.
        let redo_entry = UndoEntry {
            splices: entry.splices.clone(),
            cursor_before_line: entry.cursor_after_line,
            cursor_before_col: entry.cursor_after_col,
            cursor_after_line: entry.cursor_before_line,
            cursor_after_col: entry.cursor_before_col,
            frozen_lines_before: current_frozen_lines.to_vec(),
            lockable_through_line_before: current_lockable_through_line,
        };
        self.redo_stack.push(redo_entry);
        self.modified = !self.undo_stack.is_empty();
        self.edit_seq = self.edit_seq.wrapping_add(1);
        Some((
            entry.cursor_before_line,
            entry.cursor_before_col,
            entry.frozen_lines_before,
            entry.lockable_through_line_before,
        ))
    }

    /// Redo the last undone action. Returns cursor + frozen state to restore.
    pub fn redo(
        &mut self,
        current_frozen_lines: &[(usize, usize)],
        current_lockable_through_line: usize,
    ) -> Option<(usize, usize, Vec<(usize, usize)>, usize)> {
        let entry = self.redo_stack.pop()?;
        // Push the undo record *before* re-applying, capturing the current
        // (pre-group) frozen state so a later undo restores it — matching the
        // old snapshot ordering.
        let undo_entry = UndoEntry {
            splices: entry.splices.clone(),
            cursor_before_line: entry.cursor_after_line,
            cursor_before_col: entry.cursor_after_col,
            cursor_after_line: entry.cursor_before_line,
            cursor_after_col: entry.cursor_before_col,
            frozen_lines_before: current_frozen_lines.to_vec(),
            lockable_through_line_before: current_lockable_through_line,
        };
        self.undo_stack.push(undo_entry);
        // Replay the group's splices forward to the post-group rope — O(edit).
        self.apply_forward(&entry);
        self.modified = true;
        self.edit_seq = self.edit_seq.wrapping_add(1);
        Some((
            entry.cursor_before_line,
            entry.cursor_before_col,
            entry.frozen_lines_before,
            entry.lockable_through_line_before,
        ))
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

    /// Test-only: total bytes the top undo-stack entry retains (removed +
    /// inserted text across its splices). Guards finding #4: this must be
    /// O(edit), not O(document).
    #[cfg(test)]
    pub fn last_undo_entry_bytes(&self) -> Option<usize> {
        self.undo_stack.last().map(|e| {
            e.splices
                .iter()
                .map(|s| s.removed.len() + s.inserted.len())
                .sum()
        })
    }

    #[cfg(test)]
    pub fn undo_stack_len(&self) -> usize {
        self.undo_stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::from_text(text.to_string(), PathBuf::from("test.md"))
    }

    /// Finding #4: the undo record for a single-char insert into a large
    /// document must store O(edit) bytes, NOT a full-document snapshot.
    #[test]
    fn insert_undo_entry_is_o_edit_not_o_document() {
        // A big "frozen transcript" plus a tiny compose line, all one Document.
        let big = "lorem ipsum dolor sit amet\n".repeat(5000);
        let mut d = doc(&big);
        let n_bytes = d.len_bytes();
        assert!(n_bytes > 100_000, "fixture must be large: {n_bytes}");

        // Type one character inside an undo group (mirrors begin_insert).
        d.begin_undo_group(0, 0, &[], 0);
        d.insert_char(0, 0, 'X');
        d.end_undo_group(0, 1);

        let bytes = d.last_undo_entry_bytes().expect("one undo entry");
        // One inserted char, nothing removed.
        assert!(
            bytes <= 8,
            "undo entry must be O(edit) ({bytes} bytes), not O(document) ({n_bytes})"
        );
    }

    #[test]
    fn delete_range_undo_entry_is_o_edit() {
        let big = "abcdefghij\n".repeat(4000);
        let mut d = doc(&big);
        let n_bytes = d.len_bytes();
        d.begin_undo_group(0, 0, &[], 0);
        d.delete_range(0, 5); // remove "abcde"
        d.end_undo_group(0, 0);
        let bytes = d.last_undo_entry_bytes().expect("entry");
        assert!(bytes < 64, "delta should hold only the removed slice: {bytes}");
        assert!(n_bytes > 40_000);
    }

    #[test]
    fn undo_redo_roundtrip_insert() {
        let mut d = doc("hello\nworld\n");
        let before = d.full_text();
        d.begin_undo_group(0, 5, &[], 0);
        d.insert_str(0, 5, " there");
        d.end_undo_group(0, 11);
        let after = d.full_text();
        assert_eq!(after, "hello there\nworld\n");

        d.undo(&[], 0);
        assert_eq!(d.full_text(), before);
        d.redo(&[], 0);
        assert_eq!(d.full_text(), after);
        d.undo(&[], 0);
        assert_eq!(d.full_text(), before);
    }

    #[test]
    fn undo_redo_roundtrip_multi_op_group() {
        // A group with several mixed ops must undo/redo as one atomic step.
        let mut d = doc("one\ntwo\nthree\n");
        let before = d.full_text();
        d.begin_undo_group(0, 0, &[], 0);
        d.insert_str(0, 0, "ZERO\n"); // insert a line
        d.delete_line(3); // delete a line ("two" shifted to idx? recompute)
        d.replace_line_text(0, "AAAA"); // overwrite line 0
        d.end_undo_group(1, 0);
        let after = d.full_text();
        assert_ne!(after, before);

        d.undo(&[], 0);
        assert_eq!(d.full_text(), before, "multi-op group must fully revert");
        d.redo(&[], 0);
        assert_eq!(d.full_text(), after, "redo must replay the whole group");
    }

    #[test]
    fn undo_redo_roundtrip_unicode() {
        let mut d = doc("héllo 世界\n");
        let before = d.full_text();
        d.begin_undo_group(0, 0, &[], 0);
        d.insert_str(0, 0, "→★ "); // multibyte insert
        d.delete_char(0, 3);
        d.end_undo_group(0, 0);
        let after = d.full_text();
        d.undo(&[], 0);
        assert_eq!(d.full_text(), before);
        d.redo(&[], 0);
        assert_eq!(d.full_text(), after);
    }

    #[test]
    fn empty_group_is_dropped() {
        let mut d = doc("x\n");
        d.begin_undo_group(0, 0, &[], 0);
        // no edits
        d.end_undo_group(0, 0);
        assert_eq!(d.undo_stack_len(), 0, "no-op group must not push an entry");
    }

    #[test]
    fn frozen_state_restored_on_undo() {
        let mut d = doc("a\nb\nc\n");
        let frozen = vec![(0usize, 2usize)];
        d.begin_undo_group(0, 0, &frozen, 1);
        d.insert_str(2, 0, "X\n");
        d.end_undo_group(0, 0);
        // Undo with a *different* current frozen state; we should get the
        // snapshotted pre-group frozen back.
        let (_l, _c, restored_frozen, restored_lockable) = d.undo(&[(0, 5)], 3).unwrap();
        assert_eq!(restored_frozen, frozen);
        assert_eq!(restored_lockable, 1);
    }
}
