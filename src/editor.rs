use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::path::PathBuf;

use crate::cursor::CursorPos;
use crate::document::{AnchorShift, Document};
use crate::tree::{BlockInfo, TreeState};

// Test-only instrumentation for finding #9: counts every anchor visited by the
// O(transcript) reverse scan in `last_line_with_meta`. The `last_llm_line`
// cache should drive this to 0 on the common "continue current turn" streaming
// path, proving per-chunk work is independent of transcript size.
//
// Thread-local so `cargo test`'s parallel runner doesn't cross-contaminate the
// count between tests — the scan always runs on the calling test's thread.
#[cfg(test)]
thread_local! {
    static ANCHOR_SCAN_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
        // Perf: shift only the affected suffix in place instead of cloning both
        // maps every chunk. An insert with N newlines at `eff_line` moves every
        // anchor at-or-below the insertion point down by `inserted_nl`; lines
        // strictly above are untouched. For the common streaming case (append at
        // EOF) the affected suffix is empty/tiny, so this is ~O(1) rather than
        // O(total anchors). This turns O(A^2) streaming into O(A).
        //
        // The first line shifts only when the insert is at column 0 (a pure
        // line break before it); a mid-line insert keeps `eff_line` in place.
        let first_shifted = if eff_col == 0 { eff_line } else { eff_line + 1 };

        // Collect the affected tail keys, remove them, and re-insert shifted.
        // Iterating descending avoids transient key collisions in by_line.
        let affected: Vec<usize> = self
            .by_line
            .range(first_shifted..)
            .map(|(&line, _)| line)
            .collect();
        for line in affected.into_iter().rev() {
            let a = self.by_line.remove(&line).expect("line present");
            let new_line = line + inserted_nl;
            self.by_line.insert(new_line, a);
            self.by_anchor.insert(a, new_line);
        }
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
    /// Perf: cached tail line of the in-progress LLM turn, so `append_llm_chunk`
    /// doesn't reverse-scan the entire anchor store per streamed chunk
    /// (`last_line_with_meta` is O(distance from EOF to the LLM tail), unbounded
    /// as trailing user/tool lines accumulate). Updated when a chunk is
    /// appended, shifted in lock-step by the insert/delete paths, and reset on
    /// turn finalize / anchor reset. Treated as a hint: the caller re-validates
    /// the tag before trusting it and falls back to the full scan on a miss.
    last_llm_line: Option<usize>,
    /// Companion to `last_llm_line`: true when that line is still *mid-stream*
    /// (the last appended chunk did NOT end with `\n`), false when the chunk
    /// closed the line with a hard break. Only consulted by the floored
    /// draft-coexistence path (`append_llm_chunk_floored`) to tell an artificial
    /// "separated from the draft" newline apart from a genuine paragraph break,
    /// so mid-stream chunks keep flowing onto one agent line above the draft
    /// while real breaks still start a fresh line. Shifted/reset in lock-step
    /// with `last_llm_line`.
    last_llm_open: bool,
    /// Half-open line ranges of *atomic* frozen blocks — multi-line structural
    /// units (fenced code blocks, tables) that must NEVER be split by an insert,
    /// because they only render correctly as a whole. A SUBSET of `frozen_lines`.
    /// Each single frozen *prose* line is its own block and is deliberately
    /// absent here, so inserting *between* two prose lines stays legal (the
    /// "insert between frozen blocks" gesture); only the *interior* of an atomic
    /// block is locked. Re-seeded from the render-time block detector whenever
    /// the frozen layout changes (`set_atomic_blocks`); shifted in lock-step with
    /// `frozen_lines` by the insert/delete paths so it stays current between
    /// re-seeds. Sorted, non-overlapping.
    atomic_blocks: Vec<(usize, usize)>,
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

/// Uniform access to the `(EditorView, EditorCore)` pair behind either an owned
/// [`Editor`] or a pool-backed handle whose core lives in an `Rc<RefCell<…>>`.
/// This is the ONE thing the two storage shapes differ on; everything else (the
/// ~40 cursor/motion/edit operations) is written once as default methods on the
/// `EditOps` trait in the GPUI binary, over this accessor. Replaces the two
/// hand-written, must-stay-in-lockstep delegation impls.
pub trait EditAccess {
    fn view(&self) -> &EditorView;
    fn view_mut(&mut self) -> &mut EditorView;
    /// Run `f` with a shared borrow of the core.
    fn read_core<R>(&self, f: impl FnOnce(&EditorCore) -> R) -> R;
    /// Run `f` with mutable borrows of BOTH the view and the core (a split
    /// borrow for the owned case, `borrow_mut()` for the pooled case).
    fn edit<R>(&mut self, f: impl FnOnce(&mut EditorView, &mut EditorCore) -> R) -> R;
}

impl EditAccess for Editor {
    fn view(&self) -> &EditorView {
        &self.view
    }
    fn view_mut(&mut self) -> &mut EditorView {
        &mut self.view
    }
    fn read_core<R>(&self, f: impl FnOnce(&EditorCore) -> R) -> R {
        f(&self.core)
    }
    fn edit<R>(&mut self, f: impl FnOnce(&mut EditorView, &mut EditorCore) -> R) -> R {
        f(&mut self.view, &mut self.core)
    }
}

// =============================================================================
// EditorCore
// =============================================================================

impl EditorCore {
    pub fn new(text: String, file_path: PathBuf) -> Self {
        let mut tree_state = TreeState::new();
        tree_state.parse(text.as_bytes(), None);

        let document = Document::from_text(text, file_path);
        Self {
            document,
            tree_state,
            frozen_lines: Vec::new(),
            lockable_through_line: 0,
            line_anchors: LineAnchorStore::default(),
            line_metadata: LineMetadataStore::default(),
            last_llm_line: None,
            last_llm_open: false,
            atomic_blocks: Vec::new(),
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
    ///
    /// This is the O(transcript) reverse scan that finding #9's `last_llm_line`
    /// cache exists to avoid on the hot streaming path. The `ANCHOR_SCAN_VISITS`
    /// counter (test builds only) tallies every anchor this touches so a test
    /// can assert the cached common case visits 0 — i.e. work is independent of
    /// transcript size N.
    fn last_line_with_meta<T: Any + Send + Sync + PartialEq>(&self, tag: &T) -> Option<usize> {
        let view = self.metadata::<T>();
        for (&line, &anchor) in self.line_anchors.by_line.iter().rev() {
            #[cfg(test)]
            ANCHOR_SCAN_VISITS.with(|c| c.set(c.get() + 1));
            if let Some(v) = view.get(anchor)
                && v == tag
            {
                return Some(line);
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
            self.document
                .line_col_to_char(self.lockable_through_line, 0)
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
            if let Some(last) = merged.last_mut()
                && s <= last.1
            {
                last.1 = last.1.max(e);
                continue;
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
        self.atomic_blocks.clear();
    }

    /// Replace the atomic-block ranges (fenced code blocks / tables) — the
    /// indivisible structural subset of `frozen_lines`. Called from the render
    /// path's block detector whenever the frozen layout changes, so the insert
    /// guard (`can_insert_char_at`) reflects the current structure. Ranges are
    /// clamped to in-bounds and stored sorted; an out-of-range or empty range is
    /// dropped defensively.
    pub fn set_atomic_blocks(&mut self, mut ranges: Vec<(usize, usize)>) {
        ranges.retain(|&(s, e)| s < e);
        ranges.sort_unstable_by_key(|&(s, _)| s);
        self.atomic_blocks = ranges;
    }

    pub fn atomic_blocks(&self) -> &[(usize, usize)] {
        &self.atomic_blocks
    }

    /// Drop every allocated anchor and all `LineMetadata`. Used by undo/redo,
    /// which bulk-restore the rope and frozen ranges without going through the
    /// shift machinery; the anchor store would otherwise be left referencing
    /// stale line indices. Consumers must re-acquire anchors after this fires.
    pub fn reset_line_anchors(&mut self) {
        self.line_anchors = LineAnchorStore::default();
        self.line_metadata = LineMetadataStore::default();
        // Perf cache: the anchors it referenced are gone.
        self.last_llm_line = None;
        self.last_llm_open = false;
    }

    /// Replay an undo/redo's line-level [`AnchorShift`]s on the anchor store so
    /// frozen-line metadata (TurnId / tool tags) tracks the rope change — the
    /// fix for "undo wiped the gutter / tool calls jumped to the bottom"
    /// (worksheet-frozen-blocks ticket 001 / C3). The metadata is keyed by
    /// stable anchor id, so SHIFTING the anchors (instead of the old
    /// `reset_line_anchors`) preserves every surviving tag; only anchors on
    /// lines a delete actually consumed are dropped. The LLM-tail perf hint is
    /// line-derived, so it's safely invalidated (re-derived on next use).
    pub fn apply_anchor_shifts(&mut self, shifts: &[AnchorShift]) {
        for op in shifts {
            match *op {
                AnchorShift::Insert { line, col, nl } => {
                    self.line_anchors.shift_for_insert(line, col, nl);
                }
                AnchorShift::Delete { line, col, nl } => {
                    for a in self.line_anchors.shift_for_delete(line, col, nl) {
                        self.line_metadata.drop_anchor(a);
                    }
                }
            }
        }
        self.last_llm_line = None;
        self.last_llm_open = false;
    }

    /// Perf cache accessor: cached tail line of the in-progress LLM turn.
    fn cached_llm_line(&self) -> Option<usize> {
        self.last_llm_line
    }

    /// True when the cached LLM tail line is still mid-stream (see field docs).
    fn cached_llm_open(&self) -> bool {
        self.last_llm_open
    }

    /// Perf cache mutator: record the LLM turn's tail line after appending,
    /// plus whether that line is still open (no trailing `\n`).
    fn set_cached_llm_line(&mut self, line: usize, open: bool) {
        self.last_llm_line = Some(line);
        self.last_llm_open = open;
    }

    /// Perf cache: clear the LLM-tail hint (called on turn finalize).
    pub fn clear_cached_llm_line(&mut self) {
        self.last_llm_line = None;
        self.last_llm_open = false;
    }

    /// True if `line` is in any frozen range.
    pub fn is_frozen_line(&self, line: usize) -> bool {
        self.frozen_lines
            .iter()
            .any(|&(s, e)| line >= s && line < e)
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
    ///     before/after the frozen line without splitting it, AND
    ///   - that boundary is NOT the interior of an atomic block (a fenced code
    ///     block or table): those are one indivisible frozen block, so a `\n` is
    ///     legal only at the very top (col 0 of the first line, inserts above) or
    ///     the very bottom (end of the last line, inserts below) — never between
    ///     two interior lines, which would split the block and corrupt its render.
    ///     Single frozen *prose* lines are not atomic, so inserting between two of
    ///     them stays legal (the "insert between frozen blocks" gesture).
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
        let line_end = line_len.saturating_sub(if self.document.line_text(line).ends_with('\n') {
            1
        } else {
            0
        });
        let at_line_start = col == 0;
        let at_line_end = col >= line_end;
        if !(at_line_start || at_line_end) {
            return false;
        }
        // Atomic-block interior guard: if this line belongs to an atomic block,
        // only its outer boundaries are insertable.
        if let Some(&(s, e)) = self.atomic_blocks.iter().find(|&&(s, e)| line >= s && line < e) {
            let above_block = at_line_start && line == s;
            let below_block = at_line_end && line + 1 == e;
            return above_block || below_block;
        }
        true
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

        // Atomic blocks shift wholesale (they are never split — the insert guard
        // forbids interior inserts), so a range is either entirely below the
        // insert (move down) or entirely above (unchanged). The straddle branch
        // is unreachable via the guarded path; defensively grow the range so it
        // keeps covering its lines rather than stranding the tail.
        for (s, e) in self.atomic_blocks.iter_mut() {
            if *s >= eff_line {
                *s += inserted_nl;
                *e += inserted_nl;
            } else if *e > eff_line {
                *e += inserted_nl;
            }
        }

        self.line_anchors
            .shift_for_insert(eff_line, eff_col, inserted_nl);

        // Perf cache: keep the LLM-tail hint in lock-step with the same shift
        // applied to the anchor store (lines at-or-below the insert move down).
        if let Some(ref mut llm) = self.last_llm_line {
            let first_shifted = if eff_col == 0 { eff_line } else { eff_line + 1 };
            if *llm >= first_shifted {
                *llm += inserted_nl;
            }
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
        let (start_line, start_col) = char_to_line_col(&self.document, del_s);
        for (s, e) in self.frozen_lines.iter_mut() {
            if *s > start_line {
                *s = s.saturating_sub(deleted_nl);
                *e = e.saturating_sub(deleted_nl);
            }
        }
        // Atomic blocks mirror the frozen-range shift (a delete can never touch a
        // frozen/atomic line per `can_delete_range`, so a block only moves up
        // when editable lines above it are removed).
        for (s, e) in self.atomic_blocks.iter_mut() {
            if *s > start_line {
                *s = s.saturating_sub(deleted_nl);
                *e = e.saturating_sub(deleted_nl);
            }
        }
        if self.lockable_through_line > start_line {
            self.lockable_through_line = self.lockable_through_line.saturating_sub(deleted_nl);
        }

        let dropped = self
            .line_anchors
            .shift_for_delete(start_line, start_col, deleted_nl);
        for a in dropped {
            self.line_metadata.drop_anchor(a);
        }

        // Perf cache: mirror the delete on the LLM-tail hint. Lines wholly
        // consumed by the delete invalidate it; lines below shift up. Boundary
        // semantics match shift_for_delete (start line survives iff start_col>0).
        if let Some(llm) = self.last_llm_line {
            let consumed_lo = if start_col == 0 {
                start_line
            } else {
                start_line + 1
            };
            let consumed_hi = start_line + deleted_nl; // inclusive
            if llm >= consumed_lo && llm <= consumed_hi {
                self.last_llm_line = None;
                self.last_llm_open = false;
            } else if llm > consumed_hi {
                self.last_llm_line = Some(llm - deleted_nl);
            }
        }
    }

    /// Programmatic insert (bypasses lockable guard). Used by app.rs to push
    /// Claude replies into the *claude* buffer.
    pub fn programmatic_insert(&mut self, char_idx: usize, text: &str) {
        let (line, col) = char_to_line_col(&self.document, char_idx);
        self.shift_frozen_lines_for_insert(line, col, text);
        // NON-undoable: agent/programmatic content must never be reachable by
        // the user's undo (else a chunk streamed while the user is mid-insert
        // folds into their open undo group and a later undo wipes the whole
        // transcript). Recorded user splices are position-shifted to stay
        // correct across the interleave.
        self.document.insert_str_at_char_no_undo(char_idx, text);
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
        self.document.delete_range_no_undo(s, e);
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

    /// Re-parse the document with tree-sitter, incrementally when exactly one
    /// clean splice has happened since the last reparse (the typing hot path),
    /// else a full parse. The edit is computed in `Document::record_splice` and
    /// consumed here via `take_pending_edit`.
    pub fn reparse(&mut self) {
        let edit = self.document.take_pending_edit();
        let text = self.document.full_text();
        if std::env::var("YALDA_PARSE_TIMING").as_deref() == Ok("1") {
            let kind = if edit.is_some() { "incr" } else { "full" };
            let t0 = std::time::Instant::now();
            self.tree_state.parse(text.as_bytes(), edit);
            let us = t0.elapsed().as_micros();
            if us > 100 {
                eprintln!("[parse] {kind} reparse {} bytes in {us}µs", text.len());
            }
        } else {
            self.tree_state.parse(text.as_bytes(), edit);
        }
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
            let prev_was_line_aligned = self.selection_anchor.map(|a| a.col == 0).unwrap_or(false)
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
        core.document
            .end_undo_group(self.cursor.line, self.cursor.col);
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

    /// Replace the single character under the cursor with `ch` (vim `r`),
    /// leaving the cursor on it. No-op on an empty line or past end-of-line
    /// (nothing to replace), and respects the same frozen-line / lockable
    /// guards as delete + insert. The delete and re-insert land in one undo
    /// group so `r` is a single undo step.
    pub fn replace_char_at_cursor(&mut self, core: &mut EditorCore, ch: char) {
        let line = self.cursor.line;
        let col = self.cursor.col;
        if col >= core.document.line_len_chars(line) {
            return;
        }
        let char_idx = core.document.line_col_to_char(line, col);
        if !core.can_delete_range(char_idx, char_idx + 1) || !core.can_insert_char_at(line, col, ch)
        {
            return;
        }
        core.document.begin_undo_group(
            line,
            col,
            &core.frozen_lines,
            core.lockable_through_line,
        );
        core.shift_frozen_lines_for_delete(char_idx, char_idx + 1);
        core.document.delete_char(line, col);
        core.shift_frozen_lines_for_insert(line, col, &ch.to_string());
        core.document.insert_char(line, col, ch);
        // Cursor stays on the replaced character (normal-mode position).
        self.cursor.line = line;
        self.cursor.col = col;
        core.document.end_undo_group(line, col);
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
        if let Some((line, col, frozen, lockable, shifts)) =
            core.document.undo(&cur_frozen, cur_lockable)
        {
            core.frozen_lines = frozen;
            core.lockable_through_line = lockable;
            // C3: SHIFT the anchors to track the rope change (preserving
            // TurnId/tool metadata) instead of resetting them.
            core.apply_anchor_shifts(&shifts);
            self.cursor.line = line.min(core.document.line_count().saturating_sub(1));
            // `set_col` clears the sticky `desired_col` so the following clamp
            // restores THIS column, not a stale one from an earlier j/k run.
            self.cursor.set_col(col);
            self.clamp_cursor_col(core, false);
            core.reparse();
        }
    }

    pub fn redo(&mut self, core: &mut EditorCore) {
        let cur_frozen = core.frozen_lines.clone();
        let cur_lockable = core.lockable_through_line;
        if let Some((line, col, frozen, lockable, shifts)) =
            core.document.redo(&cur_frozen, cur_lockable)
        {
            core.frozen_lines = frozen;
            core.lockable_through_line = lockable;
            core.apply_anchor_shifts(&shifts);
            self.cursor.line = line.min(core.document.line_count().saturating_sub(1));
            // `set_col` clears the sticky `desired_col` so the following clamp
            // restores THIS column, not a stale one from an earlier j/k run.
            self.cursor.set_col(col);
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

    pub fn move_cursor_first_non_blank(&mut self, core: &EditorCore) {
        self.cursor.move_first_non_blank(&core.document);
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

    pub fn jump_to_line(&mut self, core: &EditorCore, line: usize) {
        self.cursor.jump_to_line(&core.document, line);
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

    pub fn set_atomic_blocks(&mut self, ranges: Vec<(usize, usize)>) {
        self.core.set_atomic_blocks(ranges);
    }

    pub fn atomic_blocks(&self) -> &[(usize, usize)] {
        self.core.atomic_blocks()
    }

    pub fn is_frozen_line(&self, line: usize) -> bool {
        self.core.is_frozen_line(line)
    }

    pub fn is_in_frozen_range(&self, char_idx: usize) -> bool {
        self.core.is_in_frozen_range(char_idx)
    }

    /// Programmatic insert that ALSO keeps the view cursor anchored to the
    /// user's text: an insert at or before the cursor shifts the cursor right by
    /// the inserted length. Without this, a chunk streamed ABOVE the caret (a
    /// new agent turn while the user has a worksheet draft, or a long replay on
    /// resume) strands the caret on what is now agent content — the
    /// streaming-cursor-drift bug (worksheet-frozen-blocks ticket 001 / F2). The
    /// `core` shifts frozen ranges + anchors for the same splice; this is the
    /// missing cursor half, applied here because the wrapper holds view + core.
    /// EVERY Editor-level programmatic insert routes through this.
    fn splice_insert(&mut self, char_idx: usize, text: &str) {
        let (cl, cc) = {
            let c = self.view.cursor();
            (c.line, c.col)
        };
        let cursor_char = self.core.document().line_col_to_char(cl, cc);
        self.core.programmatic_insert(char_idx, text);
        if char_idx <= cursor_char {
            let shifted = cursor_char + text.chars().count();
            let (l, c) = char_to_line_col(self.core.document(), shifted);
            let cur = self.view.cursor_mut();
            cur.line = l;
            cur.col = c;
        }
    }

    /// Splice-delete companion to [`splice_insert`]: a delete before the cursor
    /// shifts it left; a delete spanning the cursor clamps it to the deletion
    /// start. Keeps the caret anchored through agent-driven deletes.
    fn splice_delete(&mut self, del_s: usize, del_e: usize) {
        let (cl, cc) = {
            let c = self.view.cursor();
            (c.line, c.col)
        };
        let cursor_char = self.core.document().line_col_to_char(cl, cc);
        self.core.programmatic_delete(del_s, del_e);
        let new_char = if cursor_char >= del_e {
            cursor_char - del_e.saturating_sub(del_s)
        } else if cursor_char > del_s {
            del_s
        } else {
            cursor_char
        };
        let (l, c) = char_to_line_col(self.core.document(), new_char);
        let cur = self.view.cursor_mut();
        cur.line = l;
        cur.col = c;
    }

    pub fn programmatic_insert(&mut self, char_idx: usize, text: &str) {
        self.splice_insert(char_idx, text);
    }

    /// Perf cache (finding 2): drop the in-progress LLM-tail hint at turn end so
    /// the next turn's first chunk doesn't reuse a stale line index.
    pub fn clear_cached_llm_line(&mut self) {
        self.core.clear_cached_llm_line();
    }

    pub fn programmatic_delete(&mut self, del_s: usize, del_e: usize) {
        self.splice_delete(del_s, del_e);
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
        self.append_llm_chunk_floored(turn_tag, chunk, usize::MAX);
    }

    /// Like [`append_llm_chunk`] but never splices below `floor_char` — the
    /// char at the top of the user's in-progress worksheet draft (the
    /// contiguous untagged tail). `find_llm_insertion_point` falls back to EOF
    /// for a new turn or a tool-broken tail; clamping to `floor_char` keeps
    /// that streamed content ABOVE the user's pending text instead of below
    /// it. Pass `usize::MAX` for "no floor" (the plain EOF behavior).
    pub fn append_llm_chunk_floored<T>(&mut self, turn_tag: T, chunk: &str, floor_char: usize)
    where
        T: Any + Send + Sync + Clone + PartialEq,
    {
        if chunk.is_empty() {
            return;
        }
        let eof = self.core.document().rope().len_chars();
        // Mid-token tool interruption (interspersed-tool-group bug for AGENT
        // text): if this turn's tail line is still OPEN and a tool call spliced
        // itself INSIDE a token — the open line's last content char and this
        // chunk's first char are both non-whitespace — the continuation must
        // rejoin the open run's end-of-content rather than land on a fresh line
        // below the tool (where `find_llm_insertion_point` sends it once the
        // tool closed the line with a '\n'). Otherwise the token is cut in half,
        // e.g. `mode=max` rendering as "`m" | ToolSearch | "ode=max". A break at
        // a whitespace/sentence boundary is a LEGITIMATE interleave and is left
        // alone. Overrides `natural` only in the straddled-token case.
        let natural = self
            .midtoken_rejoin_point::<T>(&turn_tag, chunk)
            .unwrap_or_else(|| self.find_llm_insertion_point::<T>(&turn_tag));
        let insertion_char = if floor_char >= eof || natural < floor_char {
            // No draft below the insertion point, or `natural` already lands in
            // the agent region ABOVE the user's draft (a mid-line streaming
            // continuation). Either way the plain clamp is safe.
            natural.min(floor_char)
        } else {
            // `natural` is at/below the draft top (`floor_char`): a NEW turn, or
            // this turn's last agent line ended with a newline that sits directly
            // above the draft. Inserting at `floor_char` would fuse the chunk into
            // the user's draft line and freeze it as agent content (the draft-
            // corruption / out-of-order bug). Keep the chunk on its own agent line
            // ABOVE the draft instead.
            //
            // Continue the agent line immediately above the draft ONLY when it is
            // this turn AND still mid-stream (`cached_llm_open`) — i.e. its
            // trailing newline is the artificial "separated from the draft" break,
            // not a real paragraph break. Otherwise (new turn, or the last chunk
            // closed the line with its own `\n`) open a clean blank line at the
            // floor (mirrors `anchor_for_new_tool_call`), so genuine paragraph
            // breaks stay on separate lines.
            let draft_top = self.core.document().rope().char_to_line(floor_char);
            let continue_above = draft_top
                .checked_sub(1)
                .filter(|_| self.core.cached_llm_open())
                .filter(|&la| self.core.cached_llm_line() == Some(la))
                .filter(|&la| self.line_tagged_this_turn::<T>(la, &turn_tag));
            match continue_above {
                Some(la) => {
                    // End of that agent line's content, before its trailing '\n'.
                    let len = self.core.document().line_len_chars(la);
                    self.core.document().line_col_to_char(la, len)
                }
                None => {
                    // Open a clean blank line at the floor (shifts the draft
                    // down by one) and write the chunk into it.
                    self.splice_insert(floor_char, "\n");
                    floor_char
                }
            }
        };
        self.splice_insert(insertion_char, chunk);

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

        // Perf cache (finding 2): record this turn's tail line so the next chunk
        // can find its insertion point in O(1) instead of reverse-scanning the
        // whole anchor store. The tail is the last line we just tagged; it stays
        // OPEN (continuable on one line) until a chunk closes it with a `\n`.
        if end_line > start_line {
            self.core
                .set_cached_llm_line(end_line - 1, !chunk.ends_with('\n'));
        }
    }

    /// Append `text` as a frozen user turn — the user-side mirror of
    /// `append_llm_chunk`. Ensures the transcript ends with a newline so the
    /// body starts on its own line, inserts `text` (its single trailing
    /// newline normalized away) at EOF, ensures a terminating newline, then
    /// freezes the inserted span and tags each inserted line's anchor with
    /// `turn_tag` (typically a `TurnId::User(k)` payload). Generic over the
    /// tag type `T` because `TurnId` lives in the binary crate, not the lib.
    /// See spec-agent-render-pipeline.md (INV-6 reconstruction parity).
    pub fn freeze_as_user_turn<T>(&mut self, text: &str, turn_tag: T)
    where
        T: Any + Send + Sync + Clone + PartialEq,
    {
        // Ensure the transcript ends with a newline so the appended body
        // starts on its own line. O(1) tail probe instead of full_text().
        if !self.core.document().is_empty() && self.core.document().last_char() != Some('\n') {
            let eof = self.core.document().rope().len_chars();
            self.splice_insert(eof, "\n");
        }
        let start_line = self.core.document().line_count().saturating_sub(1);
        let to_append = text.strip_suffix('\n').unwrap_or(text);
        let eof = self.core.document().rope().len_chars();
        self.splice_insert(eof, to_append);
        // Ensure a terminating newline so the next chunk starts cleanly.
        if self.core.document().last_char() != Some('\n') {
            let eof2 = self.core.document().rope().len_chars();
            self.splice_insert(eof2, "\n");
        }
        let end_line = self.core.document().line_count();
        self.core.add_frozen_lines(start_line, end_line);
        for l in start_line..end_line {
            let a = self.core.anchor_for_line(l);
            self.core.metadata_mut::<T>().insert(a, turn_tag.clone());
        }
    }

    /// UXI-AgentTile-11 rule 5 (stage 2): freeze `text` as a committed user turn INSERTED
    /// after doc line `after_line` — an inline reply placed BETWEEN two agent lines,
    /// rather than appended at EOF. Mirrors [`freeze_as_user_turn`] but at the
    /// anchor; falls back to the EOF append when the anchor is the last line (the
    /// tail). Safe mid-document: `programmatic_insert` shifts the existing frozen
    /// ranges + anchor-keyed metadata for the spliced text, and the insert is
    /// non-undoable, so the user's undo can't wipe the transcript.
    pub fn freeze_as_user_turn_at<T>(&mut self, after_line: usize, text: &str, turn_tag: T)
    where
        T: Any + Send + Sync + Clone + PartialEq,
    {
        let lc = self.core.document().line_count();
        // Tail (or out-of-range) anchor ⇒ the EOF append handles newline guards.
        if after_line + 1 >= lc {
            self.freeze_as_user_turn(text, turn_tag);
            return;
        }
        // Insert at the start of the line after the anchor — always right after a
        // newline, so the inserted body begins cleanly on its own line(s).
        let at = self.core.document().rope().line_to_char(after_line + 1);
        let start_line = after_line + 1;
        let body = text.strip_suffix('\n').unwrap_or(text);
        let insert = format!("{body}\n");
        let n_lines = insert.matches('\n').count();
        self.splice_insert(at, &insert);
        self.core.add_frozen_lines(start_line, start_line + n_lines);
        for l in start_line..start_line + n_lines {
            let a = self.core.anchor_for_line(l);
            self.core.metadata_mut::<T>().insert(a, turn_tag.clone());
        }
    }

    /// End-of-content char of this turn's OPEN tail line when an incoming
    /// `chunk` would fuse a WORD across a tool interruption — otherwise `None`.
    ///
    /// The guard is deliberately narrow: the tail must come from the LIVE cache
    /// and still be OPEN (`cached_llm_open` — the model has not ended the run),
    /// AND the join must be mid-word: the open line's last content char and the
    /// chunk's first char are both **alphanumeric**. That is exactly the streamed
    /// artifact where a tool call landed between two halves of one word
    /// (`mode=max` → "`m" | tool | "ode=max"). Any other boundary — whitespace,
    /// or sentence/word-terminating punctuation like the '.' ending "here." — is
    /// a legitimate `text → tool → text` interleave and returns `None`, so the
    /// tool stays between the two statements (UXI-AgentTile-8). Alphanumeric-only is
    /// conservative on purpose: it fixes the word-cut-in-half case (what reads
    /// worst) without guessing at ambiguous punctuation splits (a filename like
    /// "gate.sh" broken on '.' is left to interleave rather than mis-fused).
    /// `line_len_chars` already excludes the trailing '\n', so this is the
    /// content end whether or not the tool closed the line with a synthetic
    /// newline.
    fn midtoken_rejoin_point<T: Any + Send + Sync + PartialEq>(
        &self,
        turn_tag: &T,
        chunk: &str,
    ) -> Option<usize> {
        if !self.core.cached_llm_open() {
            return None;
        }
        let doc = self.core.document();
        let line = self
            .core
            .cached_llm_line()
            .filter(|&l| l < doc.line_count() && self.line_tagged_this_turn::<T>(l, turn_tag))?;
        let chunk_head = chunk.chars().next()?;
        if !chunk_head.is_alphanumeric() {
            return None;
        }
        let content_len = doc.line_len_chars(line);
        if content_len == 0 {
            return None;
        }
        let last_char = doc.line_text(line).chars().nth(content_len - 1)?;
        if !last_char.is_alphanumeric() {
            return None;
        }
        Some(doc.line_col_to_char(line, content_len))
    }

    fn find_llm_insertion_point<T: Any + Send + Sync + PartialEq>(&self, turn_tag: &T) -> usize {
        let doc = self.core.document();
        let total_chars = doc.rope().len_chars();
        let total_lines = doc.line_count();

        // Perf (finding 2): try the O(1) cached tail first, validating it still
        // carries this turn's tag (a delete/edit may have invalidated it). On a
        // miss fall back to the reverse anchor scan.
        let cached = self
            .core
            .cached_llm_line()
            .filter(|&l| l < total_lines && self.line_tagged_this_turn::<T>(l, turn_tag));
        let Some(last_llm_line) = cached.or_else(|| self.core.last_line_with_meta::<T>(turn_tag))
        else {
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

    /// True if `line`'s anchor carries a `T` tag equal to `turn_tag`. Used to
    /// validate the cached LLM-tail hint (finding 2) before trusting it.
    fn line_tagged_this_turn<T: Any + Send + Sync + PartialEq>(
        &self,
        line: usize,
        turn_tag: &T,
    ) -> bool {
        self.core
            .anchor_for_line_opt(line)
            .and_then(|a| self.core.metadata::<T>().get(a).map(|v| v == turn_tag))
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

    pub fn replace_char_at_cursor(&mut self, ch: char) {
        self.view.replace_char_at_cursor(&mut self.core, ch);
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

    pub fn move_cursor_first_non_blank(&mut self) {
        self.view.move_cursor_first_non_blank(&self.core);
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

    pub fn jump_to_line(&mut self, line: usize) {
        self.view.jump_to_line(&self.core, line);
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
        /// Mirror of `main.rs`'s `TurnId::System`: yalda-local lifecycle
        /// notices that ride `append_llm_chunk` under a dedicated tag and
        /// must never perturb agent-turn numbering (Finding 5, INV-3).
        System,
    }

    /// Mimic `main.rs::append_system_notice`: splice a lifecycle notice
    /// tagged `System` (not `Llm(k)`) through the same append door.
    fn append_system_notice(ed: &mut Editor, msg: &str) {
        if !ed.document().is_empty() && ed.document().last_char() != Some('\n') {
            let eof = ed.document().rope().len_chars();
            ed.programmatic_insert(eof, "\n");
        }
        let notice_line = format!("― {msg}\n");
        ed.append_llm_chunk(TurnId::System, &notice_line);
    }

    fn anchor_scan_visits() -> usize {
        ANCHOR_SCAN_VISITS.with(|c| c.get())
    }
    fn reset_anchor_scan_visits() {
        ANCHOR_SCAN_VISITS.with(|c| c.set(0));
    }

    /// Mimic `main.rs::anchor_for_new_tool_call` + its `Tool(k)` re-tag: a
    /// tool block lands on its own dedicated blank line tagged with a turn
    /// distinct from the surrounding `Llm` prose.
    fn simulate_tool_call(ed: &mut Editor, turn: usize) {
        if !ed.document().full_text().is_empty() && !ed.document().full_text().ends_with('\n') {
            let len = ed.document().rope().len_chars();
            ed.programmatic_insert(len, "\n");
        }
        let len = ed.document().rope().len_chars();
        ed.programmatic_insert(len, "\n");
        let tool_line = ed.document().line_count().saturating_sub(2);
        let anchor = ed.anchor_for_line(tool_line);
        ed.metadata_mut::<TurnId>()
            .insert(anchor, TurnId::Tool(turn));
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
        assert!(
            text.contains("The key line is here."),
            "pre-tool intact: {text:?}"
        );
        assert!(
            text.contains("Found it elsewhere."),
            "post-tool present: {text:?}"
        );
        // No splice/merge of the two stretches.
        assert!(
            !text.contains("The key line is here.Found")
                && !text.contains("FoundThe")
                && !text.contains("hereFound"),
            "post-tool text must not merge into the pre-tool line: {text:?}"
        );
        let pre = text.lines().position(|l| l.contains("key line")).unwrap();
        let post = text.lines().position(|l| l.contains("Found it")).unwrap();
        assert!(
            post > pre,
            "post-tool prose must come after pre-tool: {text:?}"
        );

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
        assert_ne!(
            post, tool_line,
            "post-tool prose landed on the tool line: {text:?}"
        );
        assert!(
            pre < tool_line && tool_line < post,
            "expected pre < tool < post: {text:?}"
        );
        assert_eq!(
            tags[post],
            Some(TurnId::Llm(1)),
            "post-tool line keeps Llm(1): {text:?}"
        );
        assert_eq!(
            tags[pre],
            Some(TurnId::Llm(1)),
            "pre-tool line keeps Llm(1): {text:?}"
        );
    }

    /// UXI-AgentTile-8 (complement of `post_tool_chunk_does_not_clobber_pre_tool_line`):
    /// when a tool interrupts an OPEN run MID-WORD — the pre-tool chunk ends on an
    /// alphanumeric and the post-tool chunk starts on one — the halves REJOIN onto
    /// one line (word kept whole) and the tool renders after, instead of splitting
    /// the word around the tool. This is the screenshot bug: `` `mode=max` `` cut
    /// as "`m" | tool | "ode=max". Negative control: force `midtoken_rejoin_point`
    /// to `None` (or flip either boundary to punctuation) and the word splits.
    #[test]
    fn post_tool_chunk_rejoins_a_word_split_mid_token() {
        let mut ed = new_editor("");
        // Pre-tool chunk ends mid-word (alphanumeric 'm', no trailing '\n').
        ed.append_llm_chunk(TurnId::Llm(1), "only re-push the 8 GB `m");
        simulate_tool_call(&mut ed, 1);
        // Continuation starts on an alphanumeric ('o') — the other half of the word.
        ed.append_llm_chunk(TurnId::Llm(1), "ode=max cache.");

        let text = ed.document().full_text();
        assert!(
            text.contains("8 GB `mode=max cache."),
            "the word `mode=max` is rejoined whole, not split by the tool: {text:?}"
        );
        // The reassembled prose and the tool line stay in order: prose line first,
        // tool line strictly after it (never inside the word).
        let prose = text.lines().position(|l| l.contains("mode=max")).unwrap();
        let tool_line = (0..ed.document().line_count())
            .position(|l| {
                ed.anchor_for_line_opt(l)
                    .and_then(|a| ed.metadata::<TurnId>().get(a).copied())
                    == Some(TurnId::Tool(1))
            })
            .expect("tool line tagged Tool(1)");
        assert!(
            tool_line > prose,
            "tool renders after the completed word (prose@{prose}, tool@{tool_line}): {text:?}"
        );
    }

    #[test]
    fn system_notices_do_not_change_next_agent_chunk_turn() {
        // Finding 5 / INV-3: injecting N yalda-local system notices between
        // an agent chunk and the next one must NOT change the `Llm(k)` the
        // next agent chunk lands under, nor splice agent prose into a notice
        // line. The notices ride a dedicated `TurnId::System` tag, so the
        // `Llm(k)` insertion-point lookup (which keys off the last `Llm`-tagged
        // line) can't see them.
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "Agent prose for turn one.");

        // Inject several notices — the kind append_system_notice produces.
        for i in 0..5 {
            append_system_notice(&mut ed, &format!("notice {i}"));
        }

        // Next agent chunk for the SAME turn.
        ed.append_llm_chunk(TurnId::Llm(1), "More turn-one prose.");

        let tags: Vec<Option<TurnId>> = (0..ed.document().line_count())
            .map(|l| {
                ed.anchor_for_line_opt(l)
                    .and_then(|a| ed.metadata::<TurnId>().get(a).copied())
            })
            .collect();
        let text = ed.document().full_text();

        // No System line was ever retagged Llm, and no Llm line became System.
        for (l, t) in tags.iter().enumerate() {
            if text.lines().nth(l).is_some_and(|s| s.contains("notice")) {
                assert_eq!(
                    *t,
                    Some(TurnId::System),
                    "notice line {l} must stay System: {text:?}"
                );
            }
        }

        // Both agent stretches are tagged Llm(1) — the notices did not shift
        // the turn number for the chunk that followed them.
        let pre = text
            .lines()
            .position(|l| l.contains("Agent prose"))
            .unwrap();
        let post = text
            .lines()
            .position(|l| l.contains("More turn-one"))
            .unwrap();
        assert_eq!(
            tags[pre],
            Some(TurnId::Llm(1)),
            "pre-notice prose keeps Llm(1): {text:?}"
        );
        assert_eq!(
            tags[post],
            Some(TurnId::Llm(1)),
            "post-notice chunk keeps Llm(1): {text:?}"
        );

        // The post-notice chunk landed on its own line, not spliced into a
        // notice line.
        assert!(post > pre, "post-notice prose after pre: {text:?}");
        assert!(
            !text.contains("notice 4More") && !text.contains("Moreusing"),
            "post-notice prose must not splice into a notice line: {text:?}"
        );

        // A different agent turn after notices also gets its own correct tag.
        append_system_notice(&mut ed, "between turns");
        ed.append_llm_chunk(TurnId::Llm(2), "Turn two prose.");
        let text2 = ed.document().full_text();
        let t2_line = text2.lines().position(|l| l.contains("Turn two")).unwrap();
        let t2_tag = ed
            .anchor_for_line_opt(t2_line)
            .and_then(|a| ed.metadata::<TurnId>().get(a).copied());
        assert_eq!(
            t2_tag,
            Some(TurnId::Llm(2)),
            "turn-two chunk keeps Llm(2): {text2:?}"
        );
    }

    fn new_editor(text: &str) -> Editor {
        Editor::new(text.to_string(), PathBuf::from("test.md"))
    }

    #[test]
    fn replace_char_swaps_under_cursor_and_keeps_position() {
        let mut ed = new_editor("hello\n");
        ed.cursor_mut().line = 0;
        ed.cursor_mut().col = 1; // 'e'
        ed.replace_char_at_cursor('a');
        assert_eq!(ed.document().line_text(0).trim_end_matches('\n'), "hallo");
        // Cursor stays on the replaced char (vim `r` leaves it in place).
        assert_eq!(ed.cursor().col, 1);
    }

    #[test]
    fn replace_char_is_a_single_undo_step() {
        let mut ed = new_editor("abc\n");
        ed.cursor_mut().col = 0;
        ed.replace_char_at_cursor('X');
        assert_eq!(ed.document().line_text(0).trim_end_matches('\n'), "Xbc");
        ed.undo();
        assert_eq!(ed.document().line_text(0).trim_end_matches('\n'), "abc");
    }

    #[test]
    fn replace_char_noop_on_empty_line() {
        let mut ed = new_editor("\nx\n");
        ed.cursor_mut().line = 0;
        ed.cursor_mut().col = 0;
        ed.replace_char_at_cursor('z');
        assert_eq!(ed.document().line_text(0).trim_end_matches('\n'), "");
    }

    #[test]
    fn jump_to_line_moves_cursor_and_clamps() {
        let mut ed = new_editor("a\nb\nc\nd\ne\n");
        ed.jump_to_line(2);
        assert_eq!(ed.cursor().line, 2);
        assert_eq!(ed.cursor().col, 0);
        // Past-the-end clamps to the last line.
        ed.jump_to_line(999);
        assert_eq!(ed.cursor().line, ed.document().line_count() - 1);
    }

    /// A multi-line atomic frozen block (fenced code / table) must not be split
    /// by an insert: a newline is legal only above the first line or below the
    /// last, never between two interior lines. This is the "butchers Claude text"
    /// guard — `o`/`O`/Enter on a block interior is a no-op.
    #[test]
    fn atomic_block_interior_insert_is_rejected() {
        // lines 0..3 = an atomic code block; line 3 = editable tail.
        let mut ed = new_editor("```\ncode\n```\n\n");
        ed.add_frozen_lines(0, 3);
        ed.set_atomic_blocks(vec![(0, 3)]);
        let before = ed.document().line_count();

        // `o` at end of line 0 (between fence and body) → interior split → reject.
        ed.cursor_mut().line = 0;
        ed.cursor_mut().col = ed.document().line_len_chars(0);
        ed.open_line_below();
        assert_eq!(
            ed.document().line_count(),
            before,
            "interior split via open-line-below must be rejected"
        );

        // `O` at col 0 of an interior line → interior split → reject.
        ed.cursor_mut().line = 1;
        ed.cursor_mut().col = 0;
        ed.open_line_above();
        assert_eq!(
            ed.document().line_count(),
            before,
            "col-0 interior split via open-line-above must be rejected"
        );

        // A bare typed newline mid-interior is likewise rejected.
        ed.cursor_mut().line = 1;
        ed.cursor_mut().col = 0;
        ed.insert_char('\n');
        assert_eq!(
            ed.document().line_count(),
            before,
            "interior newline insert must be rejected"
        );
    }

    /// The atomic block's OUTER boundaries stay insertable — you can open an
    /// editable line above the whole block or below it, just not inside.
    #[test]
    fn atomic_block_outer_boundaries_allow_insert() {
        let mut ed = new_editor("```\ncode\n```\nx\n");
        ed.add_frozen_lines(0, 3);
        ed.set_atomic_blocks(vec![(0, 3)]);

        // Above the block: col 0 of the first line.
        ed.cursor_mut().line = 0;
        ed.cursor_mut().col = 0;
        let before = ed.document().line_count();
        ed.open_line_above();
        assert_eq!(
            ed.document().line_count(),
            before + 1,
            "insert above the block is allowed"
        );

        // Below the block: end of the last block line (now line 3 after the
        // insert above shifted everything down by one).
        ed.cursor_mut().line = 3;
        ed.cursor_mut().col = ed.document().line_len_chars(3);
        let before2 = ed.document().line_count();
        ed.open_line_below();
        assert_eq!(
            ed.document().line_count(),
            before2 + 1,
            "insert below the block is allowed"
        );
    }

    /// Single frozen PROSE lines are NOT atomic — inserting an editable line
    /// *between* two of them is the intended "insert between frozen blocks"
    /// gesture and must be allowed, leaving the new line editable.
    #[test]
    fn frozen_prose_lines_stay_insertable_between() {
        let mut ed = new_editor("alpha\nbeta\n\n");
        ed.add_frozen_lines(0, 2); // lines 0,1 are frozen prose; no atomic blocks
        ed.cursor_mut().line = 0;
        ed.cursor_mut().col = ed.document().line_len_chars(0);
        let before = ed.document().line_count();
        ed.open_line_below();
        assert_eq!(
            ed.document().line_count(),
            before + 1,
            "insert between two frozen prose lines is allowed"
        );
        assert!(
            !ed.is_frozen_line(1),
            "the line opened between prose lines is editable"
        );
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
        assert_eq!(ed.document().full_text(), "first turn\nsecond turn\n");
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

    #[test]
    fn append_llm_chunk_keeps_caret_on_draft_pushed_down() {
        // F2 (worksheet-frozen-blocks ticket 001 — streaming cursor drift): a
        // chunk streamed ABOVE the user's worksheet draft must carry the caret
        // DOWN with the draft, not strand it on the freshly-inserted agent line.
        // This is the "couldn't find my cursor" report on a resumed session.
        let mut ed = new_editor("Hi from agent.\nuser draft here\n");
        ed.add_frozen_lines(0, 1);
        let a0 = ed.anchor_for_line(0);
        ed.metadata_mut::<TurnId>().insert(a0, TurnId::Llm(1));
        // Caret sits inside the draft (line 1, col 5 = just after "user ").
        ed.cursor_mut().line = 1;
        ed.cursor_mut().col = 5;

        ed.append_llm_chunk(TurnId::Llm(1), "And more!\n");

        // The draft moved to line 2; the caret must have followed it — NOT stuck
        // on line 1, which is now the agent's "And more!" content.
        assert_eq!(ed.document().line_text(2), "user draft here\n");
        assert_eq!(
            ed.cursor().line,
            2,
            "caret tracked the draft down past the streamed chunk (no drift)"
        );
        assert_eq!(ed.cursor().col, 5, "caret column preserved within the draft");
    }

    #[test]
    fn undo_preserves_frozen_line_metadata() {
        // C3 (worksheet-frozen-blocks ticket 001 — undo wipes TurnId/tool tags):
        // a user edit above a tagged agent turn, then undo, must NOT blank the
        // tag or lose it — the "undo erased it / tool calls jumped to the
        // bottom" report. Before the fix, undo called reset_line_anchors,
        // dropping every TurnId tag, so the gutter blanked and a later stream
        // appended at EOF.
        let mut ed = new_editor("draft\nagent turn\n");
        ed.add_frozen_lines(1, 2); // line 1 is the frozen agent turn
        let a1 = ed.anchor_for_line(1);
        ed.metadata_mut::<TurnId>().insert(a1, TurnId::Llm(7));

        // User splits the editable draft with a newline (one undo group, exactly
        // as the GUI key handler wraps a keystroke). The frozen tagged turn
        // shifts DOWN to line 2.
        {
            let Editor { core, view } = &mut ed;
            view.cursor_mut().line = 0;
            view.cursor_mut().col = 5; // end of "draft"
            let fl = core.frozen_lines.clone();
            let lk = core.lockable_through_line;
            let (cl, cc) = {
                let c = view.cursor();
                (c.line, c.col)
            };
            core.document.begin_undo_group(cl, cc, &fl, lk);
            view.insert_char(core, '\n');
            let (al, ac) = {
                let c = view.cursor();
                (c.line, c.col)
            };
            core.document.end_undo_group(al, ac);
        }
        let shifted = ed.anchor_for_line(2);
        assert_eq!(
            ed.metadata::<TurnId>().get(shifted),
            Some(&TurnId::Llm(7)),
            "tag tracked its line down past the user's inserted newline"
        );

        // Undo. The tag MUST survive and be back on line 1 (gutter not blanked,
        // tag not lost to a reset — the C3 fix).
        ed.undo();
        let restored = ed.anchor_for_line(1);
        assert_eq!(
            ed.metadata::<TurnId>().get(restored),
            Some(&TurnId::Llm(7)),
            "agent-turn tag survives undo and is back on its line"
        );
    }

    // ---- Finding #2: freeze_as_user_turn (user-side mirror) -----

    /// Reproduce the exact manual freeze ritual `submit_chatbox` /
    /// `ServerNotification::UserPrompt` open-coded before extraction, so the
    /// equivalence check below pins the helper to the live behavior.
    fn manual_freeze_user_turn(ed: &mut Editor, text: &str, turn_k: usize) {
        if !ed.document().is_empty() && ed.document().last_char() != Some('\n') {
            let eof = ed.document().rope().len_chars();
            ed.programmatic_insert(eof, "\n");
        }
        let start_line = ed.document().line_count().saturating_sub(1);
        let to_append = text.strip_suffix('\n').unwrap_or(text).to_string();
        let eof = ed.document().rope().len_chars();
        ed.programmatic_insert(eof, &to_append);
        if ed.document().last_char() != Some('\n') {
            let eof2 = ed.document().rope().len_chars();
            ed.programmatic_insert(eof2, "\n");
        }
        let end_line = ed.document().line_count();
        ed.add_frozen_lines(start_line, end_line);
        for l in start_line..end_line {
            let a = ed.anchor_for_line(l);
            ed.metadata_mut::<TurnId>().insert(a, TurnId::User(turn_k));
        }
    }

    /// Snapshot the document text, frozen ranges, and per-line TurnId tags so
    /// two editors can be compared for structural equivalence.
    fn snapshot(ed: &mut Editor) -> (String, Vec<(usize, usize)>, Vec<Option<TurnId>>) {
        let text = ed.document().full_text();
        let frozen = ed.frozen_lines().to_vec();
        let tags: Vec<Option<TurnId>> = (0..ed.document().line_count())
            .map(|l| {
                ed.anchor_for_line_opt(l)
                    .and_then(|a| ed.metadata::<TurnId>().get(a).copied())
            })
            .collect();
        (text, frozen, tags)
    }

    #[test]
    fn freeze_as_user_turn_matches_manual_ritual_on_empty_editor() {
        let mut helper = new_editor("");
        helper.freeze_as_user_turn("hello agent\n", TurnId::User(1));
        let mut manual = new_editor("");
        manual_freeze_user_turn(&mut manual, "hello agent\n", 1);
        assert_eq!(snapshot(&mut helper), snapshot(&mut manual));
        // Spot-check the expected shape directly too.
        assert_eq!(helper.document().full_text(), "hello agent\n");
        let a = helper.anchor_for_line(0);
        assert_eq!(helper.metadata::<TurnId>().get(a), Some(&TurnId::User(1)));
        assert!(helper.is_frozen_line(0));
    }

    #[test]
    fn freeze_as_user_turn_matches_manual_with_prior_transcript_and_multiline() {
        // A prior agent reply already frozen + tagged, no trailing newline on
        // the incoming body, and multiple lines — exercises every branch.
        let setup = |ed: &mut Editor| {
            ed.append_llm_chunk(TurnId::Llm(1), "agent reply\n");
        };
        let mut helper = new_editor("");
        setup(&mut helper);
        helper.freeze_as_user_turn("line one\nline two", TurnId::User(2));
        let mut manual = new_editor("");
        setup(&mut manual);
        manual_freeze_user_turn(&mut manual, "line one\nline two", 2);
        assert_eq!(snapshot(&mut helper), snapshot(&mut manual));
        // The two new user lines are frozen and tagged User(2).
        let lc = helper.document().line_count();
        let u1 = helper.anchor_for_line(lc - 3);
        let u2 = helper.anchor_for_line(lc - 2);
        assert_eq!(helper.metadata::<TurnId>().get(u1), Some(&TurnId::User(2)));
        assert_eq!(helper.metadata::<TurnId>().get(u2), Some(&TurnId::User(2)));
    }

    #[test]
    fn freeze_as_user_turn_at_inserts_between_agent_lines() {
        // UXI-AgentTile-11 rule 5 (stage 2): a reply frozen AFTER a middle line of the
        // agent's turn lands between the right lines, is frozen + tagged, and the
        // agent lines below it keep their tags (anchor-keyed metadata auto-shifts).
        let mut ed = new_editor("");
        ed.append_llm_chunk(TurnId::Llm(1), "para one\npara two\npara three\n");
        // Lines: 0 "para one", 1 "para two", 2 "para three". Insert after line 1.
        ed.freeze_as_user_turn_at(1, "my reply", TurnId::User(2));
        let text = ed.document().full_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            &lines[..4],
            &["para one", "para two", "my reply", "para three"],
            "reply lands between para two and para three"
        );
        // The reply line is frozen + tagged User(2).
        assert!(ed.is_frozen_line(2), "reply line frozen");
        let a = ed.anchor_for_line(2);
        assert_eq!(ed.metadata::<TurnId>().get(a), Some(&TurnId::User(2)));
        // The agent line that shifted down keeps its Llm(1) tag.
        let a3 = ed.anchor_for_line(3);
        assert_eq!(
            ed.metadata::<TurnId>().get(a3),
            Some(&TurnId::Llm(1)),
            "shifted agent line kept its tag — metadata auto-shift held"
        );
    }

    #[test]
    fn freeze_as_user_turn_at_tail_degrades_to_eof_append() {
        // An out-of-range / last-line anchor behaves exactly like the EOF append.
        let mut at = new_editor("");
        at.append_llm_chunk(TurnId::Llm(1), "agent\n");
        let mut eof = new_editor("");
        eof.append_llm_chunk(TurnId::Llm(1), "agent\n");
        let last = at.document().line_count().saturating_sub(1);
        at.freeze_as_user_turn_at(last + 5, "reply", TurnId::User(2));
        eof.freeze_as_user_turn("reply", TurnId::User(2));
        assert_eq!(snapshot(&mut at), snapshot(&mut eof));
    }

    // ---- Finding #9: O(1) llm insertion-point cache -----

    /// Build an N-line transcript with a frozen, already-tagged in-flight
    /// `Llm(turn)` line at the very tail — the state during active streaming.
    fn editor_with_inflight_turn(n_lines: usize, turn: usize) -> Editor {
        let mut text = String::new();
        for i in 0..n_lines.saturating_sub(1) {
            text.push_str(&format!("frozen transcript line {i}\n"));
        }
        text.push_str("inflight ");
        let mut ed = new_editor(&text);
        let last = ed.document().line_count().saturating_sub(1);
        // Freeze + tag every prior line so the reverse scan, if it ran, would
        // have to walk all N anchors before finding the tail.
        for l in 0..=last {
            ed.add_frozen_lines(l, l + 1);
            let a = ed.anchor_for_line(l);
            ed.metadata_mut::<TurnId>().insert(a, TurnId::Llm(turn));
        }
        // Prime the cache as `append_llm_chunk` would after its first chunk
        // (the inflight tail ends without a newline, so it's still open).
        ed.core.set_cached_llm_line(last, true);
        ed
    }

    #[test]
    fn anchor_scan_cache_makes_streaming_independent_of_transcript_size() {
        // Two transcripts differing only in N. Per-chunk work (anchor-scan
        // visits) must be identical — i.e. independent of N — because the
        // cached tail short-circuits the reverse scan entirely.
        for &n in &[10usize, 2000usize] {
            let mut ed = editor_with_inflight_turn(n, 1);
            reset_anchor_scan_visits();
            for k in 0..50 {
                ed.append_llm_chunk(TurnId::Llm(1), &format!("c{k} "));
            }
            assert_eq!(
                anchor_scan_visits(),
                0,
                "cached common case must never fall back to the O(N) reverse \
                 scan (N={n})"
            );
            // Correctness: the appended chunks all landed on the tail line.
            let last = ed.document().line_count() - 1;
            let tail = ed.document().line_text(last);
            assert!(tail.starts_with("inflight c0 "), "tail={tail:?}");
            assert!(tail.contains("c49 "), "tail={tail:?}");
        }
    }

    #[test]
    fn anchor_scan_cache_invalidates_then_rebuilds_on_delete() {
        let mut ed = editor_with_inflight_turn(200, 1);
        // A delete that consumes the cached tail line must invalidate the
        // cache, forcing exactly one fallback scan; correctness must hold.
        let last = ed.document().line_count() - 1;
        let start = ed.document().line_col_to_char(last - 1, 0);
        let end = ed.document().rope().len_chars();
        ed.programmatic_delete(start, end); // removes the tail (cached) line

        reset_anchor_scan_visits();
        ed.append_llm_chunk(TurnId::Llm(1), "after delete\n");
        // The fallback scan ran at least once (cache was invalidated)...
        assert!(
            anchor_scan_visits() > 0,
            "delete consuming the cached line must invalidate the cache"
        );

        // ...and subsequent chunks are O(1) again (cache rebuilt).
        reset_anchor_scan_visits();
        ed.append_llm_chunk(TurnId::Llm(1), "and more\n");
        assert_eq!(
            anchor_scan_visits(),
            0,
            "cache must be rebuilt after the fallback scan"
        );
    }

    #[test]
    fn anchor_scan_cache_matches_uncached_result() {
        // Cached and cold paths must produce byte-identical transcripts.
        let mut cached = new_editor("");
        for k in 0..30 {
            cached.append_llm_chunk(TurnId::Llm(1), &format!("w{k} "));
        }

        let mut cold = new_editor("");
        for k in 0..30 {
            // Defeat the cache before each chunk to force the reverse scan.
            cold.core.clear_cached_llm_line();
            cold.append_llm_chunk(TurnId::Llm(1), &format!("w{k} "));
        }

        assert_eq!(cached.document().full_text(), cold.document().full_text());
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
            assert!(text.contains(&format!("turn-{i}\n")), "missing turn-{i}");
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
        assert_eq!(ed.document().full_text(), "first line\nsecond line\n");
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

    /// Smoke-exercise the reparse path that caused the crash. `reparse` used to
    /// feed tree-sitter the previous tree for incremental reuse WITHOUT ever
    /// calling `tree.edit()`, so the old tree's byte offsets went stale; for
    /// markdown with external-scanner constructs (tables / fenced code /
    /// blockquotes) that made `tree_sitter_markdown_external_scanner_serialize`
    /// read/write out of bounds. The OOB is heap-nondeterministic, so this test
    /// does NOT reliably segfault on the buggy code (it passed there too) — it's
    /// a path exerciser that's guaranteed sound after the fresh-parse fix, not a
    /// deterministic repro. The deterministic repro is the GUI (type in a file
    /// with a table; observed SIGSEGV in `external_scanner_serialize`).
    #[test]
    fn reparse_survives_many_edits_without_segfault() {
        let content = "\
# Heading

para text here

- a
- b

```rust
fn f() { let x = 1; }
```

| c1 | c2 |
|----|----|
| 1  | 2  |

> quote line
";
        let mut core = EditorCore::new(content.to_string(), std::path::PathBuf::from("t.md"));
        let mut view = EditorView::new();
        core.reparse();
        // Insert at the document head so EVERY edit shifts all byte offsets,
        // maximally desyncing a stale reused tree; reparse after each char to
        // mirror per-keystroke `end_insert`.
        for i in 0..4000 {
            let ch = if i % 11 == 0 { '\n' } else { 'x' };
            view.insert_char(&mut core, ch);
            core.reparse();
        }
        assert!(core.document.full_text().len() > content.len());
    }

    /// THE incremental-reparse safety guard. Drives the editor through long
    /// random sequences of inserts / backspaces / deletes (markdown-structural
    /// chars, newlines, and multibyte) and after EVERY edit asserts the
    /// incrementally-maintained tree-sitter tree is byte-for-byte identical (by
    /// full s-expression) to a fresh FULL parse of the same text. A wrong
    /// `InputEdit` (the only incremental crash hazard) makes the incremental
    /// tree diverge → this fails deterministically; and the thousands of edits
    /// also exercise the scanner-serialize path that segfaulted. If this
    /// passes, the `Some(edit)` path is sound.
    #[test]
    fn incremental_reparse_matches_full_parse() {
        let mut s: u64 = 0x9E3779B97F4A7C15;
        let mut rnd = |n: usize| -> usize {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % n as u64) as usize
        };
        // Markdown-structural + punctuation + newline + multibyte.
        let chars = [
            'a', 'b', 'c', ' ', '\n', '#', '-', '*', '|', '`', '>', '1', '.', '(', ')', '[', ']',
            'é', '—', '”', 'x', '\n',
        ];
        let seeds = [
            "",
            "# Heading\n\nsome text here\n",
            "para\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nmore\n",
            "```rust\nfn f() {}\n```\n\n- one\n- two\n",
            "> quote\n> more\n\ntext é unicode line\n",
        ];
        for trial in 0..300 {
            let init = seeds[trial % seeds.len()];
            let mut core = EditorCore::new(init.to_string(), std::path::PathBuf::from("t.md"));
            let mut view = EditorView::new();
            core.reparse();
            for step in 0..90 {
                // Move the cursor to a random valid position so edits land in
                // headings, tables, code fences, mid-line, line ends, etc.
                let lc = core.document.line_count().max(1);
                let line = rnd(lc);
                let col = rnd(core.document.line_len_chars(line) + 1);
                view.cursor.line = line;
                view.cursor.col = col;
                match rnd(7) {
                    0..=3 => {
                        // Insert — the GUI wraps each keystroke in begin/end.
                        let ch = chars[rnd(chars.len())];
                        view.begin_insert(&mut core);
                        view.insert_char(&mut core, ch);
                        view.end_insert(&mut core);
                    }
                    4 => {
                        view.begin_insert(&mut core);
                        view.backspace(&mut core);
                        view.end_insert(&mut core);
                    }
                    5 => {
                        // Multi-char single-splice insert (programmatic_insert /
                        // paste) — exercises the multi-line `advance_point` path.
                        let frags = ["ab", "x\ny", "# H\n", "| z |\n", "`c` é", "\n\n", "-- "];
                        let frag = frags[rnd(frags.len())];
                        let ci = core.document.line_col_to_char(line, col);
                        core.programmatic_insert(ci, frag);
                        core.reparse();
                    }
                    _ => {
                        view.delete_char_at_cursor(&mut core);
                    }
                }

                // Oracle: the incremental tree must equal a fresh full parse.
                let text = core.document.full_text();
                let incr = core
                    .tree_state
                    .tree()
                    .map(|t| t.root_node().to_sexp())
                    .unwrap_or_default();
                let mut fresh = crate::tree::TreeState::new();
                fresh.parse(text.as_bytes(), None);
                let full = fresh
                    .tree()
                    .map(|t| t.root_node().to_sexp())
                    .unwrap_or_default();
                assert_eq!(
                    incr, full,
                    "trial {trial} step {step}: incremental tree diverged from full parse.\n\
                     --- TEXT ---\n{text}\n--- END ---"
                );
            }
        }
    }

    /// MEASUREMENT (not a gate): show that an incremental single-char reparse
    /// is ~constant regardless of doc size, vs the O(doc) full parse the
    /// segfault fix had us paying per keystroke. Run with:
    ///   cargo test --lib incremental_reparse_speed -- --nocapture --ignored
    #[test]
    #[ignore]
    fn incremental_reparse_speed() {
        for &lines in &[200usize, 1000, 5000] {
            let mut text = String::new();
            let mut i = 0;
            while text.lines().count() < lines {
                i += 1;
                match i % 6 {
                    0 => text.push_str(&format!("# Section {i}\n\n")),
                    3 => text.push_str("| a | b |\n|---|---|\n| 1 | 2 |\n\n"),
                    4 => text.push_str("```\ncode line\n```\n\n"),
                    _ => text.push_str(&format!("paragraph line {i} with words\n")),
                }
            }
            let mut core = EditorCore::new(text.clone(), std::path::PathBuf::from("t.md"));
            let mut view = EditorView::new();
            core.reparse();
            // Time many incremental single-char inserts (the typing hot path).
            view.cursor.line = core.document.line_count() / 2;
            view.cursor.col = 0;
            const N: u32 = 200;
            let t0 = std::time::Instant::now();
            for _ in 0..N {
                view.begin_insert(&mut core);
                view.insert_char(&mut core, 'x');
                view.end_insert(&mut core);
            }
            let incr_us = t0.elapsed().as_micros() as f64 / N as f64;
            // Full parse cost for the same doc, for comparison.
            let bytes = core.document.full_text();
            let t1 = std::time::Instant::now();
            for _ in 0..N {
                let mut ts = crate::tree::TreeState::new();
                ts.parse(bytes.as_bytes(), None);
            }
            let full_us = t1.elapsed().as_micros() as f64 / N as f64;
            eprintln!(
                "{lines:>5} lines: incremental {incr_us:>7.1}µs/keystroke   vs full parse {full_us:>8.1}µs   ({:.0}x faster)",
                full_us / incr_us.max(0.01)
            );
        }
    }
}
