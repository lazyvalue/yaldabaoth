//! `EditBodyView` — the cached body of a Buffer tile in `Editing` mode (the
//! Code + WordProcessor rows). Text-editing review C10: the body used to be an
//! inline element tree rebuilt on EVERY root notify (typing in another tile,
//! agent streaming, a menu opening…), paying a full visible-rows rebuild each
//! time. Now it is a yux component (see `yux/CLAUDE.md`) embedded via
//! `cached_child` in `render_edit` (`screens.rs`).
//!
//! # Why this observes the ROOT (the `DiffView` shape)
//!
//! `EditState` is a plain struct living in the workspace layout tree, not a
//! GPUI entity, and the text lives in the pooled `SharedCore` (an `Rc`, shared
//! with sibling tiles on the same file). The only entity it is reachable
//! through is the root view, so — exactly like `DiffView` — this view observes
//! the root and filters every notify through a cheap fingerprint
//! ([`EditSeqs`]). Global inputs (theme, zoom, fonts) live on that same root, so
//! they fall out of the fingerprint without a separate `notify_*_views` walk
//! (the `DiffView` deviation from the transcript precedent; guarded by the
//! render-count tests in `verify_harness.rs`).
//!
//! # State ownership
//!
//! The view READS `EditState` (editor/cursor/selection/mode/view kind) off the
//! root and OWNS the body's UI state + render caches: the virtualized row list
//! (scroll), the tab-expanded line snapshot, the incremental highlight cache,
//! the WP line kinds, and the caret-reveal key.
//!
//! The body has no pointer listeners, so there is no captured-row-data hazard
//! (yux rule 4) — if click-to-caret is ever added, resolve the tile through
//! `root` + `window_id` in the handler and map the column with
//! `raw_col_from_display`.

use super::*;

/// The render-input watermark the root-observe filter compares across
/// renders. EVERY input the body reads must be covered here (yux rule 2).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct EditSeqs {
    /// Identity of the pooled core (a rebind to a different buffer).
    core_ptr: usize,
    /// The core's monotonic content generation (own AND sibling edits, reload).
    edit_seq: u64,
    /// Raw caret `(line, col)` — the caret glyph + reveal.
    cursor: (usize, usize),
    /// Raw selection range — the selection band.
    selection: Option<((usize, usize), (usize, usize))>,
    /// Normal vs Insert — the caret glyph.
    insert: bool,
    /// Code vs WordProcessor body.
    wp: bool,
    /// Whether this tile is the focused one — gates the caret reveal (C8).
    focused: bool,
    /// Global zoom (`text_scale.to_bits()`).
    text_scale_bits: u32,
    /// Global theme (colors + the syntect highlighter follow it).
    theme: yalda::theme::ThemeName,
    /// Hash of the code/body font family names.
    fonts: u64,
}

impl EditSeqs {
    pub(crate) fn of(e: &EditState, focused: bool, root: &YaldaGpuiView) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        root.code_font.hash(&mut h);
        root.body_font.hash(&mut h);
        let cursor = e.editor.cursor();
        EditSeqs {
            core_ptr: std::rc::Rc::as_ptr(&e.editor.core) as *const () as usize,
            edit_seq: e.editor.edit_seq(),
            cursor: (cursor.line, cursor.col),
            selection: e.editor.selection_range(),
            insert: matches!(e.mode, EditMode::Insert),
            wp: matches!(e.view, EditView::WordProcessor),
            focused,
            text_scale_bits: root.text_scale.to_bits(),
            theme: root.theme.name,
            fonts: h.finish(),
        }
    }
}

