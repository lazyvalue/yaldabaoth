//! Screen render bodies on SketchGpuiView: render_doc / render_edit
//! (Code + WP) / render_agent / render_browser. Extracted verbatim from
//! main.rs (split-gpui-main, stage 3).

use super::*;

impl SketchGpuiView {
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

        // Splice the list to the current block count. `blocks` only changes on
        // load / reload / edit-flush (each builds a fresh `DocState`) or theme
        // switch (`set_blocks` bumps `blocks_seq`), so a count change is the
        // reliable trigger; a plain `reset` is correct
        // and cheap relative to the per-row work it gates. Must run EVERY frame.
        let new_count = d.blocks.len();
        if new_count != d.list_item_count.get() {
            d.list_state.reset(new_count);
            d.list_item_count.set(new_count);
            // Force a re-reveal below (the reset cleared scroll position).
            d.last_cursor_block.set(None);
        }
        // Keep the focused block on-screen when it changed (this also catches
        // nav actions whose `reveal_block` ran against a stale count before the
        // list was first populated).
        if d.last_cursor_block.get() != Some(d.cursor_block) {
            d.last_cursor_block.set(Some(d.cursor_block));
            if d.cursor_block < new_count {
                d.list_state.scroll_to_reveal_item(d.cursor_block);
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

        let render_fn = move |idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
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
            };
            block_element(&ctx, idx, block)
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
                gpui::list(d.list_state.clone(), render_fn)
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
            .child(format!("sketch-gpui — {}", d.file_label))
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
                let layout_sigil = self
                    .workspace
                    .active_tab()
                    .map(|t| t.layout_mode)
                    .unwrap_or_default();
                if layout_sigil == workspace::LayoutMode::Monocle {
                    // Dynamic [n/N] for monocle
                    let leaf_ids = self
                        .workspace
                        .active_tab()
                        .map(|t| t.layout.leaf_ids())
                        .unwrap_or_default();
                    let total = leaf_ids.len();
                    let pos = self
                        .workspace
                        .focused_window_id()
                        .and_then(|fid| leaf_ids.iter().position(|&id| id == fid))
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    format!(
                        "[{pos}/{total}] block {} / {}",
                        d.cursor_block.saturating_add(1),
                        d.blocks.len()
                    )
                } else if layout_sigil == workspace::LayoutMode::Manual {
                    format!(
                        "block {} / {}",
                        d.cursor_block.saturating_add(1),
                        d.blocks.len()
                    )
                } else {
                    format!(
                        "{} block {} / {}",
                        layout_sigil.sigil(),
                        d.cursor_block.saturating_add(1),
                        d.blocks.len()
                    )
                }
            })
            .child(SharedString::new_static(
                "j/k scroll · h/l block · g/G top/bot · Ctrl-O browse · Space menu",
            ));

        root.key_context("SketchView")
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
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::open_local_menu))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::next_buffer))
            .on_action(cx.listener(Self::prev_buffer))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
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
            .on_action(cx.listener(Self::resize_shrink))
            .on_action(cx.listener(Self::resize_grow))
            .on_action(cx.listener(Self::equalize))
            // Layout patterns
            .on_action(cx.listener(Self::cycle_layout_mode))
            .on_action(cx.listener(Self::desktop_panel_size_overlay))
            .on_action(cx.listener(Self::promote_to_master))
            .on_action(cx.listener(Self::increase_master_count))
            .on_action(cx.listener(Self::decrease_master_count))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_doc_selection))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
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
                "sketch-gpui [{}] — {}",
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
        let layout_prefix = {
            let lm = self
                .workspace
                .active_tab()
                .map(|t| t.layout_mode)
                .unwrap_or_default();
            if lm == workspace::LayoutMode::Manual {
                String::new()
            } else if lm == workspace::LayoutMode::Monocle {
                let leaf_ids = self
                    .workspace
                    .active_tab()
                    .map(|t| t.layout.leaf_ids())
                    .unwrap_or_default();
                let total = leaf_ids.len();
                let pos = self
                    .workspace
                    .focused_window_id()
                    .and_then(|fid| leaf_ids.iter().position(|&id| id == fid))
                    .map(|p| p + 1)
                    .unwrap_or(0);
                format!("[{pos}/{total}] ")
            } else {
                format!("{} ", lm.sigil())
            }
        };
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
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            // Layout patterns
            .on_action(cx.listener(Self::cycle_layout_mode))
            .on_action(cx.listener(Self::desktop_panel_size_overlay))
            .on_action(cx.listener(Self::promote_to_master))
            .on_action(cx.listener(Self::increase_master_count))
            .on_action(cx.listener(Self::decrease_master_count))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(body)
            .child(footer)
    }

    /// Code (raw markdown) view: monospace, gutter with line numbers,
    /// per-line `md_highlight` source colors. Cursor splice via the shared
    /// `build_line_content` helper.
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
        let line_count = lines_rc.len();

        // Splice the list to the current line count (cheap; preserves the
        // height cache for unchanged rows) and keep the cursor on-screen when
        // the buffer or caret moved.
        let new_count = line_count.max(1);
        if new_count != e.list_item_count {
            // Line count can shrink (delete) or grow (insert/paste); a plain
            // reset is correct and cheap relative to the per-row work it gates.
            e.list_state.reset(new_count);
            e.list_item_count = new_count;
        }
        let anchor = (edit_seq, cursor_line);
        if e.last_cursor_anchor != Some(anchor) {
            e.last_cursor_anchor = Some(anchor);
            if cursor_line < new_count {
                e.list_state.scroll_to_reveal_item(cursor_line);
            }
        }

        // Owned snapshots for the `'static` per-row render closure — all cheap
        // (Rc pointer clones / Copy / SharedString refcount bumps).
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let text_size = px(14.0 * self.text_scale);

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                let line_chars = line_str.chars().count();
                if let Some((s, e_col)) = line_selection_range(sel, line_idx, line_chars)
                    && e_col > s
                {
                    segs = apply_selection_bg(&segs, s, e_col, SELECTION_BG);
                }
            }

            let gutter = div()
                .w(px(40.0))
                .flex_none()
                .text_color(dim_fg)
                .child(format!("{:>3} ", line_idx + 1));

            let content = build_line_content(
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
            );

            div()
                .flex()
                .flex_row()
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
            .px_4()
            .py_2()
            .text_size(text_size)
            .font_family(self.code_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list_state.clone(), render_fn)
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
        let line_count = lines_rc.len();

        // Per-line typographic kind. `classify_wp_line` carries a running fence
        // state, so it must be folded over the buffer in order — but it's a
        // cheap byte scan (no highlighting), and precomputing it lets the
        // virtualized render closure index any visible line directly. Cheaper
        // than the per-row *element layout* the list now skips.
        let mut kinds: Vec<WpLineKind> = Vec::with_capacity(line_count);
        let mut in_fence = false;
        for line_str in lines_rc.iter() {
            let kind = classify_wp_line(line_str, in_fence);
            if matches!(kind, WpLineKind::CodeFence) {
                in_fence = !in_fence;
            }
            kinds.push(kind);
        }

        // Splice the list to line count and keep the cursor visible on edits /
        // motion (mirrors the Code view).
        let new_count = line_count.max(1);
        if new_count != e.list_item_count {
            e.list_state.reset(new_count);
            e.list_item_count = new_count;
        }
        let anchor = (edit_seq, cursor_line);
        if e.last_cursor_anchor != Some(anchor) {
            e.last_cursor_anchor = Some(anchor);
            if cursor_line < new_count {
                e.list_state.scroll_to_reveal_item(cursor_line);
            }
        }

        // Owned snapshots for the `'static` per-row closure.
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let kinds = std::rc::Rc::new(kinds);
        let body_font = self.body_font.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let text_scale = self.text_scale;

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
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
                let line_chars = line_str.chars().count();
                if let Some((s, e_col)) = line_selection_range(sel, line_idx, line_chars)
                    && e_col > s
                {
                    segs = apply_selection_bg(&segs, s, e_col, SELECTION_BG);
                }
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
                _ => (14.0, FontWeight::NORMAL, 0.0),
            };
            let text_size_px = raw_size_px * text_scale;
            let line_font = match kind {
                WpLineKind::CodeFence | WpLineKind::CodeContent | WpLineKind::TableRow => {
                    &code_font
                }
                _ => &body_font,
            };

            let content = build_line_content(
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
            );

            // Block-level decoration per kind.
            let line_div = match kind {
                WpLineKind::Blockquote => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad))
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
                    .h(px(18.0))
                    .child(content),
                _ => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad))
                    .child(content),
            };

            line_div.into_any_element()
        };

        div()
            .id("edit-body-wp")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list_state.clone(), render_fn)
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
    // The previous `build_tool_block` / `tool_body_pane` lived here as
    // `&self` methods. They've been replaced by the free-function
    // `build_tool_block_with_weak` / `tool_body_pane_free` further up
    // in the file — necessary so the per-item closure handed to
    // `gpui::list` can construct tool blocks without holding a borrow
    // of `self`.

    /// Render the Claude (ACP) screen. Frozen lines (Claude's prior turns)
    /// get a left bar + dim color; the editable region (the user's pending
    /// draft and any inline replies) renders normally with cursor splice.
    /// Header shows attach status; footer shows mode + send hint + send
    /// state ("…" while a reply is in flight).
    pub(crate) fn render_agent(
        &self,
        root: gpui::Div,
        ring: &mut AgentRing,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Legacy multi-session sidebar removed; the workspace tabs/splits
        // model is the surface for running multiple agents. Sessions
        // within a single ring remain reachable via Ctrl-]/Ctrl-[.

        let active_slot_label = ring.active().label.clone();
        // Per-slot cwd (spec-agent-cwd.md §6). Cloned before the
        // active_mut() reborrow so the Status Strip render can compare
        // against the process cwd without holding two borrows on the ring.
        let active_slot_cwd = ring.active().cwd.clone();
        let c = &mut ring.active_mut().state;

        let cursor = c.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let line_count = c.editor.document().line_count();
        let at = &self.theme.agent; // shorthand for agent theme
        let cursor_color: Hsla = nc(at.cursor);
        let dim_fg: Hsla = nc(at.dim);
        // Frozen Claude prose vs user-authored content get distinct bars so
        // the read/write boundary reads at a glance — same idiom as the
        // rendered-mode focused-block bar.
        let frozen_bar: Hsla = nc(at.frozen_bar);
        let user_bar: Hsla = nc(at.user_bar);
        // Theme-derived background tints for turn cards. Blend a faint
        // tint into the editor background so cards work on any theme.
        let base_bg: Hsla = self.editor_bg();
        let claude_turn_bg: Hsla = nc(at.agent_turn_bg);
        let user_turn_bg: Hsla = nc(at.user_turn_bg);
        let _frozen_fg: Hsla = self.editor_fg();
        let compose_panel_bg: Hsla = tint_bg(base_bg, 0.55, 0.1, 0.03);
        // Compose input text uses the theme's editor foreground so it stays
        // legible against `compose_panel_bg` on light themes (folio, FT,
        // solarized-light) — not the hardcoded Dracula light gray, which
        // vanished into the near-white panel.
        let compose_fg: Hsla = self.editor_fg();

        let perf = perf_enabled();
        // Whole-body timer: covers extract + highlight + gutter tags + block
        // parse + flat_items build + element-tree assembly (everything in
        // render_agent up to the return). GPUI's own layout/paint happens
        // after we return and is not captured here.
        let t_render0 = perf.then(std::time::Instant::now);
        let t_extract0 = perf.then(std::time::Instant::now);
        let edit_seq = c.editor.document().edit_seq();
        // Perf: only re-extract the per-line transcript text when the document
        // actually changed. On cursor-blink / cross-pane notifies edit_seq is
        // unchanged, so reuse the cached Rc verbatim instead of re-allocating a
        // String per line (an O(L) cost that previously ran every frame). The
        // Rc clone below is O(1).
        let lines_rc: std::rc::Rc<Vec<String>> = if c.lines_cache_seq == edit_seq {
            c.lines_cache.clone()
        } else {
            let built: Vec<String> = (0..line_count.max(1))
                .map(|i| {
                    c.editor
                        .document()
                        .line_text(i)
                        .trim_end_matches('\n')
                        .replace('\t', "    ")
                })
                .collect();
            let rc = std::rc::Rc::new(built);
            c.lines_cache = rc.clone();
            c.lines_cache_seq = edit_seq;
            rc
        };
        let lines: &Vec<String> = &lines_rc;
        let t_extract = t_extract0.map(|t| t.elapsed());

        // Per-line highlight, raw + stripped. The incremental cache
        // re-highlights only changed lines and hands back a cheap `Rc`
        // snapshot; the bypass path (SKETCH_HL_CACHE=0) recomputes both
        // passes in full every frame, feeding the identical closure shape so
        // the two paths are directly comparable.
        let t_hl0 = perf.then(std::time::Instant::now);
        let hl_snap: std::rc::Rc<Vec<std::rc::Rc<LineHl>>> = if hl_cache_enabled() {
            c.highlight_cache
                .snapshot_syn(lines, &self.theme, edit_seq, &self.syntect_hl)
        } else {
            let raw = highlight_markdown_lines_syn(lines, &self.theme, &self.syntect_hl);
            let stripped =
                highlight_markdown_lines_stripped_syn(lines, &self.theme, &self.syntect_hl);
            std::rc::Rc::new(
                raw.into_iter()
                    .zip(stripped)
                    .map(|(raw, stripped)| std::rc::Rc::new(LineHl { raw, stripped }))
                    .collect(),
            )
        };
        // Stash the per-section timings; the consolidated trace prints at the
        // end of render_agent so we can attribute cost across the whole body.
        let perf_hl_ms = t_hl0.map(|t| t.elapsed().as_secs_f64() * 1e3);
        let perf_extract_ms = t_extract.map(|d| d.as_secs_f64() * 1e3);
        let (perf_recomputed, perf_skip) = if hl_cache_enabled() {
            (
                c.highlight_cache.last_recomputed,
                c.highlight_cache.last_was_skip,
            )
        } else {
            (lines.len(), false)
        };
        let perf_lines = lines.len();
        let base_style = self.theme.paragraph;

        // Frozen ranges drive both the structural-block cache and the
        // blank-collapse pass below; resolve once here so they're also
        // available for the view-model fingerprint.
        let frozen_ranges: Vec<(usize, usize)> = c.editor.frozen_lines().to_vec();
        let frozen_line_count: usize = frozen_ranges.iter().map(|(s, e)| e - s).sum();

        // ── View-model memoization (S1) ──────────────────────────────
        // `flat_items` + `gutter_tag_per_line` depend ONLY on these
        // structural inputs — NOT on cursor/selection/theme, which the
        // render closure reads afterward. On cursor-blink / cross-pane
        // notify / the ~1Hz thinking tick these inputs are unchanged, so we
        // reuse the cached `Rc`s and skip the gutter scan, tool-anchor
        // resolution, flat build and blank-collapse pass.
        //
        // Trap check: `ToolCallUpdated` mutates tool-call *content* in
        // `c.tools.calls` without touching `tool_call_order` or `edit_seq`.
        // That content is rendered inside the closure from `tool_calls_snap`,
        // never baked into a `FlatItem` (ToolGroup carries only ids), so it
        // is correctly EXCLUDED from this fingerprint.
        let view_model_fp: u64 = c.view_model_fingerprint(edit_seq, frozen_line_count);

        // S1 cache: on a fingerprint hit `cached` returns the memoized `Rc`s
        // and the rebuild below is skipped entirely; on a miss the rebuild runs
        // on `&mut AgentState` (it reads `tools`/`editor` and writes
        // `c.view_model.block_cache`) and `store` stamps the fingerprint and
        // bumps `view_model_seq`. The decision lives on `AgentViewModel`.
        let theme_ref = &self.theme;
        let (flat_items_arc, gutter_tag_snap) = match c.view_model.cached(view_model_fp) {
            Some(hit) => hit,
            None => rebuild_agent_view_model(
                c,
                lines,
                &frozen_ranges,
                frozen_line_count,
                theme_ref,
                view_model_fp,
            ),
        };

        // Splice ListState to match new item count. When block ranges
        // are active, line count can shrink unpredictably, so always
        // reset. Otherwise use incremental splice for height cache.
        // (Side-effect — must run EVERY frame, so it lives OUTSIDE the
        // memoized boundary above.)
        let new_count = flat_items_arc.len();
        // Reconcile (count parity → splice/reset) stays count-keyed, but the
        // `(list_state, list_item_count)` mutation is funneled through one
        // mutator so the two can't drift (Finding 8, INV-12).
        c.reconcile_list(new_count);
        // INV-12: after reconcile, the registered count equals what we built.
        debug_assert!(
            c.list_item_count == flat_items_arc.len(),
            "list_item_count ({}) out of sync with flat_items ({})",
            c.list_item_count,
            flat_items_arc.len(),
        );

        // Follow-scroll is SEPARATE from reconcile (F4, INV-13). Re-reveal the
        // tail whenever following AND content grew since the last reveal —
        // keyed on `edit_seq`, NOT on the count delta — so an intra-line chunk
        // (agent prose before a `\n`, a streaming code fence) that bumps the
        // last item's height without adding a row still re-pins the viewport.
        // The pump functions also scroll, but they fire before render so their
        // count is stale; this is the authoritative re-reveal with the fresh
        // post-reconcile count, and also catches unfocused panes that missed
        // the pump's scroll.
        c.reveal_tail_if_following(new_count);

        // Snapshot data for the render closure. Cloned once per
        // render_agent call; the closure is then called only for
        // visible items.
        // O(1) pointer clone — the closure shares the cached line vec for the
        // frame instead of deep-copying every transcript line each render.
        let lines_snap: std::rc::Rc<Vec<String>> = lines_rc.clone();
        // The per-line highlight snapshot `hl_snap` (an O(1) pointer clone)
        // is moved into the closure below, which indexes `.raw` / `.stripped`
        // per line.
        // `gutter_tag_snap` and `flat_items_arc` come from the memoized
        // view-model tuple above (cached `Rc`s reused across frames when the
        // structural fingerprint is unchanged).
        let tool_calls_snap = c.tools.calls.clone();
        let expanded_snap = c.tools.expanded.clone();
        let frozen_lines_snap: Vec<(usize, usize)> = c.editor.frozen_lines().to_vec();
        let lockable_through_snap = c.editor.lockable_through_line();
        let sel_snap = c.editor.selection_range();
        let mode_snap = c.mode;
        let code_font_snap = self.code_font.clone();
        let body_font_snap = self.body_font.clone();
        let theme_snap = self.theme.clone();
        let at_snap = self.theme.agent.clone();
        let self_editor_fg = self.editor_fg();
        // u32 base colors for `styled_line_element`, which falls back to the
        // base for spans without an explicit fg. Theme-derived so plain
        // editable / frozen text stays legible on light themes (folio, FT)
        // instead of using the hardcoded Dracula `DEFAULT_FG`.
        let editor_fg_u32 = ncolor_to_u32(self.theme.editor_fg, DEFAULT_FG);
        let frozen_fg_u32 = ncolor_to_u32(self.theme.agent.frozen_fg, DEFAULT_FG);
        let turn_started_snap = c.turn_phase.turn_started();
        let last_event_at_snap = c.turn_phase.last_event_at();
        let weak_self = cx.entity().downgrade();

        // Helper closures for frozen-line lookup and "block starts
        // here" gating (used to gate the T-label). Inlined inside the
        // render closure.
        let is_frozen_at = move |line_idx: usize, ranges: &[(usize, usize)]| -> bool {
            ranges.iter().any(|&(s, e)| line_idx >= s && line_idx < e)
        };

        let render_fn = {
            let flat_items = flat_items_arc.clone();
            move |idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
                let item = &flat_items[idx];
                match item {
                    FlatItem::Line(line_idx) => {
                        let line_idx = *line_idx;
                        let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
                        let is_frozen = is_frozen_at(line_idx, &frozen_lines_snap);
                        let is_locked = line_idx < lockable_through_snap;
                        let _ = is_locked; // kept for future visual cue parity

                        // md_highlight segments + author tint. Frozen (Claude)
                        // lines use stripped highlights (no raw delimiters);
                        // editable (user) lines use raw highlights.
                        let mut segs: Vec<Segment> = match hl_snap.get(line_idx) {
                            Some(hl) if is_frozen => hl.stripped.clone(),
                            Some(hl) => hl.raw.clone(),
                            None => vec![(line_str.clone(), base_style)],
                        };
                        let author_tint: NColor = if is_frozen {
                            at_snap.agent_tint
                        } else {
                            at_snap.user_tint
                        };
                        for (_text, style) in segs.iter_mut() {
                            if *style == base_style {
                                *style = style.fg(author_tint);
                            }
                        }
                        if let Some(sel) = sel_snap {
                            let line_chars = line_str.chars().count();
                            if let Some((s, e_col)) =
                                line_selection_range(sel, line_idx, line_chars)
                                && e_col > s
                            {
                                segs = apply_selection_bg(&segs, s, e_col, at_snap.selection_bg);
                            }
                        }

                        // Per-line rendering uses monospace (code_font)
                        // for all lines — the token-based flex-wrap in
                        // build_wrapped_line doesn't play well with
                        // proportional fonts. Proportional rendering is
                        // handled by the FlatItem::Block path which uses
                        // body_font through block_inner/doc_styled_line_element.
                        let line_base_fg = if is_frozen {
                            frozen_fg_u32
                        } else {
                            editor_fg_u32
                        };
                        let content = build_wrapped_line(
                            &segs,
                            &line_str,
                            line_idx == cursor_line,
                            cursor_col,
                            mode_snap,
                            cursor_color,
                            base_style,
                            line_base_fg,
                            &code_font_snap,
                        );

                        let line_has_content = !line_str.trim().is_empty();
                        let bar_color: Hsla = if is_frozen {
                            frozen_bar
                        } else if line_has_content {
                            user_bar
                        } else {
                            rgba(0x00000000).into()
                        };
                        let line_text_color = if is_frozen {
                            nc(at_snap.frozen_fg)
                        } else {
                            self_editor_fg
                        };

                        // Gutter tag from the editor's per-line `TurnId`
                        // metadata (spec §11): `N` for LLM lines, `Un`
                        // for user lines, `Tn` for tool-call anchor
                        // lines, blank for currently-editable
                        // (unsubmitted) lines. Only show the label on the
                        // first line of each contiguous turn block.
                        let tag = gutter_tag_snap.get(line_idx).copied().flatten();
                        let prev_tag = if line_idx > 0 {
                            gutter_tag_snap.get(line_idx - 1).copied().flatten()
                        } else {
                            None
                        };
                        let is_first_in_turn = tag != prev_tag;
                        let (label_text, label_color): (SharedString, Hsla) = if !is_first_in_turn {
                            ("   ".into(), dim_fg)
                        } else {
                            match tag {
                                Some(TurnId::Llm(n)) => (format!("{:>3}", n).into(), frozen_bar),
                                Some(TurnId::User(n)) => {
                                    (format!("{:>3}", format!("U{}", n)).into(), user_bar)
                                }
                                Some(TurnId::Tool(n)) => (
                                    format!("{:>3}", format!("T{}", n)).into(),
                                    nc(at_snap.tool_label),
                                ),
                                // System notices carry no turn number — blank
                                // gutter, like untagged lines (Finding 5).
                                Some(TurnId::System) | None => ("   ".into(), dim_fg),
                            }
                        };
                        let card_bg: Hsla = match tag {
                            Some(TurnId::Llm(_)) => claude_turn_bg,
                            Some(TurnId::User(_)) => user_turn_bg,
                            // Tool-anchor, System-notice, and untagged lines
                            // float on the base editor_bg — no turn tint
                            // (Constraint 6, Finding 5).
                            Some(TurnId::Tool(_)) | Some(TurnId::System) | None => {
                                rgba(0x00000000).into()
                            }
                        };
                        let row_bg: Hsla = if line_idx == cursor_line {
                            // Blend cursor highlight on top of turn bg.
                            let mut h = nc(at_snap.dim);
                            h.a = 0.2;
                            h
                        } else {
                            card_bg
                        };

                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .py(px(2.0))
                            .bg(row_bg)
                            .text_color(line_text_color)
                            .child(
                                div()
                                    .w(px(28.0))
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(label_color)
                                    .font_family(code_font_snap.clone())
                                    .pr_1()
                                    .child(label_text),
                            )
                            .child(div().w(px(3.0)).flex_none().bg(bar_color).mr_2())
                            .child(content)
                            .into_any_element()
                    }
                    FlatItem::ToolGroup { anchor_line, ids } => {
                        let anchor = *anchor_line;
                        // Collect resolved tool calls for this group.
                        let calls: Vec<&sketch::acp_channel::ToolCall> = ids
                            .iter()
                            .filter_map(|id| tool_calls_snap.get(id))
                            .collect();
                        if calls.is_empty() {
                            return div().h(px(0.0)).into_any_element();
                        }
                        let group_expanded = expanded_snap.contains(&anchor.to_string());
                        let count = calls.len();

                        // Aggregate status for the group header glyph.
                        use sketch::acp_channel::ToolCallStatus;
                        let has_failed = calls.iter().any(|tc| tc.status == ToolCallStatus::Failed);
                        let has_in_progress = calls
                            .iter()
                            .any(|tc| tc.status == ToolCallStatus::InProgress);
                        let all_completed = calls
                            .iter()
                            .all(|tc| tc.status == ToolCallStatus::Completed);
                        let (group_glyph, group_color): (&str, Hsla) = if has_failed {
                            ("✗", nc(at_snap.tool_failed))
                        } else if has_in_progress {
                            ("◐", nc(at_snap.tool_in_progress))
                        } else if all_completed {
                            ("●", nc(at_snap.tool_completed))
                        } else {
                            ("○", nc(at_snap.tool_pending))
                        };

                        let header_title: String = if count == 1 {
                            let tc = calls[0];
                            let base = if tc.title.is_empty() {
                                "(tool)".to_string()
                            } else {
                                tc.title.clone()
                            };
                            // Append a useful detail for single-tool groups so
                            // the user doesn't need to expand to see *what* was
                            // read/edited/executed.
                            if let Some(detail) = tool_inline_detail(tc) {
                                format!("{} {}", base, detail)
                            } else {
                                base
                            }
                        } else {
                            // Typed summary of the run: count each tool label in
                            // first-appearance order → "4 grep, 3 edit, 7 read".
                            let mut order: Vec<String> = Vec::new();
                            let mut counts: std::collections::HashMap<String, usize> =
                                std::collections::HashMap::new();
                            for tc in &calls {
                                let label = tool_type_label(tc);
                                if !counts.contains_key(&label) {
                                    order.push(label.clone());
                                }
                                *counts.entry(label).or_insert(0) += 1;
                            }
                            order
                                .iter()
                                .map(|l| format!("{} {}", counts[l], l))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };

                        // For single-tool groups: determine if the inner tool
                        // has a body worth showing. If HeaderOnly, the header
                        // line is the entire UI — no expand arrow, no nesting.
                        let single_policy = if count == 1 {
                            Some(tool_render_policy(calls[0]))
                        } else {
                            None
                        };
                        let expandable = if count > 1 {
                            true
                        } else {
                            !matches!(single_policy, Some(ToolRenderPolicy::HeaderOnly))
                        };
                        let arrow = if !expandable {
                            " "
                        } else if group_expanded {
                            "▼"
                        } else {
                            "▶"
                        };

                        let anchor_str = anchor.to_string();
                        let weak = weak_self.clone();
                        let click_id = anchor_str.clone();
                        let mut header_row = div()
                            .id(SharedString::from(format!("tool-group-{}", anchor)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py(px(6.0))
                            .px_2()
                            .child(div().text_color(dim_fg).child(arrow))
                            .child(div().text_color(group_color).child(group_glyph))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(self_editor_fg)
                                    .text_size(px(12.0))
                                    .child(header_title),
                            );

                        if expandable {
                            header_row = header_row.cursor_pointer().on_click(
                                move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                                    let id = click_id.clone();
                                    let _ = weak.update(app, |this, cx| {
                                        if let Some(c) = this.agent_mut() {
                                            c.tools.toggle_expanded(&id);
                                        }
                                        cx.notify();
                                    });
                                },
                            );
                        }

                        let mut block = div()
                            .flex()
                            .flex_col()
                            .mt(px(16.0))
                            .mb(px(8.0))
                            .mx_4()
                            .child(header_row);

                        // Expanded: show contents.
                        if group_expanded && expandable {
                            if count == 1 {
                                // Single-tool group: render body directly
                                // under the header — no nested sub-header.
                                let tc = calls[0];
                                block = append_tool_body(
                                    block,
                                    tc,
                                    single_policy.unwrap_or(ToolRenderPolicy::Full),
                                    &code_font_snap,
                                    &at_snap,
                                );
                            } else {
                                for tc in &calls {
                                    let expanded_detail =
                                        expanded_snap.contains(&tc.tool_call_id.0.to_string());
                                    block = block.child(build_tool_block_with_weak(
                                        tc,
                                        expanded_detail,
                                        &code_font_snap,
                                        weak_self.clone(),
                                        &at_snap,
                                    ));
                                }
                            }
                        }

                        block.into_any_element()
                    }
                    FlatItem::Block(rendered_block) => {
                        let ctx = RenderCtx {
                            theme: &theme_snap,
                            body_font: body_font_snap.clone(),
                            code_font: code_font_snap.clone(),
                            // Claude session chat blocks stay at fixed size —
                            // Cmd-zoom is scoped to the document view.
                            text_scale: 1.0,
                            cursor_block: None,
                            doc_selection: None,
                            line_layouts: None,
                            current_block: None,
                            // Wiki link clicks in Claude messages aren't
                            // wired up — they'd need a per-message source
                            // path which we don't track. Skip for v1.
                            weak_view: None,
                            doc_dir: None,
                        };
                        let inner = block_inner(&ctx, rendered_block);
                        div()
                            .mt(px(4.0))
                            .mb(px(4.0))
                            .child(inner)
                            .into_any_element()
                    }
                    FlatItem::TurnHeader { role } => {
                        let (label, accent): (&str, Hsla) = match role {
                            TurnRole::Claude => ("Claude", nc(at_snap.turn_header_agent)),
                            TurnRole::User => ("You", nc(at_snap.turn_header_user)),
                        };
                        let rule_color = nc(at_snap.turn_rule);
                        // TurnHeaders float on editor_bg — no turn tint
                        // (Constraint 6). The neutral gap between tinted
                        // text bands is the visual separator.
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .w_full()
                            .pt(px(32.0))
                            .pb(px(8.0))
                            .px_4()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(accent)
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .child(div().flex_1().h(px(1.0)).bg(rule_color))
                            .into_any_element()
                    }
                    FlatItem::ThinkingIndicator => {
                        // Pulsing dot: opacity cycles 0.3–1.0 on a sine wave.
                        let phase = if let Some(t) = turn_started_snap {
                            let ms = t.elapsed().as_millis() as f64;
                            ((ms / 750.0).sin() * 0.5 + 0.5) as f32
                        } else {
                            1.0
                        };
                        let alpha = 0.3 + phase * 0.7;

                        // Live elapsed (since the prompt was sent) and quiet
                        // time (since the last streamed event). A streaming
                        // turn keeps `quiet` near zero; a stall lets it climb,
                        // which is the tell that the API — not sketch — is
                        // wedged. Past STALL_WARN_S we switch to an explicit
                        // warning so the user knows it's abnormal.
                        const STALL_WARN_S: u64 = 30;
                        let elapsed_s = turn_started_snap
                            .map(|t| t.elapsed().as_secs())
                            .unwrap_or(0);
                        let quiet_s = last_event_at_snap
                            .map(|t| t.elapsed().as_secs())
                            .unwrap_or(0);
                        let fmt_ms = |s: u64| format!("{}:{:02}", s / 60, s % 60);
                        let stalled = quiet_s >= STALL_WARN_S;

                        let dot_color = if stalled {
                            // Amber when stalled, regardless of pulse phase.
                            nc(at_snap.warm_accent)
                        } else {
                            Hsla {
                                h: 0.53,
                                s: 0.9,
                                l: 0.76,
                                a: alpha,
                            }
                        };
                        let (label, label_color) = if stalled {
                            (
                                format!(
                                    "No reply for {} (running {}) — the API may be overloaded. ⌘. to stop · ⌘. again to force-restart",
                                    fmt_ms(quiet_s),
                                    fmt_ms(elapsed_s),
                                ),
                                nc(at_snap.warm_accent),
                            )
                        } else {
                            (
                                format!("Thinking\u{2026} {}", fmt_ms(elapsed_s)),
                                Hsla {
                                    h: 0.0,
                                    s: 0.0,
                                    l: 0.6,
                                    a: alpha,
                                },
                            )
                        };
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .pt_3()
                            .pb_2()
                            .pl_1()
                            .pr_4()
                            .gap_2()
                            .child(div().text_size(px(14.0)).text_color(dot_color).child("●"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.0))
                                    .text_color(label_color)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .into_any_element()
                    }
                }
            }
        };

        let body = div()
            .id("claude-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_3()
            .text_size(px(13.0))
            .font_family(self.code_font.clone())
            .text_color(self.editor_fg())
            .child(
                gpui::list(c.list_state.clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            );

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        // ---- Status Strip (spec §30) ----
        // Single-row header showing agent label, sub-agent breadcrumb
        // (when focused), model id, permission mode, context-window
        // usage + cost (when present), and turn / elapsed. Any field
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
        let proc_cwd = process_cwd();
        if active_slot_cwd != proc_cwd {
            let shortened = shorten_cwd_for_display(&active_slot_cwd);
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(shortened)),
            );
        }

        // Model id (best-effort: agent_mode → channel description).
        let model_label: Option<String> = c
            .agent_mode
            .as_ref()
            .map(|m| m.0.to_string())
            .or_else(|| c.channel.as_ref().map(|ch| ch.command().to_string()));
        if let Some(m) = model_label {
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(m)),
            );
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
            let is_yolo = matches!(mode, sketch::acp_channel::PermissionMode::Yolo);
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

        // Context-window usage + cost (when the unstable feature is on
        // and the agent has emitted a UsageUpdate).
        if let Some(usage) = &c.usage {
            let used_k = (usage.tokens_used as f64) / 1000.0;
            let total_k = (usage.tokens_total as f64) / 1000.0;
            let pct = if usage.tokens_total > 0 {
                (usage.tokens_used as f64 / usage.tokens_total as f64) * 100.0
            } else {
                0.0
            };
            let usage_text = format!("{:.1}k / {:.0}k ({:.0}%)", used_k, total_k, pct);
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(usage_text)),
            );
            if let Some(cost) = usage.cost_usd {
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_dim)
                        .child(SharedString::from(format!("${:.2}", cost))),
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
            strip = strip.child(div().flex_1()).child(
                div()
                    .text_color(turn_color)
                    .child(SharedString::from(label)),
            );
        }

        let header = strip;

        // ---- Agent Info Bar ----
        // Dedicated status bar showing context-window size, cwd, and active
        // subagents. Position (top/bottom) is a user preference.
        let info_bar = {
            use sketch::acp_channel::ToolCallStatus;

            // Context window segment.
            let ctx_text: String = if let Some(usage) = &c.usage {
                let used_k = (usage.tokens_used as f64) / 1000.0;
                let total_k = (usage.tokens_total as f64) / 1000.0;
                let pct = if usage.tokens_total > 0 {
                    (usage.tokens_used as f64 / usage.tokens_total as f64) * 100.0
                } else {
                    0.0
                };
                format!("{:.1}k / {:.0}k ({:.0}%)", used_k, total_k, pct)
            } else {
                "\u{2014}".to_string()
            };

            // Cwd segment — always shown.
            let cwd_text = shorten_cwd_for_display(&active_slot_cwd);

            // Subagents segment — show in-progress agents with glyphs.
            let agents_text: String = {
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
                if active.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    active.join("  ")
                }
            };

            let sep_color: Hsla = nc(at.turn_rule);

            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .py_1()
                .h(px(22.0))
                .bg(bg_or(bot, STATUS_BG))
                .text_color(fg_or(bot, 0x666666))
                .text_size(px(11.0))
                .font_family(self.code_font.clone())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("ctx")),
                        )
                        .child(SharedString::from(ctx_text)),
                )
                .child(
                    div()
                        .text_color(sep_color)
                        .child(SharedString::new_static("\u{00b7}")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("cwd")),
                        )
                        .child(SharedString::from(cwd_text)),
                )
                .child(
                    div()
                        .text_color(sep_color)
                        .child(SharedString::new_static("\u{00b7}")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("agents")),
                        )
                        .child(SharedString::from(agents_text)),
                )
        };

        let in_chatbox = c.input_surface.is_chatbox();
        let mode_label = if in_chatbox {
            match c.input_surface.chatbox().unwrap().mode {
                EditMode::Normal => "CHATBOX",
                EditMode::Insert => "CHATBOX INSERT",
            }
        } else {
            match c.mode {
                EditMode::Normal => "WORKSHEET",
                EditMode::Insert => "WORKSHEET INSERT",
            }
        };
        let dirty_mark = if c.editor.document().is_modified() {
            "•"
        } else {
            " "
        };
        let extend_mark = if c.editor.extend_mode() { " EXT" } else { "" };
        let mut left_status = format!(
            "{} CLAUDE {}{} · L{}:C{}",
            dirty_mark,
            mode_label,
            extend_mark,
            cursor_line + 1,
            cursor_col + 1,
        );
        if c.turn_phase.is_awaiting() {
            left_status.push_str(" · …awaiting reply");
        }
        if let Some(msg) = &c.status {
            left_status.push_str("  [");
            left_status.push_str(msg);
            left_status.push(']');
        }
        // dim_fg is now used actively via agent theme

        let hints = if in_chatbox {
            "Ctrl-Enter send · Ctrl-Alt-Enter worksheet · esc normal"
        } else {
            "Ctrl-Enter send · Ctrl-Alt-Enter chatbox · Ctrl-V back · i insert · esc normal"
        };

        // Right side of the footer: a Stop button (only while a reply is in
        // flight) followed by the key hints. The button dispatches the same
        // StopAgent path as Cmd-. — ACP session/cancel for the active turn.
        let stop_fg: Hsla = nc(at.tool_failed);
        let mut footer_right = div().flex().flex_row().items_center().gap_2();
        if c.turn_phase.is_awaiting() {
            // After a graceful cancel is already pending, the button (and
            // ⌘.) escalate to a hard kill + resume.
            let escalating = c.turn_phase.stop_requested();
            let stop_label = if escalating {
                "■ Force-restart ⌘."
            } else {
                "■ Stop ⌘."
            };
            let weak_stop = cx.entity().downgrade();
            footer_right = footer_right.child(
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
                        move |_ev: &gpui::ClickEvent, window: &mut Window, app: &mut App| {
                            let _ = weak_stop.update(app, |this, cx| {
                                this.stop_agent(&StopAgent, window, cx);
                            });
                        },
                    )
                    .child(SharedString::from(stop_label)),
            );
        }
        footer_right = footer_right.child(SharedString::from(hints));

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
            .child(footer_right);

        // Chatbox panel — rendered between body and footer when active.
        //
        // Each line is rendered as a non-wrapping row inside a per-line
        // overflow_hidden clip container. The cursor line is shifted left
        // via a negative pixel margin so the caret stays visible. The clip
        // container inherits its width from the flex layout — no need to
        // know the pixel width at render time.
        let compose_panel = if let InputSurface::Chatbox(tb) = &mut c.input_surface {
            let compose_lines: Vec<String> = {
                let doc = tb.editor.document();
                (0..doc.line_count().max(1))
                    .map(|i| {
                        doc.line_text(i)
                            .trim_end_matches('\n')
                            .replace('\t', "    ")
                    })
                    .collect()
            };
            let compose_cursor_line = tb.editor.cursor().line;
            let compose_cursor_col = tb.editor.cursor().col;
            let compose_mode = tb.mode;
            let compose_sel = tb.editor.selection_range();
            let sep_color: Hsla = nc(at.compose_separator);
            let compose_cursor_color: Hsla = nc(at.cursor);
            let compose_code_font = self.code_font.clone();

            let separator = div().w_full().h(px(1.0)).bg(dim_fg);

            // Cap height at ~8 logical lines, then vertical scroll kicks in.
            // Wrapped lines may exceed one row visually, so the actual cap
            // can show fewer logical lines when text wraps — that's fine,
            // overflow_y_scroll handles it.
            let max_visible_h = 8.0 * 18.0f32;

            let compose_scroll = tb.scroll_handle.clone();
            // scroll_to_item only sees direct children of the scroll
            // container, so each logical line is added straight to
            // `compose_body` (no intermediate wrapper) — that's what keeps
            // the cursor in view when the user types past the visible area.
            compose_scroll.scroll_to_item(compose_cursor_line);

            let mut compose_body = div()
                .id("compose-scroll")
                .w_full()
                .min_w_0()
                .max_h(px(max_visible_h))
                .overflow_y_scroll()
                .overflow_x_hidden()
                .track_scroll(&compose_scroll)
                .px_4()
                .py(px(8.0))
                .bg(compose_panel_bg)
                .border_1()
                .border_color(dim_fg)
                .rounded_md()
                .mx_2()
                .mb_1()
                .font_family(compose_code_font.clone())
                .text_size(px(13.0))
                .text_color(compose_fg);

            for (i, line_text) in compose_lines.iter().enumerate() {
                let is_cursor_line = i == compose_cursor_line;
                let total_chars = line_text.chars().count();
                let line_el = build_chatbox_line(
                    line_text,
                    is_cursor_line,
                    compose_cursor_col,
                    compose_mode,
                    compose_cursor_color,
                    compose_sel,
                    i,
                    total_chars,
                    &compose_code_font,
                    compose_fg,
                );
                compose_body = compose_body.child(line_el);
            }

            // Top edge: a 1px darker rule creates a subtle
            // visual separation between the scrolling transcript
            // and the fixed compose panel.
            let edge_color = {
                let mut h = sep_color;
                h.a = 0.4;
                h
            };
            Some(
                div()
                    .w_full()
                    .min_w_0()
                    .border_t_1()
                    .border_color(edge_color)
                    .child(separator)
                    .child(compose_body),
            )
        } else {
            None
        };

        // ---- Right-side sidepanes (Tasklist / Subagents) ----
        //
        // Stacked horizontally in fixed order (Tasklist innermost, then
        // Subagents) per spec §2. Each pane is a fixed 28-char column;
        // the transcript area's flex-1 shrinks to make room. Panes only
        // render when their `*_open` flag is true.
        let pane_width = px(28.0 * 7.0); // ~28 monospace cols at 13px = ~196px
        let pane_border: Hsla = nc(at.pane_border);
        let pane_header_fg: Hsla = nc(at.pane_header);
        let pane_dim_fg: Hsla = nc(at.dim);
        let pane_bg: Hsla = nc(at.pane_bg);

        let tasklist_pane = if c.tasklist_open {
            let mut pane = div()
                .id("tasklist-pane")
                .flex()
                .flex_col()
                .w(pane_width)
                .min_w(pane_width)
                .flex_none()
                .bg(pane_bg)
                .border_l_1()
                .border_color(pane_border)
                .py_1()
                .text_size(px(12.0))
                .font_family(self.code_font.clone());
            pane = pane.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(pane_header_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("Plan")),
            );
            match &c.current_plan {
                Some(plan) if !plan.entries.is_empty() => {
                    use sketch::acp_channel::PlanEntryStatus;
                    for entry in &plan.entries {
                        let glyph: &'static str = match entry.status {
                            PlanEntryStatus::Completed => "✓",
                            PlanEntryStatus::InProgress => "●",
                            PlanEntryStatus::Pending => "○",
                            // ACP marks the enum #[non_exhaustive]; a
                            // future "failed" or similar status falls
                            // back to a clear indicator (§22).
                            _ => "✗",
                        };
                        let line_text = if entry.content.chars().count() > 22 {
                            let truncated: String = entry.content.chars().take(21).collect();
                            format!("{}  {}…", glyph, truncated)
                        } else {
                            format!("{}  {}", glyph, entry.content)
                        };
                        pane = pane.child(
                            div()
                                .px_2()
                                .py(px(1.0))
                                .text_color(rgb(DEFAULT_FG))
                                .child(SharedString::from(line_text)),
                        );
                    }
                }
                _ => {
                    pane = pane.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(pane_dim_fg)
                            .child(SharedString::new_static("(no plan)")),
                    );
                }
            }
            Some(pane)
        } else {
            None
        };

        let subagents_pane = if c.subagents_open {
            let mut pane = div()
                .id("subagents-pane")
                .flex()
                .flex_col()
                .w(pane_width)
                .min_w(pane_width)
                .flex_none()
                .bg(pane_bg)
                .border_l_1()
                .border_color(pane_border)
                .py_1()
                .text_size(px(12.0))
                .font_family(self.code_font.clone());
            pane = pane.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(pane_header_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("Subagents")),
            );
            let subagents = c.subagents();
            if subagents.is_empty() {
                pane = pane.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(pane_dim_fg)
                        .child(SharedString::new_static("(no subagents)")),
                );
            } else {
                use sketch::acp_channel::ToolCallStatus;
                let focused_key = c.focused_subagent.clone();
                for (i, sa) in subagents.iter().enumerate() {
                    let glyph: &'static str = match sa.status {
                        ToolCallStatus::Completed => "✓",
                        ToolCallStatus::Failed => "✗",
                        ToolCallStatus::InProgress => "●",
                        ToolCallStatus::Pending => "○",
                        _ => "·",
                    };
                    let trunc_label: String = if sa.label.chars().count() > 20 {
                        let head: String = sa.label.chars().take(19).collect();
                        format!("{}…", head)
                    } else {
                        sa.label.clone()
                    };
                    let row_text = format!("▸ {} {}", glyph, trunc_label);
                    let is_focused = focused_key.as_ref() == Some(&sa.tool_call_id);
                    let row_fg: Hsla = if is_focused {
                        nc(at.warm_accent)
                    } else {
                        self.editor_fg()
                    };
                    let row_bg: Hsla = if is_focused {
                        let mut h = nc(at.dim);
                        h.a = 0.2;
                        h
                    } else {
                        rgba(0x00000000).into()
                    };
                    let weak = cx.entity().downgrade();
                    let row_key = sa.tool_call_id.clone();
                    let row = div()
                        .id(SharedString::from(format!("subagent-row-{}", i)))
                        .px_2()
                        .py(px(1.0))
                        .cursor_pointer()
                        .text_color(row_fg)
                        .bg(row_bg)
                        .on_click(
                            move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                                let key = row_key.clone();
                                let _ = weak.update(app, |this, cx| {
                                    this.focus_subagent(key, cx);
                                });
                            },
                        )
                        .child(SharedString::from(row_text));
                    pane = pane.child(row);
                }
            }
            Some(pane)
        } else {
            None
        };

        let mut transcript_row = div().flex().flex_row().flex_1().min_h_0().child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(body),
        );
        if let Some(p) = tasklist_pane {
            transcript_row = transcript_row.child(p);
        }
        if let Some(p) = subagents_pane {
            transcript_row = transcript_row.child(p);
        }

        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(transcript_row);
        if let Some(panel) = compose_panel {
            col = col.child(panel);
        }
        let content_area: gpui::AnyElement = col.into_any_element();

        // Build-loop candidate banner. Sits above the status strip so a
        // read-only mirror is unmistakable. Amber while the original owner
        // still holds the sessions; green once it has closed and take-over
        // will succeed.
        let candidate_banner = if self.is_candidate {
            let (bar_bg, text): (Hsla, &'static str) = if self.candidate_promote_ready {
                (
                    rgb(0x50fa7b).into(),
                    "✓ CANDIDATE · original closed — menu → claude → take over (P) to go live",
                )
            } else {
                (
                    rgb(0xffb86c).into(),
                    "🔭 CANDIDATE · read-only mirror — close the original window, then menu → claude → take over (P)",
                )
            };
            Some(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px_4()
                    .py_1()
                    .h(px(24.0))
                    .bg(bar_bg)
                    .text_color(rgb(0x1e1e2e))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static(text)),
            )
        } else {
            None
        };

        let mut root = root
            .key_context("AgentView")
            .on_key_down(cx.listener(Self::handle_claude_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(|this, _: &ToggleTasklist, _w, cx| {
                this.toggle_tasklist(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSubagents, _w, cx| {
                this.toggle_subagents(cx);
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
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Layout patterns
            .on_action(cx.listener(Self::cycle_layout_mode))
            .on_action(cx.listener(Self::desktop_panel_size_overlay))
            .on_action(cx.listener(Self::promote_to_master))
            .on_action(cx.listener(Self::increase_master_count))
            .on_action(cx.listener(Self::decrease_master_count))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view));
        if let Some(banner) = candidate_banner {
            root = root.child(banner);
        }
        let out = match self.agent_status_position {
            AgentStatusPosition::Top => root
                .child(header)
                .child(info_bar)
                .child(content_area)
                .child(footer),
            AgentStatusPosition::Bottom => root
                .child(header)
                .child(content_area)
                .child(info_bar)
                .child(footer),
        };

        if let Some(t0) = t_render0 {
            let total_ms = t0.elapsed().as_secs_f64() * 1e3;
            let extract_ms = perf_extract_ms.unwrap_or(0.0);
            let hl_ms = perf_hl_ms.unwrap_or(0.0);
            // `rest` = the untimed remainder inside render_agent: gutter tags,
            // block detect/parse, flat_items build, element-tree assembly.
            // If total is large but extract+hl are small, the cost is here
            // (or in GPUI layout after we return — not captured).
            let rest_ms = (total_ms - extract_ms - hl_ms).max(0.0);
            eprintln!(
                "[perf] agent-render lines={perf_lines} total={total_ms:.2}ms \
                 extract={extract_ms:.2}ms hl={hl_ms:.2}ms rest={rest_ms:.2}ms \
                 recomputed={perf_recomputed} skip={perf_skip} cache={}",
                if hl_cache_enabled() { "on" } else { "off" },
            );
        }
        out
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
                let visible_rows = 80usize;
                let scroll = scroll_to_keep_visible(selected, visible_rows, entries.len());
                for (i, entry) in entries.iter().enumerate().skip(scroll).take(visible_rows) {
                    list = list.child(browser_row(entry, i == selected, &self.code_font, ov));
                }
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
                        "enter:open · -:parent · /:filter · .:menu · s:sort({}) · w:wt · q:close",
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
            .on_action(cx.listener(Self::browser_close))
            .on_action(cx.listener(Self::browser_worktrees))
            .on_action(cx.listener(Self::browser_filter))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Layout patterns
            .on_action(cx.listener(Self::cycle_layout_mode))
            .on_action(cx.listener(Self::desktop_panel_size_overlay))
            .on_action(cx.listener(Self::promote_to_master))
            .on_action(cx.listener(Self::increase_master_count))
            .on_action(cx.listener(Self::decrease_master_count))
            .on_action(cx.listener(Self::tag_view_chord))
            .on_action(cx.listener(Self::tag_toggle_chord))
            .on_action(cx.listener(Self::clear_tag_view))
            .child(header)
            .child(list)
            .child(hint)
    }
}
