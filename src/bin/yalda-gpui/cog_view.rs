//! `CogView` — the cached body of a Cog explorer tile. A yux component (see
//! `yux/CLAUDE.md`): it OWNS the loaded graph list / graph bundle plus the left
//! selection and both pane scrolls, READS the global theme/fonts/zoom off the
//! root, and self-invalidates only at its mutation sites. The tile
//! (`CogTile`) holds only the cheap title/req; the heavy payload lives here so
//! the whole thing is one cached child (`cached_child`) that stays put while you
//! type elsewhere.
//!
//! Two panes: LEFT is the selector (a graph explorer first, then the chosen
//! graph's node list — j/k selects, Enter opens a graph); RIGHT is the scrollable
//! detail (graph preview, or the selected node's content, output, status,
//! status-transition timeline, and notes). Composed from `yux/detail.rs`
//! primitives (`multiline_text`, `kv_row`, `section_heading`, `note_block`).

use super::*;

/// The loaded content a Cog tile's body shows.
pub(crate) enum CogViewState {
    /// A fetch is in flight; the string is the status line.
    Loading(String),
    Error(String),
    /// The graph explorer — pick a graph. `selected` is the highlighted row.
    Graphs {
        graphs: Vec<CogGraph>,
        selected: usize,
    },
    /// A loaded graph — left is `[Overview, nodes…]`, right is the Overview
    /// (graph render + stats) or the selected node's detail. `selected` indexes
    /// into `bundle.nodes`; `overview` (top row) overrides it when set.
    Graph {
        bundle: Box<CogBundle>,
        selected: usize,
        overview: bool,
    },
}

/// Which pane the keyboard drives. `Selector` selects rows; `Detail` and
/// `Events` scroll their pane with the same j/k/arrow keys. `Events` is only
/// reachable in the `Graph` state (the explorer has no live-events pane).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CogFocus {
    Selector,
    Detail,
    Events,
}

/// The cached body view. One per Cog tile (owned by the tile via
/// `Entity<CogView>`, dropped when the tile closes — no registry).
pub(crate) struct CogView {
    state: CogViewState,
    /// Left selector scroll (follows the selection via `scroll_to_item`).
    left_scroll: ScrollHandle,
    /// Right detail scroll (`u`/`d`/PageUp/PageDown, reset on selection change).
    right_scroll: ScrollHandle,
    /// Live `cog graph watch` events, newest first (bounded). Fed by the root's
    /// drain task via `push_event`; cleared on every state change.
    events: Vec<CogEvent>,
    /// Live-events pane scroll.
    events_scroll: ScrollHandle,
    /// Monotonic event sequence (stable render key / display index).
    event_seq: u64,
    /// Which pane the keyboard drives (reset to `Selector` on state change).
    focus: CogFocus,
    /// Graph-explorer search filter (the `/` pattern) + whether it's capturing.
    graph_filter: String,
    filtering: bool,
    /// Collapsed JSON tree paths (folded rows). Keyed by a stable path id
    /// (`surface/key/idx…`); absent = expanded. Cleared on graph change.
    collapsed: std::collections::HashSet<String>,
    root: WeakEntity<YaldaGpuiView>,
    perf_label: &'static str,
}

/// Case-insensitive substring match of a graph's label + id against a filter.
fn graph_matches(g: &CogGraph, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    g.label().to_lowercase().contains(&f) || g.id.to_lowercase().contains(&f)
}

