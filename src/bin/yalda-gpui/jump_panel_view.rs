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

/// Theme-owned cool supporting copy for session summaries. This
/// explicit seam guards against accidentally routing either through the warm
/// gold accent or the intentionally low-contrast structural `dim` color.
pub(crate) fn jump_supporting_text_color(theme: &yalda::theme::AgentTheme) -> yalda::style::Color {
    theme.agent_tint
}

/// The per-project agent list selected under that project's workspace rows.
/// `All` is the default so the new control preserves the panel's existing
/// visibility until the user asks for a live-state slice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JumpAgentTab {
    Waiting,
    Working,
    #[default]
    All,
    Archived,
}

impl JumpAgentTab {
    fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::Working => "Working",
            Self::All => "All",
            Self::Archived => "Archived",
        }
    }
}

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
#[derive(Clone)]
pub(crate) struct AgentRow {
    pub(crate) target: JumpTarget,
    pub(crate) label: String,
    /// Backend that owns the ACP session. This is projected from server truth
    /// for roster rows and from the local AgentState for pre-roster rows; it is
    /// never inferred from the user-editable label (UXI-JumpPanel-22).
    pub(crate) provider: AgentProvider,
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
    /// `Some(false)` = no reply is in flight. Roster-backed rows also receive
    /// `Some(info.busy)` from the server; `None` is reserved for genuinely
    /// unknown local state. Every connected non-working row is ready for input.
    /// Drives the status-dot color in `render_jump_panel`.
    pub(crate) awaiting: Option<bool>,
    /// The session finished a turn whose output the user hasn't looked at —
    /// `AgentState.unread`. Retained for attention/accounting; it no longer
    /// changes the row's visible operational state.
    pub(crate) unread: bool,
    /// The autonamer's compact topic summary of the session (`UXI-AgentTile-27`),
    /// Rendered as a small italic second line under the label. Live local state
    /// is authoritative; roster-only sessions fall back to the durable
    /// id-keyed summary sidecar. `None` means no usable topic exists yet.
    pub(crate) summary: Option<String>,
    /// The one-shot topic summary is currently being derived. Used to render a
    /// quiet progress line instead of making the summary appear broken.
    pub(crate) summary_pending: bool,
    /// Durable visibility flag, orthogonal to Waiting / Working activity.
    pub(crate) archived: bool,
    /// The sid this row occupies in the user's drag order (`jump_session_order`).
    /// For a roster row that is its own sid. For a local-only placeholder it is
    /// its PREDECESSOR's sid when the placeholder continues a killed session
    /// (`jump_order_succession`, e.g. `/clear`) — so the row holds its slot
    /// through the whole close→create→bind window instead of falling to the
    /// bottom (bug-0007). `None` = genuinely new, unranked, sorts after.
    pub(crate) order_sid: Option<String>,
    /// When the row entered its CURRENT live state. Working rows source this
    /// from the turn start; waiting rows source it from the idle transition.
    /// State tabs sort ascending, making the most recent transition last.
    pub(crate) state_entered_at: Option<std::time::Instant>,
    /// The session's user tags (UXI-JumpPanel-20), from the id-keyed
    /// `session_tags.json` sidecar by sid. Empty = untagged (renders as a flat
    /// row); each tag groups the row under a collapsible folder within its
    /// project's tab. Only roster-backed rows (stable sid) can carry tags.
    pub(crate) tags: Vec<String>,
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

/// Drag payload for a tag folder header being reordered (UXI-JumpPanel-21).
/// Carries the owning `project` name (tags are project-scoped, so a folder drag
/// never crosses projects) plus the `tag`. Typed distinctly so tag- and cwd-level
/// drags never cross-fire.
#[derive(Clone)]
pub(crate) struct TagDrag {
    pub(crate) project: String,
    pub(crate) tag: String,
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
    /// Rendered **orange** with a filled `◆`.
    Working,
    /// The connected agent is not producing a reply and is ready for input.
    /// Rendered **green** with no redundant status word.
    WaitingForYou,
    /// Disconnected or connecting. Rendered **dim**.
    Neutral,
}

/// The operational state used by the Waiting / Working tabs. `unread` remains
/// useful internally, but it does not change the visible operational state:
/// every connected idle agent is ready for input. Disconnected/connecting
/// sessions remain available in All.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AgentActivity {
    Waiting,
    Working,
    Unavailable,
}

/// The shared status marks used by the Agent Tile and Jump Panel
/// (`UXI-AgentTile-28`, `UXI-JumpPanel-10`): `(badge glyph, status word)`.
/// The tile uses both values in its status pill. The Jump Panel uses only the
/// glyph because its tabs and All-group headers already name the state.
///
/// Pure so the mapping is headlessly guarded; the tint that goes with it is
/// paint (harness gap #1).
pub(crate) fn agent_row_marks(status: AgentDotStatus) -> (&'static str, Option<&'static str>) {
    match status {
        // A filled diamond reads as "lit up / running"; the word removes any doubt.
        AgentDotStatus::Working => ("◆", Some("working")),
        AgentDotStatus::WaitingForYou => ("✦", Some("your turn")),
        AgentDotStatus::Neutral => ("✦", None),
    }
}

/// Compact provider identity carried at the trailing edge of every agent row
/// (UXI-JumpPanel-22). Both provider names start with C, so distinct shapes are
/// more legible than an ambiguous initial.
pub(crate) fn agent_provider_mark(provider: AgentProvider) -> &'static str {
    match provider {
        AgentProvider::Claude => "✳",
        AgentProvider::Codex => "⌬",
    }
}

/// The operational status palette is deliberately small and literal.
pub(crate) fn jump_agent_status_color(
    theme: &yalda::theme::AgentTheme,
    status: AgentDotStatus,
) -> yalda::style::Color {
    match status {
        AgentDotStatus::Working => theme.jump_working,
        AgentDotStatus::WaitingForYou => theme.tool_completed,
        AgentDotStatus::Neutral => theme.dim,
    }
}

/// Selection is neutral chrome, not another operational status hue.
pub(crate) fn jump_selection_color(theme: &yalda::theme::OverlayTheme) -> yalda::style::Color {
    theme.selected_bg
}

impl AgentRow {
    pub(crate) fn activity(&self) -> AgentActivity {
        if !self.connected {
            AgentActivity::Unavailable
        } else if self.awaiting == Some(true) {
            AgentActivity::Working
        } else {
            AgentActivity::Waiting
        }
    }

