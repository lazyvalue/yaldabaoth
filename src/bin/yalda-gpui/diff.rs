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
    /// UXI-Diff-15). A card is `parts` consecutive rows forming ONE bordered
    /// box (`part` 0 = the header: id + status pills; then wrapped body lines;
    /// then a footer row — see [`comment_card_lines`]) so the body stays
    /// uniform-height and
    /// `DiffView::reveal_cursor` stays exact. `comment` indexes
    /// `Review::comments` at the time the rows were built (rows are rebuilt
    /// at every review mutation).
    Comment {
        file: usize,
        comment: usize,
        part: u8,
        parts: u8,
    },
    /// One fixed-height spacer row reserved for the inline comment compose
    /// (spec B5, UXI-Diff-15): while the compose is open, `parts` of these sit
    /// directly under the anchor's last line (after any existing cards there)
    /// — or replace the edited comment's card rows. The cached body paints
    /// them empty; the editor itself is an UNCACHED overlay the root paints
    /// over exactly these rows (`render_diff`, `yux::list_rows_overlay`), so
    /// typing never re-renders the body (UXI-Diff-12). `parts` grows with the
    /// draft's visual line count ([`compose_slot_rows`]) — only a line-count
    /// change rebuilds rows.
    ComposeSlot { file: usize, part: u8, parts: u8 },
    /// One unchanged line REVEALED by expanding context (spec B2a,
    /// UXI-Diff-18): not part of any hunk, its text comes from the file's
    /// new-side content ([`FileTexts`]). A normal context line otherwise —
    /// cursor-able, commentable, openable in Zed.
    Ctx { file: usize, old: u32, new: u32 },
    /// A context expander (spec B2a, UXI-Diff-18): the slim row standing for
    /// the still-hidden unchanged lines of gap `gap` of `file` (gap 0 = above
    /// the first hunk, `hunks.len()` = below the last, else between hunks
    /// `gap-1` and `gap`). `first_hidden` = the new-side number of its first
    /// hidden line; `hidden` = how many (`None` = unknown: below the last
    /// hunk before the file's text is loaded); `loading` = an expand is
    /// waiting on the file's text. One row tall like every row.
    Expander {
        file: usize,
        gap: u32,
        first_hidden: u32,
        hidden: Option<u32>,
        kind: GapKind,
        loading: bool,
    },
}

impl RowRef {
    /// The file index this row belongs to.
    pub(crate) fn file(&self) -> usize {
        match *self {
            RowRef::File { file, .. }
            | RowRef::Hunk { file, .. }
            | RowRef::Line { file, .. }
            | RowRef::Comment { file, .. }
            | RowRef::ComposeSlot { file, .. }
            | RowRef::Ctx { file, .. }
            | RowRef::Expander { file, .. } => file,
        }
    }

    /// `(old, new)` line numbers of a code row (a diff `Line` or a revealed
    /// `Ctx` line); `None` for every other row.
    pub(crate) fn line_numbers(&self) -> Option<(Option<u32>, Option<u32>)> {
        match *self {
            RowRef::Line { old, new, .. } => Some((old, new)),
            RowRef::Ctx { old, new, .. } => Some((Some(old), Some(new))),
            _ => None,
        }
    }


    /// A row INSERTED under a diff line (a comment-card row or a compose
    /// slot) rather than a row of the diff itself.
    pub(crate) fn is_inline_insert(&self) -> bool {
        matches!(self, RowRef::Comment { .. } | RowRef::ComposeSlot { .. })
    }

    pub(crate) fn is_compose_slot(&self) -> bool {
        matches!(self, RowRef::ComposeSlot { .. })
    }

