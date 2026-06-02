use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::path::PathBuf;

use crate::cursor::CursorPos;
use crate::document::Document;
use crate::tree::{BlockInfo, TreeState};

// =============================================================================
// LineAnchor + LineMetadata
// =============================================================================
//
// Opaque, monotonic line ids that survive inserts/deletes on *other* lines.
// Backed by a side map kept in sync by `shift_anchors_for_*` whenever the same
// edit paths shift `frozen_lines`. Anchors whose line is wholly consumed by a
// delete are dropped from the map; subsequent `line_for_anchor` calls return
// `None`. See spec-agent-window.md §E1.

#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Debug)]
pub struct LineAnchor(u64);

#[derive(Default)]
struct LineAnchorStore {
    next_id: u64,
    by_anchor: BTreeMap<LineAnchor, usize>,
    by_line: BTreeMap<usize, LineAnchor>,
}

impl LineAnchorStore {
    fn allocate(&mut self, line: usize) -> LineAnchor {
        if let Some(&a) = self.by_line.get(&line) {
            return a;
        }
        let a = LineAnchor(self.next_id);
        self.next_id += 1;
        self.by_anchor.insert(a, line);
        self.by_line.insert(line, a);
        a
    }

    fn line_for(&self, a: LineAnchor) -> Option<usize> {
        self.by_anchor.get(&a).copied()
    }

    fn shift_for_insert(&mut self, eff_line: usize, eff_col: usize, inserted_nl: usize) {
        if inserted_nl == 0 {
            return;
        }
        let mut new_by_anchor: BTreeMap<LineAnchor, usize> = BTreeMap::new();
        let mut new_by_line: BTreeMap<usize, LineAnchor> = BTreeMap::new();
        for (&a, &line) in self.by_anchor.iter() {
            let new_line = if line < eff_line {
                line
            } else if line == eff_line {
                if eff_col == 0 { line + inserted_nl } else { line }
            } else {
                line + inserted_nl
            };
            new_by_anchor.insert(a, new_line);
            new_by_line.insert(new_line, a);
        }
        self.by_anchor = new_by_anchor;
        self.by_line = new_by_line;
    }

    /// Shift anchors for a delete that started at `(start_line, start_col)`
    /// and removed `deleted_nl` newlines. Returns the set of anchors that
    /// were dropped (so the metadata store can purge them).
    ///
    /// - Lines `< start_line` are unaffected.
    /// - Line `start_line` survives if `start_col > 0` (its prefix remains);
    ///   if `start_col == 0` and `deleted_nl > 0` it is wholly consumed by
    ///   the merge and its anchor is dropped.
    /// - Lines `(start_line, start_line + deleted_nl]` are wholly consumed
    ///   and their anchors are dropped.
    /// - Lines `> start_line + deleted_nl` shift down by `deleted_nl`.
    fn shift_for_delete(
        &mut self,
        start_line: usize,
        start_col: usize,
        deleted_nl: usize,
    ) -> Vec<LineAnchor> {
        if deleted_nl == 0 {
            return Vec::new();
        }
        let start_line_consumed = start_col == 0;
        let mut dropped = Vec::new();
        let mut new_by_anchor: BTreeMap<LineAnchor, usize> = BTreeMap::new();
        let mut new_by_line: BTreeMap<usize, LineAnchor> = BTreeMap::new();
        for (&a, &line) in self.by_anchor.iter() {
            if line < start_line {
                new_by_anchor.insert(a, line);
                new_by_line.insert(line, a);
            } else if line == start_line {
                if start_line_consumed {
                    dropped.push(a);
                } else {
                    new_by_anchor.insert(a, line);
                    new_by_line.insert(line, a);
                }
            } else if line <= start_line + deleted_nl {
                dropped.push(a);
            } else {
                let nl = line - deleted_nl;
                new_by_anchor.insert(a, nl);
                new_by_line.insert(nl, a);
            }
        }
        self.by_anchor = new_by_anchor;
        self.by_line = new_by_line;
        dropped
    }
}

