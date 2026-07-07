//! The jump panel (jump-panel; spec-jump-panel.md): an always-visible root-level
//! navigator sidebar. Unlike the per-tile **rail** (`spec-rail.md`,
//! `Tab::rail`), it is a single instance laid out outside the workspace/tab
//! content, so it stays put across workspace switches (INV-JP1).
//!
//! # Why inline, not a cached child
//!
//! The reference cached surfaces (`TranscriptView`, `LinearView`) are cached
//! because they are **expensive** (O(conversation), O(issue body)) and stable
//! while you type elsewhere — caching skips that cost per keystroke. The jump
//! panel is the opposite: its content is O(workspaces + agent sessions), a
//! handful of short rows. GPUI already re-renders the root view every frame, so
//! rebuilding these few rows inline costs essentially nothing — and it sidesteps
//! the cached-child dirty-tracking a root-embedded, root-reading view would need
//! (a cached view created mid-render that READS the root is exactly the case
//! gpui's accessed-entity invalidation handles unreliably here). So the panel is
//! a plain inline element built by [`YaldaGpuiView::render_jump_panel`].
//!
//! It is a pure navigator: a row click calls an existing encapsulated root API
//! (`select_tab` / `jump_to_session`) and mutates nothing it reads (INV-JP2).
//! Free agent sessions open in an ephemeral virtual workspace (ADR-0021); bound
//! ones focus their tile in place.

use super::*;

/// Fixed sidebar width. Chrome-class — renders at native size, unaffected by
/// document zoom (consistent with the tab strip / rail).
pub(crate) const JUMP_PANEL_WIDTH: f32 = 220.0;

/// What a jump-panel agent row points at (universal-agent-list). A session may
/// be opened here (`Local`, keyed by store `SessionId`) or known only to the
/// server via the roster (`Roster`, keyed by server sid) — running but never
/// opened in this GUI.
#[derive(Clone)]
pub(crate) enum JumpTarget {
    Local(SessionId),
    Roster(String),
}

/// One row in the jump panel's "Agent sessions" section.
pub(crate) struct AgentRow {
    pub(crate) target: JumpTarget,
    pub(crate) label: String,
    /// The session's working directory. Rows are grouped under a per-cwd
    /// subheader in `render_jump_panel` (spec-agent-cwd.md; the cwd is the
    /// natural project axis for organizing sessions).
    pub(crate) cwd: PathBuf,
    /// A tile in this GUI currently binds the session (in use).
    pub(crate) bound: bool,
    /// The agent subprocess is live (from the roster's `connected`); local-only
    /// pre-attach sessions are treated as connected.
    pub(crate) connected: bool,
    /// Per-session turn activity, when this GUI has the session open (in
    /// `self.sessions`): `Some(true)` = a reply is in flight (**working**),
    /// `Some(false)` = the turn finished and it's the user's move (**waiting for
    /// you**). `None` = roster-only (running on the server but never opened
    /// here), so the phase is unknown and the dot stays neutral. Drives the
    /// status-dot color in `render_jump_panel`.
    pub(crate) awaiting: Option<bool>,
}

/// Drag payload for a session row being reordered (jump-reorder). Carries the
/// server `sid` (identity for the reorder) plus its `cwd_key` — the group's
/// display path — so a drop target can reject a cross-group drop via `can_drop`,
/// making "a session can't be dragged into another cwd" a hard gate at the
/// gesture level. Typed distinctly from `CwdDrag` so cwd- and session-level
/// drags never cross-fire (GPUI dispatches drops by payload type).
#[derive(Clone)]
pub(crate) struct SessionDrag {
    pub(crate) sid: String,
    pub(crate) cwd_key: String,
}

/// Drag payload for a cwd group header being reordered (jump-reorder). Carries
/// the group's display path key; a header drop reorders the groups.
#[derive(Clone)]
pub(crate) struct CwdDrag {
    pub(crate) cwd_key: String,
}

