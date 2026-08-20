# Component: Workspace

**Status:** living
**Component token:** `Workspace` (⇒ `UXI-Workspace-N`)

## Description

A **Workspace** (`Workspace<App>`, formerly `Tab<App>` — the `tab` vocabulary was
eradicated in ADR-0028 §5 / T007) is a plane of **attached** tiles. Visible attached
tiles participate in its n-ary **layout tree**; hidden attached tiles retain the
workspace as owner without participating in the layout. It also owns a focused-tile
pointer and a required-private **`project: ProjectId`**
foreign key into the `Projects` store. Interior nodes of the tree are **Splits**
(`Layout::Split`) — a direction (`H` = stacked, `V` = side-by-side) plus weighted
children — and each leaf is a **Tile** (`Window<App>`, a stable `WindowId` holding
exactly one `App`: `Buffer`, `Agent`, or `Linear`). The code-level struct for a
tile is still called `Window`, but in discussion we say **tile**.

A workspace **belongs to a project** and does NOT carry its own cwd: its working
directory is the project's, resolved at the point of use (`projects.cwd_of(
workspace.project())`) and **never cached** (`UXI-Project-2`, ADR-0028 §3). A new
workspace inherits the active one's project.

The container that owns the list of workspaces and the **Detached tile**
collection (one per OS-level **Frame**) is
**`Frame<App>`** (formerly the code's `Workspace<App>` — renamed in T007 so the
type name matches the user-facing "workspace" = the plane). The frame carries the
workspace strip (per-workspace label, active marker, click-to-select, rename,
next/prev, new/close, `ctrl-<n>` jump by number) and the buffer pool; the tag bar
drives tile tags. Durable workspaces are numbered `1..N` in the jump panel
(globally, across all projects), and those numbers are the jump targets.

The full hierarchy is **`Frame` → `Project` → (`Workspace` → attached tiles) +
Detached tiles**. An attached tile is either visible or hidden. Every stable
tile has exactly one owner; solo presentation is not a third owner (ADR-0034,
`UXI-Workspace-24`).

## References

- `docs/components/project.md` (`UXI-Project-1..8`) — the `Project` a workspace
  belongs to; the `ProjectId` FK + derived-cwd contract (`UXI-Project-2`).
- ADR-0028 — Projects as the top-level primitive + the `Tab`→`Workspace`,
  container→`Frame` rename (§5) this Description now reflects.
- ADR-0033 — optional workspace ownership, tile-local tags, direct unbound
  focus, and removal of ephemeral virtual workspaces.
- ADR-0034 — attachment independent of visibility; Attached/Detached
  terminology, hidden workspace ownership, typed solo presentation, and Close
  as an independent operation.
- `docs/specs/spec-workspaces-and-splits.md` — the n-ary split/layout tree +
  persistence (retroactive spec of shipped behavior; workspace/frame vocabulary).
- `docs/specs/spec-layout-patterns.md` — tile tags + automatic layout modes.
- `docs/specs/spec-desktop-mode.md` — desktop tile sizing / primary layout; the
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

**Statement.** The jump panel numbers durable workspaces `1..N` (the
`idx + 1` badge), and `ctrl-1`…`ctrl-9` / `ctrl-0` (the 10th) jump straight to
that workspace. The displayed digit and the keystroke target always agree because
direct-unbound view has no workspace number — `goto_workspace_number(n)`
selects the n-th workspace and clears direct-unbound focus. A digit past the
last workspace is a no-op.

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
and `workspace_number_ignores_direct_unbound_focus` (direct focus does not
enter workspace numbering).

---

_The invariants below (`UXI-Workspace-2..7`) define the **infinite-plane** model
(`docs/specs/spec-infinite-plane-workspace.md`). All are `implemented` (headless-
guarded; the `Ctrl-W 0/-/=` chord firing is the one `NEEDS-RUNTIME` gap). They built
on the desktop-mode geometry engine and **supersede** the split-tree / layout-mode
behavior in this component's Description above — a workspace is now one infinite
plane; the `LayoutMode` cycle, primary-stack, split-resize, and equalize surface are
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
load-bearing for new-tile creation on the plane; only the *mode / primary-stack /
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
`pixels = pan · slot_pitch(zoom)` is realized as `pan ⊗ ((160, 160) ⊗
detail_scale(zoom) + gutter)` — pitch is per-axis in the implementation even
though the Full cells are square and fixed. `zoom_in`/`zoom_out` take an explicit
`anchor: Slot` (focused tile or viewport center, resolved by the caller).

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
the fixed 160×160px Full cell (`desktop_tile_px`); `zoom_in`/`zoom_out`;
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

**Deviation from plan.** `slot_pitch(detail)->f32` was realized as
`detail_scale(Detail)->f32` (Full 1.0 / Card 0.5 / Minimap 0.2) multiplied
against the fixed per-axis `desktop_tile_px` Full cell and gutter.
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
above and right before left. The entire candidate rectangle must be free. So a
new tile opened beside a lone tile lands at `(row, col + existing_span.cols)` —
**same height, directly to the right** — never diagonally (bug-0012). With the
default 4×4 span, that is four columns to the right.
Dragging a tile moves it to the dropped slot iff its whole rectangle lands on
free slots; an overlapping drop is **rejected** (returns home). There is **no**
row-major insert-and-shift ripple. Closing a tile leaves a gap; neighbors never
move. `focus_next`/`focus_prev` traverse `slots` in signed row-major **reading
order** (top→bottom, left→right); spatial directional focus is unchanged.

**Applies to.** `workspace.rs`: `seed_span_near` / `seed_slot_near` / `seed_slot`
(rectangle-aware ring-spiral, replacing the shelf `first_free_slot`) and
`reconcile_near_with_span` (resolves the spiral center and assigns the configured
span), called from the per-frame plane upkeep in `chrome.rs`; free-placement drop (desktop-mode Behavior 4 gesture, ripple
removed); edge-resize `clamp_resize` unchanged in spirit (Block rule);
`sequence_neighbor` retained for traversal only (decoupled from placement).

**Why.** The row-major shelf assumed a top-left origin and a wrap width — neither
survives an all-directions plane; free placement + origin seeding is the natural
model for a boundless canvas.

**Status.** `implemented`.

**Enforcement.** `workspace.rs` desktop_tests: `seed_slot_spiral_deterministic`
(origin-first, occupied origin ⇒ next lands same-row-right, deterministic),
`seed_slot_near_prefers_same_row_right` (spiral centered on a non-origin tile)
`reconcile_near_places_four_by_four_tiles_side_by_side` (4×4 rectangles seed
four columns apart without overlap), and
`free_drop_rejects_overlap_without_moving_neighbors` (overlapping drop leaves
every tile's slot unchanged — no ripple). Negative-control-verified RED. On the
REAL path: `verify_harness.rs`
`new_tile_lands_same_row_right_of_the_only_tile` — the sole tile parked off-origin,
the user's `Ctrl-W v` handler, slot and 4×4 span assigned by the real `chrome.rs`
reconcile; it lands at `(1,3)` beside the existing `(1,-1)` 4×4 tile.
Existing signed-adapted desktop_tests cover the type-only reuses.

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

### UXI-Workspace-8 — SUPERSEDED: contextual new Agent in an ephemeral view

**Superseded by `UXI-Workspace-16` / ADR-0033.** Retained as implementation
history until the ephemeral-workspace code and its guards are removed.

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

### UXI-Workspace-9 — SUPERSEDED: closing a session dismisses its ephemeral view

**Superseded by `UXI-Workspace-16` / ADR-0033.** A directly focused unbound
tile is no longer an ephemeral workspace, and session lifetime is independent
of tile ownership.

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

### UXI-Workspace-10 — New desktop tiles default to 4×4 fixed cells

**Statement.** A fresh tile occupies four columns by four rows, making a new
whole-window tile useful immediately while retaining the snap-to-grid quality.
The existing desktop-grid command now chooses the span assigned to future tiles;
existing tiles keep their saved spans. Legacy preferences carrying the original
built-in `2×2` default or the subsequently shipped `3×3` value migrate once to
`4×4`. Versioned choices made after those migrations remain unchanged; asymmetric
custom spans are never mistaken for a shipped square default.

**Applies to.** `main.rs` (constructor defaults and versioned preference load)
and `persist.rs` (`desktop_grid_defaults_version`).

**Status.** `implemented`. An absent persisted span still reads as 1×1 for
backward compatibility; the render-time reconcile explicitly assigns the
configured 4×4 default only to genuinely new, slotless leaves.

**Enforcement.** `tests.rs`:
`default_tile_span_migrations_reach_four_by_four_without_overriding_later_choices`
pins the v2 `3×3` → v3 `4×4` migration plus explicit/custom-choice preservation;
`fixed_cells_and_four_by_four_tiles_match_the_retina_reference` pins the exact
4×4 footprint. `workspace.rs`:
`reconcile_near_places_four_by_four_tiles_side_by_side` pins rectangle-aware
placement without overlap.

### UXI-Workspace-11 — App resize changes the viewport, not cell size

**Statement.** Every Full-detail desktop cell is fixed at 160×160 logical pixels
with a 12px gutter. Resizing the app window, toggling the jump panel, or opening a
rail changes only the viewport: a smaller canvas covers fewer cells and a larger
canvas covers more. It never squeezes or stretches the cells or the tiles anchored
to them. Changing the desktop-grid command changes only the span of future tiles.

**Applies to.** `chrome.rs` (`DESKTOP_CELL_W`, `DESKTOP_CELL_H`,
`DESKTOP_GUTTER`, and `desktop_tile_px`).

**Status.** `implemented`. The July 30 reference screenshot is 1350×1344 physical
pixels at 2× Retina (675×672 logical). Four 160px cells plus three 12px internal
gutters produce a 676×676 logical tile (1352×1352 physical), matching the reference
to its border/crop tolerance.

**Enforcement.** `verify_harness.rs`:
`workspace_cells_keep_fixed_size_when_the_window_resizes` drives the production
size path on a real view, shrinks a 1200×900 canvas to 600×400, changes the default
new-tile span from 4×4 to 3×3, and asserts the 160×160 cell remains unchanged.

### UXI-Workspace-12 — The outer window remembers its size

**Statement.** Resizing Yalda updates its persisted outer-window width and height.
The next launch opens at that saved size. Window position and maximized/fullscreen
state are not persisted; the operating system remains responsible for placing
the window on an available display.

**Applies to.** `main.rs` (`observe_window_size`, `restore_window_size`, and
startup `WindowOptions`) and `persist.rs` (`window_width_px`,
`window_height_px`).

**Status.** `implemented`. A legacy, missing, partial, or invalid saved pair
falls back atomically to the 900×700 default.

**Enforcement.** `tests.rs` pins preference compatibility and saved-size
validation. `verify_harness.rs`:
`window_resize_observer_persists_the_size_for_next_launch` drives GPUI's real
window-resize observer, reads the written preferences, and feeds them through
the production startup-size helper.

### UXI-Workspace-13 — SUPERSEDED: closing a workspace removes its tiles

**Superseded by `UXI-Workspace-16` / ADR-0033.** The new contract moves every
tile owned by the closing workspace into Unbound instead of deleting it. The
historical behavior and guards remain below until implementation is replaced.

**Statement.** The workspace `.` menu exposes uppercase `X` as **close
workspace**; lowercase `x` remains **close tile**. Closing the active workspace
removes that workspace and every tile it owns only when another workspace
remains. An Agent tile is only a reference to its session, so removing the tile
does not stop, close, archive, or delete the session: the session remains alive
in `AgentSessions` and, when no other durable workspace tile references it,
becomes free and available for placement again. Closing the sole remaining
workspace is a no-op. No workspace-close entry point quits the app.

**Applies to.** `main.rs`: `gpui_menu` (`X` → `close-workspace`),
`dispatch_menu_command("close-workspace")`, and the global `CloseWorkspace`
action (`Cmd-Shift-W`). `workspace.rs`: `Frame::close_workspace`, whose tile
drop only releases session references because `AgentSessions` owns session
identity and runtime state. Existing lowercase `x` / `close-window` behavior is
unchanged.

**Why.** A workspace is a layout of views onto ongoing work. Dismissing that
layout must not terminate the work behind its Agent tiles, and a layout command
must never be an implicit application-quit command.

**Status.** `implemented` (headless).

**Enforcement.** `tests.rs::workspace_menu_uppercase_x_selects_close_workspace`
pins the literal menu chord and proves lowercase `x` still resolves to
`close-window`. `verify_harness.rs::closing_workspace_frees_sessions_and_never_quits`
drives the real `dispatch_menu_command("close-workspace")` path over an active
workspace containing a bound server-managed Agent session: the workspace and
tile disappear, the session remains in `AgentSessions`, its durable binding is
gone, and the selector projects it as free. The same test dispatches again at
the sole-workspace floor and drives the real `Cmd-Shift-W` keymap/action/handler
path; both retain the workspace and session. Negative control: removing the
sole-workspace early return sent the real notify/render path into `chrome.rs`
with zero workspaces and failed RED. `cargo mutants` caught all three generated
mutations of `close_active_workspace`, including `<=` → `>`.

**Deviation from plan.** GPUI 0.2.2's headless `Platform::quit()` is itself a
no-op, so restoring the former `cx.quit()` produced a false-green direct negative
control. The implementation was tightened instead: `close_active_workspace`
takes no GPUI `Context`, making app quit structurally unavailable to both the
menu and action routes; handlers only notify when it reports that a workspace
was removed. The floor mutation above guards the remaining state predicate.

### UXI-Workspace-14 — A workspace switches between the plane and a columns arrangement

> **Amended by `UXI-Workspace-26`.** The two-value `Plane ⇄ Columns` toggle is
> superseded by three UI-selectable arrangements (Columns / Tiling / Monocle);
> `Plane` is retired from the UI. `Ctrl-W a` now *cycles* the three modes and the
> `.` menu's "toggle plane / columns" entry is replaced by the `.` → layout
> submenu. The Columns arrangement + `render_columns` + persistence described
> here still hold; read `UXI-Workspace-26` for the current mode set.

**Statement.** A workspace carries a **`view: WorkspaceView`** — either `Plane`
(the infinite-plane arrangement every other `UXI-Workspace-*` describes) or
`Columns` (the default). One command toggles between them: **`Ctrl-W a`** (and the `.`
workspace menu's "toggle plane / columns"). In `Columns` every tile of the
workspace renders as an **equal-width, full-height column**, side by side, in
signed plane **reading order** (top→bottom, left→right — the order the plane
numbers tiles and `focus_next` traverses). A column's width is `flex_1`, so the
app-window width divides evenly across the tiles; there is no camera, pan, zoom,
drag, or edge-resize in columns.

1. **The toggle is a pure VIEW flip** (like the camera, `UXI-Workspace-3`): it
   never moves, re-seeds, or removes a tile. The plane slots/spans are left
   untouched, so toggling back to `Plane` restores the exact prior arrangement.
2. **Every tile appears** — including one the plane would cull off-viewport. The
   focused tile carries the focus handle so the keyboard survives, and
   click-to-focus (`UXI-Workspace-9`) still applies to each column's body.
3. **It persists** (`view`, absent/unknown ⇒ `Columns`), so either explicit
   arrangement reopens unchanged and a fresh workspace starts in columns.

**Applies to.** `workspace.rs`: `WorkspaceView` enum (hand-rolled serde,
unknown→`Columns`, the current default), the `Workspace.view` field +
`Workspace::toggle_view`. `chrome.rs`: `render_focused_window` branches on
`view`; `render_columns` (flex-row of equal-width columns); the shared
`render_tile_content` helper (extracted so plane + columns render identical
per-kind bodies); the `ToggleWorkspaceColumns` action wired on BOTH the plane
canvas root and the columns container. `main.rs`: the `ToggleWorkspaceColumns`
action + `toggle_workspace_columns` handler + `workspace-toggle-columns` menu
command + the `.` menu entry. `keymap_registry.rs`: `ctrl-w a`. `persist.rs`:
`PersistedWorkspace.view` (`#[serde(default)]`) + save; `main.rs` restore
(`wsp.view = pws.view`).

