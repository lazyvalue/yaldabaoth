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
    /// The session finished a turn whose output the user hasn't looked at
    /// ("waiting on you") — `AgentState.unread`. `false` for roster-only sessions
    /// not opened in this GUI (unknown). Drives the ● green + italic row.
    pub(crate) unread: bool,
    /// The sid this row occupies in the user's drag order (`jump_session_order`).
    /// For a roster row that is its own sid. For a local-only placeholder it is
    /// its PREDECESSOR's sid when the placeholder continues a killed session
    /// (`jump_order_succession`, e.g. `/clear`) — so the row holds its slot
    /// through the whole close→create→bind window instead of falling to the
    /// bottom (bug-0007). `None` = genuinely new, unranked, sorts after.
    pub(crate) order_sid: Option<String>,
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
    /// A reply is in flight — the agent is doing something right now.
    /// Rendered ● **orange**.
    Working,
    /// The agent finished a turn whose output you haven't read — it's waiting on
    /// you. Rendered ● **green**, and the session label goes **italic**.
    WaitingForYou,
    /// Idle with nothing unread (you've read it), or disconnected, or a
    /// roster-only session whose phase we can't know. Rendered ○ **dim**.
    Neutral,
}

impl AgentRow {
    /// Map this row to its status-dot meaning (UXI-JumpPanel-1). Disconnected wins
    /// (nothing is happening); otherwise a reply in flight is **working**, an idle
    /// turn with unread output is **waiting on you**, and anything else (idle +
    /// read, or unknown phase) is **neutral**.
    pub(crate) fn dot_status(&self) -> AgentDotStatus {
        if !self.connected {
            return AgentDotStatus::Neutral;
        }
        match self.awaiting {
            Some(true) => AgentDotStatus::Working,
            Some(false) if self.unread => AgentDotStatus::WaitingForYou,
            _ => AgentDotStatus::Neutral,
        }
    }
}

impl YaldaGpuiView {
    /// Build the deduped agent-session rows for the jump panel: the universal
    /// roster (every server session) unioned with local-only sessions not yet
    /// represented in the roster (mid-create placeholders). Sessions opened here
    /// prefer their live store label; binding status comes from the tiles.
    /// The jump-panel group header for a session cwd (ADR-0028, `UXI-Project-3`):
    /// the **project name** when a project roots that cwd (`Membership::Inferred`
    /// / `Assigned`), else the shortened path for an `Unfiled` session. This is
    /// what turns the "sessions grouped by cwd string" list into "sessions
    /// grouped under their project," representing the hierarchy.
    pub(crate) fn jump_group_header(&self, cwd: &std::path::Path) -> String {
        self.projects
            .by_cwd(cwd)
            .map(|id| self.projects.name_of(id).to_string())
            .unwrap_or_else(|| shorten_cwd_for_display(cwd))
    }

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
                // Wire boundary: the roster carries raw String sids from the server.
                .locate(&ServerSid::new(info.session_id.clone()))
                .and_then(|id| self.sessions.get(id));
            let label = opened
                .map(|e| e.read(cx).label.clone())
                .unwrap_or_else(|| info.label.clone());
            let awaiting = opened.map(|e| e.read(cx).state.turn_phase.is_awaiting());
            let unread = opened.map(|e| e.read(cx).state.unread).unwrap_or(false);
            rows.push(AgentRow {
                target: JumpTarget::Roster(info.session_id.clone()),
                label,
                cwd: info.cwd.clone(),
                bound,
                connected: info.connected,
                awaiting,
                unread,
                order_sid: Some(info.session_id.clone()),
            });
        }

        // Local-only sessions the roster hasn't caught up to (e.g. a just-created
        // placeholder whose sid isn't bound yet, or before the first refresh).
        for (id, ent) in self.sessions.iter() {
            if let Some(sid) = self.sessions.sid_of(id)
                && seen.contains(sid.as_str())
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
                unread: ent.read(cx).state.unread,
                // Its own sid when it has one (bound but the roster hasn't listed
                // it yet), else — for a `/clear` placeholder — the killed
                // session's sid, whose order slot it inherits.
                order_sid: self
                    .sessions
                    .sid_of(id)
                    .map(|s| s.as_str().to_string())
                    .or_else(|| self.jump_order_succession.get(&id).cloned()),
            });
        }
        // Order the COMBINED list (roster + local-only) by label, so a session sits
        // in the SAME slot whether or not the async roster refresh has caught up to
        // it yet. Otherwise a freshly-created local session renders LAST (appended
        // above), then HOPS into its label-sorted slot once the roster catches up —
        // the "sessions spontaneously reorder for some weird reason" bug (bug-0006).
        // Stable tiebreak by target key keeps equal labels deterministic.
        rows.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| jump_target_key(&a.target).cmp(&jump_target_key(&b.target))));
        rows
    }
}

