# Project: Workspace power commands

**Status:** complete
**Cog graph:** `dgn`
**Component contracts:** `UXI-Workspace-17..20`

## Outcome

Build four keyboard-native workspace workflows borrowed from i3, dwm, and
XMonad on top of ADR-0033's stable bound/Unbound ownership:

1. send a tile to another workspace with explicit follow/no-follow behavior;
2. stash and summon an MRU scratchpad backed by Unbound;
3. toggle to the previous workspace while restoring per-workspace tile focus;
4. control the Columns master ratio and master count.

## Model

- Sending and scratchpad moves always transfer the complete `Window<App>`;
  `WindowId`, App state, project, tags, marks, and Agent selection survive.
- Scratchpad is a persisted ordered set of ids whose tiles remain owned by
  Unbound; direct focus is still non-owning.
- Workspace history uses immutable workspace `auto_name`, not vector position.
- Columns master parameters affect render geometry only; Plane placement is
  untouched.

## Work

| Stage | Cog node | Status |
|---|---|---|
| Contract | `contract` / `gk5v` | complete |
| Core state + persistence | `core` / `kzef` | complete |
| Commands + Columns geometry | `commands` / `reaf` | complete |
| Real-path verification | `verify` / `jo7t` | complete |
| Documentation + integration | `integrate` / `r08q` | in progress |

## Acceptance

- Every command has one production action/menu implementation and a real key
  or menu-path guard.
- Same-project ownership and the durable workspace floor remain enforced.
- Old snapshots load; scratchpad and active master parameters round-trip.
- Real paint proves master/stack geometry, with observed-RED and targeted
  mutation evidence.
