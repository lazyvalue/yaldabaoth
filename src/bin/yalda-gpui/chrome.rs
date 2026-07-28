//! Window chrome on YaldaGpuiView: focused-window/layout render, wsp
//! strip, tag bar, rails (render + outline derivation). Extracted
//! verbatim from main.rs (split-gpui-main, stage 2).

use super::*;

/// Desktop-mode chrome constants (spec-desktop-mode.md Behavior 3/4).
pub(crate) const DESKTOP_GUTTER: f32 = 12.0;
pub(crate) const DESKTOP_MIN_TILE_W: f32 = 160.0;
pub(crate) const DESKTOP_MIN_TILE_H: f32 = 120.0;
const DESKTOP_TITLE_H: f32 = 20.0;
const DESKTOP_DRAG_THRESHOLD: f32 = 4.0;
const DESKTOP_EDGE_PAN_BAND: f32 = 30.0;
const DESKTOP_EDGE_PAN_STEP: f32 = 12.0;
/// Width of the east/south edge bands that arm a tile resize (spec 4b).
const DESKTOP_RESIZE_BAND: f32 = 6.0;

/// Full-detail tile size for a measured canvas. The requested grid density
/// determines slot pitch until the live-tile readability floor is reached.
/// Below that point the infinite plane simply shows fewer complete slots.
pub(crate) fn desktop_tile_size_for_canvas(
    canvas_w: f32,
    canvas_h: f32,
    cols: u32,
    rows: u32,
) -> (f32, f32) {
    let cols = cols.max(1) as f32;
    let rows = rows.max(1) as f32;
    (
        ((canvas_w - (cols + 1.0) * DESKTOP_GUTTER) / cols).max(DESKTOP_MIN_TILE_W),
        ((canvas_h - (rows + 1.0) * DESKTOP_GUTTER) / rows).max(DESKTOP_MIN_TILE_H),
    )
}

impl YaldaGpuiView {
    /// Build the menu popup as an absolutely-positioned overlay anchored
    /// to the top of the window. Renders header (breadcrumb), entry list,
    /// and a footer hint. Has *no* key handlers — the wrapper in
    /// `Render::render` handles input via `capture_key_down` so the
    /// underlying screen never sees keystrokes while the menu is open.
    /// Render the active workspace's layout tree. Leaves dispatch to per-kind
    /// render methods; splits become flex containers (row for V splits,
    /// col for H splits) with weighted children.
    pub(crate) fn render_focused_window(
        &mut self,
        root: gpui::Div,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let workspace_idx = self.workspace.active_workspace;
        let focused_id = self.workspace.workspaces[workspace_idx].focused;
        // Re-derive the outline rail (if any) once before rendering the tree,
        // so the focused leaf can render it inline without a second pass.
        self.refresh_outline_rail();
        let layout_ptr: *mut workspace::Layout<App> =
            &mut self.workspace.workspaces[workspace_idx].layout as *mut _;
        // SAFETY: `layout_ptr` is valid for as long as the active workspace's
        // `layout` field isn't structurally mutated (no splits/closes/etc.).
        // The render pipeline only reads self's other fields (theme/fonts)
        // and the layout subtree via this pointer; structural mutations
        // happen in action handlers, never inside render. This sidesteps a
        // Rust borrowck limitation where the compiler can't prove that
        // &mut Layout<App> (a field inside self.workspace.workspaces)
        // is disjoint from &self.render_X's other field accesses.
        let layout = unsafe { &mut *layout_ptr };
        // The plane is the ONLY workspace interior (infinite-plane, Stage D): a
        // workspace IS a Plane (Behavior 1), so the workspace always renders as the
        // desktop/plane canvas. The old split-tree branch (`render_layout`) is
        // retired along with the mode surface.
        self.render_desktop(root, layout, focused_id, attach_focus, rail_focusable, cx)
    }

