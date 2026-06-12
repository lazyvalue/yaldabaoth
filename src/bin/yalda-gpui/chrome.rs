//! Window chrome on YaldaGpuiView: focused-window/layout render, tab
//! strip, tag bar, rails (render + outline derivation). Extracted
//! verbatim from main.rs (split-gpui-main, stage 2).

use super::*;

/// Desktop-mode chrome constants (spec-desktop-mode.md Behavior 3/4).
const DESKTOP_GUTTER: f32 = 12.0;
const DESKTOP_TITLE_H: f32 = 20.0;
const DESKTOP_DRAG_THRESHOLD: f32 = 4.0;
const DESKTOP_EDGE_PAN_BAND: f32 = 30.0;
const DESKTOP_EDGE_PAN_STEP: f32 = 12.0;
/// Width of the east/south edge bands that arm a tile resize (spec 4b).
const DESKTOP_RESIZE_BAND: f32 = 6.0;

impl YaldaGpuiView {
    /// Build the menu popup as an absolutely-positioned overlay anchored
    /// to the top of the window. Renders header (breadcrumb), entry list,
    /// and a footer hint. Has *no* key handlers — the wrapper in
    /// `Render::render` handles input via `capture_key_down` so the
    /// underlying screen never sees keystrokes while the menu is open.
    /// Render the active tab's layout tree. Leaves dispatch to per-kind
    /// render methods; splits become flex containers (row for V splits,
    /// col for H splits) with weighted children.
    pub(crate) fn render_focused_window(
        &mut self,
        root: gpui::Div,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_idx = self.workspace.active_tab;
        let focused_id = self.workspace.tabs[tab_idx].focused;
        // Re-derive the outline rail (if any) once before rendering the tree,
        // so the focused leaf can render it inline without a second pass.
        self.refresh_outline_rail();
        let layout_ptr: *mut workspace::Layout<App> =
            &mut self.workspace.tabs[tab_idx].layout as *mut _;
        // SAFETY: `layout_ptr` is valid for as long as the active tab's
        // `layout` field isn't structurally mutated (no splits/closes/etc.).
        // The render pipeline only reads self's other fields (theme/fonts)
        // and the layout subtree via this pointer; structural mutations
        // happen in action handlers, never inside render. This sidesteps a
        // Rust borrowck limitation where the compiler can't prove that
        // &mut Layout<App> (a field inside self.workspace.tabs)
        // is disjoint from &self.render_X's other field accesses.
        let layout = unsafe { &mut *layout_ptr };
        if self.workspace.tabs[tab_idx].layout_mode == workspace::LayoutMode::Desktop {
            return self.render_desktop(root, layout, focused_id, attach_focus, rail_focusable, cx);
        }
        self.render_layout(root, layout, focused_id, attach_focus, rail_focusable, cx)
    }

