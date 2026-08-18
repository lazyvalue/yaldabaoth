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
    /// A loaded graph — left node list, right node detail. `selected` indexes
    /// into `bundle.nodes`.
    Graph {
        bundle: Box<CogBundle>,
        selected: usize,
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
    root: WeakEntity<YaldaGpuiView>,
    perf_label: &'static str,
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
        let n = self.len() as i32;
        if n == 0 {
            return;
        }
        match &mut self.state {
            CogViewState::Graphs { selected, .. } => {
                *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
            }
            CogViewState::Graph { selected, .. } => {
                *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
                self.right_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            }
            _ => {}
        }
    }

    /// Are we in the graph explorer (vs a loaded graph)?
    pub(crate) fn in_graphs(&self) -> bool {
        matches!(self.state, CogViewState::Graphs { .. })
    }

    /// The id of the highlighted graph in the explorer, if any.
    pub(crate) fn selected_graph_id(&self) -> Option<String> {
        match &self.state {
            CogViewState::Graphs { graphs, selected } => graphs.get(*selected).map(|g| g.id.clone()),
            _ => None,
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
        match &self.state {
            CogViewState::Graphs { graphs, selected } => graphs.get(*selected).map(|g| g.label()),
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
        if let CogViewState::Graph { bundle, selected } = &mut self.state
            && i < bundle.nodes.len()
        {
            *selected = i;
            changed = true;
        }
        if changed {
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
        let (st, editor_bg, border, syntect) = {
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
                r.theme.name.syntect_theme(),
            )
        };
        let hl = json_highlighter(syntect);

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
        let right = self.right_pane(&st, self.focused_right(), &hl, cx);

        let mut row = div()
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(left)
            .child(right);
        // The live-events pane is the third column, present only while a graph
        // is open (the explorer has nothing to watch).
        if self.in_graph() {
            row = row.child(self.events_pane(&st, border, self.focused_events(), &hl, cx));
        }
        row.into_any_element()
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

        match &self.state {
            CogViewState::Graphs { graphs, selected } => {
                list = list.child(left_header(&format!("Graphs ({})", graphs.len()), st));
                for (i, g) in graphs.iter().enumerate() {
                    list = list.child(
                        graph_row(g, i == *selected, st)
                            .id(SharedString::from(format!("cog-graph-{i}")))
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _ev, _w, cx| {
                                view.click_graph(i, cx);
                            })),
                    );
                }
                self.left_scroll.scroll_to_item(*selected + 1);
            }
            CogViewState::Graph { bundle, selected } => {
                let mut hdr = format!("{} · {} nodes", bundle.graph.label(), bundle.nodes.len());
                if !bundle.status.status.trim().is_empty() {
                    hdr.push_str(&format!(" · {}", bundle.status.status));
                }
                if bundle.status.has_islands() {
                    hdr.push_str(" · ⚠ islands");
                }
                list = list.child(left_header(&hdr, st));
                for (i, n) in bundle.nodes.iter().enumerate() {
                    let eff = bundle.effective_status(n);
                    list = list.child(
                        node_row(n, eff, i == *selected, st)
                            .id(SharedString::from(format!("cog-node-{i}")))
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _ev, _w, cx| {
                                view.click_node(i, cx);
                            })),
                    );
                }
                self.left_scroll.scroll_to_item(*selected + 1);
            }
            _ => {}
        }
        list.into_any_element()
    }

    /// The right detail pane (graph preview or node detail), scrollable.
    /// `focused` gets a faint accent wash (keyboard scrolls it); clicking it
    /// moves keyboard focus here.
    fn right_pane(
        &self,
        st: &DetailStyle,
        focused: bool,
        hl: &yalda::highlight::Highlighter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transparent: Hsla = rgba(0x00000000).into();
        let body: AnyElement = match &self.state {
            CogViewState::Graphs { graphs, selected } => match graphs.get(*selected) {
                Some(g) => graph_preview(g, st).into_any_element(),
                None => single_inner("Select a graph on the left.", st.dim, st).into_any_element(),
            },
            CogViewState::Graph { bundle, selected } => match bundle.nodes.get(*selected) {
                Some(n) => node_detail(bundle, n, hl, st).into_any_element(),
                None => single_inner("Select a node on the left.", st.dim, st).into_any_element(),
            },
            _ => single_inner("", st.dim, st).into_any_element(),
        };

        // Probe the viewport container and the inner content separately so a
        // test can assert the content overflows the viewport (genuinely
        // scrollable, non-vacuous — see `cog_detail_paints_and_overflows`).
        let scroll = div()
            .id("cog-right")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.right_scroll)
            .bg(if focused { focus_tint(st) } else { transparent })
            .on_click(cx.listener(|view, _ev, _w, cx| view.click_focus_right(cx)))
            .px_4()
            .py_3()
            .child(probe_bounds("cog-right-content", body));
        probe_bounds("cog-right", scroll.into_any_element())
    }

    /// The live-events pane: a scrollable, newest-first feed of `cog graph watch`
    /// events, each an aesthetically-formatted, syntax-highlighted JSON card.
    /// `focused` gets a faint accent wash; clicking it moves keyboard focus here.
    fn events_pane(
        &self,
        st: &DetailStyle,
        border: Hsla,
        focused: bool,
        hl: &yalda::highlight::Highlighter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transparent: Hsla = rgba(0x00000000).into();
        let mut list = div()
            .id("cog-events")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.events_scroll)
            .border_l_1()
            .border_color(border)
            .bg(if focused { focus_tint(st) } else { transparent })
            .on_click(cx.listener(|v, _ev, _w, cx| v.click_focus_events(cx)))
            .px_3()
            .py_3()
            .gap_2()
            .child(left_header(&format!("Live events ({})", self.events.len()), st));

        if self.events.is_empty() {
            list = list.child(dim_line("Waiting for live events…", st));
        } else {
            for ev in &self.events {
                list = list.child(event_card(ev, hl, st));
            }
        }
        probe_bounds("cog-events", list.into_any_element())
    }
}

