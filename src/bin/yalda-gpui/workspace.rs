//! Tabs / windows / splits data model for the GPUI workspace.
//!
//! This is the data substrate for `spec-tabs-and-splits.md`: a workspace
//! contains tabs, each tab roots an n-ary split tree of windows, and each
//! window holds one content kind (Doc / Edit / Browser / Agent). File-backed
//! editors live in a pooled `FileBuffer` and may be referenced by multiple
//! windows simultaneously (shared edits across splits).
//!
//! Scope note: the file-buffer pool is wired into the live `GpuiApp` as of 5c
//! (`open_and_retain` dedup-by-canonical-path + `gc_buffers` strong-count
//! liveness back every file-backed Doc/Edit view, so splits share a rope).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use yalda::editor::EditorCore;
use yalda::file_browser::FileBrowser;

/// Shared handle to a pooled `EditorCore`. Multiple `EditorView`s (one per
/// window) clone this `Rc` so they all mutate the same rope + undo stack;
/// each view keeps its own cursor/selection/scroll separately. `RefCell`
/// gives interior mutability so a view can borrow the core for an edit
/// without the workspace lending out a `&mut` that would conflict with the
/// other leaves living in the same layout tree.
pub type SharedCore = Rc<RefCell<EditorCore>>;

pub type FileBufferId = u64;
pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    H,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

/// One leaf of a tab's layout tree. A `Window` carries its stable id plus the
/// per-window content (kind- and frontend-specific state). The content kind
/// (`DocWindow`, `EditWindow`, `BrowserWindow`, `AgentWindow`) is defined in
/// the main binary because some fields reference GPUI-specific types
/// (`ScrollHandle`, etc.).
pub struct Window<C> {
    pub id: WindowId,
    pub content: C,
}

/// An n-ary split tree. A `Split` node holds `>= 2` children; pruning to one
/// child collapses the split into that child (Behavior 14). Weights inside
/// any single `Split` sum to 1.0 and renormalize proportionally on insert/
/// close.
///
/// `Empty` is a sentinel used only as a transient placeholder during
/// `std::mem::take` swaps inside mutation methods (split / close / only). It
/// MUST NOT appear in a tree at rest — mutation methods are responsible for
/// restoring a non-`Empty` root before returning. The variant exists because
/// `Layout<C>` is generic over `C` and can't construct an arbitrary placeholder
/// otherwise.
#[derive(Default)]
pub enum Layout<C> {
    #[default]
    Empty,
    Leaf(Window<C>),
    Split {
        dir: SplitDir,
        children: Vec<(f32, Layout<C>)>,
    },
}

impl<C> Layout<C> {
    /// Find the leaf with the given id (recursive).
    pub fn find_leaf(&self, id: WindowId) -> Option<&Window<C>> {
        match self {
            Layout::Empty => None,
            Layout::Leaf(w) if w.id == id => Some(w),
            Layout::Leaf(_) => None,
            Layout::Split { children, .. } => children.iter().find_map(|(_, c)| c.find_leaf(id)),
        }
    }

    /// Does this subtree contain a leaf with the given id?
    pub fn contains_leaf(&self, id: WindowId) -> bool {
        self.find_leaf(id).is_some()
    }

    /// Find the leaf with the given id (mutable, recursive).
    pub fn find_leaf_mut(&mut self, id: WindowId) -> Option<&mut Window<C>> {
        match self {
            Layout::Empty => None,
            Layout::Leaf(w) if w.id == id => Some(w),
            Layout::Leaf(_) => None,
            Layout::Split { children, .. } => {
                children.iter_mut().find_map(|(_, c)| c.find_leaf_mut(id))
            }
        }
    }

    /// Walk every leaf in tree order (depth-first, in `children` order).
    pub fn for_each_leaf<F: FnMut(&Window<C>)>(&self, f: &mut F) {
        match self {
            Layout::Empty => {}
            Layout::Leaf(w) => f(w),
            Layout::Split { children, .. } => {
                for (_, c) in children {
                    c.for_each_leaf(f);
                }
            }
        }
    }

    /// Mutable walk over every leaf's content in tree order.
    pub fn for_each_leaf_content_mut<F: FnMut(&mut C)>(&mut self, f: &mut F) {
        match self {
            Layout::Empty => {}
            Layout::Leaf(w) => f(&mut w.content),
            Layout::Split { children, .. } => {
                for (_, c) in children {
                    c.for_each_leaf_content_mut(f);
                }
            }
        }
    }

    /// Search every leaf's content for the first match. Returns the mapped
    /// value from `f` on the first `Some` return, or `None` if no leaf
    /// matches.
    pub fn find_map_leaf_content_mut<R, F: FnMut(&mut C) -> Option<R>>(
        &mut self,
        f: &mut F,
    ) -> Option<R> {
        match self {
            Layout::Empty => None,
            Layout::Leaf(w) => f(&mut w.content),
            Layout::Split { children, .. } => {
                for (_, c) in children {
                    if let Some(r) = c.find_map_leaf_content_mut(f) {
                        return Some(r);
                    }
                }
                None
            }
        }
    }

    /// Find the path from root to the leaf with `target` id, expressed as a
    /// sequence of child-indices to follow at each `Split` node. An empty
    /// path means the root itself is the target leaf. Returns `None` if the
    /// target isn't in the tree.
    pub fn path_to(&self, target: WindowId) -> Option<Vec<usize>> {
        match self {
            Layout::Empty => None,
            Layout::Leaf(w) if w.id == target => Some(Vec::new()),
            Layout::Leaf(_) => None,
            Layout::Split { children, .. } => {
                for (idx, (_, child)) in children.iter().enumerate() {
                    if let Some(mut rest) = child.path_to(target) {
                        rest.insert(0, idx);
                        return Some(rest);
                    }
                }
                None
            }
        }
    }

    /// Walk the path down to a node and return a mutable handle to it. An
    /// empty path returns `self`. Returns `None` if the path goes off the
    /// tree (shouldn't happen with paths from `path_to`).
    pub fn node_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Layout<C>> {
        let mut cur = self;
        for &idx in path {
            match cur {
                Layout::Split { children, .. } => {
                    cur = &mut children.get_mut(idx)?.1;
                }
                Layout::Leaf(_) | Layout::Empty => return None,
            }
        }
        Some(cur)
    }

    /// Count the number of leaves in this subtree.
    pub fn leaf_count(&self) -> usize {
        match self {
            Layout::Empty => 0,
            Layout::Leaf(_) => 1,
            Layout::Split { children, .. } => children.iter().map(|(_, c)| c.leaf_count()).sum(),
        }
    }

    /// Yield the ids of every leaf in tree order.
    pub fn leaf_ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.for_each_leaf(&mut |w| out.push(w.id));
        out
    }

    /// Swap the content of two leaves in the tree, identified by their window ids.
    /// Both ids must exist in this tree. No-op if either is missing.
    pub fn swap_leaf_contents(&mut self, id_a: WindowId, id_b: WindowId) {
        // Collect raw pointers to the two windows, then swap their contents.
        // SAFETY: `id_a != id_b` guarantees the two pointers are non-aliasing.
        if id_a == id_b {
            return;
        }
        let ptr_a = self.find_leaf_mut(id_a).map(|w| w as *mut Window<C>);
        let ptr_b = self.find_leaf_mut(id_b).map(|w| w as *mut Window<C>);
        if let (Some(pa), Some(pb)) = (ptr_a, ptr_b) {
            // SAFETY: we verified id_a != id_b so these can't alias.
            unsafe {
                std::ptr::swap(&mut (*pa).content, &mut (*pb).content);
            }
        }
    }

    /// Extract the tree shape as a [`LayoutSkeleton`] (ids + weights + dirs,
    /// no content). Used to save the manual tree before switching to an
    /// automatic layout mode.
    pub fn skeleton(&self) -> LayoutSkeleton {
        match self {
            Layout::Empty => LayoutSkeleton::Empty,
            Layout::Leaf(w) => LayoutSkeleton::Leaf(w.id),
            Layout::Split { dir, children } => LayoutSkeleton::Split {
                dir: *dir,
                children: children.iter().map(|(w, c)| (*w, c.skeleton())).collect(),
            },
        }
    }
}

/// Normalize a vector of weights so they sum to 1.0. If the sum is zero or
/// negative, distributes uniformly. Single-element children stay [1.0].
fn renormalize(children: &mut [(f32, impl Sized)]) {
    let n = children.len();
    if n == 0 {
        return;
    }
    let sum: f32 = children.iter().map(|(w, _)| *w).sum();
    if sum <= 0.0 {
        let even = 1.0 / n as f32;
        for (w, _) in children.iter_mut() {
            *w = even;
        }
        return;
    }
    for (w, _) in children.iter_mut() {
        *w /= sum;
    }
}

/// Which edge of the tab a rail anchors to (spec-rail.md §10). Default `Left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RailSide {
    #[default]
    Left,
    Right,
}

/// Derived outline: heading entries from the focused window (spec-rail.md §13).
/// `entries` is `(heading depth 1–6, display text, block index or line
/// number)`. Re-derived on the render frames where the focused window changed.
pub struct OutlineState {
    pub entries: Vec<(u8, String, usize)>,
    pub selected: usize,
    /// Change-key the `entries` were derived at (focused window id + that
    /// window's content version). The render loop re-derives the outline only
    /// when this changes — otherwise re-derivation was O(document) per frame
    /// (and per keystroke, via `full_text()`, with the outline rail open).
    pub last_key: Option<u64>,
}

impl OutlineState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            last_key: None,
        }
    }
}

impl Default for OutlineState {
    fn default() -> Self {
        Self::new()
    }
}

/// What the rail is showing (spec-rail.md "Data model"). Extend this enum to
/// add new rail kinds — each variant needs a render arm in `render_rail` and a
/// toggle binding. No framework changes required.
pub enum RailContent {
    FileBrowser(FileBrowser),
    Outline(OutlineState),
}

impl RailContent {
    /// Stable discriminant used for toggle equality (open-same-kind closes,
    /// open-different-kind replaces) without exposing the inner state.
    pub fn is_file_browser(&self) -> bool {
        matches!(self, RailContent::FileBrowser(_))
    }

    pub fn is_outline(&self) -> bool {
        matches!(self, RailContent::Outline(_))
    }
}

/// Default rail width in px (spec-rail.md §9). v1 has no drag-to-resize, but
/// the value is stored so a future resize handle can mutate it in place.
pub const RAIL_DEFAULT_WIDTH: f32 = 200.0;

/// Per-tab rail state (spec-rail.md "Data model"). A tab has at most one rail
/// open at a time; opening a different kind replaces it in place.
pub struct RailState {
    pub content: RailContent,
    pub side: RailSide,
    /// Column width in px. Default [`RAIL_DEFAULT_WIDTH`]. Stored for a future
    /// drag-resize handle; v1 does not expose one.
    pub width_px: f32,
    /// True when the rail div holds `track_focus`. When false, the main
    /// content leaf holds focus as usual (two-state model, spec §5).
    pub focused: bool,
    /// The leaf the rail was opened from. The rail stays visually pinned to
    /// this leaf even when focus moves to another split tile.
    pub pinned_to: WindowId,
}

impl RailState {
    pub fn new(content: RailContent, side: RailSide, pinned_to: WindowId) -> Self {
        Self {
            content,
            side,
            width_px: RAIL_DEFAULT_WIDTH,
            focused: true,
            pinned_to,
        }
    }
}

// ---------------------------------------------------------------------------
// Marks (spec-layout-patterns.md Phase 1)
// ---------------------------------------------------------------------------

/// Workspace-global mark table. Maps single characters to `WindowId`s.
/// Stored on `Workspace<C>`, not per-tab — marks span all tabs.
pub struct MarkTable {
    marks: HashMap<char, WindowId>,
    /// The window where the last text edit occurred (special mark `.`).
    pub last_edit: Option<WindowId>,
    /// The window focused before the last cross-tab jump (special mark `'`).
    pub prev_jump: Option<WindowId>,
}

impl MarkTable {
    pub fn new() -> Self {
        Self {
            marks: HashMap::new(),
            last_edit: None,
            prev_jump: None,
        }
    }

    /// Set a user mark. Keys `.` and `'` are reserved for special marks.
    pub fn set(&mut self, key: char, id: WindowId) {
        if key != '.' && key != '\'' {
            self.marks.insert(key, id);
        }
    }

