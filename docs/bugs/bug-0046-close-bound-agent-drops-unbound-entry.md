# bug-0046: close-bound-agent-drops-unbound-entry

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Workspace / Agent tile lifecycle

## Symptom

Closing a bound Agent tile removes it from its workspace but its still-live
session does not appear in the jump panel's Unbound list.

## Root cause

The shared Close Tile dispatcher calls the generic destructive
`Frame::close_focused` path without considering App lifecycle. That drops the
only stable Agent tile while the normalized session remains alive. Because the
jump panel is tile-native, it has no object to project until an unrelated roster
refresh happens to materialize another tile.

## Required fix

Close Tile on a bound Agent must use the same stable bound-to-Unbound transition
as Stash. It must preserve the tile, session, project, tags, and identity and add
the tile to scratchpad MRU. Keyboard and menu entry points must share one typed
command path.

## Verification

- `verify_harness::close_bound_agent_tile_stashes_same_tile_and_session`
- Observed RED on the production dispatcher: the tile has no membership after
  `close-window` instead of `TileMembership::Unbound`.
- The same production-path guard is green after the fix; the sole-workspace
  branch is covered by
  `verify_harness::close_sole_bound_agent_stashes_and_seeds_workspace_floor`.
- Keyboard and menu dispatch both call the exhaustive `CloseTileOutcome`
  transition, so those input surfaces cannot carry separate lifecycle rules.
