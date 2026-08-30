//! Per-branch reviewed-hash persistence for the Diff Review tile.
//!
//! Scaffold stub — implemented by cog node `review-state` (m7xl).
//! Stores `{ reviewed_hashes: [u64] }` at
//! `$(git rev-parse --git-common-dir)/yalda-review/<branch>.json`, joins
//! reviewed flags into a `DiffModel`, GCs dead hashes on write, and takes a
//! `*_PATH_OVERRIDE` seam under `cfg(test)`.
//! See docs/specs/spec-diff-review.md § Data Model / C5.
#![allow(dead_code)]

use super::*;

use std::path::Path;

/// The persisted, per-branch record of reviewed hunk hashes (spec § Data
/// Model). Lives at `<git-common-dir>/yalda-review/<branch>.json` — the git
/// common dir is shared by the primary checkout and every linked worktree, so
/// marks written from a feature worktree are visible to a hook running a
/// merge in the primary checkout, and it is never inside the tracked tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewState {
    reviewed_hashes: HashSet<u64>,
}

/// On-disk shape: `{ "reviewed_hashes": [u64, ...] }`.
#[derive(serde::Serialize, serde::Deserialize)]
struct ReviewStateFile {
    reviewed_hashes: Vec<u64>,
}

impl ReviewState {
    /// `true` iff `hash` is currently marked reviewed.
    pub fn is_reviewed(&self, hash: u64) -> bool {
        self.reviewed_hashes.contains(&hash)
    }

    /// Mark `hash` reviewed.
    pub fn mark_reviewed(&mut self, hash: u64) {
        self.reviewed_hashes.insert(hash);
    }

    /// Mark `hash` unreviewed.
    pub fn mark_unreviewed(&mut self, hash: u64) {
        self.reviewed_hashes.remove(&hash);
    }

    /// Flip `hash`'s reviewed state; returns the new state (spec B5 `v`
    /// keypress toggle).
    pub fn toggle(&mut self, hash: u64) -> bool {
        if self.reviewed_hashes.remove(&hash) {
            false
        } else {
            self.reviewed_hashes.insert(hash);
            true
        }
    }

    /// Drop any hash not present in `live_hashes` — the GC pass that runs on
    /// every [`save`](ReviewState::save) (spec: "Hashes of hunks that no
    /// longer exist are garbage-collected on write").
    pub fn gc(&mut self, live_hashes: &HashSet<u64>) {
        self.reviewed_hashes.retain(|h| live_hashes.contains(h));
    }
}

/// Root directory under the git common dir where per-branch review files
/// live: `<git-common-dir>/yalda-review/`.
const REVIEW_STATE_DIRNAME: &str = "yalda-review";

/// Test-only seam: redirect the review-state root to a tempdir so tests never
/// touch a real repo's `.git` (spec C5). Thread-local, so parallel tests don't
/// collide. Mirrors `ACP_PERSIST_PATH_OVERRIDE` / `SUMMARIES_PATH_OVERRIDE` in
/// `persist.rs`.
#[cfg(test)]
thread_local! {
    static REVIEW_STATE_PATH_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_review_state_path_override(root: PathBuf) {
    REVIEW_STATE_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(root));
}

#[cfg(test)]
pub(crate) fn clear_review_state_path_override() {
    REVIEW_STATE_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
}

#[cfg(test)]
pub(crate) fn with_review_state_path_override<R>(root: PathBuf, f: impl FnOnce() -> R) -> R {
    set_review_state_path_override(root);
    let r = f();
    clear_review_state_path_override();
    r
}

/// Resolve the `yalda-review/` root given a git common dir. Pure path
/// arithmetic — no I/O, no subprocess. Under `cfg(test)` the override (if set)
/// wins over the passed-in `git_common_dir`, so a test never has to construct
/// a real `.git` to exercise load/save.
fn review_state_root(git_common_dir: &Path) -> PathBuf {
    #[cfg(test)]
    {
        if let Some(over) = REVIEW_STATE_PATH_OVERRIDE.with(|c| c.borrow().clone()) {
            return over;
        }
    }
    git_common_dir.join(REVIEW_STATE_DIRNAME)
}

/// Path to a single branch's review-state JSON file, given the git common
/// dir. Pure — the caller resolves `git_common_dir` (see
/// [`resolve_git_common_dir`]) so this function itself never shells out
/// (spec C2: `ReviewState` I/O never runs on the render path, and the git
/// call is kept out of the pure load/save core).
fn review_state_file_path(git_common_dir: &Path, branch: &str) -> PathBuf {
    review_state_root(git_common_dir).join(format!("{}.json", sanitize_branch(branch)))
}

/// Branch names can contain `/` (e.g. `feature/foo`); turn that into a flat
/// filename component so `save` never tries to create a nested directory
/// structure it didn't ask for. `/` becomes `--` (a branch name cannot
/// contain a literal `--`-adjacent NUL, and this is a filename, not a git
/// ref, so no round-trip requirement exists — it only needs to be stable).
fn sanitize_branch(branch: &str) -> String {
    branch.replace('/', "--")
}

