//! Doc (Buffer `Viewing`) methods on the root view: block navigation, the
//! painted-Doc re-derive, view-mode mouse selection + copy, and the Doc key
//! handler. The data layer is `doc.rs`; the cached body is `doc_view.rs`; the
//! screen chrome (header/footer/action wiring) is `screens.rs::render_doc`.

use super::*;

impl YaldaGpuiView {
    /// `Some(doc)` if currently viewing a document, else `None`.
    pub(crate) fn doc_mut(&mut self) -> Option<&mut DocState> {
        match self.workspace.focused_content_mut()? {
            App::Buffer(BufferApp::Viewing(d)) => Some(d),
            _ => None,
        }
    }

    /// The `DocState` of the Viewing tile `id` (None if gone / not a Doc).
    pub(crate) fn doc_state_ref(&self, id: workspace::WindowId) -> Option<&DocState> {
        match &self.workspace.tile(id)?.content {
            App::Buffer(BufferApp::Viewing(d)) => Some(d),
            _ => None,
        }
    }

    /// A code block's copy button (`render_blocks::code_copy_button`): write
    /// the plain text of the code block at `path` in tile `tile`'s CURRENT
    /// blocks to the clipboard, and flag it "Copied" for a moment. Resolved
    /// here, at event time, never from data captured at build time.
    pub(crate) fn copy_doc_code_block(
        &mut self,
        tile: workspace::WindowId,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        let Some(App::Buffer(BufferApp::Viewing(d))) =
            self.workspace.tile_mut(tile).map(|w| &mut w.content)
        else {
            return;
        };
        let Some(text) = block_at_path(&d.blocks, path).and_then(code_block_text) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        d.code_copied = Some(Rc::new(path.to_vec()));
        d.code_copied_seq = d.code_copied_seq.wrapping_add(1);
        let seq = d.code_copied_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(App::Buffer(BufferApp::Viewing(d))) =
                    view.workspace.tile_mut(tile).map(|w| &mut w.content)
                    && d.code_copied_seq == seq
                {
                    d.code_copied = None;
                    d.code_copied_seq = d.code_copied_seq.wrapping_add(1);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- Painted-Doc re-derive (the effect path) ---------------------------

    /// C3: re-derive (`DocState::refresh_blocks`) only the Doc tiles that can be
    /// painted — the solo-presented tile if any, else the ACTIVE workspace's
    /// layout leaves. Never hidden tiles or other workspaces: a sibling Edit
    /// keystroke must not re-parse a Doc nobody can see. A skipped Doc is stale
    /// only while invisible; whatever makes it visible (workspace switch,
    /// presentation, split) notifies the root, and this runs before the next
    /// draw. O(1) per Doc when its core is unchanged. Mutation-only, never
    /// notifies.
    pub(crate) fn refresh_painted_docs(&mut self) {
        let theme = &self.theme;
        let mut refresh = |content: &mut App| {
            if let App::Buffer(BufferApp::Viewing(d)) = content {
                d.refresh_blocks(theme);
            }
        };
        if let Some(presentation) = self.workspace.presented_tile() {
            if let Some(tile) = self.workspace.tile_mut(presentation.window_id()) {
                refresh(&mut tile.content);
            }
            return;
        }
        if let Some(wsp) = self.workspace.active_workspace_mut() {
            wsp.layout.for_each_leaf_content_mut(&mut refresh);
        }
    }

    /// Hook `refresh_painted_docs` onto the root's own notify (graph 4f1
    /// doc-view): a root `cx.observe_self` callback runs in effect flush —
    /// after the mutation that notified, before the next draw — so the Doc
    /// re-derive happens on the mutation/effect path, never inside a render.
    /// Every event that edits a pooled core (Edit keystrokes, reloads, undo) or
    /// changes which tiles paint (workspace switch, presentation, splits)
    /// notifies the root. Installed once, lazily from the first root render
    /// (the ~60 construction sites have no `cx`); the first frame's Docs are
    /// fresh from construction, so nothing is missed before it.
    pub(crate) fn ensure_doc_refresh_hook(&mut self, cx: &mut Context<Self>) {
        if self.doc_refresh_hooked {
            return;
        }
        self.doc_refresh_hooked = true;
        cx.observe_self(|this, _cx| this.refresh_painted_docs())
            .detach();
    }

    // ---- Block navigation --------------------------------------------------

    /// Move the focused Doc's block cursor to the block `target` picks (given
    /// the doc), reveal it, and repaint. `target` returning `None` is a no-op.
    /// The one body behind j/k, ctrl-d/u, g/G and the local-menu gotos.
    pub(crate) fn doc_nav(
        &mut self,
        cx: &mut Context<Self>,
        target: impl FnOnce(&DocState) -> Option<usize>,
    ) {
        if let Some(d) = self.doc_mut()
            && let Some(to) = target(d)
            && d.move_cursor_to(to)
        {
            cx.notify();
        }
    }

    /// j / ↓ — next painted block (stops at the last; skips folded sections,
    /// UXI-Buffer-14).
    fn doc_next_block(d: &DocState) -> Option<usize> {
        d.visible_step(d.cursor_block, 1)
    }

    /// k / ↑ — previous painted block (stops at the first).
    fn doc_prev_block(d: &DocState) -> Option<usize> {
        d.visible_step(d.cursor_block, -1)
    }

    pub(crate) fn scroll_down(&mut self, _: &ScrollDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_next_block);
    }
    pub(crate) fn scroll_up(&mut self, _: &ScrollUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_prev_block);
    }
    pub(crate) fn page_down(&mut self, _: &ScrollPageDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, |d| d.visible_step(d.cursor_block, 8).or(Some(d.cursor_block)));
    }
    pub(crate) fn page_up(&mut self, _: &ScrollPageUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, |d| d.visible_step(d.cursor_block, -8).or(Some(d.cursor_block)));
    }
    pub(crate) fn cursor_next(&mut self, _: &CursorNextBlock, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_next_block);
    }
    pub(crate) fn cursor_prev(&mut self, _: &CursorPrevBlock, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_prev_block);
    }
    pub(crate) fn cursor_top(&mut self, _: &CursorTop, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, |_| Some(0));
    }
    pub(crate) fn cursor_bottom(&mut self, _: &CursorBottom, _w: &mut Window, cx: &mut Context<Self>) {
        // The last PAINTED block — never into a folded tail.
        self.doc_nav(cx, DocState::last_visible);
    }

    /// Move the doc cursor to the next block (wrapping past EOF) matching
    /// `pred`. Local-menu `navigate`/`goto` commands (spec-menu-scopes.md).
    pub(crate) fn doc_jump_next_matching(
        &mut self,
        label: &str,
        pred: fn(&RenderedBlock) -> bool,
        cx: &mut Context<Self>,
    ) {
        let target = match self.doc_mut() {
            Some(d) if !d.blocks.is_empty() => {
                let n = d.blocks.len();
                let start = d.cursor_block.min(n - 1);
                (1..=n)
                    .map(|off| (start + off) % n)
                    .find(|&i| pred(&d.blocks[i]))
            }
            _ => return,
        };
        match target {
            Some(idx) => {
                if let Some(d) = self.doc_mut() {
                    d.move_cursor_to(idx);
                }
            }
            None => {
                self.transient_status = Some(format!("no {label} in document").into());
            }
        }
        cx.notify();
    }

    /// Test seam: invalidate every live Doc body (and the root) so the next
    /// frame RE-RUNS their render + paint. A cached body otherwise replays its
    /// last paint on a root-only notify — which records no layout probes /
    /// render-tap entries — so a harness that wants to observe "what a fresh
    /// frame paints" forces it through here (the `md_harness::probe` idiom the
    /// Edit body uses).
    #[cfg(test)]
    pub(crate) fn test_notify_doc_bodies(&self, cx: &mut Context<Self>) {
        let mut bodies: Vec<Entity<DocView>> = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.for_each_attached_window(&mut |w| {
                if let App::Buffer(BufferApp::Viewing(d)) = &w.content
                    && let Some(b) = &d.body
                {
                    bodies.push(b.clone());
                }
            });
        }
        for b in bodies {
            b.update(cx, |_, bcx| bcx.notify());
        }
        cx.notify();
    }

    // ---- View-mode mouse selection -----------------------------------------

    /// The hit-test sink of the focused Doc tile (test seam; the handlers
    /// address a tile by id). An empty sink when the focus isn't a Doc.
    #[cfg(test)]
    pub(crate) fn focused_doc_line_layouts(&self) -> DocLineLayouts {
        match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(d))) => d.line_layouts.clone(),
            _ => DocLineLayouts::default(),
        }
    }

    /// [`Self::doc_pos_in`] against the focused Doc tile (test seam).
    #[cfg(test)]
    pub(crate) fn doc_pos_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<DocPos> {
        let id = self.workspace.focused_window_id()?;
        self.doc_pos_in(id, position)
    }

    /// Hit-test a window-space position against the per-line `TextLayout`s
    /// the Doc tile `id`'s body registered when it last painted (its own sink,
    /// `DocState::line_layouts` — a cache-hit frame replays that paint, so the
    /// geometry is still exact). Returns the doc position
    /// (block_idx, line_idx, char_offset) if the point falls on a tracked
    /// line, else `None`. For points off the right edge of a line, returns
    /// the line's end (caller may treat as past-the-end selection).
    pub(crate) fn doc_pos_in(
        &self,
        id: workspace::WindowId,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<DocPos> {
        let layouts = self.doc_state_ref(id)?.line_layouts.borrow();
        // Choose the line whose vertical band contains `position.y`. Ties are
        // broken by the smaller (block_idx, line_idx) — the map iteration
        // order doesn't matter because the bounds bands don't overlap.
        let mut hit: Option<(&(usize, usize), &TextLayout)> = None;
        for (key, layout) in layouts.iter() {
            let b = layout.bounds();
            if position.y >= b.top() && position.y <= b.bottom() {
                hit = Some((key, layout));
                break;
            }
        }
        let (key, layout) = hit?;
        // Map pixel position → byte index. `index_for_position` returns Ok
        // for in-line hits and Err for points past the right edge (it
        // still gives a valid index).
        let byte_idx = match layout.index_for_position(position) {
            Ok(i) => i,
            Err(i) => i,
        };
        let text = layout.text();
        let char_offset = text
            .char_indices()
            .position(|(b, _)| b >= byte_idx)
            .unwrap_or_else(|| text.chars().count());
        Some(DocPos {
            block_idx: key.0,
            line_idx: key.1,
            char_offset,
        })
    }

    /// Left MouseDown on the Doc tile `id`'s body: anchor a selection there
    /// (or clear one on a miss). `id` is the only thing the listener captured
    /// — everything else is resolved here, at event time (yux rule 4).
    pub(crate) fn doc_mouse_down(
        &mut self,
        id: workspace::WindowId,
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(pos) = self.doc_pos_in(id, ev.position) else {
            // Click on chrome / empty area: clear any existing selection.
            if self.doc_selection.is_some() {
                self.doc_selection = None;
                cx.notify();
            }
            return;
        };
        self.doc_selection = Some(DocSelection {
            anchor: pos,
            head: pos,
            dragging: true,
        });
        cx.notify();
    }

    pub(crate) fn doc_mouse_move(
        &mut self,
        id: workspace::WindowId,
        ev: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.doc_selection.map(|s| s.dragging).unwrap_or(false) {
            return;
        }
        let Some(pos) = self.doc_pos_in(id, ev.position) else {
            return;
        };
        if let Some(sel) = self.doc_selection.as_mut()
            && sel.head != pos
        {
            sel.head = pos;
            cx.notify();
        }
    }

    pub(crate) fn doc_mouse_up(
        &mut self,
        id: workspace::WindowId,
        _ev: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.doc_selection.as_mut() else {
            return;
        };
        sel.dragging = false;
        if sel.is_empty() {
            self.doc_selection = None;
        } else {
            // X11-style select-to-clipboard: finalizing a non-empty drag copies
            // the selection to the system clipboard automatically (no Cmd-C).
            let sel = *sel;
            if let Some(text) = self.doc_state_ref(id).and_then(|d| doc_selection_text(d, &sel))
                && !text.is_empty()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
        cx.notify();
    }

    /// Read the doc-view text covered by `doc_selection` and write it to
    /// the system clipboard. Walks blocks/lines in document order using
    /// the focused window's DocState as the source of truth for line text.
    pub(crate) fn copy_doc_selection(
        &mut self,
        _: &CopyDocSelection,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.doc_selection else {
            return;
        };
        let Some(text) = self.collect_doc_selection_text(&sel) else {
            return;
        };
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// The text `sel` covers in the FOCUSED Doc (`None` if the focus isn't one).
    pub(crate) fn collect_doc_selection_text(&self, sel: &DocSelection) -> Option<String> {
        match self.workspace.focused_content()? {
            App::Buffer(BufferApp::Viewing(d)) => doc_selection_text(d, sel),
            _ => None,
        }
    }

    // ---- Keys --------------------------------------------------------------

    /// `on_key_down` handler for the Doc view — intercepts bare `m`/`'` to
    /// start a mark chord.
    pub(crate) fn handle_doc_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Universal leaders: the doc view is never text entry, so `<space>`/`.`/
        // `?` always open the menus (with top priority).
        if self.leader_intercept(&press, cx) {
            return;
        }

        // Ctrl-S: save the backing buffer from doc view (same as edit view).
        if press.modifiers.contains(KMods::CONTROL)
            && matches!(press.key, Key::Char('s') | Key::Char('S'))
        {
            if let Some(d) = self.doc_mut() {
                if let Some(core) = d.source.as_ref().map(|s| s.core.clone()) {
                    // Same write path + conflict gate as the Edit view's Ctrl-S
                    // (UXI-Buffer-5/7).
                    let path = core.borrow().document().file_path.clone();
                    let msg: SharedString = if self.buffer_has_disk_conflict(&path) {
                        "not saved: changed on disk — space k keep mine / space R reload theirs"
                            .into()
                    } else {
                        match self.write_core_to_disk(&core) {
                            Ok(()) => "saved".into(),
                            Err(e) => format!("save failed: {}", e).into(),
                        }
                    };
                    self.transient_status = Some(msg);
                } else {
                    self.transient_status = Some("no file to save".into());
                }
            }
            cx.notify();
            return;
        }

        if self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
            cx.stop_propagation();
        }
    }
}