    /// Desktop mode (spec-desktop-mode.md): fixed-size tiles at slot
    /// positions on a pannable canvas. The layout tree is the CONTENT owner
    /// (leaves render exactly as in tiling); geometry comes from the workspace's
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
        let workspace_idx = self.workspace.active_workspace;
        let full_tile = self.desktop_tile_px();
        let (_, _, mut canvas_w, mut canvas_h) = self.desktop_canvas_bounds.get();
        // First frame: bounds not captured yet — approximate with the window
        // viewport; the next frame self-corrects.
        if canvas_w <= 0.0 {
            canvas_w = self.viewport_width_px.max(1.0);
        }
        if canvas_h <= 0.0 {
            canvas_h = self.viewport_height_px.max(1.0);
        }
        // Semantic-zoom Detail (Stage C, spec Behavior 3): the level chooses the
        // tile REPRESENTATION (Full live tile · Card placeholder · Minimap pip)
        // AND the slot pitch. Pitch is derived per-axis from the Full pitch
        // scaled by `detail_scale(zoom)` — ONE conversion boundary: `tile` and
        // `g` below are the level-effective tile size + gutter, and ALL slot
        // geometry (`slot_origin`/`tile_rect`/`slot_at`, dot grid, drag, reveal)
        // runs against them, so zooming out shrinks the whole plane uniformly
        // without touching a single slot/span.
        let zoom = self.workspace.workspaces[workspace_idx].desktop.camera.zoom;
        let scale = workspace::detail_scale(zoom);
        let tile = (full_tile.0 * scale, full_tile.1 * scale);
        let g = DESKTOP_GUTTER * scale;
        // Per-axis effective slot pitch (pixels per slot): cell = tile + gutter.
        // Camera pan is in pitch-independent SLOT units; pixels are derived here
        // at the view boundary (`pan_px = cam.pan · pitch`), and pan MUTATIONS
        // divide back by pitch. Because both `tile` and `g` scale by the same
        // factor, `pan` in slot units names the SAME plane location at every
        // Detail (spec D2) — the zoom re-anchor needs no cross-pitch conversion.
        let pitch = (tile.0 + g, tile.1 + g);

        // ── Slot upkeep: seed/reconcile every frame (cheap, O(n), no-op when
        // the non-overlap invariant already holds; slotless leaves seed on the
        // origin ring-spiral), reveal the focused tile when focus changed. Pan
        // is UNCLAMPED now — the plane is infinite in all directions
        // (Behavior 5). ──
        {
            let wsp = &mut self.workspace.workspaces[workspace_idx];
            let leaves = wsp.layout.leaf_ids();
            // Seed beside the tile the user is on (bug-0012): a brand-new leaf
            // IS the focused one and has no slot yet, so `reconcile_near` falls
            // back to `last_reveal` — still the tile focus came from.
            wsp.desktop.reconcile_near(&leaves, Some(focused_id));
            if wsp.desktop.last_reveal != Some(focused_id) {
                if let Some(slot) = wsp.desktop.slot_of(focused_id) {
                    let (x, y) = workspace::slot_origin(slot, tile, g);
                    // Reveal in pixel space, then store the pan back in slot units.
                    let mut pan_px = (
                        wsp.desktop.camera.pan.0 * pitch.0,
                        wsp.desktop.camera.pan.1 * pitch.1,
                    );
                    // Whether we actually had to pan to reveal the tile — a focus
                    // change to an already-fully-visible tile must NOT move the view
                    // (UXI-Workspace-8: only snap when a reveal actually fired).
                    let mut revealed = false;
                    if x - g < pan_px.0 {
                        pan_px.0 = x - g;
                        revealed = true;
                    } else if x + tile.0 + g > pan_px.0 + canvas_w {
                        pan_px.0 = x + tile.0 + g - canvas_w;
                        revealed = true;
                    }
                    if y - g < pan_px.1 {
                        pan_px.1 = y - g;
                        revealed = true;
                    } else if y + tile.1 + g > pan_px.1 + canvas_h {
                        pan_px.1 = y + tile.1 + g - canvas_h;
                        revealed = true;
                    }
                    if revealed {
                        wsp.desktop.camera.pan = (pan_px.0 / pitch.0, pan_px.1 / pitch.1);
                        // Rest the view cell-aligned like the tile (UXI-Workspace-8).
                        wsp.desktop.snap_camera_to_slots();
                    }
                }
                wsp.desktop.last_reveal = Some(focused_id);
            }
        }

