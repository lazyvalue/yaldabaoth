//! Screen render bodies on YaldaGpuiView: render_doc / render_edit
//! (Code + WP) / render_agent / render_browser. Extracted verbatim from
//! main.rs (split-gpui-main, stage 3).

use super::*;

/// Process cwd, read once and cached for the process lifetime.
///
/// `render_agent` compares each session's per-slot cwd against the process cwd
/// every frame (cursor blink, streamed chunk, cross-tile wakeup). The underlying
/// `process_cwd()` (persist.rs) does a `getcwd(2)` syscall on every call — an
/// O(1) but non-trivial per-frame cost on the paint thread. The cwd is
/// process-stable (Yalda never `chdir`s after launch), so a one-time read is
/// correct. Mirrors the `perf_enabled()` OnceLock idiom (main.rs:130); the
/// static is intentionally defined here, local to screens.rs.
fn cached_process_cwd() -> &'static std::path::Path {
    use std::sync::OnceLock;
    static CWD: OnceLock<std::path::PathBuf> = OnceLock::new();
    CWD.get_or_init(process_cwd).as_path()
}

impl YaldaGpuiView {
    pub(crate) fn render_doc(
        &self,
        root: gpui::Div,
        d: &DocState,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Clear the per-render layout sink before re-emitting lines. Mouse
        // hit-testing reads this map between renders, so stale entries from
        // a now-removed block would otherwise leak through.
        self.line_layouts.borrow_mut().clear();
        // Resolve the focused doc's directory for wiki link targets.
        // `file_label` is the canonicalized path of the file backing this
        // Doc — its parent dir is where `[[name]]` lookups start. `None`
        // if the doc has no parent (e.g., root-level untitled buffer).
        let doc_dir = {
            let path = PathBuf::from(d.file_label.as_ref());
            path.parent().map(|p| p.to_path_buf())
        };
        // ---- Virtualized doc body (audit #1) ----
        //
        // The body is a `gpui::list`: only the visible block window is built
        // and laid out per frame, not one element per block. This makes a
        // `cx.notify()` (j/k move, scroll, theme/zoom, and especially every
        // mouse-move during a selection drag) O(visible) instead of
        // O(blocks+spans). Audit #2 falls out of this — `line_layouts` only
        // holds the visible lines, so `doc_pos_at`'s scan collapses to
        // O(visible) too.

        // Reconcile the list to the current `blocks` by splicing ONLY the
        // changed range — never `reset()` (that drops scroll + measurements and
        // snaps the viewport to the top whenever the block count changes, e.g. a
        // live edit-flush from a sibling Edit tile). Scroll stays anchored; the
        // reveal below keeps the focused block on-screen. Must run EVERY frame
        // (the `blocks_seq` gate makes an idle frame a no-op).
        d.reconcile_list();
        let new_count = d.list.len();
        // Keep the focused block on-screen when it changed (this also catches
        // nav actions whose `reveal_block` ran against a stale count before the
        // list was first populated).
        if d.last_cursor_block.get() != Some(d.cursor_block) {
            d.last_cursor_block.set(Some(d.cursor_block));
            if d.cursor_block < new_count {
                d.list.state().scroll_to_reveal_item(d.cursor_block);
            }
        }

        // Owned snapshots for the `'static` per-row render closure — all cheap
        // (Theme clone once per frame, Rc pointer clones, SharedString refcount
        // bumps, Copy values). The closure rebuilds a `RenderCtx` borrowing
        // these owned locals for each visible block it constructs.
        let theme = self.theme.clone();
        let body_font = self.body_font.clone();
        let code_font = self.code_font.clone();
        let text_scale = self.text_scale;
        let cursor_block = d.cursor_block;
        let doc_selection = self.doc_selection;
        let line_layouts = self.line_layouts.clone();
        let weak_view = cx.entity().downgrade();
        let blocks_rc = d.blocks_rc();

        let render_fn = move |idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
            let Some(block) = blocks_rc.get(idx) else {
                return div().into_any_element();
            };
            #[cfg(test)]
            DOC_BLOCK_BUILDS.with(|c| c.set(c.get() + 1));
            let ctx = RenderCtx {
                theme: &theme,
                body_font: body_font.clone(),
                code_font: code_font.clone(),
                text_scale,
                cursor_block: Some(cursor_block),
                doc_selection,
                line_layouts: Some(line_layouts.clone()),
                current_block: None,
                weak_view: Some(weak_view.clone()),
                doc_dir: doc_dir.clone(),
                block_count: blocks_rc.len(),
                // Doc view never shows raw markdown markers — agent chat only.
                show_heading_markers: false,
                // Doc view uses `doc_selection`/`line_layouts`, not the
                // transcript code-block hit path.
                block_hits: None,
            };
            let el = block_element(&ctx, idx, block);
            // UXI-ParagraphSpacing-1 test seam: expose each doc block's painted
            // bounds so `verify_harness` can measure the inter-block gap.
            #[cfg(test)]
            let el = probe_bounds_dyn(format!("doc-block-{idx}"), el);
            el
        };