/// Load a branch's review state from disk. Missing file ⇒ empty state, not an
/// error (spec: "load: read the branch's JSON (missing file => empty state,
/// not an error)"). A present-but-unparseable file is treated the same way —
/// a corrupt sidecar should never crash the tile, it should just look
/// unreviewed.
pub fn load_review_state(git_common_dir: &Path, branch: &str) -> ReviewState {
    let path = review_state_file_path(git_common_dir, branch);
    let Ok(bytes) = std::fs::read(&path) else {
        return ReviewState::default();
    };
    let Ok(parsed) = serde_json::from_slice::<ReviewStateFile>(&bytes) else {
        return ReviewState::default();
    };
    ReviewState {
        reviewed_hashes: parsed.reviewed_hashes.into_iter().collect(),
    }
}

/// Persist a branch's review state to disk, garbage-collecting any hash not
/// present in `model`'s current hunks first (spec: "On save, GARBAGE-COLLECT:
/// drop any hash NOT present in the current DiffModel"). `state` is mutated
/// in place so the caller's in-memory copy reflects the GC too.
///
/// Best-effort: a failure to create the directory or write the file is
/// swallowed (mirrors `persist.rs`'s treatment of sidecar files — a review
/// mark is a nicety, not a reason to crash the tile), but the in-memory `state`
/// is still GC'd either way.
pub fn save_review_state(git_common_dir: &Path, branch: &str, state: &mut ReviewState, model: &DiffModel) {
    let live: HashSet<u64> = model
        .files
        .iter()
        .flat_map(|f| f.hunks.iter().map(|h| h.hunk_hash))
        .collect();
    state.gc(&live);

    let path = review_state_file_path(git_common_dir, branch);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut sorted: Vec<u64> = state.reviewed_hashes.iter().copied().collect();
    sorted.sort_unstable();
    let file = ReviewStateFile {
        reviewed_hashes: sorted,
    };
    if let Ok(json) = serde_json::to_vec_pretty(&file) {
        let _ = std::fs::write(&path, json);
    }
}

/// Resolve `$(git rev-parse --git-common-dir)` for `worktree` by shelling out.
/// The one place this module touches a subprocess — callers invoke this off
/// the paint path (spec C2) and pass the resolved path into the pure
/// load/save functions above. Returns `None` if `worktree` isn't inside a git
/// repo or the `git` binary can't be found/run.
pub fn resolve_git_common_dir(worktree: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    // `git rev-parse --git-common-dir` returns a path relative to `worktree`
    // when the common dir is the ordinary `<worktree>/.git` (the non-linked-
    // worktree case) — anchor it so callers always get an absolute path.
    let anchored = if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    };
    anchored.canonicalize().ok().or(Some(anchored))
}

