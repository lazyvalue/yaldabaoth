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
//! (`model`), the loaded `review`, the line cursor, and the fold state on the
//! TILE itself (the single source of truth persistence/menus can read without
//! a `cx` round-trip through a cached view). `DiffView` therefore does not own
//! a copy of this state — it reads it off the root view each render (see
//! `diff_view.rs`'s module doc for how that's kept O(changed) anyway).
//!
//! # The row model (spec rev 2 B2, UXI-Diff-11)
//!
//! The bound body is a flat list of **visible rows** ([`RowRef`]), derived by
//! the pure [`visible_rows`] from `(model, review, folds)`: per file a
//! `File` header row, then — unless the file is folded — for each hunk a thin
//! `Hunk` header row followed by its `Line` rows. The **line cursor** is a flat
//! index into that list (`DiffTile::cursor`). Rows are cached on the tile
//! (`DiffTile::rows`, an `Rc` so the virtualized list's `'static` row closure
//! can hold it) and rebuilt only at mutation sites (derive / fold / Viewed) —
//! never per frame. All nav (`j`/`k`, `{`/`}`, `[`/`]`, `z`, `v`) is pure
//! index arithmetic over that list, unit-tested below.
//!
//! A file is folded iff `Folds::is_collapsed(path, viewed)`: a VIEWED file is
//! folded unless the user explicitly expanded it with `z`; an unviewed one is
//! folded only if the user collapsed it with `z`.

use super::*;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// One visible row of the bound Diff body (see module docs). `Copy` + `Eq` so
/// the virtualized list can splice by content diff (`ScrollAnchoredList`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowRef {
    /// A file header: status glyph, path, `+a −r`, Viewed checkbox.
    File {
        file: usize,
        viewed: bool,
        collapsed: bool,
    },
    /// A thin hunk header (`@@ -a,b +c,d @@`, dimmed) — the `{`/`}` stop.
    Hunk { file: usize, hunk: usize },
    /// One diff line. `old`/`new` are its line numbers on each side (`None`
    /// on the side it doesn't exist on: added ⇒ no old, removed ⇒ no new).
    Line {
        file: usize,
        hunk: usize,
        line: usize,
        old: Option<u32>,
        new: Option<u32>,
    },
    /// One fixed-height row of an inline comment card (spec B5,
    /// UXI-Diff-15). A card is `parts` consecutive rows (`part` 0 = the
    /// header: id · badge · first body line; then wrapped body lines — see
    /// [`comment_card_lines`]) so the body stays uniform-height and
    /// `DiffView::reveal_cursor` stays exact. `comment` indexes
    /// `Review::comments` at the time the rows were built (rows are rebuilt
    /// at every review mutation).
    Comment {
        file: usize,
        comment: usize,
        part: u8,
        parts: u8,
    },
}

impl RowRef {
    /// The file index this row belongs to.
    pub(crate) fn file(&self) -> usize {
        match *self {
            RowRef::File { file, .. }
            | RowRef::Hunk { file, .. }
            | RowRef::Line { file, .. }
            | RowRef::Comment { file, .. } => file,
        }
    }

    pub(crate) fn is_line(&self) -> bool {
        matches!(self, RowRef::Line { .. })
    }

    /// The `Review::comments` index of a comment-card row.
    pub(crate) fn comment_index(&self) -> Option<usize> {
        match *self {
            RowRef::Comment { comment, .. } => Some(comment),
            _ => None,
        }
    }

    pub(crate) fn is_file(&self) -> bool {
        matches!(self, RowRef::File { .. })
    }

    pub(crate) fn is_hunk(&self) -> bool {
        matches!(self, RowRef::Hunk { .. })
    }
}

/// Per-file fold overrides (spec B2 `z` + B4 "a viewed file collapses").
/// `collapsed` = unviewed files the user folded; `expanded` = viewed files the
/// user unfolded. Keyed by path so folds survive a re-derive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Folds {
    pub(crate) collapsed: HashSet<PathBuf>,
    pub(crate) expanded: HashSet<PathBuf>,
}

impl Folds {
    /// Viewed ⇒ folded unless explicitly expanded; unviewed ⇒ folded only if
    /// explicitly collapsed.
    pub(crate) fn is_collapsed(&self, path: &std::path::Path, viewed: bool) -> bool {
        if viewed {
            !self.expanded.contains(path)
        } else {
            self.collapsed.contains(path)
        }
    }

    /// `z`: flip the file's effective fold.
    pub(crate) fn toggle(&mut self, path: &std::path::Path, viewed: bool) {
        let folded = self.is_collapsed(path, viewed);
        let set = if viewed { &mut self.expanded } else { &mut self.collapsed };
        // viewed: folded ⇒ add to expanded; unviewed: folded ⇒ remove from collapsed.
        if folded == viewed {
            set.insert(path.to_path_buf());
        } else {
            set.remove(path);
        }
    }

    /// Viewed flipped for `path`: drop its overrides so the default applies
    /// (marking viewed folds it, unmarking re-expands it — spec B4).
    pub(crate) fn reset(&mut self, path: &std::path::Path) {
        self.collapsed.remove(path);
        self.expanded.remove(path);
    }
}

