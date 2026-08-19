# bug-0045: ctrl-w-missing-by-tile-state

**Status:** FIXED
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
