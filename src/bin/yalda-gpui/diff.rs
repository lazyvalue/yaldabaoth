//! `App::Diff` tile data model. Cog node `app-diff-tile` (nd0e).
//! See docs/specs/spec-diff-review.md § Data Model.
//!
//! Mirrors the cheap-tile / cached-view split used by `App::Linear` and
//! `App::Cog` (`linear.rs`, `cog.rs`): `DiffTile` is a plain struct living
//! directly in the workspace layout tree (NOT a GPUI entity) holding the
//! binding + the derived [`DiffModel`] + view state; the expensive rendered
//! body is a cached [`DiffView`] entity (`diff_view.rs`), lazily created at
//! first render (`restore_content` has no `cx`).
//!
//! Unlike Linear/Cog, the spec's Data Model puts the derived payload
//! (`model`), focus, and collapse-set on the TILE itself (the single source
//! of truth other tiles/the jump panel/persistence can read without a `cx`
//! round-trip through a cached view). `DiffView` therefore does not own a
//! copy of this state — it reads it off the root view each render (see
//! `diff_view.rs`'s module doc for how that's kept O(changed) anyway).

use super::*;

/// The focused hunk, addressed by (file index, hunk index within that
/// file's `Vec<Hunk>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiffFocus {
    pub(crate) file: usize,
    pub(crate) hunk: usize,
}

/// The unbound tile's worktree picker (spec rev 2 B1, UXI-Diff-10): every
/// `git worktree list` entry of the repo containing the active workspace's
/// cwd, plus a trailing "Pick a folder…" row. `selected` ranges over
/// `0..=rows.len()` — index `rows.len()` IS the folder row, so the folder row
/// is always selectable (the only row when outside a git repo).
#[derive(Default)]
pub(crate) struct WorktreePicker {
    pub(crate) rows: Vec<WorktreeEntry>,
    pub(crate) selected: usize,
    /// A `list_worktrees` load is in flight.
    pub(crate) loading: bool,
    /// The last load found no git repository at the resolved directory.
    pub(crate) not_a_repo: bool,
    /// Any other load failure (git missing, …) — shown inline; the folder
    /// row stays available.
    pub(crate) error: Option<String>,
    /// `true` until a load has been kicked for this picker showing — the
    /// per-frame reconcile (`diff_reconcile`, `diff_ui.rs`) kicks it off the
    /// render path (a tile restored unbound never ran `open_diff_inner`).
    pub(crate) needs_load: bool,
    /// Stale-load guard (mirrors `DiffTile::req`).
    pub(crate) req: u64,
    /// Bumped on EVERY picker mutation — `DiffView`'s cheap fingerprint for
    /// the whole picker (`DiffSeqs::picker_gen`).
    pub(crate) gen_: u64,
}

impl WorktreePicker {
    /// Number of selectable rows: every worktree + the folder row.
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len() + 1
    }

    /// Index of the trailing "Pick a folder…" row.
    pub(crate) fn folder_index(&self) -> usize {
        self.rows.len()
    }

    /// Move the selection by `delta`, clamped (no wrap) to the row range.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let max = self.row_count() as i32 - 1;
        let next = (self.selected as i32 + delta).clamp(0, max) as usize;
        if next != self.selected {
            self.selected = next;
            self.gen_ = self.gen_.wrapping_add(1);
        }
    }

    pub(crate) fn bump(&mut self) {
        self.gen_ = self.gen_.wrapping_add(1);
    }
}

