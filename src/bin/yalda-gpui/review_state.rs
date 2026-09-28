//! Review persistence for the Diff Review tile (spec rev 2, ADR-0040): one
//! JSON per branch at `<primary-checkout-root>/.yaldabaoth/reviews/<branch>.json`
//! holding file-level Viewed marks + stored comments. Pure ops on [`Review`]
//! plus light file I/O (load/save), all safe off the render thread; the
//! review root takes a `*_PATH_OVERRIDE`-style seam under `cfg(test)`.
//! See docs/specs/spec-diff-review.md § Data Model / C5.
#![allow(dead_code)]

use super::*;

use std::collections::BTreeMap;
use std::path::Path;

/// Branch names can contain `/` (e.g. `feature/foo`); turn that into a flat
/// filename component so `save` never tries to create a nested directory
/// structure it didn't ask for. `/` becomes `__` (spec rev 2 § Data Model;
/// this is a filename, not a git ref, so no round-trip requirement exists — it
/// only needs to be stable).
fn sanitize_branch(branch: &str) -> String {
    branch.replace('/', "__")
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

// ═══════════════════════════════════════════════════════════════════════════
// Rev-2 review store (spec-diff-review.md rev 2 § Data Model / Interfaces,
// UXI-Diff-14..17, ADR-0040). Cog graph 8g7 node `review-store`.
//
// One JSON file per branch at
// `<primary-checkout-root>/.yaldabaoth/reviews/<sanitized-branch>.json`
// holding file-level Viewed marks (`path → file_hash`) and stored comments.
// Pure ops on `Review` + light file I/O (load/save); everything here is safe to
// call off the render thread and never touches gpui.
// Wired into the Diff tile's derive + Viewed toggle (`diff_ui.rs`).
// ═══════════════════════════════════════════════════════════════════════════

/// Current on-disk schema version.
pub const REVIEW_VERSION: u32 = 2;

/// Which side of the diff a comment anchors to (spec B5): `New` = context +
/// added lines (new-file line numbers), `Old` = context + removed lines.
/// Serialized lowercase (`"new"` / `"old"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentSide {
    #[default]
    New,
    Old,
}

/// One delivery of a comment to a session (spec B6).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SentEntry {
    pub session: String,
    pub label: String,
    /// RFC3339 UTC.
    pub at: String,
}

/// A stored review comment (spec § Data Model).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReviewComment {
    /// `"c<N>"`, allocated from `Review::next_comment`; never reused.
    pub id: String,
    /// Repo-relative file path.
    pub path: PathBuf,
    pub side: CommentSide,
    /// Inclusive `[first, last]` line numbers on `side`.
    pub lines: [usize; 2],
    /// The anchored lines' text, `\n`-joined — used to relocate / detect
    /// outdated.
    pub snippet: String,
    pub body: String,
    /// RFC3339 UTC.
    pub created: String,
    pub sent: Vec<SentEntry>,
    pub outdated: bool,
}

/// The persisted review record for one branch (spec § Data Model).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Review {
    pub version: u32,
    pub branch: String,
    pub base: String,
    pub worktree: PathBuf,
    /// Repo-relative path → the `file_hash` it was viewed at.
    pub viewed: BTreeMap<PathBuf, u64>,
    /// Next comment number to allocate (`c<next_comment>`).
    pub next_comment: u64,
    pub comments: Vec<ReviewComment>,
    pub last_sent_session: Option<String>,
}

impl Default for Review {
    fn default() -> Self {
        Review {
            version: REVIEW_VERSION,
            branch: String::new(),
            base: String::new(),
            worktree: PathBuf::new(),
            viewed: BTreeMap::new(),
            next_comment: 1,
            comments: Vec::new(),
            last_sent_session: None,
        }
    }
}