    /// A code line — a diff `Line` or a revealed context `Ctx` line (both
    /// are cursor/comment/range targets).
    pub(crate) fn is_line(&self) -> bool {
        matches!(self, RowRef::Line { .. } | RowRef::Ctx { .. })
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
#[allow(dead_code)] // the no-expansion shorthand the unit tests use.
pub(crate) fn visible_rows(model: &DiffModel, review: Option<&Review>, folds: &Folds) -> Vec<RowRef> {
    visible_rows_ex(model, review, folds, &Expansions::default(), &FileTexts::default())
}

/// [`visible_rows`] with context expansion (spec B2a): each file's gaps of
/// unchanged lines (above the first hunk, between hunks, below the last) get
/// their revealed [`RowRef::Ctx`] lines and — while lines stay hidden — one
/// [`RowRef::Expander`] row, in file order: the lines revealed downward from
/// the gap's top edge, the expander, the lines revealed upward from its
/// bottom edge, then the next hunk's header — dropped once its gap is fully
/// revealed, so the two hunks read as one. Revealed lines need the file's
/// new-side text in `texts`; until it is loaded the gap renders unexpanded.
pub(crate) fn visible_rows_ex(
    model: &DiffModel,
    review: Option<&Review>,
    folds: &Folds,
    expansions: &Expansions,
    texts: &FileTexts,
) -> Vec<RowRef> {
    // E3: comment indices grouped by path ONCE (not re-filtered per file), in
    // `Review::comments` order.
    let mut by_path: HashMap<&std::path::Path, Vec<usize>> = HashMap::new();
    if let Some(r) = review {
        for (ci, c) in r.comments.iter().enumerate() {
            #[cfg(test)]
            row_build_counters::bump_comment_visit();
            by_path.entry(c.path.as_path()).or_default().push(ci);
        }
    }
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
        let content = texts.content(&f.path, CommentSide::New);
        let gaps = file_gaps(f, content.map(|c| c.line_count()));
        let fexp = expansions.by_path.get(&f.path).filter(|e| e.file_hash == f.file_hash);
        let loading = fexp.is_some_and(|e| !e.pending.is_empty());
        // Push gap `g`'s rows; `true` when the gap is non-empty and fully
        // revealed (the following hunk header is then dropped).
        let push_gap = |body: &mut Vec<RowRef>, g: usize| -> bool {
            let Some(gap) = gaps.get(g) else {
                return false;
            };
            let reveal = match (content, fexp) {
                (Some(_), Some(e)) => e.gaps.get(&g).copied().unwrap_or_default(),
                _ => GapReveal::default(),
            };
            let (top, bottom, hidden) = gap.effective(reveal);
            let ctx = |n: u32| RowRef::Ctx {
                file: fi,
                old: (n as i64 + gap.old_delta).max(0) as u32,
                new: n,
            };
            body.extend((gap.lo..gap.lo + top).map(ctx));
            if hidden != Some(0) {
                body.push(RowRef::Expander {
                    file: fi,
                    gap: g as u32,
                    first_hidden: gap.lo + top,
                    hidden,
                    kind: gap.kind,
                    loading,
                });
            }
            if let Some(hi) = gap.hi {
                body.extend((hi + 1 - bottom..=hi).map(ctx));
            }
            hidden == Some(0) && gap.len().is_some_and(|l| l > 0)
        };
        let mut body = Vec::new();
        for (hi, h) in f.hunks.iter().enumerate() {
            let merged = push_gap(&mut body, hi);
            if !merged {
                body.push(RowRef::Hunk { file: fi, hunk: hi });
            }
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
        push_gap(&mut body, f.hunks.len());
        // Place this file's comments: `top` right after the header, `after[k]`
        // right after body row k.
        let mut top: Vec<usize> = Vec::new();
        let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
        if let (Some(r), Some(cis)) = (review, by_path.get(f.path.as_path())) {
            // E3: each side's `(row, number, text)` vector is built at most
            // once per file, not once per comment.
            let mut new_side: Option<Vec<SideLine<'_>>> = None;
            let mut old_side: Option<Vec<SideLine<'_>>> = None;
            for &ci in cis {
                let c = &r.comments[ci];
                let placed = if c.outdated {
                    None
                } else {
                    let slot = match c.side {
                        CommentSide::New => &mut new_side,
                        CommentSide::Old => &mut old_side,
                    };
                    let side = slot.get_or_insert_with(|| side_lines(f, texts, &body, c.side));
                    place_in_side(side, c)
                };
                match placed {
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

/// The text of code row `row` in `file`: a diff `Line`'s hunk text, or a
/// revealed `Ctx` line's text from the file's new-side content.
fn line_text<'a>(file: &'a FileDiff, texts: &'a FileTexts, row: RowRef) -> Option<&'a str> {
    match row {
        RowRef::Line { hunk, line, .. } => match file.hunks.get(hunk)?.lines.get(line)? {
            DiffLine::Added(t) | DiffLine::Removed(t) | DiffLine::Context(t) => Some(t.as_str()),
        },
        RowRef::Ctx { new, .. } => texts.line(&file.path, CommentSide::New, new),
        _ => None,
    }
}

/// `row`'s line number on `side` (`None` when the line isn't on that side).
fn side_number(row: RowRef, side: CommentSide) -> Option<u32> {
    let (old, new) = row.line_numbers()?;
    match side {
        CommentSide::New => new,
        CommentSide::Old => old,
    }
}

// ── Context expansion (spec B2a, UXI-Diff-18; graph kfa node context-expand) ─

/// Lines one `↑`/`↓` expand reveals …
pub(crate) const EXPAND_STEP: u32 = 20;
/// … unless fewer than this many are hidden, when any expand shows them all.
pub(crate) const EXPAND_ALL_UNDER: u32 = 30;
/// git's default unified-diff context. A last hunk ending in fewer trailing
/// context lines reaches end-of-file, so it gets no bottom expander before
/// the file's text is known.
pub(crate) const DIFF_CONTEXT_LINES: usize = 3;
/// Context lines revealed around a comment that anchors in a hidden gap.
const COMMENT_REVEAL_CONTEXT: u32 = 3;

/// One side of a file's full text, split into lines once (1-based lookup).
#[derive(Debug)]
pub(crate) struct FileContent {
    text: Arc<str>,
    /// Byte offset of each line's start.
    starts: Vec<usize>,
}

impl FileContent {
    pub(crate) fn new(text: Arc<str>) -> Self {
        let mut starts = Vec::new();
        if !text.is_empty() {
            starts.push(0);
            let len = text.len();
            starts.extend(text.bytes().enumerate().filter(|&(i, b)| b == b'\n' && i + 1 < len).map(|(i, _)| i + 1));
        }
        FileContent { text, starts }
    }

    /// Number of lines (a trailing newline does not start another line).
    pub(crate) fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// Line `n` (1-based) without its line terminator.
    pub(crate) fn line(&self, n: u32) -> Option<&str> {
        let i = (n as usize).checked_sub(1)?;
        let start = *self.starts.get(i)?;
        let end = self.starts.get(i + 1).copied().unwrap_or(self.text.len());
        let l = &self.text[start..end];
        let l = l.strip_suffix('\n').unwrap_or(l);
        Some(l.strip_suffix('\r').unwrap_or(l))
    }

    pub(crate) fn text(&self) -> &Arc<str> {
        &self.text
    }
}

/// A cache slot's state.
#[derive(Clone, Debug)]
pub(crate) enum TextSlot {
    Loading,
    Ready(Arc<FileContent>),
    /// The read failed (the caller surfaced why); a later request retries.
    Failed,
}

/// The Diff tile's per-file full-text cache (spec B2a): `(path, side)` →
/// the text, stamped with the `file_hash` it was read under. Filled async
/// on first need (the context expander; the syntax highlighter reuses it)
/// and by the derive (files whose comments anchor outside the hunks);
/// [`invalidate`](Self::invalidate)d on every derive — a slot whose file's
/// hash changed (or every slot, when the merge-base moved) is dropped.
///
/// API: [`file_text`](Self::file_text) (whole text) /
/// [`content`](Self::content) (line-indexed) / [`line`](Self::line); a
/// missing entry is requested with `YaldaGpuiView::diff_load_file_text`.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileTexts {
    slots: HashMap<(PathBuf, CommentSide), (u64, TextSlot)>,
    merge_base: String,
}

impl FileTexts {
    /// The loaded content of `path`'s `side`, if ready.
    pub(crate) fn content(&self, path: &std::path::Path, side: CommentSide) -> Option<&Arc<FileContent>> {
        match self.slots.get(&(path.to_path_buf(), side)) {
            Some((_, TextSlot::Ready(c))) => Some(c),
            _ => None,
        }
    }

    /// The whole text of `path`'s `side`, if loaded.
    pub(crate) fn file_text(&self, path: &std::path::Path, side: CommentSide) -> Option<Arc<str>> {
        self.content(path, side).map(|c| c.text().clone())
    }

    /// Line `n` (1-based) of `path`'s `side`, if loaded.
    pub(crate) fn line(&self, path: &std::path::Path, side: CommentSide, n: u32) -> Option<&str> {
        self.content(path, side)?.line(n)
    }

    pub(crate) fn is_loading(&self, path: &std::path::Path, side: CommentSide) -> bool {
        matches!(self.slots.get(&(path.to_path_buf(), side)), Some((_, TextSlot::Loading)))
    }

    /// The last read of `path`'s `side` failed.
    pub(crate) fn is_failed(&self, path: &std::path::Path, side: CommentSide) -> bool {
        matches!(self.slots.get(&(path.to_path_buf(), side)), Some((_, TextSlot::Failed)))
    }

    /// Start a load of `path`'s `side` at `hash`: `false` when one is already
    /// loading or loaded (a failed slot may retry).
    pub(crate) fn begin_load(&mut self, path: &std::path::Path, side: CommentSide, hash: u64) -> bool {
        let key = (path.to_path_buf(), side);
        if let Some((h, slot)) = self.slots.get(&key)
            && *h == hash
            && !matches!(slot, TextSlot::Failed)
        {
            return false;
        }
        self.slots.insert(key, (hash, TextSlot::Loading));
        true
    }

    /// Land a load started by [`begin_load`](Self::begin_load); `false` (and
    /// nothing stored) when it is stale — the slot was invalidated or
    /// re-requested at another hash / merge-base meanwhile.
    pub(crate) fn finish_load(
        &mut self,
        path: &std::path::Path,
        side: CommentSide,
        hash: u64,
        merge_base: &str,
        result: Result<Arc<str>, String>,
    ) -> bool {
        if merge_base != self.merge_base {
            return false;
        }
        let key = (path.to_path_buf(), side);
        match self.slots.get(&key) {
            Some((h, TextSlot::Loading)) if *h == hash => {}
            _ => return false,
        }
        let slot = match result {
            Ok(t) => TextSlot::Ready(Arc::new(FileContent::new(t))),
            Err(_) => TextSlot::Failed,
        };
        self.slots.insert(key, (hash, slot));
        true
    }

    /// Store an already-read text (the derive's reads).
    pub(crate) fn insert_ready(&mut self, path: &std::path::Path, side: CommentSide, hash: u64, text: Arc<str>) {
        self.slots
            .insert((path.to_path_buf(), side), (hash, TextSlot::Ready(Arc::new(FileContent::new(text)))));
    }

    /// Reconcile against a freshly derived `model`: a moved merge-base drops
    /// everything; otherwise a slot survives only while its file is still in
    /// the diff with the SAME `file_hash`.
    pub(crate) fn invalidate(&mut self, model: &DiffModel) {
        if self.merge_base != model.merge_base {
            self.slots.clear();
            self.merge_base = model.merge_base.clone();
            return;
        }
        self.slots.retain(|(path, _), (hash, _)| {
            model.files.iter().any(|f| &f.path == path && f.file_hash == *hash)
        });
    }

}

/// Syntax highlighting (spec B2b, UXI-Diff-19) is skipped — the file renders
/// plain — above this many lines …
pub(crate) const HL_MAX_LINES: u32 = 20_000;
/// … or this many bytes …
pub(crate) const HL_MAX_BYTES: usize = 2 * 1024 * 1024;
/// … or when any one line is longer than this (minified bundles: syntect's
/// per-line regex cost is superlinear there).
pub(crate) const HL_MAX_LINE_BYTES: usize = 10_000;

/// One highlighted token: a byte range of its line and its foreground.
pub(crate) type HlSpan = (std::ops::Range<usize>, Hsla);

/// One side of one file, highlighted as a WHOLE (spec B2b): per-line spans
/// plus the content they were computed over, so a diff row whose text does
/// not match the file line (a stale / mismatched read) renders plain instead
/// of mis-colored.
#[derive(Debug)]
pub(crate) struct FileSpans {
    pub(crate) content: Arc<FileContent>,
    /// `lines[n - 1]` = line `n`'s non-default-colored spans.
    pub(crate) lines: Vec<Vec<HlSpan>>,
}

impl FileSpans {
    /// Line `n`'s (1-based) spans, iff the file's line `n` IS `text`.
    pub(crate) fn line(&self, n: u32, text: &str) -> Option<&[HlSpan]> {
        if self.content.line(n)? != text {
            return None;
        }
        self.lines.get(n.checked_sub(1)? as usize).map(Vec::as_slice)
    }
}

/// Highlight `content` as `path`'s language under the syntect theme `theme`
/// (background executor only — this is the whole-file syntect pass). `None`
/// ⇒ plain: unknown language, over a size cap, or a syntect failure.
pub(crate) fn highlight_file(path: &std::path::Path, theme: &str, content: Arc<FileContent>) -> Option<FileSpans> {
    let text = content.text();
    if content.line_count() > HL_MAX_LINES
        || text.len() > HL_MAX_BYTES
        || text.split('\n').any(|l| l.len() > HL_MAX_LINE_BYTES)
    {
        return None;
    }
    let syntax = yalda::highlight::syntax_for_path(path)?;
    let hl = yalda::highlight::Highlighter::with_syntect_theme(theme);
    let lines = hl
        .highlight_file_spans(syntax, text)?
        .into_iter()
        .map(|spans| spans.into_iter().map(|(r, c)| (r, nc(c))).collect())
        .collect();
    Some(FileSpans { content, lines })
}

/// A highlight cache slot's state.
#[derive(Debug)]
pub(crate) enum HlSlot {
    /// The syntect pass is running on the background executor.
    Pending,
    Ready(Arc<FileSpans>),
    /// Unknown language / over a cap / read failed ⇒ plain, don't retry.
    Plain,
}

/// The Diff tile's syntax-highlight cache (spec B2b): `(path, side)` → the
/// spans, stamped with the `file_hash` + syntect theme they were computed
/// under — i.e. keyed by (path, side, file_hash, theme). A derive drops slots
/// whose hash changed ([`invalidate`](Self::invalidate)); a theme switch
/// drops everything ([`set_theme`](Self::set_theme)).
#[derive(Debug, Default)]
pub(crate) struct Highlights {
    slots: HashMap<(PathBuf, CommentSide), (u64, HlSlot)>,
    theme: &'static str,
    merge_base: String,
    /// A background `warm_syntax_set` for this tile is in flight.
    pub(crate) warming: bool,
}

impl Highlights {
    /// The ready spans of `path`'s `side`.
    pub(crate) fn spans(&self, path: &std::path::Path, side: CommentSide) -> Option<&Arc<FileSpans>> {
        match self.slots.get(&(path.to_path_buf(), side)) {
            Some((_, HlSlot::Ready(s))) => Some(s),
            _ => None,
        }
    }

    /// The syntect theme the cache is keyed on.
    pub(crate) fn theme(&self) -> &'static str {
        self.theme
    }

    /// Switch the keyed theme: `true` (and every slot dropped) when it moved.
    pub(crate) fn set_theme(&mut self, theme: &'static str) -> bool {
        if self.theme == theme {
            return false;
        }
        self.theme = theme;
        self.slots.clear();
        true
    }

    /// Whether `path`'s `side` at `hash` still needs a highlight pass.
    pub(crate) fn wants(&self, path: &std::path::Path, side: CommentSide, hash: u64) -> bool {
        !matches!(self.slots.get(&(path.to_path_buf(), side)), Some((h, _)) if *h == hash)
    }

    /// Mark a pass as started (or resolved-plain without one).
    pub(crate) fn begin(&mut self, path: &std::path::Path, side: CommentSide, hash: u64, slot: HlSlot) {
        self.slots.insert((path.to_path_buf(), side), (hash, slot));
    }

    /// Land a pass: `false` (nothing stored) when stale — the slot was
    /// invalidated / re-keyed, or the theme moved, meanwhile.
    pub(crate) fn finish(
        &mut self,
        path: &std::path::Path,
        side: CommentSide,
        hash: u64,
        theme: &str,
        spans: Option<FileSpans>,
    ) -> bool {
        if theme != self.theme {
            return false;
        }
        let key = (path.to_path_buf(), side);
        match self.slots.get(&key) {
            Some((h, HlSlot::Pending)) if *h == hash => {}
            _ => return false,
        }
        let slot = spans.map_or(HlSlot::Plain, |s| HlSlot::Ready(Arc::new(s)));
        self.slots.insert(key, (hash, slot));
        true
    }

    /// Reconcile against a fresh derive: a moved merge-base (the old side's
    /// text) drops everything; otherwise a slot survives only while its file
    /// is still in the diff with the SAME `file_hash`.
    pub(crate) fn invalidate(&mut self, model: &DiffModel) {
        if self.merge_base != model.merge_base {
            self.slots.clear();
            self.merge_base = model.merge_base.clone();
            return;
        }
        self.slots.retain(|(path, _), (hash, _)| {
            model.files.iter().any(|f| &f.path == path && f.file_hash == *hash)
        });
    }
}

/// The sides of `f` that have text to highlight: new unless deleted, old
/// unless added; nothing for a hunk-less (binary / mode-only) file or an
/// unknown language.
pub(crate) fn highlight_sides(f: &FileDiff) -> Vec<CommentSide> {
    if f.hunks.is_empty() || yalda::highlight::syntax_for_path(&f.path).is_none() {
        return Vec::new();
    }
    let mut sides = Vec::new();
    if f.status != FileStatus::Deleted {
        sides.push(CommentSide::New);
    }
    if f.status != FileStatus::Added {
        sides.push(CommentSide::Old);
    }
    sides
}

/// Where a gap sits in its file (spec B2a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GapKind {
    /// Above the first hunk.
    Top,
    /// Between two hunks.
    Between,
    /// Below the last hunk.
    Bottom,
}

/// Which lines an expand reveals: `Down` continues downward from the gap's
/// TOP edge (the lines right below the previous hunk), `Up` continues upward
/// from its BOTTOM edge (right above the next hunk), `All` the whole gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ExpandDir {
    Down,
    Up,
    All,
}

impl ExpandDir {
    pub(crate) fn slug(self) -> &'static str {
        match self {
            ExpandDir::Down => "down",
            ExpandDir::Up => "up",
            ExpandDir::All => "all",
        }
    }
}

/// A run of unchanged new-side lines no hunk shows: `lo..=hi` (`hi: None` =
/// the file's end is not known yet — only below the last hunk), with
/// `old = new + old_delta` throughout (the offset right after the preceding
/// hunk / before the first one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) lo: u32,
    pub(crate) hi: Option<u32>,
    pub(crate) old_delta: i64,
    pub(crate) kind: GapKind,
}

/// How much of one gap is revealed: `top` lines down from its top edge and
/// `bottom` lines up from its bottom edge (requested counts; clamped to the
/// gap's length by [`Gap::effective`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GapReveal {
    pub(crate) top: u32,
    pub(crate) bottom: u32,
}

