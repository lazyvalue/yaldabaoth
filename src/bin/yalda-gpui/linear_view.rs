//! `LinearView` — the cached body of a Linear tile. A yux component (see
//! `yux/CLAUDE.md`): it OWNS the loaded issue/project payload and the scroll
//! state, READS the global theme/fonts/zoom off the root view, and
//! self-invalidates only when its payload (or a pushed global) changes. The
//! tile's input line lives on `LinearTile` (not here), so typing re-renders the
//! input row while this body stays cached — the whole point of the split.
//!
//! Domain glue lives here; the building blocks it composes from
//! (`DetailStyle`, `multiline_text`, `kv_row`, `section_heading`, `note_block`,
//! `fmt_iso_datetime`) are reusable yux primitives in `yux/detail.rs`.

use super::*;

/// The loaded content a Linear tile's body shows.
pub(crate) enum LinearViewState {
    /// No query yet — show the prompt.
    Empty,
    /// A fetch is in flight; the string is the status line.
    Loading(String),
    Issue(Box<IssueDetail>),
    Project(Box<ProjectDetail>),
    /// A name search matched multiple projects — choose one (↑/↓ + Enter, or a
    /// number key). `selected` is the highlighted row.
    ProjectPicker {
        candidates: Vec<ProjectCandidate>,
        selected: usize,
    },
    Error(String),
}

/// The cached body view. One per Linear tile (owned by the tile via
/// `Entity<LinearView>`, so it drops when the tile closes — no registry).
pub(crate) struct LinearView {
    state: LinearViewState,
    scroll: ScrollHandle,
    root: WeakEntity<YaldaGpuiView>,
    perf_label: &'static str,
}

impl LinearView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        LinearView {
            state: LinearViewState::Empty,
            scroll: ScrollHandle::new(),
            root,
            perf_label: "linear",
        }
    }

    /// Replace the payload and reset scroll to the top. The caller notifies
    /// (mutation-site notify — the only thing that busts this cached view).
    pub(crate) fn set_state(&mut self, state: LinearViewState) {
        self.state = state;
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
    }

    /// Scroll the body by `down` px (negative scrolls up), clamped at the top.
    pub(crate) fn scroll_by(&mut self, down: f32) {
        let cur = self.scroll.offset();
        let y = (cur.y - px(down)).min(px(0.0));
        self.scroll.set_offset(gpui::point(cur.x, y));
    }

    pub(crate) fn perf_label(&self) -> &'static str {
        self.perf_label
    }

    /// Is the body currently a project picker? (Drives key routing.)
    pub(crate) fn is_picker(&self) -> bool {
        matches!(self.state, LinearViewState::ProjectPicker { .. })
    }

    /// Move the picker selection by `delta` rows, wrapping. No-op off-picker.
    pub(crate) fn picker_move(&mut self, delta: i32) {
        if let LinearViewState::ProjectPicker {
            candidates,
            selected,
        } = &mut self.state
            && !candidates.is_empty()
        {
            let n = candidates.len() as i32;
            *selected = (*selected as i32 + delta).rem_euclid(n) as usize;
        }
    }

    /// Set the picker selection to `idx` if in range. No-op off-picker.
    pub(crate) fn picker_set(&mut self, idx: usize) {
        if let LinearViewState::ProjectPicker {
            candidates,
            selected,
        } = &mut self.state
            && idx < candidates.len()
        {
            *selected = idx;
        }
    }

    /// The currently-highlighted candidate, if the body is a picker.
    pub(crate) fn selected_candidate(&self) -> Option<ProjectCandidate> {
        match &self.state {
            LinearViewState::ProjectPicker {
                candidates,
                selected,
            } => candidates.get(*selected).cloned(),
            _ => None,
        }
    }
}

impl Render for LinearView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        // The root owns the GLOBAL theme/fonts/zoom (not our state). If it's
        // gone (teardown), render an empty sized placeholder — never panic.
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        // Snapshot root-owned inputs into owned locals, releasing the borrow.
        let (st, editor_bg) = {
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
            )
        };

        let body: AnyElement = match &self.state {
            LinearViewState::Empty => linear_empty_body(&st).into_any_element(),
            LinearViewState::Loading(msg) => div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from(msg.clone()))
                .into_any_element(),
            LinearViewState::Error(e) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_color(st.err)
                        .font_family(st.mono.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(st.base)
                        .child(SharedString::from("error")),
                )
                .child(multiline_text(e, st.err, &st.prose, st.base))
                .into_any_element(),
            LinearViewState::Issue(i) => linear_issue_body(i, &st).into_any_element(),
            LinearViewState::Project(p) => linear_project_body(p, &st).into_any_element(),
            LinearViewState::ProjectPicker {
                candidates,
                selected,
            } => linear_picker_body(candidates, *selected, &st).into_any_element(),
        };

        let scroll = self.scroll.clone();
        div()
            .id("linear-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_4()
            .py_3()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(body)
            .into_any_element()
    }
}

