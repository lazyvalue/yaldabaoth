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

    /// Yield the ids of every leaf in tree order.
    pub fn leaf_ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.for_each_leaf(&mut |w| out.push(w.id));
        out
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

/// The interior of a workspace `Tab`. Post-Stage-D there is exactly ONE:
/// `Plane` — the infinite signed-grid + semantic-zoom camera
/// (`spec-infinite-plane-workspace.md`). The retired multi-mode surface
/// (Manual/MasterStack/Monocle/Columns) collapsed into this single value; the
/// enum is retained only so the persisted `layout_mode` field still has a type
/// and old snapshots deserialize (any value is force-mapped to `Plane`, and the
/// field is ignored on load — Behavior 7). The `Layout<C>` tree remains the
/// CONTENT owner; geometry + camera live in the tab's [`DesktopState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Plane,
}

/// Deserialize any persisted mode string (`manual`/`master_stack`/`monocle`/
/// `columns`/`desktop`/anything from a newer binary) to the sole `Plane` value.
/// Every workspace is a plane now (Behavior 1); the load path ignores this field
/// regardless (Behavior 7), so this just keeps old snapshots parseable rather
/// than failing the whole snapshot and overwriting the user's arrangement.
impl<'de> serde::Deserialize<'de> for LayoutMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let _ = String::deserialize(d)?;
        Ok(LayoutMode::Plane)
    }
}

// ---------------------------------------------------------------------------
// Desktop mode (spec-desktop-mode.md)
// ---------------------------------------------------------------------------

/// A cell address on the unbounded, **signed** plane grid
/// (`spec-infinite-plane-workspace.md`). Origin `(0, 0)`; the plane grows in all
/// four directions (rows/cols may be negative). Ordered row-major — the derived
/// `(row, col)` lexicographic `Ord` is the signed reading order that
/// `focus_next/prev` traverses (placement no longer uses it — the shelf is
/// retired).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot {
    pub row: i32,
    pub col: i32,
}

impl Slot {
    pub fn new(row: i32, col: i32) -> Self {
        Self { row, col }
    }
}

/// Discrete semantic-zoom level (`spec-infinite-plane-workspace.md` Behavior 3).
/// Lower = zoomed further out = coarser, cheaper tile representation. Not a
/// continuous scale: exactly three levels, one representation each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// 0 — full live tiles (the default and the reset target).
    Full,
    /// −1 — each tile collapses to a card (label/status, no live content).
    Card,
    /// −2 — each tile is a span-sized pip (plane shape only).
    Minimap,
}

/// Serialize `Detail` as `"full" | "card" | "minimap"` (the persisted camera
/// zoom, `spec-infinite-plane-workspace.md` D4).
impl serde::Serialize for Detail {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Detail::Full => "full",
            Detail::Card => "card",
            Detail::Minimap => "minimap",
        })
    }
}

/// Hand-rolled deserialize with an unknown-string fallback to `Full` — the
/// SAME safety `LayoutMode` uses (workspace.rs, above). The workspace snapshot
/// loader treats a failed parse as "no snapshot" and overwrites it on the next
/// save, so a derived deserializer meeting a zoom string from a NEWER binary
/// would silently reset the whole workspace arrangement. Falling back to `Full`
/// degrades one plane's camera zoom instead of dropping the snapshot.
impl<'de> serde::Deserialize<'de> for Detail {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "card" => Detail::Card,
            "minimap" => Detail::Minimap,
            // "full" and any string from the future
            _ => Detail::Full,
        })
    }
}

impl Detail {
    /// One step out (toward `Minimap`), clamped at the far end.
    fn out(self) -> Detail {
        match self {
            Detail::Full => Detail::Card,
            Detail::Card => Detail::Minimap,
            Detail::Minimap => Detail::Minimap,
        }
    }

    /// One step in (toward `Full`), clamped at the near end.
    fn inn(self) -> Detail {
        match self {
            Detail::Minimap => Detail::Card,
            Detail::Card => Detail::Full,
            Detail::Full => Detail::Full,
        }
    }
}

/// The per-plane view state (`spec-infinite-plane-workspace.md` D2). Pure view;
/// it never moves a tile (Constraint C1). `pan` is the plane point at the
/// viewport's top-left expressed in **pitch-independent slot units** — a given
/// `pan` names the same plane location at every `zoom`; the view derives pixels
/// as `pan · slot_pitch(zoom)` at its boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub pan: (f32, f32),
    pub zoom: Detail,
}

impl Default for Camera {
    /// The origin: `pan = (0,0)`, `zoom = Full` — where every plane starts and
    /// what reset-to-origin returns the view to.
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: Detail::Full,
        }
    }
}

