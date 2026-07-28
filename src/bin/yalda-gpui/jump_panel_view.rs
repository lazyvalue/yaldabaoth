//! The jump panel (jump-panel; spec-jump-panel.md): an always-visible root-level
//! navigator sidebar. Unlike the per-tile **rail** (`spec-rail.md`,
//! `Workspace::rail`), it is a single instance laid out outside the workspace
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
//! (`select_workspace` / `jump_to_session`) and mutates nothing it reads (INV-JP2).
//! Free agent sessions open in an ephemeral virtual workspace (ADR-0021); bound
//! ones focus their tile in place.

use super::*;

/// Fixed sidebar width. Chrome-class — renders at native size, unaffected by
/// document zoom (consistent with the workspace strip / rail).
pub(crate) const JUMP_PANEL_WIDTH: f32 = 320.0;

/// What a jump-panel agent row points at (universal-agent-list). A session may
/// be opened here (`Local`, keyed by store `SessionId`) or known only to the
/// server via the roster (`Roster`, keyed by server sid) — running but never
/// opened in this GUI.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The autonamer's two-sentence summary of the session (`UXI-AgentTile-27`),
    /// when this GUI has the session open. Rendered as a small italic second
    /// line under the label. `None` for roster-only sessions (never opened here,
    /// so we have no local state to read it from) and for sessions that were
    /// never named.
    pub(crate) summary: Option<String>,
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