    /// Look up a mark. Dispatches `.` and `'` to the special fields.
    pub fn get(&self, key: char) -> Option<WindowId> {
        match key {
            '.' => self.last_edit,
            '\'' => self.prev_jump,
            c => self.marks.get(&c).copied(),
        }
    }

    /// Reverse lookup: given a window id, return the first mark char pointing
    /// at it (for rendering the `[a]` badge). Returns `None` if unmarked.
    pub fn mark_for_window(&self, id: WindowId) -> Option<char> {
        self.marks
            .iter()
            .find(|&(_, &wid)| wid == id)
            .map(|(&k, _)| k)
    }

    /// Return all user marks as a sorted list.
    pub fn all_marks(&self) -> Vec<(char, WindowId)> {
        let mut out: Vec<_> = self.marks.iter().map(|(&k, &v)| (k, v)).collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }

    /// Remove marks pointing to windows not in `live_ids`.
    pub fn gc(&mut self, live_ids: &HashSet<WindowId>) {
        self.marks.retain(|_, id| live_ids.contains(id));
        if let Some(id) = self.last_edit
            && !live_ids.contains(&id)
        {
            self.last_edit = None;
        }
        if let Some(id) = self.prev_jump
            && !live_ids.contains(&id)
        {
            self.prev_jump = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Automatic layouts (spec-layout-patterns.md Phase 2)
// ---------------------------------------------------------------------------

/// Layout mode for a tab. `Manual` is the default (user-built split tree);
/// the automatic modes compute the tree algorithmically on each structural
/// change (split/close/mode-switch). `Desktop` (spec-desktop-mode.md) keeps
/// the tree as the CONTENT owner and takes geometry from the tab's
/// [`DesktopState`] slot map instead — it never drains/rebuilds the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Manual,
    MasterStack,
    Monocle,
    Columns,
    Desktop,
}

/// Hand-rolled deserialize with an unknown-variant fallback to `Manual`
/// (spec-desktop-mode.md Behavior 7): the workspace snapshot loader treats a
/// failed parse as "no snapshot", so a derived deserializer meeting a mode
/// string from a NEWER binary would discard — and on next save overwrite —
/// the user's whole arrangement. Falling back degrades one tab's layout mode
/// instead. Keep in sync with the variant list (a test enforces round-trip).
impl<'de> serde::Deserialize<'de> for LayoutMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "master_stack" => LayoutMode::MasterStack,
            "monocle" => LayoutMode::Monocle,
            "columns" => LayoutMode::Columns,
            "desktop" => LayoutMode::Desktop,
            // "manual" and any string from the future
            _ => LayoutMode::Manual,
        })
    }
}

impl LayoutMode {
    /// Cycle to the next mode:
    /// Manual → MasterStack → Monocle → Columns → Desktop → Manual.
    pub fn cycle(self) -> Self {
        match self {
            LayoutMode::Manual => LayoutMode::MasterStack,
            LayoutMode::MasterStack => LayoutMode::Monocle,
            LayoutMode::Monocle => LayoutMode::Columns,
            LayoutMode::Columns => LayoutMode::Desktop,
            LayoutMode::Desktop => LayoutMode::Manual,
        }
    }

    /// Short sigil for the status bar (spec-layout-patterns.md Behavior 16).
    pub fn sigil(&self) -> &'static str {
        match self {
            LayoutMode::Manual => "[]=",
            LayoutMode::MasterStack => "[M]=",
            LayoutMode::Monocle => "[M]", // caller computes [n/N] dynamically
            LayoutMode::Columns => "|||",
            LayoutMode::Desktop => "[#]",
        }
    }
}

// ---------------------------------------------------------------------------
// Desktop mode (spec-desktop-mode.md)
// ---------------------------------------------------------------------------

/// A cell address on the unbounded desktop grid. Origin top-left; growth is
/// rightward/downward (no negative coordinates). Ordered row-major — the
/// derived `(row, col)` lexicographic `Ord` IS the sequence order that
/// insert-and-shift and `focus_next/prev` operate on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot {
    pub row: u32,
    pub col: u32,
}

impl Slot {
    pub fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// Row-major successor under the W-wrapped chain (spec Behavior 4):
    /// `(row, col+1)` while `col + 1 < w`, else `(row+1, 0)`. Slots at
    /// `col >= w` are OUTSIDE every successor chain by construction — ripples
    /// never touch them.
    pub fn succ(self, w: u32) -> Slot {
        let w = w.max(1);
        if self.col + 1 < w {
            Slot::new(self.row, self.col + 1)
        } else {
            Slot::new(self.row + 1, 0)
        }
    }
}

/// Transient drag state while a tile is being dragged. View-layer units are
/// plain `f32` pixels in DESKTOP coordinates (pre-pan); the view converts at
/// the boundary so this module stays gpui-free. Never persisted.
#[derive(Debug, Clone, Copy)]
pub struct DesktopDrag {
    /// The tile being dragged.
    pub id: WindowId,
    /// Pointer offset within the tile at grab time (so the ghost doesn't
    /// jump to the pointer corner).
    pub grab: (f32, f32),
    /// Current pointer position in desktop coordinates.
    pub pointer: (f32, f32),
    /// Resolved drop target for the current pointer (recomputed on move).
    pub target: Option<Slot>,
    /// True once the pointer has moved past the click threshold (~4px).
    /// Below it, mouse-up is a plain focus click — no ghost, no drop.
    pub active: bool,
}

/// A tile's extent in slots (spec-desktop-mode Behavior 4b). Each axis ≥ 1;
/// default 1 × 1. A tile at anchor `(r, c)` with span `(rows, cols)` occupies
/// the rectangle `[r, r+rows) × [c, c+cols)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub rows: u32,
    pub cols: u32,
}

impl Span {
    pub const ONE: Span = Span { rows: 1, cols: 1 };

    pub fn new(rows: u32, cols: u32) -> Self {
        Self {
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::ONE
    }
}

/// Which edge a desktop resize drag is pulling (spec Behavior 4b). v1 grows
/// east/south only, so the tile's anchor never moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    East,
    South,
}

/// Transient edge-resize gesture. Like `DesktopDrag`: plain pixels in desktop
/// coordinates (pre-pan); never persisted.
#[derive(Debug, Clone, Copy)]
pub struct DesktopResize {
    /// The tile being resized.
    pub id: WindowId,
    /// Which edge is being pulled.
    pub edge: ResizeEdge,
    /// Current pointer position in desktop coordinates.
    pub pointer: (f32, f32),
}

/// Per-tab desktop-mode geometry (spec-desktop-mode.md). The layout tree
/// remains the content owner; this owns ONLY placement. Invariant
/// (Behavior 2): exactly one entry per tree leaf, no two entries share a
/// slot — maintained by [`reconcile`](Self::reconcile), which callers run on
/// mode entry and structural changes (the render path runs it every frame;
/// it is O(n) and a no-op when the invariant already holds).
#[derive(Debug, Default)]
pub struct DesktopState {
    /// Placement AND sequence: sorted by `Slot` (row-major), one entry per
    /// leaf. Kept sorted by every mutator so "the sequence" is never a second
    /// piece of state that can drift.
    pub slots: Vec<(WindowId, Slot)>,
    /// Per-tile extent (spec Behavior 4b); absent = 1 × 1. Sparse: only tiles
    /// grown past 1 × 1 appear here, so the common case stays empty.
    pub spans: HashMap<WindowId, Span>,
    /// Viewport pan over the desktop, in pixels. Transient-but-kept across
    /// mode switches; not persisted.
    pub pan: (f32, f32),
    /// Live drag, if any.
    pub drag: Option<DesktopDrag>,
    /// Live edge resize, if any (spec Behavior 4b).
    pub resize: Option<DesktopResize>,
    /// The window the auto-pan last revealed. The render path pans to the
    /// focused tile only when focus CHANGED since the last frame, so a
    /// manual pan away from the focused tile isn't fought every frame.
    pub last_reveal: Option<WindowId>,
}

impl DesktopState {
    pub fn slot_of(&self, id: WindowId) -> Option<Slot> {
        self.slots.iter().find(|(w, _)| *w == id).map(|&(_, s)| s)
    }

    /// The tile whose RECTANGLE covers `slot` (rectangle-aware, spec
    /// Behavior 4b). With all spans 1 × 1 this is just the anchor match.
    pub fn occupant(&self, slot: Slot) -> Option<WindowId> {
        self.slots.iter().find_map(|&(id, anchor)| {
            let sp = self.span_of(id);
            let covers = slot.row >= anchor.row
                && slot.row < anchor.row + sp.rows
                && slot.col >= anchor.col
                && slot.col < anchor.col + sp.cols;
            covers.then_some(id)
        })
    }

    /// A tile's extent; absent from the map = 1 × 1.
    pub fn span_of(&self, id: WindowId) -> Span {
        self.spans.get(&id).copied().unwrap_or(Span::ONE)
    }

    /// Anchor + span, or `None` if the tile has no slot.
    pub fn rect_of(&self, id: WindowId) -> Option<(Slot, Span)> {
        self.slot_of(id).map(|s| (s, self.span_of(id)))
    }

    /// Commit a tile's span, keeping the map sparse (1 × 1 stores nothing).
    pub fn set_span(&mut self, id: WindowId, span: Span) {
        if span == Span::ONE {
            self.spans.remove(&id);
        } else {
            self.spans.insert(id, span);
        }
    }

    /// True if every slot in the rectangle at `anchor` of `span` is free of
    /// any tile other than `exclude`.
    fn rect_free(&self, anchor: Slot, span: Span, exclude: WindowId) -> bool {
        for dr in 0..span.rows {
            for dc in 0..span.cols {
                let cell = Slot::new(anchor.row + dr, anchor.col + dc);
                if matches!(self.occupant(cell), Some(id) if id != exclude) {
                    return false;
                }
            }
        }
        true
    }

    /// Largest span reachable by growing `id`'s `edge` toward `desired` whole
    /// slots, clamped so no other tile is overlapped (Block rule, spec
    /// Behavior 4b). Shrinking is always allowed to the 1 × 1 minimum; the
    /// other axis is held at its current value.
    pub fn clamp_span(&self, id: WindowId, edge: ResizeEdge, desired: u32) -> Span {
        let cur = self.span_of(id);
        let Some(anchor) = self.slot_of(id) else {
            return cur;
        };
        let desired = desired.max(1);
        let mut ext = 1;
        while ext < desired {
            let cand = match edge {
                ResizeEdge::East => Span::new(cur.rows, ext + 1),
                ResizeEdge::South => Span::new(ext + 1, cur.cols),
            };
            if self.rect_free(anchor, cand, id) {
                ext += 1;
            } else {
                break;
            }
        }
        match edge {
            ResizeEdge::East => Span::new(cur.rows, ext),
            ResizeEdge::South => Span::new(ext, cur.cols),
        }
    }

    fn sort(&mut self) {
        self.slots.sort_by_key(|&(_, s)| s);
    }

    /// First unoccupied slot in W-wrapped chain order starting at the origin.
    fn first_free(&self, w: u32) -> Slot {
        let mut s = Slot::new(0, 0);
        while self.occupant(s).is_some() {
            s = s.succ(w);
        }
        s
    }

    /// First-entry placement (spec Behavior 1): leaves in tree order, row-
    /// major at effective width `w`. Replaces any existing map.
    pub fn seed(&mut self, leaves: &[WindowId], w: u32) {
        self.slots.clear();
        let mut s = Slot::new(0, 0);
        for &id in leaves {
            self.slots.push((id, s));
            s = s.succ(w);
        }
        // Built in chain order = already sorted.
    }

    /// Restore the Behavior-2 invariant: drop entries whose window is gone
    /// (their slot becomes a gap — neighbors never move); give every slotless
    /// leaf a placement after the focused tile (insert-and-shift), or at the
    /// first free slot when the focused tile has no slot yet. Returns true
    /// if anything changed.
    pub fn reconcile(&mut self, leaves: &[WindowId], focused: WindowId, w: u32) -> bool {
        let mut changed = false;
        let before = self.slots.len();
        self.slots.retain(|(id, _)| leaves.contains(id));
        changed |= self.slots.len() != before;
        // Drop spans whose tile is gone (its rectangle becomes gaps).
        let spans_before = self.spans.len();
        self.spans.retain(|id, _| leaves.contains(id));
        changed |= self.spans.len() != spans_before;

        for &leaf in leaves {
            if self.slot_of(leaf).is_some() {
                continue;
            }
            // New leaves are always 1 × 1. Prefer the slot after the focused
            // tile; if that insertion is wall-rejected (Behavior 4b), fall back
            // to the first free slot, which is guaranteed to accept.
            let target = match self.slot_of(focused) {
                Some(f) => f.succ(w),
                None => self.first_free(w),
            };
            if !self.insert_shift(leaf, target, w) {
                let free = self.first_free(w);
                self.insert_shift(leaf, free, w);
            }
            changed = true;
        }
        changed
    }