// ── Domain body builders (Linear-specific; composed from yux primitives) ─────

fn linear_empty_body(st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .text_color(st.dim)
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(SharedString::from(
            "Type an issue identifier (e.g. FUL-420) or a project name, then press Enter.",
        ))
        .child(SharedString::from(
            "↑/↓ or PageUp/PageDown scroll · Esc clears the input.",
        ))
}

fn linear_picker_body(candidates: &[ProjectCandidate], selected: usize, st: &DetailStyle) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap_1();
    col = col.child(
        div()
            .w_full()
            .pb_1()
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.9))
            .child(SharedString::from(format!(
                "{} projects — ↑/↓ to choose, Enter to open (or press its number) · Esc to edit",
                candidates.len()
            ))),
    );
    let mut sel_bg = st.accent;
    sel_bg.a = 0.16;
    let transparent: Hsla = rgba(0x00000000).into();
    for (i, c) in candidates.iter().enumerate() {
        let is_sel = i == selected;
        let name = c.name.clone().unwrap_or_else(|| "(unnamed)".into());
        let state = c.state.clone().filter(|s| !s.is_empty()).unwrap_or_default();
        col = col.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .items_center()
                .w_full()
                .px_2()
                .py_1()
                .bg(if is_sel { sel_bg } else { transparent })
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(
                    div()
                        .w(px(28.0))
                        .flex_none()
                        .text_color(if is_sel { st.accent } else { st.dim })
                        .child(SharedString::from(format!("{}.", i + 1))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(st.fg)
                        .child(SharedString::from(name)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(st.dim)
                        .child(SharedString::from(state)),
                ),
        );
    }
    col
}

fn linear_issue_body(i: &IssueDetail, st: &DetailStyle) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap_2();

    let ident = i.identifier.clone().unwrap_or_default();
    let title = i.title.clone().unwrap_or_else(|| "(untitled)".into());
    col = col.child(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .text_color(st.dim)
                    .font_family(st.mono.clone())
                    .text_size(px(st.pt * 0.9))
                    .child(SharedString::from(ident)),
            )
            .child(
                div()
                    .w_full()
                    .text_color(st.fg)
                    .font_family(st.prose.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(st.pt * 1.45))
                    .child(SharedString::from(title)),
            ),
    );

    let mut meta = div().flex().flex_col().gap_1().w_full().pt_1();
    let status = i
        .state
        .as_ref()
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| "—".into());
    meta = meta.child(kv_row("Status", status, st));
    meta = meta.child(kv_row(
        "Assignee",
        i.assignee
            .as_ref()
            .map(|a| a.label())
            .unwrap_or_else(|| "Unassigned".into()),
        st,
    ));
    if let Some(p) = i.priority_label.clone().filter(|s| !s.is_empty()) {
        meta = meta.child(kv_row("Priority", p, st));
    }
    if let Some(pr) = i.project.as_ref().and_then(|p| p.name.clone()) {
        meta = meta.child(kv_row("Project", pr, st));
    }
    if let Some(m) = i.milestone.as_ref().and_then(|m| m.name.clone()) {
        meta = meta.child(kv_row("Milestone", m, st));
    }
    if let Some(labels) = i.labels.as_ref() {
        let names: Vec<String> = labels.nodes.iter().filter_map(|l| l.name.clone()).collect();
        if !names.is_empty() {
            meta = meta.child(kv_row("Labels", names.join(", "), st));
        }
    }
    if let Some(u) = i.url.clone() {
        meta = meta.child(kv_row("URL", u, st));
    }
    col = col.child(meta);

    col = col.child(section_heading("Description", st));
    col = col.child(multiline_text(
        i.description.as_deref().unwrap_or(""),
        st.fg,
        &st.prose,
        st.base,
    ));

    let empty: &[Comment] = &[];
    let comments = i.comments.as_ref().map(|c| c.nodes.as_slice()).unwrap_or(empty);
    col = col.child(section_heading(&format!("Comments ({})", comments.len()), st));
    if comments.is_empty() {
        col = col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No comments.")),
        );
    } else {
        for c in comments {
            let author = c
                .user
                .as_ref()
                .map(|u| u.label())
                .unwrap_or_else(|| "—".into());
            let when = fmt_iso_datetime(&c.created_at);
            col = col.child(note_block(author, when, c.body.as_deref().unwrap_or(""), st));
        }
    }
    col
}

