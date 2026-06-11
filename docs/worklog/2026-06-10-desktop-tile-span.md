# Worklog: Desktop tile span — edge resize (v1)

**Date:** 2026-06-10
**Branches touched:**
- `desktop-tile-span` → folded to `master` — spec amendment + engine + view +
  persistence for spanning a desktop tile across slots via edge drag.

## Built (with status)

- **Spec amendment (`spec-desktop-mode.md`)** — Behavior-2 invariant
  generalized from one-slot-per-tile to **non-overlapping rectangles**; new
  **Behavior 4b** (east/south edge resize, whole-slot snap, **Block**-rule
  clamp); Behavior 4 ripple made rectangle-aware (multi-slot tiles and
  `col ≥ W` are **walls**; unabsorbable inserts rejected); parallel optional
  `desktop_spans` persistence. The *push* (shove-neighbors) model is deferred.
- **Engine (`workspace.rs`)** — `Span`, `ResizeEdge`, `DesktopResize`;
  `DesktopState.spans` + `resize`; rectangle-aware `occupant`; `rect_of`,
  `rect_free`, `clamp_span`, `set_span`; `insert_shift` rewritten
  rectangle-aware, returns `bool` (wall-blocked drops rejected and restored);
  `reconcile` drops stale spans + retries first-free on rejection;
  span-aware `occupied_extent`; `tile_rect` geometry. **9 new unit tests.**
- **View (`chrome.rs`)** — 6px east/south resize bands per tile (resize
  cursor); `desktop_resize_grab` + live Block-clamped preview
  (`desktop_resize_target_span`) + commit in `desktop_drop`; pointer-move and
  cancel handle resize; tiles render and cull at their span rect; drag
  ghost/outlines span-sized.
- **Persistence (`persist.rs`)** — `PersistedTab.desktop_spans` (optional,
  `(id, rows, cols)`, non-default only); restore in `main.rs`.

**Verification:** both bins build clean; 145 + 64 bin/lib tests pass (the 2
pre-existing `snapshot_test` failures are unrelated and were cleared earlier).
Human runtime smoke passed after one fix (below).

## Open / unresolved
- **Esc-to-cancel resize** is not wired (right-click cancels). Inherits the
  same backlog status as Esc-cancel for drag (a global Esc binding would shadow
  per-screen escape semantics — see desktop spec Behavior 4).
- **North/West edges** (grow up/left, moving the anchor) are deferred — v1 is
  east/south only so the anchor stays stable.
- **Push model** (expansion shoves neighbors) deferred; v1 grows into free
  desktop only (Block).
- Auto-pan reveal of a large focused tile still uses the 1×1 size — reveals the
  anchor corner, not the whole rectangle. Minor; not addressed.

## Decisions
- **Block over Push for v1** (user-confirmed): grow into free desktop only,
  clamp at the first occupied slot. Keeps the ripple bounded and overlap
  impossible without designing a shove-cascade. Push is a clean follow-up.
- **E/S edges only for v1** (user-confirmed): keeps the stored anchor stable.

## Verification status
- Engine is headlessly unit-tested. The drag/resize gesture needed a human
  runtime check (GPUI can't be driven headlessly).
- **Smoke caught a real bug:** the resize bands used `.h_full()`/`.w_full()`
  on absolutely-positioned divs, which collapse to zero on the unconstrained
  axis — 6×0 hit area, so edge drags did nothing. Fixed by pinning both insets
  (`0c88814`); re-smoked OK.

## Next
- If desired: Esc-cancel for desktop drag/resize (one backlog item, both at
  once); N/W edge resize; the push model.
