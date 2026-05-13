//! Tabs / windows / splits data model for the GPUI workspace.
//!
//! This is the data substrate for `spec-tabs-and-splits.md`: a workspace
//! contains tabs, each tab roots an n-ary split tree of windows, and each
//! window holds one content kind (Doc / Edit / Browser / Claude). File-backed
//! editors live in a pooled `FileBuffer` and may be referenced by multiple
//! windows simultaneously (shared edits across splits).
//!
//! Scope note: the types live here, but wiring them into the live `App` is
//! staged in follow-up commits — this file establishes the shapes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sketch::editor::EditorCore;

pub type FileBufferId = u64;
pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// (`DocWindow`, `EditWindow`, `BrowserWindow`, `ClaudeWindow`) is defined in
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
pub enum Layout<C> {
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
            Layout::Leaf(w) => f(w),
            Layout::Split { children, .. } => {
                for (_, c) in children {
                    c.for_each_leaf(f);
                }
            }
        }
    }
}

/// One tab in the workspace's tab strip.
pub struct Tab<C> {
    /// Monotonic auto-name set at create (e.g. "tab-1"). Survives rename so
    /// the user can clear `display_name` and recover the auto-name.
    pub auto_name: String,
    /// User-set display name. `None` means render `auto_name`.
    pub display_name: Option<String>,
    pub layout: Layout<C>,
    pub focused: WindowId,
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

impl<C> Default for Workspace<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper for new tab auto-naming.
pub fn auto_tab_name(idx: usize) -> String {
    format!("tab-{idx}")
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
}
