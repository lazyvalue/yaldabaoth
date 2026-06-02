//! Tabs / windows / splits data model for the GPUI workspace.
//!
//! This is the data substrate for `spec-tabs-and-splits.md`: a workspace
//! contains tabs, each tab roots an n-ary split tree of windows, and each
//! window holds one content kind (Doc / Edit / Browser / Agent). File-backed
//! editors live in a pooled `FileBuffer` and may be referenced by multiple
//! windows simultaneously (shared edits across splits).
//!
//! Scope note: the types live here, but wiring them into the live `App` is
//! staged in follow-up commits — this file establishes the shapes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sketch::editor::EditorCore;
use sketch::file_browser::FileBrowser;

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
pub enum Layout<C> {
    Empty,
    Leaf(Window<C>),
    Split {
        dir: SplitDir,
        children: Vec<(f32, Layout<C>)>,
    },
}

impl<C> Default for Layout<C> {
    fn default() -> Self {
        Layout::Empty
    }
}

impl<C> Layout<C> {
    /// Find the leaf with the given id (recursive).
    pub fn find_leaf(&self, id: WindowId) -> Option<&Window<C>> {
        match self {
            Layout::Empty => None,
            Layout::Leaf(w) if w.id == id => Some(w),
            Layout::Leaf(_) => None,
            Layout::Split { children, .. } => {
                children.iter().find_map(|(_, c)| c.find_leaf(id))
            }
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

    /// Count the number of leaves in this subtree.
    pub fn leaf_count(&self) -> usize {
        match self {
            Layout::Empty => 0,
            Layout::Leaf(_) => 1,
            Layout::Split { children, .. } => {
                children.iter().map(|(_, c)| c.leaf_count()).sum()
            }
        }
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
pub enum RailSide {
    Left,
    Right,
}

impl Default for RailSide {
    fn default() -> Self {
        RailSide::Left
    }
}

/// Derived outline: heading entries from the focused window (spec-rail.md §13).
/// `entries` is `(heading depth 1–6, display text, block index or line
/// number)`. Re-derived on the render frames where the focused window changed.
pub struct OutlineState {
    pub entries: Vec<(u8, String, usize)>,
    pub selected: usize,
}

impl OutlineState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
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
}

impl RailState {
    pub fn new(content: RailContent, side: RailSide) -> Self {
        Self {
            content,
            side,
            width_px: RAIL_DEFAULT_WIDTH,
            focused: true,
        }
    }
}

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
}

impl<C> Tab<C> {
    pub fn display_label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.auto_name)
    }
}

/// A pooled file-backed editor. One `FileBuffer` per canonical path; refcount
/// tracks the number of `EditorView`s in the workspace currently bound to it.
pub struct FileBuffer {
    pub id: FileBufferId,
    pub canonical_path: PathBuf,
    pub core: EditorCore,
    pub file_label: String,
    /// Active EditorViews referencing this core.
    pub refcount: usize,
}

/// Top-level container for the GPUI frontend. Owns the tab strip, the file-
/// buffer pool, and the active-tab pointer. Replaces today's
/// `App.screen` + `App.open_buffers` + `App.active_buffer_idx` triple.
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
        });
        self.active_tab = self.tabs.len() - 1;
        id
    }

    /// Close the tab at index `idx`. The active-tab pointer adjusts to stay
    /// in range; closing the last tab leaves the workspace with zero tabs
    /// (caller is responsible for the spec Behavior 2 placeholder).
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
            core: EditorCore::new(text, key.clone()),
            file_label,
            refcount: 0,
        };
        self.file_buffers.insert(id, buf);
        self.path_index.insert(key, id);
        Ok(id)
    }

    pub fn buffer(&self, id: FileBufferId) -> Option<&FileBuffer> {
        self.file_buffers.get(&id)
    }

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

    /// Decrement the refcount. Drops the buffer from the pool when refcount
    /// hits 0 AND it has no unsaved changes; dirty buffers stay pooled for
    /// recovery via `:buffers` (Behavior 21).
    pub fn buffer_release(&mut self, id: FileBufferId) {
        let drop = if let Some(b) = self.file_buffers.get_mut(&id) {
            b.refcount = b.refcount.saturating_sub(1);
            b.refcount == 0 && !b.core.document().is_modified()
        } else {
            false
        };
        if drop {
            if let Some(b) = self.file_buffers.remove(&id) {
                self.path_index.remove(&b.canonical_path);
            }
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
            let Layout::Split { dir: parent_dir, children } = parent else {
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
    /// "move pane to workspace" verb.
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
    /// a single leaf, it is wrapped in a vertical `Split` so the arriving pane
    /// sits beside it; if it is already a `Split`, the leaf is appended as a
    /// new child and weights renormalize; an `Empty` target (a tab whose sole
    /// pane was just moved away) simply adopts the leaf as its root.
    ///
    /// Returns `Err(())` if `tab_idx` is out of range.
    pub fn insert_leaf_into_tab(
        &mut self,
        tab_idx: usize,
        window: Window<C>,
    ) -> Result<(), ()> {
        let id = window.id;
        let tab = self.tabs.get_mut(tab_idx).ok_or(())?;
        let root = std::mem::take(&mut tab.layout);
        tab.layout = match root {
            Layout::Empty => Layout::Leaf(window),
            Layout::Leaf(existing) => Layout::Split {
                dir: SplitDir::V,
                children: vec![
                    (0.5, Layout::Leaf(existing)),
                    (0.5, Layout::Leaf(window)),
                ],
            },
            Layout::Split { dir, mut children } => {
                let avg = if children.is_empty() {
                    1.0
                } else {
                    children.iter().map(|(w, _)| *w).sum::<f32>()
                        / children.len() as f32
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
        let signed_delta = if leaf_idx < sibling_idx { delta } else { -delta };
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
            let Layout::Split { dir: parent_dir, children } = parent else {
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
        let p = std::env::temp_dir().join("sketch-workspace-test-buffer.md");
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
        let p = std::env::temp_dir().join("sketch-workspace-test-refcount.md");
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

    // --- Relocate (move pane to workspace) ---------------------------------

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
        });
        let w = Window { id: 9, content: TestContent("moved") };
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
        });
        let w = Window { id: 9, content: TestContent("moved") };
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
