//! Async git subprocess helper for the Diff Review tile.
//!
//! Cog node `git-boundary` (ln8z). Runs `git` for a worktree off the paint
//! path and returns raw output (merge-base, diff, status, untracked
//! listing, worktree list, branch). See docs/specs/spec-diff-review.md §
//! Interfaces / Constraint C1.
//!
//! This module does ONE thing: shell out to `git` and hand back raw text.
//! It never parses diff content (that's `diff_model.rs`, spec C1) and never
//! mutates the worktree's index or files (spec C3 / B2 — no `git add -N`,
//! no `git apply`). Every failure mode (missing worktree, git not on PATH,
//! non-zero exit, non-UTF-8 output) is returned as a `GitDiffError` value;
//! this module never panics on bad input (spec B1).
#![allow(dead_code)]

use super::*;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Fallback base branch when the caller doesn't name one (spec: "Base
/// defaults to the repo's default branch, e.g. main"). A future revision
/// may resolve the real default branch (`origin/HEAD`); until then this is
/// the documented default.
pub(crate) const DEFAULT_BASE_BRANCH: &str = "main";

/// Raw, unparsed git output collected for one worktree. Every field is a
/// straight dump of a git invocation's stdout — no interpretation happens
/// here; `diff_model.rs` turns `diff_text` into a `DiffModel` (spec C1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawGitDiff {
    /// Current branch name (`git rev-parse --abbrev-ref HEAD`); `"HEAD"`
    /// for a detached checkout.
    pub(crate) branch: String,
    /// The base branch this was diffed against (echoed back for the model).
    pub(crate) base: String,
    /// SHA of `git merge-base <base> HEAD`.
    pub(crate) merge_base: String,
    /// Whether `git status --porcelain` reported anything (tracked
    /// modifications OR untracked files).
    pub(crate) dirty: bool,
    /// Cumulative raw diff text: `git diff <merge_base> --no-color`
    /// (committed + uncommitted changes both appear) with each untracked
    /// file's all-added diff appended via a non-mutating
    /// `git diff --no-index /dev/null <file>` comparison (spec B2 — never
    /// `git add -N`, never touches the index).
    pub(crate) diff_text: String,
    /// Raw `git worktree list` output — needed by the merge gate/hook to
    /// resolve a branch's worktree (spec Interfaces).
    pub(crate) worktree_list: String,
}

/// Everything that can go wrong collecting a `RawGitDiff`, surfaced as a
/// value rather than a panic (spec B1: a deleted/invalid worktree must
/// produce an error state, never crash the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitDiffError {
    /// The worktree path doesn't exist / isn't a directory.
    InvalidWorktree(PathBuf),
    /// A `git` invocation failed to even start (binary missing, etc).
    Spawn { command: String, reason: String },
    /// `git` ran but exited with a status this module doesn't treat as
    /// success for that command.
    CommandFailed { command: String, stderr: String },
    /// stdout wasn't valid UTF-8.
    InvalidUtf8 { command: String },
}

impl std::fmt::Display for GitDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitDiffError::InvalidWorktree(path) => {
                write!(f, "not a git worktree: {}", path.display())
            }
            GitDiffError::Spawn { command, reason } => {
                write!(f, "failed to run `{command}`: {reason}")
            }
            GitDiffError::CommandFailed { command, stderr } => {
                write!(f, "`{command}` failed: {stderr}")
            }
            GitDiffError::InvalidUtf8 { command } => {
                write!(f, "`{command}` produced non-UTF-8 output")
            }
        }
    }
}

impl std::error::Error for GitDiffError {}

/// Run `git <args>` in `worktree`, requiring a zero exit status, and return
/// raw stdout. Spawn / exit / encoding failures come back as `GitDiffError`
/// values (never panics).
fn run_git(worktree: &Path, args: &[&str]) -> Result<String, GitDiffError> {
    let label = format!("git {}", args.join(" "));
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|e| GitDiffError::Spawn {
            command: label.clone(),
            reason: e.to_string(),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(GitDiffError::CommandFailed {
            command: label,
            stderr,
        });
    }
    String::from_utf8(out.stdout).map_err(|_| GitDiffError::InvalidUtf8 { command: label })
}

/// Run `git diff --no-index --no-color /dev/null <rel_path>` in `worktree`
/// to produce an untracked file's all-added patch, WITHOUT touching the
/// index or worktree (spec B2 — this is the non-mutating replacement for
/// `git add -N`). `--no-index` exits 1 when the two sides differ (the
/// expected case here, comparing against `/dev/null`) — that is success,
/// not an error; only exit codes `>= 2` are treated as a real failure.
fn run_git_diff_no_index(worktree: &Path, rel_path: &str) -> Result<String, GitDiffError> {
    let label = format!("git diff --no-index --no-color /dev/null {rel_path}");
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--no-index", "--no-color", "/dev/null", rel_path])
        .output()
        .map_err(|e| GitDiffError::Spawn {
            command: label.clone(),
            reason: e.to_string(),
        })?;
    match out.status.code() {
        Some(0) | Some(1) => {
            String::from_utf8(out.stdout).map_err(|_| GitDiffError::InvalidUtf8 { command: label })
        }
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            Err(GitDiffError::CommandFailed {
                command: label,
                stderr,
            })
        }
    }
}