/// Format a timestamp as RFC3339 UTC with a `Z` suffix, second precision
/// (`2026-09-27T14:03:00Z`).
pub fn review_timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Review {
    /// A fresh, empty review for `branch` in `worktree` against `base`.
    pub fn new(branch: &str, base: &str, worktree: &Path) -> Self {
        Review {
            branch: branch.to_string(),
            base: base.to_string(),
            worktree: worktree.to_path_buf(),
            ..Review::default()
        }
    }

    /// `true` iff `file` is marked viewed at its CURRENT `file_hash` (spec B4 —
    /// a content change yields a new hash, so Viewed clears itself).
    pub fn is_viewed(&self, file: &FileDiff) -> bool {
        self.viewed.get(&file.path) == Some(&file.file_hash)
    }

    /// Mark / unmark `file` viewed at its current `file_hash`.
    pub fn set_viewed(&mut self, file: &FileDiff, viewed: bool) {
        if viewed {
            self.viewed.insert(file.path.clone(), file.file_hash);
        } else {
            self.viewed.remove(&file.path);
        }
    }

    /// Flip Viewed for `file`; returns the new state.
    pub fn toggle_viewed(&mut self, file: &FileDiff) -> bool {
        let now = !self.is_viewed(file);
        self.set_viewed(file, now);
        now
    }

    /// `(viewed, total)` files of `model` (spec B2 `N/M files viewed`).
    pub fn viewed_count(&self, model: &DiffModel) -> (usize, usize) {
        let viewed = model.files.iter().filter(|f| self.is_viewed(f)).count();
        (viewed, model.files.len())
    }

    /// Drop `viewed` entries whose hash no longer matches a file in `model`
    /// (spec: "viewed entries whose file_hash no longer matches are dropped on
    /// write"). Comments are never touched.
    pub fn prune_viewed(&mut self, model: &DiffModel) {
        let live: HashMap<&Path, u64> = model
            .files
            .iter()
            .map(|f| (f.path.as_path(), f.file_hash))
            .collect();
        self.viewed
            .retain(|path, hash| live.get(path.as_path()) == Some(hash));
    }

    /// Add an unsent comment; returns its new id (`c<N>`). Ids are monotonic
    /// and never reused, even after deletes or a hand-edited `next_comment`.
    pub fn add_comment(
        &mut self,
        path: PathBuf,
        side: CommentSide,
        lines: [usize; 2],
        snippet: String,
        body: String,
        now: chrono::DateTime<chrono::Utc>,
    ) -> String {
        let max_existing = self
            .comments
            .iter()
            .filter_map(|c| c.id.strip_prefix('c')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let n = self.next_comment.max(max_existing + 1).max(1);
        self.next_comment = n + 1;
        let id = format!("c{n}");
        self.comments.push(ReviewComment {
            id: id.clone(),
            path,
            side,
            lines,
            snippet,
            body,
            created: review_timestamp(now),
            sent: Vec::new(),
            outdated: false,
        });
        id
    }

    /// Look up a comment by id.
    pub fn comment(&self, id: &str) -> Option<&ReviewComment> {
        self.comments.iter().find(|c| c.id == id)
    }

    /// Replace a comment's body; `false` if no such id.
    pub fn edit_comment(&mut self, id: &str, body: String) -> bool {
        match self.comments.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.body = body;
                true
            }
            None => false,
        }
    }

    /// Delete a comment; `false` if no such id. Its id is never reallocated.
    pub fn delete_comment(&mut self, id: &str) -> bool {
        let before = self.comments.len();
        self.comments.retain(|c| c.id != id);
        self.comments.len() != before
    }

    /// Recompute every comment's `outdated` flag against `model` (spec B5): a
    /// comment is outdated iff its file is absent from the model OR its
    /// `snippet` no longer appears as a contiguous run of that side's lines in
    /// the file's hunks (New = Context+Added, Old = Context+Removed; trailing
    /// whitespace trimmed per line). Returns `true` if any flag changed.
    pub fn recompute_outdated(&mut self, model: &DiffModel) -> bool {
        self.recompute_outdated_with(model, &mut |_| None)
    }

    /// [`recompute_outdated`](Self::recompute_outdated) aware of revealed
    /// context (spec B2a): a NEW-side comment whose snippet is not in the
    /// file's hunks is still live when it appears in the file's full new-side
    /// text (`new_text(path)`, `None` = unknown ⇒ outdated) — a comment on a
    /// line revealed by expanding context stays anchored. `new_text` is only
    /// asked for files still in the diff whose comment missed the hunks.
    pub fn recompute_outdated_with(
        &mut self,
        model: &DiffModel,
        new_text: &mut dyn FnMut(&Path) -> Option<std::sync::Arc<str>>,
    ) -> bool {
        let mut changed = false;
        for c in &mut self.comments {
            let outdated = match model.files.iter().find(|f| f.path == c.path) {
                None => true,
                Some(file) if snippet_present(file, c.side, &c.snippet) => false,
                Some(_) if c.side == CommentSide::New => {
                    !new_text(&c.path).is_some_and(|t| snippet_in_text(&t, &c.snippet))
                }
                Some(_) => true,
            };
            if c.outdated != outdated {
                c.outdated = outdated;
                changed = true;
            }
        }
        changed
    }

    /// Ids of comments never sent, in stored order (spec B6).
    pub fn unsent_ids(&self) -> Vec<String> {
        self.comments
            .iter()
            .filter(|c| c.sent.is_empty())
            .map(|c| c.id.clone())
            .collect()
    }

    /// Ids of every comment (for `S` = send all).
    pub fn all_comment_ids(&self) -> Vec<String> {
        self.comments.iter().map(|c| c.id.clone()).collect()
    }

    /// Record a successful delivery of `ids` to `session` (spec B6): each
    /// comment gains a `SentEntry`, and `last_sent_session` is updated.
    pub fn record_sent(
        &mut self,
        ids: &[String],
        session: &str,
        label: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        let at = review_timestamp(at);
        for c in self.comments.iter_mut().filter(|c| ids.contains(&c.id)) {
            c.sent.push(SentEntry {
                session: session.to_string(),
                label: label.to_string(),
                at: at.clone(),
            });
        }
        self.last_sent_session = Some(session.to_string());
    }
}