impl CogView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        CogView {
            state: CogViewState::Loading("loading graphs…".into()),
            left_scroll: ScrollHandle::new(),
            right_scroll: ScrollHandle::new(),
            events: Vec::new(),
            events_scroll: ScrollHandle::new(),
            event_seq: 0,
            focus: CogFocus::Selector,
            graph_filter: String::new(),
            filtering: false,
            collapsed: std::collections::HashSet::new(),
            root,
            perf_label: "cog",
        }
    }

    pub(crate) fn perf_label(&self) -> &'static str {
        self.perf_label
    }

    /// Replace the whole body state and reset both pane scrolls to the top.
    /// The caller notifies (mutation-site notify busts this cached view).
    pub(crate) fn set_state(&mut self, state: CogViewState) {
        self.state = state;
        self.reset_scrolls();
        self.events.clear();
        self.events_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.focus = CogFocus::Selector;
        self.graph_filter.clear();
        self.filtering = false;
        self.collapsed.clear();
    }

    // ── JSON tree folding ────────────────────────────────────────────────────

    /// Toggle a JSON tree row (fold ⇄ unfold) by its stable path id.
    pub(crate) fn toggle_json_fold(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        cx.notify();
    }

    /// Is the JSON tree row at `path` folded? (Test accessor.)
    pub(crate) fn json_folded(&self, path: &str) -> bool {
        self.collapsed.contains(path)
    }

    // ── Graph-explorer search (`/`) ──────────────────────────────────────────

    /// Is the graph-explorer search actively capturing text?
    pub(crate) fn is_filtering(&self) -> bool {
        self.filtering && self.in_graphs()
    }

    /// The current search filter text.
    pub(crate) fn filter_text(&self) -> &str {
        &self.graph_filter
    }

    /// Begin capturing a search filter (the `/` key), in the explorer only.
    pub(crate) fn start_filter(&mut self) {
        if self.in_graphs() {
            self.filtering = true;
        }
    }

    /// Append a char to the filter and reset the selection to the first match.
    pub(crate) fn filter_push(&mut self, c: char) {
        self.graph_filter.push(c);
        self.clamp_graph_selection();
    }

    /// Delete the last filter char (reset selection).
    pub(crate) fn filter_backspace(&mut self) {
        self.graph_filter.pop();
        self.clamp_graph_selection();
    }

    /// Exit search, clearing the filter.
    pub(crate) fn filter_clear(&mut self) {
        self.graph_filter.clear();
        self.filtering = false;
        self.clamp_graph_selection();
    }

    /// The full-list indices of graphs matching the current filter, in order.
    fn filtered_graph_indices(&self) -> Vec<usize> {
        match &self.state {
            CogViewState::Graphs { graphs, .. } => graphs
                .iter()
                .enumerate()
                .filter(|(_, g)| graph_matches(g, &self.graph_filter))
                .map(|(i, _)| i)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Keep the explorer selection within the filtered list.
    fn clamp_graph_selection(&mut self) {
        let n = self.filtered_graph_indices().len();
        if let CogViewState::Graphs { selected, .. } = &mut self.state {
            if n == 0 {
                *selected = 0;
            } else if *selected >= n {
                *selected = n - 1;
            }
        }
    }

    /// Replace the loaded graph's bundle IN PLACE (an auto-refresh / manual `r`
    /// while watching), KEEPING the events feed, selection (clamped), scroll, and
    /// focus — unlike [`set_state`], which resets everything for a graph change.
    /// No-op if we've since left the graph.
    pub(crate) fn update_bundle(&mut self, bundle: Box<CogBundle>) {
        if let CogViewState::Graph {
            bundle: current,
            selected,
            ..
        } = &mut self.state
        {
            let n = bundle.nodes.len();
            *current = bundle;
            if n == 0 {
                *selected = 0;
            } else if *selected >= n {
                *selected = n - 1;
            }
        }
    }

    // ── Live events ──────────────────────────────────────────────────────────

    /// Append a live event (newest first), bounded to the most recent 300.
    pub(crate) fn push_event(&mut self, raw: serde_json::Value) {
        self.event_seq += 1;
        self.events.insert(0, CogEvent { seq: self.event_seq, raw });
        self.events.truncate(300);
    }

    /// Number of buffered live events (test accessor).
    pub(crate) fn events_len(&self) -> usize {
        self.events.len()
    }

    /// The sequence of the newest (first-rendered) event (test accessor).
    pub(crate) fn newest_event_seq(&self) -> Option<u64> {
        self.events.first().map(|e| e.seq)
    }

    // ── Keyboard focus (which pane j/k drives) ───────────────────────────────

    /// Is the selector (left) pane focused?
    pub(crate) fn focused_selector(&self) -> bool {
        self.focus == CogFocus::Selector
    }

    /// Is the detail (middle) pane focused (so j/k scroll it)?
    pub(crate) fn focused_right(&self) -> bool {
        self.focus == CogFocus::Detail
    }

    /// Is the live-events (right) pane focused? Only ever true in a loaded graph.
    pub(crate) fn focused_events(&self) -> bool {
        self.focus == CogFocus::Events && self.in_graph()
    }

    /// Move keyboard focus to the detail pane.
    pub(crate) fn focus_right(&mut self) {
        self.focus = CogFocus::Detail;
    }

    /// Move keyboard focus back to the selector.
    pub(crate) fn focus_left(&mut self) {
        self.focus = CogFocus::Selector;
    }

    /// Move keyboard focus to the live-events pane (no-op outside a graph).
    pub(crate) fn focus_events(&mut self) {
        if self.in_graph() {
            self.focus = CogFocus::Events;
        }
    }

    /// Cycle focus Selector → Detail → Events → Selector (Events only in a
    /// graph). Bound to Tab.
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            CogFocus::Selector => CogFocus::Detail,
            CogFocus::Detail if self.in_graph() => CogFocus::Events,
            CogFocus::Detail => CogFocus::Selector,
            CogFocus::Events => CogFocus::Selector,
        };
    }

    fn reset_scrolls(&mut self) {
        self.left_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.right_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
    }

    /// Number of selectable rows in the active left list.
    fn len(&self) -> usize {
        match &self.state {
            CogViewState::Graphs { graphs, .. } => graphs.len(),
            CogViewState::Graph { bundle, .. } => bundle.nodes.len(),
            _ => 0,
        }
    }

    /// Move the left selection by `delta` rows, wrapping. Changing the selected
    /// node resets the right pane to the top (a fresh node starts at its header).
    pub(crate) fn select_move(&mut self, delta: i32) {
        // Explorer selection ranges over the FILTERED list.
        if matches!(self.state, CogViewState::Graphs { .. }) {
            let n = self.filtered_graph_indices().len() as i32;
            if let CogViewState::Graphs { selected, .. } = &mut self.state {
                if n == 0 {
                    return;
                }
                *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
            }
            return;
        }
        match &mut self.state {
            CogViewState::Graphs { .. } => {}
            CogViewState::Graph {
                bundle,
                selected,
                overview,
            } => {
                // Linear index over [Overview(0), node0(1), …]; total = n + 1.
                let total = bundle.nodes.len() as i32 + 1;
                let cur = if *overview { 0 } else { *selected as i32 + 1 };
                let next = (cur + delta).rem_euclid(total);
                if next == 0 {
                    *overview = true;
                } else {
                    *overview = false;
                    *selected = (next - 1) as usize;
                }
                self.right_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            }
            _ => {}
        }
    }

    /// Are we in the graph explorer (vs a loaded graph)?
    pub(crate) fn in_graphs(&self) -> bool {
        matches!(self.state, CogViewState::Graphs { .. })
    }

    /// The id of the highlighted graph in the (filtered) explorer, if any.
    pub(crate) fn selected_graph_id(&self) -> Option<String> {
        let idx = *self.filtered_graph_indices().get(self.graph_sel())?;
        match &self.state {
            CogViewState::Graphs { graphs, .. } => graphs.get(idx).map(|g| g.id.clone()),
            _ => None,
        }
    }

    /// The explorer's selection index (into the filtered list).
    fn graph_sel(&self) -> usize {
        match &self.state {
            CogViewState::Graphs { selected, .. } => *selected,
            _ => 0,
        }
    }

    /// The id of the currently-open graph (the `Graph` state), for reload.
    pub(crate) fn current_graph_id(&self) -> Option<String> {
        match &self.state {
            CogViewState::Graph { bundle, .. } => Some(bundle.graph.id.clone()),
            _ => None,
        }
    }

    /// The label of the highlighted graph (for the tile title on open).
    pub(crate) fn selected_graph_label(&self) -> Option<String> {
        let idx = *self.filtered_graph_indices().get(self.graph_sel())?;
        match &self.state {
            CogViewState::Graphs { graphs, .. } => graphs.get(idx).map(|g| g.label()),
            _ => None,
        }
    }

    // ── Mouse clicks ─────────────────────────────────────────────────────────

    /// Click a graph row in the explorer: select it and open it (like Enter).
    /// Opening needs the root (async fetch), reached via the weak handle. We read
    /// the id/label HERE (we hold `&mut self`) and hand them to the root, so the
    /// root never re-reads this entity while it is mutably borrowed.
    pub(crate) fn click_graph(&mut self, i: usize, cx: &mut Context<Self>) {
        let (id, label) = match &self.state {
            CogViewState::Graphs { graphs, .. } => {
                let Some(g) = graphs.get(i) else {
                    return;
                };
                (g.id.clone(), Some(g.label()))
            }
            _ => return,
        };
        // Set our OWN loading state here (we hold `&mut self`); the root only
        // bumps the request id + spawns the fetch, so it never re-updates this
        // entity while it is mutably borrowed by the click handler.
        self.set_state(CogViewState::Loading(format!("loading {id}…")));
        cx.notify();
        let view = cx.entity();
        if let Some(root) = self.root.upgrade() {
            root.update(cx, |r, rcx| r.cog_open_graph_for(view, id, label, rcx));
        }
    }

    /// Click a node row: select it (its detail fills the right pane) and put
    /// keyboard focus on the selector.
    pub(crate) fn click_node(&mut self, i: usize, cx: &mut Context<Self>) {
        let mut changed = false;
        if let CogViewState::Graph {
            bundle,
            selected,
            overview,
        } = &mut self.state
            && i < bundle.nodes.len()
        {
            *selected = i;
            *overview = false;
            changed = true;
        }
        if changed {
            self.right_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.focus = CogFocus::Selector;
            cx.notify();
        }
    }

    /// Click the Overview row: show the graph render + stats in the detail pane.
    pub(crate) fn click_overview(&mut self, cx: &mut Context<Self>) {
        if let CogViewState::Graph { overview, .. } = &mut self.state {
            *overview = true;
            self.right_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.focus = CogFocus::Selector;
            cx.notify();
        }
    }

    /// Click the right detail pane: move keyboard focus there (so j/k scroll it).
    pub(crate) fn click_focus_right(&mut self, cx: &mut Context<Self>) {
        self.focus_right();
        cx.notify();
    }

    /// Click the live-events pane: move keyboard focus there.
    pub(crate) fn click_focus_events(&mut self, cx: &mut Context<Self>) {
        self.focus_events();
        cx.notify();
    }

    /// Scroll the right detail pane by `down` px (negative scrolls up), clamped
    /// at the top.
    pub(crate) fn scroll_right(&mut self, down: f32) {
        let cur = self.right_scroll.offset();
        let y = (cur.y - px(down)).min(px(0.0));
        self.right_scroll.set_offset(gpui::point(cur.x, y));
    }

    /// Scroll the live-events pane by `down` px (negative scrolls up), clamped.
    pub(crate) fn scroll_events(&mut self, down: f32) {
        let cur = self.events_scroll.offset();
        let y = (cur.y - px(down)).min(px(0.0));
        self.events_scroll.set_offset(gpui::point(cur.x, y));
    }

    /// Jump the node-detail pane to section `i` (a Table-of-Contents click). The
    /// scroll container's children are `[header, toc, section0, section1, …]`, so
    /// section `i` is child `2 + i`.
    pub(crate) fn scroll_node_section(&mut self, i: usize) {
        self.right_scroll.scroll_to_item(2 + i);
    }

    /// A TOC chip click: jump to section `i` and repaint.
    pub(crate) fn click_node_section(&mut self, i: usize, cx: &mut Context<Self>) {
        self.scroll_node_section(i);
        cx.notify();
    }

    /// Is the detail pane showing the Overview (vs a node)?
    pub(crate) fn showing_overview(&self) -> bool {
        matches!(self.state, CogViewState::Graph { overview: true, .. })
    }

    // ── Test-facing accessors ────────────────────────────────────────────────

    /// The active left-list selection index (0 outside a list state).
    pub(crate) fn selected_index(&self) -> usize {
        match &self.state {
            CogViewState::Graphs { selected, .. } => *selected,
            CogViewState::Graph { selected, .. } => *selected,
            _ => 0,
        }
    }

    /// The right detail pane's current scroll offset (y, px). 0 = top.
    pub(crate) fn right_scroll_y(&self) -> f32 {
        f32::from(self.right_scroll.offset().y)
    }

    /// Number of selectable rows in the active left list.
    pub(crate) fn list_len(&self) -> usize {
        self.len()
    }

    /// Is the body a loaded graph (vs the explorer / loading / error)?
    pub(crate) fn in_graph(&self) -> bool {
        matches!(self.state, CogViewState::Graph { .. })
    }

    /// Is the body in the loading state (a fetch is in flight)?
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.state, CogViewState::Loading(_))
    }
}

