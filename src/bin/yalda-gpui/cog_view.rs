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

/// The cached body view. One per Cog tile (owned by the tile via
/// `Entity<CogView>`, dropped when the tile closes — no registry).
pub(crate) struct CogView {
    state: CogViewState,
    /// Left selector scroll (follows the selection via `scroll_to_item`).
    left_scroll: ScrollHandle,
    /// Right detail scroll (`u`/`d`/PageUp/PageDown, reset on selection change).
    right_scroll: ScrollHandle,
    root: WeakEntity<YaldaGpuiView>,
    perf_label: &'static str,
}

impl CogView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        CogView {
            state: CogViewState::Loading("loading graphs…".into()),
            left_scroll: ScrollHandle::new(),
            right_scroll: ScrollHandle::new(),
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

    /// Scroll the right detail pane by `down` px (negative scrolls up), clamped
    /// at the top.
    pub(crate) fn scroll_right(&mut self, down: f32) {
        let cur = self.right_scroll.offset();
        let y = (cur.y - px(down)).min(px(0.0));
        self.right_scroll.set_offset(gpui::point(cur.x, y));
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

        let left = self.left_pane(&st, border);
        let right = self.right_pane(&st);

        div()
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(left)
            .child(right)
            .into_any_element()
    }
}

impl CogView {
    /// The left selector pane (graph explorer or node list), scrollable and
    /// following the selection.
    fn left_pane(&self, st: &DetailStyle, border: Hsla) -> impl IntoElement {
        let mut list = div()
            .id("cog-left")
            .flex()
            .flex_col()
            .w(px(300.0))
            .flex_none()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.left_scroll)
            .border_r_1()
            .border_color(border)
            .px_2()
            .py_2();

        match &self.state {
            CogViewState::Graphs { graphs, selected } => {
                list = list.child(left_header(&format!("Graphs ({})", graphs.len()), st));
                for (i, g) in graphs.iter().enumerate() {
                    list = list.child(graph_row(g, i == *selected, st));
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
                    list = list.child(node_row(n, eff, i == *selected, st));
                }
                self.left_scroll.scroll_to_item(*selected + 1);
            }
            _ => {}
        }
        list
    }

    /// The right detail pane (graph preview or node detail), scrollable.
    fn right_pane(&self, st: &DetailStyle) -> impl IntoElement {
        let body: AnyElement = match &self.state {
            CogViewState::Graphs { graphs, selected } => match graphs.get(*selected) {
                Some(g) => graph_preview(g, st).into_any_element(),
                None => single_inner("Select a graph on the left.", st.dim, st).into_any_element(),
            },
            CogViewState::Graph { bundle, selected } => match bundle.nodes.get(*selected) {
                Some(n) => node_detail(bundle, n, st).into_any_element(),
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
            .px_4()
            .py_3()
            .child(probe_bounds("cog-right-content", body));
        probe_bounds("cog-right", scroll.into_any_element())
    }
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
        .text_size(px(st.pt * 0.85))
        .child(SharedString::from(text.to_string()))
}

fn graph_row(g: &CogGraph, is_sel: bool, st: &DetailStyle) -> gpui::Div {
    let transparent: Hsla = rgba(0x00000000).into();
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
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .items_center()
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if is_sel { st.accent } else { st.fg })
                        .child(SharedString::from(g.label())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(st.dim)
                        .child(SharedString::from(marks)),
                ),
        )
        .child(
            div()
                .w_full()
                .text_color(st.dim)
                .text_size(px(st.pt * 0.8))
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
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if is_sel { st.accent } else { st.fg })
                .child(SharedString::from(name)),
        )
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

fn node_detail(bundle: &CogBundle, n: &CogNode, st: &DetailStyle) -> gpui::Div {
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

    // Content.
    col = col.child(section_heading("Content", st));
    col = col.child(multiline_text(
        &json_prose(&n.content),
        st.fg,
        &st.prose,
        st.base,
    ));

    // Output (if any).
    if let Some(out) = n.output.as_ref().filter(|v| !v.is_null()) {
        col = col.child(section_heading("Output", st));
        col = col.child(multiline_text(&json_prose(out), st.fg, &st.prose, st.base));
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
            col = col.child(transition_row(&to, &e.actor, fmt_epoch_ns(e.at), st));
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
            let author = if note.actor.trim().is_empty() {
                "—".to_string()
            } else {
                note.actor.clone()
            };
            let when = fmt_epoch_ns(note.at);
            let topic = note
                .topic
                .clone()
                .filter(|t| !t.is_empty())
                .map(|t| format!("[{t}] "))
                .unwrap_or_default();
            col = col.child(note_block(
                author,
                when,
                &format!("{topic}{}", note.summary()),
                st,
            ));
        }
    }
    col
}

/// One status-transition row: `→ done   actor   2026-08-17 12:34`.
fn transition_row(to: &str, actor: &str, when: String, st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .items_baseline()
        .w_full()
        .px_1()
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(
            div()
                .flex_none()
                .w(px(120.0))
                .text_color(st.accent)
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
                .text_size(px(st.pt * 0.85))
                .child(SharedString::from(when)),
        )
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