**Why.** The plane is a boundless spatial canvas; sometimes the user wants every
tile visible at once in a simple, dense side-by-side layout without panning.
Columns gives that as a lossless alternate view over the same tiles.

**Status.** `implemented` (headless).

**Enforcement.** `verify_harness.rs::columns_view_arranges_tiles_side_by_side`
drives the REAL `toggle_workspace_columns` handler (the keybinding/menu path) on
a two-tile fixture where B is parked off-viewport: on the plane B is culled
(proven first, for non-vacuity); after the toggle BOTH tiles paint as columns —
B to the right of A, equal width, full height — via the layout probe
(`columns-tile-{id}` + `plane-tile-content-{id}`). Negative-control-verified RED
(drop the `Columns` arm → B still culled, `columns-tile-*` never paint).
`workspace.rs::new_workspace_defaults_to_columns` pins the production
`Frame::with_initial` path.
`tests.rs::workspace_view_round_trips_and_unknown_defaults_columns` pins both
explicit serde round-trips + absent/unknown → `Columns`.

**Deviation from plan.** The original columns-view implementation had none. The
2026-08-16 default-columns follow-up made `boot_desktop_two_tiles` set `Plane`
explicitly because its camera/placement guards had implicitly inherited the old
product default. The toggle handler is the exact method both the `Ctrl-W a`
binding and the `workspace-toggle-columns` menu command dispatch, so the guard
drives it directly rather than a proxy. The real macOS `Ctrl-W a` chord *firing*
is the usual `simulate_keystrokes`-vs-OS gap (CLAUDE.md rule 4), but the binding
is a `Ctrl-W`+plain-letter sequence (reliable, like `Ctrl-W 0`).