/// The visible rows of `model` given the Viewed marks in `review` and the
/// fold overrides (module docs), with each comment's card rows placed inline
/// (spec B5): a live comment right after the line its snippet ends on (the
/// match nearest its stored line numbers), an outdated / unplaceable one right
/// after its file header. Folded files show no cards. Pure; O(visible lines ×
/// comments of that file).
pub(crate) fn visible_rows(model: &DiffModel, review: Option<&Review>, folds: &Folds) -> Vec<RowRef> {
    let mut rows = Vec::new();
    for (fi, f) in model.files.iter().enumerate() {
        let viewed = review.is_some_and(|r| r.is_viewed(f));
        let collapsed = folds.is_collapsed(&f.path, viewed);
        rows.push(RowRef::File {
            file: fi,
            viewed,
            collapsed,
        });
        if collapsed {
            continue;
        }
        let mut body = Vec::new();
        for (hi, h) in f.hunks.iter().enumerate() {
            body.push(RowRef::Hunk { file: fi, hunk: hi });
            for (li, (old, new)) in h.line_numbers().into_iter().enumerate() {
                body.push(RowRef::Line {
                    file: fi,
                    hunk: hi,
                    line: li,
                    old: old.map(|n| n as u32),
                    new: new.map(|n| n as u32),
                });
            }
        }
        // Place this file's comments: `top` right after the header, `after[k]`
        // right after body row k.
        let mut top: Vec<usize> = Vec::new();
        let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
        if let Some(r) = review {
            for (ci, c) in r.comments.iter().enumerate().filter(|(_, c)| c.path == f.path) {
                match (!c.outdated).then(|| place_comment(f, &body, c)).flatten() {
                    Some(k) => after.entry(k).or_default().push(ci),
                    None => top.push(ci),
                }
            }
        }
        let push_card = |rows: &mut Vec<RowRef>, ci: usize| {
            let Some(c) = review.and_then(|r| r.comments.get(ci)) else {
                return;
            };
            let parts = comment_card_lines(c).len().clamp(1, u8::MAX as usize) as u8;
            for part in 0..parts {
                rows.push(RowRef::Comment {
                    file: fi,
                    comment: ci,
                    part,
                    parts,
                });
            }
        };
        for ci in top {
            push_card(&mut rows, ci);
        }
        for (k, row) in body.into_iter().enumerate() {
            rows.push(row);
            if let Some(cards) = after.get(&k) {
                for &ci in cards {
                    push_card(&mut rows, ci);
                }
            }
        }
    }
    rows
}

/// The text of diff line `row` (a `Line` row) in `file`.
fn line_text(file: &FileDiff, row: RowRef) -> Option<&str> {
    let RowRef::Line { hunk, line, .. } = row else {
        return None;
    };
    match file.hunks.get(hunk)?.lines.get(line)? {
        DiffLine::Added(t) | DiffLine::Removed(t) | DiffLine::Context(t) => Some(t.as_str()),
    }
}

/// `row`'s line number on `side` (`None` when the line isn't on that side).
fn side_number(row: RowRef, side: CommentSide) -> Option<u32> {
    match (row, side) {
        (RowRef::Line { new, .. }, CommentSide::New) => new,
        (RowRef::Line { old, .. }, CommentSide::Old) => old,
        _ => None,
    }
}

/// Where comment `c`'s card goes among one file's `body` rows (hunk + line
/// rows, no cards): the index of the LAST line of the run of `c.side` lines
/// matching `c.snippet` (trailing whitespace ignored, the same rule as
/// `Review::recompute_outdated`), choosing the match whose last line number
/// is nearest `c.lines[1]` so a comment follows its code when lines drift.
/// An empty snippet falls back to the exact line number. `None` ⇒ unplaceable.
pub(crate) fn place_comment(file: &FileDiff, body: &[RowRef], c: &ReviewComment) -> Option<usize> {
    let side: Vec<(usize, u32, &str)> = body
        .iter()
        .enumerate()
        .filter_map(|(k, r)| Some((k, side_number(*r, c.side)?, line_text(file, *r)?.trim_end())))
        .collect();
    let mut needle: Vec<&str> = c.snippet.split('\n').map(str::trim_end).collect();
    if needle.len() > 1 && needle.last() == Some(&"") {
        needle.pop();
    }
    let target = c.lines[1] as u32;
    if c.snippet.is_empty() {
        return side.iter().find(|(_, n, _)| *n == target).map(|(k, _, _)| *k);
    }
    if needle.len() > side.len() {
        return None;
    }
    side.windows(needle.len())
        .filter(|w| w.iter().map(|(_, _, t)| *t).eq(needle.iter().copied()))
        .map(|w| w[w.len() - 1])
        .min_by_key(|(_, n, _)| n.abs_diff(target))
        .map(|(k, _, _)| k)
}

/// Mono column budget a comment card's body wraps at (the card rows are
/// fixed-height, so wrapping is by character count, not measured text).
pub(crate) const COMMENT_WRAP_COLS: usize = 88;
/// Body rows shown per card before an ellipsis row.
pub(crate) const COMMENT_MAX_BODY_ROWS: usize = 8;
/// Snippet rows an outdated card shows (dimmed) before an ellipsis.
pub(crate) const COMMENT_MAX_SNIPPET_ROWS: usize = 3;

/// One row of a comment card's content (row 0 is always `Body` — the header
/// row shows it after the id + badge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CardLine {
    Body(String),
    /// The anchored code, shown dimmed on an outdated card.
    Snippet(String),
    /// "…" — more body / snippet than fits.
    More,
}

/// Greedy word-wrap of `text` at `cols` characters (hard-breaking a word
/// longer than `cols`); every `\n` starts a new line; an empty text is one
/// empty line.
pub(crate) fn wrap_cols(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in para.split(' ') {
            let wlen = word.chars().count();
            if len > 0 && len + 1 + wlen > cols {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            let mut chars = word.chars().peekable();
            while chars.peek().is_some() {
                if len == cols {
                    out.push(std::mem::take(&mut line));
                    len = 0;
                }
                line.push(chars.next().unwrap());
                len += 1;
            }
        }
        out.push(line);
    }
    out
}

/// The content rows of `c`'s card (see [`CardLine`]): wrapped body capped at
/// [`COMMENT_MAX_BODY_ROWS`] (last shown row becomes "…" when cut), then — for
/// an outdated comment — its snippet capped at [`COMMENT_MAX_SNIPPET_ROWS`].
pub(crate) fn comment_card_lines(c: &ReviewComment) -> Vec<CardLine> {
    let mut body = wrap_cols(c.body.trim_end(), COMMENT_WRAP_COLS);
    let mut out: Vec<CardLine> = Vec::new();
    if body.len() > COMMENT_MAX_BODY_ROWS {
        body.truncate(COMMENT_MAX_BODY_ROWS - 1);
        out.extend(body.into_iter().map(CardLine::Body));
        out.push(CardLine::More);
    } else {
        out.extend(body.into_iter().map(CardLine::Body));
    }
    if c.outdated && !c.snippet.is_empty() {
        let snip: Vec<&str> = c.snippet.trim_end_matches('\n').split('\n').collect();
        let more = snip.len() > COMMENT_MAX_SNIPPET_ROWS;
        for l in snip.into_iter().take(COMMENT_MAX_SNIPPET_ROWS) {
            out.push(CardLine::Snippet(l.to_string()));
        }
        if more {
            out.push(CardLine::Snippet("…".to_string()));
        }
    }
    out
}