impl Render for CogView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let (st, editor_bg, border) = {
            let r = root_ent.read(cx);
            let scale = r.text_scale;
            (
                DetailStyle {
                    fg: r.editor_fg(),
                    dim: nc(r.theme.agent.dim),
                    accent: nc(r.theme.agent.warm_accent),
                    err: rgb(0xff6b6b).into(),
                    mono: r.code_font.clone(),
                    prose: r.body_font.clone(),
                    base: px(14.0 * scale),
                    pt: 14.0 * scale,
                },
                r.editor_bg(),
                nc(r.theme.agent.dim),
            )
        };

        // Loading / error states fill the whole tile — no panes.
        match &self.state {
            CogViewState::Loading(msg) => {
                return single_message(msg, st.dim, &st, editor_bg).into_any_element();
            }
            CogViewState::Error(e) => {
                return cog_error_body(e, &st, editor_bg).into_any_element();
            }
            _ => {}
        }

        let left = self.left_pane(&st, border, self.focused_selector(), cx);
        let right = self.right_pane(&st, self.focused_right(), cx);

        // Top: selector | detail. Bottom (in a loaded graph): a full-width live
        // events strip across the bottom. `min_w_0` on both the column and the
        // row keeps a flex-sized ancestor (e.g. the columns workspace
        // arrangement, whose tile width is not a resolvable percentage) from
        // collapsing the detail pane to min-content (~1 char per line).
        let top = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .child(left)
            .child(right);

        let mut col = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(top);
        if self.in_graph() {
            col = col.child(self.events_pane(&st, border, self.focused_events(), cx));
        }
        col.into_any_element()
    }
}

