# 004 — Jump panel renders the project hierarchy

**Goal.** Group the jump panel by project: per-project sections with WORKSPACES +
AGENT SESSIONS sublists, per-project create rows, a top-level ＋New project, no
tile rows, global sequential `ctrl-<n>`. (`UXI-Project-3`.)

## Subtasks

- [x] `jump_panel_view.rs`: `group_agent_rows_by_cwd` → `group_rows_by_project`
      (key on `ProjectId`). Header = project name + dim cwd subtext.
- [x] Add a per-project WORKSPACES sublist (rows from the frame's workspaces
      filtered by project; keep the global `idx+1` badge = `ctrl-<n>` target).
- [x] Per-project ＋New workspace / ＋New agent session rows; top-level ＋New
      project row.
- [x] Preserve `UXI-JumpPanel-5` accent marks + `UXI-JumpPanel-1/6` status dots
      within each section. Reconcile `order_grouped_rows` (cwd order → project
      order) or note deferral.
- [x] Guard: `verify_harness.rs::jump_panel_groups_workspaces_and_sessions_by_project`
      (two projects, each a workspace + session → two sections, each lists its own;
      tiles absent). NC: flat list → RED.

## Verification

Full suite green; NC RED. Row structure headless; literal pixels/hues gap #1.

## Links

ADR-0028 §3 · `docs/components/project.md` UXI-Project-3 · `jump-panel.md`
UXI-JumpPanel-1..6.