/// Typed sparse map from `LineAnchor` to a per-type payload. One slot per
/// `T` registered with the editor; reads return `None` when the anchor has no
/// metadata of that type, or when the anchor has been dropped by a delete.
/// See spec-agent-window.md §E2.
#[derive(Default)]
struct LineMetadataStore {
    by_type: HashMap<TypeId, HashMap<LineAnchor, Box<dyn Any + Send + Sync>>>,
}

impl LineMetadataStore {
    fn drop_anchor(&mut self, a: LineAnchor) {
        for map in self.by_type.values_mut() {
            map.remove(&a);
        }
    }
}

pub struct LineMetadataView<'a, T: Any + Send + Sync> {
    map: Option<&'a HashMap<LineAnchor, Box<dyn Any + Send + Sync>>>,
    _phantom: PhantomData<T>,
}

impl<'a, T: Any + Send + Sync> LineMetadataView<'a, T> {
    pub fn get(&self, a: LineAnchor) -> Option<&T> {
        let map = self.map?;
        map.get(&a)?.downcast_ref::<T>()
    }
}

pub struct LineMetadataMut<'a, T: Any + Send + Sync> {
    map: &'a mut HashMap<LineAnchor, Box<dyn Any + Send + Sync>>,
    _phantom: PhantomData<T>,
}

impl<'a, T: Any + Send + Sync> LineMetadataMut<'a, T> {
    pub fn get(&self, a: LineAnchor) -> Option<&T> {
        self.map.get(&a)?.downcast_ref::<T>()
    }

    pub fn insert(&mut self, a: LineAnchor, v: T) {
        self.map.insert(a, Box::new(v));
    }