### UXI-Workspace-15 — Placement commands rearrange stable tiles through stable footprints

**Statement.** The `Ctrl-W` workspace family includes dwm-style placement
commands which rearrange the active workspace without changing what any tile is:

- `Ctrl-W H/J/K/L` swaps the focused tile with its nearest spatial neighbor to
  the left/down/up/right respectively. In `Plane`, the target is exactly the
  tile that the corresponding lower-case focus command (`Ctrl-W h/j/k/l`) would
  select; the `Columns` specialization is defined below.
- `Ctrl-W Return` promotes the focused tile by swapping it with the first tile
  in signed plane reading order. In `Columns`, that is the leftmost column.
- `Ctrl-W x` opens a keyboard-operated **Swap tile** picker containing every
  other tile in the active workspace; selecting a row performs the swap and
  `Esc` cancels without mutation.
- `Ctrl-W r` / `Ctrl-W R` rotates all tiles forward / backward through signed
  plane reading order.
- `Ctrl-W u` undoes the most recent successful placement command. Undo is a
  bounded, workspace-local history of complete placement snapshots; a new
  successful placement after undo replaces the popped history naturally. It
  does not undo tile creation, closing, drag, edge-resize, camera changes, or
  workspace switching.

A tile's **placement footprint** is its `(anchor Slot, Span)`. Swapping exchanges
the two complete footprints, not the Apps inside the tiles and not just their
anchors. Rotation assigns each complete footprint to the next/previous stable
`WindowId`. This is dwm slot semantics: moving into a differently-sized region
adopts that region's size. Complete-footprint exchange cannot introduce overlap,
even when the two spans differ.