    /// The Behavior-4 drop, rectangle-aware (Behavior 4b). Returns whether
    /// `id` was placed at `target`.
    ///
    /// A 1 × 1 tile inserts via the W-wrapped run, which collects only 1 × 1
    /// occupants; a multi-slot tile (and a run that never meets a gap) is a
    /// **wall** — a wall-blocked insertion is REJECTED and `id` stays at its
    /// original slot. A multi-slot tile is placed only when its whole
    /// rectangle is free; otherwise rejected. Either outcome preserves the
    /// non-overlapping-rectangles invariant.
    pub fn insert_shift(&mut self, id: WindowId, target: Slot, w: u32) -> bool {
        let orig = self.slot_of(id);
        let span = self.span_of(id);
        // Tentatively vacate so `id` never collides with itself.
        self.slots.retain(|(wid, _)| *wid != id);

        let placed = if span != Span::ONE {
            // Spanned tile: place only if the whole rectangle is free.
            if self.rect_free(target, span, id) {
                self.slots.push((id, target));
                true
            } else {
                false
            }
        } else if let Some(run) = self.absorbable_run(target, w) {
            // Shift the run back-to-front so no two tiles collide mid-flight.
            for &(occ, from) in run.iter().rev() {
                if let Some(entry) = self.slots.iter_mut().find(|(wid, _)| *wid == occ) {
                    entry.1 = from.succ(w);
                }
            }
            self.slots.push((id, target));
            true
        } else {
            false
        };

        if !placed {
            if let Some(o) = orig {
                self.slots.push((id, o)); // drop rejected — restore.
            }
        }
        self.sort();
        placed
    }

    /// The contiguous 1 × 1 occupied run starting at `target` along the
    /// W-wrapped chain, IF a gap absorbs it. `None` when the run meets a
    /// **wall** (a multi-slot tile) before any gap — an unabsorbable
    /// insertion. Terminates: `succ` strictly increases in row-major order
    /// and the tiles are finite, so a gap is always reached.
    fn absorbable_run(&self, target: Slot, w: u32) -> Option<Vec<(WindowId, Slot)>> {
        let mut run: Vec<(WindowId, Slot)> = Vec::new();
        let mut s = target;
        loop {
            match self.occupant(s) {
                None => return Some(run), // gap absorbs the ripple
                Some(occ) => {
                    if self.span_of(occ) != Span::ONE {
                        return None; // multi-slot wall
                    }
                    run.push((occ, s));
                    s = s.succ(w);
                }
            }
        }
    }

    /// Spatial focus navigation (spec Behavior 5): nearest occupied slot
    /// strictly in `direction` from `from`'s slot — primary-axis-aligned
    /// candidates first, then squared slot distance, then row-major order as
    /// the deterministic tiebreak.
    pub fn spatial_neighbor(&self, from: WindowId, direction: SpatialDir) -> Option<WindowId> {
        let origin = self.slot_of(from)?;
        self.slots
            .iter()
            .filter(|&&(id, s)| {
                id != from
                    && match direction {
                        SpatialDir::Left => s.col < origin.col,
                        SpatialDir::Right => s.col > origin.col,
                        SpatialDir::Up => s.row < origin.row,
                        SpatialDir::Down => s.row > origin.row,
                    }
            })
            .min_by_key(|&&(_, s)| {
                let dr = s.row.abs_diff(origin.row) as u64;
                let dc = s.col.abs_diff(origin.col) as u64;
                let aligned = match direction {
                    SpatialDir::Left | SpatialDir::Right => dr == 0,
                    SpatialDir::Up | SpatialDir::Down => dc == 0,
                };
                (if aligned { 0u64 } else { 1 }, dr * dr + dc * dc, s)
            })
            .map(|&(id, _)| id)
    }

    /// Row-major sequence neighbor for `focus_next` / `focus_prev`.
    pub fn sequence_neighbor(&self, from: WindowId, forward: bool) -> Option<WindowId> {
        let idx = self.slots.iter().position(|&(id, _)| id == from)?;
        let n = self.slots.len();
        if n < 2 {
            return None;
        }
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        Some(self.slots[next].0)
    }

    /// Bounding box of occupied slots: `(max_row, max_col)` inclusive, or
    /// `None` when empty. The pan clamp allows one slot of margin beyond it.
    pub fn occupied_extent(&self) -> Option<(u32, u32)> {
        let mut max: Option<(u32, u32)> = None;
        for &(id, s) in &self.slots {
            let sp = self.span_of(id);
            // Inclusive far corner of the tile's rectangle.
            let corner = (s.row + sp.rows - 1, s.col + sp.cols - 1);
            max = Some(match max {
                Some((mr, mc)) => (mr.max(corner.0), mc.max(corner.1)),
                None => corner,
            });
        }
        max
    }
}

/// Direction for desktop spatial focus navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialDir {
    Left,
    Right,
    Up,
    Down,
}

// Pure geometry (spec Interfaces). Desktop coordinates are pre-pan pixels;
// the view layer applies pan and converts to gpui units at its boundary.

/// Top-left of `slot`, given the tile pixel size and gutter.
pub fn slot_origin(slot: Slot, tile: (f32, f32), gutter: f32) -> (f32, f32) {
    (
        gutter + slot.col as f32 * (tile.0 + gutter),
        gutter + slot.row as f32 * (tile.1 + gutter),
    )
}

/// Pixel rect of a tile: anchor origin plus span extent (spec Behavior 3 /
/// 4b). Returns `(x, y, w, h)` in desktop coordinates; a 1 × 1 span is exactly
/// one `tile`-sized cell.
pub fn tile_rect(slot: Slot, span: Span, tile: (f32, f32), gutter: f32) -> (f32, f32, f32, f32) {
    let (x, y) = slot_origin(slot, tile, gutter);
    let w = span.cols as f32 * (tile.0 + gutter) - gutter;
    let h = span.rows as f32 * (tile.1 + gutter) - gutter;
    (x, y, w, h)
}

/// The slot whose cell contains (or is nearest to) a desktop-coordinate
/// point. Clamps to the non-negative grid.
pub fn slot_at(point: (f32, f32), tile: (f32, f32), gutter: f32) -> Slot {
    let cell = (tile.0 + gutter, tile.1 + gutter);
    let col = ((point.0 - gutter) / cell.0).floor().max(0.0) as u32;
    let row = ((point.1 - gutter) / cell.1).floor().max(0.0) as u32;
    Slot::new(row, col)
}

/// Drop-time effective width W (spec Overview): tile columns that fit the
/// viewport, minimum 1. Stored slots are never re-derived from it.
pub fn effective_width(viewport_w: f32, tile_w: f32, gutter: f32) -> u32 {
    (((viewport_w - gutter) / (tile_w + gutter)).floor() as i64).max(1) as u32
}

/// A skeleton of a layout tree — just the shape (window ids, weights, split
/// dirs) without the actual content. Used to save the manual tree when
/// switching to an automatic mode, so it can be restored later.
#[derive(Debug, Clone)]
pub enum LayoutSkeleton {
    Empty,
    Leaf(WindowId),
    Split {
        dir: SplitDir,
        children: Vec<(f32, LayoutSkeleton)>,
    },
}

// ---------------------------------------------------------------------------
// Tags (spec-layout-patterns.md Phase 3)
// ---------------------------------------------------------------------------

/// A set of user-assigned tag names.
pub type TagSet = BTreeSet<String>;

/// One tab in the workspace's tab strip.
///
/// User-facing name: **"Workspace"**. The product presents each `Tab` as a
/// named, swappable desktop (its own split layout, focus, and rail); the
/// container that owns the list of these is `Workspace<C>` (an unfortunate
/// internal name collision — a full rename is out of scope, so user-facing
/// strings say "workspace" while the type stays `Tab`). See
/// `docs/specs/spec-workspaces-tagging.md` (Phase 1).
pub struct Tab<C> {
    /// Monotonic auto-name set at create (e.g. "workspace-1"). Survives rename so
    /// the user can clear `display_name` and recover the auto-name.
    pub auto_name: String,
    /// User-set display name. `None` means render `auto_name`.
    pub display_name: Option<String>,
    pub layout: Layout<C>,
    pub focused: WindowId,
    /// Persistent side column (spec-rail.md). `None` when no rail is open.
    /// Per-tab — switching tabs shows/hides the arriving/departing tab's rail.
    pub rail: Option<RailState>,
    // --- Layout patterns (spec-layout-patterns.md) ---
    /// Current layout algorithm. `Manual` is the default (hand-built splits).
    pub layout_mode: LayoutMode,
    /// Saved manual tree shape. When switching from Manual to an automatic
    /// mode, the manual tree's skeleton is saved here so it can be restored.
    pub saved_manual_layout: Option<LayoutSkeleton>,
    /// MasterStack: fraction of tab width allocated to the master region.
    pub master_ratio: f32,
    /// MasterStack: number of windows in the master region.
    pub master_count: usize,
    /// Tag-view filter. When non-empty, the tab shows only windows whose
    /// buffer carries at least one tag in this set. Empty = show all.
    pub tag_view: TagSet,
    /// Desktop-mode placement (spec-desktop-mode.md). Geometry only — the
    /// layout tree above remains the content owner. Kept (not cleared) when
    /// switching away from Desktop so the arrangement survives round-trips.
    pub desktop: DesktopState,
}

impl<C> Tab<C> {
    pub fn display_label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.auto_name)
    }
}

/// A pooled file-backed editor. One `FileBuffer` per canonical path; refcount
/// tracks the number of `EditorView`s in the workspace currently bound to it.
pub struct FileBuffer {
    // unused accessor variant — pool liveness goes through open_and_retain /
    // buffer_core / gc_buffers (5c). Kept for API symmetry. See ADR-0005/0007.
    #[allow(dead_code)]
    pub id: FileBufferId,
    pub canonical_path: PathBuf,
    /// Shared, reference-counted core. Cloned into each `EditorView`/window
    /// that binds to this buffer so edits + undo are shared while the pool
    /// retains its own handle for lookups, modified-checks, and save.
    pub core: SharedCore,
    // unused accessor variant — pool liveness goes through open_and_retain /
    // buffer_core / gc_buffers (5c). Kept for API symmetry. See ADR-0005/0007.
    #[allow(dead_code)]
    pub file_label: String,
    /// Active EditorViews referencing this core.
    pub refcount: usize,
    /// User-assigned tags (spec-layout-patterns.md Phase 3). Tags live on the
    /// pooled buffer so they survive window close/reopen and are shared across
    /// all views of the same file.
    pub tags: TagSet,
}

/// Top-level container for the GPUI frontend. Owns the tab strip, the file-
/// buffer pool, and the active-tab pointer. Replaces today's
/// `GpuiApp.screen` + `GpuiApp.open_buffers` + `GpuiApp.active_buffer_idx` triple.
pub struct Workspace<C> {
    pub tabs: Vec<Tab<C>>,
    pub active_tab: usize,
    pub file_buffers: HashMap<FileBufferId, FileBuffer>,
    /// Canonical path → buffer id, for pool lookups during open.
    pub path_index: HashMap<PathBuf, FileBufferId>,
    pub next_buffer_id: u64,
    pub next_window_id: u64,
    /// Monotonic counter feeding `Tab::auto_name`. Bumped on each tab create.
    pub next_tab_index: usize,
    // --- Layout patterns (spec-layout-patterns.md) ---
    /// Workspace-global mark table (Phase 1). Cross-tab bookmarks on windows.
    pub marks: MarkTable,
    /// Shortcut keys bound to tag names (Phase 3). `Ctrl-W t {key}` views
    /// the tag mapped to `{key}`.
    pub tag_shortcuts: HashMap<char, String>,
}

