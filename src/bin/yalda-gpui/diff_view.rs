//! `DiffView` — the cached body of a Diff tile (`App::Diff` / `DiffTile`,
//! `diff.rs`). Cog node `app-diff-tile` (nd0e). A yux component (see
//! `yux/CLAUDE.md`): embedded via `cached_child` in `render_diff`
//! (`screens.rs`).
//!
//! # Why this observes the ROOT, not a domain entity
//!
//! `TranscriptView` observes its `Entity<AgentSession>`, and `LinearView` /
//! `CogView` own their payload directly (no observe at all — their tile
//! holds no content). `DiffTile` is neither: per spec § Data Model, the
//! derived `DiffModel` + review + cursor + folds are the SPEC's data model
//! fields, owned by `DiffTile` itself — a plain struct living in the
//! workspace layout tree (not a GPUI entity, like `LinearTile`/`CogTile`).
//! There is no separate "model entity" to observe. The only entity through
//! which `DiffTile` is reachable is the ROOT view (`YaldaGpuiView`), so
//! `DiffView` observes THAT, exactly like `TranscriptView` observes its
//! session — filtered through a cheap fingerprint ([`DiffSeqs`]) so an
//! unrelated root notify (typing in an agent tile elsewhere, an unrelated
//! menu, etc.) leaves this view's render flat. This also means a GLOBAL
//! input (text zoom) needs no separate `notify_diff_views` push: it already
//! lives on the same root entity this view observes, so it falls out of the
//! same fingerprint for free (deviation from the transcript/linear precedent
//! documented here, not a gap — see the render-count guard test in
//! `verify_harness.rs`).
//!
//! `render()` reads `DiffTile` fields directly off the root's `&YaldaGpuiView`
//! borrow (no `DiffModel` clone — the model and rows are `Rc`s) — the "reads,
//! does not own" contract. The one piece of UI state this view OWNS is the
//! scroll of its virtualized row list (`list`, a `ScrollAnchoredList<RowRef>`):
//! the bound body is a `gpui::list` over the tile's cached `rows`, so a
//! thousands-of-lines diff paints O(visible rows). Every row has the SAME
//! fixed height, which makes "keep the cursor row in view" exact integer
//! arithmetic (`compose_first_visible_line`) instead of trusting gpui's
//! estimate for unmeasured rows (which counts them as 0px).

use super::*;

use std::sync::Arc;

/// The slice-version watermark the observe filter compares across renders.
/// Mirrors `TranscriptSeqs` / the `RootSnapshot` fingerprint idea, but over
/// `DiffTile` fields read off the root. Cheap: every field is a `Copy` read
/// (the `DiffModel`/`Review` themselves are never hashed — `model_gen` /
/// `review_gen` / `rows_gen` are their proxies). EVERY `DiffTile` field the
/// render reads must be covered here (yux rule 2).
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) struct DiffSeqs {
    bound: bool,
    model_gen: u64,
    /// `DiffTile::rows_gen` — bumped on every rows rebuild (derive, `z`
    /// fold, Viewed toggle): covers the row list, fold glyphs, viewed flags.
    rows_gen: u64,
    /// `DiffTile::cursor` — the cursor row's highlight + scroll-into-view.
    cursor: usize,
    /// `DiffTile::review_gen` — the header's `N/M files viewed` progress,
    /// the `K unsent` count, and every comment card's content.
    review_gen: u64,
    /// `DiffTile::range_anchor` — the `V` selection tint.
    range_anchor: Option<usize>,
    /// `DiffTile::compose_gen` — the compose-anchor highlight (bumped on
    /// open/close only, so typing in the compose never re-renders the body).
    compose_gen: u64,
    refreshing: bool,
    has_error: bool,
    /// `WorktreePicker::gen_` — bumped by every picker mutation (rows loaded,
    /// selection moved, loading/not-a-repo/error flips), so the unbound
    /// picker re-renders exactly when its own state moves.
    picker_gen: u64,
    /// `text_scale.to_bits()` — global zoom input (UXI-TextZoom-1 pattern),
    /// falls out of the same root-observe fingerprint (see module docs).
    text_scale_bits: u32,
}

impl DiffSeqs {
    pub(crate) fn of(tile: &DiffTile, text_scale: f32) -> Self {
        DiffSeqs {
            bound: tile.worktree.is_some(),
            model_gen: tile.model_gen,
            rows_gen: tile.rows_gen,
            cursor: tile.cursor,
            review_gen: tile.review_gen,
            range_anchor: tile.range_anchor,
            compose_gen: tile.compose_gen,
            refreshing: tile.refreshing,
            has_error: tile.error.is_some(),
            picker_gen: tile.picker.gen_,
            text_scale_bits: text_scale.to_bits(),
        }
    }
}

/// The cached Diff body view. One per Diff tile (owned by the tile via
/// `Entity<DiffView>`, dropped when the tile closes — no registry).
pub(crate) struct DiffView {
    root: WeakEntity<YaldaGpuiView>,
    /// The stable id of the tile this view belongs to — how `render()` finds
    /// its own `DiffTile` back through the root (see module docs).
    window_id: workspace::WindowId,
    last_rendered: DiffSeqs,
    /// Scroll for the non-list bodies (picker / error / loading).
    scroll: ScrollHandle,
    /// The virtualized bound body (module docs) — this view's own UI state.
    list: ScrollAnchoredList<RowRef>,
    /// The `(cursor, rows_gen, compose_gen)` the list was last scrolled to
    /// reveal — the reveal runs only when one moves, so a wheel-scroll isn't
    /// undone by an unrelated re-render.
    revealed: Option<(usize, u64, u64)>,
    /// The comment-card snapshot shared by every row closure, keyed on the
    /// `review_gen` it was built at (E1/E2): `j`/`k` re-render the body
    /// (`cursor` is in `DiffSeqs`) but reuse this `Rc` — no deep clone of the
    /// review's comments, no re-wrap of any card body. Rebuilt only when
    /// `review_gen` moves (every review mutation bumps it).
    cards: Option<(u64, Rc<CommentCards>)>,
    perf_label: &'static str,
}

/// The review's comments plus each one's wrapped card rows
/// ([`comment_card_lines`]), computed once per `review_gen` (see
/// `DiffView::cards`). Indexed like `Review::comments` / `RowRef::Comment`.
#[derive(Default)]
pub(crate) struct CommentCards {
    comments: Vec<ReviewComment>,
    lines: Vec<Vec<CardLine>>,
}

