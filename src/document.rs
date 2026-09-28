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
/// A line-level shift an undo/redo applied to the rope, to be **replayed on the
/// editor's anchor store** so frozen-line metadata (TurnId / tool tags) tracks
/// the change instead of being reset. Fixes the worksheet "undo wiped the
/// gutter / relocated tool calls to the bottom" bug (worksheet-frozen-blocks
/// ticket 001 / C3): undo used to `reset_line_anchors`, dropping every tag;
/// these ops let the editor SHIFT the anchors (metadata is keyed by stable
/// anchor id, so it survives) exactly as a live insert/delete would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorShift {
    Insert { line: usize, col: usize, nl: usize },
    Delete { line: usize, col: usize, nl: usize },
}

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

/// B10: fold a new primitive splice into the previous one of the same undo
/// group when the two are one contiguous typing run, so typing N characters
/// records one `Splice` instead of N heap allocations. Returns true when merged.
///
/// Only newline-free pieces merge: a merged splice that spans a line break
/// would replay on undo as ONE multi-line `AnchorShift` instead of the
/// per-char sequence, which changes which line anchor survives. Keeping line
/// breaks as their own splices makes the undo anchor replay identical to the
/// uncoalesced history.
///
/// Merges:
/// - insert right after a pure insert (typing);
/// - delete immediately before a pure delete (a Backspace run);
/// - delete at the same start as a pure delete (a forward-Delete run);
/// - delete of the tail of a pure insert (Backspace over just-typed text).
fn coalesce_splice(last: &mut Splice, start: usize, removed: &str, inserted: &str) -> bool {
    if removed.contains('\n') || inserted.contains('\n') {
        return false;
    }
    if last.removed.contains('\n') || last.inserted.contains('\n') {
        return false;
    }
    let last_ins = last.inserted.chars().count();
    if removed.is_empty() && !inserted.is_empty() {
        if last.removed.is_empty() && start == last.start + last_ins {
            last.inserted.push_str(inserted);
            return true;
        }
        return false;
    }
    if !inserted.is_empty() || removed.is_empty() {
        return false;
    }
    let rem = removed.chars().count();
    if last.inserted.is_empty() && !last.removed.is_empty() {
        if start + rem == last.start {
            let mut joined = String::with_capacity(removed.len() + last.removed.len());
            joined.push_str(removed);
            joined.push_str(&last.removed);
            last.removed = joined;
            last.start = start;
            return true;
        }
        if start == last.start {
            last.removed.push_str(removed);
            return true;
        }
        return false;
    }
    if last.removed.is_empty() && start >= last.start && start + rem == last.start + last_ins {
        let keep = last_ins - rem;
        let cut = last
            .inserted
            .char_indices()
            .nth(keep)
            .map(|(b, _)| b)
            .unwrap_or(last.inserted.len());
        last.inserted.truncate(cut);
        return true;
    }
    false
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
    /// B6: undo-stack depth at which the buffer matches what is on disk (the
    /// save point). `None` once that state is unreachable through undo/redo
    /// (history diverged past it, or an unrecorded edit changed the text).
    /// Undo/redo recompute `modified` against it instead of assuming "empty
    /// undo stack == pristine".
    saved_depth: Option<usize>,
}

/// Which way an undo-history step walks (B16: one body for undo and redo).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryDir {
    Undo,
    Redo,
}

impl Document {
    pub fn from_text(text: String, file_path: PathBuf) -> Self {
        Self::from_text_with_edit_seq(text, file_path, 0)
    }