/// Where a new comment anchors (spec B5): repo path, side, inclusive line
/// span on that side, and the snippet (the anchored lines' text, `\n`-joined).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommentAnchor {
    pub(crate) path: PathBuf,
    pub(crate) side: CommentSide,
    pub(crate) lines: [usize; 2],
    pub(crate) snippet: String,
}

impl CommentAnchor {
    /// `a.txt:2` / `a.txt:2–4` (`(old)` suffix for an old-side anchor).
    pub(crate) fn label(&self) -> String {
        let span = if self.lines[0] == self.lines[1] {
            self.lines[0].to_string()
        } else {
            format!("{}–{}", self.lines[0], self.lines[1])
        };
        let old = if self.side == CommentSide::Old { " (old)" } else { "" };
        format!("{}:{span}{old}", self.path.display())
    }

    pub(crate) fn of_comment(c: &ReviewComment) -> Self {
        CommentAnchor {
            path: c.path.clone(),
            side: c.side,
            lines: c.lines,
            snippet: c.snippet.clone(),
        }
    }
}

/// The comment anchor for the `Line` rows among `rows[lo..=hi]` (spec B5):
/// New side if any of them has a new-side number (restricted to those
/// lines), Old only when every line is a removed line. `None` when the span
/// holds no line row.
pub(crate) fn comment_anchor_for_range(model: &DiffModel, rows: &[RowRef], lo: usize, hi: usize) -> Option<CommentAnchor> {
    let hi = hi.min(rows.len().checked_sub(1)?);
    let lines: Vec<LineAnchor> = rows.get(lo..=hi)?.iter().filter_map(|r| line_anchor(model, *r)).collect();
    let any_new = lines.iter().any(|l| l.side == CommentSide::New);
    let side = if any_new { CommentSide::New } else { CommentSide::Old };
    let picked: Vec<&LineAnchor> = lines.iter().filter(|l| l.side == side).collect();
    let (first, last) = (picked.first()?, picked.last()?);
    Some(CommentAnchor {
        path: first.path.clone(),
        side,
        lines: [first.line, last.line],
        snippet: picked.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"),
    })
}

/// The index of `rows[i]`'s "host" row: a comment-card row resolves to the
/// nearest preceding non-card row (its anchor line, or the file header for
/// a top-of-file card); any other row is itself.
pub(crate) fn host_row(rows: &[RowRef], i: usize) -> usize {
    let mut j = i.min(rows.len().saturating_sub(1));
    while j > 0 && matches!(rows.get(j), Some(RowRef::Comment { .. })) {
        j -= 1;
    }
    j
}

/// The comment editor pinned at the bottom of a Diff tile (spec B5): the
/// text input (the app's shared [`Compose`] editor, dispatched through
/// `dispatch_insert_core` like every other compose), what saving does, and
/// the anchor snapshot taken when it opened (a background refresh can't move
/// the ground the comment is being written against).
pub(crate) struct CommentCompose {
    pub(crate) input: Compose,
    pub(crate) target: ComposeTarget,
    pub(crate) anchor: CommentAnchor,
    /// First Esc on a non-empty draft arms this ("Esc again to discard");
    /// a second Esc discards; any other key disarms.
    pub(crate) esc_armed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposeTarget {
    /// Save adds a new unsent comment.
    New,
    /// Save replaces this comment's body.
    Edit(String),
}

/// `j`/`k`: move by `delta` rows, clamped (no wrap).
pub(crate) fn step_row(rows: &[RowRef], cur: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    (cur as i64 + delta as i64).clamp(0, rows.len() as i64 - 1) as usize
}

/// The nearest row strictly after (`forward`) / before `cur` matching `pred`.
fn seek_row(rows: &[RowRef], cur: usize, forward: bool, pred: impl Fn(&RowRef) -> bool) -> Option<usize> {
    if forward {
        (cur + 1..rows.len()).find(|&i| pred(&rows[i]))
    } else {
        (0..cur.min(rows.len())).rev().find(|&i| pred(&rows[i]))
    }
}

/// `}`/`{`: the next/prev hunk header row (None ⇒ stay).
pub(crate) fn next_hunk_row(rows: &[RowRef], cur: usize, forward: bool) -> Option<usize> {
    seek_row(rows, cur, forward, RowRef::is_hunk)
}

/// `]`/`[`: the next/prev file header row (None ⇒ stay).
pub(crate) fn next_file_row(rows: &[RowRef], cur: usize, forward: bool) -> Option<usize> {
    seek_row(rows, cur, forward, RowRef::is_file)
}

/// The header row index of file `file`.
pub(crate) fn file_header_row(rows: &[RowRef], file: usize) -> Option<usize> {
    rows.iter()
        .position(|r| matches!(r, RowRef::File { file: f, .. } if *f == file))
}

/// After marking file `after` viewed (spec B4): the header row of the next
/// UNVIEWED file after it, wrapping to the first unviewed one; `None` when
/// every other file is viewed.
pub(crate) fn next_unviewed_file_row(rows: &[RowRef], after: usize) -> Option<usize> {
    let unviewed = |r: &RowRef| matches!(r, RowRef::File { file, viewed: false, .. } if *file != after);
    let start = file_header_row(rows, after).unwrap_or(0);
    seek_row(rows, start, true, unviewed).or_else(|| rows.iter().position(unviewed))
}

/// Where the cursor sat, in model terms, so it survives a rebuild of the rows
/// (re-derive, fold, Viewed) — spec B3 / UXI-Diff-13 "same file, nearest line".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorAnchor {
    pub(crate) path: PathBuf,
    pub(crate) kind: AnchorKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnchorKind {
    File,
    Hunk { new_start: usize },
    Line { old: Option<u32>, new: Option<u32> },
}

/// The anchor of row `cursor` (None for an out-of-range cursor / no rows).
pub(crate) fn cursor_anchor(model: &DiffModel, rows: &[RowRef], cursor: usize) -> Option<CursorAnchor> {
    rows.get(cursor)?;
    let row = rows[host_row(rows, cursor)];
    let file = model.files.get(row.file())?;
    let kind = match row {
        RowRef::File { .. } | RowRef::Comment { .. } => AnchorKind::File,
        RowRef::Hunk { hunk, .. } => AnchorKind::Hunk {
            new_start: file.hunks.get(hunk).map(|h| h.starts().1).unwrap_or(0),
        },
        RowRef::Line { old, new, .. } => AnchorKind::Line { old, new },
    };
    Some(CursorAnchor {
        path: file.path.clone(),
        kind,
    })
}

/// Resolve `anchor` against a (new) `model` + `rows`: same file ⇒ the row
/// nearest the anchored line (by new-side number, else old-side), else that
/// file's header; file gone ⇒ `fallback` clamped into range.
pub(crate) fn resolve_anchor(model: &DiffModel, rows: &[RowRef], anchor: &CursorAnchor, fallback: usize) -> usize {
    let clamp = || fallback.min(rows.len().saturating_sub(1));
    let Some(fi) = model.files.iter().position(|f| f.path == anchor.path) else {
        return clamp();
    };
    let Some(header) = file_header_row(rows, fi) else {
        return clamp();
    };
    let span = rows[header..]
        .iter()
        .take_while(|r| r.file() == fi)
        .count();
    let body = header..header + span;
    let nearest = |key: &dyn Fn(&RowRef) -> Option<u32>, target: u32| -> Option<usize> {
        body.clone()
            .filter_map(|i| key(&rows[i]).map(|n| (i, n.abs_diff(target))))
            .min_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
    };
    let found = match anchor.kind {
        AnchorKind::File => None,
        AnchorKind::Hunk { new_start } => nearest(
            &|r: &RowRef| match *r {
                RowRef::Hunk { hunk, .. } => model.files[fi].hunks.get(hunk).map(|h| h.starts().1 as u32),
                _ => None,
            },
            new_start as u32,
        ),
        AnchorKind::Line { new: Some(n), .. } => nearest(
            &|r: &RowRef| match *r {
                RowRef::Line { new, .. } => new,
                _ => None,
            },
            n,
        ),
        AnchorKind::Line { old: Some(o), .. } => nearest(
            &|r: &RowRef| match *r {
                RowRef::Line { old, .. } => old,
                _ => None,
            },
            o,
        ),
        AnchorKind::Line { .. } => None,
    };
    found.unwrap_or(header)
}

/// A single diff line addressed for commenting (spec B5): the side it lives
/// on (`Old` only for a removed line), its line number on that side, and its
/// text (the comment `snippet` material). Consumed by the comments node
/// (graph 8g7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineAnchor {
    pub(crate) path: PathBuf,
    pub(crate) side: CommentSide,
    pub(crate) line: usize,
    pub(crate) text: String,
}