impl CommentCards {
    fn build(review: Option<&Review>) -> Self {
        let comments = review.map(|r| r.comments.clone()).unwrap_or_default();
        let lines = comments.iter().map(comment_card_lines).collect();
        CommentCards { comments, lines }
    }
}

impl DiffView {
    /// Construct a Diff body view and register the `cx.observe(&root)`
    /// subscription that self-notifies on a fingerprint move. `root` is a
    /// STRONG handle for the duration of this call only (needed to register
    /// the subscription); the callback receives its own copy of the
    /// observed entity from gpui, so nothing here captures `root` by `move`
    /// — no retain cycle (mirrors `TranscriptView::new`'s shape exactly,
    /// substituting the session entity for the root entity).
    pub(crate) fn new(
        root: Entity<YaldaGpuiView>,
        window_id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&root, |this: &mut DiffView, root_ent, cx| {
            let now = root_ent.read(cx).diff_seqs_for(this.window_id);
            if this.last_rendered != now {
                record_notify(this.perf_label, MissReason::Dirtied);
                cx.notify();
            }
        })
        .detach();
        DiffView {
            root: root.downgrade(),
            window_id,
            last_rendered: DiffSeqs::default(),
            scroll: ScrollHandle::new(),
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, px(DIFF_ROW_BASE_H * 20.0)),
            revealed: None,
            cards: None,
            perf_label: "diff",
        }
    }

    pub(crate) fn perf_label(&self) -> &'static str {
        self.perf_label
    }

    /// The shared card snapshot for `tile`'s current `review_gen`, rebuilt
    /// only when the generation moved (see [`DiffView::cards`]).
    fn cards_for(&mut self, tile: &DiffTile) -> Rc<CommentCards> {
        match &self.cards {
            Some((g, c)) if *g == tile.review_gen => c.clone(),
            _ => {
                let c = Rc::new(CommentCards::build(tile.review.as_ref()));
                self.cards = Some((tile.review_gen, c.clone()));
                c
            }
        }
    }

    /// Test seam: row `part` of comment `ci`'s card in the CACHED snapshot —
    /// exactly what the row closure paints.
    #[cfg(test)]
    pub(crate) fn cached_card_line(&self, ci: usize, part: usize) -> Option<CardLine> {
        self.cards.as_ref()?.1.lines.get(ci)?.get(part).cloned()
    }

    /// Keep the cursor row fully inside the list viewport with minimal
    /// scrolling. Rows are uniform (`row_h`), so the top row is exact integer
    /// arithmetic over the last laid-out viewport height. State-only (no
    /// notify) — safe on the render path, like the Doc view's reveal.
    ///
    /// `compose_span`: the inline comment compose's slot rows (spec B5). While
    /// it is open the whole editor is revealed too — its last slot row first,
    /// then the row above it (the anchor line) / the cursor, so a tall draft
    /// never pushes the commented line out of view. The key includes
    /// `rows_gen`, so the draft growing a line (a slot-row rebuild) re-reveals
    /// and the editor's bottom (the caret) stays in view.
    fn reveal_cursor(
        &mut self,
        cursor: usize,
        rows_gen: u64,
        compose_gen: u64,
        compose_span: Option<(usize, usize)>,
        row_count: usize,
        row_h: f32,
    ) {
        let key = (cursor, rows_gen, compose_gen);
        if self.revealed == Some(key) || row_count == 0 {
            return;
        }
        self.revealed = Some(key);
        let state = self.list.state();
        let vh = f32::from(state.viewport_bounds().size.height);
        if vh <= 0.0 {
            // Never laid out yet: fall back to gpui's own reveal.
            state.scroll_to_reveal_item(compose_span.map_or(cursor, |(_, last)| last));
            return;
        }
        let visible = ((vh / row_h).floor() as usize).max(1);
        let prev = state.logical_scroll_top();
        let top = match compose_span {
            Some((first, last)) => {
                let bottom = compose_first_visible_line(last, prev.item_ix, row_count, visible);
                compose_first_visible_line(first.saturating_sub(1).min(cursor), bottom, row_count, visible)
            }
            None => compose_first_visible_line(cursor, prev.item_ix, row_count, visible),
        };
        if top != prev.item_ix || (cursor == top && prev.offset_in_item != px(0.0)) {
            state.scroll_to(gpui::ListOffset {
                item_ix: top,
                offset_in_item: px(0.0),
            });
        }
    }

    /// The virtualized body's scroll state — read by the root to paint the
    /// inline comment compose over its slot rows (`list_rows_overlay`).
    pub(crate) fn list_state(&self) -> gpui::ListState {
        self.list.state().clone()
    }
}

/// Unscaled height of one diff row (every row — file header, hunk header,
/// line, comment-card row, compose slot — is exactly [`diff_row_h`]).
const DIFF_ROW_BASE_H: f32 = 22.0;

/// The painted height of every Diff body row at `text_scale` (shared with the
/// root's inline-compose overlay, which must land exactly on the slot rows).
pub(crate) fn diff_row_h(text_scale: f32) -> Pixels {
    px((DIFF_ROW_BASE_H * text_scale).round())
}

/// Left inset of a comment card / the inline compose: aligned with the code
/// text (past both line-number gutters and the sign column).
pub(crate) fn diff_card_inset_left(text_scale: f32) -> Pixels {
    diff_gutter_w(text_scale) * 2.0 + px(16.0)
}

fn diff_gutter_w(text_scale: f32) -> Pixels {
    px((40.0 * text_scale).round())
}

/// Right inset of a comment card / the inline compose.
pub(crate) const DIFF_CARD_INSET_RIGHT: f32 = 16.0;
/// Corner radius of a comment card / the inline compose box.
pub(crate) const DIFF_CARD_RADIUS: f32 = 6.0;
/// Vertical gap between a card box and the diff rows above/below it.
pub(crate) const DIFF_CARD_GAP: f32 = 4.0;