impl<C> Workspace<C> {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab: 0,
            file_buffers: HashMap::new(),
            path_index: HashMap::new(),
            next_buffer_id: 1,
            next_window_id: 1,
            next_tab_index: 1,
            marks: MarkTable::new(),
            tag_shortcuts: HashMap::new(),
        }
    }

    pub fn active_tab(&self) -> Option<&Tab<C>> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab<C>> {
        self.tabs.get_mut(self.active_tab)
    }

    /// Borrow the focused window's content (None if no tab, or the tab's
    /// focused id is missing from the layout — invariant violation).
    pub fn focused_content(&self) -> Option<&C> {
        let tab = self.active_tab()?;
        tab.layout.find_leaf(tab.focused).map(|w| &w.content)
    }

    /// Mutably borrow the focused window's content.
    pub fn focused_content_mut(&mut self) -> Option<&mut C> {
        let tab = self.active_tab_mut()?;
        let focused = tab.focused;
        tab.layout.find_leaf_mut(focused).map(|w| &mut w.content)
    }

    /// Replace the focused window's content in place. Returns the old value
    /// (or None if there's no focused window).
    pub fn replace_focused_content(&mut self, content: C) -> Option<C> {
        let tab = self.active_tab_mut()?;
        let focused = tab.focused;
        let win = tab.layout.find_leaf_mut(focused)?;
        Some(std::mem::replace(&mut win.content, content))
    }

    /// The id of the focused window (or None if the workspace has no tabs).
    pub fn focused_window_id(&self) -> Option<WindowId> {
        self.active_tab().map(|t| t.focused)
    }

    /// Construct a workspace pre-populated with one tab containing one
    /// window of `content`. Tab name is auto-assigned to `tab-1`.
    pub fn with_initial(content: C) -> Self {
        let mut ws = Self::new();
        ws.push_initial_tab(content);
        ws
    }

    /// Append a new tab containing a single window with `content`. Becomes
    /// the active tab. Returns the new window's id.
    pub fn push_initial_tab(&mut self, content: C) -> WindowId {
        let id = self.alloc_window_id();
        let name = auto_tab_name(self.next_tab_index);
        self.next_tab_index += 1;
        self.tabs.push(Tab {
            auto_name: name,
            display_name: None,
            layout: Layout::Leaf(Window { id, content }),
            focused: id,
            rail: None,
            layout_mode: LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
        });
        self.active_tab = self.tabs.len() - 1;
        id
    }

    /// Close the tab at index `idx`. The active-tab pointer adjusts to stay
    /// in range; closing the last tab leaves the workspace with zero tabs
    /// (caller is responsible for the spec Behavior 2 placeholder).
    ///
    /// Agent tiles in the closed tab hold only a `SessionId` key — the session
    /// STATE lives in `YaldaGpuiView::sessions`. So closing a tab/window FREES
    /// (does not KILL) any agent session it showed: the session stays in the
    /// store, still running, re-bindable from another tile's selector. An
    /// explicit close (`claude-close`) is the only path that kills a session.
    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        } else if idx < self.active_tab {
            self.active_tab -= 1;
        }
    }

    /// Cycle to the next tab (wraps).
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
    }

    /// Cycle to the previous tab (wraps).
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
    }

    /// Allocate the next stable window id.
    pub fn alloc_window_id(&mut self) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    /// Allocate the next stable buffer id.
    fn alloc_buffer_id(&mut self) -> FileBufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        id
    }

    /// Canonical key for the buffer pool. Existing files canonicalize via
    /// `fs::canonicalize`; for paths that don't exist on disk yet (a new
    /// file), the absolute path with `.` / `..` collapsed is the key.
    pub fn canonical_key(path: &Path) -> PathBuf {
        if let Ok(c) = std::fs::canonicalize(path) {
            return c;
        }
        // Fall back: join cwd if relative, then collapse `.` and `..`.
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let mut out = PathBuf::new();
        for comp in abs.components() {
            match comp {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// Open (or pool-lookup) a file-backed `EditorCore`. Returns the buffer id.
    /// If the file exists on disk and is readable, its contents are loaded;
    /// otherwise an empty buffer is created at the given path (Behavior 21).
    pub fn open_buffer(&mut self, path: &Path) -> std::io::Result<FileBufferId> {
        let key = Self::canonical_key(path);
        if let Some(&id) = self.path_index.get(&key) {
            return Ok(id);
        }
        let text = match std::fs::read_to_string(&key) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let id = self.alloc_buffer_id();
        let file_label = key
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| key.display().to_string());
        let buf = FileBuffer {
            id,
            canonical_path: key.clone(),
            core: Rc::new(RefCell::new(EditorCore::new(text, key.clone()))),
            file_label,
            refcount: 0,
            tags: BTreeSet::new(),
        };
        self.file_buffers.insert(id, buf);
        self.path_index.insert(key, id);
        Ok(id)
    }

    // unused accessor variant — pool liveness goes through open_and_retain /
    // buffer_core / gc_buffers (5c). Kept for API symmetry. See ADR-0005/0007.
    #[allow(dead_code)]
    pub fn buffer(&self, id: FileBufferId) -> Option<&FileBuffer> {
        self.file_buffers.get(&id)
    }

    // unused accessor variant — pool liveness goes through open_and_retain /
    // buffer_core / gc_buffers (5c). Kept for API symmetry. See ADR-0005/0007.
    #[allow(dead_code)]
    pub fn buffer_mut(&mut self, id: FileBufferId) -> Option<&mut FileBuffer> {
        self.file_buffers.get_mut(&id)
    }

    /// Increment the refcount for a buffer when a new `EditorView` binds to
    /// it (split, content kind transition, restore, etc.).
    pub fn buffer_retain(&mut self, id: FileBufferId) {
        if let Some(b) = self.file_buffers.get_mut(&id) {
            b.refcount += 1;
        }
    }

    /// Clone the shared core handle for `id` (does NOT change the refcount).
    pub fn buffer_core(&self, id: FileBufferId) -> Option<SharedCore> {
        self.file_buffers.get(&id).map(|b| Rc::clone(&b.core))
    }

    /// Garbage-collect pooled buffers that no window still references. The
    /// pool itself holds one `Rc` strong ref per buffer; each live window's
    /// `SharedCore` clone holds another. So a `strong_count == 1` means no
    /// window is bound to it. Such a buffer is dropped from the pool unless it
    /// has unsaved changes (kept for `:buffers` recovery, mirroring
    /// [`buffer_release`]). This is the app's real liveness mechanism — views
    /// just drop their `Rc` on close and the next `gc_buffers` reaps the husk,
    /// so no manual release call has to be threaded through every close path.
    pub fn gc_buffers(&mut self) {
        let dead: Vec<FileBufferId> = self
            .file_buffers
            .iter()
            .filter(|(_, b)| {
                Rc::strong_count(&b.core) == 1 && !b.core.borrow().document().is_modified()
            })
            .map(|(&id, _)| id)
            .collect();
        for id in dead {
            if let Some(b) = self.file_buffers.remove(&id) {
                self.path_index.remove(&b.canonical_path);
            }
        }
    }

    /// Open (pool-lookup or load) `path`, bump its refcount, and hand back the
    /// buffer id plus a fresh clone of the shared core for a new window to
    /// bind to. The one-call path for creating a file-backed view: a window
    /// that holds the returned `(id, core)` must `buffer_release(id)` on close.
    pub fn open_and_retain(&mut self, path: &Path) -> std::io::Result<(FileBufferId, SharedCore)> {
        let id = self.open_buffer(path)?;
        self.buffer_retain(id);
        let core = self
            .buffer_core(id)
            .expect("buffer just opened must be present");
        Ok((id, core))
    }

    /// Decrement the refcount. Drops the buffer from the pool when refcount
    /// hits 0 AND it has no unsaved changes; dirty buffers stay pooled for
    /// recovery via `:buffers` (Behavior 21).
    // unused accessor variant — pool liveness goes through open_and_retain /
    // buffer_core / gc_buffers (5c). Kept for API symmetry. See ADR-0005/0007.
    #[allow(dead_code)]
    pub fn buffer_release(&mut self, id: FileBufferId) {
        let drop = if let Some(b) = self.file_buffers.get_mut(&id) {
            b.refcount = b.refcount.saturating_sub(1);
            b.refcount == 0 && !b.core.borrow().document().is_modified()
        } else {
            false
        };
        if drop && let Some(b) = self.file_buffers.remove(&id) {
            self.path_index.remove(&b.canonical_path);
        }
    }
}

impl<C> Workspace<C> {
    /// Insert a new window adjacent to the focused leaf in the active tab.
    /// Implements Behavior 12–13 of `spec-tabs-and-splits.md`:
    /// - If the focused leaf's parent split has the same `dir`, append the
    ///   new leaf right after the focused leaf (no nesting).
    /// - Otherwise (root leaf, or perpendicular parent), wrap the focused
    ///   leaf in a fresh 2-child split.
    ///
    /// The new window's weight initializes to the average of existing
    /// siblings; all weights renormalize to sum to 1.0. Focus moves to the
    /// new window. Returns the new window's id (or `None` if the workspace
    /// has no active tab).
    pub fn split_focused(&mut self, dir: SplitDir, content: C) -> Option<WindowId> {
        let new_id = self.alloc_window_id();
        let new_window = Window {
            id: new_id,
            content,
        };
        let tab = self.active_tab_mut()?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused)?;

        if path.is_empty() {
            // Root is the focused leaf. Wrap it in a Split with the new leaf.
            let old_root = std::mem::take(&mut tab.layout);
            tab.layout = Layout::Split {
                dir,
                children: vec![(0.5, old_root), (0.5, Layout::Leaf(new_window))],
            };
        } else {
            // Walk to the parent of the focused leaf.
            let (parent_path, tail) = path.split_at(path.len() - 1);
            let leaf_idx = tail[0];
            let parent = tab.layout.node_at_path_mut(parent_path)?;
            let Layout::Split {
                dir: parent_dir,
                children,
            } = parent
            else {
                return None;
            };
            if *parent_dir == dir {
                // Same direction — insert adjacent to the focused leaf.
                let avg = if children.is_empty() {
                    1.0
                } else {
                    children.iter().map(|(w, _)| *w).sum::<f32>() / children.len() as f32
                };
                children.insert(leaf_idx + 1, (avg, Layout::Leaf(new_window)));
                renormalize(children);
            } else {
                // Perpendicular — wrap the focused leaf in a nested Split.
                let (old_weight, old_leaf) = children
                    .get_mut(leaf_idx)
                    .map(|(w, l)| (*w, std::mem::take(l)))?;
                let nested = Layout::Split {
                    dir,
                    children: vec![(0.5, old_leaf), (0.5, Layout::Leaf(new_window))],
                };
                children[leaf_idx] = (old_weight, nested);
            }
        }
        tab.focused = new_id;
        Some(new_id)
    }

    /// Close the focused window. Returns:
    /// - `Ok(Some(new_focus))` — close succeeded, focus moved to a sibling.
    /// - `Ok(None)` — the focused window was the last in the tab; the caller
    ///   should close the tab (or replace it with a placeholder per spec
    ///   Behavior 2).
    /// - `Err(())` — no active tab / no focused window.
    pub fn close_focused(&mut self) -> Result<Option<WindowId>, ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;

        if path.is_empty() {
            // Focused leaf IS the root. The tab has nothing left.
            return Ok(None);
        }

        let (parent_path, tail) = path.split_at(path.len() - 1);
        let leaf_idx = tail[0];
        let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
        let Layout::Split { children, .. } = parent else {
            return Err(());
        };
        children.remove(leaf_idx);

        // Pick focus successor: previous index in this split, else first
        // remaining child, else (if split is now single-child) the inner leaf.
        let new_focus = if children.is_empty() {
            // Shouldn't happen — invariant says split had >= 2 children.
            return Err(());
        } else if leaf_idx > 0 {
            children[leaf_idx - 1].1.leaf_ids().last().copied()
        } else {
            children[0].1.leaf_ids().first().copied()
        };

        let collapse = children.len() == 1;
        if collapse {
            // Replace this split with its sole remaining child.
            let only_child = std::mem::take(&mut children[0].1);
            let parent_slot = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
            *parent_slot = only_child;
        } else {
            // Renormalize the remaining siblings.
            renormalize(children);
        }

        if let Some(id) = new_focus {
            tab.focused = id;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Detach the focused window from the active tab's layout, returning the
    /// owned `Window<C>` (content travels with it — no cloning). Used by the
    /// "move tile to workspace" verb.
    ///
    /// Returns `Ok((window, source_now_empty))`:
    /// - `window` — the relocated leaf, ready to insert elsewhere.
    /// - `source_now_empty` — true when the focused leaf was the tab's root,
    ///   so the active tab's layout is left as `Layout::Empty` and the caller
    ///   should remove the tab (or leave it if it's the only one).
    ///
    /// On the non-root case the split is pruned exactly like `close_focused`
    /// (collapse single-child splits, renormalize, re-focus a sibling).
    ///
    /// `Err(())` — no active tab / no focused window.
    pub fn detach_focused(&mut self) -> Result<(Window<C>, bool), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;

        if path.is_empty() {
            // Focused leaf is the root — take it, leave the tab empty.
            let root = std::mem::take(&mut tab.layout);
            let Layout::Leaf(window) = root else {
                // path_to said empty path means root is the target leaf, so
                // this is unreachable; restore and bail defensively.
                tab.layout = root;
                return Err(());
            };
            return Ok((window, true));
        }

        let (parent_path, tail) = path.split_at(path.len() - 1);
        let leaf_idx = tail[0];
        let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
        let Layout::Split { children, .. } = parent else {
            return Err(());
        };
        let (_w, removed) = {
            let entry = children.remove(leaf_idx);
            (entry.0, entry.1)
        };
        let Layout::Leaf(window) = removed else {
            // The focused path pointed at a Split, not a Leaf — shouldn't
            // happen for a focused window id.
            return Err(());
        };

        // Re-focus a sibling (same successor rule as close_focused).
        let new_focus = if children.is_empty() {
            return Err(());
        } else if leaf_idx > 0 {
            children[leaf_idx - 1].1.leaf_ids().last().copied()
        } else {
            children[0].1.leaf_ids().first().copied()
        };

        let collapse = children.len() == 1;
        if collapse {
            let only_child = std::mem::take(&mut children[0].1);
            let parent_slot = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
            *parent_slot = only_child;
        } else {
            renormalize(children);
        }

        if let Some(id) = new_focus {
            tab.focused = id;
        }
        Ok((window, false))
    }

    /// Append `window` as a new leaf in the tab at `tab_idx`, focusing it
    /// there. The window keeps its existing `id` (ids are workspace-unique).
    ///
    /// Placement mirrors `split_focused`'s root case: if the target layout is
    /// a single leaf, it is wrapped in a vertical `Split` so the arriving tile
    /// sits beside it; if it is already a `Split`, the leaf is appended as a
    /// new child and weights renormalize; an `Empty` target (a tab whose sole
    /// tile was just moved away) simply adopts the leaf as its root.
    ///
    /// Returns `Err(())` if `tab_idx` is out of range.
    pub fn insert_leaf_into_tab(&mut self, tab_idx: usize, window: Window<C>) -> Result<(), ()> {
        let id = window.id;
        let tab = self.tabs.get_mut(tab_idx).ok_or(())?;
        let root = std::mem::take(&mut tab.layout);
        tab.layout = match root {
            Layout::Empty => Layout::Leaf(window),
            Layout::Leaf(existing) => Layout::Split {
                dir: SplitDir::V,
                children: vec![(0.5, Layout::Leaf(existing)), (0.5, Layout::Leaf(window))],
            },
            Layout::Split { dir, mut children } => {
                let avg = if children.is_empty() {
                    1.0
                } else {
                    children.iter().map(|(w, _)| *w).sum::<f32>() / children.len() as f32
                };
                children.push((avg, Layout::Leaf(window)));
                renormalize(&mut children);
                Layout::Split { dir, children }
            }
        };
        tab.focused = id;
        Ok(())
    }

    /// Close every window in the active tab except the focused one. The
    /// focused leaf becomes the tab's root. Returns `Err(())` if there is no
    /// focused window.
    pub fn only(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;
        if path.is_empty() {
            return Ok(()); // Already the root.
        }
        // Take the focused leaf out of the tree, replace root with it.
        let mut focused_leaf: Option<Layout<C>> = None;
        // Extract via walk: get to parent, swap leaf out with Empty.
        let (parent_path, tail) = path.split_at(path.len() - 1);
        let leaf_idx = tail[0];
        let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
        if let Layout::Split { children, .. } = parent {
            let (_w, l) = children.get_mut(leaf_idx).ok_or(())?;
            focused_leaf = Some(std::mem::take(l));
        }
        let leaf = focused_leaf.ok_or(())?;
        tab.layout = leaf;
        Ok(())
    }

    /// Shift weight between the focused leaf and its immediate next sibling
    /// inside the parent `Split`. `delta` is added to the focused leaf's
    /// weight and subtracted from the sibling's; both clamp to a 5%/95%
    /// floor/ceiling per slot. No-op if the focused leaf has no sibling in
    /// the requested direction.
    pub fn resize_focused(&mut self, delta: f32) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;
        if path.is_empty() {
            return Ok(()); // No parent to resize against.
        }
        let (parent_path, tail) = path.split_at(path.len() - 1);
        let leaf_idx = tail[0];
        let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
        let Layout::Split { children, .. } = parent else {
            return Err(());
        };
        let sibling_idx = if leaf_idx + 1 < children.len() {
            leaf_idx + 1
        } else if leaf_idx > 0 {
            leaf_idx - 1
        } else {
            return Ok(());
        };
        let (a, b) = if leaf_idx < sibling_idx {
            (leaf_idx, sibling_idx)
        } else {
            (sibling_idx, leaf_idx)
        };
        // Borrow split: take both weights.
        let (left, right) = children.split_at_mut(b);
        let leaf_w = &mut left[a].0;
        let sib_w = &mut right[0].0;
        let signed_delta = if leaf_idx < sibling_idx {
            delta
        } else {
            -delta
        };
        let new_leaf = (*leaf_w + signed_delta).clamp(0.05, 0.95);
        let new_sib = (*sib_w - signed_delta).clamp(0.05, 0.95);
        *leaf_w = new_leaf;
        *sib_w = new_sib;
        renormalize(children);
        Ok(())
    }

    /// Cycle focus to the next leaf in tree order (depth-first, in
    /// `children` order). Wraps from the last leaf to the first. No-op if
    /// the active tab has fewer than 2 leaves.
    pub fn focus_next(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        // Desktop mode: the sequence is the row-major slot order, not tree
        // order (spec-desktop-mode.md Behavior 5).
        if tab.layout_mode == LayoutMode::Desktop {
            if let Some(next) = tab.desktop.sequence_neighbor(tab.focused, true) {
                tab.focused = next;
            }
            return Ok(());
        }
        let ids = tab.layout.leaf_ids();
        if ids.len() < 2 {
            return Ok(());
        }
        let pos = ids.iter().position(|&id| id == tab.focused).ok_or(())?;
        let next = (pos + 1) % ids.len();
        tab.focused = ids[next];
        Ok(())
    }

    /// Cycle focus to the previous leaf in tree order.
    pub fn focus_prev(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        if tab.layout_mode == LayoutMode::Desktop {
            if let Some(prev) = tab.desktop.sequence_neighbor(tab.focused, false) {
                tab.focused = prev;
            }
            return Ok(());
        }
        let ids = tab.layout.leaf_ids();
        if ids.len() < 2 {
            return Ok(());
        }
        let pos = ids.iter().position(|&id| id == tab.focused).ok_or(())?;
        let prev = if pos == 0 { ids.len() - 1 } else { pos - 1 };
        tab.focused = ids[prev];
        Ok(())
    }

    /// Topological focus motion. Walks up the tree from the focused leaf to
    /// find the nearest ancestor `Split` whose direction matches:
    ///
    /// - `Left`/`Right` → nearest `SplitDir::V` ancestor (children laid out
    ///   left-to-right).
    /// - `Up`/`Down` → nearest `SplitDir::H` ancestor (children laid out
    ///   top-to-bottom).
    ///
    /// At that ancestor, moves to the sibling at `current_idx ± 1`. If the
    /// sibling is itself a `Split`, descends into its first leaf (matching
    /// vim's "land on the most-recently-focused descendant" heuristic with
    /// a simpler proxy — left-most/top-most leaf).
    ///
    /// No-op when there's no sibling in the requested direction.
    pub fn focus_motion(&mut self, dir: FocusDir) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        // Desktop mode: spatial navigation over slots, not tree topology
        // (spec-desktop-mode.md Behavior 5). No candidate = no-op.
        if tab.layout_mode == LayoutMode::Desktop {
            let sdir = match dir {
                FocusDir::Left => SpatialDir::Left,
                FocusDir::Right => SpatialDir::Right,
                FocusDir::Up => SpatialDir::Up,
                FocusDir::Down => SpatialDir::Down,
            };
            if let Some(next) = tab.desktop.spatial_neighbor(tab.focused, sdir) {
                tab.focused = next;
            }
            return Ok(());
        }
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;
        if path.is_empty() {
            return Ok(());
        }
        let want_dir = match dir {
            FocusDir::Left | FocusDir::Right => SplitDir::V,
            FocusDir::Up | FocusDir::Down => SplitDir::H,
        };
        let delta: isize = match dir {
            FocusDir::Right | FocusDir::Down => 1,
            FocusDir::Left | FocusDir::Up => -1,
        };

        // Walk path back-to-front looking for the nearest matching-direction
        // ancestor that has a sibling in the requested direction.
        for depth in (0..path.len()).rev() {
            let parent_path = &path[..depth];
            let child_idx = path[depth];
            let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
            let Layout::Split {
                dir: parent_dir,
                children,
            } = parent
            else {
                continue;
            };
            if *parent_dir != want_dir {
                continue;
            }
            let target_idx = (child_idx as isize) + delta;
            if target_idx < 0 || target_idx as usize >= children.len() {
                continue; // No sibling in this direction at this depth.
            }
            // Descend into the target sibling's first leaf (or itself if leaf).
            let target_layout = &children[target_idx as usize].1;
            if let Some(&first_leaf_id) = target_layout.leaf_ids().first() {
                tab.focused = first_leaf_id;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Equalize all weights in the focused window's parent split. No-op if
    /// the focused leaf is the tab's root.
    pub fn equalize_focused(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        let focused = tab.focused;
        let path = tab.layout.path_to(focused).ok_or(())?;
        if path.is_empty() {
            return Ok(());
        }
        let (parent_path, _) = path.split_at(path.len() - 1);
        let parent = tab.layout.node_at_path_mut(parent_path).ok_or(())?;
        if let Layout::Split { children, .. } = parent {
            let n = children.len();
            if n > 0 {
                let even = 1.0 / n as f32;
                for (w, _) in children.iter_mut() {
                    *w = even;
                }
            }
        }
        Ok(())
    }

    // --- Layout patterns: marks (Phase 1) ----------------------------------

    /// Find which tab (by index) contains the window with the given id.
    pub fn tab_containing(&self, id: WindowId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.layout.find_leaf(id).is_some())
    }

    /// Collect all window ids across all tabs (for mark GC).
    pub fn all_window_ids(&self) -> HashSet<WindowId> {
        let mut out = HashSet::new();
        for tab in &self.tabs {
            tab.layout.for_each_leaf(&mut |w| {
                out.insert(w.id);
            });
        }
        out
    }

    // --- Layout patterns: automatic layouts (Phase 2) ----------------------

    /// Re-tile the active tab's layout according to its current `layout_mode`.
    /// No-op in Manual mode. Called after split/close/mode-switch.
    pub fn retile_active(&mut self) {
        let tab = match self.active_tab_mut() {
            Some(t) => t,
            None => return,
        };
        // Manual: the user's hand-built tree IS the layout. Desktop: the tree
        // is the content owner only — geometry lives in `tab.desktop`
        // (spec-desktop-mode.md); draining/rebuilding here would destroy the
        // tree the next Manual restore appends leftovers to. Neither retiles.
        if matches!(tab.layout_mode, LayoutMode::Manual | LayoutMode::Desktop) {
            return;
        }
        let focused = tab.focused;
        let windows = drain_leaves(&mut tab.layout);
        if windows.is_empty() {
            return;
        }
        let has_focused = windows.iter().any(|w| w.id == focused);

        tab.layout = match tab.layout_mode {
            LayoutMode::Manual | LayoutMode::Desktop => unreachable!(),
            LayoutMode::MasterStack => {
                build_master_stack(windows, tab.master_count, tab.master_ratio)
            }
            LayoutMode::Monocle => build_monocle(windows),
            LayoutMode::Columns => build_columns(windows),
        };

        if has_focused {
            tab.focused = focused;
        } else if let Some(&first) = tab.layout.leaf_ids().first() {
            tab.focused = first;
        }
    }

    /// Switch the active tab's layout mode. Saves/restores the manual tree
    /// as needed.
    pub fn set_layout_mode(&mut self, new_mode: LayoutMode) {
        let tab = match self.active_tab_mut() {
            Some(t) => t,
            None => return,
        };
        let old_mode = tab.layout_mode;
        if old_mode == new_mode {
            return;
        }

        // Save manual tree skeleton when leaving Manual mode.
        if old_mode == LayoutMode::Manual && new_mode != LayoutMode::Manual {
            tab.saved_manual_layout = Some(tab.layout.skeleton());
        }

        tab.layout_mode = new_mode;

        // Restore manual tree when returning to Manual mode.
        if new_mode == LayoutMode::Manual {
            if let Some(skeleton) = tab.saved_manual_layout.take() {
                let mut current_windows = drain_leaves(&mut tab.layout);
                tab.layout = restore_from_skeleton(skeleton, &mut current_windows);
                // Append any windows created during auto mode that don't
                // exist in the saved skeleton as new leaves.
                for leftover in current_windows {
                    let root = std::mem::take(&mut tab.layout);
                    tab.layout = match root {
                        Layout::Empty => Layout::Leaf(leftover),
                        _ => Layout::Split {
                            dir: SplitDir::V,
                            children: vec![(0.9, root), (0.1, Layout::Leaf(leftover))],
                        },
                    };
                }
            }
            return;
        }

        self.retile_active();
    }
}

