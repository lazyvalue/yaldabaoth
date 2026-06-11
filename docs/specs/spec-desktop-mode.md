# Desktop Mode — Uniform Tiles on a Pannable Slot Grid

**Status:** DRAFT
**Last updated:** 2026-06-10

## Builds On

- `spec-layout-patterns.md` — defines the per-tab `LayoutMode` machinery
  (mode cycling, `LayoutSkeleton` save/restore, status-bar sigils, tag-view
  filtering). Desktop mode is a fifth `LayoutMode` variant and reuses that
  machinery unchanged: the manual tree skeleton is saved on the same
  transitions, the sigil slot gains one entry, and tag filtering applies to
  tiles exactly as it applies to tiled windows.
- `spec-rail.md` — the rail is per-tab chrome that coexists with every layout
  mode. Desktop mode renders inside the same content area the rail leaves
  free; the rail itself is unaffected.

## Overview

Desktop mode is an alternative to tiling: every window becomes a **tile**,
placed on an unbounded **desktop** of grid **slots**. Tile size derives from
a globally-configured **grid** — how many tiles fit the viewport per axis
(default **2 × 2**) — so all tiles are uniform and resize with the window
while their slots never change. The window is a viewport over the desktop:
growing the window reveals more desktop and grows each tile proportionally;
it never moves or reflows tiles (slot addresses are immutable under resize). Tiles are arranged by
mouse drag-and-drop with **insert-and-shift** sequence semantics.

Named entities introduced here:

- **Slot** — a cell address `(row, col)` on the unbounded grid (origin
  top-left, growth rightward/downward). The **anchor** (top-left) of a tile.
- **Span** — a tile's extent in cells, `(rows, cols)`, each ≥ 1 (default
  1 × 1). A tile at anchor `(r, c)` with span `(rows, cols)` **occupies the
  rectangle** `[r, r+rows) × [c, c+cols)`. Spans grow only east/south, so the
  anchor never moves under resize.
- **DesktopState** — per-tab state: the sorted slot assignment
  (`Vec<(WindowId, Slot)>`, row-major order by anchor — placement *and*
  sequence in one structure), the per-tile span map (absent = 1 × 1), the
  pan offset, and transient drag/resize state.
- **Effective width (W)** — the wrap width for row-major "next slot"
  semantics: the configured grid column count (minimum 1). Stored slots are
  never re-derived from it.
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

On first entry (empty slot map) tiles are seeded row-major in tree-leaf
order using the current effective width W. On re-entry the existing slot map
is kept — a tab's arrangement survives round-trips through other modes and
across restarts.

### 2 · Slot invariants and reconciliation [DRAFT]

Invariant: the slot map holds exactly one anchor entry per tree leaf, and **no
two tiles' rectangles overlap** (with all spans 1 × 1 this reduces to "no two
entries share a slot"). The desktop layout engine reconciles on every
structural change and on mode entry: a leaf without a slot is inserted
after the focused tile (insert-and-shift, Behavior 4); a slot entry whose
window no longer exists is dropped, leaving a gap. Gaps are intentional
structure — closing a tile never moves its neighbors.

New windows created while in Desktop mode (split actions, `Cmd+O`, agent
open) are tiles like any other; "split" loses its directional meaning and
simply inserts the new tile after the focused one.

### 3 · Geometry, panning, and rendering [DRAFT]

Tile pixel size derives at render time from the canvas and the grid:
`tile_px = (canvas − (grid + 1) × gutter) / grid` per axis, with a fixed
gutter (`DESKTOP_GUTTER`, ~12px) between slots and around the origin, and a
minimum tile size so a tiny window stays usable. Slot origin:
`gutter + slot ⊗ (tile_px + gutter) − pan`.

The canvas pans on both axes (trackpad/wheel); `pan` is clamped to the
bounding box of occupied slots plus one slot of margin. Keyboard focus
changes auto-pan the minimum needed to reveal the focused tile. Empty
desktop shows a faint dot grid at slot pitch. Window resize changes the
viewport only — pan and slots are untouched.

