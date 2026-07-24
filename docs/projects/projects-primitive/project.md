# Project: Projects as the top-level primitive

**Status:** IN-FLIGHT (started 2026-07-23 via `/new-ux`)
**Owning specs:** ADR-0028, `docs/components/project.md` (`UXI-Project-1..8`).
**Backlog:** "Projects as the top-level organizational primitive".

## Problem / why

The hierarchy tops out at the workspace; the **cwd** is the only (implicit,
string-keyed) grouping axis and lives redundantly on each workspace
(`Tab::cwd`) and each session (`AgentSession.cwd`). Nothing owns "a set of
workspaces + sessions + a shared cwd + configuration," so per-project
configuration is impossible and the two cwd fields can drift. The user wants a
first-class **Project** at the top of a named hierarchy.

## The model (see ADR-0028 for full rationale)

- **Project** = `{ name (unique key), cwd (one), params (empty bag) }`, owned by a
  `Projects` store (mirrors `AgentSessions`; name-uniqueness enforced by
  construction). Lives in a new `project` module (disambiguated from Linear's
  `Project*`).
- **Workspace** (renamed from `Tab<C>`) and **AgentSession** carry a
  `ProjectId`; cwd is **derived** from the project. Roster-only sessions resolve
  their project by cwd match.
- **Hierarchy:** `Frame` (renamed from the old container `Workspace<C>`) →
  `Project`s → `Workspace`s → `Window` tiles; sessions belong to a project,
  optionally bound to a tile (**intra-project only**).
- **Jump panel** renders the hierarchy (per-project sections; workspaces +
  sessions sublists; per-project create rows; top-level ＋New project; no tile
  rows; global sequential `ctrl-<n>`).
- **Migration:** cwds → named projects (`ws/yaldabaoth`→Yaldabaoth,
  `ws/fulcrum`→Fulcrum, else basename), total + panic-proof.
- **`tab` vocabulary eradicated** (the full type rename ADR-0002 had deferred).

## Key code anchors (from the code map)

- cwd today: `WorkspaceCwd` (`workspace.rs:1133`), `Tab::cwd` (`:1125`),
  `AgentSession.cwd` (`agent.rs:4566`), `agent_base_cwd`/`active_workspace_cwd`
  (`agent_ui.rs:985,995`), `resolve_agent_cwd_arg` (`persist.rs:187`).
- grouping: `group_agent_rows_by_cwd` (`jump_panel_view.rs:280`),
  `order_grouped_rows` (`:304`), `AgentRow.cwd` (`:47`).
- persistence: `PersistedTab.cwd` (`persist.rs:618`), `PersistedWorkspace`
  (`:633`), `SessionSnapshot.cwd` (`:1275`), `persist_cwd_key`/`cwd_match_key`
  (`:76,64`).
- rename surface: `Tab<C>` (`workspace.rs:1084`), container `Workspace<C>`
  (`:1221`), actions `NewTab/CloseTab/NextTab/PrevTab/RenameTab` (`main.rs:207-256`),
  keybindings (`keymap_registry.rs:94-144`). Physical Tab key / tables NOT touched.

## Tickets

| # | Ticket | Deps | Status |
|---|--------|------|--------|
| 001 | `project` module: `Project`/`ProjectId`/`Projects` store + unit tests | — | DONE |
| 002 | Persistence + migration (`projects.json`, cwd→named-project) | 001 | DONE |
| 003 | Re-point workspaces + sessions at `ProjectId`; cwd derived | 001,002 | READY |
| 004 | Jump panel renders the project hierarchy | 003 | READY |
| 005 | Create/delete project + per-project create; remove global cwd overlay | 003,004 | READY |
| 006 | Intra-project binding + active-project derivation | 003 | READY |
| 007 | `tab`→`workspace` / `Workspace`→`Frame` rename; eradicate `tab`; supersede ADR-0002 | — (last) | READY |

Sequencing: 001→002→003 are the load-bearing spine (types → persistence/migration
→ model re-point). 004/005/006 are the UX surface on top of 003 and can overlap.
007 (the mechanical rename) is deliberately **last** so it doesn't churn the
call-sites the earlier tickets are actively editing — one big sweep at the end,
guarded by the full suite.

## Verification

Each ticket ships its `UXI-Project-N` headless guard (see the component spec's
Enforcement lines), observed RED with the fix reverted. Tests never touch
`~/.yalda` (`*_PATH_OVERRIDE` / `None` under `cfg(test)`). Definition of done per
`CLAUDE.md`: build + full suite green + pasted evidence + runtime-checked-or-
flagged.
