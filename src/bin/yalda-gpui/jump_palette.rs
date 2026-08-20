//! The **jump palette** (`UXI-JumpPanel-9`) — `Cmd-P`'s type-to-filter dialog
//! over the jump panel's ordinary navigable set: durable workspaces and the
//! stable tiles they own, followed by the **Detached** tile collection.
//!
//! The palette is a pure alternate *input* onto that list. It builds its
//! candidates directly from the ownership model, independent of which filtered
//! tab is visible, and activates through the shared stable-tile dispatcher.
//!
//! The ranking is deliberately pure (`fuzzy_score` / `rank_palette_items` are
//! free functions over plain data) so "the top row is the best match" is a
//! headless assertion, not a paint one.

use super::*;

/// What a palette row jumps to. Projects remain containers rather than targets;
/// a tile id names the exact stateful shell object in either ownership domain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaletteTarget {
    /// GLOBAL index into `workspace.workspaces` (the same index
    /// `select_workspace` takes). Only non-ephemeral workspaces become items.
    Workspace(usize),
    Tile(workspace::WindowId),
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
    fn palette_tile_item(
        &self,
        window: &workspace::Window<App>,
        detail: String,
        cx: &gpui::App,
    ) -> Option<PaletteItem> {
        let (label, is_agent, status, archived) = match &window.content {
            App::Agent(tile) => {
                let local = tile.session();
                let remembered = tile.remembered_sid(|id| self.sessions.sid_of(id).cloned());
                let roster = remembered
                    .as_ref()
                    .and_then(|sid| self.agent_roster.get(sid.as_str()));
                let label = local
                    .and_then(|id| self.sessions.get(id))
                    .map(|session| session.read(cx).label.clone())
                    .or_else(|| roster.map(|info| info.label.clone()))
                    .unwrap_or_else(|| "Claude".to_string());
                let status = local
                    .and_then(|id| self.sessions.get(id))
                    .map(|session| {
                        if session.read(cx).state.turn_phase.is_awaiting() {
                            AgentDotStatus::Working
                        } else {
                            AgentDotStatus::WaitingForYou
                        }
                    })
                    .or_else(|| {
                        roster.map(|info| {
                            if !info.connected {
                                AgentDotStatus::Neutral
                            } else if info.busy {
                                AgentDotStatus::Working
                            } else {
                                AgentDotStatus::WaitingForYou
                            }
                        })
                    })
                    .unwrap_or(AgentDotStatus::Neutral);
                let archived = remembered
                    .as_ref()
                    .is_some_and(|sid| self.jump_archived_sessions.contains(sid.as_str()));
                (label, true, Some(status), archived)
            }
            content => (
                Self::desktop_tile_title(&self.sessions, content, cx),
                false,
                None,
                false,
            ),
        };
        (!archived).then(|| PaletteItem {
            target: PaletteTarget::Tile(window.id()),
            label,
            detail,
            is_agent,
            status,
            active: self.workspace.focused_window_id() == Some(window.id()),
        })
    }

    /// Every ordinary-navigation candidate in ownership order: each workspace
    /// folder followed by its visible and hidden tiles, then every Detached tile. A selected jump
    /// panel activity tab never changes `Cmd-P` candidates.
    pub(crate) fn jump_palette_items(&self, cx: &gpui::App) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        for (idx, wsp) in self.workspace.workspaces.iter().enumerate() {
            let project = self.projects.name_of(wsp.project()).to_string();
            let workspace_label = wsp.display_label().to_string();
            items.push(PaletteItem {
                target: PaletteTarget::Workspace(idx),
                label: workspace_label.clone(),
                detail: project.clone(),
                is_agent: false,
                status: None,
                active: self.workspace.presented_tile().is_none()
                    && self.workspace.active_workspace == idx,
            });
            wsp.layout.for_each_leaf(&mut |window| {
                if let Some(item) =
                    self.palette_tile_item(window, format!("{project} · {workspace_label}"), cx)
                {
                    items.push(item);
                }
            });
            for hidden in &wsp.hidden_tiles {
                if let Some(item) = self.palette_tile_item(
                    &hidden.window,
                    format!("{project} · {workspace_label} · Hidden"),
                    cx,
                ) {
                    items.push(item);
                }
            }
        }
        for tile in &self.workspace.detached_tiles {
            let project = self.projects.name_of(tile.project());
            if let Some(item) =
                self.palette_tile_item(&tile.window, format!("{project} · Detached"), cx)
            {
                items.push(item);
            }
        }
        items
    }

    /// The production dispatcher shared by Cmd-P and the jump panel. Agent
    /// tiles retain their attach/read semantics; every other tile is a direct
    /// stable-id focus. No branch changes membership.
    pub(crate) fn jump_to_tile(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        enum AgentDestination {
            Local(SessionId),
            Roster(String),
            Plain,
        }
        let destination = self
            .workspace
            .tile(id)
            .and_then(|window| match &window.content {
                App::Agent(tile) => Some(
                    tile.session()
                        .map(AgentDestination::Local)
                        .or_else(|| {
                            tile.remembered_sid(|local| self.sessions.sid_of(local).cloned())
                                .map(|sid| AgentDestination::Roster(sid.to_string()))
                        })
                        .unwrap_or(AgentDestination::Plain),
                ),
                _ => None,
            });
        match destination {
            Some(AgentDestination::Local(session)) => self.jump_to_session(session, cx),
            Some(AgentDestination::Roster(sid)) => self.jump_to_roster_session(sid, cx),
            _ => {
                self.workspace.focus_tile(id);
                cx.notify();
            }
        }
        self.save_workspace_state();
    }

    /// The palette's current ranked candidates, as `(item, is_selected)` pairs in
    /// display order. The single place the key handler, the render, and the tests
    /// all derive "what's on screen" from.
    pub(crate) fn jump_palette_ranked(&self, cx: &gpui::App) -> (Vec<PaletteItem>, Vec<usize>) {
        let items = self.jump_palette_items(cx);
        let query = self
            .jump_palette_ref()
            .map(|p| p.query.clone())
            .unwrap_or_default();
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
            PaletteTarget::Tile(id) => self.jump_to_tile(id, cx),
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
            let start = selected.saturating_sub(PALETTE_VISIBLE_ROWS - 1).min(
                ranked
                    .len()
                    .saturating_sub(PALETTE_VISIBLE_ROWS.min(ranked.len())),
            );
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
                    .child(div().w(px(16.0)).flex_none().text_color(badge_color).child(
                        SharedString::new_static(if it.is_agent { "✦" } else { "⊞" }),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.0))
                            .text_color(if is_sel || it.active {
                                active_accent
                            } else {
                                st.fg
                            })
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
                                PaletteTarget::Tile(id) => this.jump_to_tile(id, cx),
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
            .child(SharedString::new_static(
                "↑↓:select  enter:jump  esc:cancel",
            ));

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