/// The cached Edit body. One per Editing tile, owned by its `EditState`
/// (`body`), dropped with it — no registry.
pub(crate) struct EditBodyView {
    root: WeakEntity<YaldaGpuiView>,
    /// The tile this body belongs to — how `render` finds its `EditState`.
    window_id: workspace::WindowId,
    last_rendered: EditSeqs,
    /// Incremental per-line highlight cache, keyed on the document's
    /// `edit_seq`. Re-highlights only changed lines instead of the whole
    /// buffer, so typing stays O(changed). Shared by Code + WP (both consume
    /// each `LineHl`'s `raw` segments).
    pub(crate) highlight_cache: HighlightCache,
    /// `edit_seq` the `lines_cache` was extracted at; `u64::MAX` = never built.
    lines_cache_seq: u64,
    /// Last extracted (tab-expanded, newline-trimmed) source lines.
    lines_cache: std::rc::Rc<Vec<String>>,
    /// Virtualized line list — only the visible rows are built/laid-out.
    /// Reconciled by splicing the changed range (never `reset()`) so scroll
    /// stays anchored across edits — see `ScrollAnchoredList`.
    pub(crate) list: ScrollAnchoredList<String>,
    /// `(edit_seq, cursor_line, cursor_col)` at the last reveal decision. The
    /// COLUMN is in the key because a horizontal move along a wide soft-wrapped
    /// line changes the caret's visual row without changing its line. C8: the
    /// key only REVEALS while this tile is focused — a sibling tile editing the
    /// same buffer moves `edit_seq` (and may shift this tile's caret) but must
    /// not yank this tile's scroll back to its own caret.
    last_cursor_anchor: Option<(u64, usize, usize)>,
    /// Per-line WordProcessor kinds, cached on `edit_seq` (C2 incremental).
    wp_kinds_cache: std::rc::Rc<Vec<WpLineKind>>,
    wp_kinds_cache_seq: u64,
    /// The source lines `wp_kinds_cache` was classified from.
    wp_kinds_lines: std::rc::Rc<Vec<String>>,
    perf_label: &'static str,
}

impl EditBodyView {
    /// Construct the body and register the root-observe subscription that
    /// self-notifies only when [`EditSeqs`] moved (mirrors `DiffView::new`).
    pub(crate) fn new(
        root: Entity<YaldaGpuiView>,
        window_id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&root, |this: &mut EditBodyView, root_ent, cx| {
            let now = root_ent.read(cx).edit_seqs_for(this.window_id);
            if this.last_rendered != now {
                record_notify(this.perf_label, MissReason::Dirtied);
                cx.notify();
            }
        })
        .detach();
        EditBodyView {
            root: root.downgrade(),
            window_id,
            last_rendered: EditSeqs::default(),
            highlight_cache: HighlightCache::new(),
            lines_cache_seq: u64::MAX,
            lines_cache: std::rc::Rc::new(Vec::new()),
            // Top-aligned: editing reads from the top of the buffer, unlike the
            // agent transcript which tails the bottom.
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, gpui::px(256.0)),
            last_cursor_anchor: None,
            wp_kinds_cache: std::rc::Rc::new(Vec::new()),
            wp_kinds_cache_seq: u64::MAX,
            wp_kinds_lines: std::rc::Rc::new(Vec::new()),
            perf_label: "edit-body",
        }
    }

    /// Reconcile the row list to `lines` by splicing ONLY the changed range,
    /// then reveal the caret's line (UXI-TextEditing-1) — but only for THIS
    /// tile's own moves/edits (C8: gated on `focused`; the very first render
    /// also reveals so a freshly opened tile shows its caret). Returns the
    /// reconciled row count.
    fn reconcile_and_reveal(
        &mut self,
        lines: &std::rc::Rc<Vec<String>>,
        edit_seq: u64,
        cursor_line: usize,
        cursor_col: usize,
        focused: bool,
    ) -> usize {
        self.list.reconcile(lines, edit_seq);
        let new_count = self.list.len();
        let anchor = (edit_seq, cursor_line, cursor_col);
        if self.last_cursor_anchor != Some(anchor) {
            let first = self.last_cursor_anchor.is_none();
            self.last_cursor_anchor = Some(anchor);
            if (focused || first) && cursor_line < new_count {
                self.list.state().scroll_to_reveal_item(cursor_line);
            }
        }
        new_count
    }

    /// Per-line WordProcessor kinds, cached on `edit_seq`: the fence-carrying
    /// fold runs once per edit (incrementally, C2), never per frame.
    fn wp_kinds_snapshot(
        &mut self,
        lines: &std::rc::Rc<Vec<String>>,
        edit_seq: u64,
    ) -> std::rc::Rc<Vec<WpLineKind>> {
        if self.wp_kinds_cache_seq != edit_seq {
            let kinds = wp_kinds_incremental(&self.wp_kinds_lines, &self.wp_kinds_cache, lines);
            self.wp_kinds_cache = std::rc::Rc::new(kinds);
            self.wp_kinds_lines = lines.clone();
            self.wp_kinds_cache_seq = edit_seq;
        }
        self.wp_kinds_cache.clone()
    }

    /// Extract + highlight the buffer's source lines incrementally, keyed on
    /// the document's `edit_seq` (a frame that didn't edit recomputes zero
    /// lines; a single-char edit ~1).
    fn highlight_snapshot(
        &mut self,
        editor: &SharedEditor,
        theme: &Theme,
        hl: &yalda::highlight::Highlighter,
    ) -> (
        std::rc::Rc<Vec<String>>,
        std::rc::Rc<Vec<std::rc::Rc<LineHl>>>,
    ) {
        let edit_seq = editor.edit_seq();
        let lines_rc: std::rc::Rc<Vec<String>> = if self.lines_cache_seq == edit_seq {
            self.lines_cache.clone()
        } else {
            let core = editor.core.borrow();
            let built: Vec<String> = display_lines(core.document());
            drop(core);
            let rc = std::rc::Rc::new(built);
            self.lines_cache = rc.clone();
            self.lines_cache_seq = edit_seq;
            rc
        };
        // Buffer highlight: one document, no agent-turn boundaries.
        let snap = self
            .highlight_cache
            .snapshot_syn(&lines_rc, theme, edit_seq, &[], hl);
        (lines_rc, snap)
    }
}