The focused `WindowId` remains focused and travels to its new footprint. Its App,
agent-session binding, title, marks, and other tile-local state travel with that
stable id. The camera is unchanged; the existing minimum-reveal behavior may pan
only as needed to show the now-moved focused tile. Every successful command is
persisted immediately. A one-tile workspace, promotion when already first,
rotation with fewer than two tiles, a direction with no neighbor, or picker
cancel is a strict no-op and does not create an undo entry.

`Columns` is a view over the same persisted plane placements
(`UXI-Workspace-14`), so the commands mutate those placements in both views.
`H`/`L` exchange adjacent visible columns. Because full-height columns have no
visible tile above or below, `J`/`K` are no-ops in `Columns`; they must not use a
hidden plane neighbor that contradicts the visible arrangement. Promotion,
picker swap, rotation, and undo are lossless across a Plane⇄Columns toggle.

**Applies to.** `workspace.rs`: complete placement snapshot/history and
`swap_placements`, `promote_focused`, `rotate_placements`, `undo_arrangement`;
view-aware neighbor resolution on `Workspace`. `main.rs`: actions, handlers,
`SwapTilePicker` overlay, persistence + notify. `keymap_registry.rs`: the bindings
above. `chrome.rs`: action wiring on both Plane and Columns roots.

**Why.** Lower-case `Ctrl-W` motion already makes tile focus keyboard-native.
The matching upper-case grammar makes layout manipulation equally direct, while
complete footprints reconcile dwm's ordered-slot behavior with Yaldabaoth's
free-sized infinite plane. The picker covers distant/off-screen targets without
overloading spatial navigation.

**Status.** `implemented (headless)`.

**Enforcement.** Pure `workspace.rs` tests pin footprint exchange, spatial and
reading-order target selection, rotation direction, undo, no-overlap, and no-op
history behavior. The real-path guards
`ctrl_w_uppercase_swaps_footprints_in_plane_and_columns`,
`ctrl_w_promote_and_rotate_three_tiles`, and
`ctrl_w_x_picker_cancels_and_swaps_selected_tile` drive the production keymap →
action → handler paths and assert persistable placement plus stable focus. They
were observed RED by temporarily disabling directional swap, undo, promotion,
each rotation direction, and picker commit; every mutation failed at its
command-specific assertion before the production implementation was restored.

### UXI-Workspace-16 — Every tile is bound to one workspace or unbound

> **Superseded by `UXI-Workspace-24` / ADR-0034.** Exclusive ownership remains,
> but Attached-visible, Attached-hidden, and Detached replace this two-state
> Bound/Unbound model.

**Statement.** A stable tile has exactly one optional workspace owner:

1. A **bound** tile is a leaf in exactly one workspace layout.
2. An **unbound** tile is owned by the frame's Unbound collection and is in no
   workspace layout.
3. Binding and unbinding move the same tile. `WindowId`, App state, project,
   and tile tags are unchanged.
