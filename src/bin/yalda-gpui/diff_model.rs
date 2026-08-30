//! Pure unified-diff parser for `App::Diff` (see `docs/specs/spec-diff-review.md`,
//! § Data Model / § Interfaces).
//!
//! This module parses the text of `git diff --no-color <merge-base>` (plus a
//! handful of caller-supplied metadata strings — worktree path, branch, base
//! ref, merge-base SHA) into a [`DiffModel`]. It is the pure, unit-testable
//! core described by spec constraint C1: **no filesystem access and no
//! subprocess spawning happens in this module.** The caller (a later node,
//! `app-diff-tile`) is responsible for actually invoking `git` off the paint
//! path and handing the raw stdout text to [`parse_diff`].
//!
//! ## `hunk_hash`
//!
//! Per spec § Data Model: "`hunk_hash` excludes `@@` positions (stable across
//! unrelated edits) but is salted with the hunk's per-file occurrence index,
//! so two identical hunks in one file review independently."
//!
//! Concretely, `hunk_hash` is computed from exactly three inputs, hashed via
//! [`std::hash::Hash`] / [`std::collections::hash_map::DefaultHasher`]:
//!
//! 1. the file's repo-relative path,
//! 2. the hunk's 0-based occurrence index within that file's `Vec<Hunk>`
//!    (i.e. "this is the 2nd hunk in this file"), and
//! 3. the hunk's content lines — the parsed `Vec<DiffLine>`, which hashes
//!    both the `Context`/`Added`/`Removed` discriminant *and* the line text
//!    (the `+`/`-`/` ` prefix is encoded via the enum variant rather than
//!    kept literally in the string).
//!
//! The `@@ -a,b +c,d @@` header line itself is never fed to the hasher, so a
//! change that only shifts line numbers (because an earlier, unrelated hunk
//! in the same file grew or shrank) leaves the hash unchanged. Two hunks with
//! byte-for-byte identical content lines still hash differently as long as
//! they land at different occurrence indices in the file (input #2 differs).

#![allow(dead_code)]

use super::*;

use std::hash::{Hash, Hasher};
use std::path::Path;

/// The parsed, cumulative diff for one worktree: `merge_base(base, HEAD) →
/// working tree` (spec Overview).
#[derive(Debug, Clone, PartialEq)]
pub struct DiffModel {
    pub worktree: PathBuf,
    pub branch: String,
    pub base: String,
    pub merge_base: String,
    pub dirty: bool,
    pub files: Vec<FileDiff>,
}

/// What happened to a file between `merge_base` and the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    /// `from` is the repo-relative path the file was renamed *from*; the
    /// `FileDiff::path` this status is attached to is the *new* path.
    Renamed { from: PathBuf },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    /// Repo-relative path (the new path, for a rename).
    pub path: PathBuf,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
    /// Total added lines across all hunks in this file (convenience — spec
    /// B2 file-list add/remove counts).
    pub added: usize,
    /// Total removed lines across all hunks in this file.
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The raw `@@ -a,b +c,d @@ ...` header line, kept verbatim for display.
    /// Never fed into `hunk_hash` (see module docs).
    pub header: String,
    pub lines: Vec<DiffLine>,
    pub hunk_hash: u64,
    /// Always `false` out of this parser — joining with persisted
    /// `ReviewState` is a later node's job (spec § Data Model: "joined from
    /// ReviewState at derive time").
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

/// Parse `git diff --no-color` output (`raw`) into a [`DiffModel`].
///
/// Pure: takes only strings/a path value in, does no I/O. `worktree`,
/// `branch`, `base`, and `merge_base` are metadata the caller already knows
/// (from separate, non-parser git calls) and are copied through unchanged.
///
/// `dirty` is derived from the diff text itself: this parser only ever sees
/// the `merge_base(base, HEAD) → working tree` diff (spec Overview), so any
/// parsed file change means the working tree differs from `base` — there is
/// no narrower "uncommitted-only" signal available from this input alone.
pub fn parse_diff(
    raw: &str,
    worktree: PathBuf,
    branch: &str,
    base: &str,
    merge_base: &str,
) -> DiffModel {
    let files = parse_files(raw);
    let dirty = !files.is_empty();
    DiffModel {
        worktree,
        branch: branch.to_string(),
        base: base.to_string(),
        merge_base: merge_base.to_string(),
        dirty,
        files,
    }
}

