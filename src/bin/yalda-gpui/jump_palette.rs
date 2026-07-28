//! The **jump palette** (`UXI-JumpPanel-9`) — `Cmd-P`'s type-to-filter dialog
//! over the jump panel's ordinary navigable set: every non-ephemeral workspace
//! and every non-archived agent session (`Local` ∪ `Roster`).
//!
//! The palette is a pure alternate *input* onto that list. It builds its
//! candidates by walking the sidebar's `All` projection, independent of which
//! filtered tab is currently visible, and activates through the sidebar's
//! existing dispatchers (`select_workspace` / `jump_to_agent`), so 1:1 binding,
//! ephemeral-workspace teardown (ADR-0021) and read-marking stay owned where
//! they already are. No new jump semantics live here.
//!
//! The ranking is deliberately pure (`fuzzy_score` / `rank_palette_items` are
//! free functions over plain data) so "the top row is the best match" is a
//! headless assertion, not a paint one.

use super::*;

/// What a palette row jumps to. Mirrors the two things the sidebar can navigate
/// to; **projects are absent by design** — a project is a container, not a view
/// target (clicking one opens a menu, `UXI-JumpPanel-8`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaletteTarget {
    /// GLOBAL index into `workspace.workspaces` (the same index
    /// `select_workspace` takes). Only non-ephemeral workspaces become items.
    Workspace(usize),
    /// An agent session, keyed exactly as the sidebar keys it.
    Agent(JumpTarget),
}

/// One palette candidate: what it points at, what you read, and what you type
/// against (`label`).
pub(crate) struct PaletteItem {
    pub(crate) target: PaletteTarget,
    /// The matched text — the workspace's or session's name, exactly as the
    /// sidebar shows it.
    pub(crate) label: String,
    /// Dim secondary text: the owning project's name, or the cwd for a session
    /// no project roots. Disambiguates same-named rows; NOT matched against
    /// (what you type matches what you read as the row's name).
    pub(crate) detail: String,
    /// Agent session (`✦`) vs workspace (`⊞`).
    pub(crate) is_agent: bool,
    /// Agent activity, for the `✦` color. `None` for workspaces.
    pub(crate) status: Option<AgentDotStatus>,
    /// "You are here" — the active workspace, or the session bound to the
    /// focused tile.
    pub(crate) active: bool,
}

/// Overlay state: what you've typed and which row is highlighted. `selected`
/// indexes into the RANKED list (`rank_palette_items`), not the item list.
pub(crate) struct JumpPaletteOverlay {
    pub(crate) query: String,
    pub(crate) selected: usize,
}

/// Rows drawn at once; the window scrolls to keep `selected` visible.
const PALETTE_VISIBLE_ROWS: usize = 12;

/// Score `query` against `text` as a fuzzy subsequence match; `None` when the
/// query's characters don't appear in order (i.e. not a candidate at all).
///
/// The scale rewards, in rough order of weight: an **exact** hit (+100), a whole
/// **prefix** hit (+40), each character landing at a **word start** (+12, where a
/// word starts after any non-alphanumeric — space, `-`, `_`, `/`, `.`), each
/// character **contiguous** with the previous match (+8), and each matched
/// character at all (+4). A mild length penalty (`len/4`) breaks ties toward the
/// shorter, more specific label. Case-insensitive.
///
/// The walk is greedy-leftmost, not an optimal alignment: it takes the first
/// occurrence of each query char. That's predictable and O(n), and matches what
/// the eye expects for the short labels this palette ranks.
///
/// An **empty query scores 0 for everything**, which is what keeps the empty
/// palette in panel order (a stable sort of equal keys is the identity).
pub(crate) fn fuzzy_score(text: &str, query: &str) -> Option<i32> {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut score = 0i32;
    let mut ti = 0usize;
    let mut prev: Option<usize> = None;
    for &qc in &q {
        let mut hit = None;
        while ti < t.len() {
            let at = ti;
            ti += 1;
            if t[at] == qc {
                hit = Some(at);
                break;
            }
        }
        let idx = hit?;
        score += 4;
        if idx > 0 && prev == Some(idx - 1) {
            score += 8;
        }
        if idx == 0 || !t[idx - 1].is_alphanumeric() {
            score += 12;
        }
        prev = Some(idx);
    }
    let tl: String = t.iter().collect();
    let ql: String = q.iter().collect();
    if tl == ql {
        score += 100;
    } else if tl.starts_with(&ql) {
        score += 40;
    }
    score -= t.len() as i32 / 4;
    Some(score)
}

/// The ranked, filtered view of `items` for `query`: indices into `items`, best
/// match first. An empty query keeps every item in **panel order**; a non-empty
/// query drops non-matches and orders by `fuzzy_score` descending, with panel
/// order as the tiebreak (the sort is stable).
pub(crate) fn rank_palette_items(items: &[PaletteItem], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(&it.label, query).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(i, _)| i).collect()
}