/// A faint accent wash marking the pane that currently has keyboard focus.
fn focus_tint(st: &DetailStyle) -> Hsla {
    let mut c = st.accent;
    c.a = 0.06;
    c
}

impl CogView {
    /// The left selector pane (graph explorer or node list), scrollable and
    /// following the selection. `focused` gets a faint accent wash. Rows are
    /// clickable: a graph row opens that graph, a node row selects it.
    fn left_pane(
        &self,
        st: &DetailStyle,
        border: Hsla,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transparent: Hsla = rgba(0x00000000).into();
        let mut list = div()
            .id("cog-left")
            .flex()
            .flex_col()
            .w(px(360.0))
            .flex_none()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.left_scroll)
            .border_r_1()
            .border_color(border)
            .bg(if focused { focus_tint(st) } else { transparent })
            .px_2()
            .py_2();

        let fidx = self.filtered_graph_indices();
        match &self.state {
            CogViewState::Graphs { graphs, selected } => {
                // Header shows the search filter (the `/` pattern) when active.
                let hdr = if self.filtering || !self.graph_filter.is_empty() {
                    format!("/ {}\u{2588}  ({} match)", self.graph_filter, fidx.len())
                } else {
                    format!("Graphs ({}) · / to search", graphs.len())
                };
                list = list.child(left_header(&hdr, st));
                for (pos, &i) in fidx.iter().enumerate() {
                    let g = &graphs[i];
                    list = list.child(
                        graph_row(g, pos == *selected, st)
                            .id(SharedString::from(format!("cog-graph-{i}")))
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _ev, _w, cx| {
                                view.click_graph(i, cx);
                            })),
                    );
                }
                self.left_scroll.scroll_to_item(*selected + 1);
            }
            CogViewState::Graph {
                bundle,
                selected,
                overview,
            } => {
                let mut hdr = format!("{} · {} nodes", bundle.graph.label(), bundle.nodes.len());
                if !bundle.status.status.trim().is_empty() {
                    hdr.push_str(&format!(" · {}", bundle.status.status));
                }
                if bundle.status.has_islands() {
                    hdr.push_str(" · ⚠ islands");
                }
                list = list.child(left_header(&hdr, st));
                // The Overview row sits at the top of the list.
                list = list.child(
                    overview_row(*overview, st)
                        .id(SharedString::new_static("cog-overview-row"))
                        .cursor_pointer()
                        .on_click(cx.listener(|view, _ev, _w, cx| view.click_overview(cx))),
                );
                for (i, n) in bundle.nodes.iter().enumerate() {
                    let eff = bundle.effective_status(n);
                    let sel = !*overview && i == *selected;
                    list = list.child(
                        node_row(n, eff, sel, st)
                            .id(SharedString::from(format!("cog-node-{i}")))
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _ev, _w, cx| {
                                view.click_node(i, cx);
                            })),
                    );
                }
                // Children: [header, overview, node0, …]; reveal the active item.
                let active = if *overview { 1 } else { 2 + *selected };
                self.left_scroll.scroll_to_item(active);
            }
            _ => {}
        }
        list.into_any_element()
    }

    /// The right detail pane (graph preview or node detail), scrollable.
    /// `focused` gets a faint accent wash (keyboard scrolls it); clicking it
    /// moves keyboard focus here.
    fn right_pane(&self, st: &DetailStyle, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let transparent: Hsla = rgba(0x00000000).into();
        let mut scroll = div()
            .id("cog-right")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .gap_2()
            .overflow_y_scroll()
            .track_scroll(&self.right_scroll)
            .bg(if focused { focus_tint(st) } else { transparent })
            .on_click(cx.listener(|view, _ev, _w, cx| view.click_focus_right(cx)))
            .px_4()
            .py_3();

        match &self.state {
            // A selected node: header + Table of Contents + ordered sections as
            // DIRECT children so a TOC click's `scroll_to_item` lines up.
            CogViewState::Graph {
                bundle,
                selected,
                overview: false,
            } => match bundle.nodes.get(*selected) {
                Some(n) => {
                    scroll = scroll.child(node_header(bundle, n, st));
                    let sections = self.node_sections(bundle, n, st, cx);
                    // Table of Contents: a chip per section (jumps on click).
                    let mut toc = div().flex().flex_row().flex_wrap().w_full().gap_1().pb_1();
                    for (i, (title, _)) in sections.iter().enumerate() {
                        let label = title.clone();
                        toc = toc.child(
                            div()
                                .id(SharedString::from(format!("cog-toc-{i}")))
                                .flex_none()
                                .cursor_pointer()
                                .px_2()
                                .rounded_md()
                                .bg(st.accent.opacity(0.12))
                                .text_color(st.accent)
                                .font_family(st.mono.clone())
                                .text_size(px(st.pt * 0.82))
                                .child(SharedString::from(label))
                                .on_click(cx.listener(move |v, _ev, _w, cx| {
                                    v.click_node_section(i, cx);
                                })),
                        );
                    }
                    scroll = scroll.child(toc);
                    for (i, (_, el)) in sections.into_iter().enumerate() {
                        scroll = scroll.child(probe_bounds_dyn(
                            format!("cog-sec-{i}"),
                            el.into_any_element(),
                        ));
                    }
                }
                None => {
                    scroll =
                        scroll.child(single_inner("Select a node on the left.", st.dim, st));
                }
            },
            // The Overview: graph render + stats.
            CogViewState::Graph { bundle, .. } => {
                scroll = scroll.child(probe_bounds(
                    "cog-right-content",
                    overview_body(bundle, st).into_any_element(),
                ));
            }
            CogViewState::Graphs { graphs, selected } => {
                let body = match graphs.get(*selected) {
                    Some(g) => graph_preview(g, st).into_any_element(),
                    None => {
                        single_inner("Select a graph on the left.", st.dim, st).into_any_element()
                    }
                };
                scroll = scroll.child(probe_bounds("cog-right-content", body));
            }
            _ => scroll = scroll.child(single_inner("", st.dim, st)),
        }
        probe_bounds("cog-right", scroll.into_any_element())
    }

    /// The live-events strip: a full-width panel ACROSS THE BOTTOM of the tile, a
    /// scrollable newest-first feed of `cog graph watch` events, each an
    /// aesthetically-formatted, syntax-highlighted JSON card laid out left→right.
    /// `focused` gets a faint accent wash; clicking it moves keyboard focus here.
    fn events_pane(
        &self,
        st: &DetailStyle,
        border: Hsla,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transparent: Hsla = rgba(0x00000000).into();
        // Newest-first vertical feed of full-width cards, scrolling within the strip.
        let mut feed = div()
            .id("cog-events-feed")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.events_scroll)
            .gap_2();
        if self.events.is_empty() {
            feed = feed.child(dim_line("Waiting for live events…", st));
        } else {
            for ev in &self.events {
                feed = feed.child(self.event_card(ev, st, cx));
            }
        }

        let strip = div()
            .id("cog-events")
            .flex()
            .flex_col()
            .w_full()
            .h(px(360.0))
            .flex_none()
            .border_t_1()
            .border_color(border)
            .bg(if focused { focus_tint(st) } else { transparent })
            .on_click(cx.listener(|v, _ev, _w, cx| v.click_focus_events(cx)))
            .px_3()
            .py_2()
            .gap_1()
            .child(left_header(&format!("Live events ({})", self.events.len()), st))
            .child(feed);
        probe_bounds("cog-events", strip.into_any_element())
    }
}

