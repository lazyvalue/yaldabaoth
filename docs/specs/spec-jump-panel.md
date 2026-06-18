# Spec: Jump Panel (root-level navigator sidebar)

Status: Draft (2026-06-18)
Related: ADR-0021 (ephemeral virtual workspace), `spec-rail.md` (the *other*,
per-tile column — distinct from this), `spec-agent-session-ownership.md`.

## Summary

A permanent left-side sidebar that lets you jump to yalda objects from anywhere.
Unlike the per-tile **rail** (`spec-rail.md`, `Tab::rail`, pinned to a
`WindowId`), the jump panel lives at the **root render level** and is therefore
visible across all workspaces. It is a read-only navigator: it never owns
content, only points at it.

## Layout & placement

- Root render (`YaldaGpuiView::render`, main.rs) wraps the existing
  `screen_view` in a flex **row**: `[ JumpPanel (fixed width) | screen_view ]`.
  The panel sits outside the workspace/tab content, so switching workspaces
  never rebuilds or hides it.
- **Always visible.** No toggle, no collapse (MVP). Fixed width (reuse
  `RAIL_DEFAULT_WIDTH = 200px` as the starting constant; own constant
  `JUMP_PANEL_WIDTH`).
- Chrome-class surface: renders at native size, unaffected by document zoom
  (consistent with tab strip / rail).

## Sections (heterogeneous, ordered, titled)

The panel is a vertical list of titled sections. MVP ships three; the type is
open for more object types (files, Linear, etc.) later.

1. **Pinned** — titled section, **empty placeholder** for MVP. Pinning
   mechanics land later; the header renders now so the affordance exists.
2. **Workspaces** — one row per **non-ephemeral** `Tab`, label =
   `Tab::display_label()`. The **active** workspace row is highlighted
   (`workspace.active_tab`). Ephemeral (virtual) workspaces never appear here.
3. **Agent sessions** — one row per `SessionId` in the `AgentSessions` store
   (`self.sessions.ids()`). Row shows the session label/title and a **bound vs
   free** indicator.

## Selection semantics

- **Workspace row** → set `active_tab` to that tab (switch workspace). If a
  virtual workspace is currently active, switching away tears it down
  (ADR-0021).
- **Bound agent session** (`agent_tile_id_bound_to(sid).is_some()`) → switch to
  the tab containing its tile and focus that tile. No new tile created.
- **Free agent session** (no tile binds it) → create an **ephemeral virtual
  workspace** holding a single tile bound to that session, and make it active
  (ADR-0021). Selecting a *different* free session replaces the current virtual
  workspace rather than accumulating. Navigating to any real workspace/session
  tears the virtual one down and returns the session to **free**.

## Rendering — inline, not a cached child

The panel is rendered **inline** by `YaldaGpuiView::render_jump_panel`, not as a
`cached_child` view entity. This is a deliberate departure from the
`TranscriptView`/`LinearView` reference pattern, and the reasoning is the yux
cost model itself:

- Those surfaces are cached because they are **expensive** (O(conversation),
  O(issue body)) and stable while you type elsewhere — caching skips that cost
  per keystroke. The jump panel's content is O(workspaces + agent sessions): a
  handful of short rows. GPUI already re-renders the root every frame, so
  rebuilding these rows inline is negligible, and the perf guarantee that
  matters — *the panel must not bloat the expensive cached surfaces beneath it* —
  is unaffected (the transcript/linear render-flat tests still hold).
- A cached child that READS the root and is embedded AT the root hits gpui's
  accessed-entity invalidation in a way that does not reliably re-render here
  (a view created mid-render that reads its own leased parent). Inline avoids
  that entire class of problem, the double-lease on root reads, and any
  observe/notify wiring.

So the panel:

- **Reads directly off `self`:** `workspace` (tabs, active_tab), `sessions` (ids
  + bound state via `agent_tile_id_bound_to` + labels), and theme — no weak
  handle, no snapshot-into-entity.
- **Retains only scroll state** on the root (`jump_panel_scroll: ScrollHandle`).
- **Resolves row clicks at event time** via `cx.listener` calling
  `select_tab` / `jump_to_session` — ids/indices captured by value, never row
  data closed over from a prior build.
- **Insets the content area**, so a surface beneath it re-measures **once** as
  geometry settles (a benign one-time bounds render, not a per-keystroke cost).

## Invariants

- INV-JP1 — the jump panel is a single root-level instance, never per-tile;
  it is never nested in the layout tree.
- INV-JP2 — the panel mutates nothing it reads; selection actions call existing
  encapsulated APIs (`active_tab` setter, focus-existing-session,
  create-virtual-workspace) rather than poking fields.
- INV-JP3 — ephemeral workspaces are invisible to the Workspaces section and to
  persistence (ADR-0021).

## Out of scope (MVP)

Pinning mechanics; drag-reorder; non-session/non-workspace object types;
collapse/resize; keyboard-driven panel focus model (selection is pointer-first;
a focus/keynav pass is a follow-up ticket).
