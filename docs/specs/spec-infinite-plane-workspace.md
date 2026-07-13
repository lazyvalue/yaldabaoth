# Infinite Plane Workspace — Signed Grid + Semantic-Zoom Camera

**Status:** DRAFT
**Last updated:** 2026-07-12
**Owner:** workspace model (`Tab<C>` interior → `Plane`)

## Builds On

- **`docs/specs/spec-desktop-mode.md`** — ships the tile/slot geometry this spec
  generalizes: anchor + span rectangles, non-overlap invariant, edge-resize,
  reconcile, per-frame viewport culling, "focused tile always renders." WHY:
  that engine is the plane's placement layer; HOW: this spec *promotes* Desktop
  from a per-tab `LayoutMode` you toggle into to the **intrinsic and only**
  interior of a workspace, **widens `Slot` from `u32` to `i32`** (all
  directions), and adds a **camera** (pan + semantic zoom + reset-to-origin)
  that desktop mode lacks. It **supersedes** desktop mode's "fifth layout mode"
  framing and its row-major insert-and-shift shelf (Behavior 4 there). The
  rectangle-occupancy and Block-rule-resize *logic* carry over, but signing the
  coordinate is **not** a pure type change — several reused functions change
  semantics (the `0` wall vanishes, the bounding box gains a min corner). D1
  enumerates exactly which; "reused unchanged" in Interfaces means *type-only*,
  and is listed separately from the functions that need real edits.
- **`docs/specs/spec-tiles-and-apps.md`** — a tile holds exactly one `App`
  (`Buffer` | `Agent`). WHY: the plane places tiles; it does not change what a
  tile contains. HOW: this spec touches placement/geometry/camera only; App
  contents, the buffer pool, and agent sessions are untouched.
- **`docs/specs/spec-jump-panel.md`** — the root sidebar lists each
  non-ephemeral `Tab` as a **"Workspace"** and switches `active_tab` to select
  one. WHY: this is where the code's `Tab` == the user's *workspace*; HOW: this
  spec adopts that vocabulary (workspace = plane = one `Tab`) and leaves the
  panel's per-workspace navigation as-is — it gains nothing beyond each row now
  pointing at a plane.
- **`docs/specs/spec-layout-patterns.md`** — defines the `LayoutMode` family
  (Manual/MasterStack/Monocle/Columns) and tags/marks. WHY: this spec removes
  the layout-mode *choice* from the user model (every workspace is a plane);
  HOW: `Ctrl-W Space` mode cycling and the split-tree operations
  (`Ctrl-W s/v/c/o`) are retired from the user surface. Marks and tags remain
  applicable to tiles.

## Overview

A **workspace** is an **infinite Plane**: an unbounded grid of **slots**
addressed by *signed* coordinates. Slot `(0, 0)` is the **origin** — the
canonical center every plane starts at and that reset-to-origin returns the
**view** to. The grid extends without bound in all four directions.

In yalda's code the user-facing "workspace" is the `Tab<C>` (see
`spec-jump-panel.md`); the app owns a collection of them (`Workspace<C>.tabs`).
**Multiple planes = multiple workspaces** is therefore already the shape of the
tree — this spec changes the *interior* of one workspace, not the collection.

Named entities introduced here:

- **Plane** — a workspace's unbounded slot grid. Concretely: the existing
  per-`Tab` `DesktopState` (placement) plus the `Camera`, with `Slot` widened to
  signed. The `Layout<C>` tree remains the **content owner** (leaves hold the
  live `Window<C>`); the plane owns **geometry + camera** only (the
  slot-map-alongside-frozen-tree shape from desktop mode).
- **Slot** — `{ row: i32, col: i32 }`. A tile's anchor; the origin is `(0, 0)`.
- **Tile** — a `Window<C>` placed at an anchor `Slot` with a `Span`
  (`rows × cols`, each ≥ 1), occupying a rectangle of slots. Tiles never
  overlap (invariant inherited from desktop mode).
- **Camera** — `{ pan: (f32, f32), zoom: Detail }`. Pure **view** state over the
  plane; it never moves a tile. `pan` is the plane point at the viewport's
  top-left, expressed in **slot units** (fractional; pitch-independent — a pan of
  `(2.5, -3.0)` means the same plane location at every Detail). Pixels are derived
  at render time as `pan · slot_pitch(zoom)`. `zoom` is the current **Detail**
  level.