impl Gap {
    /// Line count (`None` = unknown end).
    pub(crate) fn len(&self) -> Option<u32> {
        self.hi.map(|hi| (hi + 1).saturating_sub(self.lo))
    }

    /// `(top, bottom, hidden)` actually revealed under `r`, clamped so the two
    /// never overlap. An unknown-length gap reveals nothing yet.
    pub(crate) fn effective(&self, r: GapReveal) -> (u32, u32, Option<u32>) {
        match self.len() {
            Some(len) => {
                let top = r.top.min(len);
                let bottom = r.bottom.min(len - top);
                (top, bottom, Some(len - top - bottom))
            }
            None => (0, 0, None),
        }
    }
}

/// Can `f` show unchanged context at all? (A deleted file's single hunk IS
/// the whole file; a hunk-less rename/binary change has no lines.)
pub(crate) fn expandable(f: &FileDiff) -> bool {
    !f.hunks.is_empty() && f.status != FileStatus::Deleted
}

/// `f`'s gaps (`hunks.len() + 1` of them, some empty; none when not
/// [`expandable`]), on the new side. `total` = the new-side line count when
/// the text is loaded; without it the bottom gap's end is unknown unless the
/// last hunk visibly reaches end-of-file (fewer than [`DIFF_CONTEXT_LINES`]
/// trailing context lines).
pub(crate) fn file_gaps(f: &FileDiff, total: Option<u32>) -> Vec<Gap> {
    if !expandable(f) {
        return Vec::new();
    }
    let n = f.hunks.len();
    (0..=n)
        .map(|g| {
            let (old_at, new_at) = if g == 0 {
                let (o, nw) = f.hunks[0].first_lines();
                (o, nw.max(1))
            } else {
                f.hunks[g - 1].next_lines()
            };
            let lo = if g == 0 { 1 } else { new_at as u32 };
            let hi = if g < n {
                Some((f.hunks[g].first_lines().1 as u32).saturating_sub(1))
            } else if let Some(t) = total {
                Some(t)
            } else if f.hunks[n - 1].trailing_context() < DIFF_CONTEXT_LINES {
                Some(lo - 1)
            } else {
                None
            };
            let kind = match g {
                0 => GapKind::Top,
                _ if g == n => GapKind::Bottom,
                _ => GapKind::Between,
            };
            Gap {
                lo,
                hi,
                old_delta: old_at as i64 - new_at as i64,
                kind,
            }
        })
        .collect()
}

/// Apply one expand to `r` (spec B2a): `Down`/`Up` reveal [`EXPAND_STEP`]
/// more lines from that edge — or the whole remainder when fewer than
/// [`EXPAND_ALL_UNDER`] are hidden — and `All` the whole gap. `false` when
/// nothing changed (fully revealed, or the gap's end is still unknown).
pub(crate) fn apply_expand(gap: &Gap, r: &mut GapReveal, dir: ExpandDir) -> bool {
    let (top, bottom, Some(hidden)) = gap.effective(*r) else {
        return false;
    };
    if hidden == 0 {
        return false;
    }
    let step = if hidden < EXPAND_ALL_UNDER { hidden } else { EXPAND_STEP.min(hidden) };
    *r = match dir {
        ExpandDir::Down => GapReveal { top: top + step, bottom },
        ExpandDir::Up => GapReveal { top, bottom: bottom + step },
        ExpandDir::All => GapReveal { top: top + hidden, bottom },
    };
    true
}

/// Enter on an expander (spec B2a): toward the hunk for the top/bottom gaps;
/// between hunks, everything when under [`EXPAND_ALL_UNDER`] lines hide,
/// else from the edge the cursor is travelling away from (moving down ⇒
/// continue below the hunk above).
pub(crate) fn default_expand_dir(kind: GapKind, hidden: Option<u32>, moving_down: bool) -> ExpandDir {
    match kind {
        GapKind::Top => ExpandDir::Up,
        GapKind::Bottom => ExpandDir::Down,
        GapKind::Between if hidden.is_some_and(|h| h < EXPAND_ALL_UNDER) => ExpandDir::All,
        GapKind::Between if moving_down => ExpandDir::Down,
        GapKind::Between => ExpandDir::Up,
    }
}

/// One file's expansion state: per gap index its [`GapReveal`], plus the
/// expands queued while its text loads. Valid only for `file_hash`.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileExpansion {
    pub(crate) file_hash: u64,
    pub(crate) gaps: HashMap<usize, GapReveal>,
    pub(crate) pending: Vec<(usize, ExpandDir)>,
}

/// Every file's context expansion, by path. Survives a re-derive for a file
/// whose `file_hash` is unchanged ([`retain_for`](Self::retain_for)).
#[derive(Clone, Debug, Default)]
pub(crate) struct Expansions {
    pub(crate) by_path: HashMap<PathBuf, FileExpansion>,
}

impl Expansions {
    /// Keep only entries whose file is still in `model` with the same hash.
    pub(crate) fn retain_for(&mut self, model: &DiffModel) {
        self.by_path
            .retain(|path, e| model.files.iter().any(|f| &f.path == path && f.file_hash == e.file_hash));
    }

    /// The (hash-checked) entry for `f`, created empty.
    pub(crate) fn entry(&mut self, f: &FileDiff) -> &mut FileExpansion {
        let e = self.by_path.entry(f.path.clone()).or_default();
        if e.file_hash != f.file_hash {
            *e = FileExpansion {
                file_hash: f.file_hash,
                ..FileExpansion::default()
            };
        }
        e
    }
}

/// The 1-based `(first, last)` line span of `text` where `snippet` occurs,
/// choosing the occurrence whose last line is nearest `target` (trailing
/// whitespace ignored, like comment placement). An empty snippet is
/// `target` itself when in range.
fn locate_snippet(text: &FileContent, snippet: &str, target: u32) -> Option<(u32, u32)> {
    let total = text.line_count();
    if snippet.is_empty() {
        return (1..=total).contains(&target).then_some((target, target));
    }
    let mut needle: Vec<&str> = snippet.split('\n').map(str::trim_end).collect();
    if needle.len() > 1 && needle.last() == Some(&"") {
        needle.pop();
    }
    let k = needle.len() as u32;
    if k == 0 || k > total {
        return None;
    }
    (1..=total + 1 - k)
        .filter(|&s| (0..k).all(|i| text.line(s + i).map(str::trim_end) == Some(needle[i as usize])))
        .map(|s| (s, s + k - 1))
        .min_by_key(|&(_, e)| e.abs_diff(target))
}

/// Test-only counters pinning E3's complexity (comment visits, side-vector
/// builds). Thread-local so parallel tests don't race.
#[cfg(test)]
pub(crate) mod row_build_counters {
    use std::cell::Cell;
    thread_local! {
        static COMMENT_VISITS: Cell<usize> = const { Cell::new(0) };
        static SIDE_BUILDS: Cell<usize> = const { Cell::new(0) };
    }
    pub(crate) fn bump_comment_visit() {
        COMMENT_VISITS.with(|c| c.set(c.get() + 1));
    }
    pub(crate) fn bump_side_build() {
        SIDE_BUILDS.with(|c| c.set(c.get() + 1));
    }
    /// `(comment visits, side-vector builds)` since the last reset.
    pub(crate) fn take() -> (usize, usize) {
        (COMMENT_VISITS.with(|c| c.replace(0)), SIDE_BUILDS.with(|c| c.replace(0)))
    }
}