/// The little floating label rendered under the cursor while dragging a
/// jump-panel row (jump-reorder). GPUI's `on_drag` wants an `Entity<impl
/// Render>` for the drag image; this is that image — just the row's label on a
/// tinted chip so the drag reads clearly.
pub(crate) struct JumpDragPreview {
    label: SharedString,
    fg: Hsla,
    bg: Hsla,
    font: SharedString,
}

impl Render for JumpDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(self.bg)
            .text_color(self.fg)
            .font_family(self.font.clone())
            .text_size(px(13.0))
            .child(self.label.clone())
    }
}

/// The semantic meaning of an agent row's status dot — the color the dot takes
/// is a pure function of `(connected, awaiting)`. Split out from the render so
/// the mapping is unit-testable headlessly (the actual hue is a paint detail).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AgentDotStatus {
    /// A reply is in flight — the agent is busy. Warm accent.
    Working,
    /// The turn finished; it's the user's move. Green.
    WaitingForYou,
    /// Phase unknown (roster-only, not open here) or the agent is disconnected.
    /// Dim chrome color.
    Neutral,
}

impl AgentRow {
    /// Map this row to its status-dot meaning (INV-UX-10). Disconnected wins
    /// (nothing is happening); otherwise a known phase chooses working vs your-
    /// turn, and an unknown phase stays neutral.
    pub(crate) fn dot_status(&self) -> AgentDotStatus {
        if !self.connected {
            return AgentDotStatus::Neutral;
        }
        match self.awaiting {
            Some(true) => AgentDotStatus::Working,
            Some(false) => AgentDotStatus::WaitingForYou,
            None => AgentDotStatus::Neutral,
        }
    }
}

impl YaldaGpuiView {
    /// Build the deduped agent-session rows for the jump panel: the universal
    /// roster (every server session) unioned with local-only sessions not yet
    /// represented in the roster (mid-create placeholders). Sessions opened here
    /// prefer their live store label; binding status comes from the tiles.
    pub(crate) fn jump_panel_agent_rows(&self, cx: &gpui::App) -> Vec<AgentRow> {
        let bound_sids = self.bound_sid_set();
        let mut rows: Vec<AgentRow> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for info in self.agent_roster.entries_by_label() {
            seen.insert(info.session_id.clone());
            let bound = bound_sids.contains(&info.session_id);
            // Prefer the live store label if this session is opened here (kept in
            // sync by SessionRenamed either way, but the entity is authoritative).
            // The same lookup tells us the turn phase (working vs your-turn) when
            // it's open; roster-only sessions have no local phase (`None`).
            let opened = self
                .sessions
                .locate(&info.session_id)
                .and_then(|id| self.sessions.get(id));
            let label = opened
                .map(|e| e.read(cx).label.clone())
                .unwrap_or_else(|| info.label.clone());
            let awaiting = opened.map(|e| e.read(cx).state.turn_phase.is_awaiting());
            rows.push(AgentRow {
                target: JumpTarget::Roster(info.session_id.clone()),
                label,
                cwd: info.cwd.clone(),
                bound,
                connected: info.connected,
                awaiting,
            });
        }

        // Local-only sessions the roster hasn't caught up to (e.g. a just-created
        // placeholder whose sid isn't bound yet, or before the first refresh).
        for (id, ent) in self.sessions.iter() {
            if let Some(sid) = self.sessions.sid_of(id)
                && seen.contains(sid)
            {
                continue;
            }
            rows.push(AgentRow {
                target: JumpTarget::Local(id),
                label: ent.read(cx).label.clone(),
                cwd: ent.read(cx).cwd.clone(),
                bound: self.agent_tile_id_bound_to(id).is_some(),
                connected: true,
                awaiting: Some(ent.read(cx).state.turn_phase.is_awaiting()),
            });
        }
        rows
    }
}