#[cfg(test)]
mod gutter_tests {
    use super::*;

    #[test]
    fn gutter_grows_with_digits_and_scales_with_char_width() {
        assert_eq!(decimal_digits(0), 1);
        assert_eq!(decimal_digits(9), 1);
        assert_eq!(decimal_digits(10), 2);
        assert_eq!(decimal_digits(10_005), 5);
        // Min 3 digit columns + 1 gap column.
        assert_eq!(edit_gutter_geometry(12, px(8.0)), (3, px(33.0)));
        assert_eq!(edit_gutter_geometry(10_005, px(8.0)), (5, px(49.0)));
        assert_eq!(edit_gutter_geometry(12, px(16.0)), (3, px(65.0)));
    }
}

/// Number of decimal digits in `n` (≥ 1).
pub(crate) fn decimal_digits(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// C11: gutter geometry for a `line_count`-line buffer — `(digit columns,
/// width)`. Numbers right-align in `digits` columns (min 3, the historical
/// look) plus one trailing gap column; the width is those columns × the code
/// font's advance at the zoomed text size, so it scales with zoom and a
/// 5+-digit line number is never clipped. Rounded up + 1px so glyph-position
/// rounding in text layout can't push the last digit past the edge.
pub(crate) fn edit_gutter_geometry(line_count: usize, char_w: Pixels) -> (usize, Pixels) {
    let digits = decimal_digits(line_count).max(3);
    let w = (f32::from(char_w) * (digits + 1) as f32).ceil() + 1.0;
    (digits, px(w))
}

impl Render for EditBodyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let r = root_ent.read(cx);
        let Some(e) = r.edit_state_ref(self.window_id) else {
            return div().size_full().into_any_element();
        };
        let focused = r.workspace.focused_window_id() == Some(self.window_id);
        self.last_rendered = EditSeqs::of(e, focused, r);
        let body = match e.view {
            EditView::Code => self.build_code(r, e, focused, window).into_any_element(),
            EditView::WordProcessor => self.build_wp(r, e, focused).into_any_element(),
        };
        // An Edit landed from a Doc (UXI-Buffer-8) re-checks, against the last
        // layout, that the caret line painted on-screen (the build above
        // reconciled the list); the follow-up frame is scheduled via defer —
        // never a notify inside render.
        if self.list.settle() {
            let me = cx.entity_id();
            cx.defer(move |app| app.notify(me));
        }
        body
    }
}