/// The side's line sequence for `file` (all hunks, in order).
fn side_lines(file: &FileDiff, side: CommentSide) -> Vec<&str> {
    file.hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter_map(|l| match (l, side) {
            (DiffLine::Context(t), _) => Some(t.as_str()),
            (DiffLine::Added(t), CommentSide::New) => Some(t.as_str()),
            (DiffLine::Removed(t), CommentSide::Old) => Some(t.as_str()),
            _ => None,
        })
        .map(str::trim_end)
        .collect()
}

/// `true` iff `snippet` appears as a contiguous run of `side`'s lines in
/// `file`. An empty snippet trivially matches.
fn snippet_present(file: &FileDiff, side: CommentSide, snippet: &str) -> bool {
    run_present(&side_lines(file, side), snippet)
}

/// `true` iff `snippet` appears as a contiguous run of `text`'s lines
/// (trailing whitespace ignored per line, like [`snippet_present`]).
pub fn snippet_in_text(text: &str, snippet: &str) -> bool {
    let hay: Vec<&str> = text.lines().map(str::trim_end).collect();
    run_present(&hay, snippet)
}

fn run_present(hay: &[&str], snippet: &str) -> bool {
    let needle: Vec<&str> = snippet.split('\n').map(str::trim_end).collect();
    let needle: &[&str] = match needle.as_slice() {
        // A single trailing `\n` in the snippet is not a line of its own.
        [head @ .., ""] if !head.is_empty() => head,
        n => n,
    };
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Build the one-message send prompt (spec § Interfaces, exact shape).
pub fn build_send_prompt(review: &Review, review_path: &Path, ids: &[String]) -> String {
    format!(
        "Review comments for branch `{}` (worktree {}).\n\
         Read {} and address comments {}.\n\
         Each comment has `path`, `lines` (may have drifted since — locate by `snippet`)\n\
         and `body`. Do not edit the review file.",
        review.branch,
        review.worktree.display(),
        review_path.display(),
        ids.join(", "),
    )
}

// ── Paths ──────────────────────────────────────────────────────────────────

// Test-only seam: replaces the primary checkout root so tests never touch a
// real repo (spec C5). Thread-local so parallel tests don't collide. When
// set, `save_review` also skips the `info/exclude` write.
#[cfg(test)]
thread_local! {
    static REVIEW_ROOT_PATH_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_review_root_path_override<R>(root: PathBuf, f: impl FnOnce() -> R) -> R {
    REVIEW_ROOT_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(root));
    let r = f();
    REVIEW_ROOT_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

fn review_root_override() -> Option<PathBuf> {
    #[cfg(test)]
    {
        if let Some(over) = REVIEW_ROOT_PATH_OVERRIDE.with(|c| c.borrow().clone()) {
            return Some(over);
        }
    }
    None
}

/// The primary checkout root for `worktree`: the parent of the absolute
/// `git rev-parse --git-common-dir` (the common dir itself if it has no
/// parent). Shells out to git — call off the render path. Under `cfg(test)`
/// the override root wins (no subprocess).
pub fn primary_checkout_root(worktree: &Path) -> Option<PathBuf> {
    if let Some(over) = review_root_override() {
        return Some(over);
    }
    let common = resolve_git_common_dir(worktree)?;
    Some(common.parent().map(Path::to_path_buf).unwrap_or(common))
}

/// `<primary_root>/.yaldabaoth/reviews/<sanitized-key>.json`. Pure.
pub fn review_file_path(primary_root: &Path, branch_key: &str) -> PathBuf {
    primary_root
        .join(".yaldabaoth")
        .join("reviews")
        .join(format!("{}.json", sanitize_branch(branch_key)))
}

/// The review key for a checkout: the branch name, or
/// `detached-<worktree dir name>` for a detached HEAD (`None`, empty, or
/// `"HEAD"`).
pub fn review_key_for(branch: Option<&str>, worktree: &Path) -> String {
    match branch.map(str::trim) {
        Some(b) if !b.is_empty() && b != "HEAD" => b.to_string(),
        _ => {
            let dir = worktree
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "worktree".to_string());
            format!("detached-{dir}")
        }
    }
}

/// Convenience: resolve the review file path for `worktree` on `branch`
/// (subprocess via [`primary_checkout_root`] — off the render path).
pub fn review_path_for(worktree: &Path, branch: Option<&str>) -> Option<PathBuf> {
    let root = primary_checkout_root(worktree)?;
    Some(review_file_path(&root, &review_key_for(branch, worktree)))
}

// ── I/O ────────────────────────────────────────────────────────────────────

/// Load a review. Missing file ⇒ `Review::default()` silently; unreadable or
/// corrupt ⇒ default with a logged warning. Never panics.
pub fn load_review(path: &Path) -> Review {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Review::default(),
        Err(e) => {
            eprintln!("[yalda-gpui] review: cannot read {}: {e}", path.display());
            return Review::default();
        }
    };
    match serde_json::from_slice::<Review>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[yalda-gpui] review: corrupt {} ({e}); starting empty",
                path.display()
            );
            Review::default()
        }
    }
}

