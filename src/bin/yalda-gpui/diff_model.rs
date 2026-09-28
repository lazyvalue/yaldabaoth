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
//! ## `file_hash`
//!
//! Each [`FileDiff`] carries a `file_hash` — its review identity (spec rev 2
//! B4): a hash of the repo-relative path plus every hunk's content lines
//! (kind + text), never the `@@` positions. A change that only shifts line
//! numbers keeps it; any content change to the file's diff changes it, which
//! is what clears Viewed. See [`compute_file_hash`].

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
    /// The file's review identity (spec rev 2 § Data Model, B4): a stable hash
    /// of the repo-relative path plus every hunk's content lines (kind + text),
    /// EXCLUDING `@@` positions. A position-only shift keeps it; any content
    /// change to the file's diff changes it, which is what clears Viewed.
    /// See [`compute_file_hash`].
    pub file_hash: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The raw `@@ -a,b +c,d @@ ...` header line, kept verbatim for display.
    /// Never fed into `file_hash` (see module docs).
    pub header: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The inclusive new-file line range this hunk covers (spec B4: "the new
    /// line range"), parsed from the `+c,d` operand of the `@@ -a,b +c,d @@`
    /// header. `header` is display-only everywhere else in this module (never
    /// fed to `file_hash`), but it is the ONLY place the new-file line numbers
    /// survive parsing — `DiffLine` content carries no position — so this is
    /// the one legitimate reader of it. Returns `(0, 0)` for a header that
    /// fails to parse (defensive; every hunk this parser emits has a
    /// well-formed header).
    pub fn new_line_range(&self) -> (usize, usize) {
        parse_new_line_range(&self.header).unwrap_or((0, 0))
    }

    /// The `(old_start, new_start)` line numbers from the `-a,b +c,d` header
    /// operands — where this hunk's first old-side / new-side line sits. Line
    /// numbering of every row (the Diff tile's gutters) counts up from here.
    /// `(0, 0)` for an unparseable header (defensive).
    pub fn starts(&self) -> (usize, usize) {
        let operand = |sigil: &str| -> Option<usize> {
            let after = self.header.split(sigil).nth(1)?;
            after.split([',', ' ']).next()?.parse().ok()
        };
        (operand(" -").unwrap_or(0), operand(" +").unwrap_or(0))
    }

    /// Per content line, its `(old, new)` line numbers — `None` on the side
    /// the line doesn't exist on (an added line has no old number, a removed
    /// line no new number). Context lines carry both.
    pub fn line_numbers(&self) -> Vec<(Option<usize>, Option<usize>)> {
        let (mut o, mut n) = self.starts();
        self.lines
            .iter()
            .map(|l| match l {
                DiffLine::Context(_) => {
                    let r = (Some(o), Some(n));
                    o += 1;
                    n += 1;
                    r
                }
                DiffLine::Removed(_) => {
                    o += 1;
                    (Some(o - 1), None)
                }
                DiffLine::Added(_) => {
                    n += 1;
                    (None, Some(n - 1))
                }
            })
            .collect()
    }

    /// Reconstruct the hunk's patch text verbatim: the header line, then each
    /// content line with its `+`/`-`/` ` prefix restored (spec B4: "the hunk
    /// patch text (header + lines)"). The inverse of `parse_hunks`' line
    /// splitting, minus any `\ No newline at end of file` markers (dropped at
    /// parse time — spec doesn't need them echoed back to the agent).
    pub fn patch_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.header);
        out.push('\n');
        for line in &self.lines {
            let (prefix, text) = match line {
                DiffLine::Added(t) => ("+", t.as_str()),
                DiffLine::Removed(t) => ("-", t.as_str()),
                DiffLine::Context(t) => (" ", t.as_str()),
            };
            out.push_str(prefix);
            out.push_str(text);
            out.push('\n');
        }
        out
    }
}