impl EditBodyView {
    /// Code (raw markdown) view: monospace, gutter with line numbers,
    /// per-line `md_highlight` source colors. Lines soft-wrap and the cursor
    /// splices inline via the shared `build_wrapped_line` helper.
    ///
    /// **Virtualized**: rendered through a `gpui::list` so only the visible
    /// rows are built/laid-out, and (C10) only when this body's own inputs
    /// moved — a root notify from elsewhere is a cache hit.
    fn build_code(
        &mut self,
        r: &YaldaGpuiView,
        e: &EditState,
        focused: bool,
        window: &mut Window,
    ) -> gpui::Stateful<gpui::Div> {
        // C4: caret + selection in DISPLAY columns (rows are tab-expanded).
        let (cursor, sel) = e.editor.display_caret_and_selection();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let dim_fg: Hsla = rgb(0x6272a4).into();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        let (lines_rc, hl_snap) = self.highlight_snapshot(&e.editor, &r.theme, &r.syntect_hl);
        let line_count =
            self.reconcile_and_reveal(&lines_rc, edit_seq, cursor_line, cursor_col, focused);

        let base_style = r.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let code_font = r.code_font.clone();
        let editor_fg = r.editor_fg();
        let selection_bg = r.theme.agent.selection_bg;
        #[cfg(test)]
        let code_block_style = r.theme.code_block_bg;
        let text_size = px(14.0 * r.text_scale);

        // C11: gutter = (digits + 1) columns of the code font's real advance at
        // the zoomed size (falls back to the 0.6em monospace estimate).
        let char_w = {
            let ts = window.text_system();
            let fid = ts.resolve_font(&gpui::font(code_font.clone()));
            ts.em_advance(fid, text_size).unwrap_or(text_size * 0.6)
        };
        let (gutter_digits, gutter_w) = edit_gutter_geometry(line_count, char_w);

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                segs =
                    apply_line_selection(&segs, &line_str, sel, line_idx, base_style, selection_bg);
            }

            #[cfg(test)]
            crate::screens::push_edit_render_line(line_idx, &line_str, &segs, code_block_style);

            let number =
                div()
                    .flex_none()
                    .child(format!("{:>w$} ", line_idx + 1, w = gutter_digits));
            #[cfg(test)]
            let number = probe_bounds_dyn(
                format!("edit-gutter-num-{line_idx}"),
                number.into_any_element(),
            );
            let gutter = div()
                .w(gutter_w)
                .flex_none()
                .flex()
                .text_color(dim_fg)
                .child(number);
            #[cfg(test)]
            let gutter =
                probe_bounds_dyn(format!("edit-gutter-{line_idx}"), gutter.into_any_element());

            // Soft-wrap: long lines break at whitespace and stack below the
            // gutter rather than running off the right edge.
            let content = build_wrapped_line(
                &segs,
                &line_str,
                line_idx == cursor_line,
                cursor_col,
                mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                &code_font,
                &code_font,
                None,
                None,
                line_idx,
                None,
            );