        // View-mode mouse selection: anchor on left MouseDown, update head on
        // every MouseMove while a button is held, release on MouseUp. The
        // wrapping doc body is the listener for all three; hit-testing falls
        // through to the registered per-line TextLayouts in `self.line_layouts`
        // (now populated only for the visible window — audit #2).
        let body = div()
            .id("doc-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(self.editor_fg())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, ev: &MouseDownEvent, _w, cx| {
                    view.doc_mouse_down(ev, cx);
                }),
            )
            .on_mouse_move(cx.listener(|view, ev: &MouseMoveEvent, _w, cx| {
                view.doc_mouse_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, ev: &MouseUpEvent, _w, cx| {
                    view.doc_mouse_up(ev, cx);
                }),
            )
            .child(
                // Default (visible-only) measuring — NOT `Auto`. `Auto` means
                // "measure all items" (gpui list.rs), which builds every line to
                // measure it and registers its `TextLayout` into `line_layouts`,
                // but only the visible lines get prepainted (bounds set). Then
                // `doc_pos_at` iterating all of them calls `.bounds()` on an
                // un-prepainted layout → panic across the input callback. The
                // agent + Edit lists already use the default; the doc body's
                // parent is `flex_1().min_h_0()`, so the list fills the viewport
                // and scrolls without needing to size to content.
                gpui::list(d.list.state().clone(), render_fn)
                    .flex_1()
                    .w_full(),
            );

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(fg_or(top, STATUS_FG))
            .font_weight(FontWeight::BOLD)
            .child(format!("yalda-gpui — {}", d.file_label))
            .child(self.multi_home_dot(d.file_label.as_ref()));

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(bg_or(bot, STATUS_BG))
            .text_color(fg_or(bot, 0x666666))
            .text_size(px(11.0))
            .child({
                // The workspace interior is always a Plane (infinite-plane,
                // Stage D) — no layout-mode sigil. Just the block counter.
                format!(
                    "block {} / {}",
                    d.cursor_block.saturating_add(1),
                    d.blocks.len()
                )
            })
            .child(SharedString::new_static(
                "j/k scroll · h/l block · g/G top/bot · Ctrl-O browse · Space tile menu · . workspace menu",
            ));

        root.key_context("YaldaView")
            .on_key_down(cx.listener(Self::handle_doc_key))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::cursor_next))
            .on_action(cx.listener(Self::cursor_prev))
            .on_action(cx.listener(Self::cursor_top))
            .on_action(cx.listener(Self::cursor_bottom))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::enter_edit))
            .on_action(cx.listener(Self::enter_wp))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::open_local_menu))
            .on_action(cx.listener(Self::open_global_menu))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::next_buffer))
            .on_action(cx.listener(Self::prev_buffer))
            // next_workspace/prev_workspace now come from `.workspace_nav(cx)` (every screen).
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::close_workspace))
            .on_action(cx.listener(Self::split_h))
            .on_action(cx.listener(Self::split_v))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::only_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            // Wired on EVERY screen root (like workspace_nav / toggle_jump_panel)
            // so the action lands whichever tile has focus.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_doc_selection))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(body)
            .child(footer)
    }

    pub(crate) fn render_edit(
        &self,
        root: gpui::Div,
        e: &mut EditState,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let mode_label = match e.mode {
            EditMode::Normal => "NORMAL",
            EditMode::Insert => "INSERT",
        };
        let view_label = match e.view {
            EditView::Code => "RAW",
            EditView::WordProcessor => "WP",
        };

        let body: AnyElement = match e.view {
            EditView::Code => self.build_edit_body_code(e).into_any_element(),
            EditView::WordProcessor => self.build_edit_body_wp(e).into_any_element(),
        };

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        let header_view_label = match e.view {
            EditView::Code => "edit",
            EditView::WordProcessor => "wp",
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(fg_or(top, STATUS_FG))
            .font_weight(FontWeight::BOLD)
            .child(format!(
                "yalda-gpui [{}] — {}",
                header_view_label, e.file_label
            ))
            .child(self.multi_home_dot(e.file_label.as_ref()));

        let dirty_mark = if e.editor.is_modified() { "•" } else { " " };
        let extend_mark = if e.editor.extend_mode() { " EXT" } else { "" };
        let sel_size: Option<usize> = e.editor.selection_range().map(|((sl, sc), (el, ec))| {
            // Cheap size summary: char count for single-line, line count otherwise.
            // Mirrors the kind of one-glance status the user wants in the footer.
            if sl == el {
                ec.saturating_sub(sc)
            } else {
                (el - sl) + 1
            }
        });
        // The workspace interior is always a Plane (infinite-plane, Stage D) —
        // no layout-mode sigil prefix in the status line.
        let layout_prefix = String::new();
        let mut left_status = format!(
            "{}{} {} {}{} · L{}:C{}",
            layout_prefix,
            dirty_mark,
            view_label,
            mode_label,
            extend_mark,
            cursor_line + 1,
            cursor_col + 1,
        );
        if let Some(n) = sel_size {
            let same_line = e
                .editor
                .selection_range()
                .map(|((sl, _), (el, _))| sl == el)
                .unwrap_or(false);
            let unit = if same_line { "ch" } else { "ln" };
            left_status.push_str(&format!(" · sel:{}{}", n, unit));
        }
        if let Some(msg) = &e.last_save_msg {
            left_status.push_str("  [");
            left_status.push_str(msg);
            left_status.push(']');
        }
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(bg_or(bot, STATUS_BG))
            .text_color(fg_or(bot, 0x666666))
            .text_size(px(11.0))
            .child(left_status)
            .child(SharedString::new_static(
                "Ctrl-W toggle wp/raw · Ctrl-S save · Ctrl-V view · v ext · d del · y yank",
            ));

        // No `actions!` wired here — the EditView key context catches all
        // keys via `on_key_down` so the same vocabulary works in both modes.
        // The menu-bar actions (Quit / OpenBrowser / OpenAgent) still need
        // explicit `on_action` listeners on this root so the macOS menu bar
        // can dispatch them to whichever screen happens to be focused.
        root.key_context("EditView")
            .on_key_down(cx.listener(Self::handle_edit_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(body)
            .child(footer)
    }

    /// Code (raw markdown) view: monospace, gutter with line numbers,
    /// per-line `md_highlight` source colors. Lines soft-wrap and the cursor
    /// splices inline via the shared `build_wrapped_line` helper.
    ///
    /// **Virtualized**: rendered through a `gpui::list` so only the visible rows
    /// are built/laid-out per frame, not one element per document line. Combined
    /// with the incremental highlight cache this makes a keystroke O(changed),
    /// not O(document).
    pub(crate) fn build_edit_body_code(&self, e: &mut EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let dim_fg: Hsla = rgb(0x6272a4).into();
        let sel = e.editor.selection_range();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        // Incremental highlight: only changed lines are re-tokenized; unchanged
        // frames recompute zero. `lines_rc`/`hl_snap` are cheap Rc clones.
        let (lines_rc, hl_snap) = e.highlight_snapshot(&self.theme, &self.syntect_hl);

        // Splice the changed range + keep the caret revealed (shared with WP).
        e.reconcile_and_reveal(&lines_rc, edit_seq, cursor_line, cursor_col);

        // Owned snapshots for the `'static` per-row render closure — all cheap
        // (Rc pointer clones / Copy / SharedString refcount bumps).
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let selection_bg = self.theme.agent.selection_bg;
        let text_size = px(14.0 * self.text_scale);

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                segs = apply_line_selection(&segs, &line_str, sel, line_idx, base_style, selection_bg);
            }

            let gutter = div()
                .w(px(40.0))
                .flex_none()
                .text_color(dim_fg)
                .child(format!("{:>3} ", line_idx + 1));

            // Soft-wrap: long lines break at whitespace and stack below the
            // gutter rather than running off the right edge — which is what
            // let the cursor scroll out of view. `build_wrapped_line` emits
            // the caret as an inline flex child so it wraps with the text.
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
            );

            div()
                .flex()
                .flex_row()
                // Fill the list width so `content`'s `flex_1` has a bounded
                // space to soft-wrap within. `gpui::list` lays each row out in
                // isolation (no parent align-items: stretch), so without this
                // the row shrinks to content width and long lines never wrap —
                // they overflow and get clipped by the body's overflow_x_hidden.
                .w_full()
                .child(gutter)
                .child(content)
                .into_any_element()
        };

        div()
            .id("edit-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // Clip the rare unbroken token (no whitespace to wrap at) instead
            // of letting it widen the row and reintroduce horizontal scroll.
            .overflow_x_hidden()
            .px_4()
            .py_2()
            .text_size(text_size)
            .font_family(self.code_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list.state().clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }

    /// Word-Processor view: proportional body font + per-line typographic
    /// styling driven by `classify_wp_line`. Headings get larger sizes and
    /// bold weight; lists/blockquote/code get block-level decorations.
    /// `md_highlight`'s segments still carry inline `**bold**`/`*italic*`
    /// modifiers, which `font_for` maps to FontWeight/FontStyle on render.
    /// No gutter — word processors don't show line numbers.
    pub(crate) fn build_edit_body_wp(&self, e: &mut EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let sel = e.editor.selection_range();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        // Incremental highlight: only changed lines are re-tokenized; unchanged
        // frames recompute zero. `lines_rc`/`hl_snap` are cheap Rc clones.
        let (lines_rc, hl_snap) = e.highlight_snapshot(&self.theme, &self.syntect_hl);

        // Per-line typographic kind, cached on `edit_seq` (012): the fold runs
        // once per edit, not once per frame, so idle frames (cursor blink,
        // selection, scroll, theme, cross-tile notify) recompute zero. The
        // virtualized render closure indexes any visible line off the `Rc`.
        let kinds = e.wp_kinds_snapshot(&lines_rc, edit_seq);

        // Splice the changed range + keep the caret revealed (shared with Code).
        e.reconcile_and_reveal(&lines_rc, edit_seq, cursor_line, cursor_col);

        // Owned snapshots for the `'static` per-row closure.
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let kinds = std::rc::Rc::new(kinds);
        let body_font = self.body_font.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let selection_bg = self.theme.agent.selection_bg;
        let text_scale = self.text_scale;

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
                segs = apply_line_selection(&segs, &line_str, sel, line_idx, base_style, selection_bg);
            }

            // Per-kind typography. Headings get scaled sizes + bold; lists
            // and paragraphs use the body font at the default size; code and
            // tables use monospace.
            let (raw_size_px, font_weight, top_pad) = match kind {
                WpLineKind::Heading(1) => (26.0, FontWeight::BOLD, 10.0),
                WpLineKind::Heading(2) => (22.0, FontWeight::BOLD, 8.0),
                WpLineKind::Heading(3) => (18.0, FontWeight::BOLD, 6.0),
                WpLineKind::Heading(4) => (16.0, FontWeight::BOLD, 5.0),
                WpLineKind::Heading(5) => (15.0, FontWeight::BOLD, 4.0),
                WpLineKind::Heading(_) => (14.0, FontWeight::BOLD, 4.0),
                WpLineKind::CodeFence | WpLineKind::CodeContent => (13.0, FontWeight::NORMAL, 0.0),
                WpLineKind::TableRow => (13.0, FontWeight::NORMAL, 0.0),
                // UXI-ParagraphSpacing-1: list items get a readability gap above
                // each one so bullets break apart (mirrors the Doc view's list gap).
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

            // Soft-wrap (mirrors the Code view): tokens break at whitespace
            // so long prose lines wrap below instead of pushing the caret
            // off-screen. WP uses a proportional `line_font`; whitespace
            // tokens at wrap boundaries can leave a slightly ragged left
            // margin — acceptable vs. an invisible cursor.
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
                None,
                None,
                line_idx,
            );

            // Block-level decoration per kind.
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
                    .bg(rgb(0x21222c))
                    .child(content),
                WpLineKind::Empty => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    // UXI-ParagraphSpacing-1: the blank paragraph-break line carries
                    // the readability gap on top of the base 18px blank row; scaled.
                    .h(px(18.0 * text_scale) + paragraph_gap(text_scale))
                    .child(content),
                _ => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad * text_scale))
                    .child(content),
            };

            // Fill the list width (same reason as the Code view): rows in a
            // `gpui::list` don't stretch, so `content`'s `flex_1` needs `w_full`
            // on the row to have a bounded width to soft-wrap within.
            line_div.w_full().into_any_element()
        };

        div()
            .id("edit-body-wp")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_x_hidden()
            .px_8()
            .py_4()
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list.state().clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }

    // Render a single ACP tool call as a collapsible block. The
    // expanded body honours a per-tool render policy that mirrors the
    // Claude Code TUI:
    //
    // - `Read` / `Search` / `SwitchMode` and `TodoWrite`: header only.
    //   The model gets the data; the user only needs to know the action
    //   happened. Click does nothing for these (no body to expand).
    // - `Execute` (Bash): show the first 3 lines of output + a "+N more"
    //   marker — same `tb1 = 3` cap the TUI uses (`Tw9` in cli.js).
    // - `Fetch`: 10 lines (web fetches are usually short HTML excerpts).
    // - `Edit` / `Move` / `Delete`: full diff/content — the visible
    //   change is the whole point.
    // - `Think` (subagents) / `Other` (MCP tools): full content.
    //
    // The previous `build_tool_block` / `tool_body` lived here as
    // `&self` methods. They've been replaced by the free-function
    // `build_tool_block_with_weak` / `tool_body_free` further up
    // in the file — necessary so the per-item closure handed to
    // `gpui::list` can construct tool blocks without holding a borrow
    // of `self`.

    /// Render the pinned session recap (recap-panel, UXI-AgentTile-15) for tile session
    /// `id`, or `None` when this session has no recap. A full-width bordered box
    /// that sits ABOVE the subagents/tasks panels inside the agent tile: a header
    /// ("Recap" + session label + re-run/dismiss buttons) over a status-dependent
    /// body — "Summarizing…" (plus any streamed-so-far text) while `Generating`,
    /// the finished prose when `Ready`, a reason when `Failed`. Long summaries
    /// scroll within a capped height so the recap never crowds out the transcript.
    ///
    /// Built with `&self` + `weak_self` (NOT `Context<Self>`) because it renders
    /// inside the session entity's `update`; buttons re-enter via `weak.update` at
    /// click time. Chrome-class: native size, unaffected by document zoom.
    /// `render_agent` notifies the root as chunks land (`apply_recap_event`) to
    /// repaint this inline element.
    pub(crate) fn render_agent_recap(
        &self,
        id: SessionId,
        weak_self: gpui::WeakEntity<Self>,
    ) -> Option<AnyElement> {
        let recap = self.recaps.get(&id)?;
        let at = &self.theme.agent;
        let accent: Hsla = nc(at.warm_accent);
        let dim: Hsla = nc(at.dim);
        let err: Hsla = rgb(0xff6b6b).into();
        let fg: Hsla = self.editor_fg();
        let mono = self.code_font.clone();
        let prose = self.body_font.clone();
        let base = px(13.0);
        let pt = 13.0f32;
        let mut box_bg = accent;
        box_bg.a = 0.08;
        let mut hover_bg = accent;
        hover_bg.a = 0.18;

        // A small icon button (re-run / dismiss). Each re-enters the root via the
        // weak handle at click time (state resolved in the handler, INV rule 4).
        let mk_btn = |btn_id: &'static str, glyph: &'static str| {
            div()
                .id(btn_id)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_sm()
                .cursor_pointer()
                .text_color(dim)
                .font_family(mono.clone())
                .text_size(px(pt * 0.9))
                .hover(move |s| s.bg(hover_bg).text_color(fg))
                .child(SharedString::from(glyph))
        };
        let rerun = mk_btn("agent-recap-rerun", "⟳").on_click({
            let weak = weak_self.clone();
            move |_ev: &gpui::ClickEvent, _w, app| {
                let _ = weak.update(app, |this, cx| this.rerun_recap(id, cx));
            }
        });
        let dismiss = mk_btn("agent-recap-dismiss", "✕").on_click({
            let weak = weak_self.clone();
            move |_ev: &gpui::ClickEvent, _w, app| {
                let _ = weak.update(app, |this, cx| this.dismiss_recap_for(id, cx));
            }
        });

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_color(accent)
                            .font_family(mono.clone())
                            .text_size(px(pt * 0.95))
                            .child(SharedString::from("Recap")),
                    )
                    .child(
                        div()
                            .text_color(dim)
                            .font_family(mono.clone())
                            .text_size(px(pt * 0.8))
                            .child(SharedString::from(recap.session_label.clone())),
                    ),
            )
            .child(rerun)
            .child(dismiss);

        let body: AnyElement = match &recap.status {
            RecapStatus::Generating => {
                let mut colb = div().flex().flex_col().gap_1().w_full().child(
                    div()
                        .text_color(dim)
                        .font_family(mono.clone())
                        .text_size(px(pt * 0.85))
                        .child(SharedString::from("Summarizing…")),
                );
                if !recap.text.trim().is_empty() {
                    colb = colb.child(multiline_text(&recap.text, fg, &prose, base));
                }
                colb.into_any_element()
            }
            RecapStatus::Ready => multiline_text(&recap.text, fg, &prose, base).into_any_element(),
            RecapStatus::Failed(reason) => div()
                .w_full()
                .text_color(err)
                .font_family(mono.clone())
                .text_size(px(pt * 0.85))
                .child(SharedString::from(format!("Recap failed: {reason}")))
                .into_any_element(),
        };

        let panel = div().px_4().pb_2().child(
            div()
                .id("agent-recap-panel")
                .flex()
                .flex_col()
                .gap_2()
                .w_full()
                .p_2()
                .max_h(px(180.0))
                .overflow_y_scroll()
                .rounded_md()
                .bg(box_bg)
                .border_1()
                .border_color(dim)
                .child(header)
                .child(body),
        );
        Some(probe_bounds("recap-panel", panel.into_any_element()))
    }

    /// Render the Claude (ACP) screen. Frozen lines (Claude's prior turns)
    /// get a left bar + dim color; the editable region (the user's pending
    /// draft and any inline replies) renders normally with cursor splice.
    /// Header shows attach status; footer shows mode + send hint + send
    /// state ("…" while a reply is in flight).
    pub(crate) fn render_agent(
        &mut self,
        root: gpui::Div,
        tile: &mut AgentTile,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Unbound tile (`bound == None`) ⇒ render the session selector
        // (SessionPicker), not a transcript. This is the canonical unbound
        // state (session close / unbind / rebind all land here).
        let Some(id) = tile.session() else {
            // A REMEMBERED session that couldn't be resumed on restart shows an
            // inline "session unavailable — start fresh" notice (UXI-AgentTile-19),
            // never the picker.
            if let Some(lost) = tile.unavailable_label() {
                return self.render_agent_unavailable(root, lost, cx);
            }
            return self.render_agent_picker(root, tile, cx);
        };
        // The bound session is a GPUI entity (spec-agent-session-ownership.md /
        // ticket 025). Clone the (cheap) handle so the store borrow ends, then
        // build the transcript body inside the entity's `update`, where the
        // render body gets a SAFE `&mut AgentState` for the whole pass — no raw
        // pointer, no `unsafe`. The body reads `self.theme`/fonts/render helpers
        // (immutable `&self` borrows, disjoint from `cx`) directly through the
        // outer `self`; values that need the outer `Context<Self>` (the weak
        // handle for click listeners) are precomputed BEFORE the update, since
        // the update borrows `cx`.
        let Some(session_ent) = self.session_entity(id) else {
            return self.render_agent_picker(root, tile, cx);
        };
        let (active_slot_label, active_slot_cwd) = {
            let s = session_ent.read(cx);
            (s.label.clone(), s.cwd.clone())
        };
        // Weak handle to THIS view, captured by the status-strip / sidebar
        // click listeners (they re-enter via `weak.update(app, …)` at click
        // time). Computed before the entity `update` borrows `cx`.
        let weak_self = cx.entity().downgrade();

        // ── Transcript (ticket 021) ──────────────────────────────────────
        // The transcript list is its own cached view entity now: it OWNS the
        // scroll/list state, READS the session in its render, and invalidates
        // itself by OBSERVING the session (slice-filtered). Lazily create the
        // per-session `TranscriptView` (the constructor registers the observe),
        // then embed it via `cached_child` so a chatbox keystroke (which never
        // moves a transcript slice) skips the transcript's render entirely.
        let transcript_view = self.transcript_view_for(id, session_ent.clone(), cx);
        // Option A backstop (dropped-self-notify class → "last message never
        // renders"): KEY the cached transcript's element id on its render
        // fingerprint. A moved fingerprint yields a fresh `GlobalElementId`, so
        // gpui's `with_element_state` misses and the transcript's `render()` is
        // FORCED — independent of `mark_view_dirty`/`view_path`, the hop that
        // silently no-ops when the view has no node in the committed frame (and
        // leaves the cached prepaint reused stale). The self-notify path stays
        // the fast O(changed) invalidation; this id only closes the hole when a
        // notify is dropped. A stable fingerprint ⇒ stable id ⇒ cache hit ⇒
        // render-skip preserved (typing elsewhere never moves it). The root
        // (uncached) recomputes this each frame, so the backstop can't itself be
        // parked. See `TranscriptSeqs::fingerprint_hash`.
        let transcript_fp = TranscriptSeqs::of(&session_ent.read(cx).state).fingerprint_hash();
        let transcript_body: AnyElement = div()
            .id(("transcript-fp", transcript_fp))
            .size_full()
            .child(cached_child(transcript_view))
            .into_any_element();

        // Build the status strips + compose + sidebars inside the session
        // entity's update — `c` is a real `&mut AgentState` for the chrome that
        // STAYS inline (tickets 022/023). The transcript body (built above) is
        // moved in and slotted into the content layout.
        let (header, content_area) =
            session_ent.update(cx, |session_payload, _scx| {
                let c: &mut AgentState = &mut session_payload.state;

        // (Model C: the transcript is read-only and renders no caret; the active
        // cursor lives in the compose, read in the status strip / compose body.)
        let at = &self.theme.agent; // shorthand for agent theme
        let dim_fg: Hsla = nc(at.dim);
        // Theme-derived background tints for turn cards. Blend a faint
        // tint into the editor background so cards work on any theme.
        let base_bg: Hsla = self.editor_bg();
        let compose_panel_bg: Hsla = tint_bg(base_bg, 0.55, 0.1, 0.03);
        // Compose input text uses the theme's editor foreground so it stays
        // legible against `compose_panel_bg` on light themes (folio, FT,
        // solarized-light) — not the hardcoded Dracula light gray, which
        // vanished into the near-white panel.
        let compose_fg: Hsla = self.editor_fg();
        let top = self.theme.top_bar;

        // ---- Status Strip (spec §30) ----
        // Single-row header showing agent label, sub-agent breadcrumb
        // (when focused), model id, permission mode, context-window
        // usage, and turn / elapsed. Any field
        // whose underlying signal is absent renders nothing — no
        // placeholder, no `?`. The strip is at most as wide as the
        // data it has.
        let strip_dim: Hsla = nc(at.dim);
        let strip_warm: Hsla = nc(at.warm_accent);
        let strip_fg = fg_or(top, STATUS_FG);

        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(strip_fg)
            .font_weight(FontWeight::BOLD)
            .text_size(px(12.0));

        // Agent label (slot label).
        strip = strip.child(
            div()
                .pr_2()
                .child(SharedString::from(active_slot_label.clone())),
        );

        // Edit-surface status (moved up from the old footer): which surface +
        // mode is active, the cursor position, and any transient status/awaiting
        // note. This is the primary "what am I doing" readout, so it sits at the
        // top next to the agent identity.
        {
            // Model C: the active edit surface is the COMPOSE buffer in both
            // placements; the placement only changes the label (CHATBOX vs
            // WORKSHEET). Mode + cursor read the compose, not the read-only
            // transcript.
            let in_chatbox = c.input_surface.is_chatbox();
            let compose = c.input_surface.compose();
            let mode_label = match (in_chatbox, compose.mode) {
                (true, EditMode::Normal) => "CHATBOX",
                (true, EditMode::Insert) => "CHATBOX INSERT",
                (false, EditMode::Normal) => "WORKSHEET",
                (false, EditMode::Insert) => "WORKSHEET INSERT",
            };
            let dirty_mark = if compose.editor.document().is_modified() {
                "•"
            } else {
                ""
            };
            let extend_mark = if compose.editor.extend_mode() {
                " EXT"
            } else {
                ""
            };
            let compose_cursor = compose.editor.cursor();
            let mut status_text = format!(
                "{}{}{} · L{}:C{}",
                dirty_mark,
                mode_label,
                extend_mark,
                compose_cursor.line + 1,
                compose_cursor.col + 1,
            );
            if c.turn_phase.is_awaiting() {
                status_text.push_str(" · …awaiting reply");
            }
            if let Some(msg) = &c.status {
                status_text.push_str("  [");
                status_text.push_str(msg);
                status_text.push(']');
            }
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .font_weight(FontWeight::NORMAL)
                    .child(SharedString::from(status_text)),
            );
        }

        // Session-server indicator.
        if c.server_managed {
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::new_static("server")),
            );
        }

        // Sub-agent breadcrumb (only when focused).
        if let Some(key) = c.focused_subagent.as_ref()
            && let Some(sa) = c.tools.calls.get(key).and_then(classify_subagent)
        {
            let crumb = format!(" ⏵ {} ◂", sa.label);
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_warm)
                    .child(SharedString::from(crumb)),
            );
        }

        // Per-slot cwd (spec-agent-cwd.md §6). Hidden when the slot cwd
        // matches the process cwd — surfacing the implicit default on
        // every session is noise. Tooltip with the absolute path is a
        // follow-up (GPUI tooltip support is patchy on this version);
        // for now the shortened display is the only affordance.
        let proc_cwd = cached_process_cwd();
        if active_slot_cwd.as_path() != proc_cwd {
            let shortened = shorten_cwd_for_display(&active_slot_cwd);
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(shortened)),
            );
        }

        // Model id: the authoritative value comes from the agent
        // (`agent_model`, mirrored from `session/new`'s `config_options`).
        // Fall back to the old best-effort guesses (session mode → channel
        // command) only when the adapter never advertised a model — e.g. an
        // older `claude-code-acp` that doesn't surface a model selector.
        let model_label: Option<String> = c
            .agent_model
            .clone()
            .or_else(|| c.agent_mode.as_ref().map(|m| m.0.to_string()))
            .or_else(|| c.channel.as_ref().map(|ch| ch.command().to_string()));
        if let Some(m) = model_label {
            // Clickable when the agent advertised a model picklist: a click
            // opens the local (space) menu where the "switch model" submenu
            // lives — a mouse affordance for the keyboard `space M` path
            // (UXI-AgentTile-16). Falls back to a plain label otherwise.
            let has_models = !c.available_models.is_empty();
            if has_models {
                strip = strip.child(
                    div()
                        .id("agent-model-badge")
                        .pr_2()
                        .text_color(strip_dim)
                        .hover(|s| s.text_color(strip_warm))
                        .cursor_pointer()
                        .child(SharedString::from(format!("{m} ▾")))
                        // Dispatch the OpenLocalMenu action (not `cx.listener` —
                        // the strip is assembled inside a `session_ent.update`
                        // closure that already holds `cx`). The agent tile is
                        // focused (we clicked its badge), so the menu opens on
                        // the AGENT scope where "switch model" lives.
                        .on_click(|_ev, window, cx| {
                            window.dispatch_action(Box::new(crate::OpenLocalMenu), cx);
                        }),
                );
            } else {
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_dim)
                        .child(SharedString::from(m)),
                );
            }
        }

        // Permission mode — made prominent so the danger level of the
        // current mode reads at a glance. Yolo (auto-approve everything,
        // the no-config default) gets the warm/danger accent + bold; the
        // restricted modes render dim. Stays at native chrome size (no
        // text_scale). Cycle it with `<space> c m`.
        //
        // Sourced from session state (`c.permission_mode`), NOT the local
        // `channel`: in session-server mode the agent/channel live in the
        // server and `c.channel` is `None`, so gating on it hid the badge
        // for every server-backed session. The session always has a mode
        // (mirrored from `SessionInfo.permission_mode`), so always render.
        {
            let mode = c.permission_mode;
            let mode_str = mode.short_label();
            let is_yolo = matches!(mode, yalda::acp_channel::PermissionMode::Yolo);
            let glyph = if is_yolo { "⚡" } else { "🔒" };
            let badge = div()
                .pr_2()
                .text_color(if is_yolo { strip_warm } else { strip_dim })
                .child(SharedString::from(format!("{glyph} perm: {mode_str}")));
            let badge = if is_yolo {
                badge.font_weight(FontWeight::BOLD)
            } else {
                badge.font_weight(FontWeight::NORMAL)
            };
            strip = strip.child(badge);
        }

        // Context-window usage — a subtle progress bar plus a compact number.
        // The bar fills proportionally and shifts to the warm/danger accent as
        // the window approaches full.
        if let Some(usage) = &c.usage {
            let used_k = (usage.tokens_used as f64) / 1000.0;
            let total_k = (usage.tokens_total as f64) / 1000.0;
            let frac = if usage.tokens_total > 0 {
                (usage.tokens_used as f64 / usage.tokens_total as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let pct = frac * 100.0;
            const BAR_W: f32 = 64.0;
            let fill_w = (BAR_W * frac as f32).max(if frac > 0.0 { 2.0 } else { 0.0 });
            let fill_color = if pct >= 85.0 { strip_warm } else { nc(at.user_bar) };
            let track_bg = {
                let mut h = strip_dim;
                h.a = 0.22;
                h
            };
            let track = div()
                .w(px(BAR_W))
                .h(px(5.0))
                .rounded_full()
                .bg(track_bg)
                .child(div().w(px(fill_w)).h_full().rounded_full().bg(fill_color));
            let label = format!("{:.0}k/{:.0}k ({:.0}%)", used_k, total_k, pct);
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .child(track)
                    .child(
                        div()
                            .text_color(strip_dim)
                            .font_weight(FontWeight::NORMAL)
                            .child(SharedString::from(label)),
                    ),
            );
        }

        // Active sub-agents — the one readout that used to live only in the
        // (now-removed) bottom info bar. Compact glyph+label list of any
        // in-progress / pending subagents; omitted entirely when none.
        {
            use yalda::acp_channel::ToolCallStatus;
            let active: Vec<String> = c
                .subagents()
                .iter()
                .filter(|sa| {
                    matches!(
                        sa.status,
                        ToolCallStatus::InProgress | ToolCallStatus::Pending
                    )
                })
                .map(|sa| {
                    let glyph = match sa.status {
                        ToolCallStatus::InProgress => "\u{25d0}",
                        ToolCallStatus::Pending => "\u{25cb}",
                        _ => "\u{00b7}",
                    };
                    let label: String = if sa.label.chars().count() > 16 {
                        let head: String = sa.label.chars().take(15).collect();
                        format!("{}\u{2026}", head)
                    } else {
                        sa.label.clone()
                    };
                    format!("{}{}", glyph, label)
                })
                .collect();
            if !active.is_empty() {
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_warm)
                        .font_weight(FontWeight::NORMAL)
                        .child(SharedString::from(active.join("  "))),
                );
            }
        }

        // Turn / elapsed. Show "turn N · M:SS" when a turn has run; "turn
        // N" alone if no timer is active; nothing if no turns have run.
        let completed_turns = c.channel.as_ref().map(|ch| ch.turn_count()).unwrap_or(0);
        let display_turn = if c.turn_phase.is_awaiting() {
            completed_turns + 1
        } else {
            completed_turns
        };
        let turn_started = c.turn_phase.turn_started();

        // Right side of the header strip: turn/elapsed plus a Stop button while
        // a reply is in flight. Pushed right with a flex spacer so it anchors to
        // the strip's trailing edge regardless of how much status sits left.
        let mut header_right = div().flex().flex_row().items_center().gap_2();
        let mut header_right_has_content = false;
        // UXI-AgentTile-28: the tile says IN WORDS whether the agent is running or
        // waiting on you — a filled pill in the same vocabulary the jump panel
        // uses (`◆ working` / `✦ your turn`), so the two surfaces read as one
        // language. The dim "· …awaiting reply" in the status strip was easy to
        // miss with a wall of transcript above it.
        if c.turn_phase.is_awaiting() || display_turn > 0 {
            let working = c.turn_phase.is_awaiting();
            let hue: Hsla = if working {
                nc(at.jump_working)
            } else {
                nc(at.tool_completed)
            };
            // ONE vocabulary with the jump panel: same glyph, same word, derived
            // from the same pure mapping (`agent_row_marks`).
            let (glyph, word) = {
                let (g, w) = agent_row_marks(if working {
                    AgentDotStatus::Working
                } else {
                    AgentDotStatus::WaitingForYou
                });
                (g, w.unwrap_or(""))
            };
            let mut pill_bg = hue;
            pill_bg.a = 0.15;
            let mut pill_edge = hue;
            pill_edge.a = 0.55;
            header_right = header_right.child(probe_bounds(
                "agent-status-pill",
                div()
                    .px_2()
                    .py(px(1.0))
                    .rounded_md()
                    .bg(pill_bg)
                    .border_1()
                    .border_color(pill_edge)
                    .text_color(hue)
                    .child(SharedString::from(format!("{glyph} {word}")))
                    .into_any_element(),
            ));
            header_right_has_content = true;
        }
        if display_turn > 0 || turn_started.is_some() {
            let elapsed_str = if let Some(t) = turn_started {
                let s = t.elapsed().as_secs();
                format!("{}:{:02}", s / 60, s % 60)
            } else {
                String::new()
            };
            let turn_color = if turn_started.is_some() {
                strip_warm
            } else {
                strip_dim
            };
            let label = if elapsed_str.is_empty() {
                format!("turn {}", display_turn)
            } else {
                format!("turn {} · {}", display_turn, elapsed_str)
            };
            header_right = header_right
                .child(div().text_color(turn_color).child(SharedString::from(label)));
            header_right_has_content = true;
        }
        // Stop button — dispatches the same StopAgent path as ⌘.; after a
        // graceful cancel is already pending it escalates to a hard kill+resume.
        if c.turn_phase.is_awaiting() {
            let stop_fg: Hsla = nc(at.tool_failed);
            let escalating = c.turn_phase.stop_requested();
            let stop_label = if escalating {
                "■ Force-restart ⌘."
            } else {
                "■ Stop ⌘."
            };
            let weak_stop = weak_self.clone();
            header_right = header_right.child(
                div()
                    .id("agent-stop-btn")
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_2()
                    .py(px(1.0))
                    .rounded_md()
                    .border_1()
                    .border_color(stop_fg)
                    .text_color(stop_fg)
                    .cursor_pointer()
                    .on_click(
                        move |_ev: &gpui::ClickEvent, window: &mut Window, app: &mut GpuiApp| {
                            let _ = weak_stop.update(app, |this, cx| {
                                this.stop_agent(&StopAgent, window, cx);
                            });
                        },
                    )
                    .child(SharedString::from(stop_label)),
            );
            header_right_has_content = true;
        }
        if header_right_has_content {
            strip = strip.child(div().flex_1()).child(header_right);
        }

        let header = strip;

        // Status (mode, cursor, awaiting) and the Stop button now live in the
        // header strip at the top; keyboard hints were removed. No footer.

        // Chatbox panel — rendered between body and the bottom edge when active.
        //
        // Each line is rendered as a non-wrapping row inside a per-line
        // overflow_hidden clip container. The cursor line is shifted left
        // via a negative pixel margin so the caret stays visible. The clip
        // container inherits its width from the flex layout — no need to
        // know the pixel width at render time.
        // Compose panel — rendered below the transcript in BOTH placements
        // (Model C). Chatbox = pinned box; Worksheet = inline at the tail. The
        // inline-flush styling (no box chrome, conversation typography, `›`
        // gutter) + the cached-child promotion are a runtime-tuning follow-up;
        // for now both placements render the same panel so worksheet is usable.
        // UXI-AgentTile-11 visibility (stage 2): the WORKSHEET shows the bottom compose box
        // ONLY mid-turn (the chatbox — rule 7). An idle You-block renders INLINE in
        // the transcript at its anchor (FlatItem::YouBlock), not here. Navigating
        // idle shows NO compose chrome. Chatbox mode always shows its pinned box.
        let show_compose = c.input_surface.is_chatbox() || c.turn_phase.is_awaiting();
        let compose_panel = if !show_compose {
            None
        } else {
            // UXI-AgentTile-11 rule 7 (bug-hunt 12): the bottom panel is now ALWAYS a pinned
            // chatbox box — it renders only in chatbox mode or as the mid-turn
            // steering box (the idle worksheet draft renders INLINE as the YouBlock,
            // never here). So it never wears the worksheet flush/accent/"You" chrome.
            let is_worksheet = false;
            // Staged image attachments (pasted via Cmd+V) → chip labels, captured
            // before the mutable compose borrow below. Rendered as chips above the
            // box so the user sees what will be sent (UXI-AgentTile-14).
            let pending_image_labels: Vec<SharedString> = c
                .input_surface
                .compose()
                .pending_images
                .iter()
                .map(|p| SharedString::from(format!("🖼 {}", p.label)))
                .collect();
            let tb = c.input_surface.compose_mut();
            // Logical lines shown before the box caps height + scrolls. At/below
            // this the panel renders every line directly (grows to content,
            // cheap — nothing to virtualise). ABOVE it, building the whole draft
            // every keystroke is O(draft) element assembly (the Message Box
            // typing lag), so the panel switches to a fixed-height `gpui::list`
            // that builds only the visible rows — same 8-line scrolling box,
            // O(visible) cost.
            const COMPOSE_MAX_VISIBLE_LINES: usize = 8;
            let line_h = 18.0f32;
            let max_visible_h = COMPOSE_MAX_VISIBLE_LINES as f32 * line_h;

            let line_count = tb.editor.document().line_count().max(1);
            let compose_cursor_line = tb.editor.cursor().line;
            let compose_cursor_col = tb.editor.cursor().col;
            let compose_mode = tb.mode;
            let compose_sel = tb.editor.selection_range();
            let sep_color: Hsla = nc(at.compose_separator);
            let compose_cursor_color: Hsla = nc(at.cursor);
            // Same theme selection color the edit view paints (see
            // `build_edit_body_*` → `self.theme.agent.selection_bg`), so the
            // chatbox highlight contrast matches the rest of the app.
            let compose_selection_bg: Hsla = nc(at.selection_bg);
            // Worksheet (inline placement) tints the box border with the accent
            // as a placement cue; chatbox stays neutral.
            let compose_border: Hsla = if is_worksheet {
                compose_cursor_color
            } else {
                dim_fg
            };
            let compose_code_font = self.code_font.clone();
            let separator = div().w_full().h(px(1.0)).bg(dim_fg);

            // ── Caret-containment window (spec-chatbox-caret-containment.md). ──
            // ONE chokepoint computes the visible top-left grid cell from the
            // current caret + the box's MEASURED inner width (written last frame
            // by CaptureBounds), and stores it back. Both render paths read it;
            // nothing else sets scroll offset.
            let box_w = tb.bounds.get().2;
            let visible_cols = if box_w > 1.0 {
                (box_w / CHATBOX_CHAR_W).floor().max(1.0) as usize
            } else {
                // First frame before the box is measured: assume "very wide" so
                // we don't scroll horizontally; the next frame self-corrects.
                4096
            };
            let compose_bounds_sink = tb.bounds.clone();

            // All logical lines, wsp-expanded. UXI-AgentTile-9: each WORD-WRAPS to rows
            // of ≤ `visible_cols` columns (no horizontal scroll). The small vs
            // virtualized decision is on TOTAL VISUAL rows so one long wrapped
            // line can't overflow the un-scrolled small box and hide the caret
            // (UXI-TextEditing-1).
            let compose_lines: std::rc::Rc<Vec<String>> = {
                let doc = tb.editor.document();
                std::rc::Rc::new(
                    (0..line_count)
                        .map(|i| {
                            doc.line_text(i).trim_end_matches('\n').replace('\t', "    ")
                        })
                        .collect(),
                )
            };
            let visual_rows_total: usize = compose_lines
                .iter()
                .map(|l| wrap_line_cols(&l.chars().collect::<Vec<_>>(), visible_cols).len())
                .sum();

            let compose_body: AnyElement = if visual_rows_total <= COMPOSE_MAX_VISIBLE_LINES {
                // ── Small draft: render every (wrapped) line directly. Total
                //    visual rows ≤ cap ⇒ fits the box height with no scroll. ──
                let min_compose_h = line_h + 16.0;
                let mut inner = div().w_full().min_w_0().flex().flex_col();
                for (i, line_text) in compose_lines.iter().enumerate() {
                    inner = inner.child(build_chatbox_wrapped_line(
                        line_text,
                        i == compose_cursor_line,
                        compose_cursor_col,
                        compose_mode,
                        compose_cursor_color,
                        compose_sel,
                        i,
                        &compose_code_font,
                        compose_fg,
                        compose_selection_bg,
                        visible_cols,
                    ));
                }
                let mut scroll = div()
                    .id("compose-scroll")
                    .w_full()
                    .min_w_0()
                    .min_h(px(min_compose_h))
                    // +16 so the inner content area (after the 16px vertical
                    // padding) fits all COMPOSE_MAX_VISIBLE_LINES rows — matching
                    // the virtualized path. Without it `overflow_hidden` would
                    // clip the 8th line of a full small draft (no scroll here).
                    .max_h(px(max_visible_h + 16.0))
                    .overflow_hidden()
                    .px_4()
                    .py(px(8.0))
                    .font_family(compose_code_font.clone())
                    .text_size(px(13.0))
                    .text_color(compose_fg);
                // Placement chrome (design-c.md §1): Worksheet renders inline
                // flush in the transcript column — no box, no margins — with an
                // accent left bar as the `›` draft gutter, so the draft reads as a
                // continuation of the conversation. Chatbox keeps the pinned box.
                if is_worksheet {
                    scroll = scroll.border_l_2().border_color(compose_cursor_color);
                } else {
                    scroll = scroll
                        .bg(compose_panel_bg)
                        .border_1()
                        .border_color(dim_fg)
                        .rounded_md()
                        .mx_2()
                        .mb_1();
                }
                scroll
                    // Capture the inner content width (inside px_4) so next
                    // frame's `visible_cols` reflects the real box, not the
                    // whole-window width.
                    .child(CaptureBounds {
                        inner: inner.into_any_element(),
                        sink: compose_bounds_sink,
                    })
                    .into_any_element()
            } else {
                // ── Long draft: virtualise. `gpui::list` builds ONLY the visible
                //    items (one per LOGICAL line, each a wrapped column of visual
                //    rows), so per-keystroke cost is O(visible), not O(draft). ──
                let lines_snap = compose_lines.clone();
                // Splice the changed range (never `reset()`, which snaps the box
                // to its top on every newline).
                let compose_edit_seq = tb.editor.document().edit_seq();
                tb.list.reconcile(&lines_snap, compose_edit_seq);
                // UXI-TextEditing-1 under UXI-AgentTile-9: once lines wrap, the box scrolls in
                // VISUAL rows, not logical lines — computing the window over
                // logical lines stranded the caret below the fold (the recurring
                // chatbox-cursor bug, reintroduced by word-wrap). So: compute the
                // vertical window over VISUAL rows, then map the authoritative top
                // visual row back into the list's (item, offset) space (its items
                // are logical lines, each a wrapped column of visual rows).
                // Anchored on the prior window (`tb.window`) so the box only moves
                // when the caret would leave it; never read back from the list's
                // own anchor (mis-fires on freshly-spliced unmeasured rows).
                let (caret_vrow, total_vrows, per_line) = compose_visual_metrics(
                    &lines_snap,
                    compose_cursor_line,
                    compose_cursor_col,
                    visible_cols,
                );
                let visual_top = compose_first_visible_line(
                    caret_vrow,
                    tb.window.get().top_line,
                    total_vrows,
                    COMPOSE_MAX_VISIBLE_LINES,
                );
                tb.window.set(ComposeWindow {
                    top_line: visual_top,
                    left_col: 0,
                });
                let (item_ix, offset_rows) = compose_item_for_visual_row(&per_line, visual_top);
                tb.list.state().scroll_to(gpui::ListOffset {
                    item_ix,
                    offset_in_item: gpui::px(offset_rows as f32 * line_h),
                });
                let font = compose_code_font.clone();
                let cur_color = compose_cursor_color;
                let fg = compose_fg;
                let sel_bg = compose_selection_bg;
                let render_fn =
                    move |idx: usize, _w: &mut Window, _a: &mut GpuiApp| -> AnyElement {
                        let Some(line_text) = lines_snap.get(idx) else {
                            return div().into_any_element();
                        };
                        build_chatbox_wrapped_line(
                            line_text,
                            idx == compose_cursor_line,
                            compose_cursor_col,
                            compose_mode,
                            cur_color,
                            compose_sel,
                            idx,
                            &font,
                            fg,
                            sel_bg,
                            visible_cols,
                        )
                    };
                let mut scroll = div()
                    .id("compose-scroll")
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w_0()
                    .h(px(max_visible_h + 16.0))
                    .px_4()
                    .py(px(8.0))
                    .font_family(compose_code_font.clone())
                    .text_size(px(13.0))
                    .text_color(compose_fg);
                // Same placement chrome as the small-draft path (design-c.md §1):
                // inline-flush worksheet (accent left bar) vs pinned box.
                if is_worksheet {
                    scroll = scroll.border_l_2().border_color(compose_cursor_color);
                } else {
                    scroll = scroll
                        .bg(compose_panel_bg)
                        .border_1()
                        .border_color(compose_border)
                        .rounded_md()
                        .mx_2()
                        .mb_1();
                }
                scroll
                    .child(CaptureBounds {
                        inner: gpui::list(tb.list.state().clone(), render_fn)
                            .flex_1()
                            .w_full()
                            .into_any_element(),
                        sink: compose_bounds_sink,
                    })
                    .into_any_element()
            };

            // Top edge: a 1px darker rule creates a subtle visual separation
            // between the scrolling transcript and the fixed compose panel.
            let edge_color = {
                let mut h = sep_color;
                h.a = 0.4;
                h
            };
            // Model C "You" boundary: in worksheet (inline) placement the panel
            // below the read-only transcript is the user's compose area, so label
            // it `You` in the accent. This is the presence cue the worksheet
            // promised — under Model C the inline compose is always present in
            // worksheet mode, so the divider is the boundary of YOUR turn. Chatbox
            // (pinned) keeps the bare rule (its box is self-evidently the input).
            let mut panel = div()
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(edge_color);
            if is_worksheet {
                panel = panel.child(
                    div()
                        .px_4()
                        .pt_1()
                        .text_size(px(11.0))
                        .font_family(compose_code_font.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_color(compose_cursor_color)
                        .child(SharedString::new_static("You")),
                );
            }
            // Probe the compose box's OUTER (post-margin) bounds so the harness
            // can prove the placement chrome differs (UXI-AgentTile-10): worksheet is
            // flush (full column width, no margin) vs chatbox's inset box.
            // Image-attachment chips, above the box: one per staged paste, tinted
            // with the accent so they read as pending payload (UXI-AgentTile-14).
            if !pending_image_labels.is_empty() {
                let mut chips = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .px_4()
                    .pt_1();
                for label in pending_image_labels {
                    chips = chips.child(
                        div()
                            .px_2()
                            .rounded_md()
                            .bg(compose_panel_bg)
                            .border_1()
                            .border_color(compose_cursor_color)
                            .text_size(px(11.0))
                            .font_family(compose_code_font.clone())
                            .text_color(compose_fg)
                            .child(label),
                    );
                }
                panel = panel.child(probe_bounds("compose-image-chips", chips.into_any_element()));
            }
            Some(
                panel
                    .child(separator)
                    .child(probe_bounds("compose-box", compose_body)),
            )
        };

        // ---- Right sidepanel (Tasklist / Subagents) ----
        //
        // A fixed-width sidepanel on the RIGHT of the agent tile, **segmented**:
        // **Plan on TOP, Subagents BELOW** (UXI-AgentTile-3), both visible at once.
        // Each segment renders only when its `*_open` flag is true (and Subagents
        // only when non-empty); with one open it fills the sidepanel height.
        // Panel focus (Cmd-0) widens the sidepanel; `h`/`l` switch the focused
        // segment, `j`/`k` move the row within it.
        let sidebar_border: Hsla = nc(at.sidebar_border);
        let sidebar_header_fg: Hsla = nc(at.sidebar_header);
        let sidebar_dim_fg: Hsla = nc(at.dim);
        // Panel body text uses the theme's editor foreground so it's readable on
        // the sidebar background across themes (the old hardcoded DEFAULT_FG was a
        // fixed light gray — invisible on light schemes like folio).
        let sidebar_fg: Hsla = self.editor_fg();
        let sidebar_bg: Hsla = nc(at.sidebar_bg);

        let panel_focused = c.focus == AgentFocus::Panel;
        let panel_col = c.panel_col;
        let panel_sel = c.panel_sel;
        // The panels live in a fixed-width RIGHT sidepanel now (segmented: Plan
        // on top, Subagents below). Panel focus (Cmd-0) widens it a touch to
        // preserve the "region enlarges" affordance of UXI-AgentTile-3.
        let sidepanel_w = if panel_focused { px(340.0) } else { px(280.0) };
        let focus_border: Hsla = nc(at.warm_accent);
        let panel_transparent: Hsla = gpui::hsla(0.0, 0.0, 0.0, 0.0);
        let sel_bg: Hsla = {
            let mut a = nc(at.warm_accent);
            a.a = 0.20;
            a
        };
        // A column is "lit" (border-accent + selection-bearing) only while the
        // whole region holds focus AND it is the active column.
        let subagents_shown = c.subagents_open && !c.subagents().is_empty();
        let both_columns = c.tasklist_open && subagents_shown;
        let tasklist_lit = panel_focused && panel_col == PanelColumn::Tasklist;
        let subagents_lit = panel_focused && panel_col == PanelColumn::Subagents;

        // Tasklist (Plan) column — LEFT. One row per plan entry: glyph + content.
        let tasklist_col: Option<gpui::AnyElement> = if c.tasklist_open {
            let mut panel = div()
                .id("tasklist-panel")
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap(px(1.0))
                .px_2()
                .py_1()
                .overflow_y_scroll()
                .border_b_1()
                .border_color(if tasklist_lit { focus_border } else { sidebar_border })
                .bg(sidebar_bg)
                .text_size(px(11.0))
                .font_family(self.code_font.clone());
            panel = panel.child(
                div()
                    .px_1()
                    .py(px(1.0))
                    .text_color(sidebar_header_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("Plan")),
            );
            match &c.current_plan {
                Some(plan) if !plan.entries.is_empty() => {
                    use yalda::acp_channel::PlanEntryStatus;
                    for (i, entry) in plan.entries.iter().enumerate() {
                        let (glyph, glyph_fg): (&'static str, Hsla) = match entry.status {
                            PlanEntryStatus::Completed => ("✓", nc(at.tool_completed)),
                            PlanEntryStatus::InProgress => ("●", nc(at.tool_in_progress)),
                            PlanEntryStatus::Pending => ("○", nc(at.tool_pending)),
                            // ACP marks the enum #[non_exhaustive]; a future
                            // "failed" or similar status falls back clearly (§22).
                            _ => ("✗", nc(at.tool_failed)),
                        };
                        // Each column has its own 0-based row index (`panel_sel`).
                        let selected = tasklist_lit && panel_sel == i;
                        panel = panel.child(
                            div()
                                .flex()
                                .flex_row()
                                // Top-align so a multi-line (wrapped) entry keeps its
                                // glyph next to the first line.
                                .items_start()
                                .gap_2()
                                .w_full()
                                .min_w_0()
                                .px_1()
                                .py(px(1.0))
                                .bg(if selected { sel_bg } else { panel_transparent })
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(glyph_fg)
                                        .child(SharedString::new_static(glyph)),
                                )
                                .child(
                                    // Full text, WRAPPED (no truncation) + theme fg so
                                    // the whole plan step is readable on any scheme.
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(sidebar_fg)
                                        .child(SharedString::from(entry.content.clone())),
                                ),
                        );
                    }
                }
                _ => {
                    panel = panel.child(
                        div()
                            .px_1()
                            .py_1()
                            .text_color(sidebar_dim_fg)
                            .child(SharedString::new_static("(no plan)")),
                    );
                }
            }
            Some(probe_bounds("tasklist-panel", panel.into_any_element()))
        } else {
            None
        };

        // Subagents column — RIGHT. One row per subagent (glyph + label + spawn-
        // prompt snippet; not cards). Clicking a row focuses the subagent (swaps
        // the transcript to its output — `focus_subagent`). A left border divides
        // it from the Plan column when both are shown.
        let subagent_col: Option<gpui::AnyElement> = {
            let subagents = c.subagents();
            if !subagents_shown {
                None
            } else {
                use yalda::acp_channel::ToolCallStatus;
                let focused_key = c.focused_subagent.clone();
                let prompt_fg: Hsla = nc(at.dim);
                let mut strip = div()
                    .id("subagent-panes")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .overflow_y_scroll()
                    .border_color(if subagents_lit { focus_border } else { sidebar_border })
                    .bg(sidebar_bg)
                    .text_size(px(11.0))
                    .font_family(self.code_font.clone());
                // Segment divider: a top border only when the Plan segment sits
                // above it in the sidepanel (both open). When Subagents is the
                // only/top segment it needs no cap — the sidepanel frames it.
                if both_columns {
                    strip = strip.border_t_1();
                }
                strip = strip.child(
                    div()
                        .px_1()
                        .py(px(1.0))
                        .text_color(sidebar_header_fg)
                        .font_weight(FontWeight::BOLD)
                        .child(SharedString::new_static("Subagents")),
                );
                for (i, sa) in subagents.iter().enumerate() {
                    // Each column has its own 0-based row index (`panel_sel`).
                    let selected = subagents_lit && panel_sel == i;
                    let glyph: &'static str = match sa.status {
                        ToolCallStatus::Completed => "✓",
                        ToolCallStatus::Failed => "✗",
                        ToolCallStatus::InProgress => "●",
                        ToolCallStatus::Pending => "○",
                        _ => "·",
                    };
                    let glyph_fg: Hsla = match sa.status {
                        ToolCallStatus::Completed => nc(at.tool_completed),
                        ToolCallStatus::Failed => nc(at.tool_failed),
                        ToolCallStatus::InProgress => nc(at.tool_in_progress),
                        ToolCallStatus::Pending => nc(at.tool_pending),
                        _ => prompt_fg,
                    };
                    // Prompt snippet: first non-empty line, truncated — the user
                    // wants to see WHAT each subagent was asked, not just a label.
                    let prompt_snip: Option<String> = sa.prompt.as_deref().map(|p| {
                        let first = p.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                        if first.chars().count() > 80 {
                            format!("{}…", first.chars().take(79).collect::<String>())
                        } else {
                            first.to_string()
                        }
                    });
                    let is_focused = focused_key.as_ref() == Some(&sa.tool_call_id);
                    let label_fg = if is_focused { nc(at.warm_accent) } else { self.editor_fg() };
                    // Two-line row (UXI-AgentTile-17): line 1 = status glyph + label;
                    // line 2 = the spawn-prompt snippet, dimmed + indented under the
                    // label, on a single ellipsised line so rows stay short. The label
                    // and prompt NEVER sit side by side — the old black-label /
                    // brown-prompt one-line layout read as two cramped columns in the
                    // narrow sidepanel. `.truncate()` keeps each line to one row.
                    let mut row = div()
                        .id(SharedString::from(format!("subagent-pane-{i}")))
                        .flex()
                        .flex_col()
                        .w_full()
                        .min_w_0()
                        .px_1()
                        .py(px(1.0))
                        .bg(if selected { sel_bg } else { panel_transparent })
                        .cursor_pointer();
                    let weak = weak_self.clone();
                    let row_key = sa.tool_call_id.clone();
                    row = row.on_click(
                        move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut GpuiApp| {
                            let key = row_key.clone();
                            let _ = weak.update(app, |this, cx| {
                                this.focus_subagent(key, cx);
                            });
                        },
                    );
                    // Line 1: status glyph + label (full width, ellipsised to one line).
                    let label_el = div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(label_fg)
                        .child(SharedString::from(sa.label.clone()));
                    let label_el = if i == 0 {
                        probe_bounds("subagent-row0-label", label_el.into_any_element())
                    } else {
                        label_el.into_any_element()
                    };
                    row = row.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(glyph_fg)
                                    .child(SharedString::new_static(glyph)),
                            )
                            .child(label_el),
                    );
                    // Line 2: dimmed prompt snippet, indented under the label, one line.
                    if let Some(snip) = prompt_snip {
                        if !snip.is_empty() {
                            let prompt_el = div()
                                .w_full()
                                .min_w_0()
                                .pl_4()
                                .truncate()
                                .text_color(prompt_fg)
                                .child(SharedString::from(snip));
                            let prompt_el = if i == 0 {
                                probe_bounds("subagent-row0-prompt", prompt_el.into_any_element())
                            } else {
                                prompt_el.into_any_element()
                            };
                            row = row.child(prompt_el);
                        }
                    }
                    strip = strip.child(row);
                }
                // Tag the strip so the headless harness can read its PAINTED
                // bounds and prove the list sits above the compose (#3.2).
                Some(probe_bounds("subagent-panes", strip.into_any_element()))
            }
        };

        // Stack the segments into one fixed-width sidepanel on the RIGHT: Plan on
        // top, Subagents below. The container carries the left border dividing it
        // from the main (transcript + compose) column; the segments share the
        // sidepanel's height (each `flex_1` + own scroll).
        // UXI-AgentTile-20: `Cmd-B` force-hides the whole sidepanel even when a
        // segment has content — so gate the container on `!sidepanel_hidden`.
        let sidepanel: Option<gpui::AnyElement> = if !c.sidepanel_hidden
            && (tasklist_col.is_some() || subagent_col.is_some())
        {
            let mut side = div()
                .id("agent-sidepanel")
                .flex()
                .flex_col()
                .flex_none()
                .w(sidepanel_w)
                .h_full()
                .min_h_0()
                .border_l_1()
                .border_color(sidebar_border)
                .bg(sidebar_bg);
            if let Some(t) = tasklist_col {
                side = side.child(t);
            }
            if let Some(s) = subagent_col {
                side = side.child(s);
            }
            Some(probe_bounds("agent-sidepanel", side.into_any_element()))
        } else {
            None
        };

        let transcript_row = div().flex().flex_row().flex_1().min_h_0().child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                // Subagent context swap (UXI-AgentTile-6): while a subagent is focused
                // (`focused_subagent`, set by clicking / highlighting it in the
                // panel), the main area shows THAT subagent's context — a Back
                // header + its prompt/content/output — in place of the cached
                // transcript. `focused_subagent == None` (the common case) renders
                // the normal transcript body built before this update.
                .child(
                    match c
                        .focused_subagent
                        .clone()
                        .filter(|k| c.tools.calls.contains_key(k))
                    {
                        Some(key) => {
                            let tc = c.tools.calls.get(&key).expect("checked");
                            let label = classify_subagent(tc)
                                .map(|s| s.label)
                                .unwrap_or_else(|| "subagent".to_string());
                            let weak = weak_self.clone();
                            let back = div()
                                .id("subagent-back")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .w_full()
                                .px_4()
                                .py_1()
                                .h(px(28.0))
                                .bg(bg_or(top, STATUS_BG))
                                .text_color(strip_fg)
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(12.0))
                                .cursor_pointer()
                                .on_click(move |_ev: &gpui::ClickEvent, _w, app| {
                                    let _ = weak.update(app, |this, cx| this.unfocus_subagent(cx));
                                })
                                .child(SharedString::new_static("← Back"))
                                .child(
                                    div()
                                        .text_color(strip_dim)
                                        .font_weight(FontWeight::NORMAL)
                                        .child(SharedString::from(format!("· {label}"))),
                                );
                            // UXI-AgentTile-25: render the subagent's context as
                            // beautiful sections (markdown prompt/report, code,
                            // diffs, chips) — NOT raw JSON. No `font_family(mono)`
                            // on the container: each section sets its own font.
                            let mut content = div()
                                .flex()
                                .flex_col()
                                .text_size(px(13.0))
                                .text_color(compose_fg);
                            let tb_ctx = ToolBodyCtx {
                                theme: &self.theme,
                                body_font: self.body_font.clone(),
                                code_font: self.code_font.clone(),
                                text_scale: self.text_scale,
                                // Focused subagent view: show the whole report.
                                markdown_block_cap: None,
                            };
                            content = append_tool_body_rich(
                                content,
                                tc,
                                ToolRenderPolicy::Full,
                                &tb_ctx,
                            );
                            let body = div()
                                .id("subagent-body")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px_4()
                                .py_2()
                                .child(content);
                            probe_bounds(
                                "subagent-view",
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .child(back)
                                    .child(body)
                                    .into_any_element(),
                            )
                        }
                        None => transcript_body,
                    },
                ),
        );

        // Main column: transcript (flex_1) + recap + compose, stacked. It takes
        // the width the right sidepanel leaves.
        let mut main_col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(transcript_row);
        // Recap (recap-panel, UXI-AgentTile-15): the pinned session summary sits ABOVE
        // the compose, specific to THIS tile's session (`self.recaps[id]`). Built
        // with `weak_self` click handlers since we're inside the entity `update`
        // (no `Context<Self>` here).
        if let Some(recap_el) = self.render_agent_recap(id, weak_self.clone()) {
            main_col = main_col.child(recap_el);
        }
        if let Some(panel) = compose_panel {
            main_col = main_col.child(panel);
        }
        // Content area: the main column on the left, the segmented Plan/Subagents
        // sidepanel on the right (when either segment is open).
        let mut content_row = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(main_col);
        if let Some(side) = sidepanel {
            content_row = content_row.child(side);
        }
        let content_area: gpui::AnyElement = content_row.into_any_element();

            (header, content_area)
        });

        let root = root
            .key_context("AgentView")
            .on_key_down(cx.listener(Self::handle_claude_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            // B2: Cmd+O is Buffer-scoped. On an Agent tile `open_browser_inner`
            // is inert — it shows a "no buffer here" hint and never stashes the
            // agent. Wired here only so the hint fires; it cannot mutate the tile.
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(|this, _: &ToggleTasklist, _w, cx| {
                this.toggle_tasklist(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSubagents, _w, cx| {
                this.toggle_subagents(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusAgentPanel, _w, cx| {
                this.focus_agent_panel(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleAgentSidepanel, _w, cx| {
                this.toggle_agent_sidepanel(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleAgentInputMode, _w, cx| {
                this.toggle_agent_input_mode(cx);
            }))
            .on_action(cx.listener(Self::stop_agent))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view));
        // The per-section `[perf] agent-render` trace moved with the transcript
        // body into `TranscriptView` (ticket 021): the extract/highlight/flat
        // costs it measured now live there, behind the cached render-skip, and
        // the `YALDA_PERF` render-count + notify-reason counters are the
        // instrumentation this refactor verifies against.
        // Single top header strip over the content; the old bottom info bar
        // (which duplicated the ctx-window readout) was removed.
        root.child(header).child(content_area)
    }

    /// Render a Linear tile (`App::Linear`): a top input line (type an issue
    /// identifier like `FUL-420`, or a project name) over a scrollable body
    /// showing the fetched issue/project. Keys go through `handle_linear_key`.
    pub(crate) fn render_linear(
        &mut self,
        root: gpui::Div,
        tile: &mut LinearTile,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // The body is a cached child entity (yux) — lazily created here because
        // `restore_content` has no `cx`. Typing in the input below notifies the
        // ROOT (this view), re-rendering only the input row; the body's entity
        // isn't notified, so its render is skipped (cached). `tile` is borrowed
        // from the layout pointer, disjoint from `cx`, so `cx.new` is fine.
        let weak = cx.entity().downgrade();
        let view = tile
            .view
            .get_or_insert_with(|| cx.new(|_| LinearView::new(weak)))
            .clone();

        // ── Input line (cheap; re-renders per keystroke) ─────────────────
        let scale = self.text_scale;
        let base = px(14.0 * scale);
        let dim = nc(self.theme.agent.dim);
        let accent = nc(self.theme.agent.warm_accent);
        let fg = self.editor_fg();
        let bg = self.editor_bg();
        // Modal: Insert shows a blinking-block caret and types into the query;
        // Normal hides the caret and a hint advertises the menu / motion keys
        // (so `<space>`/`.` are discoverable as menu openers, not typed text).
        let normal = matches!(tile.mode, LinearMode::Normal);
        let (badge, badge_bg) = if normal {
            ("NORMAL", accent)
        } else {
            ("INSERT", nc(self.theme.agent.user_bar))
        };
        let input_text = if normal {
            tile.input.clone()
        } else {
            format!("{}\u{2588}", tile.input)
        };
        let trailing: AnyElement = if normal {
            div()
                .flex_none()
                .pl_2()
                .text_color(dim)
                .child(SharedString::from(
                    "space tile menu · . workspace menu · i edit · j/k browse · ⏎ open",
                ))
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let input_row = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_4()
            .py_2()
            .bg(bg_or(self.theme.top_bar, STATUS_BG))
            .border_b_1()
            .border_color(dim)
            .font_family(self.code_font.clone())
            .text_size(base)
            .child(
                div()
                    .flex_none()
                    .mr_2()
                    .px_1()
                    .text_color(bg)
                    .bg(badge_bg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::from(badge)),
            )
            .child(
                div()
                    .flex_none()
                    .pr_1()
                    .text_color(accent)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::from("linear › ")),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(fg)
                    .child(SharedString::from(input_text)),
            )
            .child(trailing);

        // ── Body (cached child; fills the remaining height) ──────────────
        let body_area = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(cached_child(view));

        root.key_context("LinearView")
            .on_key_down(cx.listener(Self::handle_linear_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .child(input_row)
            .child(body_area)
    }

    /// Render the keybindings reference tile (`App::Keymap`). The whole surface
    /// (header + filter line + grouped list) lives in the cached `KeymapView`
    /// body, so this just lazily creates + embeds it and wires the key handler +
    /// workspace actions — mirroring `render_linear`. All keys route through
    /// `handle_keymap_key` (there is no `KeymapView`-scoped keymap; navigation
    /// and rebind are handled in `on_key_down`).
    pub(crate) fn render_keymap(
        &mut self,
        root: gpui::Div,
        tile: &mut KeymapTile,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let weak = cx.entity().downgrade();
        let view = tile
            .view
            .get_or_insert_with(|| cx.new(|_| KeymapView::new(weak)))
            .clone();
        let bg = self.editor_bg();
        let body_area = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(cached_child(view));

        root.key_context("KeymapView")
            .on_key_down(cx.listener(Self::handle_keymap_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .child(body_area)
    }

    /// Render the in-tile session picker shown on an empty Agent ring
    /// (`ring.picker`): a "start a new session" row followed by the existing
    /// sessions for this cwd. Keys go through `handle_picker_key`; rows are
    /// also clickable. Selecting a row binds the ring's first slot and clears
    /// the picker, after which `render_agent` takes over.
    /// The inline "session unavailable — start fresh" screen (UXI-AgentTile-19):
    /// a REMEMBERED session couldn't be resumed on restart. NOT the picker — one
    /// click binds a fresh session in this same tile.
    pub(crate) fn render_agent_unavailable(
        &mut self,
        root: gpui::Div,
        lost: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ov = &self.theme.overlay;
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(nc(ov.bg))
            .text_color(nc(ov.accent))
            .font_weight(FontWeight::BOLD)
            .child(SharedString::new_static("⚠ session unavailable"));

        let button = div()
            .id("agent-unavailable-start-fresh")
            .flex_none()
            .px_3()
            .py_1()
            .bg(nc(ov.accent))
            .text_color(nc(ov.bg))
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .child(SharedString::new_static("Start fresh"))
            .on_click(cx.listener(|this, _ev: &gpui::ClickEvent, _w, cx| {
                this.start_fresh_after_unavailable(cx);
            }));

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .text_color(nc(ov.fg))
            .child(
                div()
                    .text_color(nc(ov.label))
                    .child(SharedString::from(format!(
                        "“{lost}” could not be resumed — it's no longer on the server."
                    ))),
            )
            .child(button);

        root.key_context("AgentUnavailableView")
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .flex()
            .flex_col()
            .size_full()
            .bg(nc(ov.bg))
            .text_color(nc(ov.fg))
            .child(header)
            .child(probe_bounds("agent-unavailable", body.into_any_element()))
    }

    pub(crate) fn render_agent_picker(
        &self,
        root: gpui::Div,
        tile: &AgentTile,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Styled to match the file browser (`render_browser`): a compact
        // header rule, a single full-width row list, and a hint footer —
        // not the old two-column card layout.
        let ov = &self.theme.overlay;
        // Rows are PROJECTED from the universal roster (universal-agent-list)
        // for the active workspace's LIVE cwd (`agent_base_cwd`) — not a per-tile
        // cache — so the selector auto-tracks rename / add / close / selection AND
        // a `Set CWD` while it's open. `free` = selectable rows 1..=N; `bound` =
        // sessions in use by some tile (informational).
        let (free, bound): (Vec<PickerSession>, Vec<PickerSession>) =
            self.picker_projection(&self.agent_base_cwd());
        // Clamp the highlight to the current row count (the projection may have
        // shrunk since the user last moved).
        let row_count = 1 + free.len();
        let selected = tile.picker()
            .map(|p| p.selected.min(row_count.saturating_sub(1)))
            .unwrap_or(0);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(nc(ov.bg))
            .text_color(nc(ov.accent))
            .font_weight(FontWeight::BOLD)
            .child(SharedString::new_static("▸ choose a session"));

        let mut list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .text_size(px(13.0))
            .font_family(self.body_font.clone());

        // Row 0 is always "start a new session".
        list = list.child(self.picker_row(
            0,
            selected == 0,
            SharedString::new_static("＋ Start a new session"),
            None,
            ov,
            cx,
        ));

        if free.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_0p5()
                    .text_color(nc(ov.label))
                    .child(SharedString::new_static(
                        "  No existing sessions for this folder.",
                    )),
            );
        } else {
            for (i, s) in free.iter().enumerate() {
                let row = i + 1;
                let liveness = if s.connected { "live" } else { "idle" };
                let sub = format!(
                    "{} turn{} · {}",
                    s.turns,
                    if s.turns == 1 { "" } else { "s" },
                    liveness,
                );
                list = list.child(self.picker_row(
                    row,
                    selected == row,
                    SharedString::from(s.label.clone()),
                    Some(SharedString::from(sub)),
                    ov,
                    cx,
                ));
            }
        }

        // Bound sessions (bound to some tile): listed dim and non-interactive —
        // they can't be attached from here (1:1 binding).
        if !bound.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_0p5()
                    .text_size(px(11.0))
                    .text_color(nc(ov.label))
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("IN USE")),
            );
            for s in bound {
                let liveness = if s.connected { "live" } else { "idle" };
                let sub = format!(
                    "{} turn{} · {}",
                    s.turns,
                    if s.turns == 1 { "" } else { "s" },
                    liveness,
                );
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .w_full()
                        .px_2()
                        .py_0p5()
                        .bg(nc(ov.bg))
                        .child(div().w(px(20.0)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_color(nc(ov.label))
                                .child(SharedString::from(s.label.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(nc(ov.label))
                                .child(SharedString::from(sub)),
                        ),
                );
            }
        }

        let hint = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(nc(ov.bg))
            .text_color(nc(ov.label))
            .text_size(px(11.0))
            .child(SharedString::new_static(
                "↑/↓ or j/k:move · enter:open · ctrl-v:back",
            ));

        root.key_context("AgentPickerView")
            .on_key_down(cx.listener(Self::handle_picker_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            // Workspace navigation must reach the picker: an unbound agent tile
            // showing the selector is still a full tile, so ctrl-<n> / cmd-shift-[]
            // (GotoWorkspace1-10 + Next/PrevWorkspace) and the jump panel must dispatch
            // here just like on a bound AgentView (bug-0011).
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .flex()
            .flex_col()
            .size_full()
            .bg(nc(ov.bg))
            .text_color(nc(ov.fg))
            .child(header)
            .child(list)
            .child(hint)
    }

    /// One row in the session picker. `row` is the activation index handed to
    /// `agent_picker_activate` on click; `is_sel` drives the highlight. Styled
    /// like `browser_row`: a marker gutter, a flex name, and a right-aligned
    /// dim meta column — full width, compact, no card/border.
    fn picker_row(
        &self,
        row: usize,
        is_sel: bool,
        title: SharedString,
        subtitle: Option<SharedString>,
        ov: &OverlayTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let row_bg = if is_sel { nc(ov.selected_bg) } else { nc(ov.bg) };
        let name_color = if is_sel { nc(ov.accent) } else { nc(ov.fg) };
        let mut r = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_2()
            .py_0p5()
            .bg(row_bg)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                    this.agent_picker_activate(row, cx);
                }),
            )
            .child(
                div()
                    .w(px(20.0))
                    .flex_none()
                    .text_color(nc(ov.key))
                    .child(SharedString::new_static(if is_sel { "▸ " } else { "  " })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_color(name_color)
                    .child(title),
            );
        if let Some(sub) = subtitle {
            r = r.child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(nc(ov.label))
                    .child(sub),
            );
        }
        r
    }

    pub(crate) fn render_browser(
        &self,
        root: gpui::Div,
        b: &BrowserWindow,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ov = &self.theme.overlay;

        // ── Worktree-mode overlay ──────────────────────────────────
        let (header, list, hint) = if let Some(wm) = &b.fb.worktree_mode {
            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(28.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.accent))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::new_static("WORKTREES"));

            let mut list = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .text_size(px(13.0))
                .font_family(self.body_font.clone());

            if wm.worktrees.is_empty() {
                list = list.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(nc(ov.label))
                        .child(SharedString::new_static("  (no worktrees)")),
                );
            } else {
                let visible_rows = 28usize;
                let scroll = scroll_to_keep_visible(wm.selected, visible_rows, wm.worktrees.len());
                for (i, wt) in wm
                    .worktrees
                    .iter()
                    .enumerate()
                    .skip(scroll)
                    .take(visible_rows)
                {
                    list = list.child(worktree_row(wt, i == wm.selected, ov));
                }
            }

            let hint = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(22.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.label))
                .text_size(px(11.0))
                .child(SharedString::new_static(
                    "enter:switch · w:close · esc:cancel",
                ));

            (header, list, hint)
        } else {
            // ── Normal file-browser view ───────────────────────────────
            let entries: Vec<&BrowserEntry> = b.fb.visible_entries();
            let selected = b.fb.selected();
            let dir_str = b.fb.current_dir().display().to_string();

            let header_text = if b.fb.filter_mode {
                format!("▸ {} — /{}", dir_str, b.fb.filter_text())
            } else {
                format!("▸ {}", dir_str)
            };

            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(28.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.accent))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(header_text));

            let mut list = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .text_size(px(13.0))
                .font_family(self.body_font.clone());

            if b.fb.filter_mode {
                // Show filter input bar
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py_1()
                        .bg(nc(ov.selected_bg))
                        .text_color(nc(ov.input))
                        .child(SharedString::from(format!(
                            "/ {}\u{2588}",
                            b.fb.filter_text()
                        ))),
                );
            }

            if entries.is_empty() {
                let msg = if b.fb.filter_mode {
                    "  (no matches)"
                } else {
                    "  (empty)"
                };
                list = list.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(nc(ov.label))
                        .child(SharedString::new_static(msg)),
                );
            } else {
                // Entry rows live in their OWN scrollable container (separate
                // from the filter bar above) so a `scroll_to_item(selected)`
                // index lines up exactly with row indices. The viewport follows
                // the cursor instead of letting it run off the bottom edge — the
                // old fixed 80-row window let the caret clip when the real
                // viewport showed fewer rows than that.
                let mut rows = div()
                    .id("browser-rows")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&b.scroll);
                for (i, entry) in entries.iter().enumerate() {
                    if i == selected
                        && let Some(r) = &b.fb.rename
                    {
                        let mut input_row = div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px_4()
                            .py_1()
                            .bg(nc(ov.selected_bg))
                            .text_color(nc(ov.input))
                            .font_family(self.code_font.clone())
                            .child(SharedString::from(format!("{}\u{2588}", r.input)));
                        if let Some(err) = &r.error {
                            input_row = input_row.child(
                                div()
                                    .pl_4()
                                    .text_size(px(11.0))
                                    .text_color(nc(ov.accent))
                                    .child(SharedString::from(err.clone())),
                            );
                        }
                        rows = rows.child(input_row);
                    } else {
                        rows = rows.child(browser_row(entry, i == selected, &self.code_font, ov));
                    }
                }
                b.scroll.scroll_to_item(selected);
                list = list.child(rows);
            }

            let hint = if b.fb.filter_mode {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_1()
                    .h(px(22.0))
                    .bg(nc(ov.bg))
                    .text_color(nc(ov.label))
                    .text_size(px(11.0))
                    .child(SharedString::new_static(
                        "enter:open · esc:cancel · type to filter",
                    ))
            } else {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_1()
                    .h(px(22.0))
                    .bg(nc(ov.bg))
                    .text_color(nc(ov.label))
                    .text_size(px(11.0))
                    .child(format!(
                        "enter:open · -:parent · /:filter · r:rename · .:menu · s:sort({}) · w:wt · q:close",
                        b.fb.sort_order.label()
                    ))
            };

            (header, list, hint)
        };

        root.key_context("BrowserView")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_browser_filter_key(ev, w, cx);
            }))
            .on_action(cx.listener(Self::browser_down))
            .on_action(cx.listener(Self::browser_up))
            .on_action(cx.listener(Self::browser_enter))
            .on_action(cx.listener(Self::browser_parent))
            .on_action(cx.listener(Self::browser_toggle_hidden))
            .on_action(cx.listener(Self::browser_cycle_sort))
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::open_local_menu))
            .on_action(cx.listener(Self::open_global_menu))
            .on_action(cx.listener(Self::browser_close))
            .on_action(cx.listener(Self::browser_worktrees))
            .on_action(cx.listener(Self::browser_filter))
            .on_action(cx.listener(Self::browser_rename))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Plane camera (spec-infinite-plane-workspace.md): Ctrl-W -/=/0.
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            .on_action(cx.listener(Self::desktop_tile_size_overlay))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .child(header)
            .child(list)
            .child(hint)
    }
}