impl<C> Default for Workspace<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper for new workspace auto-naming. User-facing default label for a
/// freshly-created workspace (today's `Tab`) when the user hasn't renamed it.
pub fn auto_tab_name(idx: usize) -> String {
    format!("workspace-{idx}")
}

// ---------------------------------------------------------------------------
// Layout algorithms (spec-layout-patterns.md Phase 2)
// ---------------------------------------------------------------------------

/// Recursively extract all leaf windows from a layout tree, leaving `Empty`.
pub fn drain_leaves<C>(layout: &mut Layout<C>) -> Vec<Window<C>> {
    let mut out = Vec::new();
    drain_leaves_inner(layout, &mut out);
    out
}

fn drain_leaves_inner<C>(layout: &mut Layout<C>, out: &mut Vec<Window<C>>) {
    let taken = std::mem::take(layout);
    match taken {
        Layout::Empty => {}
        Layout::Leaf(w) => out.push(w),
        Layout::Split { children, .. } => {
            for (_, mut child) in children {
                drain_leaves_inner(&mut child, out);
            }
        }
    }
}

/// Build a MasterStack layout: master region on the left, stack on the right.
/// `master_count` windows go in the left column (stacked vertically if >1),
/// remaining windows go in the right column (stacked vertically).
pub fn build_master_stack<C>(
    mut windows: Vec<Window<C>>,
    master_count: usize,
    master_ratio: f32,
) -> Layout<C> {
    if windows.is_empty() {
        return Layout::Empty;
    }
    if windows.len() == 1 {
        return Layout::Leaf(windows.remove(0));
    }
    let mc = master_count.min(windows.len());
    let stack_windows: Vec<Window<C>> = windows.split_off(mc);
    let master_windows = windows;

    let master_layout = stack_vertical(master_windows);

    if stack_windows.is_empty() {
        return master_layout;
    }

    let stack_layout = stack_vertical(stack_windows);

    Layout::Split {
        dir: SplitDir::V,
        children: vec![
            (master_ratio, master_layout),
            (1.0 - master_ratio, stack_layout),
        ],
    }
}