- **Detail** — the discrete semantic-zoom level: `Full` (0) · `Card` (−1) ·
  `Minimap` (−2). Lower = zoomed further out = cheaper, coarser tile
  representation (Behavior 3). One in-between representation per step; not a
  continuous scale factor.
- **Reset-to-origin** — a camera-only action: `pan → (0,0)`, `zoom → Full`.
  Tiles are not moved or re-seeded.

## Behaviors

### 1 · A workspace is a plane; no modes, no splits [DRAFT]

Every workspace's interior is a Plane. The `LayoutMode` *choice* is removed from
the user surface: there is no mode cycling (`Ctrl-W Space`), no manual split
tree (`Ctrl-W s/v/c/o`), no MasterStack/Monocle/Columns. Creating a workspace
creates an empty plane whose camera rests at the origin (`pan=(0,0)`,
`zoom=Full`). Switching workspaces (jump panel) switches which plane is shown;
each plane keeps its own tiles and its own camera.

**Tab-management gestures are re-cast as workspace gestures, not removed.** The
code's `Tab` == a workspace (a plane), so the existing tab actions keep working
under plane vocabulary: `NewTab` (Cmd+T) = **new plane**, `CloseTab` = **close
plane**, `next_tab` / `prev_tab` = **switch plane** (equivalent to picking the
next/prev row in the jump panel). Only the *split-within-a-tab* surface
(`Ctrl-W s/v/c/o`, mode cycling, MasterStack/Monocle/Columns) is retired — those
operate on a split tree the plane no longer has. The user still never sees a tab
*strip* (already gone, main.rs:6540); "workspace" is the only word.

### 2 · Signed, all-directions grid [DRAFT]

`Slot` is signed: tiles may anchor at negative rows/cols, so the plane grows up,
down, left, and right from the origin. All desktop-mode geometry
(occupancy, edge-resize Block rule, non-overlap, reconcile) operates unchanged
over signed coordinates — the only change is the coordinate type.

### 3 · Semantic zoom — discrete Detail levels [DRAFT]

Zoom is **not** a continuous transform. The camera holds one of three Detail
levels; each renders every tile in a level-specific representation, all laid out
on the **same** signed slot grid at a level-specific slot pitch (pixels per
slot):

- **`Full` (0)** — full live tiles, exactly as desktop mode renders today
  (title bar + live App content). The default and the reset target.
- **`Card` (−1)** — each tile collapses to a **card**: title bar / label +
  status glyph + mark badge, **no live App content**. The slot pitch is smaller,
  so more of the plane fits the viewport. Cards are cheap: no transcript, no
  document render.
- **`Minimap` (−2)** — each tile is a **pip** (a filled rect the size of its
  span at a small pitch), showing only the *shape* of the plane — where work
  sits relative to the origin. A label appears only on the focused pip.

Zooming out steps `Full → Card → Minimap`; zooming in steps back
`Minimap → Card → Full`. Zoom is clamped to `[Minimap, Full]`; **zooming in past
`Full`** (a magnifier on one tile) is out of scope for v1. Zoom changes the
Detail level and re-derives the slot pitch; it **never** mutates slots or spans.

Zoom is **anchored on the focused tile** (or the viewport center if nothing is
focused): after a zoom step the camera adjusts `pan` so the anchor slot stays
under the same viewport point. Because `pan` is in **pitch-independent slot
units** (D2), the re-anchor is computed in slot space and needs no
pixel-to-pixel conversion across pitches — the transform is: keep
`anchor_slot − pan` constant in *pixels*, i.e. solve for the new `pan` such that
`(anchor_slot − pan) · slot_pitch(new)` equals the anchor's prior on-screen
pixel offset. Slots and spans are never touched.

Selecting a tile at `Card`/`Minimap` (click a card/pip, or focus-navigate to it
then zoom in) returns to `Full` centered on that tile — the primary way back
from a zoomed-out overview to work.

### 4 · Placement — free, origin-seeded [DRAFT]

Desktop mode's row-major **insert-and-shift shelf** (its Behavior 4, with the
`W`-wrapped successor chain and ripple) is **retired** — it assumed a top-left
origin and a wrap width, neither of which survives an all-directions plane.
Placement on the plane is:

- **Seeding a new tile** (agent open, Cmd+O buffer, any new `Window`): the tile
  is placed at the **first free slot on an outward ring-spiral from the origin**
  (origin first, then its 8-neighborhood, then the next ring, …), skipping any
  slot inside an existing tile's rectangle. Deterministic; independent of
  camera. This keeps new work clustered near the origin ("where all workspaces
  start") rather than scattered. Cost: the spiral tests O(ring²) slots, each an
  O(tiles) `occupant` check, so seeding is worst-case super-linear on a sparse
  far-flung plane — but it runs **once per new tile, never per frame**, and for
  the expected tile counts (< ~50, per desktop-mode Constraint) it is trivial. If
  a plane is ever deliberately populated with hundreds of scattered tiles, seed
  the spiral from the first free slot of the occupied bounding box instead — noted,
  not built.
- **Drag to move** (title-bar drag, desktop-mode Behavior 4 gesture): a tile
  moves to the dropped slot if its whole rectangle lands on otherwise-free
  slots; an overlapping drop is **rejected** and the tile returns home. **No
  ripple, no shift** — placement is free, not an ordered sequence.
- **Edge resize** (desktop-mode Behavior 4b): unchanged — Block-rule span growth
  into free slots, west/north pull-to-enlarge moves the tile's own anchor.

Gaps are ordinary: closing a tile drops its anchor + span and never moves a
neighbor. Because there is no sequence, there is no reconciliation *ordering* to
maintain — only the "one anchor per live leaf, no rectangle overlap" invariant
(desktop-mode Behavior 2), enforced by the geometry engine.

### 5 · Panning and focus navigation [DRAFT]

The camera pans on both axes via trackpad / wheel, at every Detail level. Unlike
desktop mode, **pan is not clamped to the occupied bounding box** — the plane is
infinite in all directions, so the viewport may travel into empty space (an
empty plane is legitimately just origin + dot grid). A soft **recenter affordance**
(Behavior 6) exists precisely because unbounded pan can lose the tiles.
Keyboard focus changes auto-pan the minimum needed to reveal the focused tile
(inherited from desktop mode). Empty plane shows a faint dot grid at the current
slot pitch, with the origin slot marked distinctly.

**Focus traversal.** Spatial directional focus (`spatial_neighbor`, unchanged —
nearest occupied tile in a direction) is the primary motion. `focus_next` /
`focus_prev` are **retained** and cycle over `slots` in signed row-major order —
i.e. **reading order**: top-to-bottom, left-to-right across the whole plane,
origin-relative. This is a deliberate choice, not an accident of D3's dropped
*sequence* meaning: there is no insert-and-shift, but a stable, predictable
traversal order still has value, and reading-order is the least surprising one.
(Desktop mode's row-major order *was* also the shelf sequence; here the two are
decoupled — the order remains for traversal, it just no longer drives
placement.)

### 6 · Reset-to-origin [DRAFT]

A single action returns the **camera** to the origin at full detail:
`pan → (0, 0)`, `zoom → Full`. It is **view-only** — no tile moves, no tile is
re-seeded, focus is unchanged. This is the "I panned/zoomed away and want to get
back" gesture and the answer to "where all workspaces start": every plane's
origin is the same canonical `(0,0)@Full`.

### 7 · Persistence [DRAFT]

A plane persists its **tiles** (already: `desktop_slots` / `desktop_spans` in
`PersistedTab`, widened to signed — see Data Model) **and its camera** (new:
`pan`, `zoom`). Persisting the camera means a workspace reopens exactly where you
left the view; reset-to-origin is then a meaningful, distinct action rather than
the only state. Existing `workspace.json` files load transparently: their
non-negative `desktop_slots` are valid signed slots (the old top-right quadrant),
and an absent camera restores as origin+Full. Document/agent durability (WAL,
ACP session list) is unaffected — only the plane arrangement + view is stored.

**`layout_mode` is ignored on load.** Every workspace is a plane, so the loader
**forces `Plane` (Desktop geometry) for every tab regardless of the persisted
`layout_mode`** — a `workspace.json` that says `"master_stack"` / `"monocle"` /
`"columns"` loads as a plane, seeding tiles from `desktop_slots` if present, else
reconciling the tree leaves onto the plane by origin ring-spiral (Behavior 4).
The retired modes had no `DesktopState`, so those tabs seed fresh; this is a
one-time reflow on first load of the new build, not data loss (the tree leaves —
the actual content — are preserved). The `LayoutMode` enum may remain internally
as a single `Plane` value or be deleted; either way the *field* is no longer
authoritative.

## Data Model

**D1. `Slot` widened — and the reused geometry that changes with it.** [DRAFT]
```rust
pub struct Slot { pub row: i32, pub col: i32 }   // was u32; origin (0,0)
```
`Span { rows: u32, cols: u32 }` is unchanged (extents are non-negative). Signing
the coordinate is **not** a pure type change; three reused functions change
*semantics* and must be edited (an implementer must NOT treat these as "unchanged"):

- **`occupied_extent` (workspace.rs:932) — rewrite.** Today it returns a single
  `(u32, u32)` **max** corner and computes `s.row + sp.rows - 1`; with negative
  anchors that underflows/panics and cannot express a box starting left/above
  origin. It becomes a signed **bounding box with both corners**:
  `Option<(Slot /*min*/, Slot /*max*/)>` (min over anchors, max over
  `anchor + span − 1`, in `i32`). Its consumers (dot-grid extent, any
  recenter/overview framing) read both corners.
- **`clamp_resize` (workspace.rs:675) — behavior change.** Its west/north
  pull-to-enlarge stops at the `0` wall; on an infinite plane **there is no `0`
  wall**, so the anchor may cross into negative slots. The Block-rule clamp
  against *other tiles* is retained; only the origin wall is removed.
- **Pitch / geometry math (`slot_top_left`, `tile_rect`, `drop_target`) —
  signed.** Pixel positions become `slot · pitch − pan_px`; negative slots map to
  negative desktop pixels (left/above the origin), which the view offsets by pan.

The genuinely type-only reuses (logic identical, just `i32`): `occupant`,
`rect_of`, `set_anchor`, `spatial_neighbor`, `span_of`. Those are the only
functions the Interfaces "reused unchanged" line covers.

**D2. `Camera`.** [DRAFT]
```rust
pub enum Detail { Full, Card, Minimap }          // 0, -1, -2

pub struct Camera {
    pub pan: (f32, f32),   // viewport offset in plane pixels at current Detail
    pub zoom: Detail,      // current semantic-zoom level
}
impl Default for Camera {                         // origin
    fn default() -> Self { Self { pan: (0.0, 0.0), zoom: Detail::Full } }
}
```

**D3. Plane state (on `Tab<C>`).** [DRAFT] The existing `DesktopState` gains the
camera and loses the shelf-ordering assumptions:
```rust
pub struct DesktopState {          // the Plane's geometry + camera
    pub slots: Vec<(WindowId, Slot)>,   // anchors; no longer a sequence
    pub spans: HashMap<WindowId, Span>, // sparse; absent = 1×1
    pub camera: Camera,                 // ← replaces bare `pan: (f32,f32)`
    pub drag: Option<DesktopDrag>,      // transient; never persisted
    pub resize: Option<DesktopResize>,  // transient; never persisted
    pub last_reveal: Option<WindowId>,  // auto-pan bookkeeping
}
```
`slots` stays sorted by `Slot` for deterministic iteration; row-major order no
longer drives *placement* (no insert-and-shift) but still defines the
`focus_next/prev` traversal (Behavior 5). `LayoutMode` is retired from the user
model and ignored on load (Behavior 7); if the enum is retained internally it
collapses to a single `Plane` value.

**D4. Persistence tuples widened + camera added.** [DRAFT]
```rust
pub struct PersistedTab {
    pub desktop_slots: Option<Vec<(u64, i32, i32)>>,  // (window_id, row, col) — i32
    pub desktop_spans: Option<Vec<(u64, u32, u32)>>,  // unchanged
    pub camera: Option<PersistedCamera>,              // ← new; absent = origin
    // ...
}
pub struct PersistedCamera { pub pan: (f32, f32), pub zoom: Detail }
```
`Detail` serializes as `"full" | "card" | "minimap"` via a **hand-rolled
`Deserialize`** (NOT `#[derive]`) that falls back to `Full` on an unknown string
— mirroring `LayoutMode`'s hand-rolled impl (workspace.rs:449). A derived
`Deserialize` would hard-error on an unknown variant and, per the snapshot
loader's "failed parse ⇒ discard snapshot" rule, silently reset the whole
workspace arrangement on a downgrade. The same discipline applies to
`PersistedCamera` (a bad `zoom` degrades to `Full`, not a dropped snapshot).

## Interfaces

Camera + placement operate through the plane engine (in `workspace.rs`, pure
data, *module-internal* — called by the GPUI view layer):

- **`zoom_in()` / `zoom_out()`** — step `Detail` one level, clamped
  `[Minimap, Full]`, re-anchoring pan on the focused tile / viewport center
  (Behavior 3). [DRAFT]
- **`reset_view()`** — `camera = Camera::default()` (Behavior 6). [DRAFT]
- **`pan_by(dx, dy)`** — unclamped viewport translation (Behavior 5). [DRAFT]
- **`slot_pitch(detail) -> f32`** — pixels-per-slot for a Detail level; the one
  place Full/Card/Minimap sizes are defined. [DRAFT]
- **`seed_slot(&self) -> Slot`** — first free slot on the origin ring-spiral
  (Behavior 4). Replaces desktop-mode's `first_free_slot` shelf variant. [DRAFT]
- Reused **type-only** from desktop mode (logic identical, `Slot` now `i32`):
  `occupant`, `rect_of`, `set_anchor`, `spatial_neighbor`, `span_of`,
  `sequence_neighbor` (still drives `focus_next/prev`, Behavior 5). Reused with
  **semantic edits** (D1): `occupied_extent` (signed min+max box), `clamp_resize`
  (no `0` wall), `slot_top_left` / `tile_rect` / `drop_target` (signed pixels),
  `reconcile` (now order-free: invariant is non-overlap + one-anchor-per-leaf,
  no sequence). [DRAFT]

**Commands / bindings** (indicative — final keys are a runtime detail; note the
macOS `Ctrl`+digit / `Ctrl-Tab` unreliability from CLAUDE.md, so prefer chord
*sequences* or `Cmd`): workspace zoom out / in and reset are distinct from the
`Cmd+=/-/0` **document text** zoom (INV-UX-13). Proposed:
`Ctrl-W -` zoom out · `Ctrl-W =` zoom in · `Ctrl-W 0` reset-to-origin (each a
two-key sequence, so the digit is a plain key), plus `Cmd`/`Ctrl`+scroll to
zoom and trackpad drag to pan. `Ctrl-W =` is **reclaimed** from the retired
`Equalize` split action (main.rs:3568, `spec-layout-patterns.md` B15) — splits
are gone, so the binding is free; the reflow must delete the old `Equalize`
binding, not shadow it. [DRAFT]

**Events / messages:** none — all interactions are direct view mutations of the
active plane's `DesktopState`. **Data ownership:** the plane (`DesktopState`)
owns geometry + camera; the `Layout<C>` tree owns tile content; `Preferences`
owns slot-pitch constants.

## State Machine

Camera Detail (per plane):

```
   reset_view ─────────────► Full ◄──── zoom_in ──── Card ◄──── zoom_in ── Minimap
                              │                        ▲                      ▲
                              └──── zoom_out ──────────┘──── zoom_out ────────┘
   (Full is the reset target; clamp at both ends)
```

`pan` is a free real-valued offset at every Detail; `reset_view` sets it to
`(0,0)` and forces `zoom = Full`.

## Verification

Most of this is headlessly testable via `verify_harness.rs` (see CLAUDE.md
§ Verification harness); flag only the documented genuine gaps as `NEEDS-RUNTIME`.

- **Geometry engine — headless unit tests.** Signed `occupied_extent` (negative
  anchors, min+max box), `clamp_resize` with no `0` wall (anchor crosses into
  negatives, still Block-clamped by neighbors), `seed_slot` ring-spiral
  (origin-first, skips occupied rectangles), free-drop accept/reject (no ripple).
  Pure functions, no GPUI.
- **Camera state machine — headless.** `zoom_in`/`zoom_out` clamp at
  `[Minimap, Full]`; `reset_view` ⇒ `pan=(0,0), zoom=Full`; the zoom re-anchor
  keeps the focused slot under the same viewport pixel (assert on computed `pan`
  in slot units).
- **Persistence round-trip — headless.** Old `workspace.json` (unsigned slots,
  no camera, `layout_mode:"master_stack"`) loads as a plane at origin; `Detail`
  and `LayoutMode` unknown strings fall back rather than dropping the snapshot.
  Tests MUST NOT touch `~/.yalda` (use the `*_PATH_OVERRIDE` / `None`-under-`cfg(test)`
  seam).
- **LOD render + culling — layout probe.** At `Card`/`Minimap`, assert cheap
  placeholders paint (no live transcript/doc), and that a tile off-viewport is
  culled while the **focused** tile still paints (C5). Use `probe_bounds` /
  `layout_probe_*`.
- **Genuine gaps (`NEEDS-RUNTIME`).** Trackpad pan / `Cmd`+scroll zoom *feel*;
  exact colors/glyphs of Card/Minimap; and the plane-zoom **keybindings** — even
  as chord *sequences* (`Ctrl-W` then `-`/`=`/`0`), the post-leader digit is the
  4th documented key gap (`simulate_keystrokes` is focus-accurate, not
  OS-accurate). A human runtime check confirms the bindings fire.

## Constraints

- **C1. The camera never moves a tile.** Pan, zoom, reset, and window resize
  mutate `Camera` (and viewport) only. The only slot/span mutations are seeding,
  drops, edge-resize, and reconcile — exactly desktop mode's set, minus the
  shelf ripple. (Inherits desktop-mode C-constraints.)
- **C2. Semantic zoom is discrete, not a transform.** There is no arbitrary
  scale matrix over the element tree. Each Detail level is a distinct
  representation at its own slot pitch; GPUI renders cheap placeholders
  (Card/Minimap), not shrunk live tiles. Per-frame cost stays O(visible tiles),
  and *lower* at Card/Minimap than at Full.
- **C3. Tiles never overlap** — every seed, drop, resize, and reconcile
  preserves the rectangle-non-overlap invariant (desktop-mode Behavior 2/4b),
  by clamping or rejecting, never truncating.
- **C4. The plane engine stays gpui-free** — plain `f32`/`i32`; the view layer
  converts at the boundary (as desktop mode does).
- **C5. The focused tile always renders** even when panned/zoomed out of view,
  carrying its focus handle + per-screen `on_action` wiring; plane-level actions
  (pan, zoom, reset, drag-cancel) are additionally wired on the canvas root so
  they survive any single tile's absence. (Inherited from desktop mode.)
- **C6. Out of scope for v1:** zoom-in past `Full` (per-tile magnifier),
  continuous/pinch zoom, per-plane slot-pitch overrides, cross-plane tile move,
  a plane minimap-inset overlay, and re-introducing any manual split/tiling mode.
  Marks and tags remain applicable to tiles and are not modified here.

## Revision History

- 2026-07-12 — Initial DRAFT. Promotes desktop mode from a per-tab `LayoutMode`
  to the intrinsic workspace interior: `Slot` widened to signed `i32` (all
  directions), a `Camera` (pan + discrete semantic-zoom `Detail` +
  reset-to-origin) added to `DesktopState`, the row-major insert-and-shift shelf
  retired in favor of free placement + origin ring-spiral seeding, camera
  persisted per workspace. Reuses desktop-mode's tile-rectangle geometry engine
  (occupancy, Block-rule edge resize, culling, focused-tile-always-renders)
  unchanged. `spec-desktop-mode.md`'s "fifth layout mode" framing is superseded.
- 2026-07-12 — Adversarial-review pass folded in (verdict was RETHINK): B1 — D1
  now enumerates the reused functions that change *semantics* under signing
  (`occupied_extent` → signed min+max box; `clamp_resize` loses the `0` wall;
  signed pixel geometry), split from the type-only reuses, killing the false
  "unchanged" claim + the `u32` underflow. B2 — Behavior 5 pins `focus_next/prev`
  to signed row-major reading order (traversal decoupled from placement). N1 —
  `pan` redefined in pitch-independent slot units so zoom re-anchor + persistence
  share one basis. N2 — tab gestures re-cast as new/close/switch **plane**. N3 —
  `layout_mode` ignored on load (all tabs forced to Plane; retired-mode tabs
  reflow once). N4 — seed cost bounded + noted per-tile-not-per-frame. V1 —
  `Ctrl-W =` reclaimed from retired `Equalize`. V2 — `Detail`/`PersistedCamera`
  use hand-rolled unknown-variant `Deserialize`. V3 — Verification section added
  (headless seams + the 3 genuine gaps + the post-leader-digit key gap).