4. Directly viewing an unbound tile only sets the frame's direct-focus pointer;
   it does not bind the tile or create a workspace. Selecting any workspace
   clears direct focus.
5. Closing a non-sole workspace moves all its tiles to Unbound. It never kills
   Agent sessions or quits the app.
6. A tile may bind only to a workspace in the same project.

There are no ephemeral workspaces in this model. An Agent tile whose picker has
no selected session is **empty**, not “unbound”; workspace membership and
session binding are independent axes.

**Applies to.** `workspace.rs`: `Window`, `Frame`, bind/unbind/direct-focus
and close-workspace operations; `main.rs` / `chrome.rs`: content selection,
rendering, and commands; `persist.rs`: both ownership domains.

**Why.** A workspace is a placement folder, not the lifetime boundary for the
stateful tile. Optional ownership provides the minimize-like state without fake
workspaces or duplicate viewports.

**Status.** `implemented (headless)`.

**Enforcement.** `workspace.rs` ownership tests cover stable-id/state/tag
round trips, uniqueness, same-project binding, direct focus, and workspace
close. `jump_palette_opens_unbound_tile_then_binds_same_identity` drives the
real Cmd-P and `Ctrl-W b` paths. The complete GUI and library suites pass, and
targeted ownership mutants are caught (Cog graph `9k2`).

### UXI-Workspace-17 — Send a tile without following or send and follow

> **Terminology amended by `UXI-Workspace-24` / ADR-0034.** The same destination
> picker attaches a Detached tile or moves an already Attached tile. Hidden
> attachment is not a separate destination.

**Statement.** A bound tile can move to another workspace in the same project
through the existing workspace picker:

- `Ctrl-W m` opens **SEND TILE TO WORKSPACE** and moves the complete stable tile
  while keeping the source workspace active.
- `Ctrl-W M` opens **SEND TILE AND FOLLOW** and activates the destination after
  moving the tile.
- The shell menu exposes both operations and retains **also show document** as a
  menu-only operation.

The picker contains only same-project workspaces plus **+ new workspace**. A
new destination belongs to the tile's project. Selecting the source is a no-op.
If sending the source's last tile removes that workspace, both variants must
land on the destination because there is no source left to remain on. Every
successful move preserves `WindowId`, App state, tags, marks, and Agent session
selection; the destination records the moved tile as its focused tile.
Directly focused Unbound tiles continue to use `Ctrl-W b`; send is a
workspace-to-workspace operation.

**Applies to.** `workspace.rs`: cross-workspace relocation and project checks;
`main.rs`: `WorkspacePicker`, send handlers, and shell menu;
`keymap_registry.rs`: `Ctrl-W m/M`.

**Why.** Spatial organization needs both background filing and the familiar
i3 “move container and follow” workflow without cloning or recreating state.

**Status.** `implemented (headless)`.

**Enforcement.** `workspace.rs` model tests prove stable identity/state/tag
preservation, both follow policies, same-project rejection, and last-tile
source removal in both index orders. `verify_harness.rs`:
`ctrl_w_send_and_send_follow_use_the_same_project_picker` drives the production
keymap → action → picker → Enter path for both commands and proves the picker
excludes another project's workspace. Negative control: disabling follow on
the uppercase handler made the guard RED at the active-workspace assertion.

### UXI-Workspace-18 — Scratchpad is an MRU subset of Unbound

> **Superseded by `UXI-Workspace-24` / ADR-0034.** Hide keeps a tile attached;
> the Detached scratchpad/MRU model and Stash/Summon operations are removed.

**Statement.** Scratchpad tiles are ordinary stable Unbound tiles with an
additional persisted MRU membership list:

- `Ctrl-W d` stashes the focused bound tile: it is moved to Unbound, added to
  the front of the scratchpad order, and direct focus is cleared so the user
  remains on the source workspace (or its surviving fallback).
- `Ctrl-W D` summons the newest scratchpad tile by directly focusing it without
  binding it. Repeating walks older scratchpad tiles; after the oldest it
  returns to the underlying workspace.

Stashing the sole tile in the sole workspace is a no-op because the durable
workspace floor still applies. Normal unbind/archive/workspace-close operations
do not implicitly add tiles to the scratchpad. Binding a scratchpad tile removes
its scratchpad membership. Restore accepts only live Unbound ids, deduplicates
them, and preserves their MRU order. A tile's existing title is its scratchpad
name; no second naming system is introduced.

`Ctrl-W s` remains the Vim-compatible horizontal split command, so scratchpad
uses `d/D` (“detach/drawer”) rather than replacing it.

**Applies to.** `workspace.rs`: scratchpad membership, stash, cycle, prune;
`persist.rs`: additive scratchpad id list; `main.rs` / `chrome.rs`: actions,
handlers, and shell menu; `keymap_registry.rs`: `Ctrl-W d/D`.

**Why.** Unbound already supplies the correct durable ownership state for an
i3-style scratchpad. A small MRU index adds fast hide/summon behavior without a
parallel tile store or special window type.

**Status.** `implemented (headless)`.

**Enforcement.** Model and snapshot tests cover MRU cycling, final hide, the
workspace floor, bind pruning, invalid-id pruning, and persistence round-trip.
`ctrl_w_scratchpad_and_workspace_back_and_forth_are_global` drives the real
`Ctrl-W d/D` routes, checks stable identity, walks the MRU past its oldest tile,
and returns to the workspace. Negative control: wrapping instead of hiding
after the oldest tile made the guard RED.

### UXI-Workspace-19 — Back-and-forth toggles stable workspace identity