/// Group agent rows by their cwd for the jump panel's per-cwd subheaders
/// (agent-sessions-by-cwd). Returns groups keyed by the display path
/// (`shorten_cwd_for_display`), sorted by that label for stable headers; within
/// each group the rows keep their incoming (by-label) order, each carrying its
/// original flat index so a row's id / listener key stays stable regardless of
/// grouping. Pure so the grouping is headlessly testable — the render just walks
/// the result.
pub(crate) fn group_agent_rows_by_cwd(
    rows: Vec<AgentRow>,
) -> Vec<(String, Vec<(usize, AgentRow)>)> {
    let mut groups: std::collections::BTreeMap<String, Vec<(usize, AgentRow)>> =
        std::collections::BTreeMap::new();
    for (i, row) in rows.into_iter().enumerate() {
        let key = shorten_cwd_for_display(&row.cwd);
        groups.entry(key).or_default().push((i, row));
    }
    // BTreeMap yields keys in sorted order → stable, alphabetized cwd headers.
    groups.into_iter().collect()
}

/// Apply the user's drag-reordered order (jump-reorder) on top of the cwd
/// grouping. `cwd_order` orders the group headers; `session_order` orders the
/// sessions WITHIN each group by sid. Both are applied as a STABLE sort by
/// "rank in the order list", where anything not listed ranks last (`usize::MAX`)
/// and so keeps its incoming alphabetical / by-label position after the listed
/// items. Hence an empty order list is a total no-op — the panel stays
/// alphabetical / by-label until the user actually drags something. A session
/// only ever moves within its own group (its cwd key never changes here), which
/// is what keeps the "a session can't be dragged into another cwd" invariant a
/// structural fact rather than a runtime check. Pure so the ordering is
/// headlessly testable in isolation.
pub(crate) fn order_grouped_rows(
    mut groups: Vec<(String, Vec<(usize, AgentRow)>)>,
    cwd_order: &[String],
    session_order: &[String],
) -> Vec<(String, Vec<(usize, AgentRow)>)> {
    let cwd_rank =
        |key: &str| cwd_order.iter().position(|k| k.as_str() == key).unwrap_or(usize::MAX);
    let sess_rank = |row: &AgentRow| {
        match &row.target {
            JumpTarget::Roster(sid) => session_order.iter().position(|s| s.as_str() == sid),
            JumpTarget::Local(_) => None,
        }
        .unwrap_or(usize::MAX)
    };
    for (_key, group) in groups.iter_mut() {
        group.sort_by_key(|(_, row)| sess_rank(row));
    }
    groups.sort_by_key(|(key, _)| cwd_rank(key));
    groups
}

/// Move `dragged` to the slot `target` currently occupies within `v` (dropping
/// an item onto another takes the target's position, shifting it down). Pure
/// list surgery shared by the cwd- and session-level reorders (jump-reorder), so
/// the move semantics are headlessly testable in isolation. No-op if `dragged`
/// isn't present; if `target` is absent it reinserts at the original index.
pub(crate) fn reorder_move(v: &mut Vec<String>, dragged: &str, target: &str) {
    if dragged == target {
        return;
    }
    let Some(from) = v.iter().position(|x| x == dragged) else {
        return;
    };
    v.remove(from);
    let to = v.iter().position(|x| x == target).unwrap_or(from.min(v.len()));
    v.insert(to, dragged.to_string());
}

impl YaldaGpuiView {
    /// Reorder the jump-panel cwd group `dragged` to `target`'s header position
    /// (jump-reorder, cwd-level drag). Rebuilds `jump_cwd_order` over the CURRENT
    /// set of group keys (in their present display order) so it stays a total
    /// order even as groups come and go, then persists + notifies. Called from
    /// the cwd-header drop handler.
    pub(crate) fn reorder_cwd_group(
        &mut self,
        dragged: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        let rows = self.jump_panel_agent_rows(cx);
        let grouped =
            order_grouped_rows(group_agent_rows_by_cwd(rows), &self.jump_cwd_order, &self.jump_session_order);
        let mut keys: Vec<String> = grouped.into_iter().map(|(k, _)| k).collect();
        reorder_move(&mut keys, dragged, target);
        if keys == self.jump_cwd_order {
            return;
        }
        self.jump_cwd_order = keys;
        self.save_settings();
        cx.notify();
    }

