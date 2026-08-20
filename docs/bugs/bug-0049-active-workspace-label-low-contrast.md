# bug-0049: active-workspace-label-low-contrast

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Jump Panel

## Symptom

In the Folio/light theme, the active workspace name is pale beige on the white
jump-panel surface and is practically unreadable.

## Context / root cause

The workspace header uses `theme.overlay.border` as the active label's text
color. That token is intentionally a quiet structural divider, not foreground
copy. Unlike ordinary selected navigation rows, the workspace header also lacks
a selected background and accent rail, so all selection meaning is overloaded
onto the low-contrast text.

## Planned solution

Keep workspace names at normal theme foreground contrast. Express selection
structurally with the standard neutral selected background and narrow accent
rail, reserving the pale border/accent token for the rail only.

## Approaches already tried (do NOT repeat)

- None.

---

## Log

### 2026-08-19 — Reproduced and specified

Routed the existing workspace-header colors through one production style seam
without changing behavior. `jump_panel_active_workspace_keeps_folio_foreground`
now fails because the active Folio label receives `overlay.border`, has no
selected background, and has no rail.

### 2026-08-19 — Fixed with structural selection

Workspace rows now resolve selection through one production style seam. The
name always uses editor foreground; active rows add the same neutral selected
background and two-pixel accent rail used by navigation rows. The focused guard
passes, the full GUI suite is green, and mutating the label back to
`overlay.border` reproduces the exact pale Folio failure.