/// Persist `review` atomically (tmp file in the same dir + rename), creating
/// directories as needed. When `model` is `Some`, stale `viewed` entries are
/// pruned first (in memory too). Comments are never pruned. On the first write
/// of a real (non-overridden) review file, `/.yaldabaoth/` is appended to the
/// repo's `info/exclude` (spec UXI-Diff-17) — resolving the common dir shells
/// out to git, so call off the render path.
pub fn save_review(path: &Path, review: &mut Review, model: Option<&DiffModel>) -> std::io::Result<()> {
    let exclude_common = if review_root_override().is_none() && !path.exists() {
        // path = <root>/.yaldabaoth/reviews/<key>.json
        path.parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(resolve_git_common_dir)
    } else {
        None
    };
    save_review_inner(path, review, model, exclude_common.as_deref())
}

/// Serializes every review write in the process: two quick toggles spawn two
/// background saves, and without this they could race on the tmp file.
static REVIEW_SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// [`save_review`] under the process-wide save lock, skipped when a NEWER
/// snapshot (higher `save_gen`) of the same tile's review was already written —
/// background saves may run out of order, and a stale snapshot must never
/// overwrite a newer one. `written` is the tile's shared high-water mark.
pub fn save_review_latest(
    path: &Path,
    review: &mut Review,
    model: Option<&DiffModel>,
    save_gen: u64,
    written: &std::sync::atomic::AtomicU64,
) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    let _guard = REVIEW_SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if written.load(Ordering::SeqCst) > save_gen {
        return Ok(());
    }
    save_review(path, review, model)?;
    written.store(save_gen, Ordering::SeqCst);
    Ok(())
}