/// Colors/fonts/sizes the `'static` row closure needs, snapshotted once per
/// render (all `Copy`/refcounted).
#[derive(Clone)]
struct DiffRowStyle {
    fg: Hsla,
    dim: Hsla,
    accent: Hsla,
    add: Hsla,
    remove: Hsla,
    header: Hsla,
    add_bg: Hsla,
    remove_bg: Hsla,
    file_bg: Hsla,
    cursor_bg: Hsla,
    cursor_bar: Hsla,
    /// `V` range / compose-anchor tint.
    sel_bg: Hsla,
    /// Comment card body fill (OPAQUE — the border ring shows through a
    /// translucent fill otherwise).
    card_bg: Hsla,
    /// Comment card header strip fill (opaque).
    card_header_bg: Hsla,
    /// Neutral card border ring (opaque); a focused card uses `accent`, an
    /// outdated one `card_outdated_border`.
    card_border: Hsla,
    card_outdated_border: Hsla,
    /// Context expander row tint (subdued) + its segment hover.
    expander_bg: Hsla,
    expander_hover_bg: Hsla,
    prose: SharedString,
    mono: SharedString,
    text: Pixels,
    small: Pixels,
    row_h: Pixels,
    gutter_w: Pixels,
}

impl Render for DiffView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let r = root_ent.read(cx);
        let scale = r.text_scale;
        let st = DetailStyle {
            fg: r.editor_fg(),
            dim: nc(r.theme.agent.dim),
            accent: nc(r.theme.agent.warm_accent),
            err: rgb(0xff6b6b).into(),
            mono: r.code_font.clone(),
            prose: r.body_font.clone(),
            base: px(14.0 * scale),
            pt: 14.0 * scale,
        };
        let editor_bg = r.editor_bg();
        let selected_bg: Hsla = nc(r.theme.overlay.selected_bg);
        let at = &r.theme.agent;
        let tint = |c: Hsla, a: f32| Hsla { a, ..c };
        let row_style = DiffRowStyle {
            fg: st.fg,
            dim: st.dim,
            accent: st.accent,
            add: nc(at.diff_add),
            remove: nc(at.diff_remove),
            header: nc(at.diff_header),
            add_bg: tint(nc(at.diff_add), 0.10),
            remove_bg: tint(nc(at.diff_remove), 0.10),
            file_bg: tint(selected_bg, 0.45),
            cursor_bg: tint(selected_bg, 0.70),
            cursor_bar: rgb(CURSOR_BAR_COLOR).into(),
            sel_bg: tint(st.accent, 0.16),
            card_bg: editor_bg.blend(tint(selected_bg, 0.30)),
            card_header_bg: editor_bg.blend(tint(selected_bg, 0.65)),
            card_border: editor_bg.blend(tint(st.dim, 0.55)),
            card_outdated_border: editor_bg.blend(tint(nc(at.diff_remove), 0.60)),
            expander_bg: tint(nc(at.diff_header), 0.08),
            expander_hover_bg: tint(nc(at.diff_header), 0.20),
            prose: st.prose.clone(),
            mono: st.mono.clone(),
            text: px(13.0 * scale),
            small: px(11.5 * scale),
            row_h: diff_row_h(scale),
            gutter_w: diff_gutter_w(scale),
        };
        let tile = r.diff_tile_ref(self.window_id);
        self.last_rendered = tile.map(|t| DiffSeqs::of(t, scale)).unwrap_or_default();

        // The bound, derived body: header + virtualized rows + footer.
        if let Some(t) = tile
            && t.worktree.is_some()
            && t.error.is_none()
            && let Some(model) = t.model.clone()
        {
            let (viewed, total) = t.progress();
            let header = diff_header(&model, viewed, total, t.unsent_count(), t.refreshing, &row_style, &st);
            let rows = t.rows.clone();
            let (cursor, rows_gen) = (t.cursor, t.rows_gen);
            let marks = RowMarks {
                selection: t.selection(),
                anchor: t.compose.as_ref().and_then(|c| {
                    let fi = model.files.iter().position(|f| f.path == c.anchor.path)?;
                    Some((fi, c.anchor.side, c.anchor.lines))
                }),
                cards: self.cards_for(t),
                now: chrono::Utc::now(),
            };
            let (compose_gen, compose_span) = (t.compose_gen, t.compose_slot_span());
            // Revealed context lines' text (spec B2a): the loaded new-side
            // contents, by file index. Loads rebuild rows (`rows_gen`), so
            // this snapshot is covered by `DiffSeqs`.
            let ctx_texts: Rc<HashMap<usize, Arc<FileContent>>> = Rc::new(
                model
                    .files
                    .iter()
                    .enumerate()
                    .filter_map(|(i, f)| Some((i, t.texts.content(&f.path, CommentSide::New)?.clone())))
                    .collect(),
            );
            let body: AnyElement = if model.files.is_empty() {
                diff_empty_body(&model, &st).into_any_element()
            } else {
                self.list.reconcile(&rows, rows_gen);
                self.reveal_cursor(
                    cursor,
                    rows_gen,
                    compose_gen,
                    compose_span,
                    rows.len(),
                    f32::from(row_style.row_h),
                );
                let render_fn = diff_row_renderer(
                    model,
                    ctx_texts,
                    rows,
                    cursor,
                    marks,
                    row_style,
                    self.root.clone(),
                    self.window_id,
                );
                probe_bounds(
                    "diff-list",
                    gpui::list(self.list.state().clone(), render_fn)
                        .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                        .size_full()
                        .into_any_element(),
                )
            };
            return div()
                .id("diff-body")
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .bg(editor_bg)
                .text_color(st.fg)
                .child(header)
                .child(div().flex_1().min_h_0().w_full().overflow_hidden().child(body))
                .child(key_hint_footer(DIFF_KEY_HINTS, &st))
                .into_any_element();
        }

        let body: AnyElement = match tile {
            None => div().size_full().into_any_element(),
            Some(t) => {
                if t.worktree.is_none() {
                    diff_picker_body(&t.picker, self.window_id, selected_bg, &st, cx)
                        .into_any_element()
                } else if let Some(err) = &t.error {
                    diff_error_body(err, &st).into_any_element()
                } else {
                    diff_loading_body(t.refreshing, &st).into_any_element()
                }
            }
        };

        let scroll = self.scroll.clone();
        div()
            .id("diff-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_4()
            .py_3()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(body)
            .into_any_element()
    }
}

/// The bound body's always-visible key hints (spec C6 idiot-proof).
pub(crate) const DIFF_KEY_HINTS: &str = "j/k line · {/} hunk · [/] file · enter/+ expand · v viewed · z fold · \
     c comment · V range · e edit · x delete · s send · r refresh · o zed · space menu";