/// Where comment `c`'s card goes among one file's `body` rows (hunk + line
/// rows, no cards): the index of the LAST line of the run of `c.side` lines
/// matching `c.snippet` (trailing whitespace ignored, the same rule as
/// `Review::recompute_outdated`), choosing the match whose last line number
/// is nearest `c.lines[1]` so a comment follows its code when lines drift.
/// An empty snippet falls back to the exact line number. `None` ⇒ unplaceable.
pub(crate) fn place_comment(file: &FileDiff, texts: &FileTexts, body: &[RowRef], c: &ReviewComment) -> Option<usize> {
    place_in_side(&side_lines(file, texts, body, c.side), c)
}

/// One `side` line of a file's body: `(body index, line number, trimmed text)`.
type SideLine<'a> = (usize, u32, &'a str);

/// The `side` lines of one file's `body` rows, in order (trailing whitespace
/// trimmed) — what comment placement matches snippets against.
fn side_lines<'a>(file: &'a FileDiff, texts: &'a FileTexts, body: &[RowRef], side: CommentSide) -> Vec<SideLine<'a>> {
    #[cfg(test)]
    row_build_counters::bump_side_build();
    body.iter()
        .enumerate()
        .filter_map(|(k, r)| Some((k, side_number(*r, side)?, line_text(file, texts, *r)?.trim_end())))
        .collect()
}