/// One live-event card: a small `#seq` header above the event's pretty-printed,
/// syntax-highlighted JSON.
fn event_card(ev: &CogEvent, hl: &yalda::highlight::Highlighter, st: &DetailStyle) -> gpui::Div {
    card(st)
        .child(
            div()
                .w_full()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(px(st.pt * 0.8))
                .child(SharedString::from(format!("#{}", ev.seq))),
        )
        .child(highlighted_json(&json_prose(&ev.raw), hl, st))
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

/// A syntect JSON highlighter, cached per syntect-theme name for the whole
/// tile — `Highlighter::with_syntect_theme` loads the full default `SyntaxSet`,
/// which is far too expensive to rebuild every render / every JSON block.
fn json_highlighter(syntect_theme: &'static str) -> std::rc::Rc<yalda::highlight::Highlighter> {
    thread_local! {
        static CACHE: std::cell::RefCell<
            Option<(&'static str, std::rc::Rc<yalda::highlight::Highlighter>)>,
        > = const { std::cell::RefCell::new(None) };
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((name, hl)) = c.as_ref()
            && *name == syntect_theme
        {
            return hl.clone();
        }
        let hl = std::rc::Rc::new(yalda::highlight::Highlighter::with_syntect_theme(syntect_theme));
        *c = Some((syntect_theme, hl.clone()));
        hl
    })
}

/// Render pretty-printed JSON with syntect syntax highlighting — one flex row
/// per line, one coloured monospace span per token (keys, strings, numbers,
/// literals, punctuation each get syntect's theme colour). Falls back to plain
/// monospace text if the highlighter can't parse it.
fn highlighted_json(pretty: &str, hl: &yalda::highlight::Highlighter, st: &DetailStyle) -> gpui::Div {
    let size = px(st.pt * 0.92);
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .font_family(st.mono.clone())
        .text_size(size);
    match hl.highlight("json", pretty, yalda::style::Style::default()) {
        Some(lines) => {
            for line in lines {
                let mut row = div().flex().flex_row().flex_wrap().w_full();
                for span in &line.spans {
                    let color = span
                        .style
                        .fg
                        .map(|c| ncolor_to_hsla(c, 0xcccccc))
                        .unwrap_or(st.fg);
                    row = row.child(
                        div()
                            .text_color(color)
                            .child(SharedString::from(span.text.clone())),
                    );
                }
                col = col.child(row);
            }
        }
        None => {
            col = col.child(multiline_text(pretty, st.fg, &st.mono, size));
        }
    }
    col
}

/// Render JSON: a bare string as prose; any structure as a pretty-printed,
/// syntax-highlighted monospace code block (rounded, tinted). Pretty-printing +
/// highlighting are the `/new-ux` requirement — content/output are shown
/// indented and coloured, not as one run-on line.
fn json_block(v: &serde_json::Value, hl: &yalda::highlight::Highlighter, st: &DetailStyle) -> gpui::Div {
    if json_is_structured(v) {
        div()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(card_border(st))
            .bg(code_bg(st))
            .child(highlighted_json(&json_prose(v), hl, st))
    } else {
        // Bare string / scalar → prose.
        div()
            .w_full()
            .child(multiline_text(&json_prose(v), st.fg, &st.prose, st.base))
    }
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

fn node_detail(
    bundle: &CogBundle,
    n: &CogNode,
    hl: &yalda::highlight::Highlighter,
    st: &DetailStyle,
) -> gpui::Div {
    let eff = bundle.effective_status(n);
    let mut col = div().flex().flex_col().w_full().gap_2();

    // Header: name (bold) + id + status badge.
    let name = if n.name.trim().is_empty() {
        n.id.clone()
    } else {
        n.name.clone()
    };
    col = col.child(
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
    );
    col = col.child(
        div()
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.9))
            .child(SharedString::from(n.id.clone())),
    );

    // Content (pretty-printed, syntax-highlighted JSON when structured).
    col = col.child(section_heading("Content", st));
    col = col.child(json_block(&n.content, hl, st));

    // Output (if any).
    if let Some(out) = n.output.as_ref().filter(|v| !v.is_null()) {
        col = col.child(section_heading("Output", st));
        col = col.child(json_block(out, hl, st));
    }

    // Status transitions (from the node log).
    let empty: &[CogLogEntry] = &[];
    let log = bundle.logs.get(&n.id).map(|l| l.as_slice()).unwrap_or(empty);
    let mut transitions: Vec<&CogLogEntry> =
        log.iter().filter(|e| e.kind == "status_changed").collect();
    transitions.sort_by_key(|e| e.seq);
    col = col.child(section_heading(
        &format!("Status transitions ({})", transitions.len()),
        st,
    ));
    if transitions.is_empty() {
        col = col.child(dim_line("No transitions.", st));
    } else {
        for e in transitions {
            let to = e
                .data
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            col = col.child(transition_card(&to, &e.actor, fmt_epoch_ns(e.at), st));
        }
    }

    // Notes.
    let empty_notes: &[CogNote] = &[];
    let notes = bundle
        .notes
        .get(&n.id)
        .map(|v| v.as_slice())
        .unwrap_or(empty_notes);
    col = col.child(section_heading(&format!("Notes ({})", notes.len()), st));
    if notes.is_empty() {
        col = col.child(dim_line("No notes.", st));
    } else {
        for note in notes {
            col = col.child(note_card(note, st));
        }
    }
    col
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
