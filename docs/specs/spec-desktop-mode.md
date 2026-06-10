# Desktop Mode — Fixed-Size Panels on a Pannable Grid

**Status:** DRAFT
**Last updated:** 2026-06-09

## Builds On

- `spec-layout-patterns.md` — defines the per-tab `LayoutMode` machinery
  (mode cycling, `LayoutSkeleton` save/restore, status-bar sigils, tag-view
  filtering). Desktop mode is a fifth `LayoutMode` variant and reuses that
  machinery unchanged: the manual tree skeleton is saved on the same
  transitions, the sigil slot gains one entry, and tag filtering applies to
  panels exactly as it applies to tiled windows.
- `spec-rail.md` — the rail is per-tab chrome that coexists with every layout
  mode. Desktop mode renders inside the same content area the rail leaves
  free; the rail itself is unaffected.

## Overview

Desktop mode is an alternative to tiling: every window becomes a **panel** of
one fixed, globally-configured size (default **120 columns × 40 rows** of the
mono cell grid), placed on an unbounded **desktop** of grid **slots**. The
window is a viewport over the desktop: growing the window reveals more
desktop; it never moves, rescales, or reflows panels. Panels are arranged by
mouse drag-and-drop with **insert-and-shift** sequence semantics.

Named entities introduced here:

- **Slot** — a cell address `(row, col)` on the unbounded grid (origin
  top-left, growth rightward/downward). The stored unit for all placement.
- **DesktopState** — per-tab state: the sorted slot assignment
  (`Vec<(WindowId, Slot)>`, row-major order — placement *and* sequence in
  one structure), the pan offset, and transient drag state.
- **Effective width (W)** — the drop-time wrap width: the number of panel
  columns that fit the current viewport (minimum 1). Used only when a
  mutation needs row-major "next slot" semantics; stored slots are never
  re-derived from it.
- **Desktop layout engine** — pure functions over `DesktopState` (insert-
  and-shift, first-free-slot, spatial navigation, reconciliation, slot↔pixel
  geometry) that own all placement decisions.

The tab's `Layout<C>` tree remains the **content owner** (leaves hold the
live `Window<C>`); `DesktopState` owns geometry only. This is the
slot-map-alongside-frozen-tree shape: no content migrates, and the dormant
buffer-pool plumbing stays dormant.

## Behaviors

### 1 · Mode entry, exit, and seeding [DRAFT]

`LayoutMode` gains a `Desktop` variant, cycled by the existing
`Ctrl-W Space` after `Columns`. Leaving `Manual` saves the tree skeleton and
returning restores it, exactly as for the automatic modes.

On first entry (empty slot map) panels are seeded row-major in tree-leaf
order using the current effective width W. On re-entry the existing slot map
is kept — a tab's arrangement survives round-trips through other modes and
across restarts.

### 2 · Slot invariants and reconciliation [DRAFT]

Invariant: the slot map holds exactly one entry per tree leaf, and no two
entries share a slot. The desktop layout engine reconciles on every
structural change and on mode entry: a leaf without a slot is inserted
after the focused panel (insert-and-shift, Behavior 4); a slot entry whose
window no longer exists is dropped, leaving a gap. Gaps are intentional
structure — closing a panel never moves its neighbors.

New windows created while in Desktop mode (split actions, `Cmd+O`, agent
open) are panels like any other; "split" loses its directional meaning and
simply inserts the new panel after the focused one.

### 3 · Geometry, panning, and rendering [DRAFT]

Panel pixel size derives at render time from the measured mono cell size:
`panel_px = (cols × cell_w, rows × cell_h)` with a fixed gutter
(`DESKTOP_GUTTER`, ~12px) between slots and around the origin. Slot origin:
`gutter + slot ⊗ (panel_px + gutter) − pan`.

The canvas pans on both axes (trackpad/wheel); `pan` is clamped to the
bounding box of occupied slots plus one slot of margin. Keyboard focus
changes auto-pan the minimum needed to reveal the focused panel. Empty
desktop shows a faint dot grid at slot pitch. Window resize changes the
viewport only — pan and slots are untouched.