/// Compute `hunk_hash` per the scheme documented at the top of this module.
fn compute_hunk_hash(path: &Path, occurrence_index: usize, lines: &[DiffLine]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    occurrence_index.hash(&mut hasher);
    lines.hash(&mut hasher);
    hasher.finish()
}

/// Strip a git diff `--- `/`+++ ` path operand down to a repo-relative
/// `PathBuf`: drops the `a/`/`b/` prefix, passes `/dev/null` through as-is,
/// and drops any trailing tab-separated timestamp (`git diff --no-index`
/// against a real file on disk can append one).
fn parse_diff_operand_path(rest: &str) -> PathBuf {
    let rest = rest.split('\t').next().unwrap_or(rest).trim();
    if rest == "/dev/null" {
        return PathBuf::from(rest);
    }
    let stripped = rest
        .strip_prefix("a/")
        .or_else(|| rest.strip_prefix("b/"))
        .unwrap_or(rest);
    PathBuf::from(stripped)
}

/// Split `raw` into per-file sections (each starting at a `diff --git `
/// line) and parse each into a [`FileDiff`].
fn parse_files(raw: &str) -> Vec<FileDiff> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("diff --git ") {
            let start = i;
            i += 1;
            while i < lines.len() && !lines[i].starts_with("diff --git ") {
                i += 1;
            }
            files.push(parse_file_section(&lines[start..i]));
        } else {
            // Stray preamble (e.g. a leading blank line) outside any file
            // section; nothing to do with it.
            i += 1;
        }
    }
    files
}

/// Parse one `diff --git ...` section (header lines + zero or more hunks)
/// into a [`FileDiff`].
fn parse_file_section(section: &[&str]) -> FileDiff {
    let mut rename_from: Option<PathBuf> = None;
    let mut rename_to: Option<PathBuf> = None;
    let mut is_new_file = false;
    let mut is_deleted_file = false;
    let mut old_path: Option<PathBuf> = None; // from the "--- " line
    let mut new_path: Option<PathBuf> = None; // from the "+++ " line

    for line in section.iter() {
        if line.starts_with("@@") {
            // Header metadata is always emitted before the first hunk.
            break;
        }
        if let Some(rest) = line.strip_prefix("rename from ") {
            rename_from = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            rename_to = Some(PathBuf::from(rest));
        } else if line.starts_with("new file mode") {
            is_new_file = true;
        } else if line.starts_with("deleted file mode") {
            is_deleted_file = true;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            old_path = Some(parse_diff_operand_path(rest));
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            new_path = Some(parse_diff_operand_path(rest));
        }
    }

    let dev_null = Path::new("/dev/null");
    let path = rename_to
        .clone()
        .or_else(|| new_path.clone().filter(|p| p.as_path() != dev_null))
        .or_else(|| old_path.clone().filter(|p| p.as_path() != dev_null))
        .or_else(|| parse_git_header_new_path(section.first().copied().unwrap_or("")))
        .unwrap_or_else(|| PathBuf::from("unknown"));

    let status = if let Some(from) = rename_from {
        FileStatus::Renamed { from }
    } else if is_new_file || matches!(&old_path, Some(p) if p.as_path() == dev_null) {
        FileStatus::Added
    } else if is_deleted_file || matches!(&new_path, Some(p) if p.as_path() == dev_null) {
        FileStatus::Deleted
    } else {
        FileStatus::Modified
    };

    let hunks = parse_hunks(&path, section);
    let added = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| matches!(l, DiffLine::Added(_)))
        .count();
    let removed = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| matches!(l, DiffLine::Removed(_)))
        .count();

    FileDiff {
        path,
        status,
        hunks,
        added,
        removed,
    }
}

/// Fallback path extraction straight from the `diff --git a/<p> b/<p>`
/// header line, used only when neither a `+++`/`---` operand nor a rename
/// pair yielded a usable path (e.g. a pure-mode-change section with no
/// content hunks and no `rename to`).
fn parse_git_header_new_path(header_line: &str) -> Option<PathBuf> {
    let rest = header_line.strip_prefix("diff --git ")?;
    let idx = rest.find(" b/")?;
    let b_part = &rest[idx + 3..];
    Some(PathBuf::from(b_part))
}

