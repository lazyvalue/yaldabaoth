# 005 — Create/delete project + per-project create; remove global cwd overlay

**Goal.** Full project lifecycle from the jump panel, and retire the free-floating
cwd overlays. (`UXI-Project-4`, `-5`, and the removal half of `-7`.)

## Subtasks

- [ ] Create: ＋New project overlay (name + cwd) → `Projects::create`; dup name →
      transient error, nothing created; empty name cancels. New project is empty +
      persisted.
- [ ] Per-project create: ＋New workspace → `new_workspace_in(project)`; ＋New
      agent session → `new_agent_session_in(project)` (cwd = project's, no prompt).
- [ ] Delete: action + confirm overlay; non-empty → confirm → cascade (close
      workspaces, kill sessions) → remove; empty → delete directly. Empty projects
      persist otherwise.
- [ ] Remove `RenameTarget::FreeAgentSessionCwd` + `AgentNewSessionCwd` cwd
      overlays and the global/`?`-menu "new agent session" cwd flow
      (`UXI-JumpPanel-3/4` superseded). Update those UXIs' status.
- [ ] Guards: `new_project_overlay_creates_empty_project_and_rejects_dup`;
      `delete_nonempty_project_confirms_then_cascades`;
      `global_cwd_session_overlay_is_gone`. Each NC RED.

## Verification

Full suite green; NCs RED. Overlay commit paths headless (real
`commit_rename_overlay`); the live server session create is daemon gap #2.

## Links

ADR-0028 §Consequences · `docs/components/project.md` UXI-Project-4,-5,-7 ·
`jump-panel.md` UXI-JumpPanel-3,-4 (to be marked superseded).