/// Parse the `+c,d` (new-file start,count) operand out of a `@@ -a,b +c,d @@`
/// hunk header into the inclusive `(first, last)` new-file line range. `d`
/// defaults to 1 when omitted (unified-diff convention for a single-line
/// hunk); a `d == 0` hunk (a pure deletion — no new lines survive at all)
/// reports `(c, c)` as a single-line anchor rather than an empty/inverted
/// range.
fn parse_new_line_range(header: &str) -> Option<(usize, usize)> {
    let after_plus = header.split(" +").nth(1)?;
    let operand = after_plus.split(' ').next()?;
    let mut parts = operand.splitn(2, ',');
    let start: usize = parts.next()?.parse().ok()?;
    let count: usize = match parts.next() {
        Some(c) => c.parse().ok()?,
        None => 1,
    };
    if count == 0 {
        Some((start, start))
    } else {
        Some((start, start + count - 1))
    }
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

/// Compute a file's `file_hash` (spec rev 2 B4): the repo-relative `path`
/// plus, for each hunk in order, its content lines (`DiffLine` hashes both the
/// Context/Added/Removed kind and the text; a slice hash is length-prefixed, so
/// hunk boundaries are part of the identity). The `@@` header is never fed in,
/// so a position-only shift leaves the hash unchanged.
pub fn compute_file_hash(path: &Path, hunks: &[Hunk]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hunks.len().hash(&mut hasher);
    for hunk in hunks {
        hunk.lines.hash(&mut hasher);
    }
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

    let hunks = parse_hunks(section);
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
    let file_hash = compute_file_hash(&path, &hunks);

    FileDiff {
        path,
        status,
        hunks,
        added,
        removed,
        file_hash,
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

/// Parse every `@@ ... @@` hunk in a file section.
fn parse_hunks(section: &[&str]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut i = 0;
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
        hunks.push(Hunk { header, lines });
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

    /// `starts` reads both header operands; `line_numbers` counts context on
    /// both sides, removed on old only, added on new only.
    #[test]
    fn line_numbers_count_each_side() {
        let raw = one_file("src/n.rs", "@@ -10,3 +20,3 @@ fn x()", " a\n-b\n+c\n d\n");
        let m = model(&raw);
        let h = &m.files[0].hunks[0];
        assert_eq!(h.starts(), (10, 20));
        assert_eq!(
            h.line_numbers(),
            vec![
                (Some(10), Some(20)),
                (Some(11), None),
                (None, Some(21)),
                (Some(12), Some(22)),
            ]
        );
        let single = model(&one_file("src/n.rs", "@@ -1 +1 @@", "-x\n+y\n"));
        assert_eq!(single.files[0].hunks[0].starts(), (1, 1));
    }

    /// An empty diff (nothing changed) parses to zero files and `dirty ==
    /// false`.
    #[test]
    fn empty_diff_is_not_dirty() {
        let m = model("");
        assert!(m.files.is_empty());
        assert!(!m.dirty);
    }

    // ── Cog node `comment-steering` (hk81): `Hunk::new_line_range` /
    // `Hunk::patch_text`, spec B4 ────────────────────────────────────────────

    #[test]
    fn new_line_range_reads_the_plus_operand() {
        let raw = "\
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
        assert_eq!(m.files[0].hunks[0].new_line_range(), (10, 12));
    }

    /// A single-line hunk omits the count (`+c` not `+c,1`) per unified-diff
    /// convention — the count must default to 1.
    #[test]
    fn new_line_range_defaults_omitted_count_to_one() {
        let raw = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1111111..2222222 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1 +1 @@
-old
+new
";
        let m = model(raw);
        assert_eq!(m.files[0].hunks[0].new_line_range(), (1, 1));
    }

    /// A pure deletion (`+c,0` — no new lines survive at all) reports a
    /// single-line anchor rather than an empty/inverted range.
    #[test]
    fn new_line_range_zero_count_anchors_to_start() {
        let raw = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1111111..2222222 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,2 +0,0 @@
-old1
-old2
";
        let m = model(raw);
        assert_eq!(m.files[0].hunks[0].new_line_range(), (0, 0));
    }

    #[test]
    fn patch_text_round_trips_header_and_prefixed_lines() {
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
";
        let m = model(raw);
        assert_eq!(
            m.files[0].hunks[0].patch_text(),
            "@@ -1,3 +1,3 @@\n fn foo() {\n-    old_line();\n+    new_line();\n }\n"
        );
    }

    // ── Cog graph 8g7 node `review-store`: `FileDiff::file_hash` (spec rev 2
    // B4 — Viewed is keyed by path + file_hash) ─────────────────────────────

    fn one_file(path: &str, header: &str, body: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n{header}\n{body}"
        )
    }

    /// A position-only shift (only the `@@` numbers move) keeps `file_hash`.
    #[test]
    fn file_hash_is_position_independent() {
        let before = model(&one_file("src/pos.rs", "@@ -5,3 +5,3 @@", " context\n-old\n+new\n"));
        let after = model(&one_file("src/pos.rs", "@@ -50,3 +52,3 @@ fn x()", " context\n-old\n+new\n"));
        assert_ne!(before.files[0].hunks[0].header, after.files[0].hunks[0].header);
        assert_eq!(before.files[0].file_hash, after.files[0].file_hash);
    }

    /// Any content change (text or kind) to the file's diff changes `file_hash`.
    #[test]
    fn file_hash_changes_with_content() {
        let base = model(&one_file("src/a.rs", "@@ -1,3 +1,3 @@", " context\n-old\n+new\n"));
        let text = model(&one_file("src/a.rs", "@@ -1,3 +1,3 @@", " context\n-old\n+newer\n"));
        let kind = model(&one_file("src/a.rs", "@@ -1,3 +1,3 @@", " context\n-old\n new\n"));
        assert_ne!(base.files[0].file_hash, text.files[0].file_hash);
        assert_ne!(base.files[0].file_hash, kind.files[0].file_hash);
    }

    /// Identical diff content in two different files hashes differently (path
    /// is part of the identity), and the hash is deterministic.
    #[test]
    fn file_hash_differs_across_paths() {
        let body = " context\n-old\n+new\n";
        let a = model(&one_file("src/a.rs", "@@ -1,3 +1,3 @@", body));
        let b = model(&one_file("src/b.rs", "@@ -1,3 +1,3 @@", body));
        let a2 = model(&one_file("src/a.rs", "@@ -1,3 +1,3 @@", body));
        assert_ne!(a.files[0].file_hash, b.files[0].file_hash);
        assert_eq!(a.files[0].file_hash, a2.files[0].file_hash);
        assert_eq!(
            a.files[0].file_hash,
            compute_file_hash(&a.files[0].path, &a.files[0].hunks)
        );
    }
}
