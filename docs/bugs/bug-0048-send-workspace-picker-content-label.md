# bug-0048: send-workspace-picker-content-label

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Workspace

## Symptom

The **Send tile to workspace** selector sometimes names a destination
`Claude (<workspace>)`. Its full-width, uniformly weighted list also reads like
a debug menu rather than a polished destination chooser.

## Context / root cause

The selector renders destinations with `workspace_strip_label`. That helper is
intentionally content-sensitive: a single Agent leaf becomes
`Claude (<workspace>)`, a Browser becomes `Browser (<workspace>)`, and a
document may become its filename. Those summaries are useful in the workspace
strip but violate UXI-Workspace-25 in a picker whose rows must identify stable
places. The overlay also duplicates old raw picker chrome instead of using the
standard body typography and reusable option-row hierarchy.

## Planned solution

Give destination rows an explicit workspace-identity projection based only on
`Workspace::display_label`. Recompose the overlay as a compact centered card
with a title/subtitle, accent-led selected row, quiet Current badge, separated
New workspace action, and secondary key hints. Preserve all picker behavior.

## Approaches already tried (do NOT repeat)

- None.

---

## Log

### 2026-08-19 — Reproduced and specified

The production renderer was routed through an explicit destination-label seam
that initially preserves the faulty `workspace_strip_label` projection. The
real Ctrl-W picker guard
`send_picker_agent_destination_uses_workspace_name_without_provider_prefix`
then reproduces the bug as `Claude (Research)` rather than `Research`.

### 2026-08-19 — Fixed identity and rebuilt the picker hierarchy

Destination labels now come from `Workspace::display_label` only. The overlay
is a centered, responsive 480px card with a title/follow-policy subtitle,
system-font workspace labels, accent-rail selection, Current/Create badges, a
fixed header/footer, scrollable destinations, and a separated New workspace
action. The row shape is the reusable yux `picker_option_row`; existing rows
and New workspace are clickable without changing keyboard behavior.

`send_picker_paints_compact_hierarchy_and_click_moves_tile` drives the real
Ctrl-W picker, asserts painted card/row geometry, clicks the selected row's
actual bounds, and proves the move commits without following. The identity
guard is green; mutating the destination projection back to
`Claude (<workspace>)` makes it red at the exact reported symptom.
