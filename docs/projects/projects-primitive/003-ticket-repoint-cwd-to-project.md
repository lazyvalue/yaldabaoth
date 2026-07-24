# 003 — Re-point workspaces + sessions at `ProjectId`; cwd derived

**Goal.** Make the project the single source of a directory: a workspace and a
session reference a `ProjectId`; every cwd read resolves through the store.
(`UXI-Project-2`.) This is the largest mechanical step (mirrors the
agent-model-refactor's ownership move).

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