/// The header's unsent-comment count label (spec B2; shown only when > 0).
pub(crate) fn diff_unsent_label(unsent: usize) -> String {
    format!("{unsent} unsent")
}

/// `2026-09-27T14:03:00Z` relative to `now`: "just now" / "5m ago" /
/// "3h ago" / "2d ago" (the raw string when it doesn't parse).
pub(crate) fn relative_time(at: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(at) else {
        return at.to_string();
    };
    relative_age((now - t.with_timezone(&chrono::Utc)).num_seconds())
}

/// Elapsed seconds → a compact age: "just now" / "5m ago" / "3h ago" /
/// "2d ago" / "3w ago" / "4mo ago" / "2y ago". Pure (callers pass `now`);
/// negative (clock skew) reads as "just now".
pub(crate) fn relative_age(elapsed_secs: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    let s = elapsed_secs.max(0);
    match s {
        _ if s < MIN => "just now".to_string(),
        _ if s < HOUR => format!("{}m ago", s / MIN),
        _ if s < DAY => format!("{}h ago", s / HOUR),
        _ if s < 2 * WEEK => format!("{}d ago", s / DAY),
        _ if s < 2 * MONTH => format!("{}w ago", s / WEEK),
        _ if s < YEAR => format!("{}mo ago", s / MONTH),
        _ => format!("{}y ago", s / YEAR),
    }
}

/// A worktree picker row's description line (UXI-Diff-10):
/// `<HEAD subject> · <age> · <~/path>`, or just the path when the commit is
/// unknown (fresh repo / git failure). `now_unix` is passed in (pure).
pub(crate) fn worktree_row_description(row: &WorktreeEntry, path: &str, now_unix: i64) -> String {
    match &row.head_commit {
        Some(c) if !c.subject.trim().is_empty() => {
            format!("{} · {} · {path}", c.subject.trim(), relative_age(now_unix - c.time))
        }
        Some(c) => format!("{} · {path}", relative_age(now_unix - c.time)),
        None => path.to_string(),
    }
}

/// The header progress label (spec B2/B4).
pub(crate) fn diff_progress_label(viewed: usize, total: usize) -> String {
    if total > 0 && viewed == total {
        "All files viewed ✓".to_string()
    } else {
        format!("{viewed}/{total} files viewed")
    }
}

// ── Domain body builders (Diff-specific; composed from yux primitives) ──────

fn diff_error_body(err: &str, st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(
            div()
                .text_color(st.err)
                .font_family(st.mono.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(st.base)
                .child(SharedString::from("diff error")),
        )
        .child(multiline_text(err, st.err, &st.prose, st.base))
}

fn diff_loading_body(refreshing: bool, st: &DetailStyle) -> gpui::Div {
    let msg = if refreshing {
        "deriving diff…"
    } else {
        "no diff yet — press r to refresh"
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .text_color(st.dim)
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(SharedString::from(msg))
}

/// `$HOME/…` → `~/…` for the picker's dimmed path line.
fn home_relative(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home).display().to_string();
        if let Some(rest) = raw.strip_prefix(&home)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("~{rest}");
        }
    }
    raw
}

/// The worktree picker (spec rev 2 B1, UXI-Diff-10): a title, one
/// `picker_option_row_detailed` per worktree (branch prominent; dimmed
/// description `subject · age · ~/path`, a "primary" badge on the primary checkout), then the "Pick a
/// folder…" row and the key-hint footer. Rows are clickable; the handler
/// carries only the ROW INDEX and resolves the row at event time through the
/// root (`diff_picker_activate`) — yux rule 4, since a cache hit replays this
/// render's listeners.
fn diff_picker_body(
    picker: &WorktreePicker,
    window_id: workspace::WindowId,
    selected_bg: Hsla,
    st: &DetailStyle,
    cx: &Context<DiffView>,
) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap(px(2.0));
    col = col.child(
        div()
            .pb_2()
            .text_color(st.fg)
            .font_family(st.prose.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(st.pt * 1.2))
            .child(SharedString::from("Review a worktree")),
    );

    let status: Option<(&str, Hsla)> = if picker.loading && picker.rows.is_empty() {
        Some(("Finding worktrees…", st.dim))
    } else if picker.not_a_repo {
        Some(("Not inside a git repository.", st.dim))
    } else {
        picker.error.as_deref().map(|e| (e, st.err))
    };
    if let Some((text, color)) = status {
        col = col.child(probe_bounds(
            "diff-picker-status",
            div()
                .py_1()
                .px(px(10.0))
                .text_color(color)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from(text.to_string()))
                .into_any_element(),
        ));
    }

    let root_listener = |index: usize| {
        cx.listener(move |this: &mut DiffView, _ev: &MouseDownEvent, _w, cx| {
            let Some(root) = this.root.upgrade() else {
                return;
            };
            let wid = this.window_id;
            root.update(cx, |r, cx| r.diff_picker_activate(wid, index, cx));
        })
    };

    let now_unix = chrono::Utc::now().timestamp();
    for (i, row) in picker.rows.iter().enumerate() {
        let label = row.label();
        let description = worktree_row_description(row, &home_relative(&row.path), now_unix);
        let el = picker_option_row_detailed(
            SharedString::from(format!("diff-picker-row-{window_id}-{i}")),
            "⎇",
            &label,
            Some((&description, st.dim)),
            row.is_primary.then_some(("primary", st.dim)),
            picker.selected == i,
            st.accent,
            st.fg,
            selected_bg,
            &st.prose,
            &st.mono,
        )
        .on_mouse_down(MouseButton::Left, root_listener(i));
        col = col.child(probe_bounds_dyn(
            format!("diff-picker-row-{i}"),
            el.into_any_element(),
        ));
    }

    let folder = picker.folder_index();
    let folder_row = picker_option_row(
        SharedString::from(format!("diff-picker-folder-{window_id}")),
        "…",
        "Pick a folder…",
        None,
        picker.selected == folder,
        st.accent,
        st.accent,
        selected_bg,
        &st.prose,
        &st.mono,
    )
    .on_mouse_down(MouseButton::Left, root_listener(folder));
    col = col.child(probe_bounds("diff-picker-folder-row", folder_row.into_any_element()));

    col.child(div().pt_2().child(key_hint_footer("j/k move · enter review · p pick folder", st)))
}