    /// Map this row to its status-dot meaning (UXI-JumpPanel-1). Disconnected wins
    /// (nothing is available); otherwise a reply in flight is **working** and
    /// every connected non-working agent is **ready for input**. This deliberately
    /// matches the Waiting-tab admission rule, so no Waiting row can look neutral.
    pub(crate) fn dot_status(&self) -> AgentDotStatus {
        if !self.connected {
            return AgentDotStatus::Neutral;
        }
        match self.awaiting {
            Some(true) => AgentDotStatus::Working,
            _ => AgentDotStatus::WaitingForYou,
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
            // The same lookup tells us the turn phase (working vs ready) when
            // it's open; roster-only sessions have no local phase (`None`).
            let opened = self
                .sessions
                // Wire boundary: the roster carries raw String sids from the server.
                .locate(&ServerSid::new(info.session_id.clone()))
                .and_then(|id| self.sessions.get(id));
            let label = opened
                .map(|e| e.read(cx).label.clone())
                .unwrap_or_else(|| info.label.clone());
            // Local activity can lead the roster briefly while the server's
            // SessionBusy broadcast is in flight. Capture its phase and entry
            // time together so status and chronology are one coherent read.
            let local_activity = opened.map(|e| {
                let state = &e.read(cx).state;
                let awaiting = state.turn_phase.is_awaiting();
                let entered_at = if awaiting {
                    state.turn_phase.turn_started()
                } else {
                    state.waiting_since
                };
                (awaiting, entered_at)
            });
            // bug-0022: local state is authoritative when this GUI holds the
            // session; otherwise the SERVER's in-flight flag drives it, so a
            // free session (or one another GUI is driving) shows real status
            // instead of a permanent neutral dot.
            let awaiting = local_activity
                .map(|(awaiting, _)| awaiting)
                .or(Some(info.busy));
            let unread = opened
                .map(|e| e.read(cx).state.unread)
                .unwrap_or_else(|| self.roster_unread.contains_key(&info.session_id));
            let roster_entered_at = self.agent_roster.state_since(&info.session_id);
            // UXI-JumpPanel-14: attaching/viewing a roster session constructs a
            // fresh local AgentState whose default waiting_since is "now". That
            // construction is NOT an operational transition and must not send
            // the row to the bottom of Waiting. While local + roster activity
            // agree, retain the roster's identity-stable entry time. If they
            // disagree, local state has begun a real transition ahead of the
            // corresponding server broadcast, so its timestamp leads until the
            // roster catches up.
            let state_entered_at = match local_activity {
                Some((local_busy, local_entered_at)) if local_busy == info.busy => {
                    roster_entered_at.or(local_entered_at)
                }
                Some((_, local_entered_at)) => local_entered_at.or(roster_entered_at),
                None => roster_entered_at,
            };
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
                })
                // Enforce the current compact display contract for summaries
                // written by older builds with the former 240-character cap.
                .and_then(|summary| sanitize_summary(&summary));
            let summary_pending = opened.is_some_and(|e| {
                let state = &e.read(cx).state;
                state.summary.is_none() && state.autoname == AutonameState::Requested
            });
            rows.push(AgentRow {
                target: JumpTarget::Roster(info.session_id.clone()),
                label,
                provider: info.provider,
                summary,
                summary_pending,
                archived: self.jump_archived_sessions.contains(&info.session_id),
                cwd: info.cwd.clone(),
                bound,
                connected: info.connected,
                awaiting,
                unread,
                order_sid: Some(info.session_id.clone()),
                state_entered_at,
                tags: self
                    .session_tags
                    .get(&info.session_id)
                    .cloned()
                    .unwrap_or_default(),
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
                provider: ent.read(cx).state.provider,
                // bug-0020: same sidecar fallback as the roster rows above, for a
                // session whose sid the roster hasn't listed yet.
                summary: ent
                    .read(cx)
                    .state
                    .summary
                    .clone()
                    .or_else(|| {
                        self.sessions.sid_of(id).and_then(|s| {
                            self.session_summaries
                                .get(s.as_str())
                                .filter(|v| !v.trim().is_empty())
                                .cloned()
                        })
                    })
                    .and_then(|summary| sanitize_summary(&summary)),
                summary_pending: {
                    let state = &ent.read(cx).state;
                    state.summary.is_none() && state.autoname == AutonameState::Requested
                },
                archived: self
                    .sessions
                    .sid_of(id)
                    .map(|sid| self.jump_archived_sessions.contains(sid.as_str()))
                    .or_else(|| {
                        self.jump_order_succession
                            .get(&id)
                            .map(|sid| self.jump_archived_sessions.contains(sid))
                    })
                    .unwrap_or(false),
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
                state_entered_at: {
                    let state = &ent.read(cx).state;
                    if state.turn_phase.is_awaiting() {
                        state.turn_phase.turn_started()
                    } else {
                        state.waiting_since
                    }
                },
                // Same sid resolution as `order_sid`: its own sid, else the
                // `/clear` predecessor whose tags it inherits.
                tags: self
                    .sessions
                    .sid_of(id)
                    .map(|s| s.as_str().to_string())
                    .or_else(|| self.jump_order_succession.get(&id).cloned())
                    .and_then(|sid| self.session_tags.get(&sid).cloned())
                    .unwrap_or_default(),
            });
        }
        // Order the COMBINED list (roster + local-only) by label, so a session sits
        // in the SAME slot whether or not the async roster refresh has caught up to
        // it yet. Otherwise a freshly-created local session renders LAST (appended
        // above), then HOPS into its label-sorted slot once the roster catches up —
        // the "sessions spontaneously reorder for some weird reason" bug (bug-0006).
        // Stable tiebreak by target key keeps equal labels deterministic.
        rows.sort_by(|a, b| {
            a.label
                .cmp(&b.label)
                .then_with(|| jump_target_key(&a.target).cmp(&jump_target_key(&b.target)))
        });
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
    /// server sid)`. `(None, _)` when the focused tile is a buffer or an detached
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

/// One stable tile row in the ownership tree. Agent metadata remains the same
/// `AgentRow` projection used by the activity tabs, so provider, status,
/// archive, summary, and ordering signals do not fork from server truth.
#[derive(Clone)]
pub(crate) struct JumpTileRow {
    pub(crate) id: workspace::WindowId,
    pub(crate) render_index: usize,
    pub(crate) label: String,
    pub(crate) tags: Vec<String>,
    pub(crate) active: bool,
    pub(crate) agent: Option<AgentRow>,
}

/// A collapsible workspace folder and the tiles it exclusively owns.
pub(crate) struct JumpWorkspaceFolder {
    pub(crate) index: usize,
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) active: bool,
    pub(crate) tiles: Vec<JumpTileRow>,
}