/// Build a Monocle layout: flat split of all windows. Only the focused one
/// is rendered (handled by the render path).
pub fn build_monocle<C>(windows: Vec<Window<C>>) -> Layout<C> {
    stack_vertical(windows)
}

/// Build a Columns layout: equal-width vertical columns, full height each.
pub fn build_columns<C>(windows: Vec<Window<C>>) -> Layout<C> {
    if windows.is_empty() {
        return Layout::Empty;
    }
    if windows.len() == 1 {
        return Layout::Leaf(windows.into_iter().next().unwrap());
    }
    let n = windows.len() as f32;
    Layout::Split {
        dir: SplitDir::V,
        children: windows
            .into_iter()
            .map(|w| (1.0 / n, Layout::Leaf(w)))
            .collect(),
    }
}

/// Helper: stack windows vertically (H-split) with equal heights.
fn stack_vertical<C>(windows: Vec<Window<C>>) -> Layout<C> {
    if windows.is_empty() {
        return Layout::Empty;
    }
    if windows.len() == 1 {
        return Layout::Leaf(windows.into_iter().next().unwrap());
    }
    let n = windows.len() as f32;
    Layout::Split {
        dir: SplitDir::H,
        children: windows
            .into_iter()
            .map(|w| (1.0 / n, Layout::Leaf(w)))
            .collect(),
    }
}

