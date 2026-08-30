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

/// Cog node `badge-projection` (1cxd), spec B6 / § Data Model: a root-owned,
/// worktree-keyed projection of the unreviewed-hunk count, updated ONLY at
/// `DiffModel` derive time (`diff_apply`, `diff_ui.rs`) — never by a
/// background scan. Lives on `YaldaGpuiView` (not on any `DiffTile`), so it
/// survives a tile close and is shared by every tile watching one worktree
/// (last derive wins). Not persisted: a worktree never opened in a Diff tile
/// this session has no entry, and a count goes stale-frozen once nothing
/// refreshes that worktree — both per spec B6.
pub(crate) type DiffProjections = HashMap<PathBuf, usize>;

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

/// Snapshot of the hunk a comment compose is anchored to (spec B4), captured
/// at OPEN time (`open_hunk_comment`, `diff_ui.rs`) rather than read live off
/// `DiffTile.model` at submit time — a background refresh (spec B3) can land
/// mid-compose and the tile's `focus`/`model` can move on, but the comment
/// compose is a short-lived side conversation about the hunk as it looked
/// when `c` was pressed, not a live view that would need to track a refresh.
#[derive(Clone)]
pub(crate) struct CommentTarget {
    /// Repo-relative path of the file the commented hunk belongs to.
    pub(crate) path: PathBuf,
    /// Inclusive new-file line range (`Hunk::new_line_range`).
    pub(crate) line_range: (usize, usize),
    /// Verbatim hunk patch text (`Hunk::patch_text`) — header + prefixed lines.
    pub(crate) patch: String,
}

/// Build the outgoing prompt for a hunk comment (spec B4): a machine-readable
/// prefix — repo-relative path, new-line range, and the hunk's verbatim patch
/// text, fenced so the agent can cleanly separate it from the human comment
/// that follows — then the user's typed comment. Pure and unit-tested
/// independent of the send path; `submit_hunk_comment` (`diff_ui.rs`) is the
/// only caller.
pub(crate) fn build_hunk_comment_prompt(
    path: &std::path::Path,
    line_range: (usize, usize),
    patch: &str,
    comment: &str,
) -> String {
    format!(
        "Review comment on a diff hunk:\n```\npath: {}\nlines {}-{}\n{}\n```\n{}",
        path.display(),
        line_range.0,
        line_range.1,
        patch.trim_end_matches('\n'),
        comment.trim()
    )
}

/// A Diff tile's payload (spec § Data Model). `compose` is the hunk-comment
/// surface (spec B4, "comment → steering") — opened by `open_hunk_comment`,
/// paired with `comment_target` (the hunk it's anchored to); `None` when no
/// comment is being composed (the tile's default, and its only insert-mode
/// surface — spec B9).
pub(crate) struct DiffTile {
    pub(crate) source: Option<DiffSource>,
    /// Last successfully derived diff (kept during a refresh — spec B3:
    /// "the tile shows the previous model until the new one lands").
    pub(crate) model: Option<DiffModel>,
    pub(crate) focus: DiffFocus,
    pub(crate) collapsed: HashSet<PathBuf>,
    /// The open hunk-comment compose (spec B4) — the tile's only insert-mode
    /// surface (`focused_in_insert_mode`, `main.rs`, keys off `.is_some()`).
    /// Opened by `open_hunk_comment`, cleared by `cancel_hunk_comment` (Esc)
    /// or on a successful `submit_hunk_comment` (left intact on send failure
    /// so the draft is never dropped).
    pub(crate) compose: Option<Compose>,
    /// The hunk `compose` is anchored to, snapshotted at open time (see
    /// [`CommentTarget`]). Always `Some` exactly when `compose` is.
    pub(crate) comment_target: Option<CommentTarget>,
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
            comment_target: None,
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

// ── Cog node `merge-gate` (v5tg): spec B7 pure predicate ────────────────────

/// Why `merge_gate_decision` refused a merge (spec B7). Exhaustive — these
/// are the ONLY three things B7 names as merge-blocking, so a caller can
/// match on this without a catch-all arm ever needing to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeRefusal {
    /// `usize` is the count of hunks in the current `DiffModel` that are not
    /// yet reviewed (spec: "refuses with the unreviewed count").
    UnreviewedHunks(usize),
    /// The worktree the Diff tile is bound to has uncommitted changes
    /// (tracked or untracked) — checked fresh via `git status --porcelain`
    /// at merge time, NEVER via `DiffModel::dirty` (which means "differs
    /// from merge-base", not "has uncommitted changes" — see `diff_model.rs`
    /// module docs on `parse_diff`).
    FeatureWorktreeDirty,
    /// The primary checkout (spec: "the merge executes in the primary
    /// checkout... it refuses if the primary checkout is dirty") has
    /// uncommitted changes.
    PrimaryCheckoutDirty,
}