/// The text `sel` covers in `d`, walking blocks/lines in document order
/// (block-relative `(block, line, char)` positions, `\n` between lines and a
/// blank line between blocks). `None` if `sel` addresses a block `d` lacks.
fn doc_selection_text(d: &DocState, sel: &DocSelection) -> Option<String> {
    let (start, end) = sel.normalized();
    let blocks = &d.blocks;
    let mut out = String::new();
    for bi in start.block_idx..=end.block_idx {
        let block = blocks.get(bi)?;
        if d.is_block_hidden(bi) {
            continue; // folded away (UXI-Buffer-14): not painted, not copied
        }
        let lines = block_selectable_lines(block);
        if lines.is_empty() {
            continue;
        }
        let l_start = if bi == start.block_idx {
            start.line_idx
        } else {
            0
        };
        let l_end = if bi == end.block_idx {
            end.line_idx
        } else {
            lines.len().saturating_sub(1)
        };
        for li in l_start..=l_end {
            let Some(line) = lines.get(li) else { continue };
            let line_text: String = line
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("");
            let chars: Vec<char> = line_text.chars().collect();
            let s = if bi == start.block_idx && li == start.line_idx {
                start.char_offset.min(chars.len())
            } else {
                0
            };
            let e = if bi == end.block_idx && li == end.line_idx {
                end.char_offset.min(chars.len())
            } else {
                chars.len()
            };
            if s < e {
                out.extend(chars[s..e].iter());
            }
            if li < l_end {
                out.push('\n');
            }
        }
        if bi < end.block_idx {
            out.push_str("\n\n");
        }
    }
    Some(out)
}

