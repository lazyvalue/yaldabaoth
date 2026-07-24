# 006 — Intra-project binding + active-project derivation

**Goal.** Enforce that a session binds only within its project, and derive the
active project from focus. (`UXI-Project-6`, and the derivation half of `-7`.)

## Subtasks

- [x] `agent_ui.rs`: gate the bind path on
      `session.project == tile.workspace.project`; refuse a cross-project bind
      (hard guard in `picker_attach_existing`, the shared attach choke).
- [x] Selector / free-session list: filter free sessions by the tile's project
      (`picker_projection` gates on `projects.by_cwd`).
- [x] Free-session jump (ADR-0021): open the ephemeral workspace under the
      session's own project (`open_ephemeral_tab_in`; `jump_to_session` +
      `jump_to_roster_session`).
- [x] `active_project()` derivation: focused workspace's project → focused
      session's project → first project. `agent_base_cwd` was already the active
      project's cwd (T003), so it needed no change — `active_project` is its
      id-level twin.
- [x] Guards: `bind_refused_across_projects_allowed_within`;
      `active_project_derives_from_focus`. Each NC observed RED.

## Verification

Full suite green; NCs RED.

## Links

ADR-0028 §4 · `docs/components/project.md` UXI-Project-6,-7 ·
`spec-agent-session-ownership.md` INV-2 · ADR-0021.