/// The bound header (spec B2): branch (bold) · `vs <base>` dimmed · a quiet
/// `refreshing…` while a derive is in flight · `N/M files viewed` (or "All
/// files viewed ✓" in the success color) · a slim progress bar. Chrome — fixed
/// sizes, doesn't zoom.
#[allow(clippy::too_many_arguments)]
fn diff_header(
    model: &DiffModel,
    viewed: usize,
    total: usize,
    unsent: usize,
    refreshing: bool,
    rs: &DiffRowStyle,
    st: &DetailStyle,
) -> gpui::Div {
    let label = diff_progress_label(viewed, total);
    let done = total > 0 && viewed == total;
    let frac = if total == 0 { 0.0 } else { viewed as f32 / total as f32 };
    let mut track = rs.dim;
    track.a *= 0.25;
    let mut hairline = rs.dim;
    hairline.a *= 0.35;
    let mut top = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .flex_none()
                .font_family(st.prose.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(px(15.0))
                .text_color(st.fg)
                .child(SharedString::from(model.branch.clone())),
        )
        .child(
            div()
                .flex_none()
                .font_family(st.mono.clone())
                .text_size(px(12.0))
                .text_color(st.dim)
                .child(SharedString::from(format!("vs {}", model.base))),
        )
        .child(div().flex_1());
    if refreshing {
        top = top.child(probe_bounds(
            "diff-refreshing",
            div()
                .flex_none()
                .font_family(st.mono.clone())
                .text_size(px(12.0))
                .text_color(st.dim)
                .child(SharedString::from("refreshing…"))
                .into_any_element(),
        ));
    }
    if unsent > 0 {
        let text = diff_unsent_label(unsent);
        top = top.child(probe_bounds_dyn(
            format!("diff-unsent={text}"),
            div()
                .flex_none()
                .font_family(st.mono.clone())
                .text_size(px(12.0))
                .text_color(rs.accent)
                .child(SharedString::from(text))
                .into_any_element(),
        ));
    }
    top = top.child(probe_bounds_dyn(
        format!("diff-progress={label}"),
        div()
            .flex_none()
            .font_family(st.mono.clone())
            .text_size(px(12.0))
            .text_color(if done { rs.add } else { st.fg })
            .child(SharedString::from(label))
            .into_any_element(),
    ));
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .w_full()
        .px_4()
        .pt_3()
        .pb(px(10.0))
        .border_b_1()
        .border_color(hairline)
        .child(top)
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .rounded(px(2.0))
                .bg(track)
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative(frac))
                        .rounded(px(2.0))
                        .bg(if done { rs.add } else { rs.accent }),
                ),
        )
}

/// Empty diff (spec B2): a centered, explicit sentence — never a blank pane.
fn diff_empty_body(model: &DiffModel, st: &DetailStyle) -> gpui::Div {
    let text = format!("No changes on {} vs {}.", model.branch, model.base);
    div().size_full().flex().items_center().justify_center().child(probe_bounds_dyn(
        format!("diff-empty={text}"),
        div()
            .text_color(st.dim)
            .font_family(st.prose.clone())
            .text_size(st.base)
            .child(SharedString::from(text))
            .into_any_element(),
    ))
}

/// The virtualized list's `'static` per-row builder. Holds `Rc`s of the model
/// and rows (no per-frame copy) plus the snapshotted style. Every row is the
/// same fixed height (module docs); the cursor row gets the left accent bar +
/// a tint. Clicks carry only the row INDEX and resolve it through the root at
/// event time (yux rule 4).
#[allow(clippy::too_many_arguments)]
fn diff_row_renderer(
    model: Rc<DiffModel>,
    ctx_texts: Rc<HashMap<usize, Arc<FileContent>>>,
    rows: Rc<Vec<RowRef>>,
    cursor: usize,
    marks: RowMarks,
    rs: DiffRowStyle,
    root: WeakEntity<YaldaGpuiView>,
    wid: workspace::WindowId,
) -> impl Fn(usize, &mut Window, &mut GpuiApp) -> AnyElement + 'static {
    move |ix: usize, _w: &mut Window, _cx: &mut GpuiApp| -> AnyElement {
        let Some(row) = rows.get(ix).copied() else {
            return div().h(rs.row_h).into_any_element();
        };
        // Compose slots paint empty: the root paints the inline editor over
        // them (`render_diff` → `list_rows_overlay`), outside this cache.
        if row.is_compose_slot() {
            return div().w_full().h(rs.row_h).into_any_element();
        }
        let is_cursor = ix == cursor;
        let marked = marks.is_marked(ix, row);
        let is_card = row.comment_index().is_some();
        let (content, bg): (AnyElement, Option<Hsla>) = match row {
            RowRef::ComposeSlot { .. } => unreachable!("handled above"),
            RowRef::Comment {
                comment,
                part,
                parts,
                ..
            } => {
                let focused = rows.get(cursor).and_then(RowRef::comment_index) == Some(comment);
                (diff_comment_row(&marks, comment, part, parts, focused, &rs), None)
            }
            RowRef::File {
                file,
                viewed,
                collapsed,
            } => (
                diff_file_row(&model, file, viewed, collapsed, ix, &rs, root.clone(), wid),
                Some(rs.file_bg),
            ),
            RowRef::Hunk { file, hunk } => {
                let header = model
                    .files
                    .get(file)
                    .and_then(|f| f.hunks.get(hunk))
                    .map(|h| h.header.clone())
                    .unwrap_or_default();
                (diff_hunk_row(header, &rs), None)
            }
            RowRef::Line {
                file,
                hunk,
                line,
                old,
                new,
            } => {
                let dl = model
                    .files
                    .get(file)
                    .and_then(|f| f.hunks.get(hunk))
                    .and_then(|h| h.lines.get(line));
                let (sign, text, color, bg) = match dl {
                    Some(DiffLine::Added(t)) => ("+", t.as_str(), rs.add, Some(rs.add_bg)),
                    Some(DiffLine::Removed(t)) => ("−", t.as_str(), rs.remove, Some(rs.remove_bg)),
                    Some(DiffLine::Context(t)) => (" ", t.as_str(), rs.fg, None),
                    None => (" ", "", rs.fg, None),
                };
                (diff_line_row(ix, old, new, sign, text, color, &rs), bg)
            }
            RowRef::Ctx { file, old, new } => {
                let text = ctx_texts.get(&file).and_then(|c| c.line(new)).unwrap_or("");
                (diff_line_row(ix, Some(old), Some(new), " ", text, rs.fg, &rs), None)
            }
            RowRef::Expander {
                gap,
                hidden,
                kind,
                loading,
                ..
            } => (
                diff_expander_row(ix, gap, hidden, kind, loading, &rs, root.clone(), wid),
                Some(rs.expander_bg),
            ),
        };
        let bg = if marked { Some(rs.sel_bg) } else { bg };
        let transparent: Hsla = rgba(0x00000000).into();
        let click_root = root.clone();
        let mut outer = div()
            .id(("diff-row", ix))
            .flex()
            .flex_row()
            .w_full()
            .h(rs.row_h)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_ev, _w, cx| {
                if let Some(r) = click_root.upgrade() {
                    r.update(cx, |r, cx| r.diff_click_row(wid, ix, cx));
                }
            });
        if let Some(bg) = bg {
            outer = outer.bg(bg);
        }
        let el = outer
            .child(
                div()
                    .w(px(3.0))
                    .h_full()
                    .flex_none()
                    .bg(if is_cursor { rs.cursor_bar } else { transparent }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    // A focused card is marked by its accent ring, not a
                    // row tint behind the box.
                    .bg(if is_cursor && !is_card { rs.cursor_bg } else { transparent })
                    .child(content),
            )
            .into_any_element();
        let el = if is_cursor { probe_bounds("diff-cursor-row", el) } else { el };
        #[cfg(test)]
        let el = probe_bounds_dyn(format!("diff-row-{ix}"), el);
        el
    }
}

