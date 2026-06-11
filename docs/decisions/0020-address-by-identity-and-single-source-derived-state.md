# ADR-0020: Address async results by identity; derive hot-path state from a single source

Status: Accepted (2026-06-11)

## Context

Two agent-window defects surfaced in the same session, and they rhyme — both are
instances of *ambient* state standing in for *addressed* state:

1. **Picker "loading forever" + cross-tile bleed.** With two restored agent
   tiles each showing a session picker, one hung on "loading sessions…" forever
   while the other filled, and picking in one could open a session in another.
   Root cause: the async `list_sessions` result was applied to
   `agent_tile_mut()` — *the focused tile* — not the tile that requested it.
   Focus is correct at spawn time but drifts before the async result lands, so
   two pickers raced onto whichever tile happened to hold focus, and a
   `sessions.is_none()` guard let only one win.

2. **Worksheet typing slows as the session grows.** Every Worksheet keystroke
   recomputed the cursor's position in the virtualised transcript list from
   scratch — an O(transcript) gutter scan plus a tool/anchor/turn-header walk
   (`count_turn_headers_before` + `cursor_visible_child_index`) — in the key
   handler. The cost grew linearly with conversation length. Worse, those
   helpers *duplicated* the flat-items insertion logic (their comments said
   "must match the insertion logic in `render_agent`") — a standing correctness
   landmine on top of the perf cost.

The deeper pattern (see also the regression history in
`docs/worklog/` and ADR-0004): recurring agent-window regressions come from
**(a) reaching for ambient context (the focused tile, "now") instead of a stable
address, and (b) re-deriving per-keystroke state that should be computed once
from a single source.** Single-tile, single-keystroke cases mask both, so they
slip through until two tiles coexist or a transcript gets long.

## Decision

Two binding invariants, each enforced by an encapsulated API and a headless
regression test.

### INV-PR — Async results address a tile by stable identity, never by focus

- An async continuation (`cx.spawn`) MUST NOT route its result through
  `agent_tile_mut()` / `focused_window_id()` read *inside the continuation*.
  Focus can move between spawn and resolution.
- The originating tile is named by its stable `WindowId`, captured by the
  CALLER at spawn time, and resolved at apply time via
  `agent_tile_by_id_mut(id)` (scans all tabs; ids are unique workspace-wide and
  never reused).
- `spawn_list_sessions_for_picker(target: Option<WindowId>, …)` takes the
  target **explicitly** — it no longer reads focus internally, so a caller
  cannot accidentally misroute via focus drift; it must state which tile it set
  the loading picker on. The background close/reconcile path resolves the bound
  tile through the focus-independent query `agent_tile_id_bound_to(sid)`.
- This generalises the existing `open_token` pattern (create/attach already
  route by a stamped token). After this change, **no async reducer routes by
  focus**: picker→`WindowId`, open→`open_token`, attach/pump→session `id`.

### INV-RV — Per-keystroke "reveal" reads a single build-time index

- The doc-line → flat-item-index map (`AgentViewModel::line_to_item_cache`) is
  derived from the **canonical final `flat_items`** inside
  `rebuild_agent_view_model` (one source of truth — it cannot drift from what's
  rendered), cached alongside the flat list, and read O(1) via
  `AgentViewModel::item_for_line`.
- A Worksheet keystroke does **no** transcript-sized work: the key handler only
  records intent (`pending_reveal_cursor`); the render path consumes it with a
  single array read after the (memoised) view model is current.
- The duplicated `count_turn_headers_before` / `cursor_visible_child_index`
  helpers are **deleted** — the reverse index subsumes them, so there is no
  second copy of the insertion logic to keep in sync. (It also fixes a latent
  bug: the old formula ignored the blank-collapse pass; the map reads the
  post-collapse list and is correct by construction.)

## Enforcement (why this won't silently regress)

The app can't be driven headlessly for UX, so the guard is a passing test, not
"it feels fast":

- `verify_harness::picker_list_result_routes_to_originating_tile_not_focused` —
  two picker tiles in a split; a result addressed to tile A while B is focused
  must fill A, not B. Fails on the old `agent_tile_mut()` reducer.
- `verify_harness::session_close_shows_selector_on_bound_tile_not_focused` —
  asserts `agent_tile_id_bound_to(sid)` resolves the BOUND (unfocused) tile, the
  exact value a revert to `focused_window_id()` would get wrong, *and* drives
  `reconcile_session_closed` end-to-end.
- `tests::reveal_index_mirrors_flat_items_and_is_o1` — the reverse index points
  every line at its real flat position (incl. block-covered lines), clamps out
  of range, and is part of the memoised view model so same-fingerprint renders
  do zero rebuilds (per-keystroke work is O(changed)).

Convention (lint-by-review until a real lint exists): **no `agent_tile_mut()` or
`focused_*` read inside a `cx.spawn` continuation.**

## Consequences

- New async tile work must thread a `WindowId`/token; slightly more ceremony at
  call sites, in exchange for the misroute being unwriteable.
- `line_to_item` adds one `Vec<u32>` per rebuild (cache miss only); negligible.
- The map pairs `FlatItem::Block` items with `resolved` parsed ranges by forward
  order; a `debug_assert` checks all ranges are consumed.

## Alternatives rejected

- **Split `YaldaGpuiView` into per-tile GPUI entities** so `cx.notify()` repaints
  only one tile (the architectural root of "narrow change → broad repaint").
  Correct long-term direction, but a large, runtime-unverifiable refactor;
  deferred. INV-PR/INV-RV fix the two concrete failure classes without it.
- **Stamp a token on the picker** (mirroring `open_token`) instead of a
  `WindowId`. Equivalent; `WindowId` is already the stable address and needs no
  new field.
- **Keep the reveal in the key handler but reuse the cached gutter** — removes
  the allocation but leaves O(cursor_line) work. The reverse-index map is O(1)
  and deletes the duplicated logic, so it was preferred.