/// The multiplier applied to the **Full** slot pitch for a Detail level — the
/// one place the three levels' relative sizes are defined. Pitch itself is
/// per-axis and viewport-derived (`desktop_tile_px` in `chrome.rs`), so a scalar
/// pitch is deliberately NOT defined here; the view scales its per-axis Full
/// pitch by this factor.
pub fn detail_scale(detail: Detail) -> f32 {
    match detail {
        Detail::Full => 1.0,
        Detail::Card => 0.5,
        Detail::Minimap => 0.2,
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

/// Which edge a desktop resize drag is pulling (spec Behavior 4b). East/South
/// hold the anchor fixed and grow the far edge; West/North hold the FAR edge
/// fixed and move the anchor (pull the left/top edge out to enlarge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    East,
    South,
    West,
    North,
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

/// Transient canvas-pan gesture (`Cmd+Shift`+left-drag pans the plane;
/// `spec-infinite-plane-workspace.md` Behavior 5). Plain window-space pixels +
/// the pan (slot units) captured at grab; never persisted.
#[derive(Debug, Clone, Copy)]
pub struct DesktopPan {
    /// Window-space pointer at grab.
    pub start_pointer: (f32, f32),
    /// Camera `pan` (slot units) at grab — the drag is applied relative to this.
    pub start_pan: (f32, f32),
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
    /// The plane's view state: pan (in slot units) + semantic-zoom Detail
    /// (`spec-infinite-plane-workspace.md` D2/D3). Replaces the old bare
    /// `pan: (f32,f32)`; pixels are derived at the view boundary.
    pub camera: Camera,
    /// Live drag, if any.
    pub drag: Option<DesktopDrag>,
    /// Live edge resize, if any (spec Behavior 4b).
    pub resize: Option<DesktopResize>,
    /// Live canvas pan gesture (`Cmd+Shift`+drag), if any. Transient.
    pub pan_drag: Option<DesktopPan>,
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
                && slot.row < anchor.row + sp.rows as i32
                && slot.col >= anchor.col
                && slot.col < anchor.col + sp.cols as i32;
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
        for dr in 0..span.rows as i32 {
            for dc in 0..span.cols as i32 {
                let cell = Slot::new(anchor.row + dr, anchor.col + dc);
                if matches!(self.occupant(cell), Some(id) if id != exclude) {
                    return false;
                }
            }
        }
        true
    }

    /// The (anchor, span) a Block-rule-clamped edge resize would commit, given
    /// the `desired` whole-slot extent ALONG the resize axis (cols for
    /// East/West, rows for North/South). East/South hold the anchor and grow
    /// the far edge; West/North hold the far edge and move the anchor toward
    /// the origin (pull-to-enlarge). Growth stops at the first slot owned by
    /// another tile or the `0` wall; shrinking is always allowed to 1 along the
    /// axis. The off-axis extent is held at its current value.
    pub fn clamp_resize(&self, id: WindowId, edge: ResizeEdge, desired: u32) -> (Slot, Span) {
        let cur = self.span_of(id);
        let Some(anchor) = self.slot_of(id) else {
            return (Slot::new(0, 0), cur);
        };
        let desired = desired.max(1);
        match edge {
            // Anchor-fixed: grow the far edge cell by cell while free.
            ResizeEdge::East | ResizeEdge::South => {
                let mut ext = 1;
                while ext < desired {
                    let cand = match edge {
                        ResizeEdge::East => Span::new(cur.rows, ext + 1),
                        _ => Span::new(ext + 1, cur.cols),
                    };
                    if self.rect_free(anchor, cand, id) {
                        ext += 1;
                    } else {
                        break;
                    }
                }
                let span = match edge {
                    ResizeEdge::East => Span::new(cur.rows, ext),
                    _ => Span::new(ext, cur.cols),
                };
                (anchor, span)
            }
            // Far-edge-fixed: the anchor moves toward (and past) the origin.
            // `desired` is the new total extent along the axis; the target near
            // edge is the far edge minus that. On the infinite plane there is
            // NO `0` wall — the anchor may cross into negative slots; only the
            // Block rule against other tiles clamps growth. Shrinking (target
            // nearer the far edge) is always free.
            ResizeEdge::West => {
                let right = anchor.col + cur.cols as i32; // exclusive far edge
                let target_left = right - desired as i32;
                let mut left = anchor.col;
                if target_left >= anchor.col {
                    left = target_left; // shrink toward the east edge
                } else {
                    while left > target_left
                        && self.rect_free(
                            Slot::new(anchor.row, left - 1),
                            Span::new(cur.rows, (right - (left - 1)) as u32),
                            id,
                        )
                    {
                        left -= 1;
                    }
                }
                (
                    Slot::new(anchor.row, left),
                    Span::new(cur.rows, (right - left) as u32),
                )
            }
            ResizeEdge::North => {
                let bottom = anchor.row + cur.rows as i32; // exclusive far edge
                let target_top = bottom - desired as i32;
                let mut top = anchor.row;
                if target_top >= anchor.row {
                    top = target_top; // shrink toward the south edge
                } else {
                    while top > target_top
                        && self.rect_free(
                            Slot::new(top - 1, anchor.col),
                            Span::new((bottom - (top - 1)) as u32, cur.cols),
                            id,
                        )
                    {
                        top -= 1;
                    }
                }
                (
                    Slot::new(top, anchor.col),
                    Span::new((bottom - top) as u32, cur.cols),
                )
            }
        }
    }

    /// Move a tile's anchor to `slot` directly, keeping the slot map sorted.
    /// Unlike a drop (`insert_shift`), this performs NO ripple — the caller
    /// (edge resize) has already Block-clamped the destination rectangle to be
    /// free, so neighbors must not move.
    pub fn set_anchor(&mut self, id: WindowId, slot: Slot) {
        if let Some(entry) = self.slots.iter_mut().find(|(w, _)| *w == id) {
            entry.1 = slot;
            self.sort();
        }
    }

    fn sort(&mut self) {
        self.slots.sort_by_key(|&(_, s)| s);
    }

    /// The first free slot on an outward ring-spiral from the origin
    /// (`spec-infinite-plane-workspace.md` Behavior 4): ring radius `r = 0, 1,
    /// 2, …`; within each ring, rows `-r..=r` × cols `-r..=r` in reading order,
    /// skipping the already-scanned interior (`|dr| < r && |dc| < r`) and any
    /// slot inside an existing tile's rectangle (`occupant`). Deterministic;
    /// independent of camera. Runs once per new tile, never per frame.
    pub fn seed_slot(&self) -> Slot {
        let mut r: i32 = 0;
        loop {
            for row in -r..=r {
                for col in -r..=r {
                    // Interior of this ring was covered by a smaller radius.
                    if row.abs() < r && col.abs() < r {
                        continue;
                    }
                    let cand = Slot::new(row, col);
                    if self.occupant(cand).is_none() {
                        return cand;
                    }
                }
            }
            r += 1;
        }
    }

    /// Restore the plane invariant (non-overlap + one anchor per live leaf):
    /// drop entries whose window is gone (their slot becomes a gap — neighbors
    /// never move) and give every slotless leaf a free slot via the origin
    /// ring-spiral ([`seed_slot`](Self::seed_slot)). Order-free — there is no
    /// sequence, no insert-and-shift ripple (Behavior 4). Returns true if
    /// anything changed. Fast path: a leaf that already has a slot is skipped,
    /// so the spiral runs only for genuinely slotless leaves.
    pub fn reconcile(&mut self, leaves: &[WindowId]) -> bool {
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
                continue; // already placed — the spiral never runs for it
            }
            let slot = self.seed_slot();
            self.slots.push((leaf, slot));
            self.sort();
            changed = true;
        }
        changed
    }

    /// Free-placement drop (`spec-infinite-plane-workspace.md` Behavior 4):
    /// move `id`'s whole rectangle to `target` iff every slot it would cover is
    /// otherwise free. An overlapping drop is **rejected** — `id` stays home,
    /// no neighbor moves (no ripple). Returns whether the move committed.
    pub fn free_drop(&mut self, id: WindowId, target: Slot) -> bool {
        let span = self.span_of(id);
        if !self.rect_free(target, span, id) {
            return false; // overlap — reject, leave every slot unchanged
        }
        self.set_anchor(id, target);
        true
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

    /// Signed bounding box of occupied slots (`spec-infinite-plane-workspace.md`
    /// D1): `Some((min, max))` where `min` is the min over anchors and `max`
    /// the max over each tile's inclusive far corner `anchor + span − 1`;
    /// `None` when the plane is empty. Both corners are needed now that tiles
    /// may sit left/above the origin — the old lone `(u32, u32)` max corner
    /// underflowed on negative anchors and couldn't express a min.
    pub fn occupied_extent(&self) -> Option<(Slot, Slot)> {
        let mut bb: Option<(Slot, Slot)> = None;
        for &(id, s) in &self.slots {
            let sp = self.span_of(id);
            // Inclusive far corner of the tile's rectangle.
            let far = Slot::new(s.row + sp.rows as i32 - 1, s.col + sp.cols as i32 - 1);
            bb = Some(match bb {
                Some((min, max)) => (
                    Slot::new(min.row.min(s.row), min.col.min(s.col)),
                    Slot::new(max.row.max(far.row), max.col.max(far.col)),
                ),
                None => (s, far),
            });
        }
        bb
    }

    // ── Camera (spec-infinite-plane-workspace.md Interfaces) ────────────────
    // Pure view ops over the plane; they mutate ONLY the camera (Constraint C1).

    /// Pan the viewport by `(dx, dy)` **slot units**, unclamped — the plane is
    /// infinite in all directions (Behavior 5); the viewport may travel into
    /// empty space.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.camera.pan.0 += dx;
        self.camera.pan.1 += dy;
    }

    /// Step the semantic zoom one level out (`Full → Card → Minimap`), clamped
    /// at `Minimap`, re-anchoring `pan` so the anchor slot stays under the same
    /// viewport point (Behavior 3). `anchor` is the focused tile's slot, or the
    /// viewport-center slot when nothing is focused.
    pub fn zoom_out(&mut self, anchor: Slot) {
        self.rezoom(self.camera.zoom.out(), anchor);
    }

    /// Step the semantic zoom one level in (`Minimap → Card → Full`), clamped
    /// at `Full`, re-anchoring on `anchor` (Behavior 3).
    pub fn zoom_in(&mut self, anchor: Slot) {
        self.rezoom(self.camera.zoom.inn(), anchor);
    }

    /// Apply a new Detail level, re-anchoring `pan` per-axis so `anchor` stays
    /// under the same viewport point. Because `pan` is in pitch-independent
    /// slot units, the re-anchor keeps `(anchor − pan)` constant *in pixels*:
    /// `(anchor − pan_new)·scale_new = (anchor − pan_old)·scale_old`, i.e.
    /// `pan_new = anchor − (anchor − pan_old)·scale_old/scale_new`. A no-op
    /// (already at the clamp) leaves the camera untouched.
    fn rezoom(&mut self, next: Detail, anchor: Slot) {
        if next == self.camera.zoom {
            return;
        }
        let old = detail_scale(self.camera.zoom);
        let new = detail_scale(next);
        let ratio = old / new;
        let (ax, ay) = (anchor.col as f32, anchor.row as f32);
        let (px, py) = self.camera.pan;
        self.camera.pan = (ax - (ax - px) * ratio, ay - (ay - py) * ratio);
        self.camera.zoom = next;
    }

    /// Reset-to-origin (Behavior 6): `pan = (0,0)`, `zoom = Full`. View-only —
    /// no tile moves or is re-seeded.
    pub fn reset_view(&mut self) {
        self.camera = Camera::default();
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

/// The slot whose cell contains a desktop-coordinate point. Signed: a point
/// left/above the origin maps to a negative col/row. `.floor()` (NOT a bare
/// `as i32`, which truncates toward zero) gives the correct cell for negatives.
pub fn slot_at(point: (f32, f32), tile: (f32, f32), gutter: f32) -> Slot {
    let cell = (tile.0 + gutter, tile.1 + gutter);
    let col = ((point.0 - gutter) / cell.0).floor() as i32;
    let row = ((point.1 - gutter) / cell.1).floor() as i32;
    Slot::new(row, col)
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
    /// Ephemeral *virtual workspace* marker (jump-panel; ADR-0021). `true` for a
    /// transient single-tile tab created to display a free agent session; such a
    /// tab is invisible to the jump panel's Workspaces section, the `?`
    /// workspace menu, and persistence, and is torn down (the session returning
    /// to free) the moment the active workspace switches away from it via
    /// [`Workspace::set_active_tab`]. `false` for every real workspace.
    pub ephemeral: bool,
    // --- Layout patterns (spec-layout-patterns.md) ---
    /// The tab interior. Always [`LayoutMode::Plane`] (infinite-plane, Stage D);
    /// persisted for snapshot-format stability but ignored on load (Behavior 7).
    pub layout_mode: LayoutMode,
    /// Inert persisted fields from the retired MasterStack mode. No longer read
    /// by any layout logic (the plane never re-tiles); kept only so the on-disk
    /// snapshot shape (`PersistedTab`) stays stable and old snapshots round-trip.
    pub master_ratio: f32,
    pub master_count: usize,
    /// Tag-view filter. When non-empty, the tab shows only windows whose
    /// buffer carries at least one tag in this set. Empty = show all.
    pub tag_view: TagSet,
    /// Desktop-mode placement (spec-desktop-mode.md). Geometry only — the
    /// layout tree above remains the content owner. Kept (not cleared) when
    /// switching away from Desktop so the arrangement survives round-trips.
    pub desktop: DesktopState,
    /// The workspace's working directory (spec-agent-cwd.md). **Private and
    /// required**: a `Tab` cannot be constructed without one (build via
    /// [`Tab::with_layout`]), so no workspace — real or ephemeral — can exist
    /// without a cwd. An agent created in this workspace inherits it
    /// (`agent_base_cwd`); read via [`Tab::cwd`], changed via [`Tab::set_cwd`]
    /// ("Set CWD"). Replaced a stringly `kv["cwd"]` whose omission silently fell
    /// back to the process dir — the cwd-inheritance regression (ADR-0023).
    cwd: WorkspaceCwd,
}

/// A workspace's working directory: a required, typed wrapper around a path.
/// Exists so "a workspace without a cwd" is **unrepresentable** — every [`Tab`]
/// holds one and every creation path must supply it. The process-dir default is
/// chosen once by the binary at the root workspace's creation, never silently at
/// read time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCwd(PathBuf);

impl WorkspaceCwd {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }
}

