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

        // Sessions: every store id, its label, and whether a tile binds it.
        let session_ids: Vec<SessionId> = self.sessions.ids().collect();
        let sessions: Vec<(SessionId, String, bool)> = session_ids
            .into_iter()
            .map(|id| {
                let bound = self.agent_tile_id_bound_to(id).is_some();
                let label = self
                    .sessions
                    .get(id)
                    .map(|ent| ent.read(cx).label.clone())
                    .unwrap_or_default();
                (id, label, bound)
            })
            .collect();

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
        if sessions.is_empty() {
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
        for (sid, label, bound) in sessions {
            // Bound sessions jump to their existing tile; free ones open in an
            // ephemeral virtual workspace. A small glyph distinguishes them.
            let badge = if bound { "●" } else { "○" };
            let row_id = SharedString::from(format!("jump-sess-{}", sid.0));
            col = col.child(
                jump_nav_row(row_id, &label, false, false, Some(badge), &st, sel_bg).on_click(
                    cx.listener(move |this, _ev, _window, cx| this.jump_to_session(sid, cx)),
                ),
            );
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