fn linear_project_body(p: &ProjectDetail, st: &DetailStyle) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap_2();

    let name = p.name.clone().unwrap_or_else(|| "(unnamed project)".into());
    col = col.child(
        div()
            .w_full()
            .text_color(st.fg)
            .font_family(st.prose.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(st.pt * 1.45))
            .child(SharedString::from(name)),
    );

    let mut meta = div().flex().flex_col().gap_1().w_full().pt_1();
    if let Some(s) = p.state.clone().filter(|s| !s.is_empty()) {
        meta = meta.child(kv_row("Status", s, st));
    }
    if let Some(l) = p.lead.as_ref() {
        meta = meta.child(kv_row("Lead", l.label(), st));
    }
    if let Some(t) = p.target_date.clone().filter(|s| !s.is_empty()) {
        meta = meta.child(kv_row("Target", t, st));
    }
    if let Some(u) = p.url.clone() {
        meta = meta.child(kv_row("URL", u, st));
    }
    col = col.child(meta);

    let overview = p
        .description
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| p.content.clone())
        .filter(|s| !s.trim().is_empty());
    if let Some(d) = overview {
        col = col.child(section_heading("Overview", st));
        col = col.child(multiline_text(&d, st.fg, &st.prose, st.base));
    }

    let empty_ms: &[Milestone] = &[];
    let ms = p
        .milestones
        .as_ref()
        .map(|m| m.nodes.as_slice())
        .unwrap_or(empty_ms);
    col = col.child(section_heading(&format!("Milestones ({})", ms.len()), st));
    if ms.is_empty() {
        col = col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No milestones.")),
        );
    } else {
        for m in ms {
            let nm = m.name.clone().unwrap_or_else(|| "—".into());
            let hdr = match m.target_date.clone().filter(|s| !s.is_empty()) {
                Some(w) => format!("{nm}  ·  {w}"),
                None => nm,
            };
            let mut block = div().flex().flex_col().w_full().gap_1().pb_2().child(
                div()
                    .text_color(st.fg)
                    .font_family(st.mono.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(st.base)
                    .child(SharedString::from(hdr)),
            );
            if let Some(d) = m.description.clone().filter(|s| !s.trim().is_empty()) {
                block = block.child(multiline_text(&d, st.dim, &st.prose, st.base));
            }
            col = col.child(block);
        }
    }

    let empty_ir: &[IssueRef] = &[];
    let issues = p
        .issues
        .as_ref()
        .map(|i| i.nodes.as_slice())
        .unwrap_or(empty_ir);
    col = col.child(section_heading(&format!("Issues ({})", issues.len()), st));
    if issues.is_empty() {
        col = col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No issues.")),
        );
    } else {
        for it in issues {
            let id = it.identifier.clone().unwrap_or_default();
            let state = it
                .state
                .as_ref()
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| "—".into());
            let title = it.title.clone().unwrap_or_default();
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_start()
                    .w_full()
                    .text_size(st.base)
                    .font_family(st.mono.clone())
                    .child(
                        div()
                            .w(px(84.0))
                            .flex_none()
                            .text_color(st.accent)
                            .child(SharedString::from(id)),
                    )
                    .child(
                        div()
                            .w(px(110.0))
                            .flex_none()
                            .text_color(st.dim)
                            .child(SharedString::from(state)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(st.fg)
                            .child(SharedString::from(title)),
                    ),
            );
        }
    }

    let empty_up: &[ProjectUpdate] = &[];
    let updates = p
        .updates
        .as_ref()
        .map(|u| u.nodes.as_slice())
        .unwrap_or(empty_up);
    col = col.child(section_heading(&format!("Status updates ({})", updates.len()), st));
    if updates.is_empty() {
        col = col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No status updates.")),
        );
    } else {
        for u in updates {
            let author = u
                .user
                .as_ref()
                .map(|x| x.label())
                .unwrap_or_else(|| "—".into());
            let when = fmt_iso_datetime(&u.created_at);
            col = col.child(note_block(author, when, u.body.as_deref().unwrap_or(""), st));
        }
    }
    col
}