impl YaldaGpuiView {
    /// Every ordinary-navigation candidate, in the **All tab's presentation
    /// order**: each project section's workspaces, then Working / Waiting /
    /// Unavailable sessions, then the trailing unfiled groups. A selected
    /// filtered tab never changes `Cmd-P` candidates.
    pub(crate) fn jump_palette_items(&self, cx: &gpui::App) -> Vec<PaletteItem> {
        // Cmd-P is always the ordinary navigation roster, independent of which
        // filtered tab happens to be visible in each project. Archived rows are
        // excluded by the forced All projection.
        let (sections, unfiled) =
            self.jump_panel_sections_with_tab(cx, Some(crate::JumpAgentTab::All));
        let (active_local, active_sid) = self.jump_active_session();
        let mut items = Vec::new();
        let push_session = |items: &mut Vec<PaletteItem>, row: &AgentRow, detail: &str| {
            items.push(PaletteItem {
                target: PaletteTarget::Agent(row.target.clone()),
                label: row.label.clone(),
                detail: detail.to_string(),
                is_agent: true,
                status: Some(row.dot_status()),
                active: jump_target_is_active(&row.target, active_local, active_sid.as_deref()),
            });
        };
        for s in sections {
            for (idx, label, is_active) in s.workspaces {
                items.push(PaletteItem {
                    target: PaletteTarget::Workspace(idx),
                    label,
                    detail: s.name.clone(),
                    is_agent: false,
                    status: None,
                    active: is_active,
                });
            }
            for (_, rows) in agent_row_groups_for_tab(s.sessions, JumpAgentTab::All) {
                for (_, row) in rows {
                    push_session(&mut items, &row, &s.name);
                }
            }
        }
        for (cwd_label, group) in &unfiled {
            for (_, row) in group {
                push_session(&mut items, row, cwd_label);
            }
        }
        items
    }

    /// The palette's current ranked candidates, as `(item, is_selected)` pairs in
    /// display order. The single place the key handler, the render, and the tests
    /// all derive "what's on screen" from.
    pub(crate) fn jump_palette_ranked(&self, cx: &gpui::App) -> (Vec<PaletteItem>, Vec<usize>) {
        let items = self.jump_palette_items(cx);
        let query = self.jump_palette_ref().map(|p| p.query.clone()).unwrap_or_default();
        let ranked = rank_palette_items(&items, &query);
        (items, ranked)
    }

    /// `Cmd-P`. No-op when ANY overlay is already open (including this one) —
    /// the single `ActiveOverlay` slot is never clobbered, and re-pressing the
    /// chord is not a toggle (`UXI-JumpPanel-9`).
    pub(crate) fn open_jump_palette(
        &mut self,
        _: &OpenJumpPalette,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_jump_palette_impl(cx);
    }

    pub(crate) fn open_jump_palette_impl(&mut self, cx: &mut Context<Self>) {
        if self.has_overlay() {
            return;
        }
        // A fresh palette clears any lingering toast (same idiom as the pickers).
        self.transient_status = None;
        self.open_overlay(ActiveOverlay::JumpPalette(JumpPaletteOverlay {
            query: String::new(),
            selected: 0,
        }));
        cx.notify();
    }