impl<C> Tab<C> {
    /// THE `Tab` constructor (the cwd field is private, so this is the only way
    /// to build one outside this module). Requires a [`WorkspaceCwd`] — that is
    /// what makes a cwd-less workspace unrepresentable. Non-cwd fields default
    /// (no rail, not ephemeral, Desktop layout, empty tags); callers set the
    /// public ones they need afterward (e.g. restore sets rail/desktop, the
    /// ephemeral path sets `ephemeral = true`).
    pub fn with_layout(
        auto_name: String,
        layout: Layout<C>,
        focused: WindowId,
        cwd: WorkspaceCwd,
    ) -> Self {
        Self {
            auto_name,
            display_name: None,
            layout,
            focused,
            rail: None,
            ephemeral: false,
            layout_mode: LayoutMode::default(),
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
            cwd,
        }
    }

    pub fn display_label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.auto_name)
    }

    /// The workspace's working directory. Always present (the type guarantees
    /// it). An agent created here inherits this (`agent_base_cwd`).
    pub fn cwd(&self) -> &WorkspaceCwd {
        &self.cwd
    }

    /// Change the working directory ("Set CWD"). Caller is responsible for
    /// `cx.notify()` + persist.
    pub fn set_cwd(&mut self, cwd: WorkspaceCwd) {
        self.cwd = cwd;
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
    /// The cwd a newly-created tab inherits when there is no active tab to copy
    /// from (the first/root tab). Set once by the binary at construction
    /// (`with_initial`); thereafter new tabs inherit the *active* tab's cwd.
    /// Keeps `workspace.rs` free of any process-dir knowledge.
    default_cwd: WorkspaceCwd,
}

