# bug-0009: camera-pan-does-not-snap-to-slots

**Status:** IN-PROGRESS
**First seen:** 2026-07-17
**Component:** docs/components/workspace.md (UXI-Workspace-8 / UXI-Workspace-4)

## Symptom

Panning the plane (Cmd+Shift + left-drag) and releasing leaves the view resting
on a **fraction of a slot** — the whole grid sits subtly offset from the
viewport. The user expects the camera to **snap to a whole workspace slot on
release**, the same way a tile drag/edge-resize already rests the view
cell-aligned (UXI-Workspace-8). "The camera snap to workspace slot functionality
does not work" — for the pan gesture specifically.

## Context / root cause

UXI-Workspace-8 snaps the camera pan to whole slot units at commit for two
gestures — tile **drag** and tile **edge-resize** — via
`DesktopState::snap_camera_to_slots` (rounds `camera.pan` per-axis). The spec
explicitly left the `Cmd+Shift` free-pan **out of scope** ("stays continuous").

The free-pan gesture is armed in `chrome.rs::desktop_pan_grab`, applied
continuously in `desktop_pointer_move`, and ended in `desktop_drop`. The
pan-end branch (`chrome.rs:1037-1042`) takes the `pan_drag`, calls
`save_workspace_state()`, notifies, and returns — it never calls
`snap_camera_to_slots()`. So the fractional pan the user left the gesture on is
persisted verbatim.

This is the one branch of `desktop_drop` that omits the snap the drag/resize
branches both perform. From the user's POV it's a bug: pan is the most common way
to leave the view fractional, and it's exactly the gesture that doesn't clean up.

## Planned solution

Snap on **release**, not during the drag (continuous while moving, rests aligned
when you let go). Add `snap_camera_to_slots()` to the `desktop_drop` `pan_drag`
branch before `save_workspace_state()`. This brings the free-pan into the same
cell-aligned-rest contract as drag/resize; the gesture stays continuous mid-drag.

Reconcile UXI-Workspace-8: free-pan moves from "out of scope, stays continuous"
to "snaps on release" and update the deviation note. The existing
`cmd_shift_drag_pans_the_plane` test asserts a fractional post-release pan
(> 0.1 on both axes); its small drag snaps the y-axis to 0, so it must be updated
to a larger drag whose snap keeps both axes positive AND assert integrality.

## Approaches already tried (do NOT repeat)

- <none yet — first attempt>

---

## Log

### 2026-07-17 — Snap free-pan on release — FIXED

**Root cause confirmed.** Of the three `desktop_drop` early-return branches, the
`pan_drag` (Cmd+Shift free-pan) branch was the only one that persisted the camera
without `snap_camera_to_slots()` — the drag and edge-resize branches both snap.
UXI-Workspace-8 had deliberately left free-pan out of scope; the user reported the
opposite expectation (pan should rest cell-aligned on release), so this is now a
bug, not a spec choice.

**Fix.** `chrome.rs:1042` (`desktop_drop`, `pan_drag` branch): call
`self.workspace.tabs[tab_idx].desktop.snap_camera_to_slots()` before
`save_workspace_state()`. Snap happens at **mouse-up** only, so the gesture stays
continuous while dragging and settles on a whole slot when released.

**Spec reconciled.** `docs/components/workspace.md` UXI-Workspace-8: free-pan moved
from "out of scope, stays continuous" to "continuous while dragging, snaps on
release"; Applies-to, Enforcement, and Deviation notes updated.

**Verification.**
- New guard `verify_harness::cmd_shift_pan_rests_view_cell_aligned` drives the REAL
  mouse dispatch (`simulate_mouse_down/move/up` → canvas-root handlers →
  `desktop_pan_grab`/`desktop_pointer_move`/`desktop_drop`), reads the pan
  MID-gesture and asserts it is fractional (non-vacuous), then asserts it is
  integral on both axes after release, and that no tile moved (view-only).
  Endpoints derived from the real painted geometry via `pan_drag_endpoints` (pitch
  varies with the painted canvas — an early hardcoded 800×600 guess was wrong; real
  pitch ≈ 844×534).
- **Negative control (observed RED):** neutered the `snap_camera_to_slots()` call in
  the `pan_drag` branch → pan rested at fractional `(1.4, 1.4)` → the integral
  asserts fired for the right reason. Restored; re-ran green.
- Updated `cmd_shift_drag_pans_the_plane` (its old small drag would snap the y-axis
  to 0) to a ≥1-pitch drag via the same helper; still asserts "pans + no tile moves."
- Full suite: **381 passed, 0 failed, 1 ignored.**

**Status:** FIXED on `main` (chrome.rs / verify_harness.rs / workspace.md). Runtime
still to be confirmed by the user after a rebuild (the headless probe reads pan
STATE, not painted pixels — genuine gap #1; the actual on-screen rest is a human
eye check).