        let wsp = &self.workspace.workspaces[workspace_idx];
        // Derived pixel pan for the rest of this render (all existing pixel math
        // reads `pan`).
        let pan = (
            wsp.desktop.camera.pan.0 * pitch.0,
            wsp.desktop.camera.pan.1 * pitch.1,
        );
        let drag = wsp.desktop.drag;
        let slot_list: Vec<(workspace::WindowId, workspace::Slot, workspace::Span)> = wsp
            .desktop
            .slots
            .iter()
            .map(|&(id, s)| (id, s, wsp.desktop.span_of(id)))
            .collect();
        // A lone tile fills the canvas — a desktop with one window shouldn't
        // strand it in a single grid quadrant (the jump-panel virtual
        // workspace lands here). Drag/resize/pan are meaningless with one
        // tile, so the maximized branch ignores slot geometry entirely.
        // Full-ONLY (Stage C, spec Behavior 3): at Card/Minimap a maximize would
        // fill the viewport with one card/pip and defeat the overview, so every
        // tile renders at its true slot geometry once zoomed out.
        let maximized = slot_list.len() == 1 && zoom == workspace::Detail::Full;
        // Live edge-resize preview (spec Behavior 4b): the clamped anchor +
        // span the resized tile renders at this frame, which is also what
        // commits. West/North move the anchor, so the preview carries it.
        let resize_preview: Option<(workspace::WindowId, workspace::Slot, workspace::Span)> = wsp
            .desktop
            .resize
            .map(|r| {
                let (slot, span) = self.desktop_resize_target(r);
                (r.id, slot, span)
            });
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
            // Plane-camera keymap actions (Ctrl-W -/=/0) live on the CANVAS root,
            // not the per-screen leaf roots — the canvas is the common ancestor
            // of every tile AND the ONLY thing that renders at Card/Minimap (where
            // tiles are placeholders with no per-screen `on_action` wiring). Wiring
            // here is what lets you zoom back IN from Minimap; the focused tile /
            // placeholder carries the focus handle inside this subtree so the
            // action always has a handler in its ancestry (spec Behavior 3, C5).
            .on_action(cx.listener(Self::zoom_out_workspace))
            .on_action(cx.listener(Self::zoom_in_workspace))
            .on_action(cx.listener(Self::reset_workspace_view))
            // Wheel/trackpad routing (Stage C, spec Behavior 5). This handler
            // fires in the BUBBLE phase, so at Full a scroll a live tile's inner
            // list already consumed still reaches here — `desktop_scroll` guards
            // that by testing whether the pointer sits over a tile at Full and
            // swallowing there (tile content scrolls). At Card/Minimap content
            // isn't live, so bare wheel pans the plane everywhere; and
            // `Cmd`/`Ctrl`+scroll steps the zoom at EVERY level (anchored on the
            // focused tile). Exact scroll FEEL is a NEEDS-RUNTIME gap.
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _w, cx| {
                this.desktop_scroll(ev, cx);
            }))
            // `Cmd+Shift`+left-drag pans the plane (spec Behavior 5). Armed on
            // the canvas root so it works over tiles too; the pan gesture takes
            // precedence over any tile drag in `desktop_pointer_move`.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    this.desktop_pan_grab(
                        (f32::from(ev.position.x), f32::from(ev.position.y)),
                        ev.modifiers,
                        cx,
                    );
                }),
            )
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

        // ── Dot grid over the visible area (slot-pitch corners). Signed: the
        // viewport may sit left/above the origin, so the first visible slot is
        // derived from the (possibly negative) pixel pan via `.floor() as i32`
        // (NOT a bare `as i32`, which truncates toward zero). ──
        {
            let first_col = (pan.0 / pitch.0).floor() as i32;
            let first_row = (pan.1 / pitch.1).floor() as i32;
            let ncols = (canvas_w / pitch.0).ceil() as i32 + 1;
            let nrows = (canvas_h / pitch.1).ceil() as i32 + 1;
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
        for (id, mut slot, mut span) in slot_list {
            // The tile being resized renders at its live clamped anchor + span
            // so the grow/shrink is visible under the cursor (spec Behavior 4b).
            // West/North move the anchor, hence the slot override too.
            if let Some((rid, rslot, rspan)) = resize_preview {
                if rid == id {
                    slot = rslot;
                    span = rspan;
                }
            }
            let (_, _, mut tw, mut th) = workspace::tile_rect(slot, span, tile, g);
            let dragging = drag.filter(|d| d.active && d.id == id);
            let (sx, sy) = workspace::slot_origin(slot, tile, g);
            let (mut x, mut y) = match dragging {
                // The dragged tile itself follows the pointer — the real
                // content rides along semi-transparent (no separate ghost).
                Some(d) => (
                    d.pointer.0 - d.grab.0 - pan.0,
                    d.pointer.1 - d.grab.1 - pan.1,
                ),
                None => (sx - pan.0, sy - pan.1),
            };
            if maximized {
                x = g;
                y = g;
                tw = (canvas_w - 2.0 * g).max(160.0);
                th = (canvas_h - 2.0 * g).max(120.0);
            }
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

            // ── Semantic-zoom placeholders (Stage C, spec Behavior 3). At
            // Card/Minimap the tile is a CHEAP placeholder — NO live App content
            // (no transcript, no doc render), so per-frame cost is O(visible
            // tiles) and strictly lower than Full (Constraint C2). The focused
            // tile still carries the focus handle here (C5) so the keyboard
            // survives; plane-level actions live on the canvas root regardless. ──
            if zoom != workspace::Detail::Full {
                let glyph = Self::desktop_status_glyph(&self.sessions, content, cx);
                let placeholder = self.desktop_placeholder(
                    zoom, id, x, y, tw, th, is_focused, attach_focus, &title, glyph, mark, accent,
                    dim, tile_bg, title_bg, content_fg,
                );
                canvas = canvas.child(placeholder);
                continue;
            }

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
                App::Linear(tile) => self.render_linear(leaf_root, tile, cx).into_any_element(),
                App::Keymap(tile) => self.render_keymap(leaf_root, tile, cx).into_any_element(),
            };
            // Tag the LIVE content so the layout probe can assert it paints at
            // Full and is ABSENT at Card/Minimap (the semantic-zoom guard,
            // `plane_card_zoom_paints_placeholders_not_live_content`). Only the
            // Full branch reaches here — the Card/Minimap branch above returns
            // early with no live content built.
            let inner = probe_bounds_dyn(format!("plane-tile-content-{id}"), inner);

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
                .child(title);
            // A maximized lone tile has nowhere to be dragged or resized to,
            // so it's pinned: no grab cursor, no move/resize handlers.
            if !maximized {
                title_bar = title_bar.cursor_grab().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_grab(
                            id,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                );
            }
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
            // West / north bands: pulling these moves the anchor out toward the
            // origin (enlarge leftward/upward). The north band rides the very
            // top — over the title bar's top strip — so the rest of the title
            // bar still grabs-to-move.
            let west_band = div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(DESKTOP_RESIZE_BAND))
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_resize_grab(
                            id,
                            workspace::ResizeEdge::West,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                );
            let north_band = div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(DESKTOP_RESIZE_BAND))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.desktop_resize_grab(
                            id,
                            workspace::ResizeEdge::North,
                            (f32::from(ev.position.x), f32::from(ev.position.y)),
                            cx,
                        );
                    }),
                );

            // UXI-Workspace-9 (click-to-focus): a LEFT press in the tile BODY focuses
            // an unfocused tile and is CONSUMED — capture phase + `stop_propagation`
            // breaks out of the capture loop AND skips the whole bubble loop, so the
            // content (transcript selection sink, compose input, buttons) never sees
            // the focus-changing click. You click again to interact. Focus is resolved
            // at EVENT time, not captured from this render (the interactive-rows rule
            // in `yux/CLAUDE.md` — a cache hit would otherwise reuse a stale flag).
            // The title bar and the four resize bands are SIBLINGS of this div, so
            // their focus-and-arm-a-gesture behavior is untouched by construction.
            let tile_body = div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(inner)
                .capture_any_mouse_down(cx.listener(
                    move |this, ev: &MouseDownEvent, _w, cx| {
                        if ev.button != MouseButton::Left {
                            return;
                        }
                        if this.workspace.focused_window_id() == Some(id) {
                            return; // already focused ⇒ normal interaction, nothing swallowed
                        }
                        this.desktop_focus_click(id, cx);
                        cx.stop_propagation();
                    },
                ));

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
                .child(tile_body);
            if !maximized {
                frame = frame
                    .child(east_band)
                    .child(south_band)
                    .child(west_band)
                    .child(north_band);
            }
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
    pub(crate) fn desktop_tile_px(&self) -> (f32, f32) {
        let (_, _, mut w, mut h) = self.desktop_canvas_bounds.get();
        if w <= 0.0 {
            w = self.viewport_width_px.max(1.0);
        }
        if h <= 0.0 {
            h = self.viewport_height_px.max(1.0);
        }
        desktop_tile_size_for_canvas(w, h, self.desktop_grid_cols, self.desktop_grid_rows)
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
            App::Agent(tile) => tile.session()
                .and_then(|id| sessions.get(id))
                .map(|s| s.read(cx).label.clone())
                .unwrap_or_else(|| "claude".to_string()),
            App::Linear(tile) => tile.title(),
            App::Keymap(tile) => tile.title(),
        }
    }

    /// A one-char status glyph for a tile's Card representation (Stage C, spec
    /// Behavior 3): agent tiles show `●` while a turn is awaiting a reply,
    /// otherwise `○`; other App kinds use a per-kind static glyph. Takes
    /// `sessions` directly for the same disjoint-borrow reason as
    /// [`desktop_tile_title`](Self::desktop_tile_title).
    fn desktop_status_glyph(sessions: &AgentSessions, content: &App, cx: &GpuiApp) -> &'static str {
        match content {
            App::Agent(tile) => {
                let busy = tile.session()
                    .and_then(|id| sessions.get(id))
                    .map(|s| s.read(cx).state.turn_phase.is_awaiting())
                    .unwrap_or(false);
                if busy { "●" } else { "○" }
            }
            App::Buffer(BufferApp::Editing(_)) => "✎",
            App::Buffer(BufferApp::Viewing(_)) => "▢",
            App::Buffer(BufferApp::Picking(_)) => "◇",
            App::Linear(_) => "◈",
            App::Keymap(_) => "⌘",
        }
    }

    /// Build the Card / Minimap placeholder for one tile (Stage C, spec
    /// Behavior 3). CHEAP — no live App content (Constraint C2): a `Card` is a
    /// compact frame (title/label + status glyph + mark badge) at the tile's
    /// true slot rect; a `Minimap` pip is a filled rect over the tile's span,
    /// labelled only when focused. The focused placeholder carries the focus
    /// handle so the keyboard survives a zoomed-out overview (C5). Tagged
    /// `plane-card-{id}` for the layout-probe guard.
    #[allow(clippy::too_many_arguments)]
    fn desktop_placeholder(
        &self,
        zoom: workspace::Detail,
        id: workspace::WindowId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        is_focused: bool,
        attach_focus: bool,
        title: &str,
        glyph: &'static str,
        mark: Option<char>,
        accent: Hsla,
        dim: Hsla,
        tile_bg: Hsla,
        title_bg: Hsla,
        content_fg: Hsla,
    ) -> AnyElement {
        let mut frame = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .border_1()
            .border_color(if is_focused { accent } else { dim.opacity(0.4) });
        // The focused placeholder carries the focus handle (C5) — plane-level
        // actions live on the canvas root, but the handle must exist somewhere
        // in the tree or the keyboard strands. No per-screen `on_action` wiring
        // (content isn't live at Card/Minimap).
        if is_focused && attach_focus {
            frame = frame.track_focus(&self.focus_handle);
        }

        let body: AnyElement = match zoom {
            workspace::Detail::Minimap => {
                // A pip: a filled rect the size of the tile's span. Label only on
                // the FOCUSED pip (spec Behavior 3).
                let mut pip = div()
                    .size_full()
                    .rounded_sm()
                    .bg(if is_focused { accent.opacity(0.55) } else { dim.opacity(0.45) });
                if is_focused {
                    pip = pip.child(
                        div()
                            .px_1()
                            .text_size(px(8.0))
                            .text_color(content_fg)
                            .child(title.to_string()),
                    );
                }
                pip.into_any_element()
            }
            _ => {
                // Card: title/label + status glyph + mark badge. No live content.
                let mut header = div()
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
                    .child(div().child(glyph.to_string()))
                    .child(div().child(title.to_string()));
                if let Some(m) = mark {
                    header = header
                        .child(div().px_1().text_color(accent).child(format!("[{m}]")));
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .bg(tile_bg)
                    .child(header)
                    .into_any_element()
            }
        };
        let frame = frame.child(body).into_any_element();
        probe_bounds_dyn(format!("plane-card-{id}"), frame)
    }

    /// The slot a semantic-zoom step re-anchors on (Stage C, spec Behavior 3):
    /// the focused tile's slot, or the viewport-center slot when nothing is
    /// focused / the focused tile has no slot. `pitch` is the current effective
    /// per-axis pitch; `pan` is the derived pixel pan.
    pub(crate) fn desktop_zoom_anchor(
        &self,
        workspace_idx: usize,
        focused_id: workspace::WindowId,
        tile: (f32, f32),
        g: f32,
        pan: (f32, f32),
        canvas_w: f32,
        canvas_h: f32,
    ) -> workspace::Slot {
        let wsp = &self.workspace.workspaces[workspace_idx];
        if let Some(s) = wsp.desktop.slot_of(focused_id) {
            return s;
        }
        // Viewport center in desktop (pre-pan) pixels → slot.
        let center = (pan.0 + canvas_w / 2.0, pan.1 + canvas_h / 2.0);
        workspace::slot_at(center, tile, g)
    }

    /// Wheel/trackpad routing on the desktop canvas (Stage C, spec Behavior 5).
    /// `Cmd`/`Ctrl`+scroll steps the semantic zoom (anchored on the focused tile,
    /// or viewport center) at every level. Bare scroll pans the plane — at
    /// Card/Minimap always (content isn't live); at Full only when the pointer
    /// is NOT over a live tile (this handler fires in the bubble phase, so a
    /// scroll a tile's inner list consumed still reaches here — we swallow it
    /// over a tile so the tile keeps scrolling, and pan over empty canvas). Pan
    /// is mutated in SLOT units (pixel delta ÷ pitch). Exact feel is
    /// NEEDS-RUNTIME.
    pub(crate) fn desktop_scroll(
        &mut self,
        ev: &gpui::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let full_tile = self.desktop_tile_px();
        let (cx0, cy0, mut cw, mut ch) = self.desktop_canvas_bounds.get();
        if cw <= 0.0 {
            cw = self.viewport_width_px.max(1.0);
        }
        if ch <= 0.0 {
            ch = self.viewport_height_px.max(1.0);
        }
        let workspace_idx = self.workspace.active_workspace;
        let zoom = self.workspace.workspaces[workspace_idx].desktop.camera.zoom;
        let scale = workspace::detail_scale(zoom);
        let tile = (full_tile.0 * scale, full_tile.1 * scale);
        let g = DESKTOP_GUTTER * scale;
        let pitch = (tile.0 + g, tile.1 + g);
        let pan = {
            let cam = self.workspace.workspaces[workspace_idx].desktop.camera;
            (cam.pan.0 * pitch.0, cam.pan.1 * pitch.1)
        };

        // Pixel delta (line deltas are scaled by a nominal line height).
        let delta = ev.delta.pixel_delta(px(16.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));

        // Zoom: `Cmd`/`Ctrl`+scroll steps Detail (secondary() is Cmd on macOS,
        // the platform key; also accept raw control for portability).
        if ev.modifiers.secondary() || ev.modifiers.control {
            let focused_id = self.workspace.workspaces[workspace_idx].focused;
            let anchor = self.desktop_zoom_anchor(workspace_idx, focused_id, tile, g, pan, cw, ch);
            let wsp = &mut self.workspace.workspaces[workspace_idx];
            if dy > 0.0 {
                wsp.desktop.zoom_in(anchor);
            } else if dy < 0.0 {
                wsp.desktop.zoom_out(anchor);
            } else {
                return;
            }
            self.save_workspace_state();
            cx.notify();
            return;
        }

        // Bare scroll does NOT pan — panning is `Cmd+Shift`+drag (Behavior 5).
        // Let the event bubble so a tile's own inner content still scrolls;
        // over empty canvas it is simply a no-op.
        let _ = (dx, cx0, cy0, g);
    }

    /// `Cmd+Shift`+left mouse-down anywhere on the canvas arms a plane pan
    /// (spec Behavior 5). Without both modifiers it is a no-op, so ordinary
    /// clicks / tile drags are unaffected. The gesture is applied in
    /// [`desktop_pointer_move`](Self::desktop_pointer_move) and ended in
    /// [`desktop_drop`](Self::desktop_drop).
    pub(crate) fn desktop_pan_grab(
        &mut self,
        window_pos: (f32, f32),
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        // `secondary()` is Cmd on macOS (the platform key); require Shift too.
        if !(modifiers.secondary() && modifiers.shift) {
            return;
        }
        let workspace_idx = self.workspace.active_workspace;
        let start_pan = self.workspace.workspaces[workspace_idx].desktop.camera.pan;
        self.workspace.workspaces[workspace_idx].desktop.pan_drag = Some(workspace::DesktopPan {
            start_pointer: window_pos,
            start_pan,
        });
        cx.notify();
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
        let workspace_idx = self.workspace.active_workspace;
        let wsp = &mut self.workspace.workspaces[workspace_idx];
        wsp.focused = id;
        let Some(slot) = wsp.desktop.slot_of(id) else {
            cx.notify();
            return;
        };
        let pitch = (tile.0 + DESKTOP_GUTTER, tile.1 + DESKTOP_GUTTER);
        let pan = (
            wsp.desktop.camera.pan.0 * pitch.0,
            wsp.desktop.camera.pan.1 * pitch.1,
        );
        let desktop_pos = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
        let (ox, oy) = workspace::slot_origin(slot, tile, DESKTOP_GUTTER);
        wsp.desktop.drag = Some(workspace::DesktopDrag {
            id,
            grab: (desktop_pos.0 - ox, desktop_pos.1 - oy),
            pointer: desktop_pos,
            target: None,
            active: false,
        });
        self.save_workspace_state();
        cx.notify();
    }

    /// Click-to-focus (UXI-Workspace-9): focus a tile because the user pressed
    /// inside its **body**, WITHOUT arming a drag. This is the focus-only twin of
    /// `desktop_grab` — the title bar / resize bands focus *and* arm a gesture, the
    /// content area only focuses (and the press is consumed by the caller, so the
    /// content never sees the focus-changing click).
    pub(crate) fn desktop_focus_click(
        &mut self,
        id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) {
        let workspace_idx = self.workspace.active_workspace;
        self.workspace.workspaces[workspace_idx].focused = id;
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
        let tile = self.desktop_tile_px();
        let workspace_idx = self.workspace.active_workspace;
        let wsp = &mut self.workspace.workspaces[workspace_idx];
        wsp.focused = id;
        let pitch = (tile.0 + DESKTOP_GUTTER, tile.1 + DESKTOP_GUTTER);
        let pan = (
            wsp.desktop.camera.pan.0 * pitch.0,
            wsp.desktop.camera.pan.1 * pitch.1,
        );
        let desktop_pos = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
        wsp.desktop.resize = Some(workspace::DesktopResize {
            id,
            edge,
            pointer: desktop_pos,
        });
        cx.notify();
    }

    /// The Block-clamped (anchor, span) a live resize would commit, given its
    /// pointer (spec Behavior 4b). Used both for the render preview and the
    /// commit, so what you see is exactly what lands. East/South keep the
    /// anchor and grow the far edge; West/North move the anchor (the pulled
    /// near edge follows the pointer, the far edge stays put).
    fn desktop_resize_target(&self, r: workspace::DesktopResize) -> (workspace::Slot, workspace::Span) {
        let tile = self.desktop_tile_px();
        let g = DESKTOP_GUTTER;
        let wsp = &self.workspace.workspaces[self.workspace.active_workspace];
        let Some(anchor) = wsp.desktop.slot_of(r.id) else {
            return (workspace::Slot::new(0, 0), wsp.desktop.span_of(r.id));
        };
        let span = wsp.desktop.span_of(r.id);
        let (ox, oy) = workspace::slot_origin(anchor, tile, g);
        let desired = match r.edge {
            // Far edge from the anchor: n cells end at origin + n*(tile+g) - g,
            // so n = (edge_pos - origin + g) / (tile+g).
            workspace::ResizeEdge::East => {
                ((r.pointer.0 - ox + g) / (tile.0 + g)).round().max(1.0) as u32
            }
            workspace::ResizeEdge::South => {
                ((r.pointer.1 - oy + g) / (tile.1 + g)).round().max(1.0) as u32
            }
            // Near edge moves: the new total extent is (fixed far edge - the
            // column/row the pointer lands on). Far edge sits at anchor + span.
            workspace::ResizeEdge::West => {
                let near = workspace::slot_at(r.pointer, tile, g).col;
                (anchor.col + span.cols as i32 - near).max(1) as u32
            }
            workspace::ResizeEdge::North => {
                let near = workspace::slot_at(r.pointer, tile, g).row;
                (anchor.row + span.rows as i32 - near).max(1) as u32
            }
        };
        wsp.desktop.clamp_resize(r.id, r.edge, desired)
    }

    /// Canvas mouse-move: advance the drag (threshold, pointer, drop target,
    /// edge auto-pan), or a live resize. No-op when neither is armed.
    pub(crate) fn desktop_pointer_move(&mut self, window_pos: (f32, f32), cx: &mut Context<Self>) {
        let (cx0, cy0, cw, ch) = self.desktop_canvas_bounds.get();
        let tile = self.desktop_tile_px();
        let pitch = (tile.0 + DESKTOP_GUTTER, tile.1 + DESKTOP_GUTTER);
        let workspace_idx = self.workspace.active_workspace;

        // A `Cmd+Shift` canvas pan takes precedence over any tile drag/resize:
        // move the camera relative to the grab, converting the pixel delta to
        // slot units at the CURRENT zoom pitch (pan is pitch-independent).
        if let Some(p) = self.workspace.workspaces[workspace_idx].desktop.pan_drag {
            let scale = workspace::detail_scale(self.workspace.workspaces[workspace_idx].desktop.camera.zoom);
            let zpitch = (
                (tile.0 * scale) + DESKTOP_GUTTER * scale,
                (tile.1 * scale) + DESKTOP_GUTTER * scale,
            );
            let dx = window_pos.0 - p.start_pointer.0;
            let dy = window_pos.1 - p.start_pointer.1;
            // Grab-and-drag: content follows the cursor, so the camera pan moves
            // opposite the pointer.
            self.workspace.workspaces[workspace_idx].desktop.camera.pan =
                (p.start_pan.0 - dx / zpitch.0, p.start_pan.1 - dy / zpitch.1);
            cx.notify();
            return;
        }

        // A live resize takes precedence over (and is mutually exclusive with)
        // a drag: just track the pointer; the render pass clamps the span.
        {
            let wsp = &mut self.workspace.workspaces[workspace_idx];
            if let Some(mut r) = wsp.desktop.resize {
                let pan = (
                    wsp.desktop.camera.pan.0 * pitch.0,
                    wsp.desktop.camera.pan.1 * pitch.1,
                );
                r.pointer = (window_pos.0 - cx0 + pan.0, window_pos.1 - cy0 + pan.1);
                wsp.desktop.resize = Some(r);
                cx.notify();
                return;
            }
        }

        // Edge auto-pan first (uses window-relative position within canvas).
        let mut pan_delta = (0.0f32, 0.0f32);
        let rel = (window_pos.0 - cx0, window_pos.1 - cy0);
        let wsp = &mut self.workspace.workspaces[workspace_idx];
        let Some(mut d) = wsp.desktop.drag else {
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
            // Edge auto-pan is a pixel delta; convert to slot units. Unclamped
            // — the plane is infinite in all directions (Behavior 5).
            wsp.desktop
                .pan_by(pan_delta.0 / pitch.0, pan_delta.1 / pitch.1);
        }
        let pan = (
            wsp.desktop.camera.pan.0 * pitch.0,
            wsp.desktop.camera.pan.1 * pitch.1,
        );
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
        wsp.desktop.drag = Some(d);
        cx.notify();
    }

    /// Canvas mouse-up: commit the drop (insert-and-shift) or treat as a
    /// click when the threshold was never crossed.
    pub(crate) fn desktop_drop(&mut self, cx: &mut Context<Self>) {
        let workspace_idx = self.workspace.active_workspace;

        // End a `Cmd+Shift` canvas pan (Behavior 5) — the gesture is continuous
        // while dragging but rests the view cell-aligned on release, the same
        // contract as a tile drag/edge-resize (UXI-Workspace-8 / bug-0009); then
        // persist the final view.
        if self.workspace.workspaces[workspace_idx].desktop.pan_drag.take().is_some() {
            self.workspace.workspaces[workspace_idx].desktop.snap_camera_to_slots();
            self.save_workspace_state();
            cx.notify();
            return;
        }

        // Commit a live edge resize (spec Behavior 4b) — the clamped anchor +
        // span the preview showed become the stored placement. West/North move
        // the anchor; East/South leave it unchanged.
        if let Some(r) = self.workspace.workspaces[workspace_idx].desktop.resize.take() {
            let (slot, span) = self.desktop_resize_target(r);
            let d = &mut self.workspace.workspaces[workspace_idx].desktop;
            d.set_anchor(r.id, slot);
            d.set_span(r.id, span);
            // Rest the view cell-aligned like the resized tile (UXI-Workspace-8).
            d.snap_camera_to_slots();
            self.save_workspace_state();
            cx.notify();
            return;
        }

        let wsp = &mut self.workspace.workspaces[workspace_idx];
        let Some(d) = wsp.desktop.drag.take() else {
            return;
        };
        if d.active {
            let committed = if let Some(target) = d.target
                && wsp.desktop.slot_of(d.id) != Some(target)
            {
                // Free placement (Behavior 4): commit iff the whole rectangle
                // lands on free slots; an overlapping drop is rejected (returns
                // home, no ripple).
                wsp.desktop.free_drop(d.id, target);
                true
            } else {
                false
            };
            // Any active drag may have edge-auto-panned the view to a fractional
            // slot; rest it cell-aligned like the tile (UXI-Workspace-8) even when
            // the drop itself was rejected/no-op.
            wsp.desktop.snap_camera_to_slots();
            if committed {
                self.save_workspace_state();
            }
        }
        cx.notify();
    }

    /// Cancel an in-flight drag or resize (right-click; Esc is a follow-up).
    pub(crate) fn desktop_cancel_drag(&mut self, cx: &mut Context<Self>) {
        let workspace_idx = self.workspace.active_workspace;
        let d = &mut self.workspace.workspaces[workspace_idx].desktop;
        if d.drag.take().is_some() || d.resize.take().is_some() {
            cx.notify();
        }
    }

    /// If the workspace has more than one wsp, stack a thin horizontal workspace
    /// strip above the screen view. Single-workspace workspaces render the screen
    /// alone (no strip).
    /// Render a thin tag bar above the content when any buffers in the workspace
    /// have tags. Tags in the active workspace's tag_view get accent background.
    pub(crate) fn wrap_with_tag_bar(&self, screen_view: AnyElement) -> AnyElement {
        let all_tags = self.all_tags();
        if all_tags.is_empty() {
            return screen_view;
        }
        let tag_view = self
            .workspace
            .active_workspace()
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

    /// Inject the active workspace's rail beside the **focused leaf's** content
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
            .active_workspace()
            .map(|t| t.rail.is_none())
            .unwrap_or(true)
        {
            return content_el;
        }

        let (side, focused) = {
            let r = self
                .workspace
                .active_workspace()
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
        if let Some(wsp) = self.workspace.active_workspace() {
            wsp.focused.hash(&mut h); // focus change → re-derive
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
            .active_workspace()
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
            .active_workspace()
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

    /// Render the rail column for the active workspace (spec §9, §11–§13). Chrome
    /// styling — text is fixed at 12px and does NOT scale with `text_scale`.
    pub(crate) fn render_rail(&self, focused: bool, cx: &mut Context<Self>) -> gpui::Div {
        let rail = self
            .workspace
            .active_workspace()
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
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
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