/// [`place_comment`] against a prebuilt `c.side` vector ([`side_lines`]).
fn place_in_side(side: &[SideLine<'_>], c: &ReviewComment) -> Option<usize> {
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

/// One row of a comment card (spec B5, UXI-Diff-15). A card is a single
/// visually-boxed block spread over fixed-height rows: a `Header` row (the
/// box's top edge: id + status pills), the body rows, then a `Footer` row
/// (bottom padding + the box's bottom edge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CardLine {
    Header,
    Body(String),
    /// The anchored code, shown dimmed on an outdated card.
    Snippet(String),
    /// "…" — more body / snippet than fits.
    More,
    Footer,
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

#[cfg(test)]
thread_local! {
    /// Test-only count of [`comment_card_lines`] calls — the E2 guard that a
    /// cursor move re-wraps no card body (`card_lines_calls*`).
    static CARD_LINES_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn card_lines_calls() -> usize {
    CARD_LINES_CALLS.with(|n| n.get())
}

#[cfg(test)]
pub(crate) fn card_lines_calls_reset() {
    CARD_LINES_CALLS.with(|n| n.set(0));
}

/// The rows of `c`'s card (see [`CardLine`]): the header, the wrapped body
/// capped at [`COMMENT_MAX_BODY_ROWS`] (last shown row becomes "…" when cut),
/// then — for an outdated comment — its snippet capped at
/// [`COMMENT_MAX_SNIPPET_ROWS`], then the footer.
pub(crate) fn comment_card_lines(c: &ReviewComment) -> Vec<CardLine> {
    #[cfg(test)]
    CARD_LINES_CALLS.with(|n| n.set(n.get() + 1));
    let mut body = wrap_cols(c.body.trim_end(), COMMENT_WRAP_COLS);
    let mut out: Vec<CardLine> = vec![CardLine::Header];
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
    out.push(CardLine::Footer);
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
pub(crate) fn comment_anchor_for_range(
    model: &DiffModel,
    texts: &FileTexts,
    rows: &[RowRef],
    lo: usize,
    hi: usize,
) -> Option<CommentAnchor> {
    let hi = hi.min(rows.len().checked_sub(1)?);
    let lines: Vec<LineAnchor> = rows.get(lo..=hi)?.iter().filter_map(|r| line_anchor(model, texts, *r)).collect();
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

/// The index of `rows[i]`'s "host" row: a comment-card / compose-slot row
/// resolves to the nearest preceding diff row (its anchor line, or the file
/// header for a top-of-file card); any other row is itself.
pub(crate) fn host_row(rows: &[RowRef], i: usize) -> usize {
    let mut j = i.min(rows.len().saturating_sub(1));
    while j > 0 && rows.get(j).is_some_and(RowRef::is_inline_insert) {
        j -= 1;
    }
    j
}

/// The inline comment editor of a Diff tile (spec B5, GitHub-style): it
/// opens AT the commented line — [`RowRef::ComposeSlot`] rows reserve its
/// height under the anchor's last line (or in place of the edited card) and
/// the root paints the editor over them. Holds the text input (the app's
/// shared [`Compose`] editor, dispatched through `dispatch_insert_core` like
/// every other compose), what saving does, and the anchor snapshot taken when
/// it opened (a background refresh can't move the ground the comment is
/// being written against).
pub(crate) struct CommentCompose {
    pub(crate) input: Compose,
    pub(crate) target: ComposeTarget,
    pub(crate) anchor: CommentAnchor,
    /// How many [`RowRef::ComposeSlot`] rows the draft currently reserves
    /// ([`compose_slot_rows`]); rows are rebuilt only when this changes.
    pub(crate) slot_rows: usize,
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

/// Mono columns a draft line hard-wraps at in the inline compose (the editor
/// is monospace, so a character count IS the visual width; same budget as a
/// card body).
pub(crate) const COMPOSE_WRAP_COLS: usize = COMMENT_WRAP_COLS;
/// Editor lines the inline compose always shows (an empty draft still gets a
/// comfortable box) …
pub(crate) const COMPOSE_MIN_LINES: usize = 3;
/// … and the most it grows to before scrolling its own window to the caret.
pub(crate) const COMPOSE_MAX_LINES: usize = 12;
/// Rows of compose chrome around the editor lines: the caption header, the
/// key-hint footer, and one row of vertical room for margins + borders.
pub(crate) const COMPOSE_CHROME_ROWS: usize = 3;

/// A draft laid out for the inline compose: its visual lines (each doc line
/// hard-wrapped at [`COMPOSE_WRAP_COLS`] chars; an empty line is one empty
/// visual line) and the caret as `(visual line, char column)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposeLines {
    pub(crate) lines: Vec<String>,
    pub(crate) caret: (usize, usize),
}

/// Lay `text` out for the inline compose (see [`ComposeLines`]); the caret is
/// the editor's `(line, col)` in chars.
pub(crate) fn compose_visual_lines(text: &str, caret_line: usize, caret_col: usize) -> ComposeLines {
    let mut lines = Vec::new();
    let mut caret = (0, 0);
    for (li, doc_line) in text.split('\n').enumerate() {
        let chars: Vec<char> = doc_line.chars().collect();
        let chunks: Vec<String> = if chars.is_empty() {
            vec![String::new()]
        } else {
            chars.chunks(COMPOSE_WRAP_COLS).map(|c| c.iter().collect()).collect()
        };
        if li == caret_line {
            let col = caret_col.min(chars.len());
            let k = (col / COMPOSE_WRAP_COLS).min(chunks.len() - 1);
            caret = (lines.len() + k, col - k * COMPOSE_WRAP_COLS);
        }
        lines.extend(chunks);
    }
    ComposeLines { lines, caret }
}

/// How many [`RowRef::ComposeSlot`] rows a draft of `visual_lines` reserves:
/// the editor lines (clamped to [`COMPOSE_MIN_LINES`]..=[`COMPOSE_MAX_LINES`])
/// plus [`COMPOSE_CHROME_ROWS`].
pub(crate) fn compose_slot_rows(visual_lines: usize) -> usize {
    visual_lines.clamp(COMPOSE_MIN_LINES, COMPOSE_MAX_LINES) + COMPOSE_CHROME_ROWS
}

/// The first visual line the compose's editor window shows so the caret line
/// is visible (caret kept on the last shown line once the draft outgrows
/// [`COMPOSE_MAX_LINES`]).
pub(crate) fn compose_window_top(caret_line: usize, line_count: usize) -> usize {
    let visible = line_count.clamp(COMPOSE_MIN_LINES, COMPOSE_MAX_LINES);
    (caret_line + 1).saturating_sub(visible).min(line_count.saturating_sub(visible))
}

impl CommentCompose {
    /// The draft laid out for the inline editor.
    pub(crate) fn visual_lines(&self) -> ComposeLines {
        let c = self.input.editor.cursor();
        compose_visual_lines(&self.input.text(), c.line, c.col)
    }

    /// The slot rows this draft needs right now.
    pub(crate) fn wanted_slot_rows(&self) -> usize {
        compose_slot_rows(compose_visual_lines(&self.input.text(), 0, 0).lines.len())
    }
}

/// Where the open compose's slot rows go among `rows` (built WITHOUT slots):
/// `(at, remove, file)` — splice the slots in at `at`, replacing `remove`
/// rows there. Editing a comment replaces its card rows in place; a new
/// comment goes directly under its anchor's last line, after any cards
/// already there (a thread); an unplaceable anchor falls back to under its
/// file header, else the end.
fn compose_slot_place(model: &DiffModel, review: Option<&Review>, rows: &[RowRef], c: &CommentCompose) -> (usize, usize, usize) {
    if let ComposeTarget::Edit(id) = &c.target
        && let Some(ci) = review.and_then(|r| r.comments.iter().position(|x| &x.id == id))
        && let Some(start) = rows.iter().position(|r| r.comment_index() == Some(ci))
    {
        let len = rows[start..].iter().take_while(|r| r.comment_index() == Some(ci)).count();
        return (start, len, rows[start].file());
    }
    let Some(fi) = model.files.iter().position(|f| f.path == c.anchor.path) else {
        return (rows.len(), 0, rows.last().map(RowRef::file).unwrap_or(0));
    };
    let target = c.anchor.lines[1] as u32;
    let after = rows
        .iter()
        .position(|r| r.file() == fi && r.is_line() && side_number(*r, c.anchor.side) == Some(target))
        .or_else(|| file_header_row(rows, fi));
    match after {
        Some(i) => {
            let at = i + 1 + rows[i + 1..].iter().take_while(|r| r.is_inline_insert()).count();
            (at, 0, fi)
        }
        None => (rows.len(), 0, fi),
    }
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
    /// On a context expander: the same gap's expander if it still exists,
    /// else the code row nearest the first line it hid (a fully revealed gap
    /// lands on its first revealed line).
    Expander { gap: u32, first_hidden: u32 },
}

/// The anchor of row `cursor` (None for an out-of-range cursor / no rows).
pub(crate) fn cursor_anchor(model: &DiffModel, rows: &[RowRef], cursor: usize) -> Option<CursorAnchor> {
    rows.get(cursor)?;
    let row = rows[host_row(rows, cursor)];
    let file = model.files.get(row.file())?;
    let kind = match row {
        RowRef::File { .. } | RowRef::Comment { .. } | RowRef::ComposeSlot { .. } => AnchorKind::File,
        RowRef::Hunk { hunk, .. } => AnchorKind::Hunk {
            new_start: file.hunks.get(hunk).map(|h| h.starts().1).unwrap_or(0),
        },
        RowRef::Line { old, new, .. } => AnchorKind::Line { old, new },
        RowRef::Ctx { old, new, .. } => AnchorKind::Line {
            old: Some(old),
            new: Some(new),
        },
        RowRef::Expander { gap, first_hidden, .. } => AnchorKind::Expander { gap, first_hidden },
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
        AnchorKind::Line { new: Some(n), .. } => nearest(&|r: &RowRef| r.line_numbers()?.1, n),
        AnchorKind::Line { old: Some(o), .. } => nearest(&|r: &RowRef| r.line_numbers()?.0, o),
        AnchorKind::Line { .. } => None,
        AnchorKind::Expander { gap, first_hidden } => body
            .clone()
            .find(|&i| matches!(rows[i], RowRef::Expander { gap: g, .. } if g == gap))
            .or_else(|| nearest(&|r: &RowRef| r.line_numbers()?.1, first_hidden)),
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
pub(crate) fn line_anchor(model: &DiffModel, texts: &FileTexts, row: RowRef) -> Option<LineAnchor> {
    if let RowRef::Ctx { file, new, .. } = row {
        let f = model.files.get(file)?;
        return Some(LineAnchor {
            path: f.path.clone(),
            side: CommentSide::New,
            line: new as usize,
            text: texts.line(&f.path, CommentSide::New, new)?.to_string(),
        });
    }
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
        RowRef::File { .. } | RowRef::Comment { .. } | RowRef::ComposeSlot { .. } => {
            f.hunks.first().map(|h| h.starts().1).unwrap_or(1)
        }
        RowRef::Hunk { hunk, .. } => hunk_start(hunk),
        RowRef::Ctx { new, .. } => new as usize,
        RowRef::Expander { first_hidden, .. } => first_hidden as usize,
        RowRef::Line { new: Some(n), .. } => n as usize,
        RowRef::Line { hunk, .. } => rows[..cursor]
            .iter()
            .rev()
            .filter(|r| !r.is_inline_insert())
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
    /// New-side file texts the derive read to keep comments on revealed
    /// context lines live (`(path, file_hash, text)`) — seeded into the
    /// tile's [`FileTexts`] cache (spec B2a).
    pub(crate) texts: Vec<(PathBuf, u64, Arc<str>)>,
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
/// worktree's diff. No session binding (ADR-0040).
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
    /// The open send picker (spec B6). Rendered at the SCREEN level over the
    /// tile (not in the cached `DiffView`), so it is deliberately NOT a
    /// `DiffSeqs` input — typing in its query never re-renders the body.
    pub(crate) send_picker: Option<SendPicker>,
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
    /// Per-file full-text cache (spec B2a) — see [`FileTexts`]; read through
    /// [`file_text`](Self::file_text).
    pub(crate) texts: FileTexts,
    /// Per-file context expansion (spec B2a) — an input of `rows`.
    pub(crate) expansions: Expansions,
    /// The cursor's last vertical travel was downward (`j`, `}`, `]`) —
    /// picks the default side Enter expands a between-hunks gap from.
    pub(crate) moving_down: bool,
    /// Syntax-highlight spans per (path, side) — spec B2b, [`Highlights`].
    pub(crate) highlights: Highlights,
    /// Bumped whenever a highlight pass lands (or the cache is dropped) —
    /// `DiffView`'s fingerprint for the painted token colors.
    pub(crate) hl_gen: u64,
}

/// What an expand request did (see [`DiffTile::expand_gap`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExpandOutcome {
    /// Applied now (the file's text was loaded).
    Applied,
    /// Queued: the caller must load this file's new-side text.
    NeedsText(PathBuf),
    /// Nothing to expand.
    Nothing,
}

/// Where a send-picker row delivers (spec B6): a session loaded in this GUI's
/// `AgentSessions` store, or a server-known session (universal roster) this GUI
/// has not attached — the latter is prompted by server sid directly, with no
/// attach, tile bind or focus change.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SendTarget {
    Local(SessionId),
    Server(String),
}

/// One send-picker candidate: the palette row (label/detail/status, matched by
/// the shared fuzzy ranker) plus the stable session key recorded in the
/// review's `sent` entries / `last_sent_session`.
pub(crate) struct SendCandidate {
    pub(crate) item: PaletteItem<SendTarget>,
    pub(crate) key: String,
}

/// The Diff tile's send picker (spec B6, UXI-Diff-16): a `cmd-p`-style fuzzy
/// session list over a snapshot of the candidates taken when it opened.
pub(crate) struct SendPicker {
    /// The comment ids this send names, fixed at open: the unsent ones (`s`)
    /// or every comment (`S`).
    pub(crate) ids: Vec<String>,
    pub(crate) items: Vec<PaletteItem<SendTarget>>,
    /// Parallel to `items`: each row's recorded session key.
    pub(crate) keys: Vec<String>,
    /// The item that matches the review's `last_sent_session` (tagged "last
    /// sent", and the initial selection).
    pub(crate) last_sent: Option<usize>,
    /// Private so every edit goes through [`query_key`](Self::query_key),
    /// which keeps [`ranked`](Self::ranked) in sync.
    query: LineInput,
    /// Item indices in display order for `query` — recomputed only when the
    /// query changes, not on every render / move / activate.
    ranked: Vec<usize>,
    /// DISPLAY index into [`ranked`](Self::ranked).
    pub(crate) selected: usize,
}

impl SendPicker {
    pub(crate) fn new(
        ids: Vec<String>,
        candidates: Vec<SendCandidate>,
        last_sent_session: Option<&str>,
    ) -> Self {
        let (items, keys): (Vec<_>, Vec<_>) = candidates.into_iter().map(|c| (c.item, c.key)).unzip();
        let last_sent = last_sent_session.and_then(|k| keys.iter().position(|x| x == k));
        let ranked = rank_palette_items(&items, "");
        SendPicker {
            ids,
            items,
            keys,
            last_sent,
            query: LineInput::new(),
            ranked,
            // Empty query ⇒ ranked is the identity, so the item index IS the
            // display index. No last-sent match ⇒ the first (most prominent)
            // row, i.e. the jump palette's order.
            selected: last_sent.unwrap_or(0),
        }
    }

    /// Item indices in display order for the current query (the jump
    /// palette's `rank_palette_items`).
    pub(crate) fn ranked(&self) -> &[usize] {
        &self.ranked
    }

    pub(crate) fn query(&self) -> &LineInput {
        &self.query
    }

    /// The highlighted item index (`None` when the query matches nothing).
    pub(crate) fn selected_item(&self) -> Option<usize> {
        self.ranked().get(self.selected).copied()
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let n = self.ranked().len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// Route an editing key to the query; an edit re-ranks and returns the
    /// highlight to the best match.
    pub(crate) fn query_key(&mut self, press: &KeyPress) -> LineEdit {
        let edit = self.query.handle(press);
        if edit.edited() {
            self.ranked = rank_palette_items(&self.items, self.query.text());
            self.selected = 0;
        }
        edit
    }
}

/// "1 comment" / "3 comments".
pub(crate) fn comments_phrase(n: usize) -> String {
    if n == 1 { "1 comment".to_string() } else { format!("{n} comments") }
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
            send_picker: None,
            refreshing: false,
            req: 0,
            model_gen: 0,
            error: None,
            needs_load: false,
            view: None,
            texts: FileTexts::default(),
            expansions: Expansions::default(),
            moving_down: true,
            highlights: Highlights::default(),
            hl_gen: 0,
        }
    }

    /// The full text of `path`'s `side` if cached (spec B2a) — `None` while
    /// unloaded; request it with `YaldaGpuiView::diff_load_file_text`. The
    /// context expander and the syntax highlighter share this cache.
    #[allow(dead_code)] // public cache API (syntax-highlight node).
    pub(crate) fn file_text(&self, path: &std::path::Path, side: CommentSide) -> Option<Arc<str>> {
        self.texts.file_text(path, side)
    }

    /// Rebuild rows keeping the cursor on the same thing (file + nearest
    /// line / the same expander) — used by expands, whose row inserts shift
    /// indices.
    pub(crate) fn rebuild_rows_keeping_cursor(&mut self) {
        let anchor = self.cursor_anchor();
        let old = self.cursor;
        self.rebuild_rows();
        if let (Some(a), Some(m)) = (anchor, self.model.clone()) {
            self.cursor = resolve_anchor(&m, &self.rows, &a, old);
        }
    }

    /// Expand gap `gap` of file `file` in `dir` (spec B2a). With the file's
    /// new-side text loaded the reveal applies now; otherwise it is queued
    /// (the expander shows "Loading…") and the caller loads the text, after
    /// which [`apply_pending_expansions`](Self::apply_pending_expansions)
    /// runs it. Rows are NOT rebuilt here.
    pub(crate) fn expand_gap(&mut self, file: usize, gap: usize, dir: ExpandDir) -> ExpandOutcome {
        let Some(model) = self.model.clone() else {
            return ExpandOutcome::Nothing;
        };
        let Some(f) = model.files.get(file).filter(|f| expandable(f)) else {
            return ExpandOutcome::Nothing;
        };
        if gap > f.hunks.len() {
            return ExpandOutcome::Nothing;
        }
        match self.texts.content(&f.path, CommentSide::New).map(|c| c.line_count()) {
            Some(total) => {
                let gaps = file_gaps(f, Some(total));
                let e = self.expansions.entry(f);
                if apply_expand(&gaps[gap], e.gaps.entry(gap).or_default(), dir) {
                    ExpandOutcome::Applied
                } else {
                    ExpandOutcome::Nothing
                }
            }
            None => {
                self.expansions.entry(f).pending.push((gap, dir));
                ExpandOutcome::NeedsText(f.path.clone())
            }
        }
    }

    /// The file's text just landed (or failed): run its queued expands.
    /// Returns whether anything was queued.
    pub(crate) fn apply_pending_expansions(&mut self, path: &std::path::Path) -> bool {
        let Some(model) = self.model.clone() else {
            return false;
        };
        let Some(f) = model.files.iter().find(|f| f.path == path) else {
            return false;
        };
        let total = self.texts.content(path, CommentSide::New).map(|c| c.line_count());
        let e = self.expansions.entry(f);
        let pending = std::mem::take(&mut e.pending);
        if let Some(total) = total {
            let gaps = file_gaps(f, Some(total));
            for (g, dir) in &pending {
                if let Some(gap) = gaps.get(*g) {
                    apply_expand(gap, e.gaps.entry(*g).or_default(), *dir);
                }
            }
        }
        !pending.is_empty()
    }

    /// The `(file, hunk)` the cursor is in or next to (for `+`): a diff line
    /// / hunk header's own hunk; a revealed line / expander → the hunk whose
    /// new-side span is nearest. `None` on a file header or a file without
    /// hunks.
    pub(crate) fn cursor_hunk(&self) -> Option<(usize, usize)> {
        let model = self.model.as_ref()?;
        let row = *self.rows.get(host_row(&self.rows, self.cursor))?;
        let f = model.files.get(row.file())?;
        let near = |n: u32| -> Option<usize> {
            f.hunks
                .iter()
                .enumerate()
                .min_by_key(|(_, h)| {
                    let (first, next) = (h.first_lines().1 as u32, h.next_lines().1 as u32);
                    if n < first { first - n } else { n.saturating_sub(next.saturating_sub(1)) }
                })
                .map(|(i, _)| i)
        };
        let hunk = match row {
            RowRef::Line { hunk, .. } | RowRef::Hunk { hunk, .. } => Some(hunk),
            RowRef::Ctx { new, .. } => near(new),
            RowRef::Expander { first_hidden, .. } => near(first_hidden),
            _ => None,
        }?;
        Some((row.file(), hunk))
    }

    /// Reveal the hidden lines comments anchor on (spec B2a): a live
    /// new-side comment whose snippet sits in a hidden part of a gap (its
    /// file's text loaded) gets that part revealed, plus
    /// [`COMMENT_REVEAL_CONTEXT`] lines, from the nearer gap edge — so a
    /// comment written on a revealed line keeps its place after a re-derive.
    pub(crate) fn reveal_comment_lines(&mut self) {
        let (Some(model), Some(review)) = (self.model.clone(), self.review.as_ref()) else {
            return;
        };
        let wants: Vec<(usize, u32, u32)> = review
            .comments
            .iter()
            .filter(|c| !c.outdated && c.side == CommentSide::New)
            .filter_map(|c| {
                let fi = model.files.iter().position(|f| f.path == c.path)?;
                let content = self.texts.content(&c.path, CommentSide::New)?;
                let (a, b) = locate_snippet(content, &c.snippet, c.lines[1] as u32)?;
                Some((fi, a, b))
            })
            .collect();
        for (fi, a, b) in wants {
            let f = &model.files[fi];
            let Some(total) = self.texts.content(&f.path, CommentSide::New).map(|c| c.line_count()) else {
                continue;
            };
            let gaps = file_gaps(f, Some(total));
            let e = self.expansions.entry(f);
            for (g, gap) in gaps.iter().enumerate() {
                let (Some(hi), Some(len)) = (gap.hi, gap.len()) else {
                    continue;
                };
                let (a, b) = (a.max(gap.lo), b.min(hi));
                if a > b {
                    continue;
                }
                let r = e.gaps.entry(g).or_default();
                let (top, bottom, _) = gap.effective(*r);
                if b < gap.lo + top || a > hi - bottom {
                    continue; // already revealed
                }
                if a - gap.lo <= hi - b {
                    let want = (b + COMMENT_REVEAL_CONTEXT + 1).saturating_sub(gap.lo).min(len);
                    *r = GapReveal { top: top.max(want), bottom };
                } else {
                    let want = (hi + 1).saturating_sub(a.saturating_sub(COMMENT_REVEAL_CONTEXT).max(gap.lo)).min(len);
                    *r = GapReveal { top, bottom: bottom.max(want) };
                }
            }
        }
    }

    /// Paths with a context expansion whose new-side text is not cached (a
    /// merge-base move dropped it) — the view re-requests them after a derive.
    pub(crate) fn expansions_missing_text(&self) -> Vec<PathBuf> {
        self.expansions
            .by_path
            .iter()
            .filter(|(p, e)| {
                (!e.gaps.is_empty() || !e.pending.is_empty()) && self.texts.content(p, CommentSide::New).is_none()
            })
            .map(|(p, _)| p.clone())
            .collect()
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
        self.texts = FileTexts::default();
        self.expansions = Expansions::default();
        self.highlights = Highlights {
            theme: self.highlights.theme,
            ..Highlights::default()
        };
        self.hl_gen = self.hl_gen.wrapping_add(1);
        self.cursor = 0;
        self.send_picker = None;
        self.rebuild_rows();
    }

    /// Recompute `rows` from `(model, review, folds)` and clamp the cursor.
    /// Call at every mutation site of those inputs — never from render.
    pub(crate) fn rebuild_rows(&mut self) {
        let rows = match &self.model {
            Some(m) => {
                let mut rows = visible_rows_ex(m, self.review.as_ref(), &self.folds, &self.expansions, &self.texts);
                if let Some(c) = &self.compose {
                    let (at, remove, file) = compose_slot_place(m, self.review.as_ref(), &rows, c);
                    let parts = c.slot_rows.clamp(1, u8::MAX as usize) as u8;
                    rows.splice(at..at + remove, (0..parts).map(|part| RowRef::ComposeSlot { file, part, parts }));
                }
                rows
            }
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
        line_anchor(self.model.as_ref()?, &self.texts, self.cursor_row()?)
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
        if delta != 0 {
            self.moving_down = delta > 0;
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
        comment_anchor_for_range(model, &self.texts, &self.rows, lo, hi)
    }

    /// The stored comment whose card the cursor is on.
    pub(crate) fn cursor_comment(&self) -> Option<&ReviewComment> {
        let ci = self.cursor_row()?.comment_index()?;
        self.review.as_ref()?.comments.get(ci)
    }

    /// Open the inline comment editor (bumps `compose_gen`, clears the
    /// range, and rebuilds rows so its [`RowRef::ComposeSlot`]s appear under
    /// the anchor). Editing parks the cursor on the card's first row first,
    /// which the slots then replace — so saving lands the cursor back on it.
    pub(crate) fn open_compose(&mut self, target: ComposeTarget, anchor: CommentAnchor, body: &str) {
        let input = Compose::seeded(body);
        if matches!(target, ComposeTarget::Edit(_))
            && let Some(ci) = self.cursor_row().and_then(|r| r.comment_index())
        {
            while self.cursor > 0 && self.rows.get(self.cursor - 1).and_then(RowRef::comment_index) == Some(ci) {
                self.cursor -= 1;
            }
        }
        let mut compose = CommentCompose {
            input,
            target,
            anchor,
            slot_rows: 0,
            esc_armed: false,
        };
        compose.slot_rows = compose.wanted_slot_rows();
        self.compose = Some(compose);
        self.compose_gen = self.compose_gen.wrapping_add(1);
        let cursor = self.cursor;
        self.rebuild_rows();
        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
    }

    /// Close the comment editor, discarding its draft (its slot rows go).
    pub(crate) fn close_compose(&mut self) {
        if self.compose.take().is_some() {
            self.compose_gen = self.compose_gen.wrapping_add(1);
            let cursor = self.cursor;
            self.rebuild_rows();
            self.cursor = cursor.min(self.rows.len().saturating_sub(1));
        }
    }

    /// After a compose keystroke: when the draft's visual line count moved
    /// the slot-row count, rebuild rows (the only time typing touches the
    /// cached body). Returns whether rows were rebuilt.
    pub(crate) fn sync_compose_slots(&mut self) -> bool {
        let Some(c) = self.compose.as_mut() else {
            return false;
        };
        let want = c.wanted_slot_rows();
        if want == c.slot_rows {
            return false;
        }
        c.slot_rows = want;
        let cursor = self.cursor;
        self.rebuild_rows();
        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
        true
    }

    /// The `[first, last]` row span of the open compose's slots.
    pub(crate) fn compose_slot_span(&self) -> Option<(usize, usize)> {
        let first = self.rows.iter().position(RowRef::is_compose_slot)?;
        let n = self.rows[first..].iter().take_while(|r| r.is_compose_slot()).count();
        Some((first, first + n - 1))
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
        self.compose = None;
        self.compose_gen = self.compose_gen.wrapping_add(1);
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
            self.moving_down = forward;
        }
    }

    /// `]`/`[` — stays put when there is no further file.
    pub(crate) fn jump_file(&mut self, forward: bool) {
        if let Some(i) = next_file_row(&self.rows, self.cursor, forward) {
            self.cursor = i;
            self.moving_down = forward;
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


    /// The pre-E3 placement, verbatim in shape, over the REAL row skeleton:
    /// strip the card rows from `rows` (leaving headers, hunks, lines, gaps /
    /// expanders), then per file filter ALL comments by path and place each
    /// with `place_comment` (which rebuilds the side vector per comment). The
    /// reference the optimized `visible_rows` must match row-for-row.
    fn reference_visible_rows(model: &DiffModel, review: Option<&Review>, rows: &[RowRef]) -> Vec<RowRef> {
        let texts = FileTexts::default();
        let skeleton: Vec<RowRef> = rows.iter().copied().filter(|r| !matches!(r, RowRef::Comment { .. })).collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < skeleton.len() {
            let header = skeleton[i];
            let RowRef::File { file: fi, .. } = header else { panic!("skeleton starts each file with its header") };
            let f = &model.files[fi];
            let mut j = i + 1;
            while j < skeleton.len() && !skeleton[j].is_file() {
                j += 1;
            }
            let body = &skeleton[i + 1..j];
            out.push(header);
            let mut top = Vec::new();
            let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
            if let (Some(r), false) = (review, body.is_empty()) {
                for (ci, c) in r.comments.iter().enumerate().filter(|(_, c)| c.path == f.path) {
                    match (!c.outdated).then(|| place_comment(f, &texts, body, c)).flatten() {
                        Some(k) => after.entry(k).or_default().push(ci),
                        None => top.push(ci),
                    }
                }
            }
            let push_card = |out: &mut Vec<RowRef>, ci: usize| {
                let c = &review.unwrap().comments[ci];
                let parts = comment_card_lines(c).len().clamp(1, u8::MAX as usize) as u8;
                for part in 0..parts {
                    out.push(RowRef::Comment { file: fi, comment: ci, part, parts });
                }
            };
            for ci in top {
                push_card(&mut out, ci);
            }
            for (k, row) in body.iter().enumerate() {
                out.push(*row);
                if let Some(cards) = after.get(&k) {
                    for &ci in cards {
                        push_card(&mut out, ci);
                    }
                }
            }
            i = j;
        }
        out
    }

    /// E3: many files × many comments (both sides, empty snippets, multi-line
    /// snippets, outdated, unplaceable, and comments on paths not in the diff)
    /// produce rows IDENTICAL to the pre-E3 algorithm, while visiting each
    /// comment once and building each file's side vector at most once per side.
    ///
    /// Negative control (observed RED): build the side vector per comment
    /// (drop the `get_or_insert_with` memo) → `side builds` exceeds the bound.
    #[test]
    fn visible_rows_many_files_and_comments_match_reference_in_linear_work() {
        const FILES: usize = 40;
        let mut raw = String::new();
        for i in 0..FILES {
            raw.push_str(&format!(
                "diff --git a/f{i}.rs b/f{i}.rs\nindex 1..2 100644\n--- a/f{i}.rs\n+++ b/f{i}.rs\n\
                 @@ -1,4 +1,4 @@\n ctx\n-old{i}\n+new{i}\n same\n dup\n\
                 @@ -30,3 +30,4 @@\n dup\n+added{i}\n same\n tail\n"
            ));
        }
        let m = parse_diff(&raw, PathBuf::from("/wt"), "feature", "main", "deadbeef");
        assert_eq!(m.files.len(), FILES);
        let mut review = Review::new("feature", "main", std::path::Path::new("/wt"));
        let now = chrono::Utc::now();
        let mut n = 0usize;
        for i in 0..FILES {
            let path = PathBuf::from(format!("f{i}.rs"));
            let specs: Vec<(CommentSide, [usize; 2], String)> = vec![
                (CommentSide::New, [2, 2], format!("new{i}")),
                (CommentSide::Old, [2, 2], format!("old{i}")),
                (CommentSide::New, [31, 31], "same".into()), // two matches: nearest wins
                (CommentSide::New, [4, 4], "same".into()),
                (CommentSide::New, [30, 31], format!("dup\nadded{i}")),
                (CommentSide::New, [32, 32], String::new()), // exact-line fallback
                (CommentSide::Old, [99, 99], "nowhere".into()), // unplaceable → top
            ];
            for (side, lines, snippet) in specs {
                review.add_comment(path.clone(), side, lines, snippet, format!("body {n}"), now);
                n += 1;
            }
            if i % 5 == 0 {
                review.comments.last_mut().unwrap().outdated = true;
                let c = &mut review.comments[n - 3];
                c.outdated = true;
            }
        }
        // Comments on files the diff doesn't contain are never shown.
        for j in 0..10 {
            review.add_comment(
                PathBuf::from(format!("gone{j}.rs")),
                CommentSide::New,
                [1, 1],
                "x".into(),
                "orphan".into(),
                now,
            );
        }
        let mut folds = Folds::default();
        folds.toggle(&m.files[3].path, false); // one folded file shows no cards

        let _ = row_build_counters::take();
        let rows = visible_rows(&m, Some(&review), &folds);
        let (visits, side_builds) = row_build_counters::take();
        let expected = reference_visible_rows(&m, Some(&review), &rows);
        assert_eq!(rows, expected, "optimized rows must equal the reference rows");
        assert!(
            rows.iter().filter(|r| r.is_inline_insert()).count() > FILES * 5,
            "non-vacuous: most comments placed as cards"
        );
        assert_eq!(visits, review.comments.len(), "each comment visited exactly once");
        assert!(
            side_builds <= 2 * FILES,
            "side vectors built at most once per side per file: got {side_builds}"
        );
    }


    fn file_rows(rows: &[RowRef]) -> Vec<usize> {
        rows.iter().enumerate().filter(|(_, r)| r.is_file()).map(|(i, _)| i).collect()
    }

    #[test]
    fn visible_rows_lists_headers_hunks_and_numbered_lines() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        // a.rs: header + (hunk + 4) + expander (new 4..=19 hidden) + (hunk +
        // 3); b.rs: header + (hunk + 2). Neither file gets a top expander
        // (both start at line 1) nor a bottom one (their last hunks end in
        // < 3 context lines ⇒ end-of-file).
        assert_eq!(rows.len(), 1 + 5 + 1 + 4 + 1 + 3);
        assert_eq!(file_rows(&rows), vec![0, 11]);
        assert_eq!(
            rows[6],
            RowRef::Expander {
                file: 0,
                gap: 1,
                first_hidden: 4,
                hidden: Some(16),
                kind: GapKind::Between,
                loading: false
            }
        );
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
            rows[9],
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
        assert_eq!(visible_rows(&m, Some(&review), &folds).len(), 15);
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
        assert_eq!(step_row(&rows, 14, 1), 14);
        assert_eq!(step_row(&[], 5, 1), 0);
        // Hunks at 1, 7, 12 (the expander at 6 is not a hunk stop).
        assert_eq!(next_hunk_row(&rows, 0, true), Some(1));
        assert_eq!(next_hunk_row(&rows, 1, true), Some(7));
        assert_eq!(next_hunk_row(&rows, 7, true), Some(12));
        assert_eq!(next_hunk_row(&rows, 12, true), None);
        assert_eq!(next_hunk_row(&rows, 9, false), Some(7));
        assert_eq!(next_hunk_row(&rows, 1, false), None);
        // Files at 0, 11.
        assert_eq!(next_file_row(&rows, 3, true), Some(11));
        assert_eq!(next_file_row(&rows, 11, true), None);
        assert_eq!(next_file_row(&rows, 13, false), Some(11));
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
        assert_eq!(next_unviewed_file_row(&rows, 0), Some(11));
    }

    /// UXI-Diff-13: a Line anchor re-resolves to the nearest new-side line in
    /// the same file after the rows shift; a vanished file falls back.
    #[test]
    fn anchor_survives_a_shift_and_falls_back_when_file_gone() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        let anchor = cursor_anchor(&m, &rows, 9).expect("anchor");
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
        let hdr = cursor_anchor(&m, &rows, 11).unwrap();
        assert_eq!(resolve_anchor(&m, &rows, &hdr, 0), 11);
    }

    #[test]
    fn line_anchor_and_zed_target_pick_the_right_side() {
        let m = model();
        let rows = visible_rows(&m, None, &Folds::default());
        let removed = line_anchor(&m, &FileTexts::default(), rows[3]).unwrap();
        assert_eq!((removed.side, removed.line, removed.text.as_str()), (CommentSide::Old, 2, "two"));
        let added = line_anchor(&m, &FileTexts::default(), rows[4]).unwrap();
        assert_eq!((added.side, added.line, added.text.as_str()), (CommentSide::New, 2, "TWO"));
        assert!(line_anchor(&m, &FileTexts::default(), rows[0]).is_none() && line_anchor(&m, &FileTexts::default(), rows[1]).is_none());
        assert_eq!(zed_target(&m, &rows, 4), Some((PathBuf::from("a.rs"), 2)));
        // Removed line "two" sits after new line 1 ⇒ 2.
        assert_eq!(zed_target(&m, &rows, 3), Some((PathBuf::from("a.rs"), 2)));
        assert_eq!(zed_target(&m, &rows, 7), Some((PathBuf::from("a.rs"), 20)));
        assert_eq!(zed_target(&m, &rows, 6), Some((PathBuf::from("a.rs"), 4)), "expander ⇒ its first hidden line");
        assert_eq!(zed_target(&m, &rows, 0), Some((PathBuf::from("a.rs"), 1)));
        assert_eq!(zed_target(&m, &rows, 11), Some((PathBuf::from("b.rs"), 1)));
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
        // Outdated card: header, body "old", snippet "gone", footer.
        assert!(matches!(rows[at(2) + 3], RowRef::Comment { part: 3, parts: 4, .. }), "outdated card shows its snippet");
        assert_eq!(
            comment_card_lines(&review.comments[2]),
            vec![CardLine::Header, CardLine::Body("old".into()), CardLine::Snippet("gone".into()), CardLine::Footer]
        );
        assert_eq!(host_row(&rows, at(0)), at(0) - 1);

        // Mixed removed/added span ⇒ new side, new-side lines only.
        let plain = visible_rows(&m, None, &Folds::default());
        let a = comment_anchor_for_range(&m, &FileTexts::default(), &plain, 2, 5).unwrap();
        assert_eq!((a.side, a.lines, a.snippet.as_str()), (CommentSide::New, [1, 3], "one\nTWO\nthree"));
        let removed = comment_anchor_for_range(&m, &FileTexts::default(), &plain, 3, 3).unwrap();
        assert_eq!((removed.side, removed.lines), (CommentSide::Old, [2, 2]));
        assert!(comment_anchor_for_range(&m, &FileTexts::default(), &plain, 0, 1).is_none(), "headers only");
        assert_eq!(a.label(), "a.rs:1–3");

        assert_eq!(wrap_cols("ab cd ef", 5), vec!["ab cd", "ef"]);
        assert_eq!(wrap_cols("abcdefg", 3), vec!["abc", "def", "g"]);
        assert_eq!(wrap_cols("a\n\nb", 10), vec!["a", "", "b"]);
        let long = comment("a.rs", CommentSide::New, [1, 1], "", &"x\n".repeat(20));
        let lines = comment_card_lines(&long);
        assert_eq!(lines.len(), COMMENT_MAX_BODY_ROWS + 2, "header + capped body + footer");
        assert_eq!(lines.first(), Some(&CardLine::Header));
        assert_eq!(lines[lines.len() - 2], CardLine::More);
        assert_eq!(lines.last(), Some(&CardLine::Footer));
    }

    /// `V` confines j/k to the range's file (never onto a header or the next
    /// file); `V` on a non-line row refuses.
    #[test]
    fn compose_layout_wraps_places_caret_and_sizes_slots() {
        let w = COMPOSE_WRAP_COLS;
        let long: String = "x".repeat(w + 5);
        let l = compose_visual_lines(&format!("ab\n{long}\n"), 1, w + 2);
        assert_eq!(l.lines.len(), 4, "ab · x*w · xxxxx · empty last line");
        assert_eq!(l.lines[1].chars().count(), w);
        assert_eq!(l.caret, (2, 2), "caret past the wrap lands on the continuation");
        assert_eq!(compose_visual_lines("", 0, 0), ComposeLines { lines: vec![String::new()], caret: (0, 0) });
        assert_eq!(compose_visual_lines("abc", 0, 99).caret, (0, 3), "col clamped to the line");
        assert_eq!(compose_slot_rows(1), COMPOSE_MIN_LINES + COMPOSE_CHROME_ROWS);
        assert_eq!(compose_slot_rows(5), 5 + COMPOSE_CHROME_ROWS);
        assert_eq!(compose_slot_rows(500), COMPOSE_MAX_LINES + COMPOSE_CHROME_ROWS);
        assert_eq!(compose_window_top(2, 3), 0);
        assert_eq!(compose_window_top(COMPOSE_MAX_LINES + 3, COMPOSE_MAX_LINES + 10), 4, "caret on the last shown line");
        assert_eq!(compose_window_top(0, 40), 0);
    }

    /// spec B2a: gap geometry (new-side span + old offset), including a
    /// pure-deletion hunk's zero-count side and the end-of-file heuristic.
    #[test]
    fn file_gaps_span_and_offset() {
        let m = model();
        let gaps = file_gaps(&m.files[0], None);
        assert_eq!(gaps.len(), 3);
        assert_eq!((gaps[0].lo, gaps[0].len()), (1, Some(0)), "hunk at line 1 ⇒ empty top gap");
        assert_eq!(gaps[1], Gap { lo: 4, hi: Some(19), old_delta: 0, kind: GapKind::Between });
        // Last hunk -20,2 +20,3 ends in 1 context line ⇒ end-of-file known.
        assert_eq!(gaps[2], Gap { lo: 23, hi: Some(22), old_delta: -1, kind: GapKind::Bottom });
        let loaded = file_gaps(&m.files[0], Some(30));
        assert_eq!(loaded[2].len(), Some(8), "a loaded text gives the real end");
        assert!(file_gaps(&m.files[1], None).iter().all(|g| g.len() == Some(0)), "an added file hides nothing");

        let h = Hunk { header: "@@ -5,2 +4,0 @@".into(), lines: vec![] };
        assert_eq!((h.first_lines(), h.next_lines()), ((5, 5), (7, 5)), "zero-count side starts after `start`");
        let h = Hunk { header: "@@ -9 +9 @@".into(), lines: vec![] };
        assert_eq!(h.ranges(), ((9, 1), (9, 1)), "omitted count is 1");
    }

    /// spec B2a: an expand reveals 20 from an edge (all when < 30 hide),
    /// `All` the rest; Enter's default direction; the expander segments.
    #[test]
    fn apply_expand_steps_and_defaults() {
        let gap = Gap { lo: 45, hi: Some(127), old_delta: -1, kind: GapKind::Between };
        let mut r = GapReveal::default();
        assert!(apply_expand(&gap, &mut r, ExpandDir::Down));
        assert_eq!(r, GapReveal { top: 20, bottom: 0 });
        assert!(apply_expand(&gap, &mut r, ExpandDir::Up));
        assert_eq!(r, GapReveal { top: 20, bottom: 20 });
        assert_eq!(gap.effective(r), (20, 20, Some(43)));
        assert!(apply_expand(&gap, &mut r, ExpandDir::Down));
        assert_eq!(gap.effective(r).2, Some(23));
        assert!(apply_expand(&gap, &mut r, ExpandDir::Up), "23 < 30 hidden ⇒ one step shows them all");
        assert_eq!(gap.effective(r).2, Some(0));
        assert!(!apply_expand(&gap, &mut r, ExpandDir::All), "nothing left");
        let unknown = Gap { hi: None, ..gap };
        assert!(!apply_expand(&unknown, &mut GapReveal::default(), ExpandDir::Down));

        assert_eq!(default_expand_dir(GapKind::Top, Some(90), true), ExpandDir::Up);
        assert_eq!(default_expand_dir(GapKind::Bottom, None, false), ExpandDir::Down);
        assert_eq!(default_expand_dir(GapKind::Between, Some(12), false), ExpandDir::All);
        assert_eq!(default_expand_dir(GapKind::Between, Some(83), true), ExpandDir::Down);
        assert_eq!(default_expand_dir(GapKind::Between, Some(83), false), ExpandDir::Up);

        let labels = |k, h| expander_segments(k, h).into_iter().map(|(_, l)| l).collect::<Vec<_>>();
        assert_eq!(labels(GapKind::Between, Some(83)), ["↓ 20 more lines", "↑ 20 more lines", "Show all 83 hidden lines"]);
        assert_eq!(labels(GapKind::Top, Some(36)), ["↑ 20 more lines", "Show all 36 hidden lines"]);
        assert_eq!(labels(GapKind::Bottom, None), ["↓ 20 more lines"]);
        assert_eq!(labels(GapKind::Between, Some(1)), ["Show all 1 hidden line"]);
    }

    /// spec B2a: with the text loaded, a reveal emits numbered `Ctx` rows
    /// around the expander; a fully revealed gap drops the expander AND the
    /// next hunk's header; a comment on a revealed line anchors to it.
    #[test]
    fn visible_rows_reveal_context_and_merge_hunks() {
        let m = model();
        let f = &m.files[0];
        let text: String = (1..=22).map(|i| format!("a{i}\n")).collect();
        let mut texts = FileTexts::default();
        texts.invalidate(&m);
        texts.insert_ready(&f.path, CommentSide::New, f.file_hash, text.into());
        let mut exp = Expansions::default();
        exp.entry(f).gaps.insert(1, GapReveal { top: 2, bottom: 3 });
        let rows = visible_rows_ex(&m, None, &Folds::default(), &exp, &texts);
        let ctx = |n: u32| RowRef::Ctx { file: 0, old: n, new: n };
        assert_eq!(&rows[6..13], &[
            ctx(4),
            ctx(5),
            RowRef::Expander { file: 0, gap: 1, first_hidden: 6, hidden: Some(11), kind: GapKind::Between, loading: false },
            ctx(17),
            ctx(18),
            ctx(19),
            RowRef::Hunk { file: 0, hunk: 1 },
        ]);
        let anchor = line_anchor(&m, &texts, rows[7]).unwrap();
        assert_eq!((anchor.side, anchor.line, anchor.text.as_str()), (CommentSide::New, 5, "a5"));
        assert_eq!(zed_target(&m, &rows, 7), Some((PathBuf::from("a.rs"), 5)));

        let mut review = Review::new("feature", "main", std::path::Path::new("/wt"));
        review.comments.push(comment("a.rs", CommentSide::New, [18, 18], "a18", "ctx"));
        let rows_c = visible_rows_ex(&m, Some(&review), &Folds::default(), &exp, &texts);
        let at = rows_c.iter().position(|r| r.comment_index() == Some(0)).unwrap();
        assert_eq!(rows_c[at - 1], ctx(18), "a comment on a revealed line sits under it");

        exp.entry(f).gaps.insert(1, GapReveal { top: 16, bottom: 0 });
        let rows = visible_rows_ex(&m, None, &Folds::default(), &exp, &texts);
        assert!(!rows.iter().any(|r| matches!(r, RowRef::Expander { gap: 1, .. })), "fully revealed ⇒ no expander");
        assert!(!rows.contains(&RowRef::Hunk { file: 0, hunk: 1 }), "…and the hunks merge (header dropped)");
        assert_eq!(rows[21], ctx(19));
        assert!(matches!(rows[22], RowRef::Line { hunk: 1, line: 0, new: Some(20), .. }));

        // Without the text the reveal waits (no Ctx rows), and a changed
        // file_hash invalidates both the text and the expansion.
        let rows = visible_rows_ex(&m, None, &Folds::default(), &exp, &FileTexts::default());
        assert!(!rows.iter().any(|r| matches!(r, RowRef::Ctx { .. })));
        let mut m2 = m.clone();
        m2.files[0].file_hash ^= 1;
        texts.invalidate(&m2);
        exp.retain_for(&m2);
        assert!(texts.content(&f.path, CommentSide::New).is_none() && exp.by_path.is_empty());
    }

    #[test]
    fn file_content_lines_and_snippet_location() {
        let c = FileContent::new("one\r\ntwo\nthree".into());
        assert_eq!(c.line_count(), 3);
        assert_eq!((c.line(1), c.line(2), c.line(3), c.line(4), c.line(0)), (Some("one"), Some("two"), Some("three"), None, None));
        assert_eq!(FileContent::new("x\n".into()).line_count(), 1, "a trailing newline is not a line");
        assert_eq!(FileContent::new("".into()).line_count(), 0);
        let c = FileContent::new("a\nb\na\nb\n".into());
        assert_eq!(locate_snippet(&c, "a\nb", 4), Some((3, 4)), "nearest occurrence wins");
        assert_eq!(locate_snippet(&c, "a\nb\n", 1), Some((1, 2)));
        assert_eq!(locate_snippet(&c, "zz", 1), None);
    }

    #[test]
    fn range_selection_is_clamped_to_one_file() {
        let mut t = DiffTile::bound_to(PathBuf::from("/wt"));
        t.model = Some(Rc::new(model()));
        t.rebuild_rows();
        t.cursor = 0;
        assert!(!t.toggle_range(), "V on a header refuses");
        t.cursor = 10; // a.rs's last line
        assert!(t.toggle_range());
        t.move_cursor(1);
        assert_eq!(t.cursor, 10, "can't extend into b.rs");
        for _ in 0..20 {
            t.move_cursor(-1);
        }
        assert_eq!(t.cursor, 1, "stops at a.rs's first hunk row, not its header");
        assert_eq!(t.selection(), Some((1, 10)));
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
        assert_eq!(t.rows.len(), 1 + 10 + 1, "unmarking re-expands a.rs");
    }
}