impl std::fmt::Display for MergeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeRefusal::UnreviewedHunks(n) => {
                write!(f, "{n} unreviewed hunk{}", if *n == 1 { "" } else { "s" })
            }
            MergeRefusal::FeatureWorktreeDirty => write!(f, "feature worktree dirty"),
            MergeRefusal::PrimaryCheckoutDirty => write!(f, "primary checkout dirty"),
        }
    }
}

/// THE pure merge-gate predicate (spec B7): "merges the branch into base
/// ONLY when every hunk in the current `DiffModel` is reviewed and the
/// feature worktree is clean"; independently, "it refuses if the primary
/// checkout is dirty". Pure — no I/O, no subprocess — so it's exhaustively
/// unit-testable without a real git fixture; the ONLY production caller is
/// `diff_ui.rs::run_merge_gate`, which resolves `feature_clean` /
/// `primary_clean` via real `git status --porcelain` checks
/// (`diff_git.rs::worktree_is_clean`) run off the paint path (spec C2)
/// before calling in. Checked in the order the spec lists them, though
/// callers should treat the specific `Err` variant, not the check order, as
/// the contract.
pub(crate) fn merge_gate_decision(
    model: &DiffModel,
    feature_clean: bool,
    primary_clean: bool,
) -> Result<(), MergeRefusal> {
    let unreviewed = model.unreviewed_hunk_count();
    if unreviewed > 0 {
        return Err(MergeRefusal::UnreviewedHunks(unreviewed));
    }
    if !feature_clean {
        return Err(MergeRefusal::FeatureWorktreeDirty);
    }
    if !primary_clean {
        return Err(MergeRefusal::PrimaryCheckoutDirty);
    }
    Ok(())
}

/// Build the `<abs-path>:<line>` argument `zed` takes on its command line
/// (spec B8 "Open in Zed"): the worktree root joined with the focused file's
/// repo-relative path, suffixed with the hunk's first new-file line. Pure —
/// no filesystem access, no spawn — so it's unit-tested independent of the
/// actual `zed` launch (`diff_ui.rs::open_hunk_in_zed` is the only caller).
pub(crate) fn zed_open_arg(worktree: &std::path::Path, rel_path: &std::path::Path, first_new_line: usize) -> String {
    format!("{}:{}", worktree.join(rel_path).display(), first_new_line)
}

#[cfg(test)]
mod comment_prompt_tests {
    use super::*;

    /// Cog node `comment-steering` (hk81), spec B4: the built prompt must
    /// carry the repo-relative path, the new-line range, the verbatim patch
    /// text, and the user's comment — in that order, fenced so the agent can
    /// tell the machine-readable context apart from the human text. This is
    /// the pure-fn half of DONE_WHEN #1 (see `diff_ui.rs::submit_hunk_comment`
    /// for the send-path half).
    #[test]
    fn build_hunk_comment_prompt_includes_path_range_patch_and_comment() {
        let patch = "@@ -1,3 +1,3 @@\n fn foo() {\n-    old_line();\n+    new_line();\n }\n";
        let prompt = build_hunk_comment_prompt(
            std::path::Path::new("src/foo.rs"),
            (10, 12),
            patch,
            "please rename this",
        );
        assert!(
            prompt.contains("src/foo.rs"),
            "prompt must carry the repo-relative path: {prompt:?}"
        );
        assert!(
            prompt.contains("10-12") || prompt.contains("10") && prompt.contains("12"),
            "prompt must carry the new-line range: {prompt:?}"
        );
        assert!(
            prompt.contains("@@ -1,3 +1,3 @@"),
            "prompt must carry the hunk header: {prompt:?}"
        );
        assert!(
            prompt.contains("-    old_line();") && prompt.contains("+    new_line();"),
            "prompt must carry the verbatim patch lines: {prompt:?}"
        );
        assert!(
            prompt.contains("please rename this"),
            "prompt must carry the user's comment: {prompt:?}"
        );
        // Order: path/range/patch (the machine-readable prefix) before the
        // human comment, per spec B4 ("prefixed by machine-readable context").
        let patch_pos = prompt.find("@@ -1,3").unwrap();
        let comment_pos = prompt.find("please rename this").unwrap();
        assert!(
            patch_pos < comment_pos,
            "the patch context must precede the comment text"
        );
    }

