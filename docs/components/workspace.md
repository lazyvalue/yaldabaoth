# Component: Workspace

**Status:** living
**Component token:** `Workspace` (⇒ `UXI-Workspace-N`)

## Description

A **Workspace** (`Workspace<App>`, formerly `Tab<App>` — the `tab` vocabulary was
eradicated in ADR-0028 §5 / T007) is a plane of tiles: it owns an n-ary **layout
tree**, a focused-tile pointer, and a required-private **`project: ProjectId`**
foreign key into the `Projects` store. Interior nodes of the tree are **Splits**
(`Layout::Split`) — a direction (`H` = stacked, `V` = side-by-side) plus weighted
children — and each leaf is a **Tile** (`Window<App>`, a stable `WindowId` holding
exactly one `App`: `Buffer`, `Agent`, or `Linear`). The code-level struct for a
tile is still called `Window`, but in discussion we say **tile**.

A workspace **belongs to a project** and does NOT carry its own cwd: its working
directory is the project's, resolved at the point of use (`projects.cwd_of(
workspace.project())`) and **never cached** (`UXI-Project-2`, ADR-0028 §3). A new
workspace inherits the active one's project.

The container that owns the list of workspaces (one per OS-level **Frame**) is
**`Frame<App>`** (formerly the code's `Workspace<App>` — renamed in T007 so the
type name matches the user-facing "workspace" = the plane). The frame carries the
workspace strip (per-workspace label, active marker, click-to-select, rename,
next/prev, new/close, `ctrl-<n>` jump by number) and the buffer pool; the tag bar
drives tile tags. Non-ephemeral workspaces are numbered `1..N` in the jump panel
(globally, across all projects), and those numbers are the jump targets.

The full hierarchy: **`Frame` → `Project`s → `Workspace`s → `Window` tiles**, with
`Project` the ownership axis over workspaces and agent sessions
(`docs/components/project.md`).

## References

- `docs/components/project.md` (`UXI-Project-1..8`) — the `Project` a workspace
  belongs to; the `ProjectId` FK + derived-cwd contract (`UXI-Project-2`).
- ADR-0028 — Projects as the top-level primitive + the `Tab`→`Workspace`,
  container→`Frame` rename (§5) this Description now reflects.
- `docs/specs/spec-workspaces-and-splits.md` — the n-ary split/layout tree +
  persistence (retroactive spec of shipped behavior; workspace/frame vocabulary).
- `docs/specs/spec-layout-patterns.md` — tile tags + automatic layout modes.
- `docs/specs/spec-desktop-mode.md` — desktop tile sizing / master layout; the
  tile/slot geometry engine (`Slot`, `Span`, `DesktopState`, occupancy,
  Block-rule edge resize, culling) that the plane model below reuses.
- `docs/specs/spec-infinite-plane-workspace.md` — DRAFT deep design for the
  **infinite-plane** model (`UXI-Workspace-2..7`): a workspace *is* one unbounded
  signed-coordinate plane with a pan/semantic-zoom camera. When those UXIs ship,
  this Description is rewritten around the plane and the split/layout-mode text
  above becomes historical.
- Migrated from `docs/ux-invariants.md` INV-UX-11 (`ctrl-<n>` workspace jump). That
  entry is now `→ migrated here`.

## UX invariants

### UXI-Workspace-1 — `ctrl-<n>` jumps to the n-th workspace (the number the panel shows)

**Statement.** The jump panel numbers **non-ephemeral** workspaces `1..N` (the
`idx + 1` badge), and `ctrl-1`…`ctrl-9` / `ctrl-0` (the 10th) jump straight to
that workspace. The displayed digit and the keystroke target always agree because
both skip ephemeral virtual workspaces (ADR-0021) — `goto_workspace_number(n)`
selects the n-th non-ephemeral tab. A digit past the last workspace is a no-op.

**Applies to.** `main.rs`: the `GotoWorkspace1..10` actions + `ctrl-<n>`
bindings (app-global, `None` context), `goto_workspace_number`, and the
`WorkspaceNavExt::workspace_nav` helper wired onto every screen root (the action
needs a handler in the focused element's ancestry — same discipline as
`toggle_jump_panel`). `jump_panel_view.rs`: the workspace-row number badge.

**Edge.** An **empty-layout** workspace renders a bare div with no action
handlers (chrome.rs), so global keys (incl. `ctrl-<n>`, `ctrl-tab`, `cmd-t`)
don't dispatch while sitting on one — a pre-existing, transient edge state, not
specific to this binding.

**Why.** Direct numeric workspace switching, matching the visible numbering.

**Status.** `implemented` (headless).

**Enforcement.** `verify_harness.rs`: `ctrl_digit_switches_workspace` (full
keymap→action→handler dispatch: `ctrl-3` then `ctrl-1`, plus past-the-end no-op)
and `workspace_number_skips_ephemeral` (numbering skips the ephemeral tab).

---

_The invariants below (`UXI-Workspace-2..7`) define the **infinite-plane** model
(`docs/specs/spec-infinite-plane-workspace.md`). All are `implemented` (headless-
guarded; the `Ctrl-W 0/-/=` chord firing is the one `NEEDS-RUNTIME` gap). They built
on the desktop-mode geometry engine and **supersede** the split-tree / layout-mode
behavior in this component's Description above — a workspace is now one infinite
plane; the `LayoutMode` cycle, master-stack, split-resize, and equalize surface are
retired (`SplitH`/`SplitV` remain only as the plane's new-tile mechanism). The
Description prose is due a rewrite around the plane in a follow-up pass._

### UXI-Workspace-2 — A workspace is one infinite, all-directions signed plane

**Statement.** Each workspace's interior is a single **Plane**: an unbounded grid
of slots addressed by *signed* coordinates `Slot { row: i32, col: i32 }`, origin
`(0,0)`, extending without bound up/down/left/right. A tile anchors at a signed
slot and occupies a `Span` (`rows × cols`, each ≥ 1) rectangle; **no two tiles'
rectangles overlap**. There is no split tree, no layout-mode choice, and no `0`
wall — the plane is the only interior. Multiple planes = multiple workspaces
(the code's `Workspace`s, formerly `Tab`s); workspace-management gestures re-cast
as new/close/switch **plane** (`NewWorkspace`→new plane, `CloseWorkspace`→close
plane, `next_workspace`/`prev_workspace`→switch plane; T007 rename).

**Applies to.** `workspace.rs`: `Slot` widened `u32→i32`; `DesktopState` becomes
the plane; `occupied_extent` returns a signed min+max bounding box (not a lone
`u32` max corner); `clamp_resize` drops the origin-wall clamp (keeps the
Block-rule clamp against other tiles). The `LayoutMode` enum + `Ctrl-W s/v/c/o`
split ops + mode cycling are retired from the user surface.

**Why.** The user's model is a boundless spatial canvas per workspace, not a
bounded tiled quadrant; the retired split/mode machinery is what makes "the
workspace is infinite" untrue today.

**Status.** `implemented`.

**Enforcement.** `workspace.rs` desktop_tests: `occupied_extent_signed_min_max_box`
(negative anchors, min+max corners, no underflow), `clamp_resize_west_crosses_origin`
(west growth crosses into negative slots yet still Block-clamps a negative-col
neighbor — proves the `0`-wall removal didn't remove the neighbor wall). Both
negative-control-verified RED-then-green.

**Deviation from plan.** `LayoutMode` was **collapsed to a single `Plane` variant**,
not deleted (≈100 call sites made a stub cleaner; its `Deserialize` maps any old
mode string → `Plane`). `SplitH`/`SplitV`/`split_focused` were **kept** — they are
load-bearing for new-tile creation on the plane; only the *mode / master-stack /
resize / equalize* surface + `Ctrl-W Space` cycling were retired (consistent with
Behavior 1: "split ops retired from the *user surface*"). The `chrome.rs`
`layout_mode==Desktop` gate is now unconditional and `render_layout` (the split-tree
branch) is deleted.

### UXI-Workspace-3 — The camera is view-only; it never moves a tile

**Statement.** A workspace carries a `Camera { pan, zoom }` over its plane. Pan,
zoom, reset, and window resize mutate the camera (and viewport) **only** — they
never change a tile's anchor or span. `pan` is expressed in **pitch-independent
slot units** (a given `pan` names the same plane location at every zoom level;
pixels = `pan · slot_pitch(zoom)`). The only slot/span mutations are seeding,
drag-drop, edge-resize, and reconcile (UXI-Workspace-6).

**Applies to.** `workspace.rs`: `Camera` + `Detail` on `DesktopState` (replacing
the bare `pan: (f32,f32)`); `pan_by`, `zoom_in`/`zoom_out`, `reset_view` mutate
only the camera. The focused tile always renders even when panned/zoomed off
view (carries focus + `on_action` wiring); plane-level actions are wired on the
canvas root (desktop-mode C5, inherited).

**Why.** Separating view from placement is what makes zoom/pan/reset safe and
what lets "reset the view" be a pure navigation gesture rather than a
destructive re-layout.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs`: `ctrl_w_reset_returns_camera_to_origin`
(pan+zoom away, then reset → camera at origin AND `slots`/`spans` unchanged) and
`ctrl_w_zoom_steps_detail` (zoom steps mutate only the camera). Negative-control-
verified (no-op'd `reset_view` → camera stuck; observed RED).

**Deviation from plan.** `pan` is in slot units as specced, but the Statement's
`pixels = pan · slot_pitch(zoom)` is realized as `pan ⊗ (desktop_tile_px ⊗
detail_scale(zoom))` — pitch is **per-axis and viewport-derived**, not a scalar
`slot_pitch` (see UXI-Workspace-4). `zoom_in`/`zoom_out` take an explicit `anchor:
Slot` (focused tile or viewport center, resolved by the caller).

### UXI-Workspace-4 — Zoom is semantic: discrete detail levels, not a scale transform

**Statement.** Zoom is one of three discrete **Detail** levels — `Full` (0, live
tiles), `Card` (−1, title/label/status card, no live content), `Minimap` (−2,
a span-sized pip; label only on the focused pip) — each laid out on the same
signed grid at its own slot pitch. Zoom out steps `Full→Card→Minimap`; zoom in
steps back; clamped to `[Minimap, Full]` (no zoom-in past Full in v1). A zoom
step re-anchors on the focused tile (or viewport center) so it feels centered.
Selecting a card/pip returns to `Full` centered on that tile. Zoom bindings are
distinct from the `Cmd+=/-/0` document-text zoom (`UXI-TextZoom-1`).

**Applies to.** `chrome.rs` (the `render_desktop` path — NOT `screens.rs`) +
`workspace.rs`: per-`Detail` render representations + `detail_scale(detail)` off
the per-axis viewport-derived Full pitch (`desktop_tile_px`); `zoom_in`/`zoom_out`;
frame-level culling renders cheap placeholders at Card/Minimap (per-frame cost
O(visible tiles), *lower* than Full); maximize is Full-only. Bindings live in
`keymap_registry.rs`, reflowed in one pass (`Ctrl-W =`/`Ctrl-W -` reclaimed from
`Equalize`/`ResizeShrink`); `Cmd`/`Ctrl`+scroll zooms.

**Why.** GPUI has no cheap arbitrary-scale transform, and shrunk live tiles are
illegible and expensive; discrete LOD stays legible and gets *cheaper* zoomed
out, which is the whole point of an overview.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs`:
`plane_card_zoom_paints_placeholders_not_live_content` (at Card the `plane-card-{id}`
probe paints while the live `plane-tile-content-{id}` probe is absent),
`plane_focused_tile_renders_when_off_viewport` (focused tile painted with `x+w ≤ 0`
— genuinely off-screen — while an unfocused off-viewport tile is culled; non-vacuous),
`plane_pan_at_card_leaves_transcript_render_flat` (Card render is O(visible), not
live). All negative-control-verified RED.

**Deviation from plan.** `slot_pitch(detail)->f32` was **infeasible** (pitch is
anisotropic + viewport-derived); realized as `detail_scale(Detail)->f32` (Full 1.0 /
Card 0.5 / Minimap 0.2) multiplied against the per-axis `desktop_tile_px` Full pitch.
Input routing (refined after runtime use — see below): `Cmd`/`Ctrl`+scroll steps
zoom at every level; **panning is `Cmd+Shift`+left-drag** (bare scroll no longer
pans — it bubbles so tile content still scrolls). Probe tags added:
`plane-card-{id}`, `plane-tile-content-{id}` (via a new String-keyed
`probe_bounds_dyn`). Exact scroll/drag *feel* is `NEEDS-RUNTIME`.

**Pan gesture (`Cmd+Shift`+drag), added 2026-07-14 after runtime feedback.**
`DesktopState.pan_drag` (transient) + `chrome.rs::desktop_pan_grab` /
`desktop_pointer_move` / `desktop_drop`: `Cmd+Shift`+left mouse-down on the
canvas records the grab; mouse-move sets `camera.pan = start_pan − delta/pitch`
(slot units, at the current zoom pitch), taking precedence over any tile
drag/resize; mouse-up commits + persists. Bare scroll's panning was removed.
**Enforcement.** `verify_harness.rs`: `cmd_shift_drag_pans_the_plane` (real
`simulate_mouse_*` dispatch → the camera pans AND no tile moves; NC: neuter the
pan application → RED) and `cmd_only_drag_does_not_pan_the_plane` (Cmd without
Shift reaches the handler but must not pan; NC: drop `&& modifiers.shift` → RED).
Note: GPUI's simulated dispatch doesn't deliver a *modifier-less* canvas down to
the handler, so the shift requirement is guarded via the Cmd-only case, not a
bare drag.

### UXI-Workspace-5 — Reset-to-origin returns the view to (0,0) at full detail

**Statement.** A single action sets `camera = { pan: (0,0), zoom: Full }` for the
active workspace. It is **view-only** — no tile moves, none is re-seeded, focus is
unchanged. Every plane's origin is the same canonical `(0,0)@Full` — "where all
workspaces start" — so reset is the reliable "get me back to the start" gesture.

**Applies to.** `workspace.rs`: `reset_view` (= `Camera::default()`); `main.rs`
action + binding (indicative `Ctrl-W 0`, a two-key sequence so the digit is a
plain key — the post-leader digit is the known macOS key gap, CLAUDE.md rule 4).

**Why.** Unbounded pan/zoom can lose the tiles entirely; a fixed, well-known home
is what makes an infinite plane navigable rather than a place to get lost.

**Status.** `implemented` (headless; chord-firing is `NEEDS-RUNTIME`).

**Enforcement.** `verify_harness.rs`: `ctrl_w_reset_returns_camera_to_origin` drives
the real keymap (`register_keymap` + `simulate_keystrokes("ctrl-w 0")`) → action →
handler and asserts camera == `Camera::default()` AND `slots`/`spans` unchanged.
Negative-control-verified (no-op'd `reset_view` → RED, camera stuck at
`(7,-4) Minimap`). The binding is `Ctrl-W 0` (a two-key sequence — the plain digit
after the leader). Per CLAUDE.md rule 4, `simulate_keystrokes` is focus-accurate but
not OS-accurate, so the real macOS chord firing is a **human runtime check** (the one
genuine gap for this UXI).

### UXI-Workspace-6 — Placement is free and neighbor-seeded; no insert-shift shelf

**Statement.** New tiles seed at the **first free slot on an outward ring-spiral
centered on the tile the user is on** (the focused tile if placed, else the
last-revealed one, else the origin), skipping occupied rectangles — new work
appears beside the work it came from. Within a ring the scan is in **preference
order, not reading order**: nearest row first, and at equal distance below before
above and right before left. So a new tile opened beside a lone tile lands at
`(row, col + 1)` — **same height, directly to the right** — never diagonally
(bug-0012).
Dragging a tile moves it to the dropped slot iff its whole rectangle lands on
free slots; an overlapping drop is **rejected** (returns home). There is **no**
row-major insert-and-shift ripple. Closing a tile leaves a gap; neighbors never
move. `focus_next`/`focus_prev` traverse `slots` in signed row-major **reading
order** (top→bottom, left→right); spatial directional focus is unchanged.

**Applies to.** `workspace.rs`: `seed_slot_near` / `seed_slot` (ring-spiral,
replaces the shelf `first_free_slot`) and `reconcile_near` (resolves the spiral
center), called from the per-frame plane upkeep in `chrome.rs`; free-placement drop (desktop-mode Behavior 4 gesture, ripple
removed); edge-resize `clamp_resize` unchanged in spirit (Block rule);
`sequence_neighbor` retained for traversal only (decoupled from placement).

**Why.** The row-major shelf assumed a top-left origin and a wrap width — neither
survives an all-directions plane; free placement + origin seeding is the natural
model for a boundless canvas.

**Status.** `implemented`.

**Enforcement.** `workspace.rs` desktop_tests: `seed_slot_spiral_deterministic`
(origin-first, occupied origin ⇒ next lands same-row-right, deterministic),
`seed_slot_near_prefers_same_row_right` (spiral centered on a non-origin tile)
and `free_drop_rejects_overlap_without_moving_neighbors` (overlapping
drop leaves every tile's slot unchanged — no ripple). Both negative-control-verified
RED. On the REAL path: `verify_harness.rs`
`new_tile_lands_same_row_right_of_the_only_tile` — the sole tile parked off-origin,
the user's `Ctrl-W v` handler, slot assigned by the real `chrome.rs` reconcile;
negative-controlled RED twice (origin-centered ⇒ `(0,0)`; row-major ring ⇒
`(0,-2)`). Existing signed-adapted desktop_tests cover the type-only reuses.

**Deviation from plan.** The dead shelf code was deleted (`Slot::succ`, `first_free`,
`seed(leaves,w)`, `insert_shift`, `absorbable_run`, `effective_width`); `reconcile`
lost its `focused`/`w` params and now spiral-seeds slotless leaves (order-free). The
free-drop seam is `DesktopState::free_drop(id, target) -> bool`.

### UXI-Workspace-7 — The plane persists (tiles + camera) and migrates old layouts cleanly

**Statement.** A workspace persists its tiles (signed `desktop_slots`,
`desktop_spans`) **and** its camera (`pan` in slot units, `zoom`), so it reopens
exactly where the view was left (reset-to-origin is then a meaningful distinct
state). Old `workspace.json` loads transparently: unsigned slots are valid signed
slots (the old quadrant); an absent camera restores as origin+Full; a persisted
`layout_mode` (`master_stack`/`monocle`/`columns`/…) is **ignored** — every tab is
forced to a plane, reflowing its tree leaves via origin ring-spiral once (content
preserved, not lost). Unknown `Detail`/`LayoutMode` strings **fall back**
(`Detail`→`Full`) rather than dropping the whole snapshot.

**Applies to.** `persist.rs`: `PersistedTab.desktop_slots` tuple widened to
`(u64,i32,i32)`; new `PersistedCamera { pan, zoom }`; `Detail` + camera use a
**hand-rolled** unknown-variant `Deserialize` (mirroring `LayoutMode`
workspace.rs:449 — a `#[derive]` would hard-error and reset the snapshot). Loader
ignores `layout_mode`. Tests must use the `*_PATH_OVERRIDE`/`None`-under-`cfg(test)`
seam — never touch `~/.yalda`.

**Why.** Persisting the camera is what makes each plane feel like a durable place;
the migration rules prevent an old `workspace.json` (or a downgrade) from silently
wiping a user's arrangement.

**Status.** `implemented`.

**Enforcement.** `tests.rs` (pure serde, no `~/.yalda`):
`plane_persist_round_trips_signed_slots_and_camera` (negative-coord slots + a
non-default camera round-trip faithfully), `old_workspace_json_loads_as_plane_with_origin_camera`
(literal old-format JSON: `u32` slots, no `camera`, `layout_mode:"master_stack"` →
slots intact, camera origin+Full, no panic, snapshot not dropped),
`unknown_detail_zoom_falls_back_to_full` (`"zoom":"hyper"` → `Full`, snapshot kept).
Negative-control-verified (a strict deserialize dropped the snapshot → RED).

**Deviation from plan.** `Detail`'s hand-rolled `Serialize`/`Deserialize` live in
`workspace.rs` (where `Detail` is defined); `PersistedCamera` safely `#[derive]`s serde
because the unknown-variant hardening is inside `Detail`. Stage A left a temporary
negative-slot save clamp that Stage B removed (signed rows/cols now persist directly).
The "force plane / ignore `layout_mode`" + one-time reflow is realized via the
existing seed/reconcile-on-first-render path (retired-mode tabs bulk-seed their
leaves through `seed_slot`).

### UXI-Workspace-8 — Moving or resizing a tile rests the view cell-aligned (no fractional pan)

**Statement.** Committing a **tile move** (drag-drop to a new slot) or a **tile
edge-resize** leaves the camera pan on **whole slot units**, so the plane grid rests
aligned to the viewport the same way tiles align to cells — no residual
fraction-of-a-cell offset. The offending drift comes from the drag **edge auto-pan**
(`pan_by(step/pitch)`, a fractional slot step per frame) and, on the reveal path, from
the right/bottom edge-reveal storing `pan = (x+tile+g−canvas)/pitch`; both are snapped
to the nearest whole slot at commit. **Focus reveal** (`chrome.rs` reveal block) also
cell-snaps, **but only when it actually had to pan** to bring the focused tile into
view — a focus change to an already-fully-visible tile does **not** move the view
(matches the "only pans when the tile isn't fully visible" clause). The deliberate
`Cmd+Shift` free-pan (`UXI-Workspace-4`) is **continuous while dragging** but also
rests cell-aligned **on release** (snapped at mouse-up, not mid-gesture — bug-0009).
The snap is **view-only** (Constraint C1 / `UXI-Workspace-3`): it never touches a
tile's anchor or span.

**Applies to.** `workspace.rs`: `DesktopState::snap_camera_to_slots` (rounds
`camera.pan` per-axis to the nearest integer). `chrome.rs`: `desktop_drop` calls it
after the free-drop commit (drag branch, only when the drag was `active`), after the
edge-resize commit, and on the `Cmd+Shift` free-pan **release** (the `pan_drag`
branch); the `render_desktop` reveal block calls it only when a reveal adjustment
fired.

**Why.** The user drags a tile onto a clean cell but the *view* lands a fraction of a
cell off, so the whole plane looks subtly misaligned — "the window snapped to the grid
but the workspace didn't." Cell-aligning the camera at commit makes the view snap to
the same cells the tile does.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::tile_drag_rests_view_cell_aligned` drives a real
tile drag (`desktop_grab` → `desktop_pointer_move` into the canvas edge band → edge
auto-pan leaves a fractional pan → `desktop_drop`) and asserts the pan is fractional
*before* the drop (non-vacuous) then integral on both axes after, and the un-dragged
tile never moved (view-only). `workspace.rs` desktop_tests:
`snap_camera_to_slots_rounds_and_preserves_slots` covers the pure round + no
slot/span mutation. `cmd_shift_pan_rests_view_cell_aligned` (bug-0009) drives a real
`simulate_mouse_*` `Cmd+Shift` pan, asserts the pan is fractional mid-gesture
(non-vacuous) then integral after release, tiles unmoved. All negative-control-verified
RED (drop-branch `snap_camera_to_slots()` commented out → drag/pan tests fail;
`.round()` dropped → unit test fails).

**Deviation from plan.** The reveal path snaps only when a reveal adjustment actually
fired (a focus change to an already-visible tile leaves the view still, honoring the
"only pans when not fully visible" clause). The drag branch snaps on **any active
drag** (even a rejected/no-op drop) since the edge auto-pan runs independently of
whether the drop commits. `Cmd+Shift` free-pan is continuous mid-drag but snaps on
release (bug-0009 — originally left un-snapped, corrected after user report).
Round-to-nearest is used (not directional)
— on a right/bottom reveal this could in principle clip a tile edge by <½ cell, but
the reveal keeps a full gutter of slack and tiles are ≥1 slot, so it is not observed
in practice; revisit to directional rounding only if a clipped reveal is reported.

### UXI-Workspace-9 — A click in an unfocused tile's body focuses it, and is consumed

**Statement.** A **left mouse-down anywhere in the body (content area) of a Full-detail
tile that is not currently focused** focuses that tile, and that press is **consumed** —
the tile's content never sees it. This is the classic *click-to-focus* model: the first
click on an unfocused tile only focuses; a **second** click interacts with the content
(places a caret, starts a transcript selection, presses a button, hits a wiki link).
Once the tile **is** focused, mouse-down behaves completely normally — nothing is
intercepted or swallowed, so ordinary interaction inside the focused tile is unchanged.

**Scope — deliberately narrow (three carve-outs):**

1. **Title bar and resize bands are NOT covered.** They keep today's one-gesture
   behavior: pressing them focuses the tile *and* arms the move/edge-resize
   (`desktop_grab` / `desktop_resize_grab`; `spec-desktop-mode.md` "arming a drag also
   focuses the grabbed tile", and its ~4px sub-threshold "focus click"). Requiring a
   focus-click first would make dragging or resizing an unfocused tile a two-press
   gesture — a regression in feel. The invariant covers the **content area only**.
2. **Card / Minimap placeholders are out of scope.** At non-Full detail a tile is a
   cheap placeholder with no live content, so there is nothing to consume; whether a
   card/pip click focuses and/or returns to Full is left to the plane-zoom work
   (`spec-infinite-plane-workspace.md` §"click a card/pip", still unimplemented).
3. **Left button only.** Right-click keeps its existing canvas meaning (cancel an
   in-flight drag/resize).

**Applies to.** `chrome.rs` `render_desktop`: the per-tile **content wrapper** child of
`frame` (the `div().flex_1().min_h_0().overflow_hidden().child(inner)` sibling of
`title_bar` and the four resize bands). The handler must run in the **capture** phase —
a bubble-phase handler is too late, because the content child (transcript selection
sink, compose input, buttons) has already acted by the time the event bubbles. It is
gated on `!is_focused`, so it is inert for the focused tile and cannot interfere with
normal interaction.

**Why.** On the plane the only way to focus a tile with the mouse is to hit its title
bar or a resize band — clicking into the tile's content does nothing, so the keyboard
keeps talking to the previously focused tile while the user is looking at (and clicking
in) another one. Click-to-focus is the expected desktop behavior. Consuming the first
click prevents the focus-changing press from *also* mutating the newly focused content
(dropping a caret mid-document, starting a stray selection) — you focus first, then act
deliberately.

**Status.** `implemented`.

**Enforcement.** Two headless guards in `verify_harness.rs`, both driving the REAL
mouse dispatch (`simulate_mouse_down` through the element tree — not the handlers
directly), so they actually exercise `capture_any_mouse_down` + `stop_propagation`:

- `click_in_unfocused_tile_body_focuses_and_is_consumed` — two-tile plane, tile B
  seeded with transcript text; the click point is derived from a REAL painted token
  (`TranscriptView::token_hits`), so it is provably over live content. Asserts
  **(a)** the press focuses B and **(b)** B's `state.focus` stays `Compose` (the
  transcript did not act). **Non-vacuity is structural:** the identical press is
  replayed once B *is* focused and must then flip `state.focus` to `Transcript` —
  so "the content didn't act" cannot pass merely by missing all content.
  *Negative control (observed RED):* handler removed ⇒ focus stays on A
  (`Some(1)` vs `Some(2)`).
- `title_bar_press_on_unfocused_tile_still_focuses_and_arms_drag` — keeps carve-out 1
  honest: a synthetic press on the title-bar strip (derived from the
  `plane-tile-content-{id}` probe, the band directly above it) still focuses **and**
  arms the drag. *Negative control (observed RED):* moving the capture handler from
  the tile body up to the whole `frame` swallows the title-bar press and leaves the
  drag unarmed — the exact widening mistake this guard exists to catch.

**Deviation from plan.** Three things differ from the step-4 plan:

1. **Attach point is the content wrapper, not the frame.** The handler lives on the
   `tile_body` div (sibling of `title_bar` and the resize bands) rather than on
   `frame` with region tests. This makes carve-out 1 true *by construction* instead
   of by conditional — there is no code path where a title-bar or resize-band press
   can reach the focus handler.
2. **Focus is resolved at EVENT time, not gated at render time.** The handler is
   attached unconditionally and checks `focused_window_id() == Some(id)` inside the
   closure. Gating attachment on a render-time `!is_focused` would violate the
   interactive-rows rule in `yux/CLAUDE.md` (a cache hit reuses prepaint whose
   closures captured the previous render's state).
3. **`capture_any_mouse_down` has no button filter.** GPUI 0.2.2 offers no
   `capture_mouse_down(button, …)`, so the left-button check is done inside the
   closure. Also note there was **no prior capture-phase mouse handler in this repo**
   — this is the first (capture phase was previously keys-only).

**Not covered (deliberate).** Card/Minimap placeholders (carve-out 2) remain
unimplemented — clicking a card/pip still does nothing. If that is wanted, it belongs
with the plane-zoom work, not here.

### UXI-Workspace-8 — "New agent" is contextual: a new tile in a workspace, in place in the bare agent view

**Statement.** The workspace `.` menu's **new → agent** (command `new-agent-tile`)
places the new agent tile according to what you are looking at. Both branches land
on the **same in-tile session picker** — only the placement differs:

1. **In a real workspace** (the active workspace is NOT ephemeral) — unchanged:
   a new tile is added to the plane and opens on the picker. Your existing tiles
   stay put.
2. **In the bare agent view** (the active workspace IS an ephemeral virtual
   workspace, ADR-0021 — where a free session you jumped to from the jump panel is
   shown fullscreen) — **no new tile is created**. The single tile swaps **in
   place** to a fresh unbound agent tile showing the picker. The bare agent view
   stays exactly one tile; it never splits.
3. **The session you were looking at is not killed.** Swapping the ephemeral view
   unbinds it, so it returns to being a *free* session (still running, an unbound
   `✦` row in the jump panel) — and it is re-pickable from the very picker the
   swap just opened. Closing a session is `claude-close` and nothing else
   (`UXI-AgentTile-22`).
4. The ephemeral workspace stays ephemeral: it is still torn down on switch-away,
   and the picker's cwd is still its project's (`UXI-Project-2/-6`).

**Applies to.** `main.rs` `dispatch_menu_command`'s `"new-agent-tile"` arm (the
`workspace.active_is_ephemeral()` branch); `agent_ui.rs`
`open_new_agent_selector_in_place` (the in-place swap: fresh `AgentTile::new()` via
`set_screen`, `start_server_pump` + `refresh_roster`); `workspace.rs`
`Frame::active_is_ephemeral`.

**Why.** The bare agent view is "one agent, fullscreen" — splitting a second tile
into it produces a cramped two-pane layout the user never asked for and that
evaporates on the next workspace switch. "New agent" there means "show me a
different agent," not "tile another one beside this one."

**Status.** `implemented` (headless, on the real dispatch path).

**Enforcement.** `verify_harness.rs::new_agent_splits_in_a_workspace_and_swaps_in_place_in_a_bare_agent_view`
— drives the REAL `dispatch_menu_command("new-agent-tile")` (the exact command string
the `.`→`n`→`a` entry carries) in both contexts: in a real workspace the active
workspace's tile count grows by one and the focused tile is an `App::Agent`; in an
ephemeral workspace the tile count stays **1**, the workspace count is unchanged, the
workspace is still ephemeral, the focused tile is an **unbound** `App::Agent`, and the
previously-bound session is still in the store (`sessions.contains`) bound by no tile.
*Negative control (observed RED):* disable the `active_is_ephemeral()` arm → "must NOT
split" fires with `left: 2, right: 1`.

**Deviation from plan.** The real-workspace arm asserts the new tile is an
`App::Agent`, not specifically an *unbound* one. That branch's picker-vs-bound
outcome is decided inside the pre-existing `open_agent_inner`: with a session server
(production) it opens the picker; the harness has no daemon, so it takes the legacy
direct-spawn branch and binds. The **placement** — the thing this invariant changes —
is what the guard pins. The ephemeral branch is unconditional (`AgentTile::new()` is
always `Selecting`), so its unbound-picker assert is exact.

### UXI-Workspace-9 — Closing the session in a bare agent view dismisses the view

**Statement.** When the confirmed close (`UXI-AgentTile-22`'s typed `yes`) kills the
session shown by an **ephemeral virtual workspace**, the workspace itself is torn
down and the user lands back on the workspace they jumped from — no leftover
selector tile to dismiss with a second `.` `x`.

1. **Only in the ephemeral case.** In a real workspace, closing a session is
   unchanged: the tile stays an agent tile and becomes a live unbound **selector**
   (an agent tile never vanishes and never becomes a buffer).
2. **Land in the CLOSED SESSION'S project — never a foreign one.** The ephemeral
   workspace is pinned to the session's project (`UXI-Project-6`), and that is the
   project the user stays in. In preference order:
   1. **The origin**, when it belongs to that project. The frame records the
      workspace the ephemeral was opened FROM (`Frame::ephemeral_origin`, the origin
      workspace's focused `WindowId` — a stable key, unlike an index).
   2. Else **the project's first workspace** — the origin is a foreign project (you
      jumped in from project A to a project-B session), so going "back" would drop
      you somewhere you weren't working.
   3. Else — the project has **no workspace at all** — **another agent session in
      that project**, opened in its own bare agent view. Projected from the same
      list the jump panel shows, in the order the user sees it; the session just
      closed is excluded by both its local `SessionId` and its server sid (the
      roster still lists that sid until the async `SessionDeleted` arrives).
   4. Else the origin whatever its project, then the last remaining workspace. There
      is always ≥1 real workspace, so the landing is total.

   A **workspace is never auto-bound to some other session**: clause 3 only fires
   when the project has no workspace to land on, which is exactly when a session is
   the only in-project destination that exists.
3. **Replacing one ephemeral with another keeps the ORIGINAL origin** — the
   ephemeral is the thing you return *from*, never the thing you return *to*.
4. Switch-away teardown (ADR-0021, `set_active_workspace`) is unchanged; it just
   clears the recorded origin.

**Applies to.** `workspace.rs`: `Frame::ephemeral_origin`,
`Frame::dismiss_ephemeral_workspace`, `Frame::same_project_landing`,
`Frame::active_ephemeral_project`, `Frame::workspace_index_of_window`, the origin
capture in `open_ephemeral_workspace_in`, the clear in `set_active_workspace`.
`agent_ui.rs`: the tail of `close_active_agent_session` +
`same_project_session_target`.

**Why.** The bare agent view exists *to show that one session*. Once the session is
closed the view has nothing to show, so leaving a selector behind makes the user
close the same thing twice (`<space> x … yes`, then `.` `x`) — the friction this
invariant removes. Clause 2's project rule came from the first version shipping a
plain return-to-origin: jumping to a **free** session in another project and closing
it silently dropped the user into the project they'd jumped *from*. Closing a session
is not a request to change projects.

**Status.** `implemented` (headless, end-to-end on the real close path).

**Enforcement.** `verify_harness.rs::closing_the_session_in_a_bare_agent_view_dismisses_it`
— drives the REAL `dispatch_menu_command("claude-close")` then a REAL `yes` submit
through `submit_agent` → `consume_close_confirm` → `close_active_agent_session`, on a
session jumped to from workspace **0** of two. Asserts the workspace count is back to
pre-jump, the active workspace is no longer ephemeral, and it is workspace 0 with the
very `WindowId` we left. **The origin assert is non-vacuous by construction:** the
origin (0) is deliberately NOT the last workspace, so a "land on the last one"
fallback would give 1. Clause 1 (a real workspace keeps its tile + workspace) is
asserted at the tail of
`arming_close_drops_into_insert_unless_a_draft_is_at_risk` part A, which closes a
session on a properly-bound tile in a real workspace.
*Negative controls (both observed RED):* skip the `dismiss_ephemeral_workspace` call
→ "the ephemeral view is gone" fires (`3` vs `2`); force the origin lookup to `None`
→ "we land back on the ORIGIN workspace" fires (`1` vs `0`).

Clause 2's project rule is pinned separately by
`verify_harness.rs::closing_a_free_session_lands_in_the_same_project` — three
projects (A has the workspace the user sits on, B has a workspace, C has **none**),
same REAL close path. Arm 1: a project-B session jumped to from A lands on **B's**
workspace, not the A origin. Arm 2: closing one of project C's two sessions lands on
**the other C session** in its own bare agent view, not a foreign workspace.
*Negative controls (both observed RED):* drop the `same_project` preference in
`dismiss_ephemeral_workspace` → arm 1 lands on workspace `0` (project A) — the
reported bug, reproduced exactly; drop the `session_fallback` in
`close_active_agent_session` → arm 2 ends with no bound session (`None` vs the
sibling).

**Deviation from plan.** The clause-1 contrast lives in the `UXI-AgentTile-23` test
rather than this one: binding a second free session into a *real* workspace mid-test
needs a bound-tile boot (`boot_worksheet_channel`), which that test already has —
`focus_existing_session` on a free session opens another ephemeral view instead.

### UXI-Workspace-10 — The default desktop density fits a 4×4 viewport

**Statement.** A fresh desktop divides the visible canvas into four columns by
four rows, with the jump panel visible. The existing desktop-grid command remains
authoritative for explicit user choices. Legacy preferences carrying the former
built-in `2×2` default migrate once to `4×4`; a `2×2` value explicitly saved
after the migration remains `2×2`.

**Applies to.** `main.rs` (constructor defaults and versioned preference load)
and `persist.rs` (`desktop_grid_defaults_version`).

**Status.** `implemented`; exact fit still respects the existing 160×120px
minimum tile size.