    /// Construct a whole-buffer replacement at a caller-owned generation.
    /// `EditorCore::replace_text` uses this to keep the generation monotonic
    /// across a reload instead of aliasing existing generation-keyed caches.
    pub(crate) fn from_text_with_edit_seq(
        text: String,
        file_path: PathBuf,
        edit_seq: u64,
    ) -> Self {
        Self {
            rope: Rope::from_str(&text),
            file_path,
            modified: false,
            edit_seq,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_undo: None,
            pending_edit: None,
            pending_splice_count: 0,
            saved_depth: Some(0),
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
            // An edit outside any undo group can't be walked back, so the save
            // point is no longer reachable through undo/redo (B6).
            self.saved_depth = None;
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
            if let Some(last) = entry.splices.last_mut()
                && coalesce_splice(last, s, &removed, inserted)
            {
                if last.removed.is_empty() && last.inserted.is_empty() {
                    entry.splices.pop();
                }
                return;
            }
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
        self.clear_redo();
    }

    pub fn delete_char(&mut self, line: usize, col: usize) {
        let char_idx = self.line_col_to_char(line, col);
        if char_idx < self.rope.len_chars() {
            self.record_splice(char_idx, 1, "");
            self.rope.remove(char_idx..char_idx + 1);
            self.touch();
            self.clear_redo();
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
            self.clear_redo();
        }
    }

    /// Insert a string at a (line, col) position.
    pub fn insert_str(&mut self, line: usize, col: usize, text: &str) {
        let char_idx = self.line_col_to_char(line, col);
        self.record_splice(char_idx, 0, text);
        self.rope.insert(char_idx, text);
        self.touch();
        self.clear_redo();
    }

    /// Insert a string at a rope char index. Used when splicing a precomputed
    /// region — see `app.rs::append_to_claude_buffer`.
    pub fn insert_str_at_char(&mut self, char_idx: usize, text: &str) {
        let len = self.rope.len_chars();
        let idx = char_idx.min(len);
        self.record_splice(idx, 0, text);
        self.rope.insert(idx, text);
        self.touch();
        self.clear_redo();
    }

    /// Insert `text` at `char_idx` WITHOUT recording it as a user-undoable edit.
    /// Agent-streamed / programmatic content must never be reachable by the
    /// user's `undo`: otherwise a chunk that streams while the user is mid-insert
    /// (one open undo group) gets folded into that group and a later undo wipes
    /// the whole transcript ("undo erased the buffer"). Already-recorded user
    /// splices at/after the insert are position-shifted so the user's own undo
    /// still targets the right characters despite the interleaved content. Does
    /// NOT clear the redo stack — agent content is orthogonal to user undo/redo.
    pub fn insert_str_at_char_no_undo(&mut self, char_idx: usize, text: &str) {
        let len = self.rope.len_chars();
        let idx = char_idx.min(len);
        self.note_pending_edit(idx, 0, text);
        self.rope.insert(idx, text);
        self.touch();
        self.saved_depth = None;
        self.shift_recorded_splices(idx, text.chars().count() as isize);
    }

    /// Delete `[start_char, end_char)` WITHOUT recording a user-undoable edit
    /// (programmatic/agent companion to `insert_str_at_char_no_undo`).
    pub fn delete_range_no_undo(&mut self, start_char: usize, end_char: usize) {
        let len = self.rope.len_chars();
        let s = start_char.min(len);
        let e = end_char.min(len);
        if s < e {
            self.note_pending_edit(s, e - s, "");
            self.rope.remove(s..e);
            self.touch();
            self.saved_depth = None;
            self.shift_recorded_splices(s, -((e - s) as isize));
        }
    }

    /// Position-shift every recorded user splice (the open group + both stacks)
    /// at or after `at` by `delta`, after a non-recorded programmatic splice
    /// changed the rope. Keeps user undo/redo targeting the right characters
    /// across interleaved agent content. Cheap: O(recorded user splices), which
    /// is small (a handful of user edits), not O(transcript).
    fn shift_recorded_splices(&mut self, at: usize, delta: isize) {
        let bump = |sp: &mut Splice| {
            if sp.start >= at {
                sp.start = (sp.start as isize + delta).max(0) as usize;
            }
        };
        if let Some(entry) = self.pending_undo.as_mut() {
            entry.splices.iter_mut().for_each(bump);
        }
        for entry in self.undo_stack.iter_mut().chain(self.redo_stack.iter_mut()) {
            entry.splices.iter_mut().for_each(bump);
        }
    }

    /// B4: the char range `dd` on `line` actually removes — the line plus its
    /// trailing newline, or, for a final line with no newline of its own, the
    /// PRECEDING newline (so the line disappears instead of leaving a blank).
    /// `None` when there is nothing to delete (single empty line, or out of
    /// range). Callers guard/shift exactly this range, so the frozen-line check
    /// and the rope edit can never disagree about what was removed.
    pub fn line_delete_range(&self, line: usize) -> Option<(usize, usize)> {
        let lines = self.rope.len_lines();
        if line >= lines {
            return None;
        }
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < lines {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        if start < end {
            Some((start, end))
        } else if start > 0 {
            Some((start - 1, start))
        } else {
            None
        }
    }

    pub fn delete_line(&mut self, line: usize) {
        if let Some((s, e)) = self.line_delete_range(line) {
            self.delete_range(s, e);
        }
    }

    /// Replace the text of `line` (excluding its trailing newline) with `new_text`.
    pub fn replace_line_text(&mut self, line: usize, new_text: &str) {
        if line >= self.rope.len_lines() {
            return;
        }
        let start = self.rope.line_to_char(line);
        let end_char = start + self.line_len_chars(line);
        if end_char > start {
            self.record_splice(start, end_char - start, "");
            self.rope.remove(start..end_char);
        }
        self.record_splice(start, 0, new_text);
        self.rope.insert(start, new_text);
        self.touch();
        self.clear_redo();
    }

    /// Begin an undo group. Call before a sequence of edits that should undo
    /// as one. `frozen_lines` and `lockable_through_line` are snapshotted so
    /// the editor's frozen-region state is restored on undo alongside the
    /// rope text — otherwise undo can desynchronize them, leaving stale
    /// indices that misclassify frozen vs. editable lines.
    ///
    /// B1: a group that is already open is NOT replaced — the call is a no-op
    /// and returns `false`, so an edit issued inside an insert session (e.g.
    /// Insert-mode Delete) joins that session instead of discarding its
    /// recorded splices. Returns `true` when this call opened the group; only
    /// that caller should `end_undo_group`.
    pub fn begin_undo_group(
        &mut self,
        cursor_line: usize,
        cursor_col: usize,
        frozen_lines: &[(usize, usize)],
        lockable_through_line: usize,
    ) -> bool {
        if self.pending_undo.is_some() {
            return false;
        }
        self.pending_undo = Some(UndoEntry {
            splices: Vec::new(),
            cursor_before_line: cursor_line,
            cursor_before_col: cursor_col,
            cursor_after_line: 0,
            cursor_after_col: 0,
            frozen_lines_before: frozen_lines.to_vec(),
            lockable_through_line_before: lockable_through_line,
        });
        true
    }

    /// True while an undo group is open (an insert session or a grouped edit).
    pub fn undo_group_open(&self) -> bool {
        self.pending_undo.is_some()
    }

    /// Drop the redo history after a new edit. If the save point lived in the
    /// discarded redo history it can no longer be reached (B6).
    fn clear_redo(&mut self) {
        if !self.redo_stack.is_empty()
            && self.saved_depth.is_some_and(|d| d > self.undo_stack.len())
        {
            self.saved_depth = None;
        }
        self.redo_stack.clear();
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

    // (AnchorShift defined at module scope below.)

    /// `(line, col)` of a char index in the current rope (clamped to the end).
    pub fn line_col_of_char(&self, char_idx: usize) -> (usize, usize) {
        let idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        (line, idx - self.rope.line_to_char(line))
    }

    fn count_nl(&self, s: usize, e: usize) -> usize {
        self.rope.slice(s..e).chars().filter(|c| *c == '\n').count()
    }

    /// Walk one group's splices through the rope (B16: the single body behind
    /// undo AND redo). `Undo` inverts them in reverse application order
    /// (remove what was inserted, restore what was removed); `Redo` re-applies
    /// them forward. Cost is O(sum of edit sizes), never O(document). Returns
    /// the line-level [`AnchorShift`]s the caller must replay on its anchor
    /// store so frozen-line metadata (TurnId/tool tags) tracks the change
    /// instead of being reset (C3).
    fn apply_splices(&mut self, entry: &UndoEntry, dir: HistoryDir) -> Vec<AnchorShift> {
        let mut ops = Vec::new();
        let mut step = |doc: &mut Self, sp: &Splice| {
            let (take_out, put_in) = match dir {
                HistoryDir::Undo => (&sp.inserted, &sp.removed),
                HistoryDir::Redo => (&sp.removed, &sp.inserted),
            };
            let rm_end = (sp.start + take_out.chars().count()).min(doc.rope.len_chars());
            let rm_start = sp.start.min(rm_end);
            if rm_start < rm_end {
                let (line, col) = doc.line_col_of_char(rm_start);
                let nl = doc.count_nl(rm_start, rm_end);
                doc.rope.remove(rm_start..rm_end);
                ops.push(AnchorShift::Delete { line, col, nl });
            }
            if !put_in.is_empty() {
                let at = sp.start.min(doc.rope.len_chars());
                let (line, col) = doc.line_col_of_char(at);
                let nl = put_in.chars().filter(|c| *c == '\n').count();
                doc.rope.insert(at, put_in);
                ops.push(AnchorShift::Insert { line, col, nl });
            }
        };
        match dir {
            HistoryDir::Undo => entry.splices.iter().rev().for_each(|sp| step(self, sp)),
            HistoryDir::Redo => entry.splices.iter().for_each(|sp| step(self, sp)),
        }
        ops
    }

    /// One undo-history step in `dir` (B16). Pops the source stack, pushes the
    /// mirror record (cursor before/after swapped, the editor's CURRENT frozen
    /// state captured for the step back) onto the other stack, walks the rope,
    /// and recomputes `modified` against the save point (B6).
    #[allow(clippy::type_complexity)]
    fn history_step(
        &mut self,
        dir: HistoryDir,
        current_frozen_lines: &[(usize, usize)],
        current_lockable_through_line: usize,
    ) -> Option<(usize, usize, Vec<(usize, usize)>, usize, Vec<AnchorShift>)> {
        let entry = match dir {
            HistoryDir::Undo => self.undo_stack.pop()?,
            HistoryDir::Redo => self.redo_stack.pop()?,
        };
        let mirror = UndoEntry {
            splices: entry.splices.clone(),
            cursor_before_line: entry.cursor_after_line,
            cursor_before_col: entry.cursor_after_col,
            cursor_after_line: entry.cursor_before_line,
            cursor_after_col: entry.cursor_before_col,
            frozen_lines_before: current_frozen_lines.to_vec(),
            lockable_through_line_before: current_lockable_through_line,
        };
        match dir {
            HistoryDir::Undo => self.redo_stack.push(mirror),
            HistoryDir::Redo => self.undo_stack.push(mirror),
        }
        let shifts = self.apply_splices(&entry, dir);
        self.modified = self.saved_depth != Some(self.undo_stack.len());
        self.edit_seq = self.edit_seq.wrapping_add(1);
        Some((
            entry.cursor_before_line,
            entry.cursor_before_col,
            entry.frozen_lines_before,
            entry.lockable_through_line_before,
            shifts,
        ))
    }

    /// Undo the last action. Returns the cursor position to restore, plus the
    /// frozen-line snapshot and lockable-through-line value to restore.
    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    pub fn undo(
        &mut self,
        current_frozen_lines: &[(usize, usize)],
        current_lockable_through_line: usize,
    ) -> Option<(usize, usize, Vec<(usize, usize)>, usize, Vec<AnchorShift>)> {
        self.history_step(HistoryDir::Undo, current_frozen_lines, current_lockable_through_line)
    }

    /// Redo the last undone action. Returns cursor + frozen state to restore.
    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    pub fn redo(
        &mut self,
        current_frozen_lines: &[(usize, usize)],
        current_lockable_through_line: usize,
    ) -> Option<(usize, usize, Vec<(usize, usize)>, usize, Vec<AnchorShift>)> {
        self.history_step(HistoryDir::Redo, current_frozen_lines, current_lockable_through_line)
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
        // B6: remember where on the undo stack the on-disk state lives. With an
        // open group that already recorded splices the save point sits between
        // stack entries, so no undo/redo depth reproduces it exactly.
        let mid_group = self
            .pending_undo
            .as_ref()
            .is_some_and(|p| !p.splices.is_empty());
        self.saved_depth = if mid_group {
            None
        } else {
            Some(self.undo_stack.len())
        };
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

    /// Test-only: number of primitive splices recorded in the top undo entry
    /// (B10 coalescing observable).
    #[cfg(test)]
    pub fn last_undo_splice_count(&self) -> Option<usize> {
        self.undo_stack.last().map(|e| e.splices.len())
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
        assert!(
            bytes < 64,
            "delta should hold only the removed slice: {bytes}"
        );
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
        let (_l, _c, restored_frozen, restored_lockable, _shifts) = d.undo(&[(0, 5)], 3).unwrap();
        assert_eq!(restored_frozen, frozen);
        assert_eq!(restored_lockable, 1);
    }

    /// B10: typing a run of characters in one insert session records ONE
    /// coalesced splice, not one heap `Splice` per keystroke — and it still
    /// undoes/redoes exactly.
    #[test]
    fn typed_run_coalesces_into_one_splice() {
        let mut d = doc("hello\n");
        d.begin_undo_group(0, 5, &[], 0);
        for (i, ch) in " world".chars().enumerate() {
            d.insert_char(0, 5 + i, ch);
        }
        d.end_undo_group(0, 11);
        assert_eq!(d.full_text(), "hello world\n");
        assert_eq!(d.last_undo_splice_count(), Some(1), "typed run must coalesce");
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "hello\n");
        d.redo(&[], 0);
        assert_eq!(d.full_text(), "hello world\n");
    }

    /// B10: a Backspace run, a forward-Delete run, and Backspace over
    /// just-typed text each coalesce, and the group still round-trips.
    #[test]
    fn delete_runs_coalesce_and_round_trip() {
        let mut d = doc("abcdefgh\n");
        // Backspace run: delete h, g, f (each one before the previous).
        d.begin_undo_group(0, 8, &[], 0);
        d.delete_char(0, 7);
        d.delete_char(0, 6);
        d.delete_char(0, 5);
        d.end_undo_group(0, 5);
        assert_eq!(d.full_text(), "abcde\n");
        assert_eq!(d.last_undo_splice_count(), Some(1), "backspace run coalesces");
        // Forward-Delete run at the same position.
        d.begin_undo_group(0, 0, &[], 0);
        d.delete_char(0, 0);
        d.delete_char(0, 0);
        d.end_undo_group(0, 0);
        assert_eq!(d.full_text(), "cde\n");
        assert_eq!(d.last_undo_splice_count(), Some(1), "delete run coalesces");
        // Type then backspace part of it: net one splice.
        d.begin_undo_group(0, 3, &[], 0);
        d.insert_char(0, 3, 'x');
        d.insert_char(0, 4, 'y');
        d.insert_char(0, 5, 'z');
        d.delete_char(0, 5);
        d.end_undo_group(0, 5);
        assert_eq!(d.full_text(), "cdexy\n");
        assert_eq!(d.last_undo_splice_count(), Some(1));
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "cde\n");
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "abcde\n");
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "abcdefgh\n");
        d.redo(&[], 0);
        d.redo(&[], 0);
        d.redo(&[], 0);
        assert_eq!(d.full_text(), "cdexy\n");
    }

