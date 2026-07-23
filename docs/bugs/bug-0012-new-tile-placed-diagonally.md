# bug-0012: new-tile-placed-diagonally

**Status:** FIXED
**First seen:** 2026-07-21
**Component:** `docs/components/workspace.md` (UXI-Workspace-6)

## Symptom

In a workspace holding exactly ONE tile, opening a new tile (new buffer, split,
new agent tile) puts the new tile **diagonally offset** from the existing one —
"up and to the right" — instead of at the **same height, directly to the right**.

Expected: the new tile lands beside the tile the user was just on, same row, one
column right (when that slot is free).

## Context / root cause

Placement is `DesktopState::seed_slot` (`src/bin/yalda-gpui/workspace.rs:798`),
called from `DesktopState::reconcile` (`:824`), called every frame from the plane
render path (`chrome.rs:109`). New-tile creation itself (`split_focused` /
`insert_leaf_into_tab`) never picks a slot — the leaf is slotless and gets one
lazily on the next reconcile.

Two independent defects, both structural:

1. **Origin-seeded, not neighbor-seeded.** `seed_slot` always spirals out from
   `(0,0)`, ignoring where the existing/focused tile actually is. If the sole
   tile sits at, say, `(1,-1)` (dragged, resized, or restored), the new tile
   seeds at `(0,0)` — literally up-and-to-the-right of it. That is the reported
   symptom.
2. **Ring traversal is raw row-major, so ring 1 is entered at its top-LEFT
   corner.** `for row in -r..=r { for col in -r..=r { … } }` makes the first
   ring-1 candidate `(-1,-1)`. So even with the tile at the origin, tile #2 can
   never land same-row-right; it lands diagonally up-left.

Neither is a documented contract: UXI-Workspace-6 (`docs/components/workspace.md`)
and `spec-infinite-plane-workspace.md` Behavior 4 say "origin first, then its
8-neighborhood, then the next ring" — the *order within a ring* was unspecified,
and the ring-spiral's center was assumed to be the origin.

## Planned solution

Keep the ring-spiral (clustering, determinism, no insert-shift), change two
things:

- **Center the spiral on a reference tile** — the focused tile if it is already
  placed, else `last_reveal` (which, at the moment a brand-new leaf reconciles,
  still names the tile the user was on), else the origin. New API:
  `reconcile_near(leaves, near)` / `seed_slot_near(center)`; `reconcile` /
  `seed_slot` stay as origin-centered wrappers.
- **Order candidates within a ring by preference, not row-major**: same row
  first, right before left, below before above. Ring 1 becomes
  `(0,+1), (0,-1), (+1,0), (+1,+1), (+1,-1), (-1,0), (-1,+1), (-1,-1)`.

Result for the reported case: sole tile anywhere ⇒ new tile at
`(row, col+1)` when free.

## Approaches already tried (do NOT repeat)

- <none yet>

---

## Log

### 2026-07-21 — neighbor-centered, right-first ring spiral

**Changed**

- `src/bin/yalda-gpui/workspace.rs` — `seed_slot_near(center)` (preference-ordered
  ring scan: `(|dr|, dr<=0, |dc|, dc<=0)` with positives first) + `seed_slot()`
  as the origin wrapper; `reconcile_near(leaves, near)` resolves the spiral
  center from `near` → `last_reveal` → origin, `reconcile(leaves)` delegates.
- `src/bin/yalda-gpui/chrome.rs` — the per-frame plane upkeep now calls
  `reconcile_near(&leaves, Some(focused_id))`.
- Docs reconciled: UXI-Workspace-6 in `docs/components/workspace.md`, Behavior 4
  + the interface list in `docs/specs/spec-infinite-plane-workspace.md`.

**Verified**

- New guard on the REAL path: `verify_harness.rs`
  `new_tile_lands_same_row_right_of_the_only_tile` — boots a plane workspace with
  ONE tile parked at a non-origin slot `(1,-1)` (the exact configuration that
  produced "up and to the right"), fires the REAL `SplitV` action handler
  (`split_v`), lets the REAL `chrome.rs` render path reconcile the new slotless
  leaf, and asserts the new tile's slot is `(1, 0)` — same row, one col right.
- Unit-level: `seed_slot_spiral_deterministic` (rewritten for the new order) and
  `seed_slot_near_prefers_same_row_right` in `workspace.rs` desktop_tests.
- NEGATIVE CONTROL: reverted each half separately.
  - Restoring the row-major ring scan (old `seed_slot` body) ⇒
    `new_tile_lands_same_row_right_of_the_only_tile` FAILS with the new tile at
    `(0,-2)`, `seed_slot_near_prefers_same_row_right` FAILS at `(0,-2)` and
    `seed_slot_spiral_deterministic` at `(-1,-1)`.
  - Forcing `center = Slot::new(0,0)` in `reconcile_near` (origin-centered
    again) ⇒ the harness guard FAILS with the new tile at `(0,0)` — the reported
    up-and-to-the-right placement, reproduced exactly. (Note: merely passing
    `None` from `chrome.rs` does NOT go red — the `last_reveal` fallback covers
    it, which is itself part of the fix.)
  Both restored; full `cargo test --bin yalda-gpui` green afterwards.

**Outcome:** placement is now relative to the tile the user was on and prefers
same-row-right. Runtime-unverified beyond the headless paint/state guards (slot
math, not pixels — gap 1 does not apply; the guard drives the real reconcile).