impl CogView {
    /// One live-event card: a `#seq` header above the event's JSON, rendered as a
    /// foldable tree-table.
    fn event_card(&self, ev: &CogEvent, st: &DetailStyle, cx: &mut Context<Self>) -> gpui::Div {
        card(st)
            .child(
                div()
                    .w_full()
                    .text_color(st.dim)
                    .font_family(st.mono.clone())
                    .text_size(px(st.pt * 0.8))
                    .child(SharedString::from(format!("#{}", ev.seq))),
            )
            .child(self.json_tree(&format!("ev:{}", ev.seq), &ev.raw, st, cx))
    }

    /// Render a JSON value as a foldable tree-table: one row per key, nested
    /// objects/arrays are foldable (▸/▾) so a big payload can be collapsed. Fold
    /// state lives in `self.collapsed`, keyed by `prefix` + json path.
    fn json_tree(
        &self,
        prefix: &str,
        value: &serde_json::Value,
        st: &DetailStyle,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        match value {
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                self.json_children(prefix, value, 0, st, cx, &mut rows);
            }
            other => rows.push(json_kv_row(0, "", other, st).into_any_element()),
        }
        let mut col = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .rounded_md()
            .border_1()
            .border_color(card_border(st))
            .bg(code_bg(st))
            .py_1();
        for r in rows {
            col = col.child(r);
        }
        col
    }

    /// Emit rows for each child of an object/array (skips the container's own
    /// row — the caller emitted it, or it's the tree root).
    fn json_children(
        &self,
        prefix: &str,
        value: &serde_json::Value,
        depth: usize,
        st: &DetailStyle,
        cx: &mut Context<Self>,
        out: &mut Vec<gpui::AnyElement>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    self.json_node(&format!("{prefix}/{k}"), k, v, depth, st, cx, out);
                }
            }
            serde_json::Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    let label = format!("[{i}]");
                    self.json_node(&format!("{prefix}/{i}"), &label, v, depth, st, cx, out);
                }
            }
            _ => {}
        }
    }

    /// Emit the row(s) for one keyed value: a leaf key/value row, or a foldable
    /// sub-table header + (if unfolded) its child rows.
    fn json_node(
        &self,
        path: &str,
        key: &str,
        value: &serde_json::Value,
        depth: usize,
        st: &DetailStyle,
        cx: &mut Context<Self>,
        out: &mut Vec<gpui::AnyElement>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                let folded = self.collapsed.contains(path);
                out.push(self.json_header_row(path, key, folded, map.len(), depth, st, cx));
                if !folded {
                    self.json_children(path, value, depth + 1, st, cx, out);
                }
            }
            serde_json::Value::Array(a) => {
                let folded = self.collapsed.contains(path);
                out.push(self.json_header_row(path, key, folded, a.len(), depth, st, cx));
                if !folded {
                    self.json_children(path, value, depth + 1, st, cx, out);
                }
            }
            other => out.push(json_kv_row(depth, key, other, st).into_any_element()),
        }
    }

    /// A foldable sub-table HEADER row for a dict/array key: caret + bold key +
    /// a dim item count. No `{}`/`[]` braces — the key IS the sub-table's title.
    fn json_header_row(
        &self,
        path: &str,
        key: &str,
        folded: bool,
        count: usize,
        depth: usize,
        st: &DetailStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = path.to_string();
        let caret = if folded { "▸" } else { "▾" };
        let hov = st.dim.opacity(0.12);
        json_row(depth, st)
            .id(SharedString::from(format!("cog-json{path}")))
            .cursor_pointer()
            .rounded_sm()
            .bg(st.dim.opacity(0.06))
            .hover(move |s| s.bg(hov))
            .on_click(cx.listener(move |v, _ev, _w, cx| v.toggle_json_fold(p.clone(), cx)))
            .child(caret_col(caret, st.dim))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(st.fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::from(key.to_string())),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(st.dim)
                    .text_size(px(st.pt * 0.78))
                    .child(SharedString::from(count.to_string())),
            )
            .into_any_element()
    }
}

