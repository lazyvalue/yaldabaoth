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
    /// Raw `git worktree list` output (spec Interfaces).
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

/// One entry of `git worktree list --porcelain` (spec rev 2 B1 — the rows of
/// the unbound Diff tile's worktree picker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    /// Absolute worktree root.
    pub(crate) path: PathBuf,
    /// `HEAD` commit SHA (empty for a fresh repo with no commits).
    pub(crate) head: String,
    /// Checked-out branch with `refs/heads/` stripped; `None` when detached.
    pub(crate) branch: Option<String>,
    pub(crate) detached: bool,
    /// The first entry git lists — the primary checkout.
    pub(crate) is_primary: bool,
    /// `HEAD`'s commit subject + committer time (unix seconds) — the picker
    /// row's description line (UXI-Diff-10). `None` when unknown (fresh repo,
    /// or the batched `git log` failed — errors-as-values, the row just shows
    /// its path).
    pub(crate) head_commit: Option<HeadCommit>,
}

/// The last commit on a worktree's `HEAD`, for the picker description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeadCommit {
    pub(crate) subject: String,
    /// Committer time, unix seconds.
    pub(crate) time: i64,
}

impl WorktreeEntry {
    /// The picker's prominent label: the branch, else `detached @ <sha8>`.
    pub(crate) fn label(&self) -> String {
        match &self.branch {
            Some(b) => b.clone(),
            None => format!("detached @ {}", &self.head[..self.head.len().min(8)]),
        }
    }
}

/// Parse `git worktree list --porcelain` output. Pure (unit-tested without
/// git). Bare entries and entries whose path no longer exists (prunable /
/// deleted out from under git) are skipped; `is_primary` marks the FIRST
/// entry git reports (git always lists the primary checkout first).
pub(crate) fn parse_worktree_porcelain(text: &str) -> Vec<WorktreeEntry> {
    struct Raw {
        path: PathBuf,
        head: String,
        branch: Option<String>,
        detached: bool,
        bare: bool,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            raws.push(Raw {
                path: PathBuf::from(p),
                head: String::new(),
                branch: None,
                detached: false,
                bare: false,
            });
            continue;
        }
        let Some(cur) = raws.last_mut() else { continue };
        if let Some(h) = line.strip_prefix("HEAD ") {
            cur.head = h.to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            cur.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        } else if line == "detached" {
            cur.detached = true;
        } else if line == "bare" {
            cur.bare = true;
        }
    }
    raws.into_iter()
        .enumerate()
        .filter(|(_, r)| !r.bare && r.path.is_dir())
        .map(|(i, r)| WorktreeEntry {
            path: r.path,
            head: r.head,
            branch: r.branch,
            detached: r.detached,
            is_primary: i == 0,
            head_commit: None,
        })
        .collect()
}

/// Parse `git log --no-walk=unsorted --format=%H%x00%ct%x00%s <shas…>` output
/// into `sha → HeadCommit`. Pure (unit-tested without git); malformed lines
/// are skipped.
pub(crate) fn parse_head_commits(text: &str) -> std::collections::HashMap<String, HeadCommit> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\0');
            let sha = parts.next()?.trim();
            let time = parts.next()?.trim().parse::<i64>().ok()?;
            let subject = parts.next()?.to_string();
            (!sha.is_empty()).then(|| (sha.to_string(), HeadCommit { subject, time }))
        })
        .collect()
}

/// List every worktree of the repo containing `repo_dir` (spec rev 2 B1,
/// § Interfaces). Blocking — run on the background executor, never the paint
/// path (C2). A non-repo / missing dir is an `Err` value, never a panic.
pub(crate) fn list_worktrees(repo_dir: &Path) -> Result<Vec<WorktreeEntry>, GitDiffError> {
    if !repo_dir.is_dir() {
        return Err(GitDiffError::InvalidWorktree(repo_dir.to_path_buf()));
    }
    let out = run_git(repo_dir, &["worktree", "list", "--porcelain"])?;
    let mut entries = parse_worktree_porcelain(&out);
    attach_head_commits(repo_dir, &mut entries);
    Ok(entries)
}