/// The [`LineAnchor`] of `row` — `None` unless it is a `Line` row.
pub(crate) fn line_anchor(model: &DiffModel, row: RowRef) -> Option<LineAnchor> {
    let RowRef::Line {
        file,
        hunk,
        line,
        old,
        new,
    } = row
    else {
        return None;
    };
    let f = model.files.get(file)?;
    let dl = f.hunks.get(hunk)?.lines.get(line)?;
    let (side, n, text) = match dl {
        DiffLine::Removed(t) => (CommentSide::Old, old?, t),
        DiffLine::Added(t) | DiffLine::Context(t) => (CommentSide::New, new?, t),
    };
    Some(LineAnchor {
        path: f.path.clone(),
        side,
        line: n as usize,
        text: text.clone(),
    })
}

/// The new-file line `o` (Open in Zed, spec B7) targets for row `cursor`: a
/// line's new-side number; a removed line's position in the new file (just
/// after the previous new-side line of its hunk); a hunk header's first new
/// line; a file header's first hunk (else 1). Always ≥ 1.
pub(crate) fn zed_target(model: &DiffModel, rows: &[RowRef], cursor: usize) -> Option<(PathBuf, usize)> {
    rows.get(cursor)?;
    let cursor = host_row(rows, cursor);
    let row = rows[cursor];
    let f = model.files.get(row.file())?;
    let hunk_start = |hi: usize| f.hunks.get(hi).map(|h| h.starts().1).unwrap_or(1);
    let line = match row {
        RowRef::File { .. } | RowRef::Comment { .. } => f.hunks.first().map(|h| h.starts().1).unwrap_or(1),
        RowRef::Hunk { hunk, .. } => hunk_start(hunk),
        RowRef::Line { new: Some(n), .. } => n as usize,
        RowRef::Line { hunk, .. } => rows[..cursor]
            .iter()
            .rev()
            .filter(|r| !matches!(r, RowRef::Comment { .. }))
            .take_while(|r| matches!(r, RowRef::Line { hunk: h, .. } if *h == hunk))
            .find_map(|r| match r {
                RowRef::Line { new: Some(n), .. } => Some(*n as usize + 1),
                _ => None,
            })
            .unwrap_or_else(|| hunk_start(hunk)),
    };
    Some((f.path.clone(), line.max(1)))
}