/// `save_review` with the `info/exclude` target made explicit (the testable
/// core). `exclude_common = Some(dir)` ⇒ ensure `/.yaldabaoth/` in
/// `<dir>/info/exclude` after writing.
fn save_review_inner(
    path: &Path,
    review: &mut Review,
    model: Option<&DiffModel>,
    exclude_common: Option<&Path>,
) -> std::io::Result<()> {
    if let Some(model) = model {
        review.prune_viewed(model);
    }
    review.version = REVIEW_VERSION;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("review path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&*review).map_err(std::io::Error::other)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "review.json".to_string());
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, json)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(common) = exclude_common {
        if let Err(e) = ensure_info_exclude(common) {
            eprintln!("[yalda-gpui] review: cannot update info/exclude: {e}");
        }
    }
    Ok(())
}

/// The `info/exclude` line that keeps review files out of `git status`.
pub const REVIEW_EXCLUDE_LINE: &str = "/.yaldabaoth/";

/// Idempotently append `/.yaldabaoth/` to `<git_common_dir>/info/exclude`,
/// creating the dir/file if missing.
pub fn ensure_info_exclude(git_common_dir: &Path) -> std::io::Result<()> {
    let info = git_common_dir.join("info");
    std::fs::create_dir_all(&info)?;
    let exclude = info.join("exclude");
    let existing = match std::fs::read_to_string(&exclude) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if existing.lines().any(|l| l.trim() == REVIEW_EXCLUDE_LINE) {
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(REVIEW_EXCLUDE_LINE);
    out.push('\n');
    std::fs::write(&exclude, out)
}

/// Rev-2 review store tests (graph 8g7 node `review-store`). Every test that
/// writes runs under a tempdir + `with_review_root_path_override` (spec C5).
#[cfg(test)]
mod review_v2_tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 9, 27, h, 3, 0).unwrap()
    }

    fn diff_model(raw: &str) -> DiffModel {
        parse_diff(raw, PathBuf::from("/tmp/wt"), "feature/x", "main", "deadbeef")
    }

    /// One modified file `src/foo.rs`: context `fn foo() {`, removed
    /// `    old();`, added `new_body` lines, context `}`.
    fn foo_diff(header: &str, new_body: &[&str]) -> String {
        let mut s = format!(
            "diff --git a/src/foo.rs b/src/foo.rs\nindex 1..2 100644\n--- a/src/foo.rs\n+++ b/src/foo.rs\n{header}\n fn foo() {{\n-    old();\n"
        );
        for l in new_body {
            s.push_str(&format!("+{l}\n"));
        }
        s.push_str(" }\n");
        s
    }

    fn bar_diff() -> String {
        "diff --git a/src/bar.rs b/src/bar.rs\nindex 3..4 100644\n--- a/src/bar.rs\n+++ b/src/bar.rs\n@@ -1,1 +1,2 @@\n x\n+y\n".to_string()
    }

    /// Resolve the review path the way production does (override root stands
    /// in for the primary checkout root).
    fn path_under(root: &Path, branch: &str) -> PathBuf {
        with_review_root_path_override(root.to_path_buf(), || {
            review_path_for(Path::new("/nonexistent/wt"), Some(branch)).expect("override root")
        })
    }

    #[test]
    fn path_layout_and_keys() {
        let root = Path::new("/r");
        assert_eq!(
            review_file_path(root, "feature/x"),
            PathBuf::from("/r/.yaldabaoth/reviews/feature__x.json")
        );
        let wt = Path::new("/r/.claude/worktrees/my-wt");
        assert_eq!(review_key_for(Some("main"), wt), "main");
        assert_eq!(review_key_for(None, wt), "detached-my-wt");
        assert_eq!(review_key_for(Some("HEAD"), wt), "detached-my-wt");
        assert_eq!(review_key_for(Some(""), wt), "detached-my-wt");
    }

    #[test]
    fn round_trip_save_load_and_viewed_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_under(dir.path(), "feature/x");
        let model = diff_model(&(foo_diff("@@ -1,3 +1,3 @@", &["    new();"]) + &bar_diff()));
        let mut r = Review::new("feature/x", "main", Path::new("/tmp/wt"));
        assert!(r.toggle_viewed(&model.files[0]));
        let id = r.add_comment(
            "src/foo.rs".into(),
            CommentSide::New,
            [2, 2],
            "    new();".into(),
            "rename".into(),
            t(14),
        );
        with_review_root_path_override(dir.path().to_path_buf(), || {
            save_review(&path, &mut r, Some(&model)).unwrap();
        });
        let back = load_review(&path);
        assert_eq!(back, r);
        assert_eq!(back.version, REVIEW_VERSION);
        assert!(back.is_viewed(&model.files[0]));
        assert!(!back.is_viewed(&model.files[1]));
        assert_eq!(back.viewed_count(&model), (1, 2));
        assert_eq!(back.comment(&id).unwrap().created, "2026-09-27T14:03:00Z");
        // On-disk shape matches the spec schema.
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["comments"][0]["side"], "new");
        assert_eq!(v["comments"][0]["lines"], serde_json::json!([2, 2]));
        assert!(v["viewed"]["src/foo.rs"].is_u64());
    }

    /// Editing the file's diff content ⇒ new file_hash ⇒ not viewed; save with
    /// the new model prunes the stale entry. A position-only shift keeps it.
    #[test]
    fn content_change_clears_viewed_and_save_prunes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_under(dir.path(), "b");
        let v1 = diff_model(&foo_diff("@@ -1,3 +1,3 @@", &["    new();"]));
        let shifted = diff_model(&foo_diff("@@ -40,3 +42,3 @@", &["    new();"]));
        let v2 = diff_model(&foo_diff("@@ -1,3 +1,3 @@", &["    newer();"]));
        let mut r = Review::default();
        r.set_viewed(&v1.files[0], true);
        assert!(r.is_viewed(&shifted.files[0]), "position-only shift keeps Viewed");
        assert!(!r.is_viewed(&v2.files[0]), "content change clears Viewed");
        assert_eq!(r.viewed_count(&v2), (0, 1));
        with_review_root_path_override(dir.path().to_path_buf(), || {
            save_review(&path, &mut r, Some(&v2)).unwrap();
        });
        assert!(r.viewed.is_empty(), "stale entry pruned in memory");
        assert!(load_review(&path).viewed.is_empty(), "stale entry pruned on disk");
    }

    /// Comments are never pruned, even when their file vanished from the diff.
    #[test]
    fn save_never_prunes_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_under(dir.path(), "b");
        let mut r = Review::default();
        r.add_comment("gone.rs".into(), CommentSide::New, [1, 1], "x".into(), "b".into(), t(1));
        let empty = diff_model("");
        r.recompute_outdated(&empty);
        with_review_root_path_override(dir.path().to_path_buf(), || {
            save_review(&path, &mut r, Some(&empty)).unwrap();
        });
        let back = load_review(&path);
        assert_eq!(back.comments.len(), 1);
        assert!(back.comments[0].outdated);
    }

    #[test]
    fn comment_ids_are_monotonic_across_delete_and_edit_works() {
        let mut r = Review::default();
        let a = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "a".into(), t(1));
        let b = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "b".into(), t(1));
        assert_eq!((a.as_str(), b.as_str()), ("c1", "c2"));
        assert!(r.delete_comment(&b));
        assert!(!r.delete_comment(&b));
        let c = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "c".into(), t(1));
        assert_eq!(c, "c3", "deleted id never reused");
        // A hand-lowered next_comment still can't collide with an existing id.
        r.next_comment = 1;
        let d = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "d".into(), t(1));
        assert_eq!(d, "c4");
        assert!(r.edit_comment(&a, "edited".into()));
        assert_eq!(r.comment(&a).unwrap().body, "edited");
        assert!(!r.edit_comment("c99", "x".into()));
    }

    /// spec B2a: a new-side comment on a revealed context line (snippet not
    /// in any hunk) stays live while the snippet is in the file's full text,
    /// and is outdated when it is not (or the text is unknown).
    #[test]
    fn recompute_outdated_with_full_text_keeps_context_comments_live() {
        let m = diff_model(&foo_diff("@@ -1,3 +1,3 @@", &["    new();"]));
        let mut r = Review::default();
        let id = r.add_comment("src/foo.rs".into(), CommentSide::New, [40, 40], "far away".into(), "b".into(), t(1));
        let full: std::sync::Arc<str> = "fn foo() {\nfar away  \n}\n".into();
        r.recompute_outdated_with(&m, &mut |_| Some(full.clone()));
        assert!(!r.comment(&id).unwrap().outdated, "found in the full text");
        assert!(r.recompute_outdated_with(&m, &mut |_| Some("nope\n".into())));
        assert!(r.comment(&id).unwrap().outdated, "gone from the full text");
        r.recompute_outdated_with(&m, &mut |_| Some(full.clone()));
        assert!(!r.comment(&id).unwrap().outdated);
        assert!(r.recompute_outdated(&m), "no text ⇒ outdated");
    }

    /// New side: snippet present ⇒ not outdated; the added line changes ⇒
    /// outdated; restored ⇒ back to not outdated. Trailing whitespace ignored.
    #[test]
    fn recompute_outdated_new_side() {
        let present = diff_model(&foo_diff("@@ -1,3 +1,4 @@", &["    a();", "    b();  "]));
        let gone = diff_model(&foo_diff("@@ -1,3 +1,4 @@", &["    a();", "    c();"]));
        let mut r = Review::default();
        let id = r.add_comment(
            "src/foo.rs".into(),
            CommentSide::New,
            [2, 3],
            "    a();\n    b();\n".into(),
            "x".into(),
            t(1),
        );
        r.recompute_outdated(&present);
        assert!(!r.comment(&id).unwrap().outdated);
        assert!(r.recompute_outdated(&gone));
        assert!(r.comment(&id).unwrap().outdated);
        assert!(r.recompute_outdated(&present));
        assert!(!r.comment(&id).unwrap().outdated);
    }

    /// Old side matches Context+Removed only: a removed line is found on the
    /// old side, and an added line is NOT (so it's outdated on the old side).
    #[test]
    fn recompute_outdated_old_side() {
        let m = diff_model(&foo_diff("@@ -1,3 +1,3 @@", &["    new();"]));
        let mut r = Review::default();
        let old_ok = r.add_comment(
            "src/foo.rs".into(),
            CommentSide::Old,
            [1, 2],
            "fn foo() {\n    old();".into(),
            "x".into(),
            t(1),
        );
        let old_wrong = r.add_comment(
            "src/foo.rs".into(),
            CommentSide::Old,
            [2, 2],
            "    new();".into(),
            "x".into(),
            t(1),
        );
        let new_wrong = r.add_comment(
            "src/foo.rs".into(),
            CommentSide::New,
            [2, 2],
            "    old();".into(),
            "x".into(),
            t(1),
        );
        r.recompute_outdated(&m);
        assert!(!r.comment(&old_ok).unwrap().outdated);
        assert!(r.comment(&old_wrong).unwrap().outdated);
        assert!(r.comment(&new_wrong).unwrap().outdated);
    }

    #[test]
    fn unsent_and_record_sent() {
        let mut r = Review::default();
        let a = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "a".into(), t(1));
        let b = r.add_comment("f".into(), CommentSide::New, [1, 1], "".into(), "b".into(), t(1));
        assert_eq!(r.unsent_ids(), vec![a.clone(), b.clone()]);
        r.record_sent(&[a.clone()], "sid-1", "My session", t(15));
        assert_eq!(r.unsent_ids(), vec![b.clone()]);
        assert_eq!(r.last_sent_session.as_deref(), Some("sid-1"));
        assert_eq!(
            r.comment(&a).unwrap().sent,
            vec![SentEntry {
                session: "sid-1".into(),
                label: "My session".into(),
                at: "2026-09-27T15:03:00Z".into()
            }]
        );
        r.record_sent(&r.all_comment_ids(), "sid-2", "Other", t(16));
        assert_eq!(r.comment(&a).unwrap().sent.len(), 2);
        assert!(r.unsent_ids().is_empty());
        assert_eq!(r.last_sent_session.as_deref(), Some("sid-2"));
    }

    #[test]
    fn build_send_prompt_exact() {
        let r = Review::new("diff-rework", "main", Path::new("/abs/wt"));
        let p = build_send_prompt(
            &r,
            Path::new("/abs/repo/.yaldabaoth/reviews/diff-rework.json"),
            &["c3".into(), "c5".into(), "c7".into()],
        );
        assert_eq!(
            p,
            "Review comments for branch `diff-rework` (worktree /abs/wt).\n\
             Read /abs/repo/.yaldabaoth/reviews/diff-rework.json and address comments c3, c5, c7.\n\
             Each comment has `path`, `lines` (may have drifted since — locate by `snippet`)\n\
             and `body`. Do not edit the review file."
        );
    }

    #[test]
    fn corrupt_or_missing_json_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(load_review(&missing), Review::default());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, b"{ not json").unwrap();
        assert_eq!(load_review(&bad), Review::default());
        // Partial file: missing fields default (serde(default)).
        let partial = dir.path().join("partial.json");
        std::fs::write(&partial, br#"{"branch":"b","comments":[{"id":"c1","body":"hi"}]}"#).unwrap();
        let r = load_review(&partial);
        assert_eq!(r.branch, "b");
        assert_eq!(r.version, REVIEW_VERSION);
        assert_eq!(r.next_comment, 1);
        assert_eq!(r.comments[0].body, "hi");
        assert_eq!(r.comments[0].side, CommentSide::New);
    }

    /// Two saves through the real save core against a fake common dir ⇒
    /// exactly one `/.yaldabaoth/` line, pre-existing content preserved.
    #[test]
    fn info_exclude_is_idempotent_across_saves() {
        let dir = tempfile::tempdir().unwrap();
        let common = dir.path().join("fake.git");
        std::fs::create_dir_all(common.join("info")).unwrap();
        std::fs::write(common.join("info/exclude"), "# existing\n*.swp").unwrap();
        let path = path_under(dir.path(), "b");
        let mut r = Review::default();
        save_review_inner(&path, &mut r, None, Some(&common)).unwrap();
        save_review_inner(&path, &mut r, None, Some(&common)).unwrap();
        let text = std::fs::read_to_string(common.join("info/exclude")).unwrap();
        assert_eq!(text, "# existing\n*.swp\n/.yaldabaoth/\n");
        // Missing info dir/file is created.
        let fresh = dir.path().join("fresh.git");
        ensure_info_exclude(&fresh).unwrap();
        ensure_info_exclude(&fresh).unwrap();
        assert_eq!(
            std::fs::read_to_string(fresh.join("info/exclude")).unwrap(),
            "/.yaldabaoth/\n"
        );
    }

    /// `save_review_latest`: a stale snapshot (lower generation) that lands
    /// AFTER a newer one is skipped, so out-of-order background saves can
    /// never roll the file back.
    #[test]
    fn save_review_latest_skips_stale_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_under(dir.path(), "b");
        let written = std::sync::atomic::AtomicU64::new(0);
        let mut newer = Review::default();
        newer.branch = "newer".into();
        let mut older = Review::default();
        older.branch = "older".into();
        with_review_root_path_override(dir.path().to_path_buf(), || {
            save_review_latest(&path, &mut newer, None, 5, &written).unwrap();
            save_review_latest(&path, &mut older, None, 4, &written).unwrap();
        });
        assert_eq!(load_review(&path).branch, "newer");
        with_review_root_path_override(dir.path().to_path_buf(), || {
            save_review_latest(&path, &mut older, None, 6, &written).unwrap();
        });
        assert_eq!(load_review(&path).branch, "older");
    }

    /// Everything a save writes lands under the override root (incl. no
    /// leftover tmp file), and the override suppresses any info/exclude write.
    #[test]
    fn writes_stay_inside_override_root() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let path = path_under(&root, "feature/x");
        assert!(path.starts_with(&root));
        let mut r = Review::default();
        with_review_root_path_override(root.clone(), || {
            save_review(&path, &mut r, None).unwrap();
            save_review(&path, &mut r, None).unwrap();
        });
        let entries: Vec<_> = std::fs::read_dir(outer.path()).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "nothing created beside the root");
        let reviews: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(reviews, vec!["feature__x.json".to_string()], "no tmp leftovers");
        assert!(!root.join("info").exists() && !root.join(".git").exists());
    }
}