// ---- Task checkboxes (UXI-Buffer-12) ---------------------------------------

impl YaldaGpuiView {
    /// `x` (the `ToggleTask` action) on a Doc: toggle a task item of the
    /// focused block — the first open one, or the last when all are done
    /// (`task_list::key_toggle_target`).
    pub(crate) fn doc_toggle_task(&mut self, _: &ToggleTask, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.doc_mut().map(|d| d.cursor_block) else {
            return;
        };
        self.doc_toggle_task_in(block, None, cx);
    }

    /// A click on the painted checkbox `md-task-<path>`: toggle exactly that
    /// item. `path` = `[top block, item path…]`, the only thing the listener
    /// captured; the Doc, its source and the marker are resolved here. The
    /// click reaches this only in the FOCUSED tile (UXI-Workspace-9 consumes a
    /// click on an unfocused one), so the focused Doc is the clicked one.
    pub(crate) fn doc_toggle_task_click(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some((&block, item)) = path.split_first() else {
            return;
        };
        self.doc_toggle_task_in(block, Some(item), cx);
    }

    /// The one body behind the key and the click: flip the marker through the
    /// shared buffer (one undo step, marks it dirty → autosave), re-derive the
    /// blocks, and repaint. Failures surface as a transient status.
    fn doc_toggle_task_in(&mut self, block: usize, item: Option<&[usize]>, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let Some(d) = self.doc_mut() else {
            return;
        };
        if let Err(msg) = toggle_task(d, block, item, &theme) {
            self.transient_status = Some(msg.into());
        }
        cx.notify();
    }
}