/// Collect a `RawGitDiff` for `worktree` against `base` (defaulting to
/// `DEFAULT_BASE_BRANCH` when `None`). `async fn` so a caller runs it on
/// GPUI's background executor (`cx.background_executor().spawn(..)`,
/// matching the pattern in `cog.rs`) — this function itself performs no
/// I/O beyond the blocking `git` invocations, so it must never be awaited
/// directly on the paint path (spec C2 / DONE_WHEN #2).
///
/// Every git call is `-C <worktree>`-scoped, so a deleted/invalid worktree
/// is caught up front (`InvalidWorktree`) instead of surfacing as a
/// confusing downstream git error.
pub(crate) async fn collect_raw_diff(
    worktree: PathBuf,
    base: Option<String>,
) -> Result<RawGitDiff, GitDiffError> {
    if !worktree.is_dir() {
        return Err(GitDiffError::InvalidWorktree(worktree));
    }
    let base = base.unwrap_or_else(|| DEFAULT_BASE_BRANCH.to_string());

    let branch = run_git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let merge_base = run_git(&worktree, &["merge-base", &base, "HEAD"])?
        .trim()
        .to_string();

    let mut diff_text = run_git(&worktree, &["diff", &merge_base, "--no-color"])?;

    let status_out = run_git(&worktree, &["status", "--porcelain"])?;
    let dirty = !status_out.trim().is_empty();

    let untracked = run_git(&worktree, &["ls-files", "--others", "--exclude-standard"])?;
    for rel_path in untracked.lines().filter(|line| !line.is_empty()) {
        let patch = run_git_diff_no_index(&worktree, rel_path)?;
        if !patch.is_empty() {
            if !diff_text.is_empty() && !diff_text.ends_with('\n') {
                diff_text.push('\n');
            }
            diff_text.push_str(&patch);
        }
    }

    let worktree_list = run_git(&worktree, &["worktree", "list"])?;

    Ok(RawGitDiff {
        branch,
        base,
        merge_base,
        dirty,
        diff_text,
        worktree_list,
    })
}

// ── Cog node `merge-gate` (v5tg): spec B7 git execution ─────────────────────
//
// The thin, real-git half of the merge gate — the pure go/no-go call is
// `diff.rs::merge_gate_decision`; everything below just shells out (spec C1)
// and is invoked off the paint path by `diff_ui.rs::run_merge_gate` /
// `install_merge_gate_hook` (spec C2). None of this parses diff content, so
// it stays out of `diff_model.rs` (spec C1's "parsing lives in a pure
// module" is about DIFF parsing specifically).

/// `git status --porcelain` in `path`, reporting whether it printed nothing
/// at all (spec B7 "clean"). Deliberately distinct from `DiffModel::dirty`,
/// which means "differs from merge-base" (see `diff_model.rs`'s `parse_diff`
/// docs) — a hunk can exist (committed ahead of base) while the WORKING TREE
/// itself has no uncommitted changes, which is exactly the state a clean
/// feature worktree is in right before a review-gated merge.
pub(crate) fn worktree_is_clean(path: &Path) -> Result<bool, GitDiffError> {
    Ok(run_git(path, &["status", "--porcelain"])?.trim().is_empty())
}

/// Why `execute_merge_no_ff` refused/failed. Carries git's combined
/// stdout+stderr for the status message — `git merge`'s conflict summary
/// ("Auto-merging...", "CONFLICT...") is printed to STDOUT, so stderr alone
/// is often empty for the common conflict case. The abort has ALREADY
/// happened by the time a caller sees this (see the function docs) — there
/// is nothing left for the caller to clean up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeConflict {
    pub(crate) message: String,
}

impl std::fmt::Display for MergeConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message.trim())
    }
}

/// Merge `branch` into whatever is checked out in `primary` via
/// `git merge --no-ff` (spec B7: "the merge executes in the primary
/// checkout"). On ANY non-zero exit — a real content conflict, or any other
/// merge-blocking condition — this unconditionally runs `git merge --abort`
/// in `primary` BEFORE returning `Err`, per spec B7's hard requirement that
/// "the tile never leaves conflict markers in a live checkout": a caller
/// never has to remember to clean up after a failed merge, because by the
/// time this function returns, there is nothing left to clean up.
pub(crate) fn execute_merge_no_ff(primary: &Path, branch: &str) -> Result<(), MergeConflict> {
    let out = Command::new("git")
        .arg("-C")
        .arg(primary)
        .args(["merge", "--no-ff", branch])
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            return Err(MergeConflict {
                message: format!("failed to spawn `git merge`: {e}"),
            });
        }
    };
    if out.status.success() {
        return Ok(());
    }
    // `git merge`'s conflict summary ("Auto-merging...", "CONFLICT...") goes
    // to STDOUT, not stderr — only a handful of git error paths use stderr —
    // so both streams are captured and joined to make sure the reported
    // reason is never empty for the common conflict case.
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&out.stderr);
    if !stderr_text.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr_text);
    }
    // Best-effort abort: if THIS also fails (e.g. there was nothing to
    // abort because the merge failed before even starting one), there is
    // nothing more we can do beyond reporting the original failure — but we
    // always attempt it, because leaving conflict markers is the one
    // outcome spec B7 rules out entirely.
    let _ = Command::new("git")
        .arg("-C")
        .arg(primary)
        .args(["merge", "--abort"])
        .output();
    Err(MergeConflict { message: combined })
}