    /// Reorder session `dragged` (server sid) to `target`'s slot WITHIN their
    /// shared cwd group (jump-reorder, session-level drag). The drop is cwd-gated
    /// (`can_drop`), but we defensively re-check both sids share a cwd group and
    /// bail otherwise — a session must never cross cwd groups. Rebuilds
    /// `jump_session_order` over the current sids in display order, then persists
    /// + notifies.
    pub(crate) fn reorder_session(
        &mut self,
        dragged: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        let rows = self.jump_panel_agent_rows(cx);
        // Defensive same-cwd gate (the drop predicate already enforces it).
        let cwd_of = |sid: &str| {
            rows.iter().find_map(|r| match &r.target {
                JumpTarget::Roster(s) if s == sid => Some(shorten_cwd_for_display(&r.cwd)),
                _ => None,
            })
        };
        match (cwd_of(dragged), cwd_of(target)) {
            (Some(a), Some(b)) if a == b => {}
            _ => return,
        }
        // Total order over all roster sids in present display order.
        let grouped =
            order_grouped_rows(group_agent_rows_by_cwd(rows), &self.jump_cwd_order, &self.jump_session_order);
        let mut sids: Vec<String> = grouped
            .into_iter()
            .flat_map(|(_, g)| g.into_iter())
            .filter_map(|(_, r)| match r.target {
                JumpTarget::Roster(s) => Some(s),
                JumpTarget::Local(_) => None,
            })
            .collect();
        reorder_move(&mut sids, dragged, target);
        if sids == self.jump_session_order {
            return;
        }
        self.jump_session_order = sids;
        self.save_settings();
        cx.notify();
    }
}

impl YaldaGpuiView {
    /// Build the jump-panel sidebar element (inline; see the module note).
    /// Reads workspaces + agent sessions + theme directly off `self`; row clicks
    /// re-enter through `cx.listener` and resolve their target id/index in the
    /// handler (never closed over from a prior build).
    pub(crate) fn render_jump_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        record_render("jump_panel");
        let st = DetailStyle {
            fg: self.editor_fg(),
            dim: nc(self.theme.agent.dim),
            accent: nc(self.theme.agent.warm_accent),
            err: rgb(0xff6b6b).into(),
            mono: self.code_font.clone(),
            prose: self.body_font.clone(),
            base: px(13.0),
            pt: 13.0,
        };
        let mut sel_bg = st.accent;
        sel_bg.a = 0.18;
        let panel_bg = self.editor_bg();
        let border = st.dim;
        // "Waiting for you" status-dot color (turn finished, your move). The
        // tool-completed green reads as ready/done across both themes.
        let ready = nc(self.theme.agent.tool_completed);