/// A file header row: fold chevron, status glyph, path, `+a −r`, and the
/// Viewed checkbox (clickable — toggles Viewed on this file, spec B4). A
/// viewed file's header is dimmed.
#[allow(clippy::too_many_arguments)]
fn diff_file_row(
    model: &DiffModel,
    fi: usize,
    viewed: bool,
    collapsed: bool,
    ix: usize,
    rs: &DiffRowStyle,
    root: WeakEntity<YaldaGpuiView>,
    wid: workspace::WindowId,
) -> AnyElement {
    let Some(file) = model.files.get(fi) else {
        return div().into_any_element();
    };
    let (glyph, glyph_color) = match &file.status {
        FileStatus::Modified => ("M", rs.accent),
        FileStatus::Added => ("A", rs.add),
        FileStatus::Deleted => ("D", rs.remove),
        FileStatus::Renamed { .. } => ("R", rs.header),
    };
    let path = match &file.status {
        FileStatus::Renamed { from } => format!("{} → {}", from.display(), file.path.display()),
        _ => file.path.display().to_string(),
    };
    let path_color = if viewed { rs.dim } else { rs.fg };
    let mut box_border = rs.dim;
    box_border.a *= if viewed { 0.0 } else { 0.9 };
    let mut checkbox = div()
        .id(("diff-checkbox", ix))
        .flex_none()
        .size(px(15.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(box_border)
        .font_family(rs.mono.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(11.0))
        .cursor_pointer()
        .child(SharedString::from(if viewed { "✓" } else { "" }))
        .on_mouse_down(MouseButton::Left, move |_ev, _w, cx| {
            cx.stop_propagation();
            if let Some(r) = root.upgrade() {
                r.update(cx, |r, cx| r.diff_click_checkbox(wid, ix, cx));
            }
        });
    if viewed {
        checkbox = checkbox.bg(rs.add).text_color(rgb(0x1e1f29));
    }
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .size_full()
        .pl(px(6.0))
        .pr(px(10.0))
        .font_family(rs.mono.clone())
        .text_size(rs.text)
        .child(
            div()
                .flex_none()
                .w(px(12.0))
                .text_color(rs.dim)
                .text_size(rs.small)
                .child(SharedString::from(if collapsed { "▸" } else { "▾" })),
        )
        .child(
            div()
                .flex_none()
                .w(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(if viewed { rs.dim } else { glyph_color })
                .child(SharedString::from(glyph)),
        )
        .child(
            single_line_ellipsis(&path)
                .flex_1()
                .font_weight(if viewed { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
                .text_color(path_color),
        )
        .child(
            div()
                .flex_none()
                .text_size(rs.small)
                .text_color(if viewed { rs.dim } else { rs.add })
                .child(SharedString::from(format!("+{}", file.added))),
        )
        .child(
            div()
                .flex_none()
                .text_size(rs.small)
                .text_color(if viewed { rs.dim } else { rs.remove })
                .child(SharedString::from(format!("−{}", file.removed))),
        )
        .child(probe_bounds_dyn(format!("diff-checkbox-{fi}"), checkbox.into_any_element()))
        .into_any_element()
}

/// A thin hunk header row: the `@@ -a,b +c,d @@ ctx` line, dimmed and small,
/// indented past the gutters.
fn diff_hunk_row(header: String, rs: &DiffRowStyle) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .size_full()
        .pl(rs.gutter_w * 2.0 + px(22.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .font_family(rs.mono.clone())
        .text_size(rs.small)
        .text_color(rs.header.opacity(0.75))
        .child(SharedString::from(header))
        .into_any_element()
}

/// One diff line: old / new line-number gutters (dimmed, fixed width), the
/// `+`/`−` sign, then the text (no wrap — every row is one fixed height).
/// The gutters and text are `probe_text` leaves (`diff-row-<ix>-old` /
/// `-new` / `-text`) so a test reads the SHAPED numbers and code.
fn diff_line_row(
    ix: usize,
    old: Option<u32>,
    new: Option<u32>,
    sign: &'static str,
    text: &str,
    color: Hsla,
    rs: &DiffRowStyle,
) -> AnyElement {
    let gutter = |n: Option<u32>, side: &'static str| {
        div()
            .flex_none()
            .w(rs.gutter_w)
            .pr(px(6.0))
            .text_right()
            .text_size(rs.small)
            .text_color(rs.dim.opacity(0.7))
            .child(probe_text(
                || format!("diff-row-{ix}-{side}"),
                SharedString::from(n.map(|n| n.to_string()).unwrap_or_default()),
            ))
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .size_full()
        .font_family(rs.mono.clone())
        .text_size(rs.text)
        .child(gutter(old, "old"))
        .child(gutter(new, "new"))
        .child(
            div()
                .flex_none()
                .w(px(16.0))
                .text_center()
                .text_color(color)
                .child(SharedString::from(sign)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(if sign == " " { rs.fg } else { color })
                .child(probe_text(|| format!("diff-row-{ix}-text"), SharedString::from(text.to_string()))),
        )
        .into_any_element()
}

/// "↑ 20 more lines" / "↓ 20 more lines" / "Show all 37 hidden lines" —
/// an expander's clickable segments (spec B2a): top gap ↑ + all; between
/// ↓ ↑ + all; bottom ↓ (+ all once the count is known); fewer than
/// [`EXPAND_ALL_UNDER`] hidden ⇒ just "Show all". Pure.
pub(crate) fn expander_segments(kind: GapKind, hidden: Option<u32>) -> Vec<(ExpandDir, String)> {
    let all = |h: u32| {
        let s = if h == 1 { "" } else { "s" };
        (ExpandDir::All, format!("Show all {h} hidden line{s}"))
    };
    let step = |dir: ExpandDir| {
        let arrow = if dir == ExpandDir::Up { "↑" } else { "↓" };
        (dir, format!("{arrow} {EXPAND_STEP} more lines"))
    };
    match (kind, hidden) {
        (_, Some(h)) if h < EXPAND_ALL_UNDER => vec![all(h)],
        (GapKind::Top, Some(h)) => vec![step(ExpandDir::Up), all(h)],
        (GapKind::Between, Some(h)) => vec![step(ExpandDir::Down), step(ExpandDir::Up), all(h)],
        (GapKind::Bottom, Some(h)) => vec![step(ExpandDir::Down), all(h)],
        (_, None) => vec![step(ExpandDir::Down)],
    }
}

/// A context expander row (spec B2a, UXI-Diff-18): slim, full width, a
/// subdued tint (set by the caller), a dotted gutter, then its segments —
/// each one clickable (the handler carries only the row index + direction,
/// resolved at event time via `diff_click_expander`, yux rule 4). While the
/// file's text loads it reads "Loading…". Probes: `diff-expander-<ix>-<dir>`
/// (segment box) and `diff-expander-text-<ix>-<dir>` (shaped label).
#[allow(clippy::too_many_arguments)]
fn diff_expander_row(
    ix: usize,
    gap: u32,
    hidden: Option<u32>,
    kind: GapKind,
    loading: bool,
    rs: &DiffRowStyle,
    root: WeakEntity<YaldaGpuiView>,
    wid: workspace::WindowId,
) -> AnyElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .size_full()
        .overflow_hidden()
        .font_family(rs.mono.clone())
        .text_size(rs.small)
        .text_color(rs.header)
        .child(
            div()
                .flex_none()
                .w(rs.gutter_w * 2.0 + px(16.0))
                .text_center()
                .text_color(rs.dim)
                .child(SharedString::from("⋯")),
        );
    if loading {
        return row
            .child(div().text_color(rs.dim).child(SharedString::from("Loading…")))
            .into_any_element();
    }
    let probing = layout_probe_active();
    for (dir, label) in expander_segments(kind, hidden) {
        let click_root = root.clone();
        let seg = div()
            .id(SharedString::from(format!("diff-expander-{gap}-{}", dir.slug())))
            .flex_none()
            .px(px(6.0))
            .rounded(px(3.0))
            .cursor_pointer()
            .hover(|s| s.bg(rs.expander_hover_bg))
            .child(probe_text(
                || format!("diff-expander-text-{ix}-{}", dir.slug()),
                SharedString::from(label),
            ))
            .on_mouse_down(MouseButton::Left, move |_ev, _w, cx| {
                cx.stop_propagation();
                if let Some(r) = click_root.upgrade() {
                    r.update(cx, |r, cx| r.diff_click_expander(wid, ix, dir, cx));
                }
            })
            .into_any_element();
        row = row.child(if probing {
            probe_bounds_dyn(format!("diff-expander-{ix}-{}", dir.slug()), seg)
        } else {
            seg
        });
    }
    row.into_any_element()
}

/// Per-render row decorations beyond the cursor (snapshotted into the
/// `'static` row closure): the `V` selection, the open compose's anchor
/// (file index, side, line span — resolved by line numbers so it survives a
/// rows rebuild), and the review's comments for the cards.
struct RowMarks {
    selection: Option<(usize, usize)>,
    anchor: Option<(usize, CommentSide, [usize; 2])>,
    cards: Rc<CommentCards>,
    now: chrono::DateTime<chrono::Utc>,
}

impl RowMarks {
    /// A `Line` row inside the `V` selection or the compose's anchor span.
    fn is_marked(&self, ix: usize, row: RowRef) -> bool {
        let Some((old, new)) = row.line_numbers() else {
            return false;
        };
        let file = row.file();
        if self.selection.is_some_and(|(lo, hi)| (lo..=hi).contains(&ix)) {
            return true;
        }
        let Some((f, side, [lo, hi])) = self.anchor else {
            return false;
        };
        let n = match side {
            CommentSide::New => new,
            CommentSide::Old if new.is_none() => old,
            CommentSide::Old => None,
        };
        file == f && n.is_some_and(|n| (lo..=hi).contains(&(n as usize)))
    }
}

/// A small rounded status pill (comment card header): tinted fill, colored
/// mono text. Its text is a `probe_text` leaf (`tag`) so a test can read the
/// SHAPED label.
fn card_pill(tag: impl FnOnce() -> String, text: String, color: Hsla, fill: Hsla, rs: &DiffRowStyle) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .px(px(7.0))
        .rounded_full()
        .bg(fill)
        .font_family(rs.mono.clone())
        .text_size(rs.small)
        .text_color(color)
        .whitespace_nowrap()
        .child(probe_text(tag, SharedString::from(text)))
}

/// One row of an inline comment card (spec B5, UXI-Diff-15). A card is ONE
/// visually-boxed block across its `parts` fixed-height rows
/// ([`comment_card_lines`]): row 0 is the header strip (top edge + rounded
/// top corners: an id pill, a status pill — `unsent` / `sent to <label> ·
/// <age>` / `outdated` — and, when focused, the `e`/`x` hints), then the body
/// in the prose font (an outdated card appends its snippet, dimmed mono), then
/// a footer row (bottom padding + bottom edge + rounded bottom corners).
///
/// The border is a real 1px ring: an OUTER frame filled with the border color,
/// padded 1px on the box's edges for this row (sides always, top on the
/// header, bottom on the footer), around an opaque INNER fill — so contiguous
/// rows join into one outline and the ring is testable geometry (probes
/// `diff-card-<id>-<part>` / `…-in`). Neutral ring; accent while the cursor is
/// on the card; red-tinted when outdated. No emoji.
fn diff_comment_row(marks: &RowMarks, ci: usize, part: u8, parts: u8, focused: bool, rs: &DiffRowStyle) -> AnyElement {
    let Some(c) = marks.cards.comments.get(ci) else {
        return div().into_any_element();
    };
    // E1/E2: the per-`review_gen` snapshot's pre-wrapped lines — no re-wrap
    // per row per frame.
    let line = marks
        .cards
        .lines
        .get(ci)
        .and_then(|l| l.get(part as usize))
        .cloned()
        .unwrap_or(CardLine::Footer);
    let (first, last) = (part == 0, part + 1 >= parts);
    let ring = if focused {
        rs.accent
    } else if c.outdated {
        rs.card_outdated_border
    } else {
        rs.card_border
    };
    let (r_out, r_in) = (px(DIFF_CARD_RADIUS), px(DIFF_CARD_RADIUS - 1.0));
    let gap = px(DIFF_CARD_GAP);

    let mut inner = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .flex_1()
        .min_h_0()
        .w_full()
        .px(px(12.0))
        .overflow_hidden()
        .bg(if first { rs.card_header_bg } else { rs.card_bg });
    if first {
        inner = inner.rounded_t(r_in);
        if parts > 2 {
            inner = inner.border_b_1().border_color(rs.card_border);
        }
    }
    if last {
        inner = inner.rounded_b(r_in);
    }
    let text_el = |t: String, color: Hsla, mono: bool| {
        let el = single_line_ellipsis(&t).flex_1().text_color(color);
        if mono {
            el.font_family(rs.mono.clone()).text_size(rs.small)
        } else {
            el.font_family(rs.prose.clone()).text_size(rs.text)
        }
    };
    let id = c.id.clone();
    let inner = match line {
        CardLine::Header => {
            let (status, color) = if c.outdated {
                ("outdated".to_string(), rs.remove)
            } else if let Some(s) = c.sent.last() {
                let who = if s.label.is_empty() { &s.session } else { &s.label };
                (format!("sent to {who} · {}", relative_time(&s.at, marks.now)), rs.add)
            } else {
                ("unsent".to_string(), rs.accent)
            };
            let mut header = inner
                .child(card_pill(
                    || format!("diff-card-text-{id}-id"),
                    c.id.clone(),
                    rs.fg,
                    rs.dim.opacity(0.22),
                    rs,
                ))
                .child(card_pill(
                    || format!("diff-card-text-{id}-status"),
                    status,
                    color,
                    color.opacity(0.16),
                    rs,
                ));
            if c.outdated {
                header = header.child(text_el("code changed since this comment".to_string(), rs.dim, false));
            } else {
                header = header.child(div().flex_1());
            }
            if focused {
                header = header.child(
                    div()
                        .flex_none()
                        .font_family(rs.mono.clone())
                        .text_size(rs.small)
                        .text_color(rs.dim)
                        .child(SharedString::from("e edit · x delete")),
                );
            }
            header
        }
        CardLine::Body(t) => inner.child(text_el(t, rs.fg, false)),
        CardLine::Snippet(t) => inner.child(text_el(t, rs.dim, true)),
        CardLine::More => inner.child(text_el("…".to_string(), rs.dim, false)),
        CardLine::Footer => inner,
    };

    let mut frame = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .ml(rs.gutter_w * 2.0 + px(16.0))
        .mr(px(DIFF_CARD_INSET_RIGHT))
        .bg(ring)
        .px(px(1.0));
    if first {
        frame = frame.mt(gap).pt(px(1.0)).rounded_t(r_out);
    }
    if last {
        frame = frame.mb(gap).pb(px(1.0)).rounded_b(r_out);
    }
    let probing = layout_probe_active();
    let inner = if probing {
        probe_bounds_dyn(format!("diff-card-{id}-{part}-in"), inner.into_any_element())
    } else {
        inner.into_any_element()
    };
    let frame = frame.child(inner).into_any_element();
    let frame = if probing { probe_bounds_dyn(format!("diff-card-{id}-{part}"), frame) } else { frame };
    let row = div().flex().flex_row().size_full().child(frame).into_any_element();
    if first { probe_bounds_dyn(format!("diff-comment-{id}"), row) } else { row }
}

#[cfg(test)]
mod picker_description_tests {
    use super::*;

    #[test]
    fn relative_age_buckets() {
        const DAY: i64 = 86_400;
        assert_eq!(relative_age(-5), "just now", "clock skew clamps");
        assert_eq!(relative_age(0), "just now");
        assert_eq!(relative_age(59), "just now");
        assert_eq!(relative_age(60), "1m ago");
        assert_eq!(relative_age(3_599), "59m ago");
        assert_eq!(relative_age(3_600), "1h ago");
        assert_eq!(relative_age(DAY - 1), "23h ago");
        assert_eq!(relative_age(DAY), "1d ago");
        assert_eq!(relative_age(13 * DAY), "13d ago");
        assert_eq!(relative_age(14 * DAY), "2w ago");
        assert_eq!(relative_age(59 * DAY), "8w ago");
        assert_eq!(relative_age(60 * DAY), "2mo ago");
        assert_eq!(relative_age(364 * DAY), "12mo ago");
        assert_eq!(relative_age(365 * DAY), "1y ago");
        assert_eq!(relative_age(800 * DAY), "2y ago");
    }

    fn entry(head_commit: Option<HeadCommit>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from("/x/wt"),
            head: "abc".into(),
            branch: Some("topic".into()),
            detached: false,
            is_primary: false,
            head_commit,
        }
    }

    #[test]
    fn worktree_row_description_is_subject_age_path() {
        let now = 1_700_000_000;
        let c = |subject: &str| Some(HeadCommit { subject: subject.into(), time: now - 7_200 });
        assert_eq!(
            worktree_row_description(&entry(c("feat(diff): send picker")), "~/ws/wt", now),
            "feat(diff): send picker · 2h ago · ~/ws/wt"
        );
        assert_eq!(worktree_row_description(&entry(c("  ")), "~/ws/wt", now), "2h ago · ~/ws/wt");
        assert_eq!(worktree_row_description(&entry(None), "~/ws/wt", now), "~/ws/wt");
    }
}