            let row = div()
                .flex()
                .flex_row()
                // Fill the list width so `content`'s `flex_1` has a bounded
                // space to soft-wrap within (`gpui::list` rows don't stretch).
                .w_full()
                .child(gutter)
                .child(content);
            #[cfg(test)]
            let row = probe_bounds_dyn(format!("code-line-{line_idx}"), row.into_any_element());
            row.into_any_element()
        };

        div()
            .id("edit-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            // Clip the rare unbroken token instead of letting it widen the row.
            .overflow_x_hidden()
            .px_4()
            .py_2()
            .text_size(text_size)
            .font_family(r.code_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(self.list.state().clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }

    /// Word-Processor view: proportional body font + per-line typographic
    /// styling driven by `classify_wp_line`. No gutter.
    fn build_wp(
        &mut self,
        r: &YaldaGpuiView,
        e: &EditState,
        focused: bool,
    ) -> gpui::Stateful<gpui::Div> {
        // C4: caret + selection in DISPLAY columns (rows are tab-expanded).
        let (cursor, sel) = e.editor.display_caret_and_selection();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        let (lines_rc, hl_snap) = self.highlight_snapshot(&e.editor, &r.theme, &r.syntect_hl);
        // Per-line typographic kind, cached on `edit_seq` (012).
        let kinds = self.wp_kinds_snapshot(&lines_rc, edit_seq);
        self.reconcile_and_reveal(&lines_rc, edit_seq, cursor_line, cursor_col, focused);

        let base_style = r.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let body_font = r.body_font.clone();
        let code_font = r.code_font.clone();
        let editor_fg = r.editor_fg();
        let selection_bg = r.theme.agent.selection_bg;
        let text_scale = r.text_scale;
        // Code-line bg follows the active theme. See `wp_code_block_bg`.
        let wp_code_bg = wp_code_block_bg(&r.theme);

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let kind = kinds
                .get(line_idx)
                .copied()
                .unwrap_or(WpLineKind::Paragraph);

            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                segs =
                    apply_line_selection(&segs, &line_str, sel, line_idx, base_style, selection_bg);
            }

            // Headings follow the shared type scale (`yux/typography.rs`) —
            // the same size / leading / space-above the Doc view renders.
            let (raw_size_px, font_weight, top_pad) = match kind {
                WpLineKind::Heading(l) => (
                    TYPE_SCALE.heading(l),
                    FontWeight::BOLD,
                    TYPE_SCALE.heading_space_above(l),
                ),
                WpLineKind::CodeFence | WpLineKind::CodeContent => (13.0, FontWeight::NORMAL, 0.0),
                WpLineKind::TableRow => (13.0, FontWeight::NORMAL, 0.0),
                // UXI-ParagraphSpacing-1: list items get a readability gap above.
                WpLineKind::BulletItem | WpLineKind::OrderedItem => {
                    (14.0, FontWeight::NORMAL, PARAGRAPH_GAP_PX)
                }
                _ => (14.0, FontWeight::NORMAL, 0.0),
            };
            let text_size_px = raw_size_px * text_scale;
            let line_font = match kind {
                WpLineKind::CodeFence | WpLineKind::CodeContent | WpLineKind::TableRow => {
                    &code_font
                }
                _ => &body_font,
            };

            let content = build_wrapped_line(
                &segs,
                &line_str,
                line_idx == cursor_line,
                cursor_col,
                mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                line_font,
                &code_font,
                // C5: exclude the selection bg from the inline-code font proxy.
                sel.map(|_| selection_bg),
                None,
                line_idx,
                None,
            );

            // Test seam: a WP heading line's painted text box (excludes the
            // row's space-above) — compared against the Doc's heading.
            #[cfg(test)]
            let content = if matches!(kind, WpLineKind::Heading(_)) {
                probe_bounds_dyn(format!("wp-heading-{line_idx}"), content.into_any_element())
            } else {
                content.into_any_element()
            };
            let line_div = match kind {
                WpLineKind::Blockquote => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad * text_scale))
                    .italic()
                    .text_color(rgb(0xbfbfbf))
                    .child(div().w(px(3.0)).bg(rgb(0xffb86c)).mr_2())
                    .child(content),
                WpLineKind::CodeFence | WpLineKind::CodeContent => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .px_2()
                    .py_0p5()
                    .bg(wp_code_bg)
                    .child(content),
                WpLineKind::Empty => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    // UXI-ParagraphSpacing-1: blank line carries the gap; scaled.
                    .h(px(18.0 * text_scale) + paragraph_gap(text_scale))
                    .child(content),
                WpLineKind::Heading(_) => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .line_height(TYPE_SCALE.heading_leading())
                    .font_weight(font_weight)
                    .pt(px(top_pad * text_scale))
                    .child(content),
                _ => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad * text_scale))
                    .child(content),
            };

            line_div.w_full().into_any_element()
        };

        div()
            .id("edit-body-wp")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_x_hidden()
            .px_8()
            .py_4()
            .text_size(px(14.0 * r.text_scale))
            .font_family(r.body_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(self.list.state().clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }
}

impl YaldaGpuiView {
    /// The `EditState` of the Editing tile `id` (None if gone / not editing).
    pub(crate) fn edit_state_ref(&self, id: workspace::WindowId) -> Option<&EditState> {
        match &self.workspace.tile(id)?.content {
            App::Buffer(BufferApp::Editing(e)) => Some(e),
            _ => None,
        }
    }

    /// The live render-input fingerprint for the Editing tile `id` (the
    /// `EditBodyView` root-observe filter). Default for a vanished tile.
    pub(crate) fn edit_seqs_for(&self, id: workspace::WindowId) -> EditSeqs {
        let focused = self.workspace.focused_window_id() == Some(id);
        match self.edit_state_ref(id) {
            Some(e) => EditSeqs::of(e, focused, self),
            None => EditSeqs::default(),
        }
    }
}
