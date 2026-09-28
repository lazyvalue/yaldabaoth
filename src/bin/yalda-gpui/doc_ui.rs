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

    /// j / ↓ — next block (stops at the last).
    fn doc_next_block(d: &DocState) -> Option<usize> {
        (d.cursor_block + 1 < d.blocks.len()).then_some(d.cursor_block + 1)
    }

    /// k / ↑ — previous block (stops at the first).
    fn doc_prev_block(d: &DocState) -> Option<usize> {
        d.cursor_block.checked_sub(1)
    }

    pub(crate) fn scroll_down(&mut self, _: &ScrollDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_next_block);
    }
    pub(crate) fn scroll_up(&mut self, _: &ScrollUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, Self::doc_prev_block);
    }
    pub(crate) fn page_down(&mut self, _: &ScrollPageDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, |d| Some(d.cursor_block + 8));
    }
    pub(crate) fn page_up(&mut self, _: &ScrollPageUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_nav(cx, |d| Some(d.cursor_block.saturating_sub(8)));
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
        // Clamped to the last block by `move_cursor_to`.
        self.doc_nav(cx, |_| Some(usize::MAX));
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