/// The row marks a live session wears (`UXI-JumpPanel-10`): `(badge glyph,
/// right-edge status word)`. Color alone was too quiet to catch while scanning a
/// list of sessions, so each live state also gets a WORD and its own glyph SHAPE
/// — legible without relying on hue at all. A quiet session (idle+read,
/// disconnected, or roster-only) keeps the plain `✦` and says nothing.
///
/// Pure so the mapping is headlessly guarded; the tint/outline/italic that go
/// with it are paint (harness gap #1).
pub(crate) fn agent_row_marks(status: AgentDotStatus) -> (&'static str, Option<&'static str>) {
    match status {
        // A filled diamond reads as "lit up / running"; the word removes any doubt.
        AgentDotStatus::Working => ("◆", Some("working")),
        AgentDotStatus::WaitingForYou => ("✦", Some("your turn")),
        AgentDotStatus::Neutral => ("✦", None),
    }
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
            // bug-0022: local state is authoritative when this GUI holds the
            // session; otherwise the SERVER's in-flight flag drives it, so a
            // free session (or one another GUI is driving) shows real status
            // instead of a permanent neutral dot.
            let awaiting = opened
                .map(|e| e.read(cx).state.turn_phase.is_awaiting())
                .or(Some(info.busy));
            let unread = opened
                .map(|e| e.read(cx).state.unread)
                .unwrap_or_else(|| self.roster_unread.contains(&info.session_id));
            // bug-0020: the live session is authoritative, but a session that is
            // NOT open here (free, or freshly restored before attach) still has a
            // durable summary in the id-keyed sidecar. Without this fallback the
            // explainer line only existed for the run that generated it.
            let summary = opened
                .and_then(|e| e.read(cx).state.summary.clone())
                .or_else(|| {
                    self.session_summaries
                        .get(&info.session_id)
                        .filter(|s| !s.trim().is_empty())
                        .cloned()
                });
            rows.push(AgentRow {
                target: JumpTarget::Roster(info.session_id.clone()),
                label,
                summary,
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
                // bug-0020: same sidecar fallback as the roster rows above, for a
                // session whose sid the roster hasn't listed yet.
                summary: ent.read(cx).state.summary.clone().or_else(|| {
                    self.sessions.sid_of(id).and_then(|s| {
                        self.session_summaries
                            .get(s.as_str())
                            .filter(|v| !v.trim().is_empty())
                            .cloned()
                    })
                }),
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

/// One rendered project section in the jump panel (UXI-Project-3): a project's
/// name + cwd, the workspaces that belong to it (each carrying its GLOBAL workspace
/// index so the `ctrl-<n>` badge stays sequential across projects), and the
/// agent sessions rooted at its cwd. Pure view-model so the section structure is
/// headlessly assertable — the render just walks it.
pub(crate) struct JumpProjectSection {
    pub(crate) id: ProjectId,
    pub(crate) name: String,
    pub(crate) cwd_display: String,
    /// `(global workspace idx, label, is-active)` — idx+1 is the `ctrl-<n>` number.
    pub(crate) workspaces: Vec<(usize, String, bool)>,
    /// `(flat row index, row)` — the flat index is the stable listener key.
    pub(crate) sessions: Vec<(usize, AgentRow)>,
}

impl YaldaGpuiView {
    /// Build the jump panel's per-project sections plus the trailing UNFILED
    /// groups (sessions whose cwd no project roots), for `render_jump_panel`
    /// (UXI-Project-3). Every project renders (so an EMPTY project still shows
    /// its header + create rows); its sessions are the cwd-groups that resolve to
    /// it (cwd is unique per project, so each group maps to at most one project),
    /// and its workspaces are the non-ephemeral workspaces whose `wsp.project()` is it.
    /// Sections are ordered by the user's `jump_cwd_order` drag order (keyed on
    /// the project's cwd display — the same key `CwdDrag` carries), stable so
    /// undragged projects stay in id order. Pure (no listeners) so it is
    /// testable in isolation.
    pub(crate) fn jump_panel_sections(
        &self,
        cx: &gpui::App,
    ) -> (Vec<JumpProjectSection>, Vec<(String, Vec<(usize, AgentRow)>)>) {
        let rows = self.jump_panel_agent_rows(cx);
        let grouped = order_grouped_rows(
            group_agent_rows_by_cwd(rows),
            &self.jump_cwd_order,
            &self.jump_session_order,
        );
        // Bucket each cwd-group under the project that roots its cwd; a group with
        // no owning project is Unfiled.
        let mut by_project: std::collections::BTreeMap<ProjectId, Vec<(usize, AgentRow)>> =
            std::collections::BTreeMap::new();
        let mut unfiled: Vec<(String, Vec<(usize, AgentRow)>)> = Vec::new();
        for (cwd_label, group) in grouped {
            let pid = group.first().and_then(|(_, r)| self.projects.by_cwd(&r.cwd));
            match pid {
                Some(id) => by_project.entry(id).or_default().extend(group),
                None => unfiled.push((cwd_label, group)),
            }
        }
        let mut sections: Vec<JumpProjectSection> = Vec::new();
        for (id, p) in self.projects.iter() {
            let workspaces: Vec<(usize, String, bool)> = self
                .workspace
                .workspaces
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.ephemeral && t.project() == id)
                .map(|(idx, t)| {
                    (idx, t.display_label().to_string(), idx == self.workspace.active_workspace)
                })
                .collect();
            let sessions = by_project.remove(&id).unwrap_or_default();
            sections.push(JumpProjectSection {
                id,
                name: p.name.clone(),
                cwd_display: shorten_cwd_for_display(&p.cwd),
                workspaces,
                sessions,
            });
        }
        let cwd_rank = |key: &str| {
            self.jump_cwd_order
                .iter()
                .position(|k| k.as_str() == key)
                .unwrap_or(usize::MAX)
        };
        sections.sort_by_key(|s| cwd_rank(&s.cwd_display));
        (sections, unfiled)
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
    /// Fold or unfold one project's children. Project names are the durable
    /// human key (project ids are runtime-local), so folded state is keyed and
    /// persisted by name.
    pub(crate) fn toggle_project_fold(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.jump_folded_projects.remove(name) {
            self.jump_folded_projects.insert(name.to_string());
        }
        self.save_settings();
        cx.notify();
    }

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
            err: nc(self.theme.agent.jump_header),
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
        // UXI-JumpPanel-11 (reverses UXI-JumpPanel-7's recessed shade): the panel
        // wears the SAME surface as the command menu / jump palette — the theme's
        // `overlay.bg`. The derived recessed shade read muddy on paper-toned
        // themes (Folio); sharing the menu surface makes every chrome popup and
        // the sidebar one material.
        let panel_bg = jump_panel_surface(self.editor_bg());
        let border = nc(self.theme.overlay.border);
        // Inter-section hairline (a rule ABOVE each project header): the dim
        // border color at low alpha, so internal structure stays quieter than
        // the panel's outer right border.
        let mut divider_color = st.dim;
        divider_color.a = 0.4;
        // Project-name header red, softened a hair (it repeats per section, so
        // full-strength `err` reads as an alarm for nav chrome).
        let mut header_red = st.err;
        header_red.a = 0.9;
        // "Waiting for you" status-dot color (turn finished, your move). The
        // tool-completed green reads as ready/done across both themes.
        let ready = nc(self.theme.agent.tool_completed);
        // Header palette (theme-owned; `UXI-JumpPanel-7`): top-level section
        // headers use `st.err` (= `agent.jump_header`); per-cwd "Unfiled"
        // subheaders use `agent.jump_subheader`. Italic is reserved for the
        // "waiting on you" session state (below), so headers carry no italic.
        let electric: Hsla = nc(self.theme.agent.jump_subheader);
        // "Working" status star (a reply in flight) — `agent.jump_working`, warm
        // and distinct from the gold `warm_accent`.
        let working_orange: Hsla = nc(self.theme.agent.jump_working);

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

        // ── Per-project sections (UXI-Project-3, UXI-JumpPanel-7): one section
        // per project. A **hairline rule** separates sections (drawn ABOVE each
        // header); the header is the project NAME only (bold uppercase) — its cwd
        // subtext, the ✕ delete affordance, and the inline ＋create rows are gone.
        // Clicking the name opens a **context menu** (New workspace / New agent
        // session / Delete project; UXI-JumpPanel-8). Each section owns its
        // WORKSPACES sublist (workspaces whose `wsp.project()` is it; the ctrl-<n>
        // number moves to a dim right-edge hint) and its AGENT SESSIONS. Individual
        // tiles are NOT listed. Unfiled sessions (a cwd no project roots) trail
        // under path headers. See `jump_panel_sections`.
        let (sections, unfiled) = self.jump_panel_sections(cx);
        let drag_fg = st.fg;
        let drag_font = st.mono.clone();

        for section in sections {
            let pid = section.id;
            let cwd_key = section.cwd_display.clone();
            let project_name = section.name.clone();
            let folded = self.jump_folded_projects.contains(&project_name);
            // Inter-section rule, above the header (Pinned always precedes the
            // first project, so every project section gets a top rule).
            col = col.child(jump_divider(divider_color));
            // Project header: disclosure chevron + NAME. The chevron owns folding;
            // clicking the name still opens the project menu. Keeping those targets
            // distinct preserves the existing menu gesture. The name remains the
            // drag source and the whole header remains the drop target.
            let fold_name = project_name.clone();
            let name_label: SharedString = project_name.to_uppercase().into();
            let drag_label: SharedString = project_name.into();
            let header = div()
                .id(SharedString::from(format!("jump-proj-{}", pid.0)))
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_3()
                .pb(px(4.0))
                .child(
                    div()
                        .id(SharedString::from(format!("jump-proj-fold-{}", pid.0)))
                        .w(px(18.0))
                        .flex_none()
                        .cursor_pointer()
                        .text_color(st.dim)
                        .child(SharedString::new_static(if folded { "▸" } else { "▾" }))
                        .on_click(cx.listener(move |this, _ev, _window, cx| {
                            this.toggle_project_fold(&fold_name, cx);
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("jump-proj-name-{}", pid.0)))
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .text_color(header_red)
                        .font_family(st.mono.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(st.pt * 0.95))
                        .child(name_label)
                        .on_click(cx.listener(
                            move |this, ev: &gpui::ClickEvent, _window, cx| {
                                let p = ev.position();
                                this.open_project_menu(
                                    pid,
                                    (f32::from(p.x), f32::from(p.y)),
                                    cx,
                                );
                            },
                        ))
                        .on_drag(CwdDrag { cwd_key: cwd_key.clone() }, {
                            let (fg, bg, font) = (drag_fg, sel_bg, drag_font.clone());
                            move |_payload, _pos, _window, cx| {
                                cx.new(|_| JumpDragPreview {
                                    label: drag_label.clone(),
                                    fg,
                                    bg,
                                    font: font.clone(),
                                })
                            }
                        }),
                )
                .drag_over::<CwdDrag>(move |s, _, _, _| s.bg(sel_bg))
                .on_drop(cx.listener({
                    let target_key = cwd_key.clone();
                    move |this, dragged: &CwdDrag, _window, cx| {
                        this.reorder_cwd_group(&dragged.cwd_key, &target_key, cx)
                    }
                }));
            col = col.child(header);

            if folded {
                continue;
            }

            // WORKSPACES sublist — a `⊞` icon leads; the GLOBAL idx+1 (ctrl-<n>)
            // becomes a dim right-edge shortcut hint.
            for (idx, label, active) in section.workspaces {
                let row_id = SharedString::from(format!("jump-ws-{idx}"));
                let num = format!("{}", idx + 1);
                let row = jump_nav_row(
                        row_id,
                        &label,
                        Some("⊞"),
                        None,
                        Some(&num),
                        &st,
                        sel_bg,
                        active.then_some(active_accent),
                    )
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.select_workspace(idx, cx)
                    }));
                col = col.child(probe_bounds_dyn(
                    format!("jump-workspace-row-{idx}"),
                    row.into_any_element(),
                ));
            }

            // AGENT SESSIONS sublist (status-colored `✦` + accent marks preserved).
            for (i, row) in section.sessions {
                let active =
                    jump_target_is_active(&row.target, active_local, active_sid.as_deref());
                col = col.child(jump_session_row_el(
                    i,
                    &row,
                    &st,
                    sel_bg,
                    active_accent,
                    ready,
                    working_orange,
                    active,
                    drag_fg,
                    drag_font.clone(),
                    cx,
                ));
            }
        }

        // ── Unfiled sessions (no project roots their cwd) ─ path headers.
        if !unfiled.is_empty() {
            col = col.child(section_heading("Unfiled", &st).px_3().text_color(st.err));
            for (cwd_label, group) in unfiled {
                let header = div()
                    .w_full()
                    .pt_2()
                    .pb_1()
                    .pl(px(20.0))
                    .pr_3()
                    .text_color(electric)
                    .font_family(st.mono.clone())
                    .text_size(px(st.pt * 0.85))
                    .child(SharedString::from(cwd_label.clone()));
                col = col.child(header);
                for (i, row) in group {
                    let active =
                        jump_target_is_active(&row.target, active_local, active_sid.as_deref());
                    col = col.child(jump_session_row_el(
                        i,
                        &row,
                        &st,
                        sel_bg,
                        active_accent,
                        ready,
                        working_orange,
                        active,
                        drag_fg,
                        drag_font.clone(),
                        cx,
                    ));
                }
            }
        }

        col.into_any_element()
    }

}