/// A JSON table row: monospace, indented by `depth`. `items_start` keeps a
/// wrapped value column aligned to the top of its key.
fn json_row(depth: usize, st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .w_full()
        .min_w_0()
        .pl(px(6.0 + depth as f32 * 14.0))
        .pr_2()
        .py(px(2.0))
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.9))
}

/// The fixed-width caret column (▸/▾ for a header, blank for a leaf).
fn caret_col(caret: &'static str, color: Hsla) -> gpui::Div {
    div()
        .flex_none()
        .w(px(14.0))
        .text_color(color)
        .font_weight(FontWeight::BOLD)
        .child(SharedString::new_static(caret))
}

/// A JSON scalar's display text + a HIGH-CONTRAST colour — the editor foreground
/// for strings, the theme accent for numbers/bools, muted for null. (No washed
/// mid-tone greens/purples, which read as low-contrast on light themes.)
fn json_scalar(value: &serde_json::Value, st: &DetailStyle) -> (String, Hsla) {
    // A calm slate blue for numbers/bools — a light, clean accent that reads on
    // both the warm-linen light theme and dark themes (no brown accent tint).
    let numeric: Hsla = rgb(0x4a7a9e).into();
    match value {
        serde_json::Value::String(s) => (s.replace('\n', " "), st.fg),
        serde_json::Value::Number(n) => (n.to_string(), numeric),
        serde_json::Value::Bool(b) => (b.to_string(), numeric),
        serde_json::Value::Null => ("null".to_string(), st.dim),
        other => (other.to_string(), st.fg),
    }
}

/// A leaf key/value TABLE row: blank caret column, the bold key column, then the
/// value column (no colon, no braces). An empty key (a root scalar) omits the
/// key column.
fn json_kv_row(depth: usize, key: &str, value: &serde_json::Value, st: &DetailStyle) -> gpui::Div {
    let (text, color) = json_scalar(value, st);
    let mut row = json_row(depth, st).child(div().flex_none().w(px(14.0)));
    if !key.is_empty() {
        row = row.child(
            div()
                .flex_none()
                .min_w(px(96.0))
                .pr_2()
                .text_color(st.fg)
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(key.to_string())),
        );
    }
    row.child(
        div()
            .flex_1()
            .min_w_0()
            .text_color(color)
            .child(SharedString::from(text)),
    )
}

// ── Status colour ────────────────────────────────────────────────────────────

fn status_color(eff: EffStatus, st: &DetailStyle) -> Hsla {
    match eff {
        EffStatus::Done => rgb(0x5fb35f).into(),
        EffStatus::Ready => st.accent,
        EffStatus::Claimed => rgb(0xd7a44a).into(),
        EffStatus::Blocked => st.dim,
        EffStatus::Failed => st.err,
        EffStatus::Abandoned => rgb(0x9b8aa8).into(),
    }
}

/// A small `[status]` badge in the status's colour.
fn status_badge(eff: EffStatus, st: &DetailStyle) -> gpui::Div {
    div()
        .flex_none()
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.85))
        .text_color(status_color(eff, st))
        .child(SharedString::from(format!("[{}]", eff.label())))
}

fn nav_sel_bg(st: &DetailStyle) -> Hsla {
    let mut bg = st.accent;
    bg.a = 0.16;
    bg
}

// ── Left-list rows ───────────────────────────────────────────────────────────

fn left_header(text: &str, st: &DetailStyle) -> gpui::Div {
    div()
        .w_full()
        .pb_1()
        .px_1()
        .text_color(st.dim)
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.8))
        .child(SharedString::from(text.to_string()))
}

// ── Cards (each "update" — a note or a transition — is its own boxed card) ────

/// A faint fill for a card's interior.
fn card_bg(st: &DetailStyle) -> Hsla {
    let mut c = st.accent;
    c.a = 0.05;
    c
}

/// A subtle hairline border for a card / code block.
fn card_border(st: &DetailStyle) -> Hsla {
    st.dim.opacity(0.35)
}

/// A stronger fill for a monospace JSON code block.
fn code_bg(st: &DetailStyle) -> Hsla {
    st.dim.opacity(0.12)
}

/// An empty stylish card container: rounded, hairline border, faint fill.
fn card(st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(card_border(st))
        .bg(card_bg(st))
}

/// A single-line label that truncates with an ellipsis rather than wrapping —
/// keeps the narrow left list tidy for long graph/node names.
fn truncating_label(text: String, color: Hsla, size: gpui::Pixels, st: &DetailStyle) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(size)
        .text_color(color)
        .font_family(st.mono.clone())
        .child(SharedString::from(text))
}

fn graph_row(g: &CogGraph, is_sel: bool, st: &DetailStyle) -> gpui::Div {
    let transparent: Hsla = rgba(0x00000000).into();
    let name_size = px(st.pt * 0.88);
    let mut marks = String::new();
    if g.sealed {
        marks.push('🔒');
    }
    if g.prototype {
        marks.push('⚗');
    }
    div()
        .flex()
        .flex_col()
        .w_full()
        .px_2()
        .py_1()
        .bg(if is_sel { nav_sel_bg(st) } else { transparent })
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .items_center()
                .w_full()
                .child(truncating_label(
                    g.label(),
                    if is_sel { st.accent } else { st.fg },
                    name_size,
                    st,
                ))
                .child(
                    div()
                        .flex_none()
                        .font_family(st.mono.clone())
                        .text_color(st.dim)
                        .child(SharedString::from(marks)),
                ),
        )
        .child(
            div()
                .w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(st.mono.clone())
                .text_color(st.dim)
                .text_size(px(st.pt * 0.76))
                .child(SharedString::from(g.id.clone())),
        )
}