impl<C> Workspace<C> {
    /// Bare workspace with no tabs. `default_cwd` is a last-resort placeholder
    /// (`.`) only ever read if a tab is created before any exists AND without an
    /// explicit cwd — in practice the first tab comes via `with_initial` (real
    /// cwd) or restore (per-tab cwd), so it is never the live source.
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
            default_cwd: WorkspaceCwd::new(PathBuf::from(".")),
        }
    }

    /// The cwd a new tab should inherit: the active tab's, else `default_cwd`.
    pub fn inherited_cwd(&self) -> WorkspaceCwd {
        self.active_tab()
            .map(|t| t.cwd().clone())
            .unwrap_or_else(|| self.default_cwd.clone())
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
    pub fn with_initial(content: C, cwd: WorkspaceCwd) -> Self {
        let mut ws = Self::new();
        // This cwd seeds the root tab AND becomes the fallback every later tab
        // inherits from when there's nothing active to copy.
        ws.default_cwd = cwd.clone();
        ws.push_initial_tab(content, cwd);
        ws
    }

    /// Append a new tab containing a single window with `content`, with working
    /// directory `cwd`. Becomes the active tab. Returns the new window's id.
    /// (Callers wanting "inherit the current workspace's cwd" pass
    /// [`Workspace::inherited_cwd`].)
    pub fn push_initial_tab(&mut self, content: C, cwd: WorkspaceCwd) -> WindowId {
        let id = self.alloc_window_id();
        let name = auto_tab_name(self.next_tab_index);
        self.next_tab_index += 1;
        self.tabs
            .push(Tab::with_layout(name, Layout::Leaf(Window { id, content }), id, cwd));
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

    /// Is the tab at `idx` an ephemeral virtual workspace (ADR-0021)?
    pub fn is_ephemeral(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|t| t.ephemeral)
    }

    /// Is the *active* tab an ephemeral virtual workspace? (Equivalently: does a
    /// virtual workspace currently exist — by invariant it is always the active
    /// one, created active and torn down the instant focus leaves it.)
    pub fn active_is_ephemeral(&self) -> bool {
        self.is_ephemeral(self.active_tab)
    }

    /// THE workspace-switch chokepoint (ADR-0021). Activate the tab at `idx`,
    /// first tearing down a departing **ephemeral** virtual workspace: if the
    /// currently-active tab is ephemeral and we are leaving it, that tab is
    /// removed (dropping its single agent tile, which returns the session to
    /// *free* in the store — the tile holds only a `SessionId` key). Index math
    /// accounts for the removal so `idx` still lands on its intended tab. No-op
    /// if `idx` is out of range. Does NOT notify — callers do.
    pub fn set_active_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let cur = self.active_tab;
        if cur != idx && self.is_ephemeral(cur) {
            self.tabs.remove(cur);
            // The ephemeral tab is gone; shift `idx` down if it sat after it.
            let target = if idx > cur { idx - 1 } else { idx };
            self.active_tab = target.min(self.tabs.len().saturating_sub(1));
        } else {
            self.active_tab = idx;
        }
    }

    /// Open an **ephemeral virtual workspace** (ADR-0021): a transient,
    /// single-tile tab holding `content`, made active. If a virtual workspace is
    /// already open it is replaced (we never accumulate more than one). Returns
    /// the new tile's window id. Does NOT notify — callers do.
    pub fn open_ephemeral_tab(&mut self, content: C) -> WindowId {
        // Inherit the spawning workspace's cwd BEFORE we (possibly) drop it, so
        // an agent created in the virtual workspace lands in the same dir as the
        // workspace you jumped from — not the process dir (the regression this
        // typed cwd makes impossible; ADR-0023).
        let cwd = self.inherited_cwd();
        // Replace any existing virtual workspace rather than stacking.
        if self.active_is_ephemeral() {
            let cur = self.active_tab;
            self.tabs.remove(cur);
        }
        let id = self.alloc_window_id();
        let name = auto_tab_name(self.next_tab_index);
        self.next_tab_index += 1;
        let mut tab = Tab::with_layout(name, Layout::Leaf(Window { id, content }), id, cwd);
        tab.ephemeral = true;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        id
    }

    /// Cycle to the next tab (wraps). Routes through [`set_active_tab`] so a
    /// departing virtual workspace is torn down.
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let next = (self.active_tab + 1) % self.tabs.len();
        self.set_active_tab(next);
    }

    /// Cycle to the previous tab (wraps). Routes through [`set_active_tab`].
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let prev = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
        self.set_active_tab(prev);
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

    /// Cycle focus to the next tile in the plane's row-major slot order
    /// (spec-infinite-plane-workspace.md Behavior 5). No-op if the active tab
    /// has fewer than 2 tiles.
    pub fn focus_next(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        if let Some(next) = tab.desktop.sequence_neighbor(tab.focused, true) {
            tab.focused = next;
        }
        Ok(())
    }

    /// Cycle focus to the previous tile in the plane's row-major slot order.
    pub fn focus_prev(&mut self) -> Result<(), ()> {
        let tab = self.active_tab_mut().ok_or(())?;
        if let Some(prev) = tab.desktop.sequence_neighbor(tab.focused, false) {
            tab.focused = prev;
        }
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
        // Spatial navigation over plane slots (spec-infinite-plane-workspace.md
        // Behavior 5). No candidate = no-op.
        let sdir = match dir {
            FocusDir::Left => SpatialDir::Left,
            FocusDir::Right => SpatialDir::Right,
            FocusDir::Up => SpatialDir::Up,
            FocusDir::Down => SpatialDir::Down,
        };
        if let Some(next) = tab.desktop.spatial_neighbor(tab.focused, sdir) {
            tab.focused = next;
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

    /// Retained no-op (infinite-plane, Stage D). The plane's `Layout<C>` tree is
    /// the CONTENT owner only; geometry lives in `tab.desktop` and never rebuilds
    /// the tree, so there is nothing to re-tile. Kept as a stable seam so the
    /// (many) callers that punctuated a structural mutation with a "settle the
    /// layout" call don't each need editing; on a plane it does nothing.
    pub fn retile_active(&mut self) {}
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

#[cfg(test)]
mod desktop_tests {
    use super::*;

    /// Place `id` at an explicit slot (test scaffold; the shelf `seed` is
    /// retired — planes place via `reconcile`/`seed_slot`/`free_drop`).
    fn put(d: &mut DesktopState, id: WindowId, row: i32, col: i32) {
        d.slots.retain(|(w, _)| *w != id);
        d.slots.push((id, Slot::new(row, col)));
        d.sort();
    }

    /// Free placement (Behavior 4): seed the plane, then drop a tile onto a
    /// free slot — it moves and leaves a gap; no neighbor shifts.
    #[test]
    fn seed_fills_row_major_at_width() {
        // (Kept name for continuity; now asserts ring-spiral seeding from
        // empty via reconcile — origin-first, deterministic.)
        let mut d = DesktopState::default();
        assert!(d.reconcile(&[1, 2, 3, 4, 5]));
        // Spiral order from origin: (0,0) then ring 1 in reading order.
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)));
        assert_eq!(d.slot_of(2), Some(Slot::new(-1, -1)));
        assert_eq!(d.slot_of(3), Some(Slot::new(-1, 0)));
        assert_eq!(d.slot_of(4), Some(Slot::new(-1, 1)));
        assert_eq!(d.slot_of(5), Some(Slot::new(0, -1)));
    }

    /// A drop onto a free slot is a plain move leaving a gap (Behavior 4).
    #[test]
    fn drop_on_occupied_slot_ripples_run_until_gap() {
        // (Kept name; NEW behavior — a drop onto a FREE slot moves cleanly and
        // leaves the old slot a gap; nothing ripples.)
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        put(&mut d, 3, 0, 2);
        assert!(d.free_drop(1, Slot::new(2, 2)));
        assert_eq!(d.slot_of(1), Some(Slot::new(2, 2)));
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)), "neighbor unmoved");
        assert_eq!(d.slot_of(3), Some(Slot::new(0, 2)), "neighbor unmoved");
        assert_eq!(d.occupant(Slot::new(0, 0)), None, "old slot becomes a gap");
    }

    /// An overlapping drop is rejected — the tile stays home, no neighbor
    /// moves (Behavior 4: free placement, no ripple).
    #[test]
    fn ripple_wraps_rows_at_effective_width() {
        // (Kept name; NEW behavior — a drop onto an OCCUPIED slot is rejected.)
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        assert!(!d.free_drop(2, Slot::new(0, 0)), "overlap rejected");
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)), "2 stays home");
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "1 never moves");
    }

    /// Closing a tile leaves a gap; reconcile drops it and never moves a
    /// neighbor (Behavior 4).
    #[test]
    fn tiles_beyond_effective_width_never_ripple() {
        // (Kept name; NEW behavior — closing a tile leaves a gap, no ripple.)
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        put(&mut d, 3, 0, 2);
        // Tile 2 closes; 1 and 3 keep their exact slots (gap at (0,1)).
        assert!(d.reconcile(&[1, 3]));
        assert_eq!(d.slot_of(2), None);
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "unmoved");
        assert_eq!(d.slot_of(3), Some(Slot::new(0, 2)), "unmoved");
        assert_eq!(d.occupant(Slot::new(0, 1)), None, "gap, not backfilled");
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
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        // Tile 1 wants to grow east a lot, but tile 2 sits at (0,1).
        assert_eq!(d.clamp_resize(1, ResizeEdge::East, 4).1, Span::new(1, 1));
        // Move 2 out of the way; now 1 can grow east up to the requested cols.
        assert!(d.free_drop(2, Slot::new(0, 4)));
        assert_eq!(d.clamp_resize(1, ResizeEdge::East, 3).1, Span::new(1, 3));
        // ...but not past the relocated neighbor at col 4.
        assert_eq!(d.clamp_resize(1, ResizeEdge::East, 9).1, Span::new(1, 4));
    }

    #[test]
    fn resize_south_independent_of_columns() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        assert_eq!(d.clamp_resize(1, ResizeEdge::South, 3).1, Span::new(3, 1));
        d.set_span(1, Span::new(3, 1));
        // A blocker two rows down clamps further south growth.
        d.slots.push((2, Slot::new(3, 0)));
        assert_eq!(d.clamp_resize(1, ResizeEdge::South, 5).1, Span::new(3, 1));
    }

    #[test]
    fn shrink_is_always_allowed_to_one() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.set_span(1, Span::new(3, 3));
        // Even fully surrounded, a tile may shrink.
        d.slots.push((2, Slot::new(0, 3)));
        d.slots.push((3, Slot::new(3, 0)));
        assert_eq!(d.clamp_resize(1, ResizeEdge::East, 1).1, Span::new(3, 1));
        assert_eq!(d.clamp_resize(1, ResizeEdge::South, 1).1, Span::new(1, 3));
    }

    /// A spanned tile is a wall: a free drop landing anywhere inside its
    /// rectangle is rejected (Behavior 4 — free placement, no ripple).
    #[test]
    fn spanned_tile_is_a_wall_that_rejects_unabsorbable_inserts() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        d.set_span(2, Span::new(1, 2)); // 2 covers (0,1),(0,2)
        put(&mut d, 3, 1, 0);
        // Drop 3 onto (0,2) — inside 2's rectangle → rejected, nothing moves.
        assert!(!d.free_drop(3, Slot::new(0, 2)), "wall rejects the drop");
        assert_eq!(d.slot_of(3), Some(Slot::new(1, 0)), "3 stays home");
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "1 didn't move");
    }

    #[test]
    fn drop_onto_a_spanned_tile_is_rejected() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        d.set_span(1, Span::new(2, 2));
        put(&mut d, 2, 2, 2);
        // Drop 2 onto (0,1) — inside 1's rectangle.
        assert!(!d.free_drop(2, Slot::new(0, 1)));
        assert_eq!(d.slot_of(2), Some(Slot::new(2, 2)), "2 stays home");
    }

    #[test]
    fn spanned_tile_moves_only_onto_a_free_rectangle() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        d.set_span(1, Span::new(2, 2));
        put(&mut d, 2, 0, 2);
        // 1 (2×2) onto (0,1): rect (0,1)(0,2)(1,1)(1,2) overlaps 2@(0,2).
        // Rejected.
        assert!(!d.free_drop(1, Slot::new(0, 1)));
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "stayed home");
        // Onto (2,0): rect (2,0)(2,1)(3,0)(3,1) — all free. Accepted.
        assert!(d.free_drop(1, Slot::new(2, 0)));
        assert_eq!(d.slot_of(1), Some(Slot::new(2, 0)));
    }

    #[test]
    fn occupied_extent_accounts_for_span() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        d.set_span(1, Span::new(2, 3));
        // Signed box: min = anchor (0,0), max = far corner (0+2-1, 0+3-1)=(1,2).
        assert_eq!(
            d.occupied_extent(),
            Some((Slot::new(0, 0), Slot::new(1, 2)))
        );
    }

    #[test]
    fn reconcile_drops_spans_of_closed_tiles() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        d.set_span(1, Span::new(2, 2));
        // Tile 1 closes; only 2 remains.
        d.reconcile(&[2]);
        assert_eq!(d.span_of(1), Span::ONE, "stale span dropped");
        assert!(d.spans.is_empty());
        assert_eq!(d.slot_of(2), Some(Slot::new(0, 1)), "2's slot untouched");
    }

    /// Reconcile is order-free: it drops stale entries and seeds slotless
    /// leaves via the origin ring-spiral; placed leaves never move (Behavior 4).
    #[test]
    fn reconcile_drops_stale_and_inserts_after_focused() {
        // (Kept name for continuity; NEW behavior — spiral seeding, not
        // insert-after-focused.)
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        put(&mut d, 3, 0, 2);
        // Window 2 closed; window 9 opened.
        let changed = d.reconcile(&[1, 3, 9]);
        assert!(changed);
        assert_eq!(d.slot_of(2), None);
        // 9 seeds at the first free spiral slot. (0,0),(0,2) occupied; spiral
        // from origin: (0,0) taken → ring 1 (-1,-1) is free and first.
        assert_eq!(d.slot_of(9), Some(Slot::new(-1, -1)), "spiral-seeded");
        assert_eq!(d.slot_of(1), Some(Slot::new(0, 0)), "placed leaf unmoved");
        assert_eq!(d.slot_of(3), Some(Slot::new(0, 2)), "placed leaf unmoved");
        assert!(
            !d.reconcile(&[1, 3, 9]),
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
        // Signed round-trip: a point in a negative-slot cell maps back to it
        // (floor, not truncate-toward-zero).
        let ns = Slot::new(-2, -3);
        let (nx, ny) = slot_origin(ns, tile, g);
        assert_eq!(slot_at((nx + 5.0, ny + 5.0), tile, g), ns);
    }

    // ── Infinite-plane engine (spec-infinite-plane-workspace.md) ────────────

    /// D1: `occupied_extent` is a signed min+max bounding box — negative
    /// anchors give a negative min corner, and the far corner adds the span;
    /// no `u32` underflow/panic on the negative side.
    #[test]
    fn occupied_extent_signed_min_max_box() {
        let mut d = DesktopState::default();
        put(&mut d, 1, -3, -5); // anchor left/above origin
        put(&mut d, 2, 2, 1);
        d.set_span(2, Span::new(2, 3)); // far corner (2+2-1, 1+3-1) = (3, 3)
        let (min, max) = d.occupied_extent().expect("non-empty");
        assert_eq!(min, Slot::new(-3, -5), "min over anchors, may be negative");
        assert_eq!(max, Slot::new(3, 3), "max over anchor+span-1");
        // Empty plane yields None (no panic).
        assert_eq!(DesktopState::default().occupied_extent(), None);
    }

    /// Behavior 4 / D1: west edge-resize crosses the origin (no `0` wall) but
    /// STILL Block-clamps on a neighbor sitting at a negative col — proving the
    /// removed 0-wall didn't remove the neighbor wall.
    #[test]
    fn clamp_resize_west_crosses_origin() {
        let mut d = DesktopState::default();
        // Target tile at (0,0); a neighbor occupies the NEGATIVE col (0,-2).
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, -2);
        // Pull tile 1 West a lot: it grows past col 0 into negatives, but the
        // neighbor at col -2 blocks it — anchor clamps at col -1 (span 2).
        let (a, s) = d.clamp_resize(1, ResizeEdge::West, 9);
        assert_eq!(a, Slot::new(0, -1), "crossed origin, blocked by neighbor");
        assert_eq!(s, Span::new(1, 2));
        // With no neighbor, the same pull crosses origin unimpeded.
        let mut d2 = DesktopState::default();
        put(&mut d2, 1, 0, 0);
        let (a2, s2) = d2.clamp_resize(1, ResizeEdge::West, 4);
        assert_eq!(a2, Slot::new(0, -3), "no 0-wall: anchor goes negative");
        assert_eq!(s2, Span::new(1, 4));
    }

    /// Behavior 4: `seed_slot` walks the origin ring-spiral deterministically —
    /// origin first when free, else the exact next reading-order ring slot.
    #[test]
    fn seed_slot_spiral_deterministic() {
        let mut d = DesktopState::default();
        // Empty plane: origin is the first free slot.
        assert_eq!(d.seed_slot(), Slot::new(0, 0));
        // Occupy the origin; the spiral's next reading-order slot is (-1,-1)
        // (ring 1: rows -1..=1 × cols -1..=1, interior (0,0) skipped later).
        put(&mut d, 1, 0, 0);
        assert_ne!(d.seed_slot(), Slot::new(0, 0));
        assert_eq!(d.seed_slot(), Slot::new(-1, -1));
        // Fill the whole of ring 1's leading edge up to (0,-1); next is... the
        // first still-free ring-1 slot after the occupied ones.
        put(&mut d, 2, -1, -1);
        put(&mut d, 3, -1, 0);
        assert_eq!(d.seed_slot(), Slot::new(-1, 1), "reading order within ring");
    }

    /// Behavior 4: an overlapping free drop is rejected and moves NOTHING —
    /// every tile's slot is byte-identical before and after.
    #[test]
    fn free_drop_rejects_overlap_without_moving_neighbors() {
        let mut d = DesktopState::default();
        put(&mut d, 1, 0, 0);
        put(&mut d, 2, 0, 1);
        put(&mut d, 3, 1, 0);
        let before: Vec<(WindowId, Slot)> = {
            let mut v = d.slots.clone();
            v.sort_by_key(|&(id, _)| id);
            v
        };
        // Drop 3 onto (0,1) — occupied by 2 → rejected.
        assert!(!d.free_drop(3, Slot::new(0, 1)));
        let after: Vec<(WindowId, Slot)> = {
            let mut v = d.slots.clone();
            v.sort_by_key(|&(id, _)| id);
            v
        };
        assert_eq!(before, after, "rejected drop moved nothing");
    }

    /// Old-binary safety (spec Behavior 7): the multi-mode surface is retired,
    /// so EVERY persisted mode string — the old known ones and anything from a
    /// newer binary — deserializes to the sole `Plane` value instead of failing
    /// the whole snapshot parse. (The load path ignores this field regardless.)
    #[test]
    fn layout_mode_deserialize_collapses_every_string_to_plane() {
        for s in [
            "\"manual\"",
            "\"master_stack\"",
            "\"monocle\"",
            "\"columns\"",
            "\"desktop\"",
            "\"plane\"",
            "\"some_future_mode\"",
        ] {
            let got: LayoutMode = serde_json::from_str(s).unwrap();
            assert_eq!(got, LayoutMode::Plane, "{s}");
        }
        // The sole variant serializes as "plane" (snake_case of `Plane`).
        assert_eq!(
            serde_json::to_string(&LayoutMode::Plane).unwrap(),
            "\"plane\""
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

    // Edge resize (Behavior 4b). West/North move the anchor (pull-to-enlarge)
    // while the far edge stays put; East/South hold the anchor and grow the far
    // edge. Block-clamped against neighbours — but NOT against a `0` wall
    // (infinite plane): the anchor may cross into negative slots.
    #[test]
    fn clamp_resize_west_north_move_anchor() {
        let mut d = DesktopState::default();
        // A 1×1 tile at (1,1) with open plane around it.
        d.slots.push((7, Slot::new(1, 1)));

        // Pull West two columns: no 0-wall, so the anchor moves freely to col 0
        // (far edge fixed at exclusive col 2), span widens to 2.
        let (a, s) = d.clamp_resize(7, ResizeEdge::West, 2);
        assert_eq!(a, Slot::new(1, 0));
        assert_eq!(s, Span::new(1, 2));

        // Pull North two rows: anchor moves to row 0, span heightens to 2.
        let (a, s) = d.clamp_resize(7, ResizeEdge::North, 2);
        assert_eq!(a, Slot::new(0, 1));
        assert_eq!(s, Span::new(2, 1));

        // East/South keep the anchor fixed.
        let (a, s) = d.clamp_resize(7, ResizeEdge::East, 3);
        assert_eq!(a, Slot::new(1, 1));
        assert_eq!(s, Span::new(1, 3));
    }

    #[test]
    fn clamp_resize_blocks_on_neighbor_and_shrinks_freely() {
        let mut d = DesktopState::default();
        d.slots.push((1, Slot::new(0, 0)));
        d.slots.push((2, Slot::new(0, 2)));
        d.sort();
        // Tile 1 at (0,0), tile 2 at (0,2). Tile 2 pulled West can only reach
        // col 1 — col 0 is owned by tile 1 (Block rule).
        let (a, s) = d.clamp_resize(2, ResizeEdge::West, 9);
        assert_eq!(a, Slot::new(0, 1));
        assert_eq!(s, Span::new(1, 2));

        // Grow tile 2 west to (0,1) span 2, then shrink back: anchor returns
        // east toward the fixed far edge, always free.
        d.set_anchor(2, Slot::new(0, 1));
        d.set_span(2, Span::new(1, 2));
        let (a, s) = d.clamp_resize(2, ResizeEdge::West, 1);
        assert_eq!(a, Slot::new(0, 2));
        assert_eq!(s, Span::new(1, 1));
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
            ephemeral: false,
            layout_mode: LayoutMode::Plane,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
            cwd: WorkspaceCwd::new(PathBuf::from(".")),
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

    // NOTE (infinite-plane, Stage D): the retired split-tree topology tests
    // `resize_focused_shifts_weights`, `focus_next_cycles_in_tree_order`,
    // `focus_motion_left_right_walks_v_split`,
    // `focus_motion_down_descends_through_nested_split`, and
    // `equalize_focused_resets_to_equal` were REMOVED — the weight-resize /
    // equalize / tree-order-focus behaviors they guarded no longer exist (the
    // plane is the only interior; focus traverses slot order, not tree order).
    // Plane focus navigation is covered by the `desktop_tests` module
    // (`spatial_neighbor_prefers_aligned_then_nearest`, `reconcile_*`).

    #[test]
    fn focus_motion_no_op_at_root() {
        // A single unseeded tile has no slot neighbor — focus_motion is a no-op.
        let mut ws = ws_with_layout(leaf(1, "only"), 1);
        ws.focus_motion(FocusDir::Right).unwrap();
        assert_eq!(ws.active_tab().unwrap().focused, 1);
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
            ephemeral: false,
            layout_mode: LayoutMode::Plane,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
            cwd: WorkspaceCwd::new(PathBuf::from(".")),
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
            ephemeral: false,
            layout_mode: LayoutMode::Plane,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
            cwd: WorkspaceCwd::new(PathBuf::from(".")),
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
            ephemeral: false,
            layout_mode: LayoutMode::Plane,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: BTreeSet::new(),
            desktop: DesktopState::default(),
            cwd: WorkspaceCwd::new(PathBuf::from(".")),
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