A tile's pixel rect covers its span: origin at the anchor slot, size
`span ⊗ (tile_px + gutter) − gutter` per axis (a 1 × 1 tile is exactly one
slot). Only tiles whose rect intersects the viewport render (frame-level
culling), with one exemption: **the focused tile always renders** even when
panned out of view. The focused leaf's element carries the focus handle and the
per-screen `on_action` wiring (CLAUDE.md "GUI key conventions"); culling it
would drop the key contexts from the dispatch tree and strand the keyboard
— including the very focus/auto-pan actions that would recover. Desktop-
level actions (pan, drag-cancel) are additionally wired on the canvas root
so they survive any single tile's absence. Each tile renders the same
inner content as its tiled form (Doc / Edit / Browser / Agent), in a fixed
frame with a thin **title bar** (~20px): buffer name or session label, the
existing mark badge, and the focus accent. The status bar shows sigil `[#]`
(extends layout-patterns Behavior 16).

Tag filtering (layout-patterns Behavior 5) hides non-matching tiles;
hidden tiles keep their slots and reappear in place.

### 4 · Drag and drop: insert-and-shift [DRAFT]

The title bar is the drag handle; tile content keeps its normal mouse
semantics. Mouse-down on a title bar arms a drag; movement past a small
threshold (~4px) starts it (below the threshold it's a focus click).

While dragging: the tile follows the pointer as a semi-transparent ghost;
its home slot shows an empty outline; the computed drop target slot is
highlighted. Dragging into a ~30px band at the viewport edge auto-pans.
`Esc` cancels (tile returns home).

Drop resolution, against the grid *as it is at drop time*:

- **Empty target slot** — the tile moves there. Its old slot becomes a gap.
- **Occupied target slot** — insertion: the dragged tile takes the target
  slot; the occupant and each subsequent tile in the contiguous occupied
  run shift forward one slot. The run stops at the first gap, which absorbs
  the ripple. Shifting is evaluated back-to-front so no two tiles collide.

Run semantics, pinned: the successor function is the **W-wrapped chain** —
`succ(row, col) = (row, col+1)` if `col + 1 < W`, else `(row+1, 0)` — and
run contiguity follows that chain, not the unbounded grid order. Tiles at
`col ≥ W` (seeded when the window was wider, or the tile size was raised)
are therefore outside every successor chain: ripples never touch them, and
they move only by direct drop. They remain focusable and pannable-to.

A drop may target any visible empty slot; the pan clamp (Behavior 3) admits
one slot of margin beyond the occupied bounding box, so the desktop grows
incrementally — one ring of slots per drag, by design.

`Esc` cancel is handled at the canvas root (the dragged tile may not be
the focused one), and arming a drag also focuses the grabbed tile. Stored
anchors change only through drops, insertion-reconciliation, and seeding —
never as a side effect of resize, zoom, or tile-size changes. (Span changes
through edge resize, Behavior 4b, never move the anchor.)

**Spans and the ripple.** Occupancy is rectangle-aware: a slot is "occupied"
if it falls inside any tile's rectangle. The insert-and-shift run collects
only **single-slot (1 × 1)** occupants along the W-wrapped chain; a
multi-slot tile and the `col ≥ W` edge are **walls**. If the run reaches a
wall before a gap absorbs it, the insertion is rejected and the dragged tile
returns home — no overlap is ever created. A multi-slot tile is moved by
title-bar drag like any other, but it is *placed* only when its whole
rectangle lands on otherwise-free slots; otherwise the drop is rejected. A
spanned tile emits no ripple of its own. (Pushing neighbors aside on growth —
the *push* model — is deferred; v1 grows into free desktop only.)

### 4b · Edge resize — spanning a tile across slots [DRAFT]

The **east** and **south** edges of every tile carry a thin (~6px) resize
band; the cursor changes to a resize affordance there, while the title bar
(drag-move, Behavior 4) and tile content keep their semantics. Dragging the
east band changes the tile's **colspan**, the south band its **rowspan**, in
whole-slot increments snapped to the grid pitch. Only east/south growth is
offered in v1, so the tile's **anchor never moves** (Behavior 4's stored-slot
stability holds).