/// One rendered project section in the jump panel (UXI-Project-3): a project's
/// name + cwd, collapsible workspace folders, and its Detached tile collection.
/// The legacy flat fields remain during the compatibility transition for pure
/// ordering tests; production paint consumes `workspace_folders` / `detached`.
pub(crate) struct JumpProjectSection {
    pub(crate) id: ProjectId,
    pub(crate) name: String,
    pub(crate) cwd_display: String,
    pub(crate) agent_tab: JumpAgentTab,
    /// Live non-archived totals for the two operational tabs.
    pub(crate) waiting_count: usize,
    pub(crate) working_count: usize,
    /// `(global workspace idx, label, is-active)` — idx+1 is the `ctrl-<n>` number.
    pub(crate) workspaces: Vec<(usize, String, bool)>,
    /// `(flat row index, row)` — the flat index is the stable listener key.
    pub(crate) sessions: Vec<(usize, AgentRow)>,
    pub(crate) workspace_folders: Vec<JumpWorkspaceFolder>,
    pub(crate) detached: Vec<JumpTileRow>,
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
    ) -> (
        Vec<JumpProjectSection>,
        Vec<(String, Vec<(usize, AgentRow)>)>,
    ) {
        self.jump_panel_sections_with_tab(cx, None)
    }

    /// Alternate projection used by Cmd-P: force each project's tab without
    /// mutating the per-project UI selection. This keeps the palette's
    /// candidate set stable even while the visible panel is on a filtered tab.
    pub(crate) fn jump_panel_sections_with_tab(
        &self,
        cx: &gpui::App,
        forced_tab: Option<JumpAgentTab>,
    ) -> (
        Vec<JumpProjectSection>,
        Vec<(String, Vec<(usize, AgentRow)>)>,
    ) {
        let rows = self.jump_panel_agent_rows(cx);
        let tile_agent_rows = rows.clone();
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
            let pid = group
                .first()
                .and_then(|(_, r)| self.projects.by_cwd(&r.cwd));
            match pid {
                Some(id) => by_project.entry(id).or_default().extend(group),
                None => {
                    let visible: Vec<_> =
                        group.into_iter().filter(|(_, row)| !row.archived).collect();
                    if !visible.is_empty() {
                        unfiled.push((cwd_label, visible));
                    }
                }
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
                    (
                        idx,
                        t.display_label().to_string(),
                        idx == self.workspace.active_workspace,
                    )
                })
                .collect();
            let selected_tab = self.jump_agent_tabs.get(&id).copied().unwrap_or_default();
            let agent_tab = forced_tab.unwrap_or(selected_tab);
            let project_rows = by_project.remove(&id).unwrap_or_default();
            let sessions = agent_rows_for_tab(project_rows, agent_tab);
            let workspace_folders: Vec<JumpWorkspaceFolder> = self
                .workspace
                .workspaces
                .iter()
                .enumerate()
                .filter(|(_, wsp)| wsp.project() == id)
                .map(|(index, wsp)| {
                    let mut tiles = Vec::new();
                    wsp.layout.for_each_leaf(&mut |window| {
                        tiles.push(self.jump_tile_row(window, &tile_agent_rows, cx));
                    });
                    for hidden in &wsp.hidden_tiles {
                        tiles.push(self.jump_tile_row(&hidden.window, &tile_agent_rows, cx));
                    }
                    JumpWorkspaceFolder {
                        index,
                        key: Self::workspace_fold_key(&p.name, &wsp.auto_name),
                        label: wsp.display_label().to_string(),
                        active: self.workspace.presented_tile().is_none()
                            && self.workspace.active_workspace == index,
                        tiles,
                    }
                })
                .collect();
            let mut detached: Vec<JumpTileRow> = self
                .workspace
                .detached_tiles
                .iter()
                .filter(|tile| tile.project() == id)
                .map(|tile| self.jump_tile_row(&tile.window, &tile_agent_rows, cx))
                .filter(|tile| match (&tile.agent, agent_tab) {
                    (None, JumpAgentTab::All) => true,
                    (None, _) => false,
                    (Some(row), JumpAgentTab::Waiting) => {
                        !row.archived && row.activity() == AgentActivity::Waiting
                    }
                    (Some(row), JumpAgentTab::Working) => {
                        !row.archived && row.activity() == AgentActivity::Working
                    }
                    (Some(row), JumpAgentTab::All) => !row.archived,
                    (Some(row), JumpAgentTab::Archived) => row.archived,
                })
                .collect();
            detached.sort_by(|a, b| a.label.cmp(&b.label));
            let waiting_count = detached
                .iter()
                .filter_map(|tile| tile.agent.as_ref())
                .filter(|row| !row.archived && row.activity() == AgentActivity::Waiting)
                .count();
            let working_count = detached
                .iter()
                .filter_map(|tile| tile.agent.as_ref())
                .filter(|row| !row.archived && row.activity() == AgentActivity::Working)
                .count();
            sections.push(JumpProjectSection {
                id,
                name: p.name.clone(),
                cwd_display: shorten_cwd_for_display(&p.cwd),
                agent_tab: selected_tab,
                waiting_count,
                working_count,
                workspaces,
                sessions,
                workspace_folders,
                detached,
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

    fn jump_tile_row(
        &self,
        window: &workspace::Window<App>,
        agent_rows: &[AgentRow],
        cx: &gpui::App,
    ) -> JumpTileRow {
        let mut agent_match = match &window.content {
            App::Agent(tile) => {
                let local = tile.session();
                let remembered = tile.remembered_sid(|id| self.sessions.sid_of(id).cloned());
                agent_rows
                    .iter()
                    .enumerate()
                    .find(|(_, row)| match &row.target {
                        JumpTarget::Local(id) => Some(*id) == local,
                        JumpTarget::Roster(sid) => remembered
                            .as_ref()
                            .is_some_and(|known| known.as_str() == sid),
                    })
                    .map(|(index, row)| (index, row.clone()))
            }
            _ => None,
        };
        let tags: Vec<String> = window.tags.iter().cloned().collect();
        let render_index = agent_match
            .as_ref()
            .map(|(index, _)| *index)
            .unwrap_or(window.id() as usize);
        let mut agent = agent_match.take().map(|(_, row)| row);
        if let Some(row) = &mut agent {
            row.tags = tags.clone();
        }
        let label = agent
            .as_ref()
            .map(|row| row.label.clone())
            .unwrap_or_else(|| Self::desktop_tile_title(&self.sessions, &window.content, cx));
        JumpTileRow {
            id: window.id(),
            render_index,
            label,
            tags,
            active: self.workspace.focused_window_id() == Some(window.id()),
            agent,
        }
    }
}

/// Apply one project's selected agent-state tab. Waiting and Working are live
/// queues sorted by when each row entered that state (oldest first, newest
/// last). All and Archived preserve the incoming custom order exactly.
pub(crate) fn agent_rows_for_tab(
    mut rows: Vec<(usize, AgentRow)>,
    tab: JumpAgentTab,
) -> Vec<(usize, AgentRow)> {
    match tab {
        JumpAgentTab::Waiting => {
            rows.retain(|(_, row)| !row.archived && row.activity() == AgentActivity::Waiting);
            rows.sort_by_key(|(_, row)| row.state_entered_at);
        }
        JumpAgentTab::Working => {
            rows.retain(|(_, row)| !row.archived && row.activity() == AgentActivity::Working);
            rows.sort_by_key(|(_, row)| row.state_entered_at);
        }
        JumpAgentTab::All => rows.retain(|(_, row)| !row.archived),
        JumpAgentTab::Archived => rows.retain(|(_, row)| row.archived),
    }
    rows
}

/// Prepare a tab's rows for paint. All is a stable activity partition over the
/// durable custom order: Working first, Waiting second, then exceptional
/// Unavailable. Other tabs remain one unheaded list.
pub(crate) fn agent_row_groups_for_tab(
    rows: Vec<(usize, AgentRow)>,
    tab: JumpAgentTab,
) -> Vec<(Option<AgentActivity>, Vec<(usize, AgentRow)>)> {
    if tab != JumpAgentTab::All {
        return (!rows.is_empty())
            .then_some((None, rows))
            .into_iter()
            .collect();
    }
    let mut working = Vec::new();
    let mut waiting = Vec::new();
    let mut unavailable = Vec::new();
    for row in rows {
        match row.1.activity() {
            AgentActivity::Working => working.push(row),
            AgentActivity::Waiting => waiting.push(row),
            AgentActivity::Unavailable => unavailable.push(row),
        }
    }
    [
        (AgentActivity::Working, working),
        (AgentActivity::Waiting, waiting),
        (AgentActivity::Unavailable, unavailable),
    ]
    .into_iter()
    .filter_map(|(activity, rows)| (!rows.is_empty()).then_some((Some(activity), rows)))
    .collect()
}

/// Split a tab's rows into tag folders + an untagged residual (UXI-JumpPanel-20).
/// Each row appears under EVERY tag it carries (multi-appearance); a row with no
/// tags falls to the untagged list, rendered flat below the folders. Folders are
/// ordered by the project's manual `tag_order` (a stable sort by rank), any tag
/// not listed sorting after alphabetically. The incoming row order is preserved
/// WITHIN each folder and within untagged, so the caller's sort carries through
/// (chronological for Waiting/Working, by-label for All). Pure so the grouping is
/// headlessly testable in isolation.
pub(crate) fn partition_rows_by_tag(
    rows: Vec<(usize, AgentRow)>,
    tag_order: &[String],
) -> (
    Vec<(String, Vec<(usize, AgentRow)>)>,
    Vec<(usize, AgentRow)>,
) {
    let mut folders: std::collections::BTreeMap<String, Vec<(usize, AgentRow)>> =
        std::collections::BTreeMap::new();
    let mut untagged: Vec<(usize, AgentRow)> = Vec::new();
    for (i, row) in rows {
        if row.tags.is_empty() {
            untagged.push((i, row));
            continue;
        }
        // A row appears once per DISTINCT tag it carries.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for tag in row.tags.iter() {
            if seen.insert(tag.as_str()) {
                folders
                    .entry(tag.clone())
                    .or_default()
                    .push((i, row.clone()));
            }
        }
    }
    // BTreeMap yields alpha order (the default); a stable sort by manual rank
    // floats the user's ordered tags to the top, unlisted ones keep alpha after.
    let mut folders: Vec<(String, Vec<(usize, AgentRow)>)> = folders.into_iter().collect();
    let rank = |tag: &str| {
        tag_order
            .iter()
            .position(|t| t == tag)
            .unwrap_or(usize::MAX)
    };
    folders.sort_by_key(|(tag, _)| rank(tag));
    (folders, untagged)
}