// ── Cog node `merge-gate` (v5tg): spec B7 hook installer ────────────────────

/// Marker line every hook file THIS installer writes carries at its top, so
/// a re-install can tell "a hook we own" (safe to overwrite) apart from "a
/// foreign hook that predates us" (must be preserved, not clobbered).
const YALDA_HOOK_MARKER: &str = "# yalda-merge-gate-hook (auto-installed by yalda-gpui; safe to remove)";

/// The canonical hook logic, checked into the repo at
/// `scripts/yalda-pre-merge-hook` (same file a human/CI can read, and the
/// literal file the hook tests in `verify_harness.rs` shell out to
/// directly). Embedded at COMPILE time so the installed hook is fully
/// self-contained in the TARGET repo — it must keep working even if this
/// yaldabaoth checkout later moves or is deleted.
const PRE_MERGE_COMMIT_HOOK_SOURCE: &str =
    include_str!("../../../scripts/yalda-pre-merge-hook");

/// `pre-commit` fragment installed alongside `pre-merge-commit` (spec B7:
/// "installs a pre-commit fragment that runs the same check when MERGE_HEAD
/// exists"). Deliberately does NOT duplicate the check logic — it just execs
/// the sibling `pre-merge-commit` hook (which is itself name-agnostic; it
/// only branches on whether `MERGE_HEAD` exists) when a merge is in
/// progress, then chains to whatever pre-commit hook was already installed
/// here before this installer ran (renamed to `pre-commit.pre-yalda` — see
/// `install_pre_commit_fragment`), so installing this gate never silently
/// disables an unrelated pre-existing pre-commit hook.
const PRE_COMMIT_FRAGMENT: &str = "#!/bin/sh
# yalda-merge-gate-hook (auto-installed by yalda-gpui; safe to remove)
# Cog node `merge-gate` (v5tg), spec B7: `pre-merge-commit` never fires for a
# merge finished by hand via `git commit` after resolving conflicts, so this
# fragment re-runs the SAME check (the installed `pre-merge-commit` hook is
# name-agnostic — it only cares whether MERGE_HEAD exists) whenever
# MERGE_HEAD is present, then chains to whatever pre-commit hook was already
# installed here (if any), preserving it.
set -eu
HOOKS_DIR=\"$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\"
if git rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then
    \"$HOOKS_DIR/pre-merge-commit\" || exit 1