        // Snapshot the rows up-front (releases the session-entity reads before we
        // wire listeners). Workspaces: non-ephemeral tabs, active marked.
        let workspaces: Vec<(usize, String, bool)> = self
            .workspace
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.ephemeral)
            .map(|(idx, t)| (idx, t.display_label().to_string(), idx == self.workspace.active_tab))
            .collect();

        // Agent sessions: the UNIVERSAL roster (universal-agent-list) — every
        // session the server knows about, opened here or not — unioned with any
        // local-only sessions still mid-create. See `jump_panel_agent_rows`.
        let rows = self.jump_panel_agent_rows(cx);

        let mut col = div()
            .id("jump-panel")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.jump_panel_scroll)
            .bg(panel_bg)
            .text_color(st.fg)
            .border_r_1()
            .border_color(border)
            .py_2();

        // ── Pinned (placeholder; pinning mechanics land later) ───────────────
        col = col.child(section_heading("Pinned", &st).px_3());
        col = col.child(
            div()
                .px_3()
                .py_1()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(px(st.pt * 0.9))
                .child(SharedString::from("Nothing pinned yet.")),
        );

        // ── Workspaces ───────────────────────────────────────────────────────
        // The badge shows the 1-based workspace number — the same digit that
        // `ctrl-<n>` switches to (`goto_workspace_number`). Non-ephemeral tabs
        // occupy indices `0..N` contiguously (ephemeral virtual workspaces sort
        // last; see the goto-workspace menu), so `idx + 1` is a stable number.
        col = col.child(section_heading("Workspaces", &st).px_3());
        for (idx, label, active) in workspaces {
            let row_id = SharedString::from(format!("jump-ws-{idx}"));
            let num = format!("{}", idx + 1);
            col = col.child(
                jump_nav_row(row_id, &label, active, active, Some(&num), None, &st, sel_bg)
                    .on_click(
                        cx.listener(move |this, _ev, _window, cx| this.select_tab(idx, cx)),
                    ),
            );
        }

        // ── Agent sessions ─────────────────────────────────────────────────—
        col = col.child(section_heading("Agent sessions", &st).px_3());
        if rows.is_empty() {
            col = col.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(st.dim)
                    .font_family(st.mono.clone())
                    .text_size(px(st.pt * 0.9))
                    .child(SharedString::from("No sessions.")),
            );
        }
        // Group the session rows by cwd, then apply the user's drag-reordered
        // order (jump-reorder): `order_grouped_rows` reorders the cwd groups by
        // `jump_cwd_order` and the sessions within each group by
        // `jump_session_order`. Both default to alphabetical / by-label until the
        // user drags. The enumerate index `i` from the pre-group flat order is
        // retained as the row id / listener key so ids stay stable regardless of
        // grouping OR reorder. A cwd header can be dragged to reorder groups; a
        // session can be dragged to reorder within its group — never across
        // groups (the `can_drop` cwd gate).
        let grouped = order_grouped_rows(
            group_agent_rows_by_cwd(rows),
            &self.jump_cwd_order,
            &self.jump_session_order,
        );
        // Chip colors for the floating drag image (captured per-drag below).
        let drag_fg = st.fg;
        let drag_font = st.mono.clone();
        for (cwd_label, group) in grouped {
            let cwd_key = cwd_label.clone();
            // The cwd subheader is itself a drag SOURCE (reorder groups) and a
            // drop TARGET for other headers. It's a plain heading turned into a
            // stateful div (needs an id for drag/drop + a drop-highlight).
            let header_id = SharedString::from(format!("jump-cwd-{cwd_label}"));
            let header = section_heading(&cwd_label, &st)
                .id(header_id)
                .pl(px(20.0))
                .pr_3()
                .text_size(px(st.pt * 0.85))
                .cursor_pointer()
                .on_drag(CwdDrag { cwd_key: cwd_key.clone() }, {
                    let label: SharedString = cwd_label.clone().into();
                    let (fg, bg, font) = (drag_fg, sel_bg, drag_font.clone());
                    move |_payload, _pos, _window, cx| {
                        cx.new(|_| JumpDragPreview {
                            label: label.clone(),
                            fg,
                            bg,
                            font: font.clone(),
                        })
                    }
                })
                .drag_over::<CwdDrag>(move |s, _, _, _| s.bg(sel_bg))
                .on_drop(cx.listener({
                    let target_key = cwd_key.clone();
                    move |this, dragged: &CwdDrag, _window, cx| {
                        this.reorder_cwd_group(&dragged.cwd_key, &target_key, cx)
                    }
                }));
            col = col.child(header);
            for (i, row) in group {
                // Bound (in-use) sessions jump to their existing tile; free ones
                // open in an ephemeral virtual workspace. ● = in use, ○ = free.
                //
                // The dot's COLOR is a per-session status light (universal-agent-
                // list gives the row; the open session gives the phase):
                //   • working (a reply is in flight)      → warm accent (`st.accent`)
                //   • waiting for you (turn finished, idle) → green   (`ready`)
                //   • unknown phase (roster-only, not open here) or disconnected → dim
                // A disconnected session is also dimmed wholesale (the label too).
                let badge = if row.bound { "●" } else { "○" };
                let badge_color = match row.dot_status() {
                    AgentDotStatus::Working => st.accent,
                    AgentDotStatus::WaitingForYou => ready,
                    AgentDotStatus::Neutral => st.dim,
                };
                let row_id = SharedString::from(format!("jump-sess-{i}"));
                let target = row.target.clone();
                let mut r = jump_nav_row(
                    row_id,
                    &row.label,
                    false,
                    false,
                    Some(badge),
                    Some(badge_color),
                    &st,
                    sel_bg,
                );
                if !row.connected {
                    r = r.text_color(st.dim);
                }
                r = r.on_click(cx.listener({
                    let target = target.clone();
                    move |this, _ev, _window, cx| this.jump_to_agent(target.clone(), cx)
                }));
                // Only roster-backed sessions (with a stable sid) participate in
                // drag-reorder; local-only mid-create placeholders don't.
                if let JumpTarget::Roster(sid) = &row.target {
                    let sid = sid.clone();
                    let cwd_key = cwd_key.clone();
                    let label: SharedString = row.label.clone().into();
                    let (fg, font) = (drag_fg, drag_font.clone());
                    r = r
                        .on_drag(
                            SessionDrag { sid: sid.clone(), cwd_key: cwd_key.clone() },
                            move |_payload, _pos, _window, cx| {
                                cx.new(|_| JumpDragPreview {
                                    label: label.clone(),
                                    fg,
                                    bg: sel_bg,
                                    font: font.clone(),
                                })
                            },
                        )
                        // Gate: only accept a session drag from the SAME cwd group
                        // — this is what makes "a session can't be dragged into a
                        // cwd it doesn't belong in" a hard rule at the gesture.
                        .can_drop({
                            let cwd_key = cwd_key.clone();
                            move |dragged, _window, _cx| {
                                dragged
                                    .downcast_ref::<SessionDrag>()
                                    .is_some_and(|d| d.cwd_key == cwd_key)
                            }
                        })
                        .drag_over::<SessionDrag>(move |s, _, _, _| s.bg(sel_bg))
                        .on_drop(cx.listener({
                            let target_sid = sid.clone();
                            move |this, dragged: &SessionDrag, _window, cx| {
                                this.reorder_session(&dragged.sid, &target_sid, cx)
                            }
                        }));
                }
                col = col.child(r);
            }
        }

        col.into_any_element()
    }
}