/// What a background derive hands back to `diff_apply`: the parsed diff, the
/// review reconciled against it (`None` when the review root can't be
/// resolved), and where that review lives.
pub(crate) struct DiffDerived {
    pub(crate) model: DiffModel,
    pub(crate) review: Option<Review>,
    pub(crate) review_path: Option<PathBuf>,
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
    /// Last successfully derived diff (kept during a refresh — spec B3). An
    /// `Rc` so the virtualized body's `'static` row closure can hold it
    /// without cloning the diff each frame.
    pub(crate) model: Option<Rc<DiffModel>>,
    /// The loaded review for `model.branch` (Viewed marks + comments).
    pub(crate) review: Option<Review>,
    /// Where `review` persists (`None` ⇒ the root couldn't be resolved;
    /// marks then live in memory only).
    pub(crate) review_path: Option<PathBuf>,
    /// Bumped on every change to `review` (load or local edit) — `DiffView`'s
    /// fingerprint for the header progress, and the save generation.
    pub(crate) review_gen: u64,
    /// Highest `review_gen` written to disk (shared with background saves so
    /// an out-of-order stale save never overwrites a newer one).
    pub(crate) review_saved: Arc<AtomicU64>,
    /// Per-file fold overrides (module docs).
    pub(crate) folds: Folds,
    /// The cached visible rows (module docs); rebuilt by [`rebuild_rows`](Self::rebuild_rows).
    pub(crate) rows: Rc<Vec<RowRef>>,
    /// Bumped on every `rows` rebuild — `DiffView`'s fingerprint + the
    /// list's splice version.
    pub(crate) rows_gen: u64,
    /// The line cursor: an index into `rows`.
    pub(crate) cursor: usize,
    /// `V` range-selection start (a `Line` row index in `rows`); the range is
    /// `range_anchor..=cursor` (either order), confined to one file. Cleared
    /// by every rows rebuild.
    pub(crate) range_anchor: Option<usize>,
    /// The open comment editor (spec B5) — the tile's only text-input
    /// surface while it is open.
    pub(crate) compose: Option<CommentCompose>,
    /// Bumped when `compose` opens or closes (NOT per keystroke): the body's
    /// fingerprint for the anchor highlight, so typing never re-renders it.
    pub(crate) compose_gen: u64,
    /// `x` pressed once on this comment: the next `x` deletes it (spec C6).
    pub(crate) pending_delete: Option<String>,
    pub(crate) refreshing: bool,
    /// Monotonic guard so a stale in-flight refresh can't clobber a newer
    /// one (mirrors `LinearTile::req` / `CogTile::req`).
    pub(crate) req: u64,
    /// Bumped every time `model` is replaced. Cheap render-input fingerprint
    /// for `DiffView` so it never has to hash the whole `DiffModel`.
    pub(crate) model_gen: u64,
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
            review: None,
            review_path: None,
            review_gen: 0,
            review_saved: Arc::new(AtomicU64::new(0)),
            folds: Folds::default(),
            rows: Rc::new(Vec::new()),
            rows_gen: 0,
            cursor: 0,
            range_anchor: None,
            compose: None,
            compose_gen: 0,
            pending_delete: None,
            refreshing: false,
            req: 0,
            model_gen: 0,
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

    /// Drop the derived state (bind / unbind).
    pub(crate) fn clear_derived(&mut self) {
        self.model = None;
        self.review = None;
        self.review_path = None;
        self.review_gen = self.review_gen.wrapping_add(1);
        self.folds = Folds::default();
        self.cursor = 0;
        self.rebuild_rows();
    }

    /// Recompute `rows` from `(model, review, folds)` and clamp the cursor.
    /// Call at every mutation site of those inputs — never from render.
    pub(crate) fn rebuild_rows(&mut self) {
        let rows = match &self.model {
            Some(m) => visible_rows(m, self.review.as_ref(), &self.folds),
            None => Vec::new(),
        };
        self.rows = Rc::new(rows);
        self.rows_gen = self.rows_gen.wrapping_add(1);
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.range_anchor = None;
        self.pending_delete = None;
    }

    /// The row under the cursor.
    pub(crate) fn cursor_row(&self) -> Option<RowRef> {
        self.rows.get(self.cursor).copied()
    }

    /// The cursor's position in model terms (see [`CursorAnchor`]).
    pub(crate) fn cursor_anchor(&self) -> Option<CursorAnchor> {
        cursor_anchor(self.model.as_ref()?, &self.rows, self.cursor)
    }

    /// The commentable line under the cursor (spec B5; `None` on a header).
    #[allow(dead_code)] // unit-tested helper; the tile uses `comment_anchor_at_cursor`.
    pub(crate) fn cursor_line_anchor(&self) -> Option<LineAnchor> {
        line_anchor(self.model.as_ref()?, self.cursor_row()?)
    }

    /// `(viewed, total)` files (spec B2 `N/M files viewed`).
    pub(crate) fn progress(&self) -> (usize, usize) {
        match (&self.model, &self.review) {
            (Some(m), Some(r)) => r.viewed_count(m),
            (Some(m), None) => (0, m.files.len()),
            _ => (0, 0),
        }
    }

    /// `j`/`k`. With a `V` range active the cursor stays inside the range's
    /// file (never onto its header or another file — spec B5).
    pub(crate) fn move_cursor(&mut self, delta: i32) {
        let next = step_row(&self.rows, self.cursor, delta);
        if let Some(a) = self.range_anchor {
            let file = self.rows.get(a).map(RowRef::file);
            let ok = self.rows.get(next).is_some_and(|r| Some(r.file()) == file && !r.is_file());
            if !ok {
                return;
            }
        }
        self.cursor = next;
    }