fn node_row(n: &CogNode, eff: EffStatus, is_sel: bool, st: &DetailStyle) -> gpui::Div {
    let transparent: Hsla = rgba(0x00000000).into();
    let name = if n.name.trim().is_empty() {
        n.id.clone()
    } else {
        n.name.clone()
    };
    div()
        .flex()
        .flex_row()
        .gap_2()
        .items_center()
        .w_full()
        .px_2()
        .py_1()
        .bg(if is_sel { nav_sel_bg(st) } else { transparent })
        .child(truncating_label(
            name,
            if is_sel { st.accent } else { st.fg },
            px(st.pt * 0.88),
            st,
        ))
        .child(status_badge(eff, st))
}

// ── Right-pane bodies ────────────────────────────────────────────────────────

/// The "Overview" row at the top of the node list (selected ⇒ accent wash).
fn overview_row(is_sel: bool, st: &DetailStyle) -> gpui::Div {
    let transparent: Hsla = rgba(0x00000000).into();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .px_2()
        .py_1()
        .bg(if is_sel { nav_sel_bg(st) } else { transparent })
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.88))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if is_sel { st.accent } else { st.fg })
                .child(SharedString::new_static("▤ Overview")),
        )
}

/// The Overview detail: the graph's ASCII DAG render plus aggregate stats
/// (node counts by status + claimed→done completion min/max/avg).
fn overview_body(bundle: &CogBundle, st: &DetailStyle) -> gpui::Div {
    let s = bundle.stats();
    let mut col = div().flex().flex_col().w_full().gap_2();

    col = col.child(
        div()
            .w_full()
            .text_color(st.fg)
            .font_family(st.prose.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(st.pt * 1.45))
            .child(SharedString::from(bundle.graph.label())),
    );

    // Stats.
    col = col.child(section_heading("Stats", st));
    let mut meta = div().flex().flex_col().gap_1().w_full();
    meta = meta.child(kv_row("Nodes", s.total.to_string(), st));
    meta = meta.child(kv_row(
        "By status",
        format!(
            "{} done · {} claimed · {} open · {} failed",
            s.done, s.claimed, s.open, s.failed
        ),
        st,
    ));
    let dur = |v: Option<i64>| v.map(fmt_duration_ns).unwrap_or_else(|| "—".into());
    meta = meta.child(kv_row(
        "Completion",
        format!(
            "{} completed · quickest {} · longest {} · avg {}",
            s.completed,
            dur(s.quickest_ns),
            dur(s.longest_ns),
            dur(s.average_ns)
        ),
        st,
    ));
    col = col.child(meta);

    // Graph render (ASCII DAG).
    col = col.child(section_heading("Graph", st));
    let render = if bundle.render.trim().is_empty() {
        "(no render)".to_string()
    } else {
        bundle.render.clone()
    };
    col = col.child(
        div()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(card_border(st))
            .bg(code_bg(st))
            .child(multiline_text(&render, st.fg, &st.mono, px(st.pt * 0.9))),
    );
    col
}

fn graph_preview(g: &CogGraph, st: &DetailStyle) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap_2();
    col = col.child(
        div()
            .w_full()
            .text_color(st.fg)
            .font_family(st.prose.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(st.pt * 1.45))
            .child(SharedString::from(g.label())),
    );
    col = col.child(
        div()
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.9))
            .child(SharedString::from(g.id.clone())),
    );

    let mut meta = div().flex().flex_col().gap_1().w_full().pt_1();
    meta = meta.child(kv_row(
        "Sealed",
        if g.sealed { "yes" } else { "no" }.into(),
        st,
    ));
    if g.prototype {
        meta = meta.child(kv_row("Prototype", "yes".into(), st));
    }
    if !g.omega.trim().is_empty() {
        meta = meta.child(kv_row("Omega", g.omega.clone(), st));
    }
    col = col.child(meta);

    if !g.description.trim().is_empty() {
        col = col.child(section_heading("Description", st));
        col = col.child(multiline_text(&g.description, st.fg, &st.prose, st.base));
    }
    col = col.child(
        div()
            .pt_2()
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.85))
            .child(SharedString::new_static(
                "Enter opens this graph · j/k select · Esc back",
            )),
    );
    col
}

/// The node detail header: name (bold) + id + status badge.
fn node_header(bundle: &CogBundle, n: &CogNode, st: &DetailStyle) -> gpui::Div {
    let eff = bundle.effective_status(n);
    let name = if n.name.trim().is_empty() {
        n.id.clone()
    } else {
        n.name.clone()
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap_1()
        .pb_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(st.fg)
                        .font_family(st.prose.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(st.pt * 1.45))
                        .child(SharedString::from(name)),
                )
                .child(status_badge(eff, st)),
        )
        .child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(px(st.pt * 0.9))
                .child(SharedString::from(n.id.clone())),
        )
}

/// The node detail sections, in TOC order — **State transitions is always
/// first**, then Content, Output (when present), Notes. Each carries its title
/// (for the Table of Contents) and a self-contained heading + body block so a
/// TOC jump reveals the heading.
/// The ordered node-detail section kinds — **State transitions first**, then
/// Content, Output (when present), Notes. The single source of truth for section
/// order (guarded by a test); `CogView::node_sections` renders in this order.
pub(crate) fn node_section_titles(n: &CogNode) -> Vec<&'static str> {
    let mut v = vec!["Status transitions", "Content"];
    if n.output.as_ref().filter(|o| !o.is_null()).is_some() {
        v.push("Output");
    }
    v.push("Notes");
    v
}