/// Join reviewed flags from `state` into `model` at derive time (spec §
/// Interfaces: "joined from ReviewState at derive time"). Sets each hunk's
/// `reviewed` flag from `state.is_reviewed(hunk.hunk_hash)`, overwriting
/// whatever the parser produced (the parser always emits `false`).
pub fn join_reviewed_flags(model: &mut DiffModel, state: &ReviewState) {
    for file in &mut model.files {
        for hunk in &mut file.hunks {
            hunk.reviewed = state.is_reviewed(hunk.hunk_hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as StdPath; // alias avoids clashing with outer `use std::path::Path`

    /// Build a minimal one-hunk `DiffModel` for a given hash, so tests don't
    /// need real diff text — GC/join only look at `hunk_hash`.
    fn model_with_hashes(hashes: &[u64]) -> DiffModel {
        DiffModel {
            worktree: PathBuf::from("/tmp/fake-worktree"),
            branch: "feature/x".into(),
            base: "main".into(),
            merge_base: "deadbeef".into(),
            dirty: false,
            files: vec![FileDiff {
                path: PathBuf::from("src/lib.rs"),
                status: FileStatus::Modified,
                hunks: hashes
                    .iter()
                    .map(|&h| Hunk {
                        header: "@@ -1,1 +1,1 @@".into(),
                        lines: vec![],
                        hunk_hash: h,
                        reviewed: false,
                    })
                    .collect(),
                added: 0,
                removed: 0,
            }],
        }
    }

    /// Round-trip: mark a hash, save, reload from disk (a *fresh*
    /// `ReviewState`, not the in-memory one) — is_reviewed must come back
    /// true. Everything happens under a tempdir override so nothing outside
    /// it is ever touched (spec C5).
    #[test]
    fn round_trip_mark_persists_across_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        with_review_state_path_override(dir.path().to_path_buf(), || {
            let common_dir = StdPath::new("/unused-because-overridden");
            let branch = "feature/x";
            let hash = 111_222_333_u64;
            let model = model_with_hashes(&[hash]);

            let mut state = load_review_state(common_dir, branch);
            assert!(!state.is_reviewed(hash), "fresh state starts unreviewed");
            state.mark_reviewed(hash);
            save_review_state(common_dir, branch, &mut state, &model);

            // Reload as an independent `ReviewState` — proves the mark
            // actually reached disk, not just the in-memory struct.
            let reloaded = load_review_state(common_dir, branch);
            assert!(reloaded.is_reviewed(hash), "mark must survive reload");
        });
    }

    /// Editing a hunk's content changes its `hunk_hash` (per spec: hash is
    /// content-derived). The *new* hash for the edited hunk must read back
    /// unreviewed even though the *old* hash for that same logical hunk was
    /// marked — staleness needs no timestamps, it falls out of the hash
    /// changing.
    #[test]
    fn edited_hunk_hash_is_unreviewed() {
        let dir = tempfile::tempdir().expect("tempdir");
        with_review_state_path_override(dir.path().to_path_buf(), || {
            let common_dir = StdPath::new("/unused-because-overridden");
            let branch = "feature/x";
            let old_hash = 1_u64;
            let new_hash = 2_u64; // stand-in for "content edited => new hash"

            let mut state = load_review_state(common_dir, branch);
            state.mark_reviewed(old_hash);
            let model = model_with_hashes(&[old_hash]);
            save_review_state(common_dir, branch, &mut state, &model);

            let reloaded = load_review_state(common_dir, branch);
            assert!(reloaded.is_reviewed(old_hash));
            assert!(
                !reloaded.is_reviewed(new_hash),
                "a hash that was never marked must never read as reviewed"
            );
        });
    }

    /// GC test: a previously-marked hash that is absent from the current
    /// `DiffModel`'s hunks at save time must be dropped from the persisted
    /// file — not merely left un-set in a fresh load, but actually gone from
    /// what's on disk. Constructed so it would FAIL if `save_review_state`
    /// forgot to GC (the reload would still show the dead hash reviewed).
    #[test]
    fn save_garbage_collects_dead_hashes() {
        let dir = tempfile::tempdir().expect("tempdir");
        with_review_state_path_override(dir.path().to_path_buf(), || {
            let common_dir = StdPath::new("/unused-because-overridden");
            let branch = "feature/x";
            let live_hash = 10_u64;
            let dead_hash = 99_u64;

            let mut state = load_review_state(common_dir, branch);
            state.mark_reviewed(live_hash);
            state.mark_reviewed(dead_hash);

            // Current model only knows about `live_hash` — `dead_hash`
            // belongs to a hunk that no longer exists (e.g. its diff was
            // resolved/removed).
            let model = model_with_hashes(&[live_hash]);
            save_review_state(common_dir, branch, &mut state, &model);

            // In-memory copy is GC'd immediately.
            assert!(state.is_reviewed(live_hash));
            assert!(!state.is_reviewed(dead_hash));

            // And the persisted file agrees — reload independently.
            let reloaded = load_review_state(common_dir, branch);
            assert!(reloaded.is_reviewed(live_hash));
            assert!(
                !reloaded.is_reviewed(dead_hash),
                "dead hash must be gone from disk after save, not just skipped on load"
            );
        });
    }

    /// `join_reviewed_flags` sets each hunk's `reviewed` bool from the state,
    /// per hunk_hash — including leaving a not-reviewed hunk false, so this
    /// can't pass by e.g. setting everything true.
    #[test]
    fn join_sets_reviewed_flag_per_hunk_hash() {
        let reviewed_hash = 5_u64;
        let unreviewed_hash = 6_u64;
        let mut state = ReviewState::default();
        state.mark_reviewed(reviewed_hash);

        let mut model = model_with_hashes(&[reviewed_hash, unreviewed_hash]);
        join_reviewed_flags(&mut model, &state);

        assert!(model.files[0].hunks[0].reviewed);
        assert!(!model.files[0].hunks[1].reviewed);
    }

    /// Toggle flips both directions.
    #[test]
    fn toggle_flips_reviewed_state() {
        let mut state = ReviewState::default();
        let hash = 42_u64;
        assert!(!state.is_reviewed(hash));
        assert!(state.toggle(hash));
        assert!(state.is_reviewed(hash));
        assert!(!state.toggle(hash));
        assert!(!state.is_reviewed(hash));
    }

    /// Nothing is written outside the tempdir override root: the review-state
    /// file lands INSIDE the override dir, and the override dir's parent
    /// gains no new unexpected siblings (guards against a path-join bug that
    /// escapes the sandbox, e.g. via an absolute-looking branch name).
    #[test]
    fn writes_stay_inside_override_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        with_review_state_path_override(root.clone(), || {
            let common_dir = StdPath::new("/unused-because-overridden");
            let mut state = ReviewState::default();
            state.mark_reviewed(1);
            let model = model_with_hashes(&[1]);
            save_review_state(common_dir, "main", &mut state, &model);
        });

        let expected = root.join("main.json");
        assert!(expected.is_file(), "expected file at {:?}", expected);
        // Everything created lives directly under `root` — walk it and
        // confirm every entry's path starts with `root`.
        for entry in walkdir_flat(&root) {
            assert!(
                entry.starts_with(&root),
                "escaped the override root: {:?}",
                entry
            );
        }
    }

    /// Tiny non-recursive-dep directory walk (avoids pulling in a walkdir
    /// crate dependency just for one test assertion).
    fn walkdir_flat(root: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        out
    }
}