/// A Diff tile's payload (spec rev 2 § Data Model). `worktree: None` ⇒ the
/// tile renders the worktree picker; `Some` ⇒ it derives and shows that
/// worktree's diff. No session binding (ADR-0039).
pub(crate) struct DiffTile {
    /// The bound worktree root (`None` ⇒ picker).
    pub(crate) worktree: Option<PathBuf>,
    /// Picker state — meaningful only while `worktree` is `None`.
    pub(crate) picker: WorktreePicker,
    /// Last successfully derived diff (kept during a refresh — spec B3:
    /// "the tile shows the previous model until the new one lands").
    pub(crate) model: Option<DiffModel>,
    pub(crate) focus: DiffFocus,
    pub(crate) collapsed: HashSet<PathBuf>,
    pub(crate) refreshing: bool,
    /// Monotonic guard so a stale in-flight refresh can't clobber a newer
    /// one (mirrors `LinearTile::req` / `CogTile::req`).
    pub(crate) req: u64,
    /// Bumped every time `model` is replaced (success OR failure clears it
    /// to `None` without bumping — only a fresh model counts). Cheap render-
    /// input fingerprint for `DiffView` so it never has to hash the whole
    /// `DiffModel` (`diff_view.rs`'s `DiffSeqs`).
    pub(crate) model_gen: u64,
    /// Bumped on every `toggle_collapsed` — the collapse-set's cheap
    /// fingerprint (a `HashSet<PathBuf>` isn't `Hash`-friendly to fingerprint
    /// directly).
    pub(crate) collapsed_gen: u64,
    /// Error from the last failed derive (spec B1: a deleted worktree
    /// renders inline, never panics). Cleared on a new bind / success.
    pub(crate) error: Option<String>,
    /// `true` until the first derive has been kicked for the bound worktree —
    /// a tile restored from disk never ran a bind flow, so the per-frame
    /// reconcile (`diff_reconcile`) kicks it once (mirrors `CogTile::needs_load`).
    pub(crate) needs_load: bool,
    /// The cached body view — lazily created at first render (mirrors
    /// `LinearTile::view` / `CogTile::view`).
    pub(crate) view: Option<Entity<DiffView>>,
}

impl DiffTile {
    /// A fresh, UNBOUND tile — renders the worktree picker, whose list load
    /// is kicked by the reconcile (or directly by `open_diff_inner`).
    pub(crate) fn new() -> Self {
        DiffTile {
            worktree: None,
            picker: WorktreePicker {
                needs_load: true,
                ..WorktreePicker::default()
            },
            model: None,
            focus: DiffFocus::default(),
            collapsed: HashSet::new(),
            refreshing: false,
            req: 0,
            model_gen: 0,
            collapsed_gen: 0,
            error: None,
            needs_load: false,
            view: None,
        }
    }

    /// A tile bound to `path` whose first derive is still pending (restore).
    pub(crate) fn bound_to(path: PathBuf) -> Self {
        let mut t = Self::new();
        t.worktree = Some(path);
        t.picker.needs_load = false;
        t.needs_load = true;
        t
    }