    /// Desktop mode (spec-desktop-mode.md): fixed-size tiles at slot
    /// positions on a pannable canvas. The layout tree is the CONTENT owner
    /// (leaves render exactly as in tiling); geometry comes from the tab's
    /// `DesktopState`. Only viewport-intersecting tiles render, except the
    /// focused tile — it carries the focus handle and the per-screen action
    /// wiring, and culling it would strand the keyboard (spec Behavior 3).
    fn render_desktop(
        &mut self,
        root: gpui::Div,
        layout: &mut workspace::Layout<App>,
        focused_id: workspace::WindowId,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_idx = self.workspace.active_tab;
        let tile = self.desktop_tile_px();
        let g = DESKTOP_GUTTER;
        let (_, _, mut canvas_w, mut canvas_h) = self.desktop_canvas_bounds.get();
        // First frame: bounds not captured yet — approximate with the window
        // viewport; the next frame self-corrects.
        if canvas_w <= 0.0 {
            canvas_w = self.viewport_width_px.max(1.0);
        }
        if canvas_h <= 0.0 {
            canvas_h = self.viewport_height_px.max(1.0);
        }
        // Grid semantics: the wrap width IS the configured column count —
        // no longer derived from the viewport (the viewport now derives the
        // tile SIZE instead).
        let eff_w = self.desktop_grid_cols.max(1);

        // ── Slot upkeep: seed on first entry, reconcile every frame (cheap,
        // O(n), no-op when the Behavior-2 invariant already holds), reveal
        // the focused tile when focus changed, clamp pan to the occupied
        // bounding box + one slot of margin. ──
        {
            let tab = &mut self.workspace.tabs[tab_idx];
            let leaves = tab.layout.leaf_ids();
            if tab.desktop.slots.is_empty() && !leaves.is_empty() {
                tab.desktop.seed(&leaves, eff_w);
            } else {
                tab.desktop.reconcile(&leaves, focused_id, eff_w);
            }
            if tab.desktop.last_reveal != Some(focused_id) {
                if let Some(slot) = tab.desktop.slot_of(focused_id) {
                    let (x, y) = workspace::slot_origin(slot, tile, g);
                    let pan = &mut tab.desktop.pan;
                    if x - g < pan.0 {
                        pan.0 = (x - g).max(0.0);
                    } else if x + tile.0 + g > pan.0 + canvas_w {
                        pan.0 = x + tile.0 + g - canvas_w;
                    }
                    if y - g < pan.1 {
                        pan.1 = (y - g).max(0.0);
                    } else if y + tile.1 + g > pan.1 + canvas_h {
                        pan.1 = y + tile.1 + g - canvas_h;
                    }
                }
                tab.desktop.last_reveal = Some(focused_id);
            }
            let (max_r, max_c) = tab.desktop.occupied_extent().unwrap_or((0, 0));
            // Pannable extent: through one margin slot beyond occupied.
            let extent =
                workspace::slot_origin(workspace::Slot::new(max_r + 2, max_c + 2), tile, g);
            let pan = &mut tab.desktop.pan;
            pan.0 = pan.0.clamp(0.0, (extent.0 - canvas_w).max(0.0));
            pan.1 = pan.1.clamp(0.0, (extent.1 - canvas_h).max(0.0));
        }

        let tab = &self.workspace.tabs[tab_idx];
        let pan = tab.desktop.pan;
        let drag = tab.desktop.drag;
        let slot_list: Vec<(workspace::WindowId, workspace::Slot, workspace::Span)> = tab
            .desktop
            .slots
            .iter()
            .map(|&(id, s)| (id, s, tab.desktop.span_of(id)))
            .collect();
        // Live edge-resize preview (spec Behavior 4b): the clamped span the
        // resized tile renders at this frame, which is also what commits.
        let resize_preview: Option<(workspace::WindowId, workspace::Span)> = tab
            .desktop
            .resize
            .map(|r| (r.id, self.desktop_resize_target_span(r)));
        let base_bg = self.editor_bg();
        let content_fg = self.editor_fg();
        let dim: Hsla = nc(self.theme.agent.dim);
        let accent: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let tile_bg = tint_bg(base_bg, 0.5, 0.06, 0.02);
        let title_bg = tint_bg(base_bg, 0.5, 0.12, 0.05);

        let mut canvas = root
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(base_bg)
            // Swallow scroll-wheel on the desktop canvas so tile content
            // scrolls stay contained and the desktop itself never pans from
            // the mousewheel. Pan is available via drag or keyboard.
            .on_scroll_wheel(cx.listener(|_this, _ev: &gpui::ScrollWheelEvent, _w, _cx| {}))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                this.desktop_pointer_move((f32::from(ev.position.x), f32::from(ev.position.y)), cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _w, cx| {
                    this.desktop_drop(cx);
                }),
            )
            // Right-click cancels an in-flight drag (Esc-at-canvas-root is a
            // follow-up — a global escape binding would shadow the
            // per-screen escape semantics; see backlog).
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.desktop_cancel_drag(cx);
                }),
            );

        // ── Dot grid over the visible area (slot-pitch corners). ──
        {
            let pitch = (tile.0 + g, tile.1 + g);
            let first_col = (pan.0 / pitch.0).floor().max(0.0) as u32;
            let first_row = (pan.1 / pitch.1).floor().max(0.0) as u32;
            let ncols = (canvas_w / pitch.0).ceil() as u32 + 1;
            let nrows = (canvas_h / pitch.1).ceil() as u32 + 1;
            let dot = dim.opacity(0.35);
            for r in first_row..first_row + nrows {
                for c in first_col..first_col + ncols {
                    let (x, y) = workspace::slot_origin(workspace::Slot::new(r, c), tile, g);
                    canvas = canvas.child(
                        div()
                            .absolute()
                            .left(px(x - pan.0 - 1.0))
                            .top(px(y - pan.1 - 1.0))
                            .w(px(3.0))
                            .h(px(3.0))
                            .rounded_full()
                            .bg(dot),
                    );
                }
            }
        }

        // ── Drag affordances: home-slot outline + drop-target highlight. ──
        if let Some(d) = drag.filter(|d| d.active) {
            let dspan = slot_list
                .iter()
                .find(|&&(id, _, _)| id == d.id)
                .map(|&(_, _, sp)| sp)
                .unwrap_or(workspace::Span::ONE);
            if let Some(home) = slot_list
                .iter()
                .find(|&&(id, _, _)| id == d.id)
                .map(|&(_, s, _)| s)
            {
                let (x, y, w, h) = workspace::tile_rect(home, dspan, tile, g);
                canvas = canvas.child(
                    div()
                        .absolute()
                        .left(px(x - pan.0))
                        .top(px(y - pan.1))
                        .w(px(w))
                        .h(px(h))
                        .border_1()
                        .border_color(dim.opacity(0.6))
                        .rounded_md(),
                );
            }
            if let Some(t) = d.target {
                let (x, y, w, h) = workspace::tile_rect(t, dspan, tile, g);
                canvas = canvas.child(
                    div()
                        .absolute()
                        .left(px(x - pan.0))
                        .top(px(y - pan.1))
                        .w(px(w))
                        .h(px(h))
                        .border_2()
                        .border_color(accent.opacity(0.8))
                        .rounded_md(),
                );
            }
        }

        // ── Tiles. ──
        for (id, slot, mut span) in slot_list {
            // The tile being resized renders at its live clamped span so the
            // grow/shrink is visible under the cursor (spec Behavior 4b).
            if let Some((rid, rspan)) = resize_preview {
                if rid == id {
                    span = rspan;
                }
            }
            let (_, _, tw, th) = workspace::tile_rect(slot, span, tile, g);
            let dragging = drag.filter(|d| d.active && d.id == id);
            let (sx, sy) = workspace::slot_origin(slot, tile, g);
            let (x, y) = match dragging {
                // The dragged tile itself follows the pointer — the real
                // content rides along semi-transparent (no separate ghost).
                Some(d) => (
                    d.pointer.0 - d.grab.0 - pan.0,
                    d.pointer.1 - d.grab.1 - pan.1,
                ),
                None => (sx - pan.0, sy - pan.1),
            };
            let visible = x + tw > 0.0 && x < canvas_w && y + th > 0.0 && y < canvas_h;
            let is_focused = id == focused_id;
            // Focused tile is exempt from culling — its element tree holds
            // the focus handle + per-screen action wiring (spec Behavior 3).
            if !visible && !is_focused {
                continue;
            }

            let Some(window) = layout.find_leaf_mut(id) else {
                continue; // stale entry; reconcile drops it next frame
            };
            let content_ptr: *mut App = &mut window.content as *mut _;
            // SAFETY: same argument as `render_focused_window` — no
            // structural tree mutation happens during this render pass.
            let content = unsafe { &mut *content_ptr };

            let title = Self::desktop_tile_title(&self.sessions, content, cx);
            let mark = self.workspace.marks.mark_for_window(id);

            // The leaf-root CONTRACT (see `screen_root` in render() and the
            // split-child roots in render_layout): the per-kind renderers
            // never set their own root layout — they expect a div that is
            // already `size_full + flex + flex_col` with the editor colors.
            // Missing any piece breaks them: without flex_col the header/
            // body/footer stack in block layout and the flex_1 virtualized
            // body collapses to its minimum height (content huddled at the
            // top of the tile, dead space below).
            let base = div()
                .size_full()
                .flex()
                .flex_col()
                .bg(base_bg)
                .text_color(content_fg);
            let leaf_root = if is_focused && attach_focus {
                base.track_focus(&self.focus_handle)
            } else {
                base
            };
            let inner: AnyElement = match content {
                App::Buffer(BufferApp::Viewing(d)) => {
                    self.render_doc(leaf_root, d, cx).into_any_element()
                }
                App::Buffer(BufferApp::Editing(e)) => {
                    self.render_edit(leaf_root, e, cx).into_any_element()
                }
                App::Buffer(BufferApp::Picking(b)) => {
                    self.render_browser(leaf_root, b, cx).into_any_element()
                }
                App::Agent(tile) => self.render_agent(leaf_root, tile, cx).into_any_element(),
            };

            let mut title_bar = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(DESKTOP_TITLE_H))
                .px_2()
                .flex_none()
                .bg(title_bg)
                .text_size(px(11.0))
                .text_color(if is_focused { accent } else { dim })
                .cursor_grab()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_grab(
                            id,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                )
                .child(title);
            if let Some(m) = mark {
                title_bar =
                    title_bar.child(div().px_1().text_color(accent).child(format!("[{m}]")));
            }

            // East / south resize bands (spec Behavior 4b): thin overlays at
            // the grow edges; the title bar (move) and content keep theirs.
            let east_band = div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(DESKTOP_RESIZE_BAND))
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_resize_grab(
                            id,
                            workspace::ResizeEdge::East,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                );
            let south_band = div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(DESKTOP_RESIZE_BAND))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_resize_grab(
                            id,
                            workspace::ResizeEdge::South,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                );

            let mut frame = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(tw))
                .h(px(th))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded_md()
                .bg(tile_bg)
                .border_1()
                .border_color(if is_focused { accent } else { dim.opacity(0.4) })
                .child(title_bar)
                .child(div().flex_1().min_h_0().overflow_hidden().child(inner))
                .child(east_band)
                .child(south_band);
            if dragging.is_some() {
                frame = frame.opacity(0.85);
            }
            canvas = canvas.child(frame);
        }

        let canvas_el = CaptureBounds {
            inner: canvas.into_any_element(),
            sink: self.desktop_canvas_bounds.clone(),
        }
        .into_any_element();
        // Rail coexistence (spec Builds On / spec-rail.md): in desktop mode
        // the rail wraps the whole CANVAS (there is no single focused-leaf
        // wrapper to ride). This is also load-bearing for the keyboard: when
        // the rail holds focus, `leaf_attach_focus` is false and the RAIL
        // element must exist to carry the focus handle — before this wrap,
        // opening the outline rail in desktop mode rendered no rail at all
        // and attached focus to NOTHING, killing every key binding (menus
        // included) until restart. CaptureBounds sits INSIDE the wrap so
        // mouse math tracks the rail-shifted canvas origin.
        self.wrap_leaf_with_rail(canvas_el, rail_focusable, cx)
    }

    /// Tile pixel size derived from the desktop GRID config (spec
    /// Behavior 6, grid revision): the viewport is divided into
    /// `desktop_grid_cols × desktop_grid_rows` tiles (gutters between and
    /// around), so changing the grid — or the window — resizes tiles while
    /// slots stay untouched (slots, not pixels, remain the stored unit).
    /// Floors keep tiles usable when the window gets tiny.
    fn desktop_tile_px(&self) -> (f32, f32) {
        let (_, _, mut w, mut h) = self.desktop_canvas_bounds.get();
        if w <= 0.0 {
            w = self.viewport_width_px.max(1.0);
        }
        if h <= 0.0 {
            h = self.viewport_height_px.max(1.0);
        }
        let cols = self.desktop_grid_cols.max(1) as f32;
        let rows = self.desktop_grid_rows.max(1) as f32;
        (
            ((w - (cols + 1.0) * DESKTOP_GUTTER) / cols).max(160.0),
            ((h - (rows + 1.0) * DESKTOP_GUTTER) / rows).max(120.0),
        )
    }

    /// Title-bar label for a tile. Agent labels live in the session store, so
    /// this takes the `sessions` field DIRECTLY (not `&self`) — the caller
    /// holds a live `&mut App` (`content_ptr`) into the layout tree, and
    /// reborrowing all of `&self` here would alias it (UB under Stacked
    /// Borrows). `sessions` is field-disjoint from the layout tree.
    fn desktop_tile_title(sessions: &AgentSessions, content: &App, cx: &GpuiApp) -> String {
        match content {
            App::Buffer(BufferApp::Viewing(d)) => d.file_label.to_string(),
            App::Buffer(BufferApp::Editing(e)) => e.file_label.to_string(),
            App::Buffer(BufferApp::Picking(_)) => "files".to_string(),
            App::Agent(tile) => tile
                .bound
                .and_then(|id| sessions.get(id))
                .map(|s| s.read(cx).label.clone())
                .unwrap_or_else(|| "claude".to_string()),
        }
    }

    /// Mouse-down on a tile title bar: focus the tile (spec Behavior 4 —
    /// arming a drag also focuses) and arm a drag. The drag activates only
    /// once the pointer crosses the click threshold in
    /// [`desktop_pointer_move`](Self::desktop_pointer_move).
    pub(crate) fn desktop_grab(
        &mut self,
        id: workspace::WindowId,
        window_pos: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        let (cx0, cy0, _, _) = self.desktop_canvas_bounds.get();
        let tile = self.desktop_tile_px();
        let tab_idx = self.workspace.active_tab;
        let tab = &mut self.workspace.tabs[tab_idx];
        tab.focused = id;
        let Some(slot) = tab.desktop.slot_of(id) else {
            cx.notify();
            return;
        };
        let pan = tab.desktop.pan;
        let desktop_pos = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
        let (ox, oy) = workspace::slot_origin(slot, tile, DESKTOP_GUTTER);
        tab.desktop.drag = Some(workspace::DesktopDrag {
            id,
            grab: (desktop_pos.0 - ox, desktop_pos.1 - oy),
            pointer: desktop_pos,
            target: None,
            active: false,
        });
        self.save_workspace_state();
        cx.notify();
    }

    /// Mouse-down on a tile's east/south resize band (spec Behavior 4b):
    /// focus the tile and arm an edge resize. Live span is previewed in the
    /// render pass; the clamped span commits on mouse-up.
    pub(crate) fn desktop_resize_grab(
        &mut self,
        id: workspace::WindowId,
        edge: workspace::ResizeEdge,
        window_pos: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        let (cx0, cy0, _, _) = self.desktop_canvas_bounds.get();
        let tab_idx = self.workspace.active_tab;
        let tab = &mut self.workspace.tabs[tab_idx];
        tab.focused = id;
        let pan = tab.desktop.pan;
        let desktop_pos = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
        tab.desktop.resize = Some(workspace::DesktopResize {
            id,
            edge,
            pointer: desktop_pos,
        });
        cx.notify();
    }

    /// The Block-clamped span a live resize would commit, given its pointer
    /// (spec Behavior 4b). Used both for the render preview and the commit, so
    /// what you see is exactly what lands.
    fn desktop_resize_target_span(&self, r: workspace::DesktopResize) -> workspace::Span {
        let tile = self.desktop_tile_px();
        let g = DESKTOP_GUTTER;
        let tab = &self.workspace.tabs[self.workspace.active_tab];
        let Some(anchor) = tab.desktop.slot_of(r.id) else {
            return tab.desktop.span_of(r.id);
        };
        let (ox, oy) = workspace::slot_origin(anchor, tile, g);
        // Invert tile_rect: the far edge of `n` cells sits at
        // origin + n*(tile+g) - g, so n = (edge_pos - origin + g) / (tile+g).
        let desired = match r.edge {
            workspace::ResizeEdge::East => {
                ((r.pointer.0 - ox + g) / (tile.0 + g)).round().max(1.0) as u32
            }
            workspace::ResizeEdge::South => {
                ((r.pointer.1 - oy + g) / (tile.1 + g)).round().max(1.0) as u32
            }
        };
        tab.desktop.clamp_span(r.id, r.edge, desired)
    }

    /// Canvas mouse-move: advance the drag (threshold, pointer, drop target,
    /// edge auto-pan), or a live resize. No-op when neither is armed.
    pub(crate) fn desktop_pointer_move(&mut self, window_pos: (f32, f32), cx: &mut Context<Self>) {
        let (cx0, cy0, cw, ch) = self.desktop_canvas_bounds.get();
        let tile = self.desktop_tile_px();
        let tab_idx = self.workspace.active_tab;

        // A live resize takes precedence over (and is mutually exclusive with)
        // a drag: just track the pointer; the render pass clamps the span.
        {
            let tab = &mut self.workspace.tabs[tab_idx];
            if let Some(mut r) = tab.desktop.resize {
                let pan = tab.desktop.pan;
                r.pointer = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
                tab.desktop.resize = Some(r);
                cx.notify();
                return;
            }
        }

        // Edge auto-pan first (uses window-relative position within canvas).
        let mut pan_delta = (0.0f32, 0.0f32);
        let rel = (window_pos.0 - cx0, window_pos.1 - cy0);
        let tab = &mut self.workspace.tabs[tab_idx];
        let Some(mut d) = tab.desktop.drag else {
            return;
        };
        if d.active {
            if rel.0 < DESKTOP_EDGE_PAN_BAND {
                pan_delta.0 = -DESKTOP_EDGE_PAN_STEP;
            } else if rel.0 > cw - DESKTOP_EDGE_PAN_BAND {
                pan_delta.0 = DESKTOP_EDGE_PAN_STEP;
            }
            if rel.1 < DESKTOP_EDGE_PAN_BAND {
                pan_delta.1 = -DESKTOP_EDGE_PAN_STEP;
            } else if rel.1 > ch - DESKTOP_EDGE_PAN_BAND {
                pan_delta.1 = DESKTOP_EDGE_PAN_STEP;
            }
            tab.desktop.pan.0 = (tab.desktop.pan.0 + pan_delta.0).max(0.0);
            tab.desktop.pan.1 = (tab.desktop.pan.1 + pan_delta.1).max(0.0);
        }
        let pan = tab.desktop.pan;
        let desktop_pos = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);

        if !d.active {
            let dx = desktop_pos.0 - d.pointer.0;
            let dy = desktop_pos.1 - d.pointer.1;
            if (dx * dx + dy * dy).sqrt() < DESKTOP_DRAG_THRESHOLD {
                return; // still a click, not a drag
            }
            d.active = true;
        }
        d.pointer = desktop_pos;
        // Target from the ghost's CENTER, not the raw pointer — dragging by
        // the title bar biases the pointer to the top edge otherwise.
        let center = (
            d.pointer.0 - d.grab.0 + tile.0 / 2.0,
            d.pointer.1 - d.grab.1 + tile.1 / 2.0,
        );
        d.target = Some(workspace::slot_at(center, tile, DESKTOP_GUTTER));
        tab.desktop.drag = Some(d);
        cx.notify();
    }

    /// Canvas mouse-up: commit the drop (insert-and-shift) or treat as a
    /// click when the threshold was never crossed.
    pub(crate) fn desktop_drop(&mut self, cx: &mut Context<Self>) {
        let eff_w = self.desktop_grid_cols.max(1);
        let tab_idx = self.workspace.active_tab;

        // Commit a live edge resize (spec Behavior 4b) — the clamped span the
        // preview showed becomes the stored span.
        if let Some(r) = self.workspace.tabs[tab_idx].desktop.resize.take() {
            let span = self.desktop_resize_target_span(r);
            self.workspace.tabs[tab_idx].desktop.set_span(r.id, span);
            self.save_workspace_state();
            cx.notify();
            return;
        }

        let tab = &mut self.workspace.tabs[tab_idx];
        let Some(d) = tab.desktop.drag.take() else {
            return;
        };
        if d.active
            && let Some(target) = d.target
            && tab.desktop.slot_of(d.id) != Some(target)
        {
            tab.desktop.insert_shift(d.id, target, eff_w);
            self.save_workspace_state();
        }
        cx.notify();
    }

    /// Cancel an in-flight drag or resize (right-click; Esc is a follow-up).
    pub(crate) fn desktop_cancel_drag(&mut self, cx: &mut Context<Self>) {
        let tab_idx = self.workspace.active_tab;
        let d = &mut self.workspace.tabs[tab_idx].desktop;
        if d.drag.take().is_some() || d.resize.take().is_some() {
            cx.notify();
        }
    }

    /// Recursively render a `Layout<App>`. The `root` div is used
    /// only for the leaf case (so leaves can attach focus + key bindings);
    /// split branches build their own container.
    ///
    /// `attach_focus` is true when no overlay is open — in that case the
    /// focused leaf attaches `track_focus(&self.focus_handle)` so the focus
    /// handle sits inside that leaf's key context. When an overlay is open,
    /// focus belongs on the overlay wrapper and no leaf attaches it.
    pub(crate) fn render_layout(
        &mut self,
        root: gpui::Div,
        layout: &mut workspace::Layout<App>,
        focused_id: workspace::WindowId,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            workspace::Layout::Empty => div().size_full().into_any_element(),
            workspace::Layout::Leaf(window) => {
                let is_focused = window.id == focused_id;
                let content_ptr: *mut App = &mut window.content as *mut _;
                // SAFETY: same as in render_focused_window — the leaf's
                // content sits inside a layout tree we won't structurally
                // mutate during this render call.
                let content = unsafe { &mut *content_ptr };
                let leaf_root = if is_focused && attach_focus {
                    root.track_focus(&self.focus_handle)
                } else {
                    root
                };
                let painted: AnyElement = match content {
                    App::Buffer(BufferApp::Viewing(d)) => {
                        self.render_doc(leaf_root, d, cx).into_any_element()
                    }
                    App::Buffer(BufferApp::Editing(e)) => {
                        self.render_edit(leaf_root, e, cx).into_any_element()
                    }
                    App::Buffer(BufferApp::Picking(b)) => {
                        self.render_browser(leaf_root, b, cx).into_any_element()
                    }
                    App::Agent(tile) => self.render_agent(leaf_root, tile, cx).into_any_element(),
                };
                // Pin the rail to the leaf it was opened from, not whichever
                // leaf currently has focus. Falls back to the focused leaf
                // when no pinned_to is set (single-tile case).
                let is_rail_pinned = self
                    .workspace
                    .active_tab()
                    .and_then(|t| t.rail.as_ref())
                    .map(|r| r.pinned_to == window.id)
                    .unwrap_or(false);
                let with_rail = if is_rail_pinned {
                    self.wrap_leaf_with_rail(painted, rail_focusable, cx)
                } else {
                    painted
                };
                // Focus indicator: thick border around the whole tile+rail
                // group when there's more than one leaf, plus a small "focused"
                // tag in the upper-right corner.
                let multi_leaf = self.active_tab_leaf_count() > 1;
                let mark_ch = self.workspace.marks.mark_for_window(window.id);
                if (is_focused && multi_leaf) || mark_ch.is_some() {
                    let accent: Hsla = rgb(STATUS_FG).into();
                    let mut wrapper = div().size_full().relative();
                    if is_focused && multi_leaf {
                        wrapper = wrapper.border_2().border_color(accent);
                    }
                    wrapper = wrapper.child(with_rail);
                    if is_focused && multi_leaf {
                        let tag = div()
                            .absolute()
                            .top_1()
                            .right_1()
                            .px_1p5()
                            .py_0p5()
                            .bg(accent)
                            .text_color(rgb(BG))
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .rounded_sm()
                            .child("focused");
                        wrapper = wrapper.child(tag);
                    }
                    // Mark badge: small orange label in top-left corner
                    if let Some(ch) = mark_ch {
                        let mark_badge = div()
                            .absolute()
                            .top_1()
                            .left_1()
                            .px_1p5()
                            .py_0p5()
                            .bg(gpui::hsla(0.08, 0.9, 0.55, 1.0))
                            .text_color(gpui::hsla(0.0, 0.0, 0.0, 1.0))
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .rounded_sm()
                            .child(SharedString::from(format!("[{ch}]")));
                        wrapper = wrapper.child(mark_badge);
                    }
                    wrapper.into_any_element()
                } else {
                    with_rail
                }
            }
            workspace::Layout::Split { dir, children } => {
                // Monocle mode: render only the child subtree containing
                // the focused leaf, giving it the full area.
                let is_monocle = self
                    .workspace
                    .active_tab()
                    .map(|t| t.layout_mode == workspace::LayoutMode::Monocle)
                    .unwrap_or(false);
                if is_monocle {
                    // Find the child subtree containing the focused leaf.
                    let focused_idx = children
                        .iter()
                        .position(|(_, child)| child.contains_leaf(focused_id))
                        .unwrap_or(0);
                    let (_, child) = &mut children[focused_idx];
                    let child_root = div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(self.editor_bg())
                        .text_color(self.editor_fg());
                    let child_el = self.render_layout(
                        child_root,
                        child,
                        focused_id,
                        attach_focus,
                        rail_focusable,
                        cx,
                    );
                    return root.child(child_el).into_any_element();
                }

                // The `root` div carries `track_focus(&self.focus_handle)`
                // when no overlay is open, so we must include it in the
                // tree. Without it the focus handle isn't attached to any
                // rendered element and global key bindings (e.g. Space →
                // OpenMenu) have nowhere to dispatch. Wrap the split's
                // flex container inside `root` rather than discarding it.
                // Tag view filtering: when active, check which children
                // have visible leaves and skip the rest.
                let tag_view = self
                    .workspace
                    .active_tab()
                    .map(|t| &t.tag_view)
                    .cloned()
                    .unwrap_or_default();
                let has_tag_filter = !tag_view.is_empty();
                let visible_mask: Vec<bool> = if has_tag_filter {
                    children
                        .iter()
                        .map(|(_, child)| {
                            Self::subtree_has_visible_leaf(
                                child,
                                &tag_view,
                                &self.workspace.file_buffers,
                            )
                        })
                        .collect()
                } else {
                    vec![true; children.len()]
                };
                // Calculate total visible weight for redistribution.
                let total_visible_weight: f32 = children
                    .iter()
                    .zip(visible_mask.iter())
                    .filter(|&(_, vis)| *vis)
                    .map(|((w, _), _)| *w)
                    .sum();

                let mut container = div().size_full().flex().min_w_0().min_h_0();
                container = match dir {
                    workspace::SplitDir::V => container.flex_row(),
                    workspace::SplitDir::H => container.flex_col(),
                };
                let editor_bg = self.editor_bg();
                let editor_fg = self.editor_fg();
                for (i, (weight, child)) in children.iter_mut().enumerate() {
                    if !visible_mask[i] {
                        continue;
                    }
                    let w = if has_tag_filter && total_visible_weight > 0.0 {
                        *weight / total_visible_weight
                    } else {
                        *weight
                    };
                    let child_root = div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(editor_bg)
                        .text_color(editor_fg);
                    let child_el = self.render_layout(
                        child_root,
                        child,
                        focused_id,
                        attach_focus,
                        rail_focusable,
                        cx,
                    );
                    let mut slot = div().min_w_0().min_h_0().overflow_hidden();
                    {
                        let style = slot.style();
                        style.flex_grow = Some(w);
                        style.flex_shrink = Some(1.0);
                        style.flex_basis = Some(gpui::relative(0.0).into());
                    }
                    slot = slot.child(child_el);
                    container = container.child(slot);
                }
                root.child(container).into_any_element()
            }
        }
    }

    /// How many leaves does the active tab's layout contain?
    pub(crate) fn active_tab_leaf_count(&self) -> usize {
        self.workspace
            .active_tab()
            .map(|t| t.layout.leaf_count())
            .unwrap_or(0)
    }

    /// If the workspace has more than one tab, stack a thin horizontal tab
    /// strip above the screen view. Single-tab workspaces render the screen
    /// alone (no strip).
    pub(crate) fn wrap_with_tab_strip(
        &self,
        screen_view: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.workspace.tabs.len() <= 1 {
            return screen_view;
        }

        let active_idx = self.workspace.active_tab;
        // Pull chrome colors from the active theme so the strip matches the
        // light/dark palette. Active tab inverts to editor_bg (the doc body
        // colour) so the focused tab visually connects to the work area.
        let top_bar = self.theme.top_bar;
        let active_fg: Hsla = fg_or(top_bar, STATUS_FG);
        let inactive_fg: Hsla = rgb(0x6272a4).into();
        let strip_bg: Hsla = bg_or(top_bar, STATUS_BG);
        let active_bg: Hsla = self.editor_bg();

        // Vertical sidebar on the left, fixed-width. Flex default for
        // align-items is stretch, which is what we want — entries fill the
        // strip width and truncate via overflow_hidden.
        let mut strip = div()
            .flex()
            .flex_col()
            .px_1()
            .py_2()
            .w(px(160.0))
            .min_w(px(160.0))
            .bg(strip_bg)
            .text_size(px(12.0))
            .font_family(self.body_font.clone())
            .gap_1();

        for (i, tab) in self.workspace.tabs.iter().enumerate() {
            let label = tab_strip_label(tab);
            let is_active = i == active_idx;
            let fg = if is_active { active_fg } else { inactive_fg };
            let bg = if is_active { active_bg } else { strip_bg };

            let entry = div()
                .id(("tab-strip-entry", i))
                .w_full()
                .px_2()
                .py_1()
                .rounded(px(3.0))
                .bg(bg)
                .text_color(fg)
                .overflow_hidden()
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, ev: &MouseDownEvent, _w, cx| {
                        // Double-click on a tab entry opens the rename
                        // overlay for that tab. Single-click just
                        // switches to it.
                        if ev.click_count >= 2 {
                            view.workspace.active_tab = i;
                            view.open_rename_active_tab_overlay(cx);
                        } else {
                            view.select_tab(i, cx);
                        }
                    }),
                );
            strip = strip.child(entry);
        }

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(strip)
            .child(div().flex_1().min_w_0().min_h_0().child(screen_view))
            .into_any_element()
    }

    /// Render a thin tag bar above the content when any buffers in the workspace
    /// have tags. Tags in the active tab's tag_view get accent background.
    pub(crate) fn wrap_with_tag_bar(&self, screen_view: AnyElement) -> AnyElement {
        let all_tags = self.all_tags();
        if all_tags.is_empty() {
            return screen_view;
        }
        let tag_view = self
            .workspace
            .active_tab()
            .map(|t| &t.tag_view)
            .cloned()
            .unwrap_or_default();
        let accent: Hsla = rgb(STATUS_FG).into();
        let dimmed: Hsla = rgb(0x666666).into();
        let strip_bg: Hsla = bg_or(self.theme.top_bar, STATUS_BG);

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_4()
            .h(px(20.0))
            .bg(strip_bg)
            .text_size(px(10.0));

        for tag in &all_tags {
            let is_active = tag_view.contains(tag);
            let chip = div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .bg(if is_active { accent } else { strip_bg })
                .text_color(if is_active { rgb(BG).into() } else { dimmed })
                .font_weight(if is_active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(SharedString::from(tag.clone()));
            bar = bar.child(chip);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h_0().child(screen_view))
            .into_any_element()
    }

    /// Inject the active tab's rail beside the **focused leaf's** content
    /// (spec-rail.md §8, adjusted: the rail is chrome local to the focused
    /// tile, not the whole window — so in a split it sits against the focused
    /// content, not at the window edge). `content_el` is the already-rendered
    /// focused-leaf element. No-op passthrough when no rail is open.
    /// `rail_focusable` is false when an overlay owns focus — the rail still
    /// renders as background but is not focusable (constraint §4).
    ///
    /// The outline entries are re-derived once per frame in
    /// `render_focused_window` before this runs (spec §13).
    pub(crate) fn wrap_leaf_with_rail(
        &mut self,
        content_el: AnyElement,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .workspace
            .active_tab()
            .map(|t| t.rail.is_none())
            .unwrap_or(true)
        {
            return content_el;
        }

        let (side, focused) = {
            let r = self
                .workspace
                .active_tab()
                .and_then(|t| t.rail.as_ref())
                .expect("rail present");
            (r.side, r.focused && rail_focusable)
        };

        let rail = self.render_rail(focused, cx);

        let content_slot = div().flex_1().min_w_0().min_h_0().child(content_el);

        let row = div().size_full().flex().flex_row().min_w_0().min_h_0();
        let row = match side {
            workspace::RailSide::Left => row.child(rail).child(content_slot),
            workspace::RailSide::Right => row.child(content_slot).child(rail),
        };
        row.into_any_element()
    }

    /// Re-derive the outline rail's heading entries from the focused window
    /// (spec §13). No-op when the rail is closed or showing the file browser.
    /// Change-key for the outline: focused window id + that window's content
    /// version. Re-deriving the outline is O(document) (an Edit tile allocates
    /// the whole rope via `full_text()` and scans every line), and the render
    /// loop runs every frame — including every keystroke. Keying on this lets
    /// `refresh_outline_rail` skip the work when nothing relevant changed.
    pub(crate) fn outline_change_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(tab) = self.workspace.active_tab() {
            tab.focused.hash(&mut h); // focus change → re-derive
        }
        match self.workspace.focused_content() {
            // Edit: edit_seq is the exact monotonic content version.
            Some(App::Buffer(BufferApp::Editing(e))) => e.editor.edit_seq().hash(&mut h),
            // Doc: blocks only change on load/reload/edit-flush; block count is
            // a cheap proxy (outline is cosmetic, so a same-count content change
            // leaving it briefly stale is acceptable).
            Some(App::Buffer(BufferApp::Viewing(d))) => d.blocks.len().hash(&mut h),
            // Agent/Browser have no outline; constant so it derives once (empty).
            _ => 0u64.hash(&mut h),
        }
        h.finish()
    }

    pub(crate) fn refresh_outline_rail(&mut self) {
        let is_outline = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .map(|r| r.content.is_outline())
            .unwrap_or(false);
        if !is_outline {
            return;
        }
        // Skip the O(document) re-derivation when neither the focused window nor
        // its content changed since the last derive (the common case — cursor
        // blink, scroll, cross-tile notify, and unrelated keystrokes).
        let key = self.outline_change_key();
        let unchanged = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .and_then(|r| match &r.content {
                workspace::RailContent::Outline(o) => o.last_key,
                _ => None,
            })
            == Some(key);
        if unchanged {
            return;
        }
        let entries = self.derive_outline();
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::Outline(o) = &mut r.content
        {
            o.entries = entries;
            o.last_key = Some(key);
            if o.selected >= o.entries.len() {
                o.selected = o.entries.len().saturating_sub(1);
            }
        }
    }

    /// Build `(depth, text, block_index_or_line)` heading entries from the
    /// focused window's content (spec §13).
    pub(crate) fn derive_outline(&self) -> Vec<(u8, String, usize)> {
        match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(d))) => {
                let mut out = Vec::new();
                for (idx, block) in d.blocks.iter().enumerate() {
                    if let RenderedBlock::Heading { level, content } = block {
                        out.push((*level, styled_line_plain(content), idx));
                    }
                }
                out
            }
            Some(App::Buffer(BufferApp::Editing(e))) => {
                let text = e.editor.full_text();
                let mut out = Vec::new();
                for (line_no, line) in text.lines().enumerate() {
                    if let Some((level, heading)) = atx_heading(line) {
                        out.push((level, heading, line_no));
                    }
                }
                out
            }
            // Agent / Browser have no outline.
            _ => Vec::new(),
        }
    }

    /// Render the rail column for the active tab (spec §9, §11–§13). Chrome
    /// styling — text is fixed at 12px and does NOT scale with `text_scale`.
    pub(crate) fn render_rail(&self, focused: bool, cx: &mut Context<Self>) -> gpui::Div {
        let rail = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .expect("rail present");

        let top_bar = self.theme.top_bar;
        let rail_bg: Hsla = bg_or(top_bar, STATUS_BG);
        // Unselected entry text: use the brighter overlay *foreground* rather
        // than the dim `overlay.label` token — the label color reads too
        // low-contrast against the rail background. `overlay.fg` is the same
        // high-contrast body color the command menu uses for its entries.
        let label_fg: Hsla = nc(self.theme.overlay.fg);
        // Placeholder text ("(empty)", "(no outline)") stays intentionally dim.
        let muted_fg: Hsla = nc(self.theme.overlay.label);
        let accent_fg: Hsla = nc(self.theme.overlay.accent);
        let selected_bg: Hsla = self.editor_bg();
        let selected_fg: Hsla = rgb(STATUS_FG).into();
        let border_color: Hsla = rgb(0x6272a4).into();
        let side = rail.side;
        let width = rail.width_px;

        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_w(px(width))
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(rail_bg)
            .text_size(px(12.0))
            .font_family(self.body_font.clone());

        // Content-facing border (right when Left, left when Right).
        col = match side {
            workspace::RailSide::Left => col.border_r_1().border_color(border_color),
            workspace::RailSide::Right => col.border_l_1().border_color(border_color),
        };

        // When focused, attach the focus handle inside the RailView key
        // context so its context-scoped bindings (j/k/enter/…) match.
        let mut col = col.key_context("RailView");
        if focused {
            col = col.track_focus(&self.focus_handle);
        }
        col = col
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_rail_filter_key(ev, w, cx);
            }))
            .on_action(cx.listener(Self::rail_down))
            .on_action(cx.listener(Self::rail_up))
            .on_action(cx.listener(Self::rail_select))
            .on_action(cx.listener(Self::rail_close))
            .on_action(cx.listener(Self::rail_parent))
            .on_action(cx.listener(Self::rail_toggle_hidden))
            .on_action(cx.listener(Self::rail_cycle_sort))
            .on_action(cx.listener(Self::rail_worktrees))
            .on_action(cx.listener(Self::rail_filter))
            // Global actions forwarded so they keep working while the rail is
            // focused (same pattern as every other screen root).
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
            .on_action(cx.listener(Self::move_tile))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Tile focus motion — without these the ctrl-w h/j/k/l chords
            // are swallowed when the rail holds `track_focus`.
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev));

        match &rail.content {
            workspace::RailContent::FileBrowser(fb) => {
                if let Some(wm) = &fb.worktree_mode {
                    // ── Worktree picker overlay ──────────────────────
                    let header = div()
                        .px_2()
                        .py_1()
                        .flex_none()
                        .text_color(accent_fg)
                        .font_weight(FontWeight::BOLD)
                        .overflow_hidden()
                        .child(SharedString::new_static("WORKTREES"));

                    let mut list = div().flex().flex_col().flex_1().min_h_0().overflow_hidden();

                    if wm.worktrees.is_empty() {
                        list = list.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(muted_fg)
                                .child(SharedString::new_static("  (no worktrees)")),
                        );
                    } else {
                        let visible_rows = 40usize;
                        let scroll =
                            scroll_to_keep_visible(wm.selected, visible_rows, wm.worktrees.len());
                        for (i, wt) in wm
                            .worktrees
                            .iter()
                            .enumerate()
                            .skip(scroll)
                            .take(visible_rows)
                        {
                            let is_sel = i == wm.selected;
                            let marker = if wt.is_current {
                                "* "
                            } else if is_sel {
                                "▸ "
                            } else {
                                "  "
                            };
                            let label = format!("{}{}", marker, wt.label);
                            let (rbg, rfg) = if is_sel {
                                (selected_bg, selected_fg)
                            } else {
                                (rail_bg, accent_fg)
                            };
                            list = list.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .py_0p5()
                                    .bg(rbg)
                                    .text_color(rfg)
                                    .overflow_hidden()
                                    .child(SharedString::from(label)),
                            );
                        }
                    }
                    col.child(header).child(list)
                } else {
                    // ── Normal file browser ──────────────────────────
                    let dir_str = fb.current_dir().display().to_string();
                    let header_text = if fb.filter_mode {
                        format!("/{}", fb.filter_text())
                    } else {
                        format!("▸ {}", dir_str)
                    };
                    let header = div()
                        .px_2()
                        .py_1()
                        .flex_none()
                        .text_color(accent_fg)
                        .font_weight(FontWeight::BOLD)
                        .overflow_hidden()
                        .child(SharedString::from(header_text));

                    let mut list = div().flex().flex_col().flex_1().min_h_0().overflow_hidden();

                    let entries = fb.visible_entries();
                    let selected = fb.selected();
                    if entries.is_empty() {
                        let msg = if fb.filter_mode {
                            "  (no matches)"
                        } else {
                            "  (empty)"
                        };
                        list = list.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(muted_fg)
                                .child(SharedString::new_static(msg)),
                        );
                    } else {
                        let visible_rows = 40usize;
                        let scroll = scroll_to_keep_visible(selected, visible_rows, entries.len());
                        for (i, entry) in entries.iter().enumerate().skip(scroll).take(visible_rows)
                        {
                            let is_sel = i == selected;
                            let suffix = if entry.is_dir { "/" } else { "" };
                            let name = format!("{}{}", entry.name, suffix);
                            let (rbg, rfg) = if is_sel {
                                (selected_bg, selected_fg)
                            } else if entry.is_dir {
                                (rail_bg, accent_fg)
                            } else {
                                (rail_bg, label_fg)
                            };
                            list = list.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .py_0p5()
                                    .bg(rbg)
                                    .text_color(rfg)
                                    .overflow_hidden()
                                    .child(SharedString::from(name)),
                            );
                        }
                    }
                    col.child(header).child(list)
                }
            }
            workspace::RailContent::Outline(o) => {
                let header = div()
                    .px_2()
                    .py_1()
                    .flex_none()
                    .text_color(accent_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("OUTLINE"));

                let mut list = div().flex().flex_col().flex_1().min_h_0().overflow_hidden();

                if o.entries.is_empty() {
                    list = list.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(muted_fg)
                            .child(SharedString::new_static("(no outline)")),
                    );
                } else {
                    let visible_rows = 40usize;
                    let scroll = scroll_to_keep_visible(o.selected, visible_rows, o.entries.len());
                    for (i, (level, text, _)) in
                        o.entries.iter().enumerate().skip(scroll).take(visible_rows)
                    {
                        let is_sel = i == o.selected;
                        // Indent by heading depth; depth-1 headings are
                        // section headers (accent + bold).
                        let indent = "  ".repeat((*level as usize).saturating_sub(1));
                        let label_text = format!("{}{}", indent, text);
                        let mut row = div().w_full().px_2().py_0p5().overflow_hidden();
                        if is_sel {
                            row = row.bg(selected_bg).text_color(selected_fg);
                        } else if *level == 1 {
                            row = row.text_color(accent_fg).font_weight(FontWeight::BOLD);
                        } else {
                            row = row.text_color(label_fg);
                        }
                        list = list.child(row.child(SharedString::from(label_text)));
                    }
                }
                col.child(header).child(list)
            }
        }
    }
}