    pub fn remove(&mut self, a: LineAnchor) {
        self.map.remove(&a);
    }
}

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
    /// Opaque, monotonic line ids that survive inserts/deletes on other lines
    /// (§E1). Side map maintained in lock-step with `frozen_lines` by the
    /// `shift_*` paths. Anchors for lines wholly consumed by a delete are
    /// dropped.
    line_anchors: LineAnchorStore,
    /// Typed sparse map from `LineAnchor` to per-type payloads (§E2). The
    /// Worksheet gutter reads `TurnId` via this store keyed by line anchors.
    line_metadata: LineMetadataStore,
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
            line_anchors: LineAnchorStore::default(),
            line_metadata: LineMetadataStore::default(),
        }
    }

    // --- LineAnchor + LineMetadata (§E1, §E2) ---

    /// Allocate (or return the existing) anchor for `line`.
    pub fn anchor_for_line(&mut self, line: usize) -> LineAnchor {
        self.line_anchors.allocate(line)
    }

    /// `None` once the anchored line is gone (consumed by a delete).
    pub fn line_for_anchor(&self, a: LineAnchor) -> Option<usize> {
        self.line_anchors.line_for(a)
    }

    /// Read-only counterpart to `anchor_for_line`: returns the existing
    /// anchor for `line` without allocating. Useful for the render path
    /// (no `&mut`) — anchors not yet allocated still produce `None` and
    /// the caller treats those lines as "no metadata yet".
    pub fn anchor_for_line_opt(&self, line: usize) -> Option<LineAnchor> {
        self.line_anchors.by_line.get(&line).copied()
    }

    /// Read-only handle to per-line metadata of type `T`. Returns a view with
    /// `.get(anchor)`; missing entries yield `None`.
    pub fn metadata<T: Any + Send + Sync>(&self) -> LineMetadataView<'_, T> {
        LineMetadataView {
            map: self.line_metadata.by_type.get(&TypeId::of::<T>()),
            _phantom: PhantomData,
        }
    }

    /// Mutable handle to per-line metadata of type `T`. The underlying slot is
    /// created on demand. Use `.insert(anchor, v)` / `.remove(anchor)`.
    pub fn metadata_mut<T: Any + Send + Sync>(&mut self) -> LineMetadataMut<'_, T> {
        let map = self
            .line_metadata
            .by_type
            .entry(TypeId::of::<T>())
            .or_default();
        LineMetadataMut {
            map,
            _phantom: PhantomData,
        }
    }

    /// Walk anchors in descending line order, returning the highest line whose
    /// `T` metadata equals `tag`. Used by `append_llm_chunk` to find the tail
    /// of an in-progress LLM turn.
    fn last_line_with_meta<T: Any + Send + Sync + PartialEq>(
        &self,
        tag: &T,
    ) -> Option<usize> {
        let view = self.metadata::<T>();
        for (&line, &anchor) in self.line_anchors.by_line.iter().rev() {
            if let Some(v) = view.get(anchor) {
                if v == tag {
                    return Some(line);
                }
            }
        }
        None
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

    /// Drop every allocated anchor and all `LineMetadata`. Used by undo/redo,
    /// which bulk-restore the rope and frozen ranges without going through the
    /// shift machinery; the anchor store would otherwise be left referencing
    /// stale line indices. Consumers must re-acquire anchors after this fires.
    pub fn reset_line_anchors(&mut self) {
        self.line_anchors = LineAnchorStore::default();
        self.line_metadata = LineMetadataStore::default();
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

        self.line_anchors
            .shift_for_insert(eff_line, eff_col, inserted_nl);
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
        let (start_line, start_col) = char_to_line_col(&self.document, del_s);
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

        let dropped = self
            .line_anchors
            .shift_for_delete(start_line, start_col, deleted_nl);
        for a in dropped {
            self.line_metadata.drop_anchor(a);
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
            core.reset_line_anchors();
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
            core.reset_line_anchors();
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

    // --- LineAnchor / LineMetadata (delegate to core) ---

    pub fn anchor_for_line(&mut self, line: usize) -> LineAnchor {
        self.core.anchor_for_line(line)
    }

    pub fn line_for_anchor(&self, a: LineAnchor) -> Option<usize> {
        self.core.line_for_anchor(a)
    }

    pub fn anchor_for_line_opt(&self, line: usize) -> Option<LineAnchor> {
        self.core.anchor_for_line_opt(line)
    }

    pub fn metadata<T: Any + Send + Sync>(&self) -> LineMetadataView<'_, T> {
        self.core.metadata::<T>()
    }

    pub fn metadata_mut<T: Any + Send + Sync>(&mut self) -> LineMetadataMut<'_, T> {
        self.core.metadata_mut::<T>()
    }

    pub fn reset_line_anchors(&mut self) {
        self.core.reset_line_anchors();
    }

    /// Append an LLM chunk for `turn_tag` (typically a `TurnId::Llm(k)`
    /// payload). Locates the insertion point as the end of the last frozen
    /// line whose metadata of type `T` equals `turn_tag` (mid-line if that
    /// line didn't end with `\n`), or EOF if no line carries this turn yet.
    /// Inserts the chunk via `programmatic_insert`, extends the frozen range
    /// to cover the newly-inserted lines, and tags each new line's anchor
    /// with `turn_tag`. Editable user lines anywhere else in the document are
    /// not touched. See spec-agent-window.md §E3.
    pub fn append_llm_chunk<T>(&mut self, turn_tag: T, chunk: &str)
    where
        T: Any + Send + Sync + Clone + PartialEq,
    {
        if chunk.is_empty() {
            return;
        }
        let insertion_char = self.find_llm_insertion_point::<T>(&turn_tag);
        self.core.programmatic_insert(insertion_char, chunk);

        let chunk_chars = chunk.chars().count();
        let chunk_end_char = insertion_char + chunk_chars;
        let doc = self.core.document();
        let start_line = char_to_line_col(doc, insertion_char).0;
        let mut end_line = char_to_line_col(doc, chunk_end_char).0;
        if !chunk.ends_with('\n') {
            end_line += 1;
        }
        self.core.add_frozen_lines(start_line, end_line);

        for l in start_line..end_line {
            let a = self.core.anchor_for_line(l);
            self.core.metadata_mut::<T>().insert(a, turn_tag.clone());
        }
    }

    fn find_llm_insertion_point<T: Any + Send + Sync + PartialEq>(
        &self,
        turn_tag: &T,
    ) -> usize {
        let doc = self.core.document();
        let total_chars = doc.rope().len_chars();
        let total_lines = doc.line_count();

        let Some(last_llm_line) = self.core.last_line_with_meta::<T>(turn_tag) else {
            return total_chars;
        };

        let line_text = doc.line_text(last_llm_line);
        if line_text.ends_with('\n') {
            let next = last_llm_line + 1;
            if next >= total_lines {
                total_chars
            } else if self.line_tagged_other_turn::<T>(next, turn_tag) {
                // The line immediately after our last tagged line belongs to a
                // *different* turn — e.g. a tool-call block anchored on its own
                // line between two stretches of this turn's prose. Don't splice
                // into it (that interleaves the tool line with our text and
                // corrupts both); append on a fresh line at EOF instead.
                total_chars
            } else {
                doc.line_col_to_char(next, 0)
            }
        } else {
            let line_len = doc.line_len_chars(last_llm_line);
            doc.line_col_to_char(last_llm_line, line_len)
        }
    }

    /// True if `line` carries a metadata tag of type `T` that differs from
    /// `turn_tag`. Untagged lines (e.g. the empty trailing line) return false,
    /// preserving the normal same-turn continuation path.
    fn line_tagged_other_turn<T: Any + Send + Sync + PartialEq>(
        &self,
        line: usize,
        turn_tag: &T,
    ) -> bool {
        self.core
            .anchor_for_line_opt(line)
            .and_then(|a| self.core.metadata::<T>().get(a).map(|v| v != turn_tag))
            .unwrap_or(false)
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

// =============================================================================
// Tests — LineAnchor / LineMetadata / append_llm_chunk (§E1–§E3)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TurnId {
        Llm(usize),
        User(usize),
        Tool(usize),
    }

    /// Mimic `main.rs::anchor_for_new_tool_call` + its `Tool(k)` re-tag: a
    /// tool block lands on its own dedicated blank line tagged with a turn
    /// distinct from the surrounding `Llm` prose.
    fn simulate_tool_call(ed: &mut Editor, turn: usize) {
        if !ed.document().full_text().is_empty()
            && !ed.document().full_text().ends_with('\n')
        {
            let len = ed.document().rope().len_chars();
            ed.programmatic_insert(len, "\n");
        }
        let len = ed.document().rope().len_chars();
        ed.programmatic_insert(len, "\n");
        let tool_line = ed.document().line_count().saturating_sub(2);
        let anchor = ed.anchor_for_line(tool_line);
        ed.metadata_mut::<TurnId>().insert(anchor, TurnId::Tool(turn));
    }

    #[test]
    fn post_tool_chunk_does_not_clobber_pre_tool_line() {
        // Regression: streamed prose, a tool call, then more prose in the
        // same turn. The post-tool chunk must start a fresh line after the
        // tool block — not splice into an earlier Llm line (the "ThereLet" /
        // "Found key line" garble).
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "The key line is here.");
        simulate_tool_call(&mut ed, 1);
        ed.append_llm_chunk(TurnId::Llm(1), "Found it elsewhere.");

        let text = ed.document().full_text();
        assert!(text.contains("The key line is here."), "pre-tool intact: {text:?}");
        assert!(text.contains("Found it elsewhere."), "post-tool present: {text:?}");
        // No splice/merge of the two stretches.
        assert!(
            !text.contains("The key line is here.Found")
                && !text.contains("FoundThe")
                && !text.contains("hereFound"),
            "post-tool text must not merge into the pre-tool line: {text:?}"
        );
        let pre = text.lines().position(|l| l.contains("key line")).unwrap();
        let post = text.lines().position(|l| l.contains("Found it")).unwrap();
        assert!(post > pre, "post-tool prose must come after pre-tool: {text:?}");

        // Collect per-line tags (immutable borrow snapshot).
        let tags: Vec<Option<TurnId>> = (0..ed.document().line_count())
            .map(|l| {
                ed.anchor_for_line_opt(l)
                    .and_then(|a| ed.metadata::<TurnId>().get(a).copied())
            })
            .collect();
        let tool_line = tags
            .iter()
            .position(|t| matches!(t, Some(TurnId::Tool(1))))
            .expect("tool line should be tagged Tool(1)");
        // The discriminating checks: post-tool prose lands on its OWN line
        // (tagged Llm(1)), strictly after the tool line — not spliced onto the
        // tool's line. Reverting the find_llm_insertion_point skip fails here.
        assert_ne!(post, tool_line, "post-tool prose landed on the tool line: {text:?}");
        assert!(pre < tool_line && tool_line < post, "expected pre < tool < post: {text:?}");
        assert_eq!(tags[post], Some(TurnId::Llm(1)), "post-tool line keeps Llm(1): {text:?}");
        assert_eq!(tags[pre], Some(TurnId::Llm(1)), "pre-tool line keeps Llm(1): {text:?}");
    }

    fn new_editor(text: &str) -> Editor {
        Editor::new(text.to_string(), PathBuf::from("test.md"))
    }

    #[test]
    fn anchor_for_line_returns_same_id_on_repeat() {
        let mut ed = new_editor("a\nb\nc\n");
        let a0 = ed.anchor_for_line(1);
        let a1 = ed.anchor_for_line(1);
        assert_eq!(a0, a1);
        assert_eq!(ed.line_for_anchor(a0), Some(1));
    }

    #[test]
    fn anchor_distinct_per_line() {
        let mut ed = new_editor("a\nb\nc\n");
        let a0 = ed.anchor_for_line(0);
        let a1 = ed.anchor_for_line(1);
        let a2 = ed.anchor_for_line(2);
        assert_ne!(a0, a1);
        assert_ne!(a1, a2);
        assert_ne!(a0, a2);
    }

    #[test]
    fn anchor_shifts_when_inserts_above_at_col_zero() {
        let mut ed = new_editor("a\nb\nc\n");
        let a = ed.anchor_for_line(2); // "c"
        // Insert a new line at start of line 1 ("b"). col==0, one newline.
        ed.programmatic_insert(2, "X\n");
        // Document is now: a\nX\nb\nc\n; anchor for original "c" → line 3.
        assert_eq!(ed.line_for_anchor(a), Some(3));
    }

    #[test]
    fn anchor_does_not_shift_for_inserts_below() {
        let mut ed = new_editor("a\nb\nc\n");
        let a = ed.anchor_for_line(0);
        ed.programmatic_insert(ed.document().rope().len_chars(), "d\n");
        assert_eq!(ed.line_for_anchor(a), Some(0));
    }

    #[test]
    fn anchor_dropped_when_line_consumed_by_delete() {
        let mut ed = new_editor("a\nb\nc\nd\n");
        let a_b = ed.anchor_for_line(1);
        let a_c = ed.anchor_for_line(2);
        let a_d = ed.anchor_for_line(3);
        // Delete "b\nc\n" — del_s=2 (col 0 of line 1), del_e=6 (col 0 of
        // line 3), deleted_nl=2. Because del_s is at col 0 of start_line,
        // original line 1 ("b") is wholly consumed along with lines 2 ("c")
        // and 3 ("d"). After delete the rope is "a\nd\n"; the surviving
        // line 1 is the former "d", but with a fresh identity (no anchor).
        ed.programmatic_delete(2, 6);
        assert_eq!(ed.line_for_anchor(a_b), None);
        assert_eq!(ed.line_for_anchor(a_c), None);
        assert_eq!(ed.line_for_anchor(a_d), None);
        // A new anchor on the surviving line gets a fresh id.
        let fresh = ed.anchor_for_line(1);
        assert_ne!(fresh, a_b);
        assert_eq!(ed.line_for_anchor(fresh), Some(1));
    }

    #[test]
    fn anchor_preserved_when_delete_starts_mid_line() {
        // Mid-line delete: line at start_line keeps its prefix and absorbs the
        // tail of the deleted range. Anchor on start_line stays put.
        let mut ed = new_editor("hello\nworld\n!\n");
        let a0 = ed.anchor_for_line(0);
        // del_s=3 (mid-"hello", col 3), del_e=7 (mid-"world", col 1).
        // deleted_nl=1. start_line=0 survives; line 1 ("world") is consumed.
        ed.programmatic_delete(3, 7);
        assert_eq!(ed.line_for_anchor(a0), Some(0));
        // Surviving doc: "hel" + "orld\n" + "!\n" = "helorld\n!\n"
        assert_eq!(ed.document().full_text(), "helorld\n!\n");
    }

    #[test]
    fn metadata_get_after_insert_returns_value() {
        let mut ed = new_editor("hello\n");
        let a = ed.anchor_for_line(0);
        ed.metadata_mut::<TurnId>().insert(a, TurnId::User(3));
        assert_eq!(ed.metadata::<TurnId>().get(a), Some(&TurnId::User(3)));
    }

    #[test]
    fn metadata_dropped_when_anchor_dropped() {
        let mut ed = new_editor("a\nb\nc\n");
        let a = ed.anchor_for_line(1);
        ed.metadata_mut::<TurnId>().insert(a, TurnId::Llm(1));
        ed.programmatic_delete(2, 4); // delete "b\n"
        assert_eq!(ed.line_for_anchor(a), None);
        assert_eq!(ed.metadata::<TurnId>().get(a), None);
    }

    #[test]
    fn append_llm_chunk_to_empty_editor_appends_and_freezes() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "Hello, world!\n");
        assert_eq!(ed.document().full_text(), "Hello, world!\n");
        assert!(!ed.frozen_lines().is_empty());
        // Line 0 has the chunk; should be tagged Llm(1).
        let a = ed.anchor_for_line(0);
        assert_eq!(ed.metadata::<TurnId>().get(a), Some(&TurnId::Llm(1)));
    }

    #[test]
    fn append_llm_chunk_continues_mid_line_within_same_turn() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "Hello, ");
        ed.append_llm_chunk(TurnId::Llm(1), "world!\n");
        // Two chunks for the same turn should join into one line.
        assert_eq!(ed.document().full_text(), "Hello, world!\n");
    }

    #[test]
    fn append_llm_chunk_starts_new_line_for_new_turn() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "first turn\n");
        ed.append_llm_chunk(TurnId::Llm(2), "second turn\n");
        assert_eq!(
            ed.document().full_text(),
            "first turn\nsecond turn\n"
        );
        let a0 = ed.anchor_for_line(0);
        let a1 = ed.anchor_for_line(1);
        assert_eq!(ed.metadata::<TurnId>().get(a0), Some(&TurnId::Llm(1)));
        assert_eq!(ed.metadata::<TurnId>().get(a1), Some(&TurnId::Llm(2)));
    }

    #[test]
    fn append_llm_chunk_preserves_editable_draft_below() {
        // Simulate: turn 1's LLM line is frozen, user has typed a draft after.
        let mut ed = new_editor("Hi from agent.\nuser draft here\n");
        ed.add_frozen_lines(0, 1);
        let a0 = ed.anchor_for_line(0);
        ed.metadata_mut::<TurnId>().insert(a0, TurnId::Llm(1));
        // New turn 2 chunk arrives. Insertion point should be at start of
        // line 1 (after the last Llm(1) line, which has a trailing newline).
        // But wait — turn 2 is a new turn so insertion is at EOF, not line 1.
        ed.append_llm_chunk(TurnId::Llm(2), "Reply!\n");
        assert_eq!(
            ed.document().full_text(),
            "Hi from agent.\nuser draft here\nReply!\n"
        );
        // User draft on line 1 should still be there.
        assert_eq!(ed.document().line_text(1), "user draft here\n");
    }

    #[test]
    fn append_llm_chunk_within_same_turn_inserts_above_draft() {
        // Same setup but the chunk belongs to turn 1 (continuation), so it
        // should insert at end of line 0 (last Llm(1) line, which ends \n →
        // insertion at line 1 col 0), pushing the draft down.
        let mut ed = new_editor("Hi from agent.\nuser draft here\n");
        ed.add_frozen_lines(0, 1);
        let a0 = ed.anchor_for_line(0);
        ed.metadata_mut::<TurnId>().insert(a0, TurnId::Llm(1));
        ed.append_llm_chunk(TurnId::Llm(1), "And more!\n");
        assert_eq!(
            ed.document().full_text(),
            "Hi from agent.\nAnd more!\nuser draft here\n"
        );
        // The user's draft line should now be at line 2 and remain editable.
        assert!(!ed.is_frozen_line(2));
        // The new chunk line (line 1) should be frozen.
        assert!(ed.is_frozen_line(1));
    }

    // ---- Comprehensive buffer-pumping / append_llm_chunk tests -----

    #[test]
    fn rapid_single_char_chunks_reassemble_correctly() {
        let mut ed = new_editor("");
        let msg = "Hello, world!\n";
        for ch in msg.chars() {
            ed.append_llm_chunk(TurnId::Llm(1), &ch.to_string());
        }
        assert_eq!(ed.document().full_text(), msg);
    }

    #[test]
    fn many_rapid_chunks_same_turn() {
        let mut ed = new_editor("");
        for i in 0..100 {
            ed.append_llm_chunk(TurnId::Llm(1), &format!("chunk{i} "));
        }
        let text = ed.document().full_text();
        for i in 0..100 {
            assert!(text.contains(&format!("chunk{i} ")), "missing chunk{i}");
        }
    }

    #[test]
    fn alternating_turns_preserve_all_content() {
        let mut ed = new_editor("");
        for i in 1..=20 {
            ed.append_llm_chunk(TurnId::Llm(i), &format!("turn-{i}\n"));
        }
        let text = ed.document().full_text();
        for i in 1..=20 {
            assert!(
                text.contains(&format!("turn-{i}\n")),
                "missing turn-{i}"
            );
        }
        assert_eq!(ed.document().line_count(), 21); // 20 lines + trailing empty
    }

    #[test]
    fn large_single_chunk_appended_correctly() {
        let mut ed = new_editor("");
        let big = "x".repeat(10_000) + "\n";
        ed.append_llm_chunk(TurnId::Llm(1), &big);
        assert_eq!(ed.document().full_text(), big);
        assert!(ed.is_frozen_line(0));
    }

    #[test]
    fn multi_line_chunk_freezes_all_lines() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "line1\nline2\nline3\n");
        assert_eq!(ed.document().full_text(), "line1\nline2\nline3\n");
        for i in 0..3 {
            assert!(ed.is_frozen_line(i), "line {i} should be frozen");
            let a = ed.anchor_for_line(i);
            assert_eq!(
                ed.metadata::<TurnId>().get(a),
                Some(&TurnId::Llm(1)),
                "line {i} should be tagged Llm(1)"
            );
        }
    }

    #[test]
    fn chunks_without_trailing_newline_join_correctly() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "Hello");
        ed.append_llm_chunk(TurnId::Llm(1), ", ");
        ed.append_llm_chunk(TurnId::Llm(1), "world!");
        ed.append_llm_chunk(TurnId::Llm(1), "\n");
        assert_eq!(ed.document().full_text(), "Hello, world!\n");
    }

    #[test]
    fn empty_chunks_are_no_ops() {
        let mut ed = new_editor("existing\n");
        let before = ed.document().full_text();
        ed.append_llm_chunk(TurnId::Llm(1), "");
        ed.append_llm_chunk(TurnId::Llm(1), "");
        assert_eq!(ed.document().full_text(), before);
    }

    #[test]
    fn interleaved_user_and_llm_content() {
        // Simulate: LLM writes turn 1, user types, LLM writes turn 2.
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "Agent reply 1\n");

        // Simulate user typing at EOF (editable region).
        let eof = ed.document().rope().len_chars();
        ed.programmatic_insert(eof, "user message\n");

        // Now LLM turn 2 arrives — should go to EOF, after user content.
        ed.append_llm_chunk(TurnId::Llm(2), "Agent reply 2\n");
        let text = ed.document().full_text();
        assert!(text.contains("Agent reply 1\n"));
        assert!(text.contains("user message\n"));
        assert!(text.contains("Agent reply 2\n"));
    }

    #[test]
    fn continuation_chunk_after_newline_goes_to_next_line() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "first line\n");
        ed.append_llm_chunk(TurnId::Llm(1), "second line\n");
        assert_eq!(
            ed.document().full_text(),
            "first line\nsecond line\n"
        );
        assert!(ed.is_frozen_line(0));
        assert!(ed.is_frozen_line(1));
    }

    #[test]
    fn stress_many_small_chunks_many_turns() {
        let mut ed = new_editor("");
        for turn in 1..=50 {
            for chunk_idx in 0..10 {
                let text = if chunk_idx == 9 {
                    format!("t{turn}c{chunk_idx}\n")
                } else {
                    format!("t{turn}c{chunk_idx}-")
                };
                ed.append_llm_chunk(TurnId::Llm(turn), &text);
            }
        }
        let text = ed.document().full_text();
        // Verify every turn's content is present.
        for turn in 1..=50 {
            assert!(
                text.contains(&format!("t{turn}c0-")),
                "missing start of turn {turn}"
            );
            assert!(
                text.contains(&format!("t{turn}c9\n")),
                "missing end of turn {turn}"
            );
        }
        // 50 turns, each ending with \n, so 50 content lines.
        assert_eq!(
            text.lines().count(),
            50,
            "expected 50 lines, got {}",
            text.lines().count()
        );
    }

    #[test]
    fn chunk_with_only_newlines() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "before");
        ed.append_llm_chunk(TurnId::Llm(1), "\n\n\n");
        ed.append_llm_chunk(TurnId::Llm(1), "after\n");
        let text = ed.document().full_text();
        assert!(text.starts_with("before\n\n\n"));
        assert!(text.contains("after\n"));
    }

    #[test]
    fn frozen_lines_count_matches_content() {
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "a\nb\nc\nd\ne\n");
        let frozen: Vec<(usize, usize)> = ed.frozen_lines().to_vec();
        let frozen_count: usize = frozen.iter().map(|(s, e)| e - s).sum();
        assert_eq!(frozen_count, 5, "5 content lines should be frozen");
    }

    #[test]
    fn append_to_editor_with_preexisting_content_no_tags() {
        // Editor has content but no frozen/tagged lines. Chunk should
        // append at EOF.
        let mut ed = new_editor("preexisting content\n");
        ed.append_llm_chunk(TurnId::Llm(1), "agent says hi\n");
        assert_eq!(
            ed.document().full_text(),
            "preexisting content\nagent says hi\n"
        );
    }
}