    /// Tab / window title: the bound worktree's dir name, else "diff".
    pub(crate) fn title(&self) -> String {
        self.worktree
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "diff".to_string())
    }

    /// The currently-focused hunk's content hash, if any — used to preserve
    /// focus across a refresh (spec B3).
    pub(crate) fn focused_hunk_hash(&self) -> Option<u64> {
        let model = self.model.as_ref()?;
        let file = model.files.get(self.focus.file)?;
        file.hunks.get(self.focus.hunk).map(|h| h.hunk_hash)
    }

    /// Clamp `focus` into range for the current model, or reset to the
    /// origin if there is no model / it has no files.
    pub(crate) fn clamp_focus(&mut self) {
        let Some(model) = &self.model else {
            self.focus = DiffFocus::default();
            return;
        };
        if model.files.is_empty() {
            self.focus = DiffFocus::default();
            return;
        }
        self.focus.file = self.focus.file.min(model.files.len() - 1);
        let hunks = model.files[self.focus.file].hunks.len();
        self.focus.hunk = if hunks == 0 {
            0
        } else {
            self.focus.hunk.min(hunks - 1)
        };
    }

    /// Restore a focused hunk by content hash after a refresh (spec B3:
    /// "Hunk focus survives refresh when the focused hunk's hash still
    /// exists; otherwise focus moves to the nearest hunk"). Falls back to
    /// [`clamp_focus`](Self::clamp_focus) when the hash is gone or absent.
    pub(crate) fn restore_focus_by_hash(&mut self, hash: Option<u64>) {
        if let Some(hash) = hash
            && let Some(model) = &self.model
        {
            for (fi, file) in model.files.iter().enumerate() {
                if let Some(hi) = file.hunks.iter().position(|h| h.hunk_hash == hash) {
                    self.focus = DiffFocus { file: fi, hunk: hi };
                    return;
                }
            }
        }
        self.clamp_focus();
    }

    /// The flattened `(file_index, hunk_index)` sequence across every file,
    /// in display order. Shared by focus-move and rendering.
    fn flat_hunks(&self) -> Vec<(usize, usize)> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        model
            .files
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| (0..f.hunks.len()).map(move |hi| (fi, hi)))
            .collect()
    }

    /// Move the hunk focus by `delta` (±1 for j/k — spec B2), wrapping and
    /// stepping across file boundaries. No-op if there are no hunks at all.
    pub(crate) fn move_hunk_focus(&mut self, delta: i32) {
        let flat = self.flat_hunks();
        if flat.is_empty() {
            return;
        }
        let cur = flat
            .iter()
            .position(|&(fi, hi)| fi == self.focus.file && hi == self.focus.hunk)
            .unwrap_or(0);
        let n = flat.len() as i32;
        let next = (cur as i32 + delta).rem_euclid(n) as usize;
        let (fi, hi) = flat[next];
        self.focus = DiffFocus { file: fi, hunk: hi };
    }

    /// Jump to the next/prev file (spec B2 `[`/`]`), wrapping, landing on
    /// that file's first hunk.
    pub(crate) fn jump_file(&mut self, delta: i32) {
        let Some(model) = &self.model else { return };
        if model.files.is_empty() {
            return;
        }
        let n = model.files.len() as i32;
        let next = (self.focus.file as i32 + delta).rem_euclid(n) as usize;
        self.focus = DiffFocus {
            file: next,
            hunk: 0,
        };
    }

    /// Toggle a file's collapsed state (spec B2 file-level collapse/expand).
    pub(crate) fn toggle_collapsed(&mut self, path: &std::path::Path) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.collapsed_gen = self.collapsed_gen.wrapping_add(1);
    }
}

/// Build the `<abs-path>:<line>` argument `zed` takes on its command line
/// (spec B8 "Open in Zed"): the worktree root joined with the focused file's
/// repo-relative path, suffixed with the hunk's first new-file line. Pure —
/// no filesystem access, no spawn — so it's unit-tested independent of the
/// actual `zed` launch (`diff_ui.rs::open_hunk_in_zed` is the only caller).
pub(crate) fn zed_open_arg(worktree: &std::path::Path, rel_path: &std::path::Path, first_new_line: usize) -> String {
    format!("{}:{}", worktree.join(rel_path).display(), first_new_line)
}

// ── Cog node `open-in-zed` (oc72): spec B8 ──────────────────────────────────

#[cfg(test)]
mod zed_open_tests {
    use super::*;

    /// Spec B8 DONE_WHEN #1: the pure `<abs-path>:<line>` composition —
    /// worktree joined with the repo-relative path, colon-suffixed with the
    /// hunk's first new-file line. No spawn, no filesystem access.
    #[test]
    fn zed_open_arg_joins_worktree_rel_path_and_line() {
        let worktree = std::path::Path::new("/repo/worktree");
        let rel = std::path::Path::new("src/foo.rs");
        let arg = zed_open_arg(worktree, rel, 42);
        assert_eq!(arg, "/repo/worktree/src/foo.rs:42");
    }

    #[test]
    fn zed_open_arg_handles_nested_rel_path_and_line_one() {
        let worktree = std::path::Path::new("/Users/scott/ws/proj");
        let rel = std::path::Path::new("src/bin/app/main.rs");
        let arg = zed_open_arg(worktree, rel, 1);
        assert_eq!(arg, "/Users/scott/ws/proj/src/bin/app/main.rs:1");
    }
}
