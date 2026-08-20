# bug-0045: ctrl-w-missing-by-tile-state

**Status:** RECURRENT→FIXED
**First seen:** 2026-08-19
**Component:** Shell action routing / tile Apps

## Symptom

`Ctrl-W` followed by a focus direction (`h/j/k/l`) works in some tiles and
silently does nothing in others. Agent picker/session, Linear, Cog, Keymap, and
Buffer states had different subsets of the shell command family, which made the
focus chord feel intermittently captured by tile content.

## Root cause

The bindings are globally matched, but GPUI still requires an action listener in
the focused element's ancestry. Shell listeners are copied by hand onto every
App screen root and a second incomplete subset is copied onto the workspace
wrapper. Missing listeners silently drop the already-matched action. For
example, AgentPicker handles Close/Focus but omits SplitH/SplitV; other screens
omit different commands. There is no exhaustive central owner or registry
coverage check.

The 2026-08-20 recurrence has a different cause. The central action router is
present and dispatches reliably, but `Frame::focus_motion` always resolved the
direction against retained two-dimensional Plane slots. Columns and Tiling
derive different painted arrangements from the slots' reading order. A tile
that is visibly left could therefore have a larger hidden Plane column, causing
`h` to move right, `l` to move left, or either command to find no candidate.
The first guard forced `WorkspaceView::Plane`, so it could not detect
disagreement between the active arrangement and retained Plane geometry.

## Fix

One generated declaration now owns the complete `Ctrl-W` action vocabulary and
wires it once on the common tile-shell ancestor for both bound and directly
focused Unbound surfaces. Per-App, arrangement-root, and rail duplicates were
removed. App renderers now own only their local commands.

## Verification

- `tests::ctrl_w_registry_exactly_matches_central_shell_actions` compares the
  shipped `ctrl-w …` registry actions to the central router as exact sets.
- `verify_harness::ctrl_w_shell_commands_reach_every_tile_app` drives the real
  `Ctrl-W h/j/k/l` chords and verifies the exact focused neighbor across Buffer
  picker/view/edit, Agent picker/session, Linear, Cog, and Keymap tiles.
- Negative controls were observed RED both when `FocusRight` was removed from
  the router and when the common ancestor wiring itself was removed.

## Recurrence log

### 2026-08-20 — Visible Columns order disagrees with hidden Plane coordinates

A production-keymap and production-paint guard places three tiles in signed
reading order `left, center, right`, verifies the Columns renderer paints them
in that horizontal order, and deliberately assigns hidden Plane columns in the
opposite direction. `Ctrl-W h` from the center selected the visibly-right tile
(`WindowId 3`) instead of the visibly-left tile (`WindowId 2`), reproducing the
reported apparent intermittency. A separate staggered-key guard proved that a
render between `Ctrl-W` and the direction does not lose GPUI's pending prefix;
the recurrence is layout target resolution, not timing or App capture.

### 2026-08-20 — Focus now follows the active arrangement

`Workspace::focus_target` is now the single view-aware resolver used by
`Frame::focus_motion`. Plane retains two-dimensional spatial navigation.
Columns use their painted non-wrapping horizontal reading order, with `j`/`k` as
no-ops. Tiling follows its two painted vertical panes: `j`/`k` stay in a pane and
`h`/`l` cross between primary and stack at the closest row. Monocle maps `h`/`k`
backward and `l`/`j` forward through reading order so changing the sole presented
tile is deterministic. The stale Buffer Edit handler for bare `Ctrl-W` was also
removed; Code/WP remains available from the Buffer tile menu, leaving the prefix
exclusively owned by the shell. The common pre-App key interceptor now explicitly
consumes bare Ctrl-W if GPUI ever delivers the unresolved prefix to raw handling.

The real-path recurrence guard covers contradictory hidden Plane geometry under
Columns and Tiling, Columns x-order, Tiling's primary/vertical-stack geometry,
and all four Monocle directions. Restoring the old unconditional
`desktop.spatial_neighbor` implementation reproduces the visibly inverted RED.