/// Parse every `@@ ... @@` hunk in a file section, salting each hunk's hash
/// with its 0-based occurrence index within this file.
fn parse_hunks(path: &Path, section: &[&str]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut i = 0;
    let mut occurrence = 0usize;
    while i < section.len() {
        if !section[i].starts_with("@@") {
            i += 1;
            continue;
        }
        let header = section[i].to_string();
        i += 1;
        let mut lines = Vec::new();
        while i < section.len() && !section[i].starts_with("@@") && !section[i].starts_with("diff --git ") {
            let raw_line = section[i];
            i += 1;
            if raw_line.starts_with('\\') {
                // e.g. "\ No newline at end of file" — not a content line.
                continue;
            }
            let mut chars = raw_line.chars();
            let (marker, content) = match chars.next() {
                Some(m) => (m, chars.as_str().to_string()),
                None => (' ', String::new()),
            };
            let dl = match marker {
                '+' => DiffLine::Added(content),
                '-' => DiffLine::Removed(content),
                _ => DiffLine::Context(content),
            };
            lines.push(dl);
        }
        let hunk_hash = compute_hunk_hash(path, occurrence, &lines);
        hunks.push(Hunk {
            header,
            lines,
            hunk_hash,
            reviewed: false,
        });
        occurrence += 1;
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(raw: &str) -> DiffModel {
        parse_diff(
            raw,
            PathBuf::from("/tmp/some-worktree"),
            "feature-branch",
            "main",
            "deadbeef",
        )
    }

    /// Multi-file diff: two independently modified files, each with one
    /// hunk. Every top-level metadata field round-trips and both files
    /// parse with the right path + hunk content.
    #[test]
    fn parses_multi_file_diff() {
        let raw = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1111111..2222222 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,3 +1,3 @@
 fn foo() {
-    old_line();
+    new_line();
 }
diff --git a/src/bar.rs b/src/bar.rs
index 3333333..4444444 100644
--- a/src/bar.rs
+++ b/src/bar.rs
@@ -10,2 +10,3 @@ fn bar() {
     let x = 1;
+    let y = 2;
     let z = 3;
";
        let m = model(raw);
        assert_eq!(m.worktree, PathBuf::from("/tmp/some-worktree"));
        assert_eq!(m.branch, "feature-branch");
        assert_eq!(m.base, "main");
        assert_eq!(m.merge_base, "deadbeef");
        assert!(m.dirty);
        assert_eq!(m.files.len(), 2);

        assert_eq!(m.files[0].path, PathBuf::from("src/foo.rs"));
        assert_eq!(m.files[0].status, FileStatus::Modified);
        assert_eq!(m.files[0].hunks.len(), 1);
        assert_eq!(
            m.files[0].hunks[0].lines,
            vec![
                DiffLine::Context("fn foo() {".to_string()),
                DiffLine::Removed("    old_line();".to_string()),
                DiffLine::Added("    new_line();".to_string()),
                DiffLine::Context("}".to_string()),
            ]
        );
        assert_eq!(m.files[0].added, 1);
        assert_eq!(m.files[0].removed, 1);

        assert_eq!(m.files[1].path, PathBuf::from("src/bar.rs"));
        assert_eq!(m.files[1].status, FileStatus::Modified);
        assert_eq!(m.files[1].added, 1);
        assert_eq!(m.files[1].removed, 0);
    }

    /// A pure rename (no content change) is reported as `Renamed { from }`
    /// with the new path as `FileDiff::path`, and produces no hunks.
    #[test]
    fn parses_pure_rename() {
        let raw = "\
diff --git a/src/old_name.rs b/src/new_name.rs
similarity index 100%
rename from src/old_name.rs
rename to src/new_name.rs
";
        let m = model(raw);
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].path, PathBuf::from("src/new_name.rs"));
        assert_eq!(
            m.files[0].status,
            FileStatus::Renamed {
                from: PathBuf::from("src/old_name.rs")
            }
        );
        assert!(m.files[0].hunks.is_empty());
    }

    /// A rename that also changes content: still `Renamed { from }`, and the
    /// content hunk parses normally under the new path.
    #[test]
    fn parses_rename_with_content_change() {
        let raw = "\
diff --git a/src/old_name.rs b/src/new_name.rs
similarity index 90%
rename from src/old_name.rs
rename to src/new_name.rs
index 5555555..6666666 100644
--- a/src/old_name.rs
+++ b/src/new_name.rs
@@ -1,2 +1,2 @@
-fn old_name() {}
+fn new_name() {}
";
        let m = model(raw);
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].path, PathBuf::from("src/new_name.rs"));
        assert_eq!(
            m.files[0].status,
            FileStatus::Renamed {
                from: PathBuf::from("src/old_name.rs")
            }
        );
        assert_eq!(m.files[0].hunks.len(), 1);
    }

    /// An untracked file (surfaced via a non-mutating `git diff --no-index
    /// /dev/null <file>` per spec B2) parses as `Added`, with every line in
    /// its single hunk classified `Added`.
    #[test]
    fn untracked_file_is_all_added() {
        let raw = "\
diff --git a/dev/null b/new_file.txt
new file mode 100644
index 0000000..abc1234
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,3 @@
+line one
+line two
+line three
";
        let m = model(raw);
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].path, PathBuf::from("new_file.txt"));
        assert_eq!(m.files[0].status, FileStatus::Added);
        assert_eq!(m.files[0].hunks.len(), 1);
        assert!(m.files[0].hunks[0]
            .lines
            .iter()
            .all(|l| matches!(l, DiffLine::Added(_))));
        assert_eq!(m.files[0].added, 3);
        assert_eq!(m.files[0].removed, 0);
    }

    /// A deleted file parses as `Deleted`.
    #[test]
    fn parses_deleted_file() {
        let raw = "\
diff --git a/src/gone.rs b/src/gone.rs
deleted file mode 100644
index 1234567..0000000 100644
--- a/src/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn gone() {}
-
";
        let m = model(raw);
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].path, PathBuf::from("src/gone.rs"));
        assert_eq!(m.files[0].status, FileStatus::Deleted);
    }

    /// Two hunks in the same file with byte-for-byte identical content
    /// lines (only their `@@` position numbers differ) must still hash
    /// DIFFERENTLY — the per-file occurrence-index salt is what distinguishes
    /// them, since content alone would collide.
    #[test]
    fn duplicate_hunks_in_one_file_hash_differently() {
        let raw = "\
diff --git a/src/dup.rs b/src/dup.rs
index 1111111..2222222 100644
--- a/src/dup.rs
+++ b/src/dup.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old();
+    new();
 }
