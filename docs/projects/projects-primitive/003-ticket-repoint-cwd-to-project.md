# 003 — Re-point workspaces + sessions at `ProjectId`; cwd derived

**Goal.** Make the project the single source of a directory: a workspace and a
session hold a `ProjectId` **foreign key** and the cwd is resolved at each point
of use — **never cached** on the workspace/session (ADR-0028 §3, the locked
model). Delete `WorkspaceCwd` / `Tab::cwd` / `default_cwd` outright. This is the
largest mechanical step (mirrors the agent-model-refactor's ownership move).

**Boot ordering (new constraint from the FK model):** the `Projects` store must
be built *before* the `Workspace`, because `Tab::with_layout` now takes a
`ProjectId` (not a cwd). Boot: load `projects.json` (`projects_from_persisted`)
else `migrate_cwds_to_projects(existing workspace+session cwds)` → then build the
workspace passing project ids. The store lives on `YaldaGpuiView` (`projects:
Projects`).

**Session membership:** locally-created sessions store `ProjectId`
(`Membership::Assigned`); roster/server sessions resolve `Membership::Inferred`
via `projects.by_cwd(session.cwd)` at the roster boundary — never persisted as an
assignment.

## Subtasks

- [ ] `workspace.rs`: replace `Tab::cwd: WorkspaceCwd` with `project: ProjectId`;
      `cwd()` reads `projects.get(project).cwd`. Keep `WorkspaceCwd` only if still
      needed at a boundary, else retire. `inherited_cwd` → active workspace's
      project cwd.
- [ ] `agent.rs`: `AgentSession.cwd` → `project: ProjectId` for local sessions;
      add `AgentSession::cwd(&projects)` resolver. Roster-only rows resolve via
      `Projects::by_cwd`.
- [ ] `agent_ui.rs`: `active_workspace_cwd` / `agent_base_cwd` resolve through the
      active workspace's project; `create_agent_session` spawn cwd from project.
- [ ] `persist.rs`: `PersistedWorkspace`/`SessionSnapshot` persist a project
      reference (by name) instead of / alongside cwd; load resolves name→id after
      the store is built (002).
- [ ] Guard: `verify_harness.rs::workspace_and_session_cwd_derive_from_project`
      (change a project cwd → workspace + session spawn cwd both follow). NC:
      stored-field revert → no propagation (RED).

## Verification

Full suite green; the derive guard NC RED. `./dev-gui.sh` boots with migrated
projects (runtime check for the live spawn cwd — harness gap #2 daemon).

## Links

ADR-0028 §3 · `docs/components/project.md` UXI-Project-2 · ADR-0023.
