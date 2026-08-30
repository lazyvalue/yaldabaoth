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

/// What a Diff tile shows the cumulative diff for (spec § Data Model).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum DiffSource {
    /// Worktree derived from a session's `cwd` at bind time (spec B1).
    Session(SessionId),
    /// An explicit worktree, no session — e.g. restored from disk (spec
    /// "Persistence": a session-bound tile always restores `Path`-bound).
    Path(PathBuf),
}

/// The focused hunk, addressed by (file index, hunk index within that
/// file's `Vec<Hunk>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiffFocus {
    pub(crate) file: usize,
    pub(crate) hunk: usize,
}

/// A Diff tile's payload (spec § Data Model). `compose` is reserved for the
/// hunk-comment surface (spec B4, "comment → steering") — a LATER cog node
/// wires it; this node always leaves it `None`.
pub(crate) struct DiffTile {
    pub(crate) source: Option<DiffSource>,
    /// Last successfully derived diff (kept during a refresh — spec B3:
    /// "the tile shows the previous model until the new one lands").
    pub(crate) model: Option<DiffModel>,
    pub(crate) focus: DiffFocus,
    pub(crate) collapsed: HashSet<PathBuf>,
    /// Reserved for spec B4 — always `None` out of this node.
    pub(crate) compose: Option<Compose>,
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
    /// `true` until the first refresh has been kicked — a tile restored from
    /// disk (or freshly bound) never ran an explicit "open" flow, so
    /// `render_diff` kicks the derive once on first paint (mirrors
    /// `CogTile::needs_load`).
    pub(crate) needs_load: bool,
    /// The cached body view — lazily created at first render (mirrors
    /// `LinearTile::view` / `CogTile::view`).
    pub(crate) view: Option<Entity<DiffView>>,
}

impl DiffTile {
    pub(crate) fn new() -> Self {
        DiffTile {
            source: None,
            model: None,
            focus: DiffFocus::default(),
            collapsed: HashSet::new(),
            compose: None,
            refreshing: false,
            req: 0,
            model_gen: 0,
            collapsed_gen: 0,
            error: None,
            needs_load: true,
            view: None,
        }
    }

    pub(crate) fn bound_to_path(path: PathBuf) -> Self {
        let mut t = Self::new();
        t.source = Some(DiffSource::Path(path));
        t
    }

    pub(crate) fn bound_to_session(id: SessionId) -> Self {
        let mut t = Self::new();
        t.source = Some(DiffSource::Session(id));
        t
    }

    /// Tab / window title: the worktree dir name once known, else a generic
    /// placeholder (mirrors `LinearTile::title`).
    pub(crate) fn title(&self) -> String {
        self.worktree()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "diff".to_string())
    }

    /// The worktree path this tile is (or was last) deriving for. Used to
    /// persist a session-bound tile as `Path`-bound (spec "Persistence") and
    /// to fall back to `Path` binding when the bound session closes (spec
    /// B1). Prefers the last-derived model's worktree (authoritative once
    /// known) over the raw source.
    pub(crate) fn worktree(&self) -> Option<PathBuf> {
        self.model
            .as_ref()
            .map(|m| m.worktree.clone())
            .or_else(|| match &self.source {
                Some(DiffSource::Path(p)) => Some(p.clone()),
                _ => None,
            })
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