    #[test]
    fn build_hunk_comment_prompt_trims_comment_whitespace() {
        let prompt =
            build_hunk_comment_prompt(std::path::Path::new("a.rs"), (1, 1), "@@ -1 +1 @@\n", "  hi  \n");
        assert!(prompt.trim_end().ends_with("hi"), "got {prompt:?}");
    }
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

// ── Cog node `merge-gate` (v5tg): spec B7 pure predicate ────────────────────

#[cfg(test)]
mod merge_gate_decision_tests {
    use super::*;

    /// Build a `DiffModel` with `n_unreviewed` unreviewed hunks and
    /// `n_reviewed` reviewed hunks (all in one fake file) — `merge_gate_decision`
    /// only ever reads `unreviewed_hunk_count()`, so no real diff text is
    /// needed.
    fn model_with(n_unreviewed: usize, n_reviewed: usize) -> DiffModel {
        let mut hunks = Vec::new();
        for i in 0..n_unreviewed {
            hunks.push(Hunk {
                header: "@@ -1,1 +1,1 @@".into(),
                lines: vec![],
                hunk_hash: i as u64,
                reviewed: false,
            });
        }
        for i in 0..n_reviewed {
            hunks.push(Hunk {
                header: "@@ -1,1 +1,1 @@".into(),
                lines: vec![],
                hunk_hash: 1000 + i as u64,
                reviewed: true,
            });
        }
        DiffModel {
            worktree: PathBuf::from("/tmp/fake"),
            branch: "feature".into(),
            base: "main".into(),
            merge_base: "deadbeef".into(),
            dirty: n_unreviewed + n_reviewed > 0,
            files: vec![FileDiff {
                path: PathBuf::from("src/lib.rs"),
                status: FileStatus::Modified,
                hunks,
                added: 0,
                removed: 0,
            }],
        }
    }

    /// The all-clear case: every hunk reviewed, both worktrees clean — the
    /// gate ALLOWS (`Ok(())`).
    #[test]
    fn allows_when_all_reviewed_and_both_worktrees_clean() {
        let model = model_with(0, 3);
        assert_eq!(merge_gate_decision(&model, true, true), Ok(()));
    }

    /// Spec B7 reason #1: any unreviewed hunk refuses, carrying the count —
    /// this is the check the NEGATIVE CONTROL below disables to prove the
    /// guard is load-bearing.
    #[test]
    fn refuses_with_unreviewed_count_when_hunks_unreviewed() {
        let model = model_with(2, 1);
        assert_eq!(
            merge_gate_decision(&model, true, true),
            Err(MergeRefusal::UnreviewedHunks(2))
        );
    }

    /// Spec B7 reason #2: a dirty feature worktree refuses even when every
    /// hunk is reviewed.
    #[test]
    fn refuses_when_feature_worktree_dirty() {
        let model = model_with(0, 1);
        assert_eq!(
            merge_gate_decision(&model, false, true),
            Err(MergeRefusal::FeatureWorktreeDirty)
        );
    }

    /// Spec B7 reason #3: a dirty primary checkout refuses even when
    /// everything about the feature side is clean/reviewed.
    #[test]
    fn refuses_when_primary_checkout_dirty() {
        let model = model_with(0, 1);
        assert_eq!(
            merge_gate_decision(&model, true, false),
            Err(MergeRefusal::PrimaryCheckoutDirty)
        );
    }

    /// Unreviewed hunks take priority over a dirty primary checkout when
    /// BOTH are true — the returned reason is deterministic, not whichever
    /// happened to be checked last.
    #[test]
    fn unreviewed_hunks_take_priority_over_dirty_checkouts() {
        let model = model_with(1, 0);
        assert_eq!(
            merge_gate_decision(&model, false, false),
            Err(MergeRefusal::UnreviewedHunks(1))
        );
    }

    /// `MergeRefusal::Display` renders the human-readable reason strings the
    /// spec names verbatim (`transient_status` text — spec B7: "refuses with
    /// the unreviewed count" / "feature worktree dirty" / "primary checkout
    /// dirty").
    #[test]
    fn merge_refusal_display_matches_spec_wording() {
        assert_eq!(MergeRefusal::UnreviewedHunks(1).to_string(), "1 unreviewed hunk");
        assert_eq!(MergeRefusal::UnreviewedHunks(3).to_string(), "3 unreviewed hunks");
        assert_eq!(MergeRefusal::FeatureWorktreeDirty.to_string(), "feature worktree dirty");
        assert_eq!(MergeRefusal::PrimaryCheckoutDirty.to_string(), "primary checkout dirty");
    }
}