**Statement.** `Ctrl-W Backspace` and the shell workspace menu's
**back and forth** command toggle between the current and previously activated
workspace. History stores the previous workspace's immutable `auto_name`, not
its mutable vector index, so workspace insertion/removal cannot redirect the
command. Every real workspace activation updates the history; selecting the
already-active workspace or merely leaving a direct Unbound view does not.

Each workspace already owns its focused `WindowId`, so arriving restores the
tile that was focused there. With no live previous workspace, the command is a
strict no-op. Closing the remembered workspace makes the next invocation a
no-op rather than selecting an unrelated index. History is runtime navigation
state and is not persisted across application restarts.

**Applies to.** `workspace.rs`: activation chokepoint and stable previous key;
`main.rs`: action/menu handler; `keymap_registry.rs`: `Ctrl-W Backspace`.

**Why.** Workspace switching often alternates between two contexts; preserving
each folder's tile focus makes that alternation a single reliable command.

**Status.** `implemented (headless)`.

**Enforcement.** Model tests cover stable-name toggling, per-workspace focus,
same-index behavior, and deletion of the remembered workspace.
`ctrl_w_scratchpad_and_workspace_back_and_forth_are_global` drives the global
`Ctrl-W Backspace` action from direct Unbound focus and asserts the remembered
workspace and its focused tile are restored. Negative control: making the
model command a no-op made the guard RED.

### UXI-Workspace-20 — Columns has a controllable primary area

> **Amended by `UXI-Workspace-26`.** The primary area moved from `Columns` to the
> new `Tiling` arrangement — `Columns` is now plain equal-width columns. The four
> `Ctrl-W f/F/n/N` primary mutators (and the layout submenu's grow/shrink/count
> entries) now act on `Tiling`; every non-Tiling view is a no-op. Everything else
> below (clamps, no plane-slot mutation, persistence) is unchanged.

**Statement.** Columns divides its reading-order tiles into two horizontal
areas. The first `primary_count` tiles share `primary_ratio` of the available
width equally; remaining tiles share the stack area equally. If every tile is
in the primary area, all tiles divide the full width evenly.

- `Ctrl-W f` / `Ctrl-W F` grow/shrink the primary ratio by `0.05`, clamped to
  `[0.20, 0.80]`.
- `Ctrl-W n` / `Ctrl-W N` increase/decrease primary count, clamped to
  `[1, tile_count]`.

The commands mutate only a Columns workspace; in Plane they are no-ops. They
never alter the plane slots, tile order, ownership, or focus. The existing
`primary_ratio` and `primary_count` snapshot fields become active again, so old
snapshots retain their values and missing fields still default to `0.60` / `1`.
The shell workspace menu exposes all four adjustments.

**Applies to.** `workspace.rs`: clamped primary mutators; `chrome.rs`:
`render_columns` width allocation; `main.rs`: handlers/menu;
`keymap_registry.rs`: `Ctrl-W f/F/n/N`; `persist.rs`: existing fields.

**Why.** Dwm's primary area keeps the primary work visibly dominant while
allowing several secondary tiles to remain available, without introducing a
nested split-tree layout model.

**Status.** `implemented (headless)`.

**Enforcement.** Pure tests prove ratio/count clamps and Plane no-ops.
`ctrl_w_primary_commands_change_columns_state_and_geometry` drives all four real
key paths and uses layout probes to prove ratio changes painted primary/stack
widths and all-primary count divides the full width evenly. Negative controls:
hard-coding either the rendered ratio or count made the geometry guard RED.

### UXI-Workspace-21 — Close Tile closes a directly focused Unbound tile

**Statement.** The shell's **Close Tile** command has the same object regardless
of placement: it removes the focused stable tile when that tile is bound or
Unbound. For a directly focused Unbound tile, closing removes it from Unbound,
removes any scratchpad reference to its id, clears direct focus, and reveals the
still-active workspace. This applies to every App state, including a Buffer file
picker and an Agent tile with no selected session.

Closing an Unbound tile does not close a workspace, kill the app, or reinterpret
an empty Agent picker as a session-lifecycle command. The sole-workspace floor
continues to apply only when closing the last bound leaf.

**Applies to.** `workspace.rs::Frame::close_focused`; `main.rs::close_window`
and the `close-window` menu dispatcher.

**Why.** Unbound is an ownership domain, not a special preview. Once the jump
panel directly focuses one of its tiles, shell verbs must act on that tile just
as they do on a workspace leaf.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::close_tile_removes_unbound_buffer_and_agent_picker`
drives the real `close-window` menu dispatcher against a directly focused
Buffer picker and empty Agent picker, and proves both stable ids leave Unbound
while the workspace floor survives.

### UXI-Workspace-22 — Ctrl-W is owned once by the shell, never by an App

**Statement.** The complete `Ctrl-W` command family is registered on one common
shell ancestor that wraps every focused tile surface. App renderers own only
App-specific actions; they do not opt into, duplicate, intercept, or omit shell
workspace actions. Consequently every `Ctrl-W` binding has the same dispatch
availability for every bound and directly focused Unbound App state, including
Buffer Picking/Viewing/Editing, Agent picker/session/unavailable, Linear, Cog,
and Keymap.

The central handler list and the `GLOBAL_BINDINGS` entries beginning with
`ctrl-w` have one declarative source/coverage contract: adding a binding without
central shell wiring is a test failure. Overlay focus remains an intentional
boundary; an active modal overlay owns its own keyboard surface and is not a
tile App silently consuming the prefix.

**Applies to.** `main.rs`: the central Ctrl-W action-wiring extension and
coverage vocabulary; `chrome.rs::render_focused_window`: the sole tile-shell
wiring point; `screens.rs`: App-local action wiring only;
`keymap_registry.rs::GLOBAL_BINDINGS`.

**Why.** GPUI resolves a binding to an action before an App's key listener, but
the action is silently dropped when no listener exists in the focused element's
ancestry. Duplicating shell listeners across each screen made every new picker
or App mode a regression opportunity and made the failure look like tile key
capture.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::ctrl_w_shell_commands_reach_every_tile_app`
uses the production keymap and real `Ctrl-W` keystrokes across the App-state
matrix. A registry coverage guard compares every configured `ctrl-w` action to
the central declarative handler vocabulary.