/// Reconstruct a layout from a saved [`LayoutSkeleton`] by matching window ids.
/// Windows matched by id are placed in their saved positions; unmatched slots
/// become `Empty`. Consumed windows are removed from `pool` so the caller can
/// detect leftovers (windows created after the skeleton was saved).
fn restore_from_skeleton<C>(skeleton: LayoutSkeleton, pool: &mut Vec<Window<C>>) -> Layout<C> {
    match skeleton {
        LayoutSkeleton::Empty => Layout::Empty,
        LayoutSkeleton::Leaf(id) => {
            if let Some(pos) = pool.iter().position(|w| w.id == id) {
                Layout::Leaf(pool.remove(pos))
            } else {
                Layout::Empty
            }
        }
        LayoutSkeleton::Split { dir, children } => {
            let mut rebuilt: Vec<(f32, Layout<C>)> = children
                .into_iter()
                .map(|(w, skel)| (w, restore_from_skeleton(skel, pool)))
                .filter(|(_, layout)| !matches!(layout, Layout::Empty))
                .collect();
            match rebuilt.len() {
                0 => Layout::Empty,
                1 => rebuilt.remove(0).1,
                _ => {
                    renormalize(&mut rebuilt);
                    Layout::Split {
                        dir,
                        children: rebuilt,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod desktop_tests {
    use super::*;

    fn slots_of(d: &DesktopState) -> Vec<(WindowId, (u32, u32))> {
        d.slots
            .iter()
            .map(|&(id, s)| (id, (s.row, s.col)))
            .collect()
    }

    #[test]
    fn seed_fills_row_major_at_width() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3, 4, 5], 3);
        assert_eq!(
            slots_of(&d),
            vec![
                (1, (0, 0)),
                (2, (0, 1)),
                (3, (0, 2)),
                (4, (1, 0)),
                (5, (1, 1)),
            ]
        );
    }

    #[test]
    fn drop_on_empty_slot_is_plain_move_leaving_gap() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3], 3);
        d.insert_shift(1, Slot::new(2, 2), 3);
        assert_eq!(d.slot_of(1), Some(Slot::new(2, 2)));
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)));
        assert_eq!(d.slot_of(3), Some(Slot::new(0, 2)));
        assert_eq!(d.occupant(Slot::new(0, 0)), None, "old slot becomes a gap");
    }

    #[test]
    fn drop_on_occupied_slot_ripples_run_until_gap() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3, 4], 3); // (0,0) (0,1) (0,2) (1,0)
        // Drop 4 onto 2's slot (0,1). 4's own (1,0) is vacated FIRST, so the
        // run is [2 @ (0,1), 3 @ (0,2)] and 3's wrap target (1,0) is the gap
        // that absorbs the ripple.
        d.insert_shift(4, Slot::new(0, 1), 3);
        assert_eq!(d.slot_of(4), Some(Slot::new(0, 1)));
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 2)));
        assert_eq!(d.slot_of(3), Some(Slot::new(1, 0)));
        assert_eq!(
            d.slot_of(1),
            Some(Slot::new(0, 0)),
            "tile before the run never moves"
        );
    }

    #[test]
    fn ripple_wraps_rows_at_effective_width() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2], 2); // (0,0), (0,1)
        // Drop 2 onto (0,0): run = [1 @ (0,0)] → 1 shifts to (0,1) (2's gap).
        d.insert_shift(2, Slot::new(0, 0), 2);
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 0)));
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 1)));
        // Drop 2 onto (0,1): run = [1 @ (0,1)] → 1 wraps to (1,0).
        d.insert_shift(2, Slot::new(0, 1), 2);
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)));
        assert_eq!(d.slot_of(1), Some(Slot::new(1, 0)));
    }

    /// Spec Behavior 4: tiles at `col >= W` sit outside every successor
    /// chain — ripples never touch them.
    #[test]
    fn tiles_beyond_effective_width_never_ripple() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3], 5); // seeded wide: cols 0, 1, 2
        // Window narrowed to W = 2; tile 3 at (0,2) is beyond W.
        d.insert_shift(2, Slot::new(0, 0), 2);
        // Run at (0,0) = [1]; succ((0,0), 2) = (0,1), 2's vacated gap.
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 0)));
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 1)));
        assert_eq!(
            d.slot_of(3),
            Some(Slot::new(0, 2)),
            "beyond-W tile untouched"
        );
    }

    // ── Tile span / edge resize (spec Behavior 4b) ──────────────────────

    #[test]
    fn occupant_is_rectangle_aware() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(2, 2)); // covers (0,0),(0,1),(1,0),(1,1)
        for cell in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            assert_eq!(d.occupant(Slot::new(cell.0, cell.1)), Some(1));
        }
        assert_eq!(d.occupant(Slot::new(0, 2)), None);
        assert_eq!(d.occupant(Slot::new(2, 0)), None);
    }

    #[test]
    fn resize_east_grows_into_free_desktop_and_clamps_at_neighbor() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2], 5); // 1@(0,0), 2@(0,1)
        // Tile 1 wants to grow east a lot, but tile 2 sits at (0,1).
        assert_eq!(d.clamp_span(1, ResizeEdge::East, 4), Span::new(1, 1));
        // Move 2 out of the way; now 1 can grow east up to the requested cols.
        d.insert_shift(2, Slot::new(0, 4), 5);
        assert_eq!(d.clamp_span(1, ResizeEdge::East, 3), Span::new(1, 3));
        // ...but not past the relocated neighbor at col 4.
        assert_eq!(d.clamp_span(1, ResizeEdge::East, 9), Span::new(1, 4));
    }

    #[test]
    fn resize_south_independent_of_columns() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        assert_eq!(d.clamp_span(1, ResizeEdge::South, 3), Span::new(3, 1));
        d.set_span(1, Span::new(3, 1));
        // A blocker two rows down clamps further south growth.
        d.slots.push((2, Slot::new(3, 0)));
        assert_eq!(d.clamp_span(1, ResizeEdge::South, 5), Span::new(3, 1));
    }

    #[test]
    fn shrink_is_always_allowed_to_one() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(3, 3));
        // Even fully surrounded, a tile may shrink.
        d.slots.push((2, Slot::new(0, 3)));
        d.slots.push((3, Slot::new(3, 0)));
        assert_eq!(d.clamp_span(1, ResizeEdge::East, 1), Span::new(3, 1));
        assert_eq!(d.clamp_span(1, ResizeEdge::South, 1), Span::new(1, 3));
    }

    #[test]
    fn spanned_tile_is_a_wall_that_rejects_unabsorbable_inserts() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3], 3); // 1@(0,0) 2@(0,1) 3@(0,2)
        d.set_span(2, Span::new(1, 2)); // 2 now covers (0,1),(0,2) — overlaps 3!
        // (Set up directly for the test; reposition 3 to avoid overlap.)
        d.slots.retain(|(id, _)| *id != 3);
        d.slots.push((3, Slot::new(1, 0)));
        d.sort();
        // Drop 3 onto (0,0): run = [1@(0,0)], next is (0,1) which is the
        // spanned tile 2 → wall, no gap before it → rejected, nothing moves.
        let placed = d.insert_shift(3, Slot::new(0, 0), 3);
        assert!(!placed, "insertion blocked by the spanned wall");
        assert_eq!(d.slot_of(3), Some(Slot::new(1, 0)), "3 stays home");
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "1 didn't move");
    }

    #[test]
    fn drop_onto_a_spanned_tile_is_rejected() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(2, 2));
        d.slots.push((2, Slot::new(2, 2)));
        d.sort();
        // Drop 2 onto (0,1) — inside 1's rectangle.
        let placed = d.insert_shift(2, Slot::new(0, 1), 4);
        assert!(!placed);
        assert_eq!(d.slot_of(2), Some(Slot::new(2, 2)), "2 returns home");
    }

    #[test]
    fn spanned_tile_moves_only_onto_a_free_rectangle() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(2, 2));
        d.slots.push((2, Slot::new(0, 2)));
        d.sort();
        // 1 (2×2) onto (0,1) would overlap 2 at (0,2)..(1,2)? 1's rect there is
        // (0,1)(0,2)(1,1)(1,2) — overlaps 2@(0,2). Rejected.
        assert!(!d.insert_shift(1, Slot::new(0, 1), 8));
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "stayed home");
        // Onto (2,0): rect (2,0)(2,1)(3,0)(3,1) — all free. Accepted.
        assert!(d.insert_shift(1, Slot::new(2, 0), 8));
        assert_eq!(d.slot_of(1), Some(Slot::new(2, 0)));
    }

    #[test]
    fn occupied_extent_accounts_for_span() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(2, 3));
        // Far corner is (0+2-1, 0+3-1) = (1, 2).
        assert_eq!(d.occupied_extent(), Some((1, 2)));
    }

    #[test]
    fn reconcile_drops_spans_of_closed_tiles() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2], 3);
        d.set_span(1, Span::new(2, 2));
        // Tile 1 closes; only 2 remains.
        d.reconcile(&[2], 2, 3);
        assert_eq!(d.span_of(1), Span::ONE, "stale span dropped");
        assert!(d.spans.is_empty());
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)), "2's slot untouched");
    }

    #[test]
    fn reconcile_drops_stale_and_inserts_after_focused() {
        let mut d = DesktopState::default();
        d.seed(&[1, 2, 3], 3);
        // Window 2 closed; window 9 opened; focused = 1.
        let changed = d.reconcile(&[1, 3, 9], 1, 3);
        assert!(changed);
        assert_eq!(d.slot_of(2), None);
        assert_eq!(
            d.slot_of(9),
            Some(Slot::new(0, 1)),
            "new window lands after focused, in the gap 2 left"
        );
        assert_eq!(d.slot_of(3), Some(Slot::new(0, 2)), "no ripple needed");
        assert!(
            !d.reconcile(&[1, 3, 9], 1, 3),
            "idempotent when the invariant already holds"
        );
    }

    #[test]
    fn spatial_neighbor_prefers_aligned_then_nearest() {
        let mut d = DesktopState::default();
        d.slots = vec![
            (1, Slot::new(0, 0)),
            (2, Slot::new(0, 2)),
            (3, Slot::new(1, 1)),
        ];
        d.slots.sort_by_key(|&(_, s)| s);
        assert_eq!(
            d.spatial_neighbor(1, SpatialDir::Right),
            Some(2),
            "same-row beats nearer diagonal"
        );
        assert_eq!(d.spatial_neighbor(1, SpatialDir::Down), Some(3));
        assert_eq!(d.spatial_neighbor(1, SpatialDir::Left), None);
        assert_eq!(d.sequence_neighbor(1, true), Some(2));
        assert_eq!(d.sequence_neighbor(2, true), Some(3));
        assert_eq!(d.sequence_neighbor(1, false), Some(3), "wraps");
    }

    #[test]
    fn geometry_round_trips() {
        let tile = (960.0, 800.0);
        let g = 12.0;
        let s = Slot::new(2, 3);
        let (x, y) = slot_origin(s, tile, g);
        assert_eq!(slot_at((x + 5.0, y + 5.0), tile, g), s);
        assert_eq!(effective_width(2000.0, tile.0, g), 2);
        assert_eq!(effective_width(100.0, tile.0, g), 1, "minimum 1");
    }

    /// Old-binary safety (spec Behavior 7): unknown mode strings fall back
    /// to Manual instead of failing the whole snapshot parse; known strings
    /// round-trip.
    #[test]
    fn layout_mode_deserialize_falls_back_to_manual() {
        for (s, want) in [
            ("\"manual\"", LayoutMode::Manual),
            ("\"master_stack\"", LayoutMode::MasterStack),
            ("\"monocle\"", LayoutMode::Monocle),
            ("\"columns\"", LayoutMode::Columns),
            ("\"desktop\"", LayoutMode::Desktop),
            ("\"some_future_mode\"", LayoutMode::Manual),
        ] {
            let got: LayoutMode = serde_json::from_str(s).unwrap();
            assert_eq!(got, want, "{s}");
        }
        assert_eq!(
            serde_json::to_string(&LayoutMode::Desktop).unwrap(),
            "\"desktop\""
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A trivial content type for testing the generic layout/workspace logic
    // without needing to depend on the binary's real content enum.
    #[derive(Debug, PartialEq, Eq)]
    struct TestContent(&'static str);

    fn leaf(id: WindowId, c: &'static str) -> Layout<TestContent> {
        Layout::Leaf(Window {
            id,
            content: TestContent(c),
        })
    }

    // 5c / ADR-0007: two views of the same file share ONE pooled core, so an
    // edit in one is live in the other and undo is unified. This pins the
    // structural contract (`open_and_retain` dedups by canonical path) that the
    // GUI's Doc/Edit tiles and splits rely on; it is the headlessly-verifiable
    // half of the "Doc tracks live Edit edits" behavior (the per-frame
    // re-render itself needs a GPUI runtime).
    #[test]
    fn pool_dedups_by_path_so_two_views_share_one_core() {
        let dir = std::env::temp_dir().join(format!(
            "yalda_pool_share_{}_{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.md");
        std::fs::write(&path, "hello\n").unwrap();

        let mut ws: Workspace<TestContent> = Workspace::new();
        let (id1, core1) = ws.open_and_retain(&path).unwrap();
        let (id2, core2) = ws.open_and_retain(&path).unwrap();

        // Dedup by canonical path: one buffer, two handles onto the SAME core.
        assert_eq!(id1, id2, "same path must map to one pooled buffer");
        assert!(
            Rc::ptr_eq(&core1, &core2),
            "both views must share one EditorCore"
        );

        // An edit through one handle is immediately visible through the other.
        {
            let mut c = core1.borrow_mut();
            let doc = c.document_mut();
            doc.begin_undo_group(0, 0, &[], 0);
            doc.insert_str(0, 0, "X");
            doc.end_undo_group(0, 1);
        }
        assert!(
            core2.borrow().document().full_text().starts_with("Xhello"),
            "edit via core1 must be live in core2 (shared rope)"
        );

        // Undo is unified: undoing through the OTHER handle reverts the first's
        // edit — one history per file, not per view.
        {
            let mut c = core2.borrow_mut();
            c.document_mut().undo(&[], 0);
        }
        assert!(
            core1.borrow().document().full_text().starts_with("hello"),
            "undo via core2 must revert the edit made via core1 (unified undo)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_leaf_walks_tree() {
        let layout: Layout<TestContent> = Layout::Split {
            dir: SplitDir::H,
            children: vec![
                (0.5, leaf(1, "a")),
                (
                    0.5,
                    Layout::Split {
                        dir: SplitDir::V,
                        children: vec![(0.5, leaf(2, "b")), (0.5, leaf(3, "c"))],
                    },
                ),
            ],
        };
        assert_eq!(layout.find_leaf(1).map(|w| w.content.0), Some("a"));
        assert_eq!(layout.find_leaf(3).map(|w| w.content.0), Some("c"));
        assert!(layout.find_leaf(99).is_none());
    }

    #[test]
    fn for_each_leaf_visits_in_tree_order() {
        let layout: Layout<TestContent> = Layout::Split {
            dir: SplitDir::H,
            children: vec![
                (0.5, leaf(1, "a")),
                (
                    0.5,
                    Layout::Split {
                        dir: SplitDir::V,
                        children: vec![(0.5, leaf(2, "b")), (0.5, leaf(3, "c"))],
                    },
                ),
            ],
        };
        let mut seen = Vec::new();
        layout.for_each_leaf(&mut |w| seen.push(w.id));
        assert_eq!(seen, vec![1, 2, 3]);
    }

    #[test]
    fn alloc_ids_are_monotonic() {
        let mut ws: Workspace<TestContent> = Workspace::new();
        assert_eq!(ws.alloc_window_id(), 1);
        assert_eq!(ws.alloc_window_id(), 2);
        assert_eq!(ws.alloc_buffer_id(), 1);
        assert_eq!(ws.alloc_buffer_id(), 2);
    }

    #[test]
    fn canonical_key_handles_relative_paths() {
        let key = Workspace::<TestContent>::canonical_key(Path::new("./nonexistent.md"));
        assert!(key.is_absolute());
        assert!(key.ends_with("nonexistent.md"));
    }

    #[test]
    fn open_buffer_pools_by_canonical_path() {
        let mut ws: Workspace<TestContent> = Workspace::new();
        // Use a path that probably doesn't exist on the FS so we exercise the
        // empty-buffer branch.
        let p = std::env::temp_dir().join("yalda-workspace-test-buffer.md");
        let _ = std::fs::remove_file(&p);
        let id1 = ws.open_buffer(&p).unwrap();
        let id2 = ws.open_buffer(&p).unwrap();
        assert_eq!(id1, id2, "same path should return same id");
        assert_eq!(ws.file_buffers.len(), 1);
    }

    // --- Mutation methods (split / close / only / resize / equalize) ---

    fn ws_with_layout(layout: Layout<TestContent>, focused: WindowId) -> Workspace<TestContent> {
        let mut ws: Workspace<TestContent> = Workspace::new();
        ws.tabs.push(Tab {
            auto_name: "tab-1".into(),
            display_name: None,
            layout,
            focused,
            rail: None,
            layout_mode: LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
        });
        // Ensure window-id allocator skips past the ids we hand-rolled.
        let max_id = ws.tabs[0].layout.leaf_ids().into_iter().max().unwrap_or(0);
        ws.next_window_id = max_id + 1;
        ws
    }

    #[test]
    fn split_focused_on_root_wraps_in_split() {
        let mut ws = ws_with_layout(leaf(1, "a"), 1);
        let new_id = ws.split_focused(SplitDir::V, TestContent("b")).unwrap();
        let tab = ws.active_tab().unwrap();
        match &tab.layout {
            Layout::Split { dir, children } => {
                assert_eq!(*dir, SplitDir::V);
                assert_eq!(children.len(), 2);
                assert_eq!(children[0].1.leaf_ids(), vec![1]);
                assert_eq!(children[1].1.leaf_ids(), vec![new_id]);
            }
            _ => panic!("expected Split at root, got {:?}", tab.layout.leaf_ids()),
        }
        assert_eq!(tab.focused, new_id);
    }

    #[test]
    fn split_focused_same_dir_appends_to_existing_split() {
        // [a | b]  → split focused (b) vertically again → [a | b | c]
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 2);
        let new_id = ws.split_focused(SplitDir::V, TestContent("c")).unwrap();
        let tab = ws.active_tab().unwrap();
        match &tab.layout {
            Layout::Split { dir, children } => {
                assert_eq!(*dir, SplitDir::V);
                assert_eq!(children.len(), 3);
                assert_eq!(children[2].1.leaf_ids(), vec![new_id]);
                // weights renormalize.
                let sum: f32 = children.iter().map(|(w, _)| *w).sum();
                assert!((sum - 1.0).abs() < 1e-5);
            }
            _ => panic!("expected flat 3-way Split"),
        }
    }

    #[test]
    fn split_focused_perpendicular_dir_nests() {
        // [a | b]  → split focused (b) horizontally → [a | (b/c)]
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 2);
        let _new_id = ws.split_focused(SplitDir::H, TestContent("c")).unwrap();
        let tab = ws.active_tab().unwrap();
        match &tab.layout {
            Layout::Split { dir, children } => {
                assert_eq!(*dir, SplitDir::V);
                assert_eq!(children.len(), 2);
                match &children[1].1 {
                    Layout::Split { dir, children } => {
                        assert_eq!(*dir, SplitDir::H);
                        assert_eq!(children.len(), 2);
                    }
                    _ => panic!("expected nested Split at child 1"),
                }
            }
            _ => panic!("expected outer V-split"),
        }
    }

    #[test]
    fn close_focused_in_2child_split_collapses() {
        // [a | b]  → close focused (b) → root becomes Leaf(a).
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 2);
        let new_focus = ws.close_focused().unwrap();
        assert_eq!(new_focus, Some(1));
        let tab = ws.active_tab().unwrap();
        match &tab.layout {
            Layout::Leaf(w) => assert_eq!(w.id, 1),
            _ => panic!("expected collapsed root Leaf(1)"),
        }
        assert_eq!(tab.focused, 1);
    }

    #[test]
    fn close_focused_in_3child_split_keeps_split() {
        // [a | b | c]  → close b → [a | c]
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![
                (0.33, leaf(1, "a")),
                (0.33, leaf(2, "b")),
                (0.34, leaf(3, "c")),
            ],
        };
        let mut ws = ws_with_layout(layout, 2);
        let new_focus = ws.close_focused().unwrap();
        // After close, focus moves to previous index → leaf 1.
        assert_eq!(new_focus, Some(1));
        let tab = ws.active_tab().unwrap();
        match &tab.layout {
            Layout::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                let sum: f32 = children.iter().map(|(w, _)| *w).sum();
                assert!((sum - 1.0).abs() < 1e-5);
            }
            _ => panic!("expected 2-child split"),
        }
    }

    #[test]
    fn close_focused_on_root_leaf_signals_tab_empty() {
        let mut ws = ws_with_layout(leaf(1, "a"), 1);
        let result = ws.close_focused().unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn only_replaces_root_with_focused_leaf() {
        // [a | (b/c)]  → only on c → root = Leaf(c)
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![
                (0.5, leaf(1, "a")),
                (
                    0.5,
                    Layout::Split {
                        dir: SplitDir::H,
                        children: vec![(0.5, leaf(2, "b")), (0.5, leaf(3, "c"))],
                    },
                ),
            ],
        };
        let mut ws = ws_with_layout(layout, 3);
        ws.only().unwrap();
        let tab = ws.active_tab().unwrap();
        assert_eq!(tab.layout.leaf_ids(), vec![3]);
    }

    #[test]
    fn resize_focused_shifts_weights() {
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 1);
        // Grow leaf 1's weight by 0.1; sibling 2 shrinks by 0.1.
        ws.resize_focused(0.1).unwrap();
        let tab = ws.active_tab().unwrap();
        if let Layout::Split { children, .. } = &tab.layout {
            assert!((children[0].0 - 0.6).abs() < 1e-5);
            assert!((children[1].0 - 0.4).abs() < 1e-5);
        }
    }

    #[test]
    fn focus_next_cycles_in_tree_order() {
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![
                (0.5, leaf(1, "a")),
                (
                    0.5,
                    Layout::Split {
                        dir: SplitDir::H,
                        children: vec![(0.5, leaf(2, "b")), (0.5, leaf(3, "c"))],
                    },
                ),
            ],
        };
        let mut ws = ws_with_layout(layout, 1);
        ws.focus_next().unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 2);
        ws.focus_next().unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 3);
        ws.focus_next().unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 1, "wraps around");
    }

    #[test]
    fn focus_motion_left_right_walks_v_split() {
        // [a | b | c] — V split with three children.
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![
                (0.33, leaf(1, "a")),
                (0.34, leaf(2, "b")),
                (0.33, leaf(3, "c")),
            ],
        };
        let mut ws = ws_with_layout(layout, 2);
        ws.focus_motion(FocusDir::Right).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 3);
        ws.focus_motion(FocusDir::Right).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 3, "no-op at right edge");
        ws.focus_motion(FocusDir::Left).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 2);
    }

    #[test]
    fn focus_motion_down_descends_through_nested_split() {
        // Outer H split [top / bottom]; bottom is a V split [b | c].
        // From top (1), Down should land on bottom's first leaf (b → 2).
        let layout = Layout::Split {
            dir: SplitDir::H,
            children: vec![
                (0.5, leaf(1, "top")),
                (
                    0.5,
                    Layout::Split {
                        dir: SplitDir::V,
                        children: vec![(0.5, leaf(2, "b")), (0.5, leaf(3, "c"))],
                    },
                ),
            ],
        };
        let mut ws = ws_with_layout(layout, 1);
        ws.focus_motion(FocusDir::Down).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 2);
    }

    #[test]
    fn focus_motion_no_op_at_root() {
        let mut ws = ws_with_layout(leaf(1, "only"), 1);
        ws.focus_motion(FocusDir::Right).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 1);
    }

    #[test]
    fn equalize_focused_resets_to_equal() {
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![
                (0.7, leaf(1, "a")),
                (0.2, leaf(2, "b")),
                (0.1, leaf(3, "c")),
            ],
        };
        let mut ws = ws_with_layout(layout, 2);
        ws.equalize_focused().unwrap();
        let tab = ws.active_tab().unwrap();
        if let Layout::Split { children, .. } = &tab.layout {
            for (w, _) in children {
                assert!((w - 1.0 / 3.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn buffer_retain_release_lifecycle() {
        let mut ws: Workspace<TestContent> = Workspace::new();
        let p = std::env::temp_dir().join("yalda-workspace-test-refcount.md");
        let _ = std::fs::remove_file(&p);
        let id = ws.open_buffer(&p).unwrap();

        ws.buffer_retain(id);
        ws.buffer_retain(id);
        assert_eq!(ws.buffer(id).unwrap().refcount, 2);

        ws.buffer_release(id);
        assert_eq!(ws.buffer(id).unwrap().refcount, 1);

        // Releasing to 0 with a clean buffer drops it from the pool.
        ws.buffer_release(id);
        assert!(
            ws.buffer(id).is_none(),
            "clean buffer with refcount 0 should be dropped"
        );
        assert!(ws.path_index.is_empty());
    }

    #[test]
    fn shared_core_propagates_edits_across_views() {
        // Two windows of one file get two clones of the SAME core; an edit
        // through one is visible through the other, and the document's
        // `edit_seq` (the perf-cache key) advances for both since it's one doc.
        let mut ws: Workspace<TestContent> = Workspace::new();
        let p = std::env::temp_dir().join("yalda-shared-edit-test.md");
        let _ = std::fs::remove_file(&p);

        let (id_a, core_a) = ws.open_and_retain(&p).unwrap();
        let (id_b, core_b) = ws.open_and_retain(&p).unwrap();
        assert_eq!(id_a, id_b, "same path pools to one buffer id");
        assert!(Rc::ptr_eq(&core_a, &core_b), "both views share one core");

        let seq_before = core_a.borrow().document().edit_seq();
        core_a.borrow_mut().programmatic_insert(0, "hello");
        // View B sees A's edit (shared rope).
        assert_eq!(core_b.borrow().document().full_text(), "hello");
        // The single shared doc's edit_seq advanced — so any view-model cache
        // keyed on edit_seq invalidates for every view.
        assert!(core_b.borrow().document().edit_seq() > seq_before);
    }

    #[test]
    fn gc_reaps_unreferenced_clean_buffers_keeps_dirty() {
        let mut ws: Workspace<TestContent> = Workspace::new();
        let clean = std::env::temp_dir().join("yalda-gc-clean.md");
        let dirty = std::env::temp_dir().join("yalda-gc-dirty.md");
        let _ = std::fs::remove_file(&clean);
        let _ = std::fs::remove_file(&dirty);

        // Clean buffer: open, then drop the view's core handle → only the pool
        // holds it → gc should reap it.
        let (clean_id, clean_core) = ws.open_and_retain(&clean).unwrap();
        drop(clean_core);

        // Dirty buffer: edit it, keep no view handle → gc must KEEP it.
        let (dirty_id, dirty_core) = ws.open_and_retain(&dirty).unwrap();
        dirty_core.borrow_mut().programmatic_insert(0, "x");
        drop(dirty_core);

        ws.gc_buffers();
        assert!(
            ws.buffer(clean_id).is_none(),
            "clean, unreferenced → reaped"
        );
        assert!(
            ws.buffer(dirty_id).is_some(),
            "dirty → retained for recovery"
        );
    }

    #[test]
    fn gc_keeps_buffers_a_view_still_holds() {
        let mut ws: Workspace<TestContent> = Workspace::new();
        let p = std::env::temp_dir().join("yalda-gc-live.md");
        let _ = std::fs::remove_file(&p);
        let (id, core) = ws.open_and_retain(&p).unwrap();
        ws.gc_buffers();
        assert!(
            ws.buffer(id).is_some(),
            "a buffer a live view still references must not be reaped"
        );
        drop(core);
        ws.gc_buffers();
        assert!(ws.buffer(id).is_none(), "once the view drops, gc reaps it");
    }

    // --- Relocate (move tile to workspace) ---------------------------------

    #[test]
    fn detach_focused_root_leaves_tab_empty() {
        let mut ws = ws_with_layout(leaf(1, "a"), 1);
        let (window, empty) = ws.detach_focused().unwrap();
        assert_eq!(window.id, 1);
        assert_eq!(window.content.0, "a");
        assert!(empty, "detaching the root leaf empties the tab");
        assert!(matches!(ws.tabs[0].layout, Layout::Empty));
    }

    #[test]
    fn detach_focused_in_split_prunes_and_refocuses() {
        // [a | b]  → detach focused (b) → root collapses to Leaf(a), focus → a.
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 2);
        let (window, empty) = ws.detach_focused().unwrap();
        assert_eq!(window.id, 2);
        assert!(!empty, "tab still has the sibling");
        match &ws.tabs[0].layout {
            Layout::Leaf(w) => assert_eq!(w.id, 1),
            _ => panic!("expected collapsed Leaf(1)"),
        }
        assert_eq!(ws.tabs[0].focused, 1);
    }

    #[test]
    fn insert_leaf_into_empty_tab_adopts_root() {
        let mut ws = ws_with_layout(leaf(1, "a"), 1);
        // Give it a second, empty tab.
        ws.tabs.push(Tab {
            auto_name: "workspace-2".into(),
            display_name: None,
            layout: Layout::Empty,
            focused: 0,
            rail: None,
            layout_mode: LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
        });
        let w = Window {
            id: 9,
            content: TestContent("moved"),
        };
        ws.insert_leaf_into_tab(1, w).unwrap();
        match &ws.tabs[1].layout {
            Layout::Leaf(w) => assert_eq!(w.id, 9),
            _ => panic!("empty tab should adopt the leaf as root"),
        }
        assert_eq!(ws.tabs[1].focused, 9);
    }

    #[test]
    fn insert_leaf_into_leaf_tab_wraps_in_split() {
        let mut ws = ws_with_layout(leaf(1, "a"), 1);
        ws.tabs.push(Tab {
            auto_name: "workspace-2".into(),
            display_name: None,
            layout: leaf(5, "x"),
            focused: 5,
            rail: None,
            layout_mode: LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
        });
        let w = Window {
            id: 9,
            content: TestContent("moved"),
        };
        ws.insert_leaf_into_tab(1, w).unwrap();
        match &ws.tabs[1].layout {
            Layout::Split { children, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(children[0].1.leaf_ids(), vec![5]);
                assert_eq!(children[1].1.leaf_ids(), vec![9]);
            }
            _ => panic!("expected a 2-child split"),
        }
        assert_eq!(ws.tabs[1].focused, 9);
    }

    #[test]
    fn move_focused_leaf_across_tabs_roundtrip() {
        // Source tab [a | b]; move focused b to a second (empty) tab.
        let layout = Layout::Split {
            dir: SplitDir::V,
            children: vec![(0.5, leaf(1, "a")), (0.5, leaf(2, "b"))],
        };
        let mut ws = ws_with_layout(layout, 2);
        ws.tabs.push(Tab {
            auto_name: "workspace-2".into(),
            display_name: None,
            layout: Layout::Empty,
            focused: 0,
            rail: None,
            layout_mode: LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
        });
        let (window, empty) = ws.detach_focused().unwrap();
        assert!(!empty);
        ws.insert_leaf_into_tab(1, window).unwrap();
        // Source collapsed to a.
        assert_eq!(ws.tabs[0].layout.leaf_ids(), vec![1]);
        // Target now holds the moved leaf.
        assert_eq!(ws.tabs[1].layout.leaf_ids(), vec![2]);
        assert_eq!(ws.tabs[1].focused, 2);
    }
}