/// Fill each entry's `head_commit` with ONE batched `git log --no-walk` over
/// every distinct `HEAD` (worktrees share one object store). Best-effort: a
/// failure leaves every `head_commit` `None` — the picker still lists rows.
fn attach_head_commits(repo_dir: &Path, entries: &mut [WorktreeEntry]) {
    let mut heads: Vec<&str> = entries
        .iter()
        .map(|e| e.head.as_str())
        .filter(|h| !h.is_empty())
        .collect();
    heads.sort_unstable();
    heads.dedup();
    if heads.is_empty() {
        return;
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x00%ct%x00%s"];
    args.extend(heads.iter().copied());
    let Ok(out) = run_git(repo_dir, &args) else {
        return;
    };
    let commits = parse_head_commits(&out);
    for e in entries.iter_mut() {
        e.head_commit = commits.get(&e.head).cloned();
    }
}

impl GitDiffError {
    /// `true` when git refused because `dir` is not inside any repository —
    /// the picker renders that as the plain "Not inside a git repository."
    /// state rather than an error (spec B1).
    pub(crate) fn is_not_a_repo(&self) -> bool {
        match self {
            GitDiffError::CommandFailed { stderr, .. } => {
                stderr.to_ascii_lowercase().contains("not a git repository")
            }
            _ => false,
        }
    }
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

    /// Spec rev 2 B1: a repo with a linked worktree lists both — primary
    /// first (flagged), branch names with `refs/heads/` stripped, real paths.
    #[test]
    fn list_worktrees_lists_primary_and_linked_worktree() {
        let temp = build_fixture();
        let primary = temp.path().to_path_buf();
        let linked_parent = tempfile::tempdir().expect("tempdir");
        let linked = linked_parent.path().join("wt-topic");
        git_ok(
            &primary,
            &["worktree", "add", "--quiet", "-b", "topic", linked.to_str().unwrap()],
        );

        let rows = list_worktrees(&primary).expect("list_worktrees on a real repo");
        assert_eq!(rows.len(), 2, "primary + linked worktree: {rows:?}");
        assert!(rows[0].is_primary && !rows[1].is_primary);
        assert_eq!(rows[0].branch.as_deref(), Some("feature"));
        assert_eq!(rows[1].branch.as_deref(), Some("topic"));
        assert_eq!(
            rows[1].path.canonicalize().unwrap(),
            linked.canonicalize().unwrap()
        );
        assert!(!rows[1].head.is_empty());
        // UXI-Diff-10 description: each row carries HEAD's subject + time,
        // from the ONE batched `git log --no-walk` (both share `feature`'s HEAD).
        for r in &rows {
            let c = r.head_commit.as_ref().expect("head commit attached");
            assert_eq!(c.subject, "committed change");
            assert!(c.time > 1_500_000_000, "plausible unix time: {}", c.time);
        }

        // Listing from INSIDE the linked worktree finds the same set.
        let from_linked = list_worktrees(&linked).expect("list from linked");
        assert_eq!(from_linked.len(), 2);
        assert!(from_linked[0].is_primary);
    }

    /// `parse_head_commits`: NUL-separated `sha, ct, subject` (a subject may
    /// itself contain separators like `·` or `:`); malformed lines skipped.
    #[test]
    fn parse_head_commits_reads_batched_log() {
        let text = "aaa\x001700000000\x00feat(diff): a · b: c\nbad line\nbbb\x00notanumber\x00x\nccc\x001700000100\x00\n";
        let m = parse_head_commits(text);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(
            m["aaa"],
            HeadCommit { subject: "feat(diff): a · b: c".to_string(), time: 1_700_000_000 }
        );
        assert_eq!(m["ccc"].subject, "");
    }

    /// A worktree whose directory was deleted (prunable) is skipped, not
    /// listed as a dead row.
    #[test]
    fn list_worktrees_skips_deleted_worktree() {
        let temp = build_fixture();
        let primary = temp.path().to_path_buf();
        let linked_parent = tempfile::tempdir().expect("tempdir");
        let linked = linked_parent.path().join("wt-gone");
        git_ok(
            &primary,
            &["worktree", "add", "--quiet", "-b", "gone", linked.to_str().unwrap()],
        );
        std::fs::remove_dir_all(&linked).unwrap();
        let rows = list_worktrees(&primary).expect("list");
        assert_eq!(rows.len(), 1, "deleted worktree must be skipped: {rows:?}");
        assert!(rows[0].is_primary);
    }

    /// Non-repo dir ⇒ an `Err` value flagged not-a-repo; missing dir ⇒
    /// `InvalidWorktree`. Never a panic.
    #[test]
    fn list_worktrees_non_repo_is_error_value() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = list_worktrees(temp.path()).expect_err("non-repo must error");
        assert!(err.is_not_a_repo(), "expected not-a-repo, got {err:?}");
        let bogus = PathBuf::from("/nonexistent/definitely-not-a-repo-wt-list");
        assert_eq!(
            list_worktrees(&bogus),
            Err(GitDiffError::InvalidWorktree(bogus))
        );
    }

    #[test]
    fn parse_worktree_porcelain_handles_detached_and_bare() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let text = format!(
            "worktree {}\nbare\n\nworktree {}\nHEAD 0123456789abcdef\ndetached\n\nworktree {}\nHEAD fff\nbranch refs/heads/x/y\n\n",
            dir.path().display(),
            a.display(),
            b.display()
        );
        let rows = parse_worktree_porcelain(&text);
        assert_eq!(rows.len(), 2, "bare entry skipped: {rows:?}");
        assert!(rows[0].detached && rows[0].branch.is_none());
        assert_eq!(rows[0].label(), "detached @ 01234567");
        assert_eq!(rows[1].branch.as_deref(), Some("x/y"));
        assert!(!rows[0].is_primary && !rows[1].is_primary, "primary was the bare entry");
    }
}