Only panels intersecting the viewport render (frame-level culling), with
one exemption: **the focused panel always renders** even when panned out of
view. The focused leaf's element carries the focus handle and the
per-screen `on_action` wiring (CLAUDE.md "GUI key conventions"); culling it
would drop the key contexts from the dispatch tree and strand the keyboard
— including the very focus/auto-pan actions that would recover. Desktop-
level actions (pan, drag-cancel) are additionally wired on the canvas root
so they survive any single panel's absence. Each panel renders the same
inner content as its tiled form (Doc / Edit / Browser / Agent), in a fixed
frame with a thin **title bar** (~20px): buffer name or session label, the
existing mark badge, and the focus accent. The status bar shows sigil `[#]`
(extends layout-patterns Behavior 16).

Tag filtering (layout-patterns Behavior 5) hides non-matching panels;
hidden panels keep their slots and reappear in place.

### 4 · Drag and drop: insert-and-shift [DRAFT]

The title bar is the drag handle; panel content keeps its normal mouse
semantics. Mouse-down on a title bar arms a drag; movement past a small
threshold (~4px) starts it (below the threshold it's a focus click).

While dragging: the panel follows the pointer as a semi-transparent ghost;
its home slot shows an empty outline; the computed drop target slot is
highlighted. Dragging into a ~30px band at the viewport edge auto-pans.
`Esc` cancels (panel returns home).

Drop resolution, against the grid *as it is at drop time*:

- **Empty target slot** — the panel moves there. Its old slot becomes a gap.
- **Occupied target slot** — insertion: the dragged panel takes the target
  slot; the occupant and each subsequent panel in the contiguous occupied
  run shift forward one slot. The run stops at the first gap, which absorbs
  the ripple. Shifting is evaluated back-to-front so no two panels collide.

Run semantics, pinned: the successor function is the **W-wrapped chain** —
`succ(row, col) = (row, col+1)` if `col + 1 < W`, else `(row+1, 0)` — and
run contiguity follows that chain, not the unbounded grid order. Panels at
`col ≥ W` (seeded when the window was wider, or the panel size was raised)
are therefore outside every successor chain: ripples never touch them, and
they move only by direct drop. They remain focusable and pannable-to.

A drop may target any visible empty slot; the pan clamp (Behavior 3) admits
one slot of margin beyond the occupied bounding box, so the desktop grows
incrementally — one ring of slots per drag, by design.

`Esc` cancel is handled at the canvas root (the dragged panel may not be
the focused one), and arming a drag also focuses the grabbed panel. Stored
slots change only through drops, insertion-reconciliation, and seeding —
never as a side effect of resize, zoom, or panel-size changes.

### 5 · Focus and keyboard navigation [DRAFT]

Directional focus (the existing `focus_left/right/up/down` actions) moves to
the nearest occupied slot in that direction — same row/column preferred,
then nearest Euclidean slot distance; no candidate = no-op. `focus_next` /
`focus_prev` follow row-major sequence order. Marks (`'` + key) work
unchanged and auto-pan to reveal the target panel. All per-screen key
contexts behave as in tiling — desktop changes where panels sit, not what
they are.

### 6 · Panel size configuration [DRAFT]

`desktop_panel_cols` / `desktop_panel_rows` live in `Preferences`
(persisted, default 120 × 40), one global setting for all panels in all
tabs. Runtime configuration uses a small text overlay in the existing
`ActiveOverlay` family (the `TagInput`/`Rename` pattern — sketch-gpui has
no `:` command line), accepting `{cols}x{rows}`; values clamp to
`[20, 400]` cols and `[5, 200]` rows. Changing it re-renders
immediately; slot addresses are size-independent, so no migration occurs —
panels keep their slots and simply grow/shrink in place (which can change
what overlaps the viewport, never what neighbors what).

### 7 · Persistence [DRAFT]

`PersistedTab` gains an optional `desktop_slots` vector of
`(window_id, row, col)`, keyed by the same stable `PersistedLeaf.id` the
layout snapshot already uses (NOT positional alignment — order-aligned bare
pairs would silently scramble on any length/order mismatch; id-keyed
entries degrade to reconciliation instead). Absent field or unmatched ids =
seed/reconcile on the first render after restore (W needs a measured cell
size, so restore-time seeding is deferred to first paint). Existing
`workspace.json` files load unchanged. `pan` is not persisted; on restore
it reveals the focused panel. The preferences fields persist with the
existing `Preferences` round-trip.

**Old-binary blast radius:** `LayoutMode` has no unknown-variant fallback,
and the snapshot loader treats a failed parse as "no snapshot" — an older
build reading `layout_mode: "desktop"` would discard and then overwrite the
workspace arrangement (documents are unaffected; `workspace.json` only).
Mitigation shipped WITH this feature: `LayoutMode` deserialization falls
back to `Manual` on an unrecognized string, so the next binary boundary is
safe; binaries older than the fallback still reset layout on downgrade —
accepted, and noted for the `:promote` blue-green loop.

## Data Model

- `LayoutMode::Desktop` — fifth variant; serializes as `"desktop"`.
- `Slot { row: u32, col: u32 }` — ordered row-major (`(row, col)` lexicographic).
- `DesktopState` on `Tab<C>`:
  - `slots: Vec<(WindowId, Slot)>` — sorted by slot; placement and sequence.
  - `pan: (f32, f32)` — viewport offset; transient-but-kept across mode
    switches, not persisted. Plain floats: the engine (and all of
    `workspace.rs`) stays free of gpui types; the view layer converts at
    the boundary.
  - `drag: Option<DragState>` — dragged id, grab offset, pointer position,
    resolved drop target; never persisted.
- `Preferences { desktop_panel_cols, desktop_panel_rows, .. }`.
- `PersistedTab { desktop_slots: Option<Vec<(u64, u32, u32)>>, .. }` —
  `(persisted window id, row, col)`.

## Interfaces

Desktop layout engine (in `workspace.rs`, pure data — *module-internal*,
called by the GPUI view layer):

- `seed(leaves, w) -> slots` — first-entry row-major placement.
- `reconcile(slots, leaves, focused, w) -> slots` — restore the Behavior-2
  invariant.
- `insert_shift(slots, dragged, target, w) -> slots` — Behavior-4 drop.
- `spatial_neighbor(slots, from, direction) -> Option<WindowId>` — Behavior-5.
- `slot_rect(slot, panel_px, gutter) -> Bounds` / `drop_target(point, pan,
  panel_px, gutter) -> Slot` — geometry, *external* to the render path.

Events / messages: none (all interactions are direct view mutations).
Data ownership: `DesktopState` owns placement; the `Layout<C>` tree owns
content; `Preferences` owns panel size.

## Constraints

- Window resize, text zoom, and panel-size changes MUST NOT mutate stored
  slots. The only slot mutations are seeding, reconciliation, and drops.
- The slot map is geometry only. Any code that needs "which windows exist"
  keeps reading tree leaves; the Behavior-2 invariant is maintained at the
  engine boundary, not assumed by callers.
- Engine functions are pure and unit-tested headlessly. Drag gestures and
  panning feel are GPUI-event-driven and need a human runtime check
  (per the dev-system definition of done).
- Per-frame cost in Desktop mode is O(visible panels), not O(all panels).
- Marks, tags, rails, and the buffer pool are integration points, not
  dependencies — desktop mode must not require changes to any of them.

## Revision History

- 2026-06-09 — Initial draft (clarified with user: ordered-shelf model,
  stored slots with drop-time-only shifting, 120×40 global panel size,
  fifth LayoutMode, insert-and-shift drops).
- 2026-06-09 — Adversarial review folded in (user-authorized): focused
  panel exempt from culling + canvas-root action wiring; W-wrapped
  successor chain pinned (col ≥ W panels outside ripples); id-keyed
  `desktop_slots` persistence; LayoutMode unknown-variant fallback +
  old-binary blast radius; overlay (not `:` command) for size config;
  engine stays gpui-free; restore-time seeding deferred to first paint;
  drag-arm focuses; Esc at canvas root.