/// Build one agent-session row (status dot + accent mark + drag) shared by the
/// per-project sections and the trailing Unfiled groups (UXI-Project-3), so the
/// dot/italic/active/drag semantics stay identical in both. `active` is the
/// precomputed "this row is the focused tile's bound session" mark
/// (UXI-JumpPanel-5). Only roster-backed rows (stable sid) participate in the
/// session drag-reorder.
#[allow(clippy::too_many_arguments)]
fn jump_session_row_el(
    i: usize,
    row: &AgentRow,
    st: &DetailStyle,
    sel_bg: Hsla,
    active_accent: Hsla,
    ready: Hsla,
    working_orange: Hsla,
    active: bool,
    drag_fg: Hsla,
    drag_font: SharedString,
    cx: &mut Context<YaldaGpuiView>,
) -> gpui::AnyElement {
    // The agent-session icon is a `✦` whose COLOR carries the status (one glyph =
    // "this is an agent" + what it's doing): working (reply in flight) → orange;
    // waiting on you (idle + unread) → green + italic label; idle+read /
    // disconnected / unknown → dim.
    let status = row.dot_status();
    let badge_color = match status {
        AgentDotStatus::Working => working_orange,
        AgentDotStatus::WaitingForYou => ready,
        AgentDotStatus::Neutral => st.dim,
    };
    let (badge_glyph, hint) = agent_row_marks(status);
    let row_id = SharedString::from(format!("jump-sess-{i}"));
    let target = row.target.clone();
    let mut r = jump_nav_row_hinted(
        row_id,
        &row.label,
        Some(badge_glyph),
        Some(badge_color),
        hint,
        // The hint IS the status, so it wears the status hue (the workspace
        // rows' `ctrl-<n>` digits stay dim).
        Some(badge_color),
        st,
        sel_bg,
        active.then_some(active_accent),
    );
    if let Some(hue) = match status {
        AgentDotStatus::Working => Some(working_orange),
        AgentDotStatus::WaitingForYou => Some(ready),
        AgentDotStatus::Neutral => None,
    } {
        // The chip: a tint of the status hue behind the row plus a hairline
        // outline in the same hue. Both are alpha-derived from the theme color,
        // so a re-themed palette carries through with no new theme fields. The
        // ACTIVE row keeps its own accent background (you-are-here wins).
        let mut tint = hue;
        tint.a = 0.12;
        let mut edge = hue;
        edge.a = 0.55;
        if !active {
            r = r.bg(tint);
        }
        r = r.border_1().border_color(edge).rounded_md();
    }
    if !row.connected {
        r = r.text_color(st.dim);
    }
    if status == AgentDotStatus::WaitingForYou {
        r = r.italic();
    }
    r = r.on_click(cx.listener({
        let target = target.clone();
        move |this, _ev, _window, cx| this.jump_to_agent(target.clone(), cx)
    }));
    if let JumpTarget::Roster(sid) = &row.target {
        let sid = sid.clone();
        let cwd_key = shorten_cwd_for_display(&row.cwd);
        let label: SharedString = row.label.clone().into();
        r = r
            .on_drag(
                SessionDrag { sid: sid.clone(), cwd_key: cwd_key.clone() },
                move |_payload, _pos, _window, cx| {
                    cx.new(|_| JumpDragPreview {
                        label: label.clone(),
                        fg: drag_fg,
                        bg: sel_bg,
                        font: drag_font.clone(),
                    })
                },
            )
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
    // UXI-AgentTile-27: the autoname summary sits UNDER the label as a small
    // italic dim line. Chrome-class (fixed size, unaffected by document zoom),
    // single-line-height text indented to the label's x so it reads as a
    // subtitle of the row rather than a row of its own. A session with no
    // summary renders exactly as before — no reserved space, no layout shift.
    let Some(summary) = row.summary.as_ref().filter(|s| !s.trim().is_empty()) else {
        return r.into_any_element();
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .child(r)
        .child(
            div()
                // 2px accent gutter + px_3 padding + 16px badge + gap_2 — line the
                // summary up with the label above it.
                .pl(px(38.0))
                .pr_3()
                .pb_1()
                .w_full()
                .min_w_0()
                .italic()
                .text_size(px(st.pt * 0.8))
                .text_color(st.dim)
                .child(SharedString::from(summary.clone())),
        )
        .into_any_element()
}

/// One selectable row: optional leading badge glyph + label + optional trailing
/// dim hint. Returns a `Stateful<Div>` (has an `id`, so it supports
/// `hover`/`on_click`); the caller attaches the click listener. `badge_color`
/// colors the leading badge cell (a status light for agent rows, the dim icon for
/// workspaces); `None` falls back to the dim chrome color. `hint` is a right-edge
/// dim accelerator (the workspace's `ctrl-<n>` digit), rendered small and quiet.
/// Row labels sit at `SEMIBOLD` (they read too thin at normal weight, and this
/// stays a step under the `BOLD` project headers so the hierarchy holds).
/// `active` marks "this is where you are" (UXI-JumpPanel-5): `Some(accent)` draws
/// a left accent bar in that hue, tints the row background, and colors the label
/// with the accent; `None` is a plain row (hover still tints). Every row reserves
/// the 2px left-bar gutter (transparent when inactive) so the mark never shifts
/// row geometry.
#[allow(clippy::too_many_arguments)]
fn jump_nav_row(
    id: impl Into<ElementId>,
    label: &str,
    badge: Option<&str>,
    badge_color: Option<Hsla>,
    hint: Option<&str>,
    st: &DetailStyle,
    sel_bg: Hsla,
    active: Option<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    jump_nav_row_hinted(id, label, badge, badge_color, hint, None, st, sel_bg, active)
}

/// [`jump_nav_row`] with an explicit color for the right-edge hint: agent rows
/// put their status word there in the status hue (`UXI-JumpPanel-10`), while a
/// workspace's `ctrl-<n>` digit stays dim (`None`).
#[allow(clippy::too_many_arguments)]
fn jump_nav_row_hinted(
    id: impl Into<ElementId>,
    label: &str,
    badge: Option<&str>,
    badge_color: Option<Hsla>,
    hint: Option<&str>,
    hint_color: Option<Hsla>,
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
    let mut row = div()
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
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(active.unwrap_or(st.fg))
                .child(SharedString::from(label)),
        );
    if let Some(hint) = hint {
        row = row.child(
            div()
                .flex_none()
                .text_color(hint_color.unwrap_or(st.dim))
                .text_size(px(st.pt * 0.85))
                .child(SharedString::from(hint.to_string())),
        );
    }
    row
}

/// Panel background (`UXI-JumpPanel-11`): the **command-menu surface** —
/// literally `menu_panel_bg`, the elevated card the `?`/`.`/space menus are
/// painted on.
///
/// This REVERSES `UXI-JumpPanel-7`'s "recessed shade" (a ΔL DARKEN of the editor
/// bg, plus a per-theme `agent.jump_panel_bg` art-direction override, both now
/// gone). The recessed derivation read muddy on paper-toned themes (Folio) and
/// made the sidebar a third material next to the editor and the menus; sharing
/// the menu's elevated surface makes all chrome one material and needs no
/// per-theme tuning.
pub(crate) fn jump_panel_surface(editor: Hsla) -> Hsla {
    menu_panel_bg(editor)
}

/// The inter-section hairline drawn above each project header (inset both sides
/// to read as content grouping, not hard chrome).
fn jump_divider(color: Hsla) -> gpui::Div {
    div().mx_3().mt(px(14.0)).mb(px(6.0)).h(px(1.0)).bg(color)
}