impl CogView {
    /// The node-detail sections in [`node_section_titles`] order, each with its
    /// display title (for the Table of Contents) + a heading + body block. JSON
    /// bodies (Content / Output) are foldable tree-tables.
    fn node_sections(
        &self,
        bundle: &CogBundle,
        n: &CogNode,
        st: &DetailStyle,
        cx: &mut Context<Self>,
    ) -> Vec<(String, gpui::Div)> {
        let section = |title: &str, body: gpui::Div| {
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap_2()
                .child(section_heading(title, st))
                .child(body)
        };
        let mut out: Vec<(String, gpui::Div)> = Vec::new();
        for kind in node_section_titles(n) {
            match kind {
                "Status transitions" => {
                    let empty: &[CogLogEntry] = &[];
                    let log = bundle.logs.get(&n.id).map(|l| l.as_slice()).unwrap_or(empty);
                    let mut transitions: Vec<&CogLogEntry> =
                        log.iter().filter(|e| e.kind == "status_changed").collect();
                    transitions.sort_by_key(|e| e.seq);
                    let title = format!("Status transitions ({})", transitions.len());
                    let body = if transitions.is_empty() {
                        div().child(dim_line("No transitions.", st))
                    } else {
                        let mut b = div().flex().flex_col().w_full().gap_2();
                        for e in transitions {
                            let to = e
                                .data
                                .get("to")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string();
                            b = b.child(transition_card(&to, &e.actor, fmt_epoch_ns(e.at), st));
                        }
                        b
                    };
                    out.push((title.clone(), section(&title, body)));
                }
                "Content" => {
                    let body = self.json_body(&format!("n:{}/content", n.id), &n.content, st, cx);
                    out.push(("Content".into(), section("Content", body)));
                }
                "Output" => {
                    if let Some(o) = n.output.as_ref().filter(|v| !v.is_null()) {
                        let body = self.json_body(&format!("n:{}/output", n.id), o, st, cx);
                        out.push(("Output".into(), section("Output", body)));
                    }
                }
                _ => {
                    // Notes.
                    let empty_notes: &[CogNote] = &[];
                    let notes = bundle
                        .notes
                        .get(&n.id)
                        .map(|v| v.as_slice())
                        .unwrap_or(empty_notes);
                    let title = format!("Notes ({})", notes.len());
                    let body = if notes.is_empty() {
                        div().child(dim_line("No notes.", st))
                    } else {
                        let mut b = div().flex().flex_col().w_full().gap_2();
                        for note in notes {
                            b = b.child(note_card(note, st));
                        }
                        b
                    };
                    out.push((title.clone(), section(&title, body)));
                }
            }
        }
        out
    }

    /// A JSON body: a foldable tree-table when structured, else bare prose.
    fn json_body(
        &self,
        prefix: &str,
        v: &serde_json::Value,
        st: &DetailStyle,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if json_is_structured(v) {
            div().w_full().child(self.json_tree(prefix, v, st, cx))
        } else {
            div()
                .w_full()
                .child(multiline_text(&json_prose(v), st.fg, &st.prose, st.base))
        }
    }
}

/// A status transition as a stylish card: `→ done` (in the status colour), the
/// actor, and the timestamp.
fn transition_card(to: &str, actor: &str, when: String, st: &DetailStyle) -> gpui::Div {
    let eff = crate::parse_eff_status(to);
    card(st).child(
        div()
            .flex()
            .flex_row()
            .gap_2()
            .items_baseline()
            .w_full()
            .font_family(st.mono.clone())
            .text_size(st.base)
            .child(
                div()
                    .flex_none()
                    .w(px(110.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(status_color(eff, st))
                    .child(SharedString::from(format!("→ {to}"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(st.fg)
                    .child(SharedString::from(actor.to_string())),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(st.dim)
                    .text_size(px(st.pt * 0.82))
                    .child(SharedString::from(when)),
            ),
    )
}

/// One note as a stylish card: a header row (topic badge · author, then the
/// timestamp) above the note prose.
fn note_card(note: &CogNote, st: &DetailStyle) -> gpui::Div {
    let author = if note.actor.trim().is_empty() {
        "—".to_string()
    } else {
        note.actor.clone()
    };
    let when = fmt_epoch_ns(note.at);
    let topic = note.topic.clone().filter(|t| !t.is_empty());

    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.82));
    let mut left = div().flex().flex_row().items_center().gap_2().min_w_0();
    if let Some(t) = topic {
        left = left.child(
            div()
                .flex_none()
                .px_1()
                .rounded_md()
                .bg(st.accent.opacity(0.16))
                .text_color(st.accent)
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(t)),
        );
    }
    left = left.child(div().flex_none().text_color(st.dim).child(SharedString::from(author)));
    head = head
        .child(left)
        .child(div().flex_none().text_color(st.dim).child(SharedString::from(when)));

    card(st)
        .child(head)
        .child(multiline_text(&note.summary(), st.fg, &st.prose, st.base))
}

// ── Full-tile message bodies ─────────────────────────────────────────────────

fn dim_line(text: &str, st: &DetailStyle) -> gpui::Div {
    div()
        .text_color(st.dim)
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(SharedString::from(text.to_string()))
}

fn single_inner(text: &str, color: Hsla, st: &DetailStyle) -> gpui::Div {
    div()
        .text_color(color)
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(SharedString::from(text.to_string()))
}

fn single_message(msg: &str, color: Hsla, st: &DetailStyle, bg: Hsla) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(bg)
        .px_4()
        .py_3()
        .child(single_inner(msg, color, st))
}

fn cog_error_body(e: &str, st: &DetailStyle, bg: Hsla) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .size_full()
        .bg(bg)
        .px_4()
        .py_3()
        .child(
            div()
                .text_color(st.err)
                .font_family(st.mono.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(st.base)
                .child(SharedString::new_static("error")),
        )
        .child(multiline_text(e, st.err, &st.prose, st.base))
}