    /// The selected row span `(lo, hi)`: `range_anchor..=cursor` while a `V`
    /// range is active.
    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        let a = self.range_anchor?;
        Some((a.min(self.cursor), a.max(self.cursor)))
    }

    /// `V`: start a range at the cursor (a `Line` row only — `false`
    /// otherwise), or clear an active one.
    pub(crate) fn toggle_range(&mut self) -> bool {
        if self.range_anchor.take().is_some() {
            return true;
        }
        if self.cursor_row().is_some_and(|r| r.is_line()) {
            self.range_anchor = Some(self.cursor);
            return true;
        }
        false
    }

    /// Where `c` would anchor: the active range's line rows, else the
    /// cursor's line (spec B5). `None` when neither covers a diff line.
    pub(crate) fn comment_anchor_at_cursor(&self) -> Option<CommentAnchor> {
        let model = self.model.as_ref()?;
        let (lo, hi) = self.selection().unwrap_or((self.cursor, self.cursor));
        comment_anchor_for_range(model, &self.rows, lo, hi)
    }

    /// The stored comment whose card the cursor is on.
    pub(crate) fn cursor_comment(&self) -> Option<&ReviewComment> {
        let ci = self.cursor_row()?.comment_index()?;
        self.review.as_ref()?.comments.get(ci)
    }

    /// Open the comment editor (bumps `compose_gen`, clears the range).
    pub(crate) fn open_compose(&mut self, target: ComposeTarget, anchor: CommentAnchor, body: &str) {
        let mut input = Compose::new();
        for ch in body.chars() {
            input.editor.insert_char(ch);
        }
        self.compose = Some(CommentCompose {
            input,
            target,
            anchor,
            esc_armed: false,
        });
        self.range_anchor = None;
        self.pending_delete = None;
        self.compose_gen = self.compose_gen.wrapping_add(1);
    }

    /// Close the comment editor, discarding its draft.
    pub(crate) fn close_compose(&mut self) {
        if self.compose.take().is_some() {
            self.compose_gen = self.compose_gen.wrapping_add(1);
        }
    }

    /// Save the open compose into the review (spec B5): a `New` target adds
    /// an unsent comment, `Edit` replaces the body. Empty (whitespace-only)
    /// drafts are refused. Bumps `review_gen`, rebuilds rows (the card
    /// appears), closes the compose, and returns the saved comment id.
    /// Persisting is the caller's job (off the render path).
    pub(crate) fn save_compose(&mut self, now: chrono::DateTime<chrono::Utc>) -> Result<String, &'static str> {
        let model = self.model.clone().ok_or("no diff loaded")?;
        let compose = self.compose.as_ref().ok_or("no comment open")?;
        let body = compose.input.text().trim_end().to_string();
        if body.trim().is_empty() {
            return Err("Comment is empty — type something or Esc to cancel");
        }
        let (target, anchor) = (compose.target.clone(), compose.anchor.clone());
        let review = self
            .review
            .get_or_insert_with(|| Review::new(&model.branch, &model.base, &model.worktree));
        let id = match target {
            ComposeTarget::New => review.add_comment(anchor.path, anchor.side, anchor.lines, anchor.snippet, body, now),
            ComposeTarget::Edit(id) => {
                if !review.edit_comment(&id, body) {
                    return Err("That comment no longer exists");
                }
                id
            }
        };
        self.close_compose();
        self.review_gen = self.review_gen.wrapping_add(1);
        let cursor = self.cursor;
        self.rebuild_rows();
        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
        Ok(id)
    }

    /// `x` on a card (spec B5/C6 idiot-proof): the first press arms
    /// (`Armed(id)`), a second press on the same card deletes it.
    pub(crate) fn delete_at_cursor(&mut self) -> DeleteOutcome {
        let Some(id) = self.cursor_comment().map(|c| c.id.clone()) else {
            self.pending_delete = None;
            return DeleteOutcome::NotOnComment;
        };
        if self.pending_delete.as_deref() != Some(id.as_str()) {
            self.pending_delete = Some(id.clone());
            return DeleteOutcome::Armed(id);
        }
        self.pending_delete = None;
        if let Some(r) = self.review.as_mut() {
            r.delete_comment(&id);
        }
        self.review_gen = self.review_gen.wrapping_add(1);
        let cursor = self.cursor;
        self.rebuild_rows();
        self.cursor = host_row(&self.rows, cursor.min(self.rows.len().saturating_sub(1)));
        DeleteOutcome::Deleted(id)
    }

    /// Unsent comments in the loaded review (spec B2 header "K unsent").
    pub(crate) fn unsent_count(&self) -> usize {
        self.review.as_ref().map_or(0, |r| r.comments.iter().filter(|c| c.sent.is_empty()).count())
    }

    /// `}`/`{` — stays put when there is no further hunk.
    pub(crate) fn jump_hunk(&mut self, forward: bool) {
        if let Some(i) = next_hunk_row(&self.rows, self.cursor, forward) {
            self.cursor = i;
        }
    }

    /// `]`/`[` — stays put when there is no further file.
    pub(crate) fn jump_file(&mut self, forward: bool) {
        if let Some(i) = next_file_row(&self.rows, self.cursor, forward) {
            self.cursor = i;
        }
    }

    /// `G`.
    pub(crate) fn cursor_to_end(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    /// `z`: fold/unfold the cursor's file; folding parks the cursor on its
    /// header (the only row of that file left).
    pub(crate) fn toggle_fold_at_cursor(&mut self) {
        let Some(row) = self.cursor_row() else { return };
        let Some(model) = self.model.clone() else { return };
        let Some(file) = model.files.get(row.file()) else { return };
        let viewed = self.review.as_ref().is_some_and(|r| r.is_viewed(file));
        self.folds.toggle(&file.path, viewed);
        let header = file_header_row(&self.rows, row.file()).unwrap_or(0);
        self.rebuild_rows();
        self.cursor = header;
    }

    /// Put the cursor on row `index` (a click), clamped.
    pub(crate) fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.rows.len().saturating_sub(1));
    }

    /// `v` / checkbox: flip Viewed on the cursor's file (spec B4). Marking
    /// folds the file and advances the cursor to the next unviewed file's
    /// header (wrapping; stays on this header when none is left); unmarking
    /// re-expands it with the cursor on its header. Returns the new state, or
    /// `None` when there is no file under the cursor. Persistence is the
    /// caller's job (off the render path).
    pub(crate) fn toggle_viewed_at_cursor(&mut self) -> Option<bool> {
        let row = self.cursor_row()?;
        let model = self.model.clone()?;
        let file = model.files.get(row.file())?;
        let review = self
            .review
            .get_or_insert_with(|| Review::new(&model.branch, &model.base, &model.worktree));
        let now = review.toggle_viewed(file);
        self.review_gen = self.review_gen.wrapping_add(1);
        self.folds.reset(&file.path);
        self.rebuild_rows();
        let header = file_header_row(&self.rows, row.file()).unwrap_or(0);
        self.cursor = if now {
            next_unviewed_file_row(&self.rows, row.file()).unwrap_or(header)
        } else {
            header
        };
        Some(now)
    }
}

/// What an `x` press did (see [`DiffTile::delete_at_cursor`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeleteOutcome {
    NotOnComment,
    Armed(String),
    Deleted(String),
}

/// Build the `<abs-path>:<line>` argument `zed` takes on its command line
/// (spec B7 "Open in Zed"): the worktree root joined with the cursor file's
/// repo-relative path, suffixed with the cursor's new-file line ([`zed_target`]). Pure —
/// no filesystem access, no spawn — so it's unit-tested independent of the
/// actual `zed` launch (`diff_ui.rs::open_in_zed` is the only caller).
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

// ── Cog graph 8g7 node `file-viewed-ui`: the row model + pure nav ──────────

#[cfg(test)]
mod row_model_tests {
    use super::*;

    /// Two files: `a.rs` with two hunks (context/removed/added), `b.rs` with
    /// one all-added hunk.
    fn model() -> DiffModel {
        let raw = "\
diff --git a/a.rs b/a.rs
index 1..2 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
@@ -20,2 +20,3 @@ fn f()
 twenty
+inserted
 twentyone
diff --git a/b.rs b/b.rs
index 3..4 100644
--- a/b.rs
+++ b/b.rs
@@ -0,0 +1,2 @@
+x
+y
";
        parse_diff(raw, PathBuf::from("/wt"), "feature", "main", "deadbeef")
    }