/// Tile-native twin of `partition_rows_by_tag`. Tags live on the stable tile,
/// so the same grouping works for Agent and non-Agent detached rows.
pub(crate) fn partition_tiles_by_tag(
    rows: Vec<JumpTileRow>,
    tag_order: &[String],
) -> (Vec<(String, Vec<JumpTileRow>)>, Vec<JumpTileRow>) {
    let mut folders: std::collections::BTreeMap<String, Vec<JumpTileRow>> =
        std::collections::BTreeMap::new();
    let mut untagged = Vec::new();
    for row in rows {
        if row.tags.is_empty() {
            untagged.push(row);
            continue;
        }
        for tag in &row.tags {
            folders.entry(tag.clone()).or_default().push(row.clone());
        }
    }
    let mut folders: Vec<_> = folders.into_iter().collect();
    let rank = |tag: &str| {
        tag_order
            .iter()
            .position(|t| t == tag)
            .unwrap_or(usize::MAX)
    };
    folders.sort_by_key(|(tag, _)| rank(tag));
    (folders, untagged)
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
    let cwd_rank = |key: &str| {
        cwd_order
            .iter()
            .position(|k| k.as_str() == key)
            .unwrap_or(usize::MAX)
    };
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
    let to = v
        .iter()
        .position(|x| x == target)
        .unwrap_or(from.min(v.len()));
    v.insert(to, dragged.to_string());
}

