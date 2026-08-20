# bug-0051: hidden-tile-has-no-indicator

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Jump Panel

## Symptom

A hidden tile remains correctly listed under its workspace but looks identical
to a visible tile, so the user cannot tell why it is absent from the workspace.

## Context / root cause

The workspace model distinguishes visible and hidden attachments, but
`jump_panel_sections` flattens both into `JumpTileRow` without retaining that
visibility state. The row renderer therefore has no state from which to paint
an indicator.

## Planned solution

Carry typed attachment visibility through the jump-panel projection and render
a compact subdued `hidden` marker at the trailing edge of every hidden tile
row, including Agent rows that also show provider and activity marks.

## Approaches already tried (do NOT repeat)

- None.

---

## Log

### 2026-08-19 — Reproduced and specified

`jump_panel_hidden_tiles_paint_indicator` hides a real attached Linear tile,
confirms its row still paints under the workspace, and fails because no
dedicated hidden-state marker is present.

### 2026-08-19 — Fixed with typed placement and a compact status mark

`JumpTilePlacement` is a closed projection with `AttachedVisible`,
`AttachedHidden`, and `Detached`, so detached-plus-hidden is not representable.
Hidden Agent and non-Agent rows both paint the reusable yux `hidden` pill while
Agent activity/provider marks remain intact. The real paint guard also proves a
Detached row has no marker and the pill remains at most 16.5px tall. Forcing
hidden detection false removes the pill and turns the guard RED; forcing it true
incorrectly marks Detached and is also caught.