    pub(crate) fn handle_jump_palette_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let len = self.jump_palette_ranked(cx).1.len();
        match press.key {
            Key::Esc => {
                self.clear_overlay();
                cx.notify();
            }
            Key::Enter => self.activate_jump_palette_selection(cx),
            Key::Down => {
                if let Some(p) = self.jump_palette_mut()
                    && len > 0
                {
                    p.selected = (p.selected + 1) % len;
                }
                cx.notify();
            }
            Key::Up => {
                if let Some(p) = self.jump_palette_mut()
                    && len > 0
                {
                    p.selected = (p.selected + len - 1) % len;
                }
                cx.notify();
            }
            Key::Backspace => {
                if let Some(p) = self.jump_palette_mut() {
                    p.query.pop();
                    // Editing the query re-ranks, so the highlight returns to the
                    // (new) best match rather than a stale row index.
                    p.selected = 0;
                }
                cx.notify();
            }
            // A modified chord (Cmd-P itself, Cmd-anything) must never type its
            // bare letter into the query — the overlay captures keys before
            // action dispatch, so this is where that chord dies.
            Key::Char(c)
                if !press.modifiers.contains(KMods::PLATFORM)
                    && !press.modifiers.contains(KMods::CONTROL)
                    && !press.modifiers.contains(KMods::ALT) =>
            {
                if let Some(p) = self.jump_palette_mut() {
                    p.query.push(c);
                    p.selected = 0;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Activate the highlighted row: dismiss the palette FIRST (so the jump's own
    /// `has_overlay()` guards pass and focus settles into the destination), then
    /// dispatch through the sidebar's existing activators. A no-match query has
    /// nothing highlighted — that's a **no-op that leaves the palette open**.
    pub(crate) fn activate_jump_palette_selection(&mut self, cx: &mut Context<Self>) {
        let (items, ranked) = self.jump_palette_ranked(cx);
        let selected = match self.jump_palette_ref() {
            Some(p) => p.selected,
            None => return,
        };
        let Some(&idx) = ranked.get(selected) else {
            return;
        };
        let target = items[idx].target.clone();
        self.clear_overlay();
        match target {
            PaletteTarget::Workspace(i) => self.select_workspace(i, cx),
            PaletteTarget::Agent(t) => self.jump_to_agent(t, cx),
        }
        cx.notify();
    }

    pub(crate) fn render_jump_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let st = DetailStyle {
            fg: self.editor_fg(),
            dim: nc(self.theme.agent.dim),
            accent: nc(self.theme.agent.warm_accent),
            err: nc(self.theme.agent.jump_header),
            mono: self.code_font.clone(),
            prose: self.body_font.clone(),
            base: px(13.0),
            pt: 13.0,
        };
        let ov = &self.theme.overlay;
        let popup_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let label_fg: Hsla = nc(ov.label);
        let input_fg: Hsla = nc(ov.input);
        let active_accent = nc(self.theme.agent.frozen_bar);
        let mut sel_bg = active_accent;
        sel_bg.a = 0.15;
        let working_orange: Hsla = nc(self.theme.agent.jump_working);
        let ready: Hsla = nc(self.theme.agent.tool_completed);

        let (items, ranked) = self.jump_palette_ranked(cx);
        let (query, selected) = match self.jump_palette_ref() {
            Some(p) => (p.query.clone(), p.selected),
            None => (String::new(), 0),
        };

        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .text_size(px(11.0))
            .child(SharedString::new_static("JUMP TO"));

        let input_row = div()
            .px_4()
            .py_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(st.mono.clone())
            .child(SharedString::from(format!("{query}\u{2588}")));

        let mut list = div().flex().flex_col().w_full();
        if ranked.is_empty() {
            list = list.child(
                div()
                    .px_4()
                    .py_2()
                    .text_color(st.dim)
                    .font_family(st.mono.clone())
                    .text_size(px(12.0))
                    .child(SharedString::new_static("No matches")),
            );
        } else {
            // Scroll the fixed-height window so the highlight is always on screen.
            let start = selected
                .saturating_sub(PALETTE_VISIBLE_ROWS - 1)
                .min(ranked.len().saturating_sub(PALETTE_VISIBLE_ROWS.min(ranked.len())));
            for (row_n, &idx) in ranked
                .iter()
                .enumerate()
                .skip(start)
                .take(PALETTE_VISIBLE_ROWS)
            {
                let it = &items[idx];
                let is_sel = row_n == selected;
                let badge_color = match it.status {
                    Some(AgentDotStatus::Working) => working_orange,
                    Some(AgentDotStatus::WaitingForYou) => ready,
                    _ => st.dim,
                };
                let target = it.target.clone();
                let mut row = div()
                    .id(SharedString::from(format!("jump-palette-row-{row_n}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .px_4()
                    .py_1()
                    .hover(|s| s.bg(sel_bg))
                    .child(
                        div()
                            .w(px(16.0))
                            .flex_none()
                            .text_color(badge_color)
                            .child(SharedString::new_static(if it.is_agent {
                                "✦"
                            } else {
                                "⊞"
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.0))
                            .text_color(if is_sel || it.active { active_accent } else { st.fg })
                            .child(SharedString::from(it.label.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(st.dim)
                            .child(SharedString::from(it.detail.clone())),
                    );
                if is_sel {
                    row = row.bg(sel_bg);
                }
                row = row
                    .on_click(cx.listener({
                        let target = target.clone();
                        move |this, _ev, _w, cx| {
                            this.clear_overlay();
                            match target.clone() {
                                PaletteTarget::Workspace(i) => this.select_workspace(i, cx),
                                PaletteTarget::Agent(t) => this.jump_to_agent(t, cx),
                            }
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                        if *hovered && let Some(p) = this.jump_palette_mut() {
                            p.selected = row_n;
                            cx.notify();
                        }
                    }));
                list = list.child(row);
            }
        }

        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static("↑↓:select  enter:jump  esc:cancel"));

        probe_bounds(
            "jump-palette",
            div()
                .absolute()
                .top(px(80.0))
                .left_0()
                .right_0()
                .flex()
                .flex_row()
                .justify_center()
                .child(
                    div()
                        .w(px(560.0))
                        .bg(popup_bg)
                        .border_2()
                        .border_color(popup_border)
                        .flex()
                        .flex_col()
                        .child(header)
                        .child(input_row)
                        .child(list)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