    /// B10: line breaks are never folded into a neighbouring splice (so the
    /// undo anchor replay stays per-line, identical to the uncoalesced history).
    #[test]
    fn newlines_stay_separate_splices() {
        let mut d = doc("\n");
        d.begin_undo_group(0, 0, &[], 0);
        d.insert_char(0, 0, 'a');
        d.insert_char(0, 1, '\n');
        d.insert_char(1, 0, 'b');
        d.end_undo_group(1, 1);
        assert_eq!(d.last_undo_splice_count(), Some(3));
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "\n");
    }

    /// B1: opening a group while one is already open is a no-op — the open
    /// group (an insert session) keeps its recorded splices.
    #[test]
    fn nested_begin_undo_group_does_not_clobber_open_group() {
        let mut d = doc("abc\n");
        assert!(d.begin_undo_group(0, 0, &[], 0));
        d.insert_char(0, 0, 'X');
        assert!(!d.begin_undo_group(0, 1, &[], 0), "nested begin must not open");
        d.delete_char(0, 1);
        d.end_undo_group(0, 1);
        assert_eq!(d.full_text(), "Xbc\n");
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "abc\n", "one undo reverts the whole session");
    }

    fn save_tmp(d: &mut Document, dir: &tempfile::TempDir) {
        d.save_to(&dir.path().join("f.md")).expect("save");
    }

    /// B6: undoing back to the save point clears `modified`; undoing past it
    /// (or redoing away from it) sets it.
    #[test]
    fn modified_tracks_the_save_point_not_an_empty_undo_stack() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc("a\n");
        d.begin_undo_group(0, 1, &[], 0);
        d.insert_char(0, 1, 'b');
        d.end_undo_group(0, 2);
        save_tmp(&mut d, &dir);
        assert!(!d.is_modified());
        d.begin_undo_group(0, 2, &[], 0);
        d.insert_char(0, 2, 'c');
        d.end_undo_group(0, 3);
        assert!(d.is_modified());
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "ab\n");
        assert!(!d.is_modified(), "back at the saved text: not modified");
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "a\n");
        assert!(d.is_modified(), "past the save point: modified");
        d.redo(&[], 0);
        assert!(!d.is_modified(), "redo back onto the save point");
        d.redo(&[], 0);
        assert!(d.is_modified());
    }

    /// B6: once the save point is discarded from the redo history it can no
    /// longer be reached — the buffer stays modified.
    #[test]
    fn save_point_lost_after_diverging_edit() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc("a\n");
        d.begin_undo_group(0, 1, &[], 0);
        d.insert_char(0, 1, 'b');
        d.end_undo_group(0, 2);
        save_tmp(&mut d, &dir);
        d.undo(&[], 0); // "a" — save point now in redo history
        d.begin_undo_group(0, 1, &[], 0);
        d.insert_char(0, 1, 'z'); // diverge: redo cleared
        d.end_undo_group(0, 2);
        d.undo(&[], 0);
        assert_eq!(d.full_text(), "a\n");
        assert!(d.is_modified(), "'a' was never saved");
    }
}
