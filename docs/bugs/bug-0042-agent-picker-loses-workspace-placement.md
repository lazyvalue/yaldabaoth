# bug-0042: agent-picker-loses-workspace-placement

**Status:** RECURRENT→FIXED
**First seen:** 2026-08-19
**Component:** Agent Tile picker / workspace ownership

## Symptom

Selecting an existing Agent from an empty Agent tile inside a workspace could
navigate to the Agent's previous direct view instead of placing it in the
workspace. The behavior appeared intermittent across mouse and Enter because it
depended on whether that server session was already attached locally. There was
also no discoverable Agent-local command for sending an Agent tile to a
workspace.

The same release enlarged workspace folder labels in Cmd-P from the standard
29px row height to 33.5px.

## Root cause

Roster sessions had already been materialized as stable unbound Agent tiles,
but picker activation still created a second local placeholder in the temporary
empty workspace tile. Strict 1:1 session binding then focused the existing owner
when the session was local, producing the jump; for a roster-only session it
attached to the temporary tile and stranded the stable tile. The workspace move
picker also rejected unbound tiles, and the jump workspace header inherited a
larger text size because it did not set the base jump typography explicitly.

## Fix

Picker activation now atomically replaces the temporary bound picker leaf with
the session's stable unbound Agent tile. The stable `WindowId`, state, and tags
move into the exact layout slot; the temporary tile is retired. If the session
is already local, the stable tile binds that existing owner without creating a
duplicate. Mouse and Enter use the same activation path.

The Agent local menu now includes **send to workspace**. Its workspace picker
accepts either bound or unbound focused tiles, preserves the stable tile, and
follows it to the chosen same-project workspace. Workspace folder headers now
set the base jump font size explicitly.

## Verification

- Observed RED before the fix for real painted-row click and real Enter: focus
  remained on temporary tile 3 instead of stable tile 2.
- Observed RED before the command: `agent-send-workspace` did not open a picker.
- Observed RED before the font fix: workspace row 33.5px vs standard row 29px.
- Guards cover click, Enter, already-local/no-duplicate placement, the command,
  stable replacement semantics, and exact jump-row typography.

## Recurrence: live-roster Enter no-op

The first fix proved real Enter only when the roster stayed unchanged. The
already-local case was mistakenly guarded by calling `agent_picker_activate`
directly, so the handoff overclaimed keyboard consistency.

The remaining intermittent failure occurred when the live picker projection
shrunk while open. Rendering clamped the visible highlight to the final valid
row, but `SessionPicker.selected` retained its old larger index. Enter submitted
that stale index; `agent_picker_activate` found no row and silently did nothing.
Mouse activation was unaffected because the painted row carries its current
index.

Keyboard movement and Enter now normalize the stored cursor against the current
projection before moving or activating. The recurrence guard uses real Down and
Enter events with a tagged roster row removed between them. Its negative control
failed with focus still on picker tile 4 instead of stable Agent tile 3.