    fn file_rows(rows: &[RowRef]) -> Vec<usize> {
        rows.iter().enumerate().filter(|(_, r)| r.is_file()).map(|(i, _)| i).collect()
    }

    #[test]
    fn visible_rows_lists_headers_hunks_and_numbered_lines() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        // a.rs: header + (hunk + 4) + (hunk + 3); b.rs: header + (hunk + 2).
        assert_eq!(rows.len(), 1 + 5 + 4 + 1 + 3);
        assert_eq!(file_rows(&rows), vec![0, 10]);
        assert_eq!(rows[1], RowRef::Hunk { file: 0, hunk: 0 });
        assert_eq!(
            rows[3],
            RowRef::Line { file: 0, hunk: 0, line: 1, old: Some(2), new: None },
            "a removed line has only an old number"
        );
        assert_eq!(
            rows[4],
            RowRef::Line { file: 0, hunk: 0, line: 2, old: None, new: Some(2) }
        );
        assert_eq!(
            rows[8],
            RowRef::Line { file: 0, hunk: 1, line: 1, old: None, new: Some(21) }
        );
    }

    #[test]
    fn viewed_file_folds_unless_expanded_and_z_overrides() {
        let m = model();
        let mut review = Review::new("feature", "main", std::path::Path::new("/wt"));
        review.set_viewed(&m.files[0], true);
        let mut folds = Folds::default();
        let rows = visible_rows(&m, Some(&review), &folds);
        assert_eq!(rows[0], RowRef::File { file: 0, viewed: true, collapsed: true });
        assert_eq!(rows[1], RowRef::File { file: 1, viewed: false, collapsed: false });
        // z on the viewed file expands it; z again folds it.
        folds.toggle(&m.files[0].path, true);
        assert_eq!(visible_rows(&m, Some(&review), &folds).len(), 14);
        folds.toggle(&m.files[0].path, true);
        assert_eq!(visible_rows(&m, Some(&review), &folds).len(), 1 + 1 + 3);
        // z on an unviewed file folds it.
        folds.toggle(&m.files[1].path, false);
        assert_eq!(visible_rows(&m, Some(&review), &folds).len(), 2);
        folds.reset(&m.files[1].path);
        assert!(!folds.is_collapsed(&m.files[1].path, false));
    }

    #[test]
    fn nav_steps_clamp_and_jumps_stop_at_ends() {
        let rows = visible_rows(&model(), None, &Folds::default());
        assert_eq!(step_row(&rows, 0, -1), 0);
        assert_eq!(step_row(&rows, 0, 1), 1);
        assert_eq!(step_row(&rows, 13, 1), 13);
        assert_eq!(step_row(&[], 5, 1), 0);
        // Hunks at 1, 6, 11.
        assert_eq!(next_hunk_row(&rows, 0, true), Some(1));
        assert_eq!(next_hunk_row(&rows, 1, true), Some(6));
        assert_eq!(next_hunk_row(&rows, 6, true), Some(11));
        assert_eq!(next_hunk_row(&rows, 11, true), None);
        assert_eq!(next_hunk_row(&rows, 8, false), Some(6));
        assert_eq!(next_hunk_row(&rows, 1, false), None);
        // Files at 0, 10.
        assert_eq!(next_file_row(&rows, 3, true), Some(10));
        assert_eq!(next_file_row(&rows, 10, true), None);
        assert_eq!(next_file_row(&rows, 12, false), Some(10));
        assert_eq!(next_file_row(&rows, 3, false), Some(0));
    }

    #[test]
    fn next_unviewed_wraps_and_is_none_when_all_viewed() {
        let m = model();
        let mut review = Review::new("feature", "main", std::path::Path::new("/wt"));
        review.set_viewed(&m.files[1], true);
        let rows = visible_rows(&m, Some(&review), &Folds::default());
        // b.rs (viewed) → wraps to a.rs.
        assert_eq!(next_unviewed_file_row(&rows, 1), Some(0));
        review.set_viewed(&m.files[0], true);
        let rows = visible_rows(&m, Some(&review), &Folds::default());
        assert_eq!(next_unviewed_file_row(&rows, 0), None);
        let rows = visible_rows(&m, None, &Folds::default());
        assert_eq!(next_unviewed_file_row(&rows, 0), Some(10));
    }

    /// UXI-Diff-13: a Line anchor re-resolves to the nearest new-side line in
    /// the same file after the rows shift; a vanished file falls back.
    #[test]
    fn anchor_survives_a_shift_and_falls_back_when_file_gone() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        let anchor = cursor_anchor(&m, &rows, 8).expect("anchor");
        assert_eq!(anchor.kind, AnchorKind::Line { old: None, new: Some(21) });
        // Fold nothing but add a leading file: indices shift, anchor holds.
        let raw2 = "\
diff --git a/0.rs b/0.rs
index 1..2 100644
--- a/0.rs
+++ b/0.rs
@@ -1 +1 @@
-p
+q
diff --git a/a.rs b/a.rs
index 1..2 100644
--- a/a.rs
+++ b/a.rs
@@ -20,2 +20,4 @@ fn f()
 twenty
+inserted
+more
 twentyone