Span is clamped by the **Block rule**: a tile may grow only into slots that
are empty or already its own. Growth stops at the first slot inside another
tile's rectangle — the candidate span is clamped to the largest rectangle
from the anchor that overlaps no other tile. To grow past a neighbor, move
the neighbor first (Behavior 4). Shrinking is always allowed down to the
1 × 1 minimum; freed slots become gaps. While resizing, a preview outline
shows the candidate rectangle; `Esc` cancels (the span returns to its prior
value); mouse-up commits the clamped span. A spanned tile that loses its
backing leaf (closed) drops its anchor and span together, leaving the whole
rectangle as gaps (Behavior 2). New tiles (seed, reconcile, split, open) are
always 1 × 1.

### 5 · Focus and keyboard navigation [DRAFT]

Directional focus (the existing `focus_left/right/up/down` actions) moves to
the nearest occupied slot in that direction — same row/column preferred,
then nearest Euclidean slot distance; no candidate = no-op. `focus_next` /
`focus_prev` follow row-major sequence order. Marks (`'` + key) work
unchanged and auto-pan to reveal the target tile. All per-screen key
contexts behave as in tiling — desktop changes where tiles sit, not what
they are.

### 6 · Desktop grid configuration [DRAFT]

`desktop_grid_cols` / `desktop_grid_rows` live in `Preferences` (persisted,
default 2 × 2): how many tiles fit the viewport per axis, one global
setting for all tabs. Tile size derives from it (Behavior 3) — a 3×2 grid
shows six full tiles per screen. Runtime configuration uses a small text
overlay in the existing `ActiveOverlay` family (the `TagInput`/`Rename`
pattern — yalda-gpui has no `:` command line), accepting `{cols}x{rows}`,
clamped to `[1, 12]` per axis, reachable via `Ctrl-W p` and the layout
menu (which also offers direct mode selection without cycling). The grid
column count is also the effective width W. Changing it re-renders
immediately; slot addresses are grid-independent, so no migration occurs —
tiles keep their slots and simply resize in place (which can change what
overlaps the viewport, never what neighbors what).

### 7 · Persistence [DRAFT]

`PersistedTab` gains an optional `desktop_slots` vector of
`(window_id, row, col)`, keyed by the same stable `PersistedLeaf.id` the
layout snapshot already uses (NOT positional alignment — order-aligned bare
pairs would silently scramble on any length/order mismatch; id-keyed
entries degrade to reconciliation instead). Absent field or unmatched ids =
seed/reconcile on the first render after restore (W needs a measured cell
size, so restore-time seeding is deferred to first paint). Existing
`workspace.json` files load unchanged. `pan` is not persisted; on restore
it reveals the focused tile. The preferences fields persist with the
existing `Preferences` round-trip.

Tile spans persist in a **separate optional** `desktop_spans` vector of
`(window_id, rows, cols)`, keyed by the same `PersistedLeaf.id`, holding only
non-default (≠ 1 × 1) tiles. Keeping spans parallel to `desktop_slots`
(rather than widening the slot tuple) means older `workspace.json` files —
which have no `desktop_spans` — load with every tile at 1 × 1, and a new file
with no spanned tiles omits the field entirely. An id present in
`desktop_slots` but absent from `desktop_spans` is 1 × 1; a span id with no
matching anchor is dropped on reconcile.

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
- `Span { rows: u32, cols: u32 }` — each ≥ 1; default 1 × 1.
- `DesktopState` on `Tab<C>`:
  - `slots: Vec<(WindowId, Slot)>` — sorted by anchor; placement and sequence.
  - `spans: HashMap<WindowId, Span>` — per-tile extent; absent = 1 × 1.
  - `pan: (f32, f32)` — viewport offset; transient-but-kept across mode
    switches, not persisted. Plain floats: the engine (and all of
    `workspace.rs`) stays free of gpui types; the view layer converts at
    the boundary.
  - `drag: Option<DragState>` — dragged id, grab offset, pointer position,
    resolved drop target; never persisted.
  - `resize: Option<ResizeState>` — resized id, edge (East/South), pointer;
    never persisted.
- `Preferences { desktop_grid_cols, desktop_grid_rows, .. }`.
- `PersistedTab { desktop_slots: Option<Vec<(u64, u32, u32)>>,
  desktop_spans: Option<Vec<(u64, u32, u32)>>, .. }` — `(id, row, col)` and
  `(id, rows, cols)`.

## Interfaces

Desktop layout engine (in `workspace.rs`, pure data — *module-internal*,
called by the GPUI view layer):