/// Toggle one task marker of top-level `block` in `d`'s shared source: the
/// item at `item` (a structural path, from a click) or, for the key, the
/// [`yalda::task_list::key_toggle_target`]. Moves the block cursor onto
/// `block`. Returns the item's new checked state.
fn toggle_task(
    d: &mut DocState,
    block: usize,
    item: Option<&[usize]>,
    theme: &Theme,
) -> Result<bool, &'static str> {
    use yalda::task_list::{key_toggle_target, task_item_paths, task_markers_in};
    if d.source.is_none() {
        return Err("read-only document: no source to toggle");
    }
    // Spans and blocks must describe the CURRENT text (a sibling Edit may have
    // typed since the last paint-side re-derive).
    d.refresh_blocks(theme);
    let span = d.spans.get(block).ok_or("no source for this block")?.bytes.clone();
    let rendered = d.blocks.get(block).ok_or("no such block")?;
    let core = d.source.as_ref().ok_or("read-only document")?.core.clone();
    let text = core.borrow().document().full_text();
    let markers = task_markers_in(&text, &span);
    let idx = match item {
        None => key_toggle_target(&markers).ok_or("no task in this block")?,
        Some(item) => {
            let paths = task_item_paths(rendered);
            if paths.len() != markers.len() {
                return Err("task list out of sync with its source");
            }
            paths.iter().position(|p| p == item).ok_or("no such task")?
        }
    };
    let m = &markers[idx];
    let replaced = {
        let mut c = core.borrow_mut();
        let char_idx = c.document().rope().byte_to_char(m.state_byte());
        c.replace_char_undoable(char_idx, m.toggled_char())
    };
    if !replaced {
        return Err("task is read-only here");
    }
    d.refresh_blocks(theme);
    d.cursor_block = block.min(d.blocks.len().saturating_sub(1));
    Ok(!m.checked)
}