@@ -20,3 +20,3 @@
 fn a() {
-    old();
+    new();
 }
";
        let m = model(raw);
        assert_eq!(m.files.len(), 1);
        let hunks = &m.files[0].hunks;
        assert_eq!(hunks.len(), 2);
        // Content lines are identical between the two hunks...
        assert_eq!(hunks[0].lines, hunks[1].lines);
        // ...but the occurrence-index salt must still make the hashes differ.
        assert_ne!(hunks[0].hunk_hash, hunks[1].hunk_hash);
    }

    /// A position-only change — the `@@ -a,b +c,d @@` numbers move because an
    /// earlier, unrelated hunk in the file grew/shrank — must NOT change the
    /// hash of a hunk whose content lines and occurrence index are unchanged.
    #[test]
    fn position_only_change_keeps_hash_stable() {
        let raw_before = "\
diff --git a/src/pos.rs b/src/pos.rs
index 1111111..2222222 100644
--- a/src/pos.rs
+++ b/src/pos.rs
@@ -5,3 +5,3 @@
 context
-old
+new
";
        let raw_after = "\
diff --git a/src/pos.rs b/src/pos.rs
index 1111111..2222222 100644
--- a/src/pos.rs
+++ b/src/pos.rs
@@ -50,3 +52,3 @@
 context
-old
+new
";
        let before = model(raw_before);
        let after = model(raw_after);
        assert_eq!(before.files[0].hunks[0].header, "@@ -5,3 +5,3 @@");
        assert_eq!(after.files[0].hunks[0].header, "@@ -50,3 +52,3 @@");
        assert_eq!(
            before.files[0].hunks[0].hunk_hash,
            after.files[0].hunks[0].hunk_hash
        );
    }

    /// Every hunk out of the parser starts unreviewed — joining with
    /// persisted `ReviewState` is a later node's job (spec § Data Model).
    #[test]
    fn every_hunk_starts_unreviewed() {
        let raw = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1111111..2222222 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,1 +1,1 @@
-old
+new
";
        let m = model(raw);
        assert!(!m.files[0].hunks[0].reviewed);
    }

    /// An empty diff (nothing changed) parses to zero files and `dirty ==
    /// false`.
    #[test]
    fn empty_diff_is_not_dirty() {
        let m = model("");
        assert!(m.files.is_empty());
        assert!(!m.dirty);
    }
}