- `seed(leaves, w) -> slots` — first-entry row-major placement.
- `reconcile(slots, leaves, focused, w) -> slots` — restore the Behavior-2
  invariant.
- `insert_shift(slots, spans, dragged, target, w) -> slots` — Behavior-4 drop,
  rectangle-aware (multi-slot tiles are walls; a spanned dragged tile is placed
  only if its whole rectangle is free).
- `occupant(slot) -> Option<WindowId>` / `rect_of(id) -> (Slot, Span)` —
  rectangle-aware occupancy used by drops, hit-testing, and resize clamping.
- `clamp_span(slots, spans, id, edge, desired) -> Span` — Behavior-4b Block-rule
  clamp: the largest east/south growth from the anchor overlapping no other tile.
- `spatial_neighbor(slots, from, direction) -> Option<WindowId>` — Behavior-5.
- `tile_rect(slot, span, tile_px, gutter) -> Bounds` / `drop_target(point, pan,
  tile_px, gutter) -> Slot` — geometry, *external* to the render path.

Events / messages: none (all interactions are direct view mutations).
Data ownership: `DesktopState` owns placement; the `Layout<C>` tree owns
content; `Preferences` owns tile size.

## Constraints

- Window resize, text zoom, and grid changes MUST NOT mutate stored
  slots. The only slot mutations are seeding, reconciliation, and drops.
  (Pixel size is viewport-derived by design — grid revision; the original
  fixed-pixel sizing was superseded by user feedback after runtime use.)
- The slot map is geometry only. Any code that needs "which windows exist"
  keeps reading tree leaves; the Behavior-2 invariant is maintained at the
  engine boundary, not assumed by callers.
- Engine functions are pure and unit-tested headlessly. Drag gestures and
  panning feel are GPUI-event-driven and need a human runtime check
  (per the dev-system definition of done).
- Per-frame cost in Desktop mode is O(visible tiles), not O(all tiles).
- Marks, tags, rails, and the buffer pool are integration points, not
  dependencies — desktop mode must not require changes to any of them.
- Tile rectangles never overlap — every drop, resize, reconcile, and seed
  preserves it. The Block rule (Behavior 4b) and the wall semantics
  (Behavior 4) enforce non-overlap by *clamping or rejecting*, never by
  silently truncating a tile. Resize changes span only; it never moves an
  anchor or another tile.

## Revision History

- 2026-06-09 — Initial draft (clarified with user: ordered-shelf model,
  stored slots with drop-time-only shifting, 120×40 global tile size,
  fifth LayoutMode, insert-and-shift drops).
- 2026-06-09 — Adversarial review folded in (user-authorized): focused
  tile exempt from culling + canvas-root action wiring; W-wrapped
  successor chain pinned (col ≥ W tiles outside ripples); id-keyed
  `desktop_slots` persistence; LayoutMode unknown-variant fallback +
  old-binary blast radius; overlay (not `:` command) for size config;
  engine stays gpui-free; restore-time seeding deferred to first paint;
  drag-arm focuses; Esc at canvas root.
- 2026-06-10 — Grid revision (user feedback after runtime use): tile size
  is now derived from a configured viewport grid (`cols × rows` of tiles,
  default 2×2) instead of fixed mono-cell dimensions; W = grid cols. Menu
  gains direct layout-mode selection and the grid input.
- 2026-06-10 — **Tile span (edge resize), v1.** A tile may span a rectangle
  of slots; the east/south edges resize it in whole-slot steps (Behavior 4b).
  The Behavior-2 invariant generalizes from one-slot-per-tile to
  non-overlapping rectangles; occupancy becomes rectangle-aware; the
  insert-shift ripple treats multi-slot tiles (and `col ≥ W`) as walls and
  rejects an unabsorbable insertion (Behavior 4). Collision policy is **Block**
  (grow into free desktop only) — the *push* model is deferred. Spans persist
  in a parallel optional `desktop_spans` vector (1 × 1 default; old files load
  unchanged). Anchors never move on resize. **Implemented and runtime-smoked
  the same day** — engine is headlessly unit-tested (9 cases: rectangle-aware
  occupancy, Block clamp, wall-rejected inserts, span persistence); the
  east/south bands + live clamped preview were confirmed by hand.