### UXI-Workspace-23 — Closing a bound Agent tile stashes it

> **Superseded by `UXI-Workspace-24` / ADR-0034.** Close is independent from
> Hide and Detach; Scratchpad is removed.

**Statement.** **Close Tile** on a bound Agent tile is a placement transition,
not destruction. It performs the same ownership move as **Stash**: the complete
stable tile moves from its workspace to Unbound, retains its `WindowId`, App
state, selected session, tags, and project, and enters the scratchpad MRU. The
session remains alive and the tile appears in the jump panel and Cmd-P Unbound
projection on the same state change.

This is intentionally asymmetric with Close Tile on an already-Unbound empty
Agent picker, which removes that empty tile under `UXI-Workspace-21`. Session
termination remains an explicit Agent lifecycle command and is never implied by
shell placement commands.

**Applies to.** The shared Close Tile command handler and the typed
bound-to-Unbound transition in `workspace.rs`.

**Why.** An Agent tile is the durable shell for a live conversation. Removing
its workspace placement must never make the conversation disappear from the
navigation model.

**Status.** `implemented`.

**Enforcement.** `close_bound_agent_tile_stashes_same_tile_and_session` drives
the production `close-window` menu command and asserts stable identity, session
liveness, Unbound projection, and scratchpad membership.
`close_sole_bound_agent_stashes_and_seeds_workspace_floor` covers the
one-workspace replacement branch. `ownership_invariants_hold_across_placement_operation_sequence`
and `ownership_guard_rejects_each_illegal_domain_state` enforce exclusive
WindowId ownership, immutable stable identity and tile project membership, and
valid direct-focus / scratchpad indices after every placement transition and
before persistence. `Window` keeps both `id` and `project` private and exposes
read-only accessors; placement APIs move the complete value instead of allowing
callers to rewrite either identity field.

### UXI-Workspace-24 — Attachment, visibility, presentation, and close are independent

**Statement.** Every stable tile has exactly one of three placement states:

1. **Attached + visible** — owned by one workspace and present in its layout.
2. **Attached + hidden** — owned by one workspace and absent from its layout.
3. **Detached** — owned by the frame and associated with no workspace.

There is no Detached + hidden state. Attach accepts a Detached tile and makes it
visible in a same-project workspace. Detach accepts either attached state,
clears hidden state, and places the unchanged tile in Detached. Hide accepts
only Attached + visible and leaves the tile owned by that workspace. Unhide
accepts only Attached + hidden, restores it through the current layout manager,
selects its workspace, and focuses it. Close retires the tile shell and never
dispatches any of those four placement transitions.

Hiding records the tile's last plane footprint as a best-effort restoration
preference. Other visible tiles may invalidate it. Unhide restores the footprint
only if it is still valid and unoccupied; otherwise the current Plane/Columns
arrangement inserts the tile by the same neighbor-seeding rules as a new tile.

A workspace may have all of its tiles hidden. It remains durable and renders an
explicit all-tiles-hidden empty state without creating a replacement. A hidden
attached or Detached tile may be presented alone through a typed solo target;
leaving that presentation changes neither ownership nor visibility. A visible
attached tile cannot be a solo target.

**Applies to.** `workspace.rs`: workspace-owned hidden tiles, typed
membership/presentation enums, transition APIs, placement restoration, complete
ownership validator; `main.rs` / `chrome.rs`: independent action handlers,
all-hidden canvas, and solo presentation; `persist.rs`: visibility and migration.

**Why.** Workspace association, layout visibility, temporary navigation, and
tile lifetime are independent facts. Encoding them as separate legal states
prevents Hide from silently detaching, Close from silently hiding, and a solo
visit from silently changing ownership.

**Status.** `implemented (headless)`.

**Enforcement.** `workspace::tests::{hide_solo_visit_and_unhide_follow_preserve_identity_and_best_effort_placement,
unhide_uses_normal_placement_when_saved_footprint_was_taken,
all_hidden_workspace_is_valid_and_detaching_hidden_clears_hidden_state,
close_retires_tile_without_hiding_or_detaching_it,
send_tile_transition_covers_visible_hidden_and_detached_membership}` cover the
typed transition matrix and layout fallback. `ownership_invariants_hold_across_placement_operation_sequence`
and `ownership_guard_rejects_each_illegal_domain_state` guard the exclusive
ownership graph. Production-path guards
`ctrl_w_hide_unhide_and_workspace_back_and_forth_are_global`,
`hidden_tile_navigation_is_solo_until_explicit_unhide`, and
`close_bound_agent_tile_retires_tile_without_hiding_or_detaching` exercise the shared
shell actions, solo visit/Unhide-follow, and independent Close behavior.

### UXI-Workspace-25 — Destination pickers name places, not their contents

**Statement.** The **Send tile to workspace** picker is a compact destination
chooser. Every existing destination row uses only the workspace's durable
display label. It never substitutes the focused tile's kind, provider, file,
or title, so a workspace named **Research** remains **Research** even when its
only tile is a Claude Agent.