impl YaldaGpuiView {
    /// Append newly discovered server sessions to the durable All order without
    /// disturbing any existing slot. On the first roster seed this freezes the
    /// historical by-label default; every later Created event appends exactly
    /// one sid at the bottom.
    pub(crate) fn append_new_jump_sessions(
        &mut self,
        sids: impl IntoIterator<Item = String>,
    ) -> bool {
        let mut changed = false;
        for sid in sids {
            if !self.jump_session_order.contains(&sid) {
                self.jump_session_order.push(sid);
                changed = true;
            }
        }
        changed
    }

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
        let grouped = order_grouped_rows(
            group_agent_rows_by_cwd(rows),
            &self.jump_cwd_order,
            &self.jump_session_order,
        );
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
    pub(crate) fn reorder_session(&mut self, dragged: &str, target: &str, cx: &mut Context<Self>) {
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
        let grouped = order_grouped_rows(
            group_agent_rows_by_cwd(rows),
            &self.jump_cwd_order,
            &self.jump_session_order,
        );
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
    /// Request a durable cold-storage transition for one server-backed session.
    /// The server persists the state and releases/recreates its runtime
    /// resources; `apply_session_archived_local` mirrors the acknowledged state
    /// into this GUI's navigation projections.
    ///
    /// UXI-JumpPanel-18: a real toggle announces itself — one `Info` console
    /// line naming the agent, plus a `TurnId::System` transcript notice when
    /// this GUI has the session open. A no-op toggle is silent (the early
    /// return below is what makes that true for both command surfaces at once).
    pub(crate) fn set_session_archived(
        &mut self,
        sid: &str,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        if self.jump_archived_sessions.contains(sid) == archived {
            return;
        }
        let Some(handle) = self.session_server.as_ref().map(|server| server.handle()) else {
            // Hermetic/legacy fallback: there is no daemon authority to ask.
            self.apply_session_archived_local(sid, archived, cx);
            return;
        };
        let sid = sid.to_string();
        cx.spawn(async move |this, cx| {
            let request_sid = sid.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle
                        .set_archived(&request_sid, archived)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    // The broadcast normally arrives first, but applying here
                    // also covers a reconnect between ACK and notification.
                    this.apply_session_archived_local(&sid, archived, cx);
                }
                Err(error) => this.append_system_console(
                    ConsoleLevel::Error,
                    format!(
                        "could not {} agent session: {error}",
                        if archived { "archive" } else { "unarchive" }
                    ),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Apply server-authoritative lifecycle state without issuing another wire
    /// request. Shared by the request completion and broadcast paths; idempotent
    /// so their normal race produces exactly one announcement.
    pub(crate) fn apply_session_archived_local(
        &mut self,
        sid: &str,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = if archived {
            self.jump_archived_sessions.insert(sid.to_string())
        } else {
            self.jump_archived_sessions.remove(sid)
        };
        if !changed {
            return;
        }
        self.announce_session_archived(sid, archived, cx);
        // Archiving detaches the complete Agent tile so its transcript and tags
        // remain reachable from Archived. Empty workspaces are valid.
        if archived
            && let Some(local) = self.sessions.locate(&ServerSid::new(sid.to_string()))
            && let Some(tile) = self.agent_tile_id_for_session(local)
            && self.workspace.detach_window(tile).is_ok()
        {
            self.workspace.clear_solo_presentation();
            self.save_agent_ring(cx);
        }
        self.save_settings();
        cx.notify();
    }

    /// The UXI-JumpPanel-18 announcement for one real archive-flag change.
    /// Split out so the mutator above stays a plain durable-flag edit.
    fn announce_session_archived(&mut self, sid: &str, archived: bool, cx: &mut Context<Self>) {
        let verb = if archived { "archived" } else { "unarchived" };
        // The live store name is authoritative when this GUI holds the session
        // (a rename lands there first); the roster is the fallback that covers
        // every session we have never opened.
        let opened = self.sessions.locate(&ServerSid::new(sid.to_string()));
        let label = opened
            .and_then(|id| self.sessions.get(id))
            .map(|e| e.read(cx).label.clone())
            .or_else(|| self.agent_roster.get(sid).map(|info| info.label.clone()))
            .unwrap_or_else(|| sid.to_string());
        self.append_system_console(
            ConsoleLevel::Info,
            format!("{verb} agent session \"{label}\""),
            cx,
        );
        // A roster-only session has no in-memory transcript to write into; the
        // console line above is its whole announcement (deliberate scope
        // boundary — this notice is a local view event, not server transcript).
        if let Some(id) = opened {
            self.with_session(id, cx, |state| {
                Self::append_system_notice(state, &format!("session {verb}"));
            });
        }
    }

    /// Contextual `<space>` action for the focused agent tile. Sid-less
    /// placeholders are intentionally a no-op; the menu disables this command.
    pub(crate) fn set_focused_session_archived(&mut self, archived: bool, cx: &mut Context<Self>) {
        let Some(sid) = self.active_server_session_id() else {
            return;
        };
        self.set_session_archived(&sid, archived, cx);
    }

    /// Select the agent-state slice for one project. `All` is represented by an
    /// absent entry, keeping the runtime map sparse.
    pub(crate) fn select_jump_agent_tab(
        &mut self,
        project: ProjectId,
        tab: JumpAgentTab,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .jump_agent_tabs
            .get(&project)
            .copied()
            .unwrap_or_default();
        if current == tab {
            return;
        }
        if tab == JumpAgentTab::All {
            self.jump_agent_tabs.remove(&project);
        } else {
            self.jump_agent_tabs.insert(project, tab);
        }
        cx.notify();
    }

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

    pub(crate) fn workspace_fold_key(project: &str, auto_name: &str) -> String {
        format!("{project}\u{1f}{auto_name}")
    }

    pub(crate) fn toggle_workspace_fold(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.jump_folded_workspaces.remove(key) {
            self.jump_folded_workspaces.insert(key.to_string());
        }
        self.save_settings();
        cx.notify();
    }

    /// The composite key a tag folder folds by (`"{project}\u{1f}{tag}"`,
    /// UXI-JumpPanel-21) — `\u{1f}` (unit separator) can't appear in a project
    /// name or tag, so the join is unambiguous.
    pub(crate) fn tag_fold_key(project: &str, tag: &str) -> String {
        format!("{project}\u{1f}{tag}")
    }

    /// Is this project's tag folder folded (UXI-JumpPanel-21)?
    pub(crate) fn tag_folder_folded(&self, project: &str, tag: &str) -> bool {
        self.jump_folded_tags
            .contains(&Self::tag_fold_key(project, tag))
    }

    /// Fold or unfold one project's tag folder (UXI-JumpPanel-21). Keyed by
    /// durable project name + tag, persisted like `jump_folded_projects`.
    pub(crate) fn toggle_tag_fold(&mut self, project: &str, tag: &str, cx: &mut Context<Self>) {
        let key = Self::tag_fold_key(project, tag);
        if !self.jump_folded_tags.remove(&key) {
            self.jump_folded_tags.insert(key);
        }
        self.save_settings();
        cx.notify();
    }

    /// The tags currently present across a project's detached tiles, in the
    /// user's manual order (`jump_tag_order[project]`, then alphabetical for
    /// unlisted tags). Used by the reorder to rebuild a total order over the tags
    /// actually shown. Pure read.
    pub(crate) fn ordered_project_tags(&self, project: &str, cx: &gpui::App) -> Vec<String> {
        let _ = cx;
        let mut present: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for tile in &self.workspace.detached_tiles {
            if self.projects.name_of(tile.project()) == project {
                present.extend(tile.window.tags.iter().cloned());
            }
        }
        let order = self
            .jump_tag_order
            .get(project)
            .cloned()
            .unwrap_or_default();
        let mut tags: Vec<String> = present.into_iter().collect();
        let rank = |t: &str| order.iter().position(|x| x == t).unwrap_or(usize::MAX);
        tags.sort_by_key(|t| rank(t));
        tags
    }

    /// Reorder tag folder `dragged` to `target`'s slot within `project`
    /// (UXI-JumpPanel-21). Tags are project-scoped, so the reorder is confined to
    /// one project: both tags must be present in it or the drag is refused (the
    /// cross-project guard, mirroring `reorder_session`'s cwd gate). Rebuilds that
    /// project's `jump_tag_order` entry over the tags currently shown, in present
    /// display order, then persists + notifies.
    pub(crate) fn reorder_tag(
        &mut self,
        project: &str,
        dragged: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        if dragged == target {
            return;
        }
        let mut tags = self.ordered_project_tags(project, cx);
        // Cross-project guard: a tag not present in this project can't be moved here.
        if !tags.iter().any(|t| t == dragged) || !tags.iter().any(|t| t == target) {
            return;
        }
        reorder_move(&mut tags, dragged, target);
        let entry = self.jump_tag_order.entry(project.to_string()).or_default();
        if *entry == tags {
            return;
        }
        *entry = tags;
        self.save_settings();
        cx.notify();
    }

    /// Build the jump-panel sidebar element (inline; see the module note).
    /// Reads workspaces + agent sessions + theme directly off `self`; row clicks
    /// re-enter through `cx.listener` and resolve their target id/index in the
    /// handler (never closed over from a prior build).
    pub(crate) fn render_jump_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        record_render("jump_panel");
        // Supporting copy in the panel stays on the theme's cool prose palette.
        // Never inherit `warm_accent`: Folio's gold and Nightfox's very dark
        // `dim` both failed as readable navigation text.
        let supporting_text = nc(jump_supporting_text_color(&self.theme.agent));
        let selection_mark = nc(self.theme.overlay.border);
        let st = DetailStyle {
            fg: self.editor_fg(),
            dim: nc(self.theme.agent.dim),
            accent: supporting_text,
            err: nc(self.theme.agent.jump_header),
            mono: self.code_font.clone(),
            prose: self.body_font.clone(),
            base: px(13.0),
            pt: 13.0,
        };
        // Selection is deliberately neutral gray. Operational state owns the
        // saturated hues: orange means working and green means ready for input.
        let sel_bg = nc(jump_selection_color(&self.theme.overlay));
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
        // Green means ready for input across every connected non-working row.
        let ready = nc(jump_agent_status_color(
            &self.theme.agent,
            AgentDotStatus::WaitingForYou,
        ));
        // Header palette (theme-owned; `UXI-JumpPanel-7`): top-level section
        // headers use `st.err` (= `agent.jump_header`); per-cwd "Unfiled"
        // subheaders use `agent.jump_subheader`.
        let electric: Hsla = nc(self.theme.agent.jump_subheader);
        // "Working" status star (a reply in flight) — `agent.jump_working`, warm
        // and distinct from the gold `warm_accent`.
        let working_orange: Hsla = nc(jump_agent_status_color(
            &self.theme.agent,
            AgentDotStatus::Working,
        ));

        // The active screen element for the neutral selected treatment
        // (UXI-JumpPanel-5): the session bound to the focused tile.
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

        // ── System console (UXI-SystemConsole-1) ─────────────────────────────
        // This occupies the former empty PINNED slot. It is global operational
        // chrome, so it never carries the active-workspace selection mark.
        col = col.child(section_heading("System", &st).px_3().text_color(st.err));
        col = col.child(probe_bounds(
            "jump-system-console",
            div()
                .id("jump-system-console")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .cursor_pointer()
                .hover(|s| s.bg(sel_bg))
                .text_color(st.fg)
                .font_family(st.mono.clone())
                .text_size(px(st.pt))
                .child(
                    div()
                        .text_color(st.err)
                        .font_weight(FontWeight::BOLD)
                        .child(SharedString::new_static("▾")),
                )
                .child(SharedString::new_static("System console"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.open_system_console(cx);
                }))
                .into_any_element(),
        ));

        // ── Per-project sections (UXI-Project-3, UXI-JumpPanel-7): one section
        // per project. A **hairline rule** separates sections (drawn ABOVE each
        // header); the header is the project NAME only (bold uppercase) — its cwd
        // subtext, the ✕ delete affordance, and the inline ＋create rows are gone.
        // Clicking the name opens a **context menu** (New workspace / New agent
        // session / Delete project; UXI-JumpPanel-8). Each section owns its
        // WORKSPACES sublist (workspaces whose `wsp.project()` is it; the ctrl-<n>
        // number moves to a dim right-edge hint) and its UNBOUND tiles. Bound
        // tiles are children of workspace folders; detached tiles live below the
        // activity tabs and optional tag folders. See `jump_panel_sections`.
        let (sections, unfiled) = self.jump_panel_sections(cx);
        let drag_fg = st.fg;
        let drag_font = st.mono.clone();

        for section in sections {
            let pid = section.id;
            let agent_tab = section.agent_tab;
            let waiting_count = section.waiting_count;
            let working_count = section.working_count;
            let cwd_key = section.cwd_display.clone();
            let project_name = section.name.clone();
            let folded = self.jump_folded_projects.contains(&project_name);
            // Inter-section rule, above the header (System always precedes the
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
                        .on_click(
                            cx.listener(move |this, ev: &gpui::ClickEvent, _window, cx| {
                                let p = ev.position();
                                this.open_project_menu(pid, (f32::from(p.x), f32::from(p.y)), cx);
                            }),
                        )
                        .on_drag(
                            CwdDrag {
                                cwd_key: cwd_key.clone(),
                            },
                            {
                                let (fg, bg, font) = (drag_fg, sel_bg, drag_font.clone());
                                move |_payload, _pos, _window, cx| {
                                    cx.new(|_| JumpDragPreview {
                                        label: drag_label.clone(),
                                        fg,
                                        bg,
                                        font: font.clone(),
                                    })
                                }
                            },
                        ),
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

            // Every workspace is now an independently collapsible folder. Its
            // children are exactly the tiles owned by that workspace.
            for folder in section.workspace_folders {
                let idx = folder.index;
                let key = folder.key.clone();
                let folder_folded = self.jump_folded_workspaces.contains(&key);
                let num = format!("{}", idx + 1);
                let label = folder.label.clone();
                let header = div()
                    .id(SharedString::from(format!("jump-ws-{idx}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px_3()
                    .py_1()
                    .text_size(st.base)
                    .hover(|s| s.bg(sel_bg))
                    .child(
                        div()
                            .id(SharedString::from(format!("jump-ws-fold-{idx}")))
                            .w(px(18.0))
                            .flex_none()
                            .cursor_pointer()
                            .text_color(st.dim)
                            .child(SharedString::new_static(if folder_folded {
                                "▸"
                            } else {
                                "▾"
                            }))
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.toggle_workspace_fold(&key, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("jump-ws-label-{idx}")))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .text_color(if folder.active { selection_mark } else { st.fg })
                            .font_family(st.mono.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(format!("⊞ {label}")))
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.select_workspace(idx, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(st.dim)
                            .font_family(st.mono.clone())
                            .text_size(px(st.pt * 0.75))
                            .child(SharedString::from(num)),
                    );
                col = col.child(probe_bounds_dyn(
                    format!("jump-workspace-row-{idx}"),
                    header.into_any_element(),
                ));
                if !folder_folded {
                    let mut children = div()
                        .id(SharedString::from(format!("jump-ws-children-{idx}")))
                        .flex()
                        .flex_col()
                        .w_full()
                        .ml(px(20.0))
                        .border_l_1()
                        .border_color(divider_color)
                        .pl(px(2.0));
                    for tile in &folder.tiles {
                        let suffix = if tile.agent.is_some() {
                            String::new()
                        } else {
                            format!("-ws{idx}")
                        };
                        children = children.child(jump_tile_row_el(
                            tile,
                            &suffix,
                            &st,
                            sel_bg,
                            selection_mark,
                            ready,
                            working_orange,
                            drag_fg,
                            drag_font.clone(),
                            supporting_text,
                            cx,
                        ));
                    }
                    col = col.child(children);
                }
            }

            // Per-project state tabs sit directly under the workspace list.
            // Their selection is independent across projects.
            let tab_edge = border;
            let mut tabs = div()
                .flex()
                .flex_col()
                .w_full()
                .p(px(2.0))
                .border_1()
                .border_color(tab_edge)
                .rounded_md();
            for (row_idx, row_tabs) in [
                [JumpAgentTab::Waiting, JumpAgentTab::Working],
                [JumpAgentTab::All, JumpAgentTab::Archived],
            ]
            .into_iter()
            .enumerate()
            {
                if row_idx > 0 {
                    tabs = tabs.child(div().h(px(1.0)).mx_1().bg(tab_edge));
                }
                let mut row = div().flex().flex_row().w_full();
                for (tab_idx, tab) in row_tabs.into_iter().enumerate() {
                    if tab_idx > 0 {
                        row = row.child(div().w(px(1.0)).my_1().bg(tab_edge));
                    }
                    let tab_probe =
                        format!("jump-agent-tab-{}-{}", pid.0, tab.label().to_lowercase());
                    let indicator = match tab {
                        JumpAgentTab::Waiting => Some(("waiting", waiting_count, ready)),
                        JumpAgentTab::Working => Some(("working", working_count, working_orange)),
                        JumpAgentTab::All | JumpAgentTab::Archived => None,
                    }
                    .map(|(slug, count, tint)| {
                        let probe = format!("jump-agent-tab-count-{}-{slug}", pid.0);
                        let indicator = compact_count_indicator(
                            SharedString::from(probe.clone()),
                            count,
                            tint,
                            &st,
                        );
                        probe_bounds_dyn(probe, indicator.into_any_element())
                    });
                    let button = compact_tab(
                        SharedString::from(tab_probe.clone()),
                        tab.label(),
                        indicator,
                        tab == agent_tab,
                        sel_bg,
                        &st,
                    )
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.select_jump_agent_tab(pid, tab, cx)
                    }));
                    row = row.child(probe_bounds_dyn(tab_probe, button.into_any_element()));
                }
                tabs = tabs.child(row);
            }
            col = col.child(div().w_full().px_3().pt(px(10.0)).pb(px(6.0)).child(
                probe_bounds_dyn(
                    format!("jump-agent-tabs-{}", pid.0),
                    tabs.into_any_element(),
                ),
            ));

            // DETACHED is tile-native: attached tiles cannot enter it,
            // non-Agent tiles participate, and tag
            // folders read the tags carried by each stable tile.
            let proj_name = section.name.clone();
            col = col.child(
                div()
                    .w_full()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_color(electric)
                    .font_family(st.mono.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(st.pt * 0.82))
                    .child(SharedString::new_static("DETACHED")),
            );
            let tag_order = self
                .jump_tag_order
                .get(&proj_name)
                .cloned()
                .unwrap_or_default();
            let (folders, untagged) = partition_tiles_by_tag(section.detached, &tag_order);
            let had_folders = !folders.is_empty();
            for (folder_idx, (tag, rows)) in folders.into_iter().enumerate() {
                let folder_folded = self.tag_folder_folded(&proj_name, &tag);
                let project_for_fold = proj_name.clone();
                let tag_for_fold = tag.clone();
                let header = div()
                    .id(SharedString::from(format!(
                        "jump-detached-tag-folder-{}-{folder_idx}",
                        pid.0
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .pl(px(20.0))
                    .pr_3()
                    .py_1()
                    .cursor_pointer()
                    .text_color(electric)
                    .font_family(st.mono.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(st.pt * 0.82))
                    .child(SharedString::from(format!(
                        "{} 🏷 {}  {}",
                        if folder_folded { "▸" } else { "▾" },
                        tag,
                        rows.len()
                    )))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.toggle_tag_fold(&project_for_fold, &tag_for_fold, cx)
                    }));
                col = col.child(probe_bounds_dyn(
                    format!("jump-tag-folder-{}-{folder_idx}", pid.0),
                    header.into_any_element(),
                ));
                if !folder_folded {
                    let mut body = div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .ml(px(26.0))
                        .border_l_1()
                        .border_color(divider_color)
                        .pl(px(2.0));
                    for tile in &rows {
                        body = body.child(jump_tile_row_el(
                            tile,
                            &format!("-tg{folder_idx}"),
                            &st,
                            sel_bg,
                            selection_mark,
                            ready,
                            working_orange,
                            drag_fg,
                            drag_font.clone(),
                            supporting_text,
                            cx,
                        ));
                    }
                    col = col.child(body);
                }
            }
            if had_folders && !untagged.is_empty() {
                col = col.child(probe_bounds_dyn(
                    format!("jump-untagged-sep-{}", pid.0),
                    div()
                        .w_full()
                        .pl(px(20.0))
                        .pr_3()
                        .py_1()
                        .text_color(st.dim)
                        .font_family(st.mono.clone())
                        .text_size(px(st.pt * 0.72))
                        .child(SharedString::new_static("untagged"))
                        .into_any_element(),
                ));
            }
            for tile in &untagged {
                col = col.child(jump_tile_row_el(
                    tile,
                    "",
                    &st,
                    sel_bg,
                    selection_mark,
                    ready,
                    working_orange,
                    drag_fg,
                    drag_font.clone(),
                    supporting_text,
                    cx,
                ));
            }

            // Compatibility-only legacy session renderer. Kept typechecked
            // while older pure tests are migrated, but never enters production
            // paint: every session is represented by its stable tile above.
            if false {
                let render_flat_row =
                    |col: gpui::Stateful<gpui::Div>,
                     i: usize,
                     row: &AgentRow,
                     suffix: &str,
                     allow_drag: bool,
                     cx: &mut Context<Self>| {
                        let active =
                            jump_target_is_active(&row.target, active_local, active_sid.as_deref());
                        col.child(jump_session_row_el(
                            i,
                            row,
                            suffix,
                            &st,
                            sel_bg,
                            selection_mark,
                            ready,
                            working_orange,
                            active,
                            drag_fg,
                            drag_font.clone(),
                            allow_drag,
                            supporting_text,
                            cx,
                        ))
                    };
                if agent_tab == JumpAgentTab::Archived {
                    for (i, row) in section.sessions {
                        col = render_flat_row(col, i, &row, "", false, cx);
                    }
                } else {
                    let mut rows = section.sessions;
                    // All drops the activity sub-headers and SORTS by label
                    // (UXI-JumpPanel-20 clause 5); Waiting/Working keep chronology.
                    if agent_tab == JumpAgentTab::All {
                        rows.sort_by(|(_, a), (_, b)| a.label.cmp(&b.label));
                    }
                    let tag_order = self
                        .jump_tag_order
                        .get(&proj_name)
                        .cloned()
                        .unwrap_or_default();
                    let (folders, untagged) = partition_rows_by_tag(rows, &tag_order);
                    let had_folders = !folders.is_empty();
                    for (folder_idx, (tag, folder_rows)) in folders.into_iter().enumerate() {
                        let folded = self.tag_folder_folded(&proj_name, &tag);
                        let folder_count = folder_rows.len();
                        let probe = format!("jump-tag-folder-{}-{folder_idx}", pid.0);
                        // Folder header: chevron (folds) + tag name + count. The label
                        // is the drag source + drop target for reorder (UXI-JumpPanel-21).
                        let tag_for_fold = tag.clone();
                        let tag_for_drag = tag.clone();
                        let proj_for_fold = proj_name.clone();
                        let proj_for_drag = proj_name.clone();
                        let proj_for_drop = proj_name.clone();
                        let drag_label: SharedString = tag.clone().into();
                        let header = div()
                            .id(SharedString::from(format!(
                                "jump-tagfold-{}-{folder_idx}",
                                pid.0
                            )))
                            .flex()
                            .flex_row()
                            .items_center()
                            .w_full()
                            .pl(px(20.0))
                            .pr_3()
                            .pt_1()
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "jump-tagchev-{}-{folder_idx}",
                                        pid.0
                                    )))
                                    .w(px(16.0))
                                    .flex_none()
                                    .cursor_pointer()
                                    .text_color(st.dim)
                                    .child(SharedString::new_static(if folded {
                                        "▸"
                                    } else {
                                        "▾"
                                    }))
                                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                                        this.toggle_tag_fold(&proj_for_fold, &tag_for_fold, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "jump-tagname-{}-{folder_idx}",
                                        pid.0
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    // Tag name in the grouping/subheader blue so a
                                    // folder reads as a header, not a session row.
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(electric)
                                            .font_family(st.mono.clone())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(px(st.pt * 0.82))
                                            .child(SharedString::from(format!("🏷 {tag}"))),
                                    )
                                    // The count, quiet.
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(st.dim)
                                            .font_family(st.mono.clone())
                                            .text_size(px(st.pt * 0.72))
                                            .child(SharedString::from(folder_count.to_string())),
                                    )
                                    // A trailing hairline rule fills the row so the
                                    // header spans the panel and separates cleanly.
                                    .child(div().flex_1().h(px(1.0)).bg(divider_color))
                                    .on_drag(
                                        TagDrag {
                                            project: proj_for_drag.clone(),
                                            tag: tag_for_drag.clone(),
                                        },
                                        {
                                            let (fg, bg, font) =
                                                (drag_fg, sel_bg, drag_font.clone());
                                            move |_p, _pos, _window, cx| {
                                                cx.new(|_| JumpDragPreview {
                                                    label: drag_label.clone(),
                                                    fg,
                                                    bg,
                                                    font: font.clone(),
                                                })
                                            }
                                        },
                                    )
                                    .can_drop({
                                        let proj = proj_for_drag.clone();
                                        move |dragged, _window, _cx| {
                                            dragged
                                                .downcast_ref::<TagDrag>()
                                                .is_some_and(|d| d.project == proj)
                                        }
                                    })
                                    .drag_over::<TagDrag>(move |s, _, _, _| s.bg(sel_bg))
                                    .on_drop(cx.listener({
                                        let target_tag = tag.clone();
                                        move |this, dragged: &TagDrag, _window, cx| {
                                            this.reorder_tag(
                                                &proj_for_drop,
                                                &dragged.tag,
                                                &target_tag,
                                                cx,
                                            )
                                        }
                                    })),
                            );
                        col = col.child(probe_bounds_dyn(probe, header.into_any_element()));
                        if !folded {
                            let suffix = format!("-tg{folder_idx}");
                            // Wrap the folder's rows in an indented container with a
                            // left guide line, so they clearly read as children OF the
                            // tag header above them (UXI-JumpPanel-20).
                            let mut body = div()
                                .id(SharedString::from(format!(
                                    "jump-tagbody-{}-{folder_idx}",
                                    pid.0
                                )))
                                .flex()
                                .flex_col()
                                .w_full()
                                .ml(px(26.0))
                                .border_l_1()
                                .border_color(divider_color)
                                .pl(px(2.0));
                            for (i, row) in folder_rows {
                                body = render_flat_row(body, i, &row, &suffix, false, cx);
                            }
                            col = col.child(body);
                        }
                    }
                    // A labeled hairline separates the tagged folders from the loose
                    // untagged sessions below — only when both are present.
                    if had_folders && !untagged.is_empty() {
                        let sep = div()
                            .w_full()
                            .pl(px(20.0))
                            .pr_3()
                            .pt_2()
                            .pb_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(st.dim)
                                    .font_family(st.mono.clone())
                                    .text_size(px(st.pt * 0.72))
                                    .child(SharedString::new_static("untagged")),
                            )
                            .child(div().flex_1().h(px(1.0)).bg(divider_color));
                        col = col.child(probe_bounds_dyn(
                            format!("jump-untagged-sep-{}", pid.0),
                            sep.into_any_element(),
                        ));
                    }
                    // Untagged residual, flat, below the folders.
                    for (i, row) in untagged {
                        col = render_flat_row(col, i, &row, "", false, cx);
                    }
                }
            }
        }

        // ── Unfiled sessions (no project roots their cwd) ─ path headers.
        if false && !unfiled.is_empty() {
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
                        "",
                        &st,
                        sel_bg,
                        selection_mark,
                        ready,
                        working_orange,
                        active,
                        drag_fg,
                        drag_font.clone(),
                        true,
                        supporting_text,
                        cx,
                    ));
                }
            }
        }

        col.into_any_element()
    }
}

