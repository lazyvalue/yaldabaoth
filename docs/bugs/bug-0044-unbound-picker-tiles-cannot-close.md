# bug-0044: unbound-picker-tiles-cannot-close

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Workspace / Unbound tile lifecycle

## Symptom

Close Tile sometimes does nothing for entries opened from the jump panel's
Unbound list, especially Buffer pickers and Agent tiles still showing their
session picker.

## Root cause

`Frame::close_focused` explicitly returns `Err(())` whenever direct Unbound focus
is set. Both the keyboard handler and the system-menu `close-window` dispatcher
silently ignore that result, so the command cannot remove any directly focused
Unbound tile. Picker-mode tiles make the failure especially visible because no
separate file/session lifecycle action can make them disappear.

## Fix

`Frame::close_focused` now treats a directly focused Unbound tile as the object
of Close Tile: it removes the stable tile, prunes every scratchpad reference,
clears direct focus, and returns the active workspace's focused tile for the
shell to reveal. Bound-workspace close behavior is unchanged.

## Verification

- `workspace::tests::close_focused_removes_direct_unbound_and_reveals_workspace`
- `verify_harness::close_tile_removes_unbound_buffer_and_agent_picker`
- Negative control: reversing the scratchpad-pruning predicate made the model
  guard fail with the stale id still present.
