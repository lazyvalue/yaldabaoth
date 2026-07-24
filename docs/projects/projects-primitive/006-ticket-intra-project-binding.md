# 006 — Intra-project binding + active-project derivation

**Goal.** Enforce that a session binds only within its project, and derive the
active project from focus. (`UXI-Project-6`, and the derivation half of `-7`.)

## Subtasks

- [ ] `agent_sessions.rs` / `agent_ui.rs`: gate the bind path on
      `session.project == tile.workspace.project`; refuse a cross-project bind.
- [ ] Selector / free-session list: filter free sessions by the tile's project.
- [ ] Free-session jump (ADR-0021): open the ephemeral workspace under the
      session's own project.
- [ ] `active_project()` derivation: focused workspace's project → focused
      session's project → first project. Replace `agent_base_cwd`'s active-tab-cwd
      lookup with the active project's cwd.
- [ ] Guards: `bind_refused_across_projects_allowed_within`;
      `active_project_derives_from_focus`. Each NC RED.

## Verification

Full suite green; NCs RED.

## Links

ADR-0028 §4 · `docs/components/project.md` UXI-Project-6,-7 ·
`spec-agent-session-ownership.md` INV-2 · ADR-0021.