/// One selectable row: optional leading badge glyph + label, tinted when
/// `selected`. Returns a `Stateful<Div>` (has an `id`, so it supports
/// `hover`/`on_click`); the caller attaches the click listener. `accent_text`
/// draws the label in the accent color (used to mark the active workspace).
/// `badge_color` colors the leading badge cell (a status light for agent rows);
/// `None` falls back to the dim chrome color (workspace numbers).
#[allow(clippy::too_many_arguments)]
fn jump_nav_row(
    id: impl Into<ElementId>,
    label: &str,
    selected: bool,
    accent_text: bool,
    badge: Option<&str>,
    badge_color: Option<Hsla>,
    st: &DetailStyle,
    sel_bg: Hsla,
) -> gpui::Stateful<gpui::Div> {
    let transparent: Hsla = rgba(0x00000000).into();
    let label = if label.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        label.to_string()
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .px_3()
        .py_1()
        .cursor_pointer()
        .text_size(st.base)
        .font_family(st.mono.clone())
        .bg(if selected { sel_bg } else { transparent })
        .hover(|s| s.bg(sel_bg))
        .child(
            div()
                .w(px(16.0))
                .flex_none()
                .text_color(badge_color.unwrap_or(st.dim))
                .child(SharedString::from(badge.unwrap_or("").to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if accent_text { st.accent } else { st.fg })
                .child(SharedString::from(label)),
        )
}
