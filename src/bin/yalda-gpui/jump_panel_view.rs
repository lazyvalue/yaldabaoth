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
    /// A tile in this GUI currently binds the session (in use).
    pub(crate) bound: bool,
    /// The agent subprocess is live (from the roster's `connected`); local-only
    /// pre-attach sessions are treated as connected.
    pub(crate) connected: bool,
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
            let label = self
                .sessions
                .locate(&info.session_id)
                .and_then(|id| self.sessions.get(id))
                .map(|e| e.read(cx).label.clone())
                .unwrap_or_else(|| info.label.clone());
            rows.push(AgentRow {
                target: JumpTarget::Roster(info.session_id.clone()),
                label,
                bound,
                connected: info.connected,
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
                bound: self.agent_tile_id_bound_to(id).is_some(),
                connected: true,
            });
        }
        rows
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
        col = col.child(section_heading("Workspaces", &st).px_3());
        for (idx, label, active) in workspaces {
            let row_id = SharedString::from(format!("jump-ws-{idx}"));
            col = col.child(
                jump_nav_row(row_id, &label, active, active, None, &st, sel_bg).on_click(
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
        for (i, row) in rows.into_iter().enumerate() {
            // Bound (in-use) sessions jump to their existing tile; free ones
            // open in an ephemeral virtual workspace. ● = in use, ○ = free; a
            // disconnected (agent not running) session is dimmed.
            let badge = if row.bound { "●" } else { "○" };
            let row_id = SharedString::from(format!("jump-sess-{i}"));
            let target = row.target.clone();
            let mut r = jump_nav_row(row_id, &row.label, false, false, Some(badge), &st, sel_bg);
            if !row.connected {
                r = r.text_color(st.dim);
            }
            col = col.child(r.on_click(
                cx.listener(move |this, _ev, _window, cx| this.jump_to_agent(target.clone(), cx)),
            ));
        }

        col.into_any_element()
    }
}

/// One selectable row: optional leading badge glyph + label, tinted when
/// `selected`. Returns a `Stateful<Div>` (has an `id`, so it supports
/// `hover`/`on_click`); the caller attaches the click listener. `accent_text`
/// draws the label in the accent color (used to mark the active workspace).
#[allow(clippy::too_many_arguments)]
fn jump_nav_row(
    id: impl Into<ElementId>,
    label: &str,
    selected: bool,
    accent_text: bool,
    badge: Option<&str>,
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
                .w(px(12.0))
                .flex_none()
                .text_color(st.dim)
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