fn jump_tile_row_el(
    row: &JumpTileRow,
    suffix: &str,
    st: &DetailStyle,
    sel_bg: Hsla,
    selection_mark: Hsla,
    ready: Hsla,
    working_orange: Hsla,
    drag_fg: Hsla,
    drag_font: SharedString,
    supporting_text: Hsla,
    cx: &mut Context<YaldaGpuiView>,
) -> AnyElement {
    if let Some(agent) = &row.agent {
        return jump_session_row_el(
            row.render_index,
            agent,
            suffix,
            st,
            sel_bg,
            selection_mark,
            ready,
            working_orange,
            row.active,
            drag_fg,
            drag_font,
            false,
            supporting_text,
            cx,
        );
    }
    let id = row.id;
    probe_bounds_dyn(
        format!("jump-tile-row-{id}{suffix}"),
        jump_nav_row(
            SharedString::from(format!("jump-tile-{id}{suffix}")),
            &row.label,
            Some("◇"),
            None,
            None,
            st,
            sel_bg,
            row.active.then_some(selection_mark),
        )
        .on_click(cx.listener(move |this, _ev, _window, cx| this.jump_to_tile(id, cx)))
        .into_any_element(),
    )
}

/// Build one agent-session row (status dot + accent mark + drag) shared by the
/// per-project sections and the trailing Unfiled groups (UXI-Project-3), so the
/// status/active/drag semantics stay identical in both. `active` is the
/// precomputed "this row is the focused tile's bound session" mark
/// (UXI-JumpPanel-5). Only roster-backed rows (stable sid) participate in the
/// session drag-reorder.
#[allow(clippy::too_many_arguments)]
fn jump_session_row_el(
    i: usize,
    row: &AgentRow,
    // Disambiguates GPUI element ids when the same session appears under more
    // than one tag folder (UXI-JumpPanel-20). `""` for flat/untagged/archived
    // rows keeps the historical ids exactly (`jump-sess-{i}`, …).
    id_suffix: &str,
    st: &DetailStyle,
    sel_bg: Hsla,
    selection_mark: Hsla,
    ready: Hsla,
    working_orange: Hsla,
    active: bool,
    drag_fg: Hsla,
    drag_font: SharedString,
    allow_drag: bool,
    supporting_text: Hsla,
    cx: &mut Context<YaldaGpuiView>,
) -> gpui::AnyElement {
    // The agent-session icon is a `✦` whose COLOR carries the status (one glyph =
    // "this is an agent" + what it's doing): working (reply in flight) → orange;
    // ready for input (every connected non-working agent) → green; disconnected
    // or connecting → dim.
    let status = row.dot_status();
    let badge_color = match status {
        AgentDotStatus::Working => working_orange,
        AgentDotStatus::WaitingForYou => ready,
        AgentDotStatus::Neutral => st.dim,
    };
    let (badge_glyph, _) = agent_row_marks(status);
    let row_id = SharedString::from(format!("jump-sess-{i}{id_suffix}"));
    let target = row.target.clone();
    let mut r = jump_nav_row_hinted(
        row_id,
        &row.label,
        Some(badge_glyph),
        Some(badge_color),
        Some(format!("jump-session-status-mark-{i}{id_suffix}")),
        // UXI-JumpPanel-10: tabs + All-group headers already name activity.
        // Repeating "working" / "your turn" on every row is redundant noise.
        // UXI-JumpPanel-22: the trailing mark is provider identity, not state.
        Some(agent_provider_mark(row.provider)),
        Some(supporting_text),
        Some(format!(
            "jump-session-provider-{i}{id_suffix}-{}",
            row.provider.label()
        )),
        st,
        sel_bg,
        active.then_some(selection_mark),
    );
    if let Some(hue) = match status {
        AgentDotStatus::Working => Some(working_orange),
        AgentDotStatus::WaitingForYou => Some(ready),
        AgentDotStatus::Neutral => None,
    } {
        // State is a quiet wash, never a chip/bounding box. The selected row's
        // neutral gray wins so orange and green keep one unambiguous meaning.
        let mut tint = hue;
        tint.a = match status {
            AgentDotStatus::Working => 0.07,
            AgentDotStatus::WaitingForYou => 0.08,
            AgentDotStatus::Neutral => 0.0,
        };
        if !active {
            r = r.bg(tint);
        }
    }
    if !row.connected {
        r = r.text_color(st.dim);
    }
    r = r.on_click(cx.listener({
        let target = target.clone();
        move |this, _ev, _window, cx| this.jump_to_agent(target.clone(), cx)
    }));
    if allow_drag && let JumpTarget::Roster(sid) = &row.target {
        let sid = sid.clone();
        let cwd_key = shorten_cwd_for_display(&row.cwd);
        let label: SharedString = row.label.clone().into();
        r = r
            .on_drag(
                SessionDrag {
                    sid: sid.clone(),
                    cwd_key: cwd_key.clone(),
                },
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
    // italic cool-prose line. Chrome-class (fixed size, unaffected by document
    // zoom), single-line-height text indented to the label's x so it reads as a
    // subtitle of the row rather than a row of its own. A settled session with
    // no summary reserves no space; an in-flight one gets explicit feedback.
    let mut content = div()
        .id(SharedString::from(format!(
            "jump-session-wrap-{i}{id_suffix}"
        )))
        .flex()
        .flex_col()
        .w_full()
        .child(r)
        .on_mouse_down(
            MouseButton::Right,
            cx.listener({
                let target = target.clone();
                move |this, ev: &MouseDownEvent, _window, cx| {
                    this.open_session_menu(
                        target.clone(),
                        (f32::from(ev.position.x), f32::from(ev.position.y)),
                        cx,
                    );
                }
            }),
        );
    if let Some((summary, pending)) =
        if let Some(summary) = row.summary.as_ref().filter(|s| !s.trim().is_empty()) {
            Some((summary.clone(), false))
        } else if row.summary_pending {
            Some(("summarizing topic…".to_string(), true))
        } else {
            None
        }
    {
        let mut summary_color = supporting_text;
        if pending {
            summary_color.a = 0.72;
        }
        content = content.child(
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
                .text_color(summary_color)
                .child(SharedString::from(summary)),
        );
    }
    probe_bounds_dyn(
        format!("jump-session-row-{i}{id_suffix}"),
        content.into_any_element(),
    )
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
    jump_nav_row_hinted(
        id,
        label,
        badge,
        badge_color,
        None,
        hint,
        None,
        None,
        st,
        sel_bg,
        active,
    )
}

/// [`jump_nav_row`] with explicit colors and optional paint probes for the
/// leading badge and right-edge hint. Workspace `ctrl-<n>` digits stay dim;
/// Jump Panel session rows use the hint cell for provider identity while still
/// omitting redundant status words (`UXI-JumpPanel-10/-22`).
#[allow(clippy::too_many_arguments)]
fn jump_nav_row_hinted(
    id: impl Into<ElementId>,
    label: &str,
    badge: Option<&str>,
    badge_color: Option<Hsla>,
    badge_probe: Option<String>,
    hint: Option<&str>,
    hint_color: Option<Hsla>,
    hint_probe: Option<String>,
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
    let badge_el = div()
        .w(px(16.0))
        .flex_none()
        .text_color(badge_color.unwrap_or(st.dim))
        .child(SharedString::from(badge.unwrap_or("").to_string()));
    let badge_el = if let Some(probe) = badge_probe {
        probe_bounds_dyn(probe, badge_el.into_any_element())
    } else {
        badge_el.into_any_element()
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
        .bg(if active.is_some() {
            sel_bg
        } else {
            transparent
        })
        .hover(|s| s.bg(sel_bg))
        .child(badge_el)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(st.fg)
                .child(SharedString::from(label)),
        );
    if let Some(hint) = hint {
        let hint_el = div()
            .flex_none()
            .text_color(hint_color.unwrap_or(st.dim))
            .text_size(px(st.pt * 0.85))
            .child(SharedString::from(hint.to_string()));
        let hint_el = if let Some(probe) = hint_probe {
            probe_bounds_dyn(probe, hint_el.into_any_element())
        } else {
            hint_el.into_any_element()
        };
        row = row.child(hint_el);
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