The overlay presents one clear action title, a short sentence describing the
follow policy, and a restrained list hierarchy. Selection uses both an accent
rail and background; the active source carries a quiet **Current** badge; and
**New workspace** is a visually separated creation action. Body labels use the
standard UI font and fixed chrome sizing rather than inheriting document zoom.
Keyboard hints remain secondary and the full existing `j`/`k`, arrows,
`g`/`G`, Enter/`l`, and Esc/`q` interaction contract is unchanged.

**Applies to.** `main.rs`: workspace-picker presentation model and overlay;
`yux/detail.rs`: reusable option-row chrome.

**Why.** A destination selector answers “where?” Workspace-strip content
summaries answer “what is open there?” Mixing them produces unstable labels
such as `Claude (Research)` and makes the same workspace look like a different
place as its contents change.

**Status.** `implemented (headless)`.

**Enforcement.** `send_picker_agent_destination_uses_workspace_name_without_provider_prefix`
drives the production `Ctrl-W m` picker path and asserts the rendered row's
production label projection. A painted geometry guard covers the compact card,
row hierarchy, separated creation action, and real click dispatch. Restoring
the Agent-derived `Claude (<workspace>)` projection makes the identity guard
fail.

### UXI-Workspace-26 — Three UI layout modes: Columns, Tiling, Monocle (Plane retired)

**Statement.** A workspace's **`view: WorkspaceView`** offers **three
UI-selectable arrangements** over the SAME tiles, all lossless pure-view choices
(no tile is moved, re-seeded, or closed):

- **`Columns`** (the default) — every tile is an **equal-width, full-height
  column**, side by side in signed reading order. No primary area.
- **`Tiling`** — dwm-style **primary/stack**: the first `primary_count` tiles fill
  `primary_ratio` of the width in a full-height column on the **left** (the primary
  area, its tiles stacked vertically); the remaining (non-primary) tiles are
  **stacked vertically** in a second full-height column on the **right**, NOT laid
  side by side. The `Ctrl-W f/F/n/N` primary mutators and the layout submenu's
  grow/shrink/count entries act on this mode only (`UXI-Workspace-20`, amended).
- **`Monocle`** — only the **focused** tile paints, filling the whole content
  region; the others stay materialized (never moved/closed) but are not painted.
  Switching focus swaps which tile shows.

The **`Plane`** arrangement (`UXI-Workspace-2` … the infinite signed-grid camera)
is **retired from the UI**: no menu selects it, `Ctrl-W a` never lands on it, and
a persisted `"plane"` loads as `Columns`. The `Plane` enum variant + `render_desktop`
path are kept so the arrangement can be reinstated later without a data migration.

Selection:
- **`.` → layout submenu** picks a mode directly: `c` columns, `t` tiling, `m`
  monocle (`layout-columns` / `layout-tiling` / `layout-monocle`). The submenu
  also carries the four primary-area adjustments.
- **`Ctrl-W a`** *cycles* Columns → Tiling → Monocle → Columns.
- The mode **persists** (`"columns" | "tiling" | "monocle"`; absent, `"plane"`,
  or any unknown value ⇒ `Columns`), so a fresh workspace starts in Columns.

**Applies to.** `workspace.rs`: `WorkspaceView` (four variants, `next()`,
`set_view`, hand-rolled serde: `plane`→`Columns`), primary mutators gated on
`Tiling`, `placement_target` (Columns/Tiling share left/right adjacency, Monocle
none). `chrome.rs`: `render_focused_window` dispatch; `render_columns(use_primary)`
(false = equal Columns, true = Tiling primary/stack); `render_monocle`. `main.rs`:
`layout-columns/tiling/monocle` dispatch → `set_active_workspace_view`,
`toggle_workspace_columns` (cycles), the `.` layout submenu. `keymap_registry.rs`:
`ctrl-w a` label. `persist.rs`: `PersistedWorkspace.view` round-trip.

**Why.** The infinite plane was powerful but demanded panning/zoom to see
everything. Most sessions want a simple dense arrangement — equal columns, a
dwm primary/stack, or one maximized tile — chosen by name. Three explicit modes
give that directly; retiring the plane (reversibly) removes the mode nobody
selected by default without discarding its code or anyone's saved slots.

**Status.** `implemented (headless)`.

**Enforcement.** `verify_harness.rs::columns_view_arranges_tiles_side_by_side`
(Columns = equal-width, both tiles paint side by side, negative-control RED),
`ctrl_w_primary_commands_change_columns_state_and_geometry` (Tiling primary/stack
geometry via the real `Ctrl-W f/F/n/N` paths),
`tiling_stacks_non_primary_tiles_vertically` (3 tiles: stack tiles share an x
column and descend in y, right of the primary; negative control flips the stack
pane to `flex_row` → RED),
`monocle_view_paints_only_the_focused_tile` (only the focused tile paints;
negative control routes Monocle to `render_columns` → RED),
`layout_mode_commands_set_the_active_arrangement` (the `.` `layout-*` dispatch
sets `view`). `tests.rs::workspace_view_round_trips_and_unknown_defaults_columns`
(serde round-trip for the three modes + `plane`/unknown → Columns);
`workspace.rs::tiling_primary_controls_clamp_and_other_views_unchanged` (primary
mutators act on Tiling only). `tests.rs::shell_layout_submenu_selects_modes`
(the `.` → layout submenu keys).

**Deviation from plan.** The `/new-ux` brief said "retire plane"; the code keeps
the `Plane` variant + `render_desktop` behind a UI cutoff (no menu / persisted
`plane`→`Columns`) rather than deleting the ~500-line plane render path, so the
arrangement is reversible without a migration. Hide/unhide of individual tiles is
unchanged and orthogonal to Monocle (Monocle is a pure paint filter, not a
membership change).