fi
if [ -x \"$HOOKS_DIR/pre-commit.pre-yalda\" ]; then
    exec \"$HOOKS_DIR/pre-commit.pre-yalda\"
fi
exit 0
";

/// Write `content` to `path` and mark it executable (spec: hooks must be
/// runnable by git directly).
fn write_executable_hook(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| format!("couldn't stat {}: {e}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|e| format!("couldn't chmod {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Install (or update) the `pre-commit` fragment in `hooks_dir`, preserving
/// any FOREIGN (not-ours) pre-existing `pre-commit` hook by renaming it to
/// `pre-commit.pre-yalda` the first time — never on a subsequent reinstall,
/// which would otherwise clobber the real backup with our own fragment's
/// prior content.
fn install_pre_commit_fragment(hooks_dir: &Path) -> Result<(), String> {
    let pre_commit = hooks_dir.join("pre-commit");
    let backup = hooks_dir.join("pre-commit.pre-yalda");
    if pre_commit.exists() {
        let existing = std::fs::read_to_string(&pre_commit).unwrap_or_default();
        if !existing.contains(YALDA_HOOK_MARKER) && !backup.exists() {
            std::fs::rename(&pre_commit, &backup)
                .map_err(|e| format!("couldn't back up existing pre-commit hook: {e}"))?;
        }
    }
    write_executable_hook(&pre_commit, PRE_COMMIT_FRAGMENT)
}

/// Cog node `merge-gate` (v5tg), spec B7 installer (`diff_ui.rs`'s
/// `diff_install_hook_focused` is the only caller): installs the two-layer
/// merge-gate hook into `worktree`'s git COMMON dir (shared by every linked
/// worktree of this repo, spec § Data Model) — `hooks/pre-merge-commit` with
/// `yalda_gpui_bin`'s resolved absolute path baked into the
/// `@@YALDA_GPUI_BIN@@` placeholder (spec: "bakes the RESOLVED ABSOLUTE path
/// ... into the installed hook"), a chained `hooks/pre-commit` fragment, and
/// `merge.ff false` (spec: "`pre-merge-commit` does not fire on fast-forward
/// merges"). Never called automatically — spec B7: "installed per-repo by an
/// explicit tile command, never automatically". Idempotent: re-running
/// updates our own fragments in place without re-chaining a second copy of a
/// pre-existing foreign hook.
pub(crate) fn install_merge_gate_hook(worktree: &Path, yalda_gpui_bin: &Path) -> Result<String, String> {
    let common = resolve_git_common_dir(worktree)
        .ok_or_else(|| format!("not a git repo (or git not found): {}", worktree.display()))?;
    let hooks_dir = common.join("hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("couldn't create {}: {e}", hooks_dir.display()))?;

    let bin_str = yalda_gpui_bin.to_string_lossy();
    let pre_merge_commit = PRE_MERGE_COMMIT_HOOK_SOURCE.replace("@@YALDA_GPUI_BIN@@", &bin_str);
    write_executable_hook(&hooks_dir.join("pre-merge-commit"), &pre_merge_commit)?;

    install_pre_commit_fragment(&hooks_dir)?;

    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["config", "merge.ff", "false"])
        .output()
        .map_err(|e| format!("couldn't run git config: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "installed hooks in {} but `git config merge.ff false` failed: {}",
            hooks_dir.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    Ok(format!(
        "merge-gate hook installed in {} (pre-merge-commit + pre-commit fragment, merge.ff=false)",
        hooks_dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `git <args>` in `dir` for fixture setup, panicking on failure —
    /// this is test-fixture plumbing, not the code under test (that's
    /// `run_git` above, exercised through `collect_raw_diff`).
    fn git_ok(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    /// Build a repo with a `main` branch and a `feature` branch one commit
    /// ahead, then leave uncommitted + untracked changes on `feature` — the
    /// shape needed to exercise committed + uncommitted + untracked all at
    /// once, entirely inside a tempdir (spec C5 — never the user's repos).
    fn build_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();

        git_ok(dir, &["init", "--quiet"]);
        git_ok(dir, &["config", "user.email", "test@example.com"]);
        git_ok(dir, &["config", "user.name", "Test"]);
        git_ok(dir, &["config", "commit.gpgsign", "false"]);

        std::fs::write(dir.join("a.txt"), "line1\n").unwrap();
        git_ok(dir, &["add", "a.txt"]);
        git_ok(dir, &["commit", "--quiet", "-m", "initial"]);
        // Normalize the branch name regardless of the local `init.defaultBranch`.
        git_ok(dir, &["branch", "-M", "main"]);

        git_ok(dir, &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.join("a.txt"), "line1\ncommitted line\n").unwrap();
        git_ok(dir, &["add", "a.txt"]);
        git_ok(dir, &["commit", "--quiet", "-m", "committed change"]);

        // Uncommitted change to a tracked file.
        std::fs::write(dir.join("a.txt"), "line1\ncommitted line\nuncommitted line\n").unwrap();

        // Untracked file.
        std::fs::write(dir.join("new.txt"), "untracked content\n").unwrap();

        temp
    }

    /// DONE_WHEN #1: a tempdir git fixture derives a full raw diff with
    /// committed + uncommitted + untracked changes all present, without
    /// touching any real repo.
    #[test]
    fn collects_committed_uncommitted_and_untracked_changes() {
        let temp = build_fixture();
        let worktree = temp.path().to_path_buf();

        let result = futures::executor::block_on(collect_raw_diff(
            worktree.clone(),
            Some("main".to_string()),
        ));
        let raw = result.expect("collect_raw_diff should succeed on a valid fixture");

        assert_eq!(raw.branch, "feature");
        assert_eq!(raw.base, "main");
        assert!(raw.dirty, "uncommitted + untracked changes must mark dirty");

        let expected_merge_base = std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["merge-base", "main", "feature"])
            .output()
            .expect("run git merge-base for comparison");
        let expected_merge_base = String::from_utf8_lossy(&expected_merge_base.stdout)
            .trim()
            .to_string();
        assert_eq!(raw.merge_base, expected_merge_base);

        assert!(
            raw.diff_text.contains("committed line"),
            "committed change missing from diff_text:\n{}",
            raw.diff_text
        );
        assert!(
            raw.diff_text.contains("uncommitted line"),
            "uncommitted change missing from diff_text:\n{}",
            raw.diff_text
        );
        assert!(
            raw.diff_text.contains("untracked content"),
            "untracked file's added content missing from diff_text:\n{}",
            raw.diff_text
        );
        // The untracked file must appear as an ADDED line (`+...`), not just
        // present somewhere in the text.
        assert!(
            raw.diff_text
                .lines()
                .any(|l| l.starts_with('+') && l.contains("untracked content")),
            "untracked content must appear as an added diff line:\n{}",
            raw.diff_text
        );

        assert!(
            raw.worktree_list.contains(&worktree.to_string_lossy().to_string())
                || raw
                    .worktree_list
                    .contains(worktree.file_name().unwrap().to_str().unwrap()),
            "worktree_list should mention the fixture worktree:\n{}",
            raw.worktree_list
        );
    }

    /// spec B1: a deleted/invalid worktree must return an error value, not
    /// panic.
    #[test]
    fn missing_worktree_returns_error_not_panic() {
        let bogus = PathBuf::from("/nonexistent/definitely-not-a-repo-path-12345");
        let result = futures::executor::block_on(collect_raw_diff(bogus.clone(), None));
        assert_eq!(result, Err(GitDiffError::InvalidWorktree(bogus)));
    }

    /// A real directory that is nonetheless not a git repo must also error
    /// as a value rather than panicking (still B1: any invalid worktree).
    #[test]
    fn non_repo_directory_returns_error_not_panic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = futures::executor::block_on(collect_raw_diff(
            temp.path().to_path_buf(),
            None,
        ));
        assert!(
            matches!(result, Err(GitDiffError::CommandFailed { .. })),
            "expected a CommandFailed error for a non-repo directory, got {result:?}"
        );
    }

    /// This helper never mutates anything outside its own tempdir fixture:
    /// no `git add -N`, no writes to the fixture's index beyond the
    /// deliberate fixture-setup commits above, and the untracked file stays
    /// untracked after collection (spec C3 / B2).
    #[test]
    fn untracked_file_stays_untracked_after_collection() {
        let temp = build_fixture();
        let worktree = temp.path().to_path_buf();

        let _ = futures::executor::block_on(collect_raw_diff(worktree.clone(), None));

        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["status", "--porcelain"])
            .output()
            .expect("run git status");
        let status_text = String::from_utf8_lossy(&status.stdout);
        assert!(
            status_text.lines().any(|l| l.starts_with("??") && l.contains("new.txt")),
            "new.txt must remain untracked (?? in porcelain status) after collect_raw_diff:\n{status_text}"
        );
    }

    // ── Cog node `merge-gate` (v5tg): spec B7 ───────────────────────────────

    /// A two-worktree fixture: `<tmp>/primary` is the MAIN checkout (on
    /// `main`), `<tmp>/feature` is a LINKED worktree checked out on `feature`
    /// one commit ahead — the exact shape `execute_merge_no_ff` /
    /// `install_merge_gate_hook` are meant to operate over (spec B7: "the
    /// merge executes in the primary checkout"; the primary must be resolved
    /// via `git worktree list`, not assumed to be the same directory as the
    /// worktree being reviewed). Entirely inside a tempdir (spec C5).
    /// Returns `(tempdir, feature_worktree_path)`; the primary checkout is
    /// always `tempdir.path().join("primary")`.
    fn build_merge_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("primary");
        std::fs::create_dir_all(&primary).unwrap();

        git_ok(&primary, &["init", "--quiet"]);
        git_ok(&primary, &["config", "user.email", "test@example.com"]);
        git_ok(&primary, &["config", "user.name", "Test"]);
        git_ok(&primary, &["config", "commit.gpgsign", "false"]);

        std::fs::write(primary.join("a.txt"), "line1\n").unwrap();
        git_ok(&primary, &["add", "a.txt"]);
        git_ok(&primary, &["commit", "--quiet", "-m", "initial"]);
        git_ok(&primary, &["branch", "-M", "main"]);
        git_ok(&primary, &["branch", "feature"]);

        let feature = temp.path().join("feature");
        git_ok(
            &primary,
            &["worktree", "add", "--quiet", feature.to_str().unwrap(), "feature"],
        );
        std::fs::write(feature.join("a.txt"), "line1\nfeature change\n").unwrap();
        git_ok(&feature, &["add", "a.txt"]);
        git_ok(&feature, &["commit", "--quiet", "-m", "feature change"]);

        (temp, feature)
    }

    /// `worktree_is_clean` must distinguish a freshly-committed worktree
    /// (clean) from one with an uncommitted edit (dirty) — the exact
    /// "current git state" check spec B7 requires INSTEAD of trusting
    /// `DiffModel::dirty`.
    #[test]
    fn worktree_is_clean_reports_clean_then_dirty() {
        let (_temp, feature) = build_merge_fixture();
        assert!(
            worktree_is_clean(&feature).expect("status should succeed"),
            "freshly committed worktree must be clean"
        );
        std::fs::write(feature.join("a.txt"), "line1\nfeature change\nuncommitted\n").unwrap();
        assert!(
            !worktree_is_clean(&feature).expect("status should succeed"),
            "worktree with an uncommitted edit must be dirty"
        );
    }

    /// DONE_WHEN: "merge ALLOWED when all reviewed + clean... the real git
    /// merge succeeds in the fixture" — `execute_merge_no_ff` actually
    /// performs a real `--no-ff` merge in the primary checkout when there is
    /// no conflict, producing a merge commit (not a fast-forward).
    #[test]
    fn execute_merge_no_ff_merges_cleanly_when_no_conflict() {
        let (temp, _feature) = build_merge_fixture();
        let primary = temp.path().join("primary");

        let result = execute_merge_no_ff(&primary, "feature");
        assert!(result.is_ok(), "expected a clean merge, got {result:?}");

        let log = Command::new("git")
            .arg("-C")
            .arg(&primary)
            .args(["log", "-1", "--format=%s"])
            .output()
            .expect("git log");
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(
            subject.to_lowercase().contains("merge"),
            "expected a --no-ff merge commit (not a fast-forward), got subject: {subject:?}"
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("a.txt")).unwrap(),
            "line1\nfeature change\n",
            "feature's content must have actually landed in primary"
        );
        assert!(worktree_is_clean(&primary).expect("status should succeed"));
    }

    /// DONE_WHEN: "conflict path calls merge --abort and leaves NO conflict
    /// markers" — force a real content conflict (both sides edit the same
    /// line differently since their common ancestor), then assert
    /// `execute_merge_no_ff` reports failure AND the primary checkout is
    /// left clean with no `<<<<<<<` markers and no `MERGE_HEAD` — i.e. the
    /// abort actually ran, not just that an error was returned.
    #[test]
    fn execute_merge_no_ff_conflict_aborts_and_leaves_no_markers() {
        let (temp, _feature) = build_merge_fixture();
        let primary = temp.path().join("primary");

        // Diverge primary from the common ancestor with a CONFLICTING edit
        // to the same line `feature` already changed.
        std::fs::write(primary.join("a.txt"), "line1\nprimary change\n").unwrap();
        git_ok(&primary, &["add", "a.txt"]);
        git_ok(&primary, &["commit", "--quiet", "-m", "primary conflicting change"]);

        let result = execute_merge_no_ff(&primary, "feature");
        assert!(result.is_err(), "expected a merge conflict");
        assert!(
            !result.unwrap_err().message.is_empty(),
            "conflict error should carry git's stderr"
        );

        let content = std::fs::read_to_string(primary.join("a.txt")).unwrap();
        assert!(
            !content.contains("<<<<<<<"),
            "no conflict markers may remain in the working tree: {content:?}"
        );
        assert!(
            worktree_is_clean(&primary).expect("status should succeed"),
            "primary must be clean after merge --abort, not left mid-conflict"
        );
        assert!(
            !primary.join(".git").join("MERGE_HEAD").exists(),
            "MERGE_HEAD must be gone — the abort must have actually run"
        );
    }

    /// `install_merge_gate_hook` writes both hook files, bakes the resolved
    /// binary path into `pre-merge-commit` (spec: "bakes the RESOLVED
    /// ABSOLUTE path... into the installed hook"), marks them executable,
    /// and sets `merge.ff false` (spec: required because `pre-merge-commit`
    /// never fires on a fast-forward merge).
    #[test]
    fn install_merge_gate_hook_writes_hooks_bakes_path_and_sets_merge_ff_false() {
        let (_temp, feature) = build_merge_fixture();
        let bin = PathBuf::from("/opt/fake/yalda-gpui-test-binary");

        let msg = install_merge_gate_hook(&feature, &bin).expect("install should succeed");
        assert!(msg.contains("installed"), "got: {msg:?}");

        let common = resolve_git_common_dir(&feature).expect("common dir");
        let hooks_dir = common.join("hooks");

        let pre_merge_commit = std::fs::read_to_string(hooks_dir.join("pre-merge-commit"))
            .expect("pre-merge-commit must exist");
        assert!(
            pre_merge_commit.contains(bin.to_str().unwrap()),
            "the resolved binary path must be baked into the hook"
        );
        assert!(
            !pre_merge_commit.contains("@@YALDA_GPUI_BIN@@"),
            "the placeholder must be fully substituted"
        );

        let pre_commit =
            std::fs::read_to_string(hooks_dir.join("pre-commit")).expect("pre-commit must exist");
        assert!(pre_commit.contains("MERGE_HEAD"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["pre-merge-commit", "pre-commit"] {
                let mode = std::fs::metadata(hooks_dir.join(name))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_ne!(mode & 0o111, 0, "{name} must be executable");
            }
        }

        let cfg = Command::new("git")
            .arg("-C")
            .arg(&feature)
            .args(["config", "merge.ff"])
            .output()
            .expect("git config");
        assert_eq!(String::from_utf8_lossy(&cfg.stdout).trim(), "false");
    }

    /// Installing over a repo that ALREADY has an unrelated `pre-commit`
    /// hook must preserve it (as `pre-commit.pre-yalda`) rather than
    /// silently deleting it — and a SECOND install must not clobber that
    /// preserved backup with the fragment's own prior content.
    #[test]
    fn install_merge_gate_hook_preserves_foreign_pre_commit_hook() {
        let (_temp, feature) = build_merge_fixture();
        let common = resolve_git_common_dir(&feature).expect("common dir");
        let hooks_dir = common.join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let foreign_hook = "#!/bin/sh\necho existing-hook-ran\nexit 0\n";
        write_executable_hook(&hooks_dir.join("pre-commit"), foreign_hook)
            .expect("write foreign hook");

        install_merge_gate_hook(&feature, &PathBuf::from("/bin/true")).expect("install");

        let backup = std::fs::read_to_string(hooks_dir.join("pre-commit.pre-yalda"))
            .expect("the foreign hook must be preserved as a backup");
        assert_eq!(backup, foreign_hook);
        let installed = std::fs::read_to_string(hooks_dir.join("pre-commit")).unwrap();
        assert!(installed.contains(YALDA_HOOK_MARKER));

        // Reinstalling again must not clobber the ORIGINAL foreign backup.
        install_merge_gate_hook(&feature, &PathBuf::from("/bin/true")).expect("reinstall");
        let backup_after = std::fs::read_to_string(hooks_dir.join("pre-commit.pre-yalda")).unwrap();
        assert_eq!(
            backup_after, foreign_hook,
            "a reinstall must not overwrite the original foreign hook's backup"
        );
    }

    // ── Cog node `merge-gate` (v5tg): C6 (`--hash-diff`) + hook script ──────
    //
    // These tests shell out to the REAL `yalda-gpui` debug binary (for
    // `--hash-diff`) and the REAL `scripts/yalda-pre-merge-hook` file (not a
    // copy/rewrite of its logic) — the actual artifacts a human's git hook
    // would run, not a simulation of them.

    /// Absolute path to `target/debug/yalda-gpui`, derived from this TEST
    /// binary's own `current_exe()` (`.../target/debug/deps/yalda_gpui-<hash>`
    /// → `.../target/debug/yalda-gpui`) rather than `CARGO_BIN_EXE_*` — that
    /// env var is only populated for `tests/` integration binaries, not for
    /// `#[test]`s compiled into the binary crate itself (`cargo test --bin
    /// yalda-gpui`), which is what these are.
    fn yalda_gpui_bin_path() -> PathBuf {
        let mut exe = std::env::current_exe().expect("current_exe");
        exe.pop(); // drop the test binary's own filename
        if exe.file_name().is_some_and(|n| n == "deps") {
            exe.pop(); // deps/ -> debug/
        }
        let name = if cfg!(windows) { "yalda-gpui.exe" } else { "yalda-gpui" };
        exe.join(name)
    }

    /// `target/debug/yalda-gpui`, building it first via `cargo build --bin
    /// yalda-gpui` if it isn't already there — `cargo test --bin yalda-gpui`
    /// alone does not build the production binary artifact, only the test
    /// harness binary, so a bare `cargo test` run (outside `scripts/ci.sh`,
    /// which builds bins first) would otherwise find it missing.
    fn ensure_yalda_gpui_built() -> PathBuf {
        let bin = yalda_gpui_bin_path();
        if bin.is_file() {
            return bin;
        }
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let status = Command::new(env!("CARGO"))
            .current_dir(manifest_dir)
            .args(["build", "--bin", "yalda-gpui"])
            .status()
            .expect("failed to invoke cargo build --bin yalda-gpui");
        assert!(status.success(), "cargo build --bin yalda-gpui failed");
        assert!(bin.is_file(), "expected {} to exist after building", bin.display());
        bin
    }

    /// Path to the checked-in hook script, resolved from `CARGO_MANIFEST_DIR`
    /// so it works regardless of the test runner's cwd.
    fn hook_script_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/yalda-pre-merge-hook")
    }

    /// **REQUIRED DONE_WHEN**: "`--hash-diff` output matches `diff_model.rs`
    /// hashes for the same input" (spec C6). Runs the REAL binary's
    /// `--hash-diff` over a two-file, two-hunk fixture and compares its
    /// stdout, line for line, against `DiffModel::hunk_hashes()` computed
    /// in-process from `collect_raw_diff` + `parse_diff` over the SAME
    /// fixture — proving the CLI path and the tile's own derive path
    /// (`diff_ui.rs::refresh_diff`) compute IDENTICAL hashes, never a second
    /// implementation.
    #[test]
    fn hash_diff_subcommand_output_matches_diff_model_hashes() {
        let bin = ensure_yalda_gpui_built();
        let temp = build_fixture(); // two-file fixture (`a.txt`, `b.txt`), from the top of this module
        let worktree = temp.path().to_path_buf();

        // Expected: the SAME in-process path `refresh_diff` uses.
        let raw = futures::executor::block_on(collect_raw_diff(worktree.clone(), Some("main".into())))
            .expect("collect_raw_diff should succeed on the fixture");
        let model = crate::diff_model::parse_diff(&raw.diff_text, worktree.clone(), &raw.branch, &raw.base, &raw.merge_base);
        let expected: Vec<u64> = model.hunk_hashes();
        assert!(!expected.is_empty(), "fixture must have at least one hunk");

        // Actual: the real subprocess.
        let out = Command::new(&bin)
            .args(["--hash-diff", worktree.to_str().unwrap(), "main"])
            .output()
            .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));
        assert!(
            out.status.success(),
            "--hash-diff should exit 0, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let actual: Vec<u64> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.parse::<u64>().unwrap_or_else(|e| panic!("non-numeric hash line {l:?}: {e}")))
            .collect();

        assert_eq!(
            actual, expected,
            "--hash-diff output must match diff_model.rs's own hunk_hashes() exactly (spec C6)"
        );
    }

    /// `--hash-diff` on a non-repo path exits non-zero and touches nothing.
    #[test]
    fn hash_diff_subcommand_nonzero_exit_on_invalid_worktree() {
        let bin = ensure_yalda_gpui_built();
        let out = Command::new(&bin)
            .args(["--hash-diff", "/definitely/not/a/repo/xyz"])
            .output()
            .expect("run --hash-diff");
        assert!(!out.status.success());
    }

    /// Build a two-worktree fixture (like `build_merge_fixture`) but leave
    /// `primary` mid-merge (`git merge --no-commit --no-ff feature`) so
    /// `MERGE_HEAD` is set — the exact state git puts a repo in right before
    /// firing `pre-merge-commit`, which is what the hook script keys off.
    /// The merge here is content-clean (primary hasn't diverged from the
    /// fixture's common ancestor), so `--no-commit` always succeeds.
    fn build_hook_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (temp, feature) = build_merge_fixture();
        let primary = temp.path().join("primary");
        let status = Command::new("git")
            .arg("-C")
            .arg(&primary)
            .args(["merge", "--no-commit", "--no-ff", "feature"])
            .status()
            .expect("git merge --no-commit");
        assert!(status.success(), "expected a clean (non-conflicting) merge --no-commit");
        assert!(primary.join(".git").join("MERGE_HEAD").exists());
        (temp, primary, feature)
    }

    /// **REQUIRED DONE_WHEN**: "unreviewed-branch merge attempt exits
    /// non-zero" — with NO `ReviewState` file present at all (nothing ever
    /// reviewed), the hook, run from `primary` with `MERGE_HEAD` set, must
    /// refuse.
    #[test]
    fn pre_merge_hook_refuses_unreviewed_merge() {
        let bin = ensure_yalda_gpui_built();
        let (_temp, primary, _feature) = build_hook_fixture();

        let out = Command::new("sh")
            .arg(hook_script_path())
            .current_dir(&primary)
            .env("YALDA_GPUI_BIN", &bin)
            .output()
            .expect("run hook script");
        assert!(
            !out.status.success(),
            "hook must refuse an unreviewed merge; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unreviewed"),
            "refusal message should mention 'unreviewed', got: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The other half of the same DONE_WHEN: once every current hunk hash is
    /// written into the branch's `ReviewState` file (the exact file/shape
    /// `save_review_state`, `review_state.rs`, writes) AND the feature
    /// worktree is clean, the hook ALLOWS (exit 0).
    #[test]
    fn pre_merge_hook_allows_fully_reviewed_clean_merge() {
        let bin = ensure_yalda_gpui_built();
        let (_temp, primary, feature) = build_hook_fixture();

        let hash_out = Command::new(&bin)
            .args(["--hash-diff", feature.to_str().unwrap()])
            .output()
            .expect("run --hash-diff");
        assert!(hash_out.status.success());
        let hashes: Vec<String> = String::from_utf8_lossy(&hash_out.stdout)
            .lines()
            .map(|l| l.to_string())
            .collect();
        assert!(!hashes.is_empty(), "feature fixture must have at least one hunk");

        let common = resolve_git_common_dir(&feature).expect("common dir");
        let review_dir = common.join("yalda-review");
        std::fs::create_dir_all(&review_dir).unwrap();
        let json = format!("{{\n  \"reviewed_hashes\": [\n    {}\n  ]\n}}\n", hashes.join(",\n    "));
        std::fs::write(review_dir.join("feature.json"), json).unwrap();

        let out = Command::new("sh")
            .arg(hook_script_path())
            .current_dir(&primary)
            .env("YALDA_GPUI_BIN", &bin)
            .output()
            .expect("run hook script");
        assert!(
            out.status.success(),
            "hook must allow a fully-reviewed, clean merge; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// **REQUIRED DONE_WHEN**: "missing-binary fails closed (exit non-zero)"
    /// — with `YALDA_GPUI_BIN` unset and the script's own baked-in
    /// placeholder left un-substituted (this is the checked-in file, not an
    /// installed copy), the binary path resolves to a nonexistent path, so
    /// the hook must refuse rather than silently allow.
    #[test]
    fn pre_merge_hook_fails_closed_when_binary_missing() {
        let (_temp, primary, _feature) = build_hook_fixture();

        let out = Command::new("sh")
            .arg(hook_script_path())
            .current_dir(&primary)
            .env_remove("YALDA_GPUI_BIN")
            .output()
            .expect("run hook script");
        assert!(
            !out.status.success(),
            "missing binary must fail closed, not silently allow"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("failing closed"),
            "expected an explicit fail-closed message, got: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A completely ordinary (non-merge) commit — no `MERGE_HEAD` at all —
    /// must be a silent allow: the hook only ever gates an in-progress
    /// merge, so `pre-commit`-fragment invocations on every other commit
    /// must not refuse.
    #[test]
    fn pre_merge_hook_allows_when_no_merge_in_progress() {
        let (_temp, feature) = build_merge_fixture();
        let out = Command::new("sh")
            .arg(hook_script_path())
            .current_dir(&feature)
            .env_remove("YALDA_GPUI_BIN")
            .output()
            .expect("run hook script");
        assert!(
            out.status.success(),
            "no MERGE_HEAD must allow unconditionally, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The installer's `git config merge.ff false` is exactly the config the
    /// hook relies on to fire on a merge that WOULD otherwise fast-forward
    /// (spec B7: "`pre-merge-commit` does not fire on fast-forward merges").
    /// This is the Rust-side half of that DONE_WHEN bullet (the shell script
    /// itself has no config-setting responsibility — only the installer
    /// does, already covered by
    /// `install_merge_gate_hook_writes_hooks_bakes_path_and_sets_merge_ff_false`
    /// above); restated here for discoverability next to the other hook
    /// DONE_WHEN coverage.
    #[test]
    fn installer_merge_ff_false_prevents_fast_forward_merge_commit() {
        let (temp, _feature) = build_merge_fixture();
        let primary = temp.path().join("primary");
        install_merge_gate_hook(&primary, &PathBuf::from("/bin/true")).expect("install");

        // A fast-forward-ELIGIBLE merge (primary hasn't diverged from
        // feature) must still produce a MERGE COMMIT, not a fast-forward,
        // once `merge.ff false` is set — proving `pre-merge-commit` WILL be
        // invoked for this merge.
        let status = Command::new("git")
            .arg("-C")
            .arg(&primary)
            .args(["merge", "feature"])
            .status()
            .expect("git merge");
        assert!(status.success());
        let log = Command::new("git")
            .arg("-C")
            .arg(&primary)
            .args(["log", "-1", "--format=%P"])
            .output()
            .expect("git log");
        let parents = String::from_utf8_lossy(&log.stdout);
        assert_eq!(
            parents.split_whitespace().count(),
            2,
            "merge.ff=false must force a real 2-parent merge commit (not a fast-forward): {parents:?}"
        );
    }
}
