# Project: Unbound tiles and workspace folders

**Status:** complete
**Cog graph:** `9k2`
**Decision:** `docs/decisions/0033-tiles-have-optional-workspace-ownership.md`
**Component contracts:** `UXI-Workspace-16`, `UXI-JumpPanel-23`,
`UXI-AgentTile-34`

## Outcome

Make a tile a durable object whose workspace membership is optional. Tiles
outside a workspace are **unbound**: they retain their App state, project,
identity, and tags; remain directly reachable from Cmd-P and the jump panel; and
can later be bound without recreation.

## Model

```
Frame
├─ Workspace A (folder)
│  ├─ bound tile 10
│  └─ bound tile 14
├─ Workspace B (folder)
│  └─ bound tile 21
└─ Unbound
   ├─ tag: review
   │  └─ unbound tile 8
   └─ untagged tile 11
```

The two ownership domains are exclusive and exhaustive. `direct_unbound` is
only a focus pointer into Unbound; it is not a third owner.

## Work

| Stage | Cog node | Status |
|---|---|---|
| Contract + architecture | `contract-architecture` / `4edc` | complete |
| Frame ownership API | `unbound-core` / `5enx` | complete |
| Persistence + old-state migration | `persistence-migration` / `rczp` | complete |
| Cmd-P direct access | `direct-access` / `jr0y` | complete |
| Jump-panel workspace folders + Unbound | `jump-panel-tree` / `4d9y` | complete |
| Real-path verification + mutation tests | `real-path-verification` / `kfs9` | complete |
| Docs, worklog, branch integration | `document-integrate` / `vm1a` | complete |

## Acceptance

- Every live tile is classified exactly once as bound or unbound.
- Bind/unbind preserves `WindowId`, App state, project, and tags.
- Cmd-P directly opens unbound tiles without changing membership.
- The jump panel shows bound tiles only under their collapsible workspace and
  unbound tiles only in Unbound, grouped by tag.
- Old snapshots restore without loss or duplicate tile/session rows.
- Production key/menu/click paths are headlessly guarded with observed-RED
  controls; focused mutants do not survive.