/// A stable, total-order key for a jump target — the tiebreak when two rows share a
/// label, so the combined roster+local ordering is deterministic across renders.
fn jump_target_key(t: &JumpTarget) -> String {
    match t {
        JumpTarget::Roster(sid) => format!("r:{sid}"),
        JumpTarget::Local(id) => format!("l:{id:?}"),
    }
}

/// Does this row point at the **active** (focused-tile-bound) agent session
/// (UXI-JumpPanel-5)? `active_local` is the focused tile's local `SessionId`;
/// `active_sid` its server sid, if it has one. A focused session with a sid
/// surfaces as a `Roster` row (roster wins the dedup), but before the roster
/// catches up it can be a `Local` row — so match BOTH so the box never blinks
/// off across that window. Pure so the "which row is active" derivation is
/// headlessly testable in isolation.
pub(crate) fn jump_target_is_active(
    target: &JumpTarget,
    active_local: Option<SessionId>,
    active_sid: Option<&str>,
) -> bool {
    match target {
        JumpTarget::Local(id) => active_local == Some(*id),
        JumpTarget::Roster(sid) => active_sid == Some(sid.as_str()),
    }
}

impl YaldaGpuiView {
    /// The identity of the active agent session for the jump-panel active box
    /// (UXI-JumpPanel-5): the session bound to the FOCUSED tile, as `(local id,
    /// server sid)`. `(None, _)` when the focused tile is a buffer or an unbound
    /// agent tile. `render_jump_panel` consumes this; exposed for headless
    /// assertion so the test drives the same derivation the paint does.
    pub(crate) fn jump_active_session(&self) -> (Option<SessionId>, Option<String>) {
        // Non-panicking: the jump panel can render before/without a focused
        // window (e.g. transient ephemeral-only states), so match the focused
        // content directly rather than `focused_bound_session()` (which
        // `expect`s a focused window).
        let local = match self.workspace.focused_content() {
            Some(App::Agent(tile)) => tile.session(),
            _ => None,
        };
        let sid = local.and_then(|id| self.sessions.sid_of(id).map(|s| s.as_str().to_string()));
        (local, sid)
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
    // Rank by the row's ORDER sid, not its target: a `/clear` placeholder carries
    // its predecessor's sid (`order_sid`) and so keeps that slot across the
    // close→create→bind window (bug-0007).
    let sess_rank = |row: &AgentRow| {
        row.order_sid
            .as_ref()
            .and_then(|sid| session_order.iter().position(|s| s == sid))
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
        // The "active / selection" hue is the theme's cool primary accent
        // (`frozen_bar` — cyan/teal across all themes), NOT the warm accent. A
        // low-alpha cool tint reads as a clean editor-sidebar selection; the old
        // warm_accent tint muddied to brown/olive over the background, and the
        // "you are here" mark is now this same accent as a left bar rather than a
        // bright red bounding box (UXI-JumpPanel-5).
        let active_accent = nc(self.theme.agent.frozen_bar);
        let mut sel_bg = active_accent;
        sel_bg.a = 0.15;
        let panel_bg = self.editor_bg();
        let border = st.dim;
        // "Waiting for you" status-dot color (turn finished, your move). The
        // tool-completed green reads as ready/done across both themes.
        let ready = nc(self.theme.agent.tool_completed);
        // Header palette: top-level section headers are RED (`st.err`); per-cwd
        // subheaders are ELECTRIC BLUE (`0x3b9eff`, a vivid theme-neutral blue).
        // Italic is reserved for the "waiting on you" session state (below), so
        // headers carry no italic.
        let electric: Hsla = rgb(0x3b9eff).into();
        // Orange "working" status-dot (a reply is in flight). Fixed warm orange,
        // distinct from the warm gold `warm_accent` and legible on every theme.
        let working_orange: Hsla = rgb(0xff9e64).into();

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

        // The active screen element for the red "you are here" box (UXI-JumpPanel-5):
        // the session bound to the focused tile (matched against each row below).
        let (active_local, active_sid) = self.jump_active_session();

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
        col = col.child(section_heading("Pinned", &st).px_3().text_color(st.err));
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
        col = col.child(section_heading("Workspaces", &st).px_3().text_color(st.err));
        for (idx, label, active) in workspaces {
            let row_id = SharedString::from(format!("jump-ws-{idx}"));
            let num = format!("{}", idx + 1);
            col = col.child(
                // Active workspace wears the "you are here" mark (UXI-JumpPanel-5):
                // a left accent bar + accent label + selection tint.
                jump_nav_row(
                    row_id,
                    &label,
                    Some(&num),
                    None,
                    &st,
                    sel_bg,
                    active.then_some(active_accent),
                )
                    .on_click(
                        cx.listener(move |this, _ev, _window, cx| this.select_tab(idx, cx)),
                    ),
            );
        }

        // ── Agent sessions ─────────────────────────────────────────────────—
        col = col.child(section_heading("Agent sessions", &st).px_3().text_color(st.err));
        // A discoverable create-affordance for a FREE (tile-less) session
        // (UXI-JumpPanel-3): clicking opens the cwd picker (UXI-JumpPanel-4), then
        // spawns a session bound to no tile/workspace via
        // `spawn_free_agent_session_at`. It lands in the roster above as a new
        // unbound (○) row — never auto-bound — bindable later by selecting it.
        // Placed here, where free sessions surface, so creating one is a click
        // away instead of buried in the `?` menu.
        col = col.child(
            jump_nav_row(
                SharedString::from("jump-new-agent"),
                "New agent session",
                Some("＋"),
                Some(active_accent),
                &st,
                sel_bg,
                None,
            )
            .on_click(
                cx.listener(|this, _ev, _window, cx| {
                    this.open_free_agent_session_cwd_overlay(cx)
                }),
            ),
        );
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
            // Display the PROJECT NAME as the group header (UXI-Project-3),
            // resolved from a row's cwd; the drag machinery below still keys on
            // the cwd label. Falls back to the shortened path for unfiled sessions.
            let header_text = group
                .first()
                .map(|(_, r)| self.jump_group_header(&r.cwd))
                .unwrap_or_else(|| cwd_label.clone());
            // The cwd subheader is itself a drag SOURCE (reorder groups) and a
            // drop TARGET for other headers. It's a plain heading turned into a
            // stateful div (needs an id for drag/drop + a drop-highlight).
            let header_id = SharedString::from(format!("jump-cwd-{cwd_label}"));
            // A cwd subheader is a SECONDARY grouping label — electric blue, real
            // path casing (not the bold red uppercased top-level `section_heading`),
            // so the two header tiers read as a clear hierarchy. No italic (italic
            // is reserved for the "waiting on you" session state).
            let header = div()
                .id(header_id)
                .w_full()
                .pt_2()
                .pb_1()
                .pl(px(20.0))
                .pr_3()
                .text_color(electric)
                .font_family(st.mono.clone())
                .text_size(px(st.pt * 0.85))
                .cursor_pointer()
                .child(SharedString::from(header_text.clone()))
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
                // The status dot encodes what the AGENT is doing (not binding):
                //   • working  (reply in flight)                 → ● orange
                //   • waiting on you (idle + unread output)       → ● green + italic
                //   • idle+read / disconnected / unknown phase    → ○ dim
                let status = row.dot_status();
                let (badge, badge_color) = match status {
                    AgentDotStatus::Working => ("●", working_orange),
                    AgentDotStatus::WaitingForYou => ("●", ready),
                    AgentDotStatus::Neutral => ("○", st.dim),
                };
                let row_id = SharedString::from(format!("jump-sess-{i}"));
                let target = row.target.clone();
                // Left accent bar when this row is the focused tile's bound session
                // (UXI-JumpPanel-5).
                let active = jump_target_is_active(&row.target, active_local, active_sid.as_deref());
                let mut r = jump_nav_row(
                    row_id,
                    &row.label,
                    Some(badge),
                    Some(badge_color),
                    &st,
                    sel_bg,
                    active.then_some(active_accent),
                );
                if !row.connected {
                    r = r.text_color(st.dim);
                }
                // Italic == "waiting on you" (idle with unread output). The one
                // meaning italic carries in the panel.
                if status == AgentDotStatus::WaitingForYou {
                    r = r.italic();
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

/// One selectable row: optional leading badge glyph + label. Returns a
/// `Stateful<Div>` (has an `id`, so it supports `hover`/`on_click`); the caller
/// attaches the click listener. `badge_color` colors the leading badge cell (a
/// status light for agent rows); `None` falls back to the dim chrome color
/// (workspace numbers). `active` marks "this is where you are" (UXI-JumpPanel-5):
/// `Some(accent)` draws a left accent bar in that hue, tints the row background,
/// and colors the label with the accent; `None` is a plain row (hover still
/// tints). Every row reserves the 2px left-bar gutter (transparent when inactive)
/// so the mark never shifts row geometry.
fn jump_nav_row(
    id: impl Into<ElementId>,
    label: &str,
    badge: Option<&str>,
    badge_color: Option<Hsla>,
    st: &DetailStyle,
    sel_bg: Hsla,
    active: Option<Hsla>,
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
        // Left accent bar for the active row; a transparent bar of the same width
        // on every other row keeps content alignment identical.
        .border_l_2()
        .border_color(active.unwrap_or(transparent))
        .bg(if active.is_some() { sel_bg } else { transparent })
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
                .text_color(active.unwrap_or(st.fg))
                .child(SharedString::from(label)),
        )
}