";
        let m2 = parse_diff(raw2, PathBuf::from("/wt"), "feature", "main", "d");
        let rows2 = visible_rows(&m2, None, &Folds::default());
        let at = resolve_anchor(&m2, &rows2, &anchor, 0);
        assert_eq!(rows2[at], RowRef::Line { file: 1, hunk: 0, line: 1, old: None, new: Some(21) });
        // File gone ⇒ clamp fallback.
        let gone = CursorAnchor { path: PathBuf::from("zzz.rs"), kind: AnchorKind::File };
        assert_eq!(resolve_anchor(&m2, &rows2, &gone, 99), rows2.len() - 1);
        // Header anchor ⇒ header.
        let hdr = cursor_anchor(&m, &rows, 10).unwrap();
        assert_eq!(resolve_anchor(&m, &rows, &hdr, 0), 10);
    }

    #[test]
    fn line_anchor_and_zed_target_pick_the_right_side() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        let removed = line_anchor(&m, rows[3]).unwrap();
        assert_eq!((removed.side, removed.line, removed.text.as_str()), (CommentSide::Old, 2, "two"));
        let added = line_anchor(&m, rows[4]).unwrap();
        assert_eq!((added.side, added.line, added.text.as_str()), (CommentSide::New, 2, "TWO"));
        assert!(line_anchor(&m, rows[0]).is_none() && line_anchor(&m, rows[1]).is_none());
        assert_eq!(zed_target(&m, &rows, 4), Some((PathBuf::from("a.rs"), 2)));
        // Removed line "two" sits after new line 1 ⇒ 2.
        assert_eq!(zed_target(&m, &rows, 3), Some((PathBuf::from("a.rs"), 2)));
        assert_eq!(zed_target(&m, &rows, 6), Some((PathBuf::from("a.rs"), 20)));
        assert_eq!(zed_target(&m, &rows, 0), Some((PathBuf::from("a.rs"), 1)));
        assert_eq!(zed_target(&m, &rows, 10), Some((PathBuf::from("b.rs"), 1)));
    }

    fn comment(path: &str, side: CommentSide, lines: [usize; 2], snippet: &str, body: &str) -> ReviewComment {
        ReviewComment {
            id: "c1".into(),
            path: PathBuf::from(path),
            side,
            lines,
            snippet: snippet.into(),
            body: body.into(),
            ..ReviewComment::default()
        }
    }

    /// Cards follow their snippet (nearest match to the stored line), land
    /// after the header when outdated/unplaceable, and a range anchor picks
    /// the new side for a mixed span.
    #[test]
    fn comment_placement_anchor_and_card_lines() {
        let m = model();
        let mut review = Review::new("feature", "main", std::path::Path::new("/wt"));
        // "inserted" is new line 21 in a.rs's second hunk (rows[8]).
        review.comments.push(comment("a.rs", CommentSide::New, [21, 21], "inserted", "why"));
        // Stale line number: still placed by snippet.
        review.comments.push(comment("a.rs", CommentSide::New, [99, 99], "TWO", "drifted"));
        let mut outdated = comment("a.rs", CommentSide::New, [2, 2], "gone", "old");
        outdated.outdated = true;
        review.comments.push(outdated);
        let rows = visible_rows(&m, Some(&review), &Folds::default());
        let at = |ci: usize| rows.iter().position(|r| r.comment_index() == Some(ci)).unwrap();
        assert_eq!(at(2), 1, "outdated card right after the header");
        assert!(matches!(rows[at(1) - 1], RowRef::Line { new: Some(2), .. }), "drifted card after TWO");
        assert!(matches!(rows[at(0) - 1], RowRef::Line { new: Some(21), .. }), "card after inserted");
        assert!(matches!(rows[at(2) + 1], RowRef::Comment { part: 1, parts: 2, .. }), "outdated card shows its snippet");
        assert_eq!(host_row(&rows, at(0)), at(0) - 1);

        // Mixed removed/added span ⇒ new side, new-side lines only.
        let plain = visible_rows(&m, None, &Folds::default());
        let a = comment_anchor_for_range(&m, &plain, 2, 5).unwrap();
        assert_eq!((a.side, a.lines, a.snippet.as_str()), (CommentSide::New, [1, 3], "one\nTWO\nthree"));
        let removed = comment_anchor_for_range(&m, &plain, 3, 3).unwrap();
        assert_eq!((removed.side, removed.lines), (CommentSide::Old, [2, 2]));
        assert!(comment_anchor_for_range(&m, &plain, 0, 1).is_none(), "headers only");
        assert_eq!(a.label(), "a.rs:1–3");

        assert_eq!(wrap_cols("ab cd ef", 5), vec!["ab cd", "ef"]);
        assert_eq!(wrap_cols("abcdefg", 3), vec!["abc", "def", "g"]);
        assert_eq!(wrap_cols("a\n\nb", 10), vec!["a", "", "b"]);
        let long = comment("a.rs", CommentSide::New, [1, 1], "", &"x\n".repeat(20));
        let lines = comment_card_lines(&long);
        assert_eq!(lines.len(), COMMENT_MAX_BODY_ROWS);
        assert_eq!(lines.last(), Some(&CardLine::More));
    }

    /// `V` confines j/k to the range's file (never onto a header or the next
    /// file); `V` on a non-line row refuses.
    #[test]
    fn range_selection_is_clamped_to_one_file() {
        let mut t = DiffTile::bound_to(PathBuf::from("/wt"));
        t.model = Some(Rc::new(model()));
        t.rebuild_rows();
        t.cursor = 0;
        assert!(!t.toggle_range(), "V on a header refuses");
        t.cursor = 9; // a.rs's last line
        assert!(t.toggle_range());
        t.move_cursor(1);
        assert_eq!(t.cursor, 9, "can't extend into b.rs");
        for _ in 0..20 {
            t.move_cursor(-1);
        }
        assert_eq!(t.cursor, 1, "stops at a.rs's first hunk row, not its header");
        assert_eq!(t.selection(), Some((1, 9)));
        assert!(t.toggle_range() && t.selection().is_none(), "V again clears");
    }

    #[test]
    fn tile_toggle_viewed_advances_then_unmark_reexpands() {
        let mut t = DiffTile::bound_to(PathBuf::from("/wt"));
        t.model = Some(Rc::new(model()));
        t.rebuild_rows();
        t.cursor = 3;
        assert_eq!(t.toggle_viewed_at_cursor(), Some(true));
        assert_eq!(t.progress(), (1, 2));
        // a.rs folded to its header; cursor on b.rs's header (row 1).
        assert_eq!(t.rows.len(), 1 + 1 + 3);
        assert_eq!(t.cursor, 1);
        assert_eq!(t.toggle_viewed_at_cursor(), Some(true));
        assert_eq!(t.progress(), (2, 2));
        assert_eq!(t.cursor, 1, "no unviewed file left ⇒ stay on this header");
        t.cursor = 0;
        assert_eq!(t.toggle_viewed_at_cursor(), Some(false));
        assert_eq!(t.cursor, 0);
        assert_eq!(t.rows.len(), 1 + 9 + 1, "unmarking re-expands a.rs");
    }
}
