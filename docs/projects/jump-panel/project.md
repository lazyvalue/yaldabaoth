# Project: jump-panel

**Status:** 🟢 MVP code-complete — all of ticket 001 landed; builds + 214 tests
green. Runtime check still owed (GPUI can't be driven headlessly for paint):
panel visible across workspace switches, active workspace highlighted, free
session opens transiently and vanishes on jump-away. Built on the `jump-panel`
worktree/branch (off `main` @ c6e7e01).
**Spec:** `docs/specs/spec-jump-panel.md`.
**Decision:** `docs/decisions/0021-ephemeral-virtual-workspace.md`.
**Tickets:** `001-ticket-jump-panel-mvp.md`.

## Problem / Why

There is no persistent, cross-workspace way to see and reach yalda objects.
Workspaces are switched only from the `?` menu; **free** agent sessions (in the
store, bound by no tile) have no entry point at all once their tile is closed.
The old side tab-strip was removed (main.rs:6286) and nothing replaced its
navigational role.

We want a permanent left **jump panel**: a heterogeneous, sectioned navigator
(Pinned / Workspaces / Agent sessions, more object types later) that is visible
across all workspaces and lets you jump to anything — including displaying a free
session transiently without permanently creating a workspace for it.

## Goals

- A single **root-level** sidebar, visible across workspaces, built as one
  cached-child yux view (render-flat while typing elsewhere).
- Heterogeneous titled sections; active workspace indicated.
- Free agent sessions reachable via an **ephemeral virtual workspace** that
  self-destructs on jump-away (ADR-0021), preserving the 1:1 session↔tile
  invariant.
- No new scattered special-casing: ephemeral lifecycle in one chokepoint;
  selection actions go through existing encapsulated APIs.

## Scope

**In:** the root-level panel surface + its three MVP sections; the
ephemeral-virtual-workspace model + teardown chokepoint; persistence/menu
filters for ephemeral tabs; headless render-count + lifecycle tests.
**Out (MVP):** pinning mechanics (placeholder section only), panel keyboard
focus/keynav, resize/collapse, drag-reorder, non-session/non-workspace object
types.

## Model

```
YaldaGpuiView
 ├─ render_jump_panel(cx) -> AnyElement   ← INLINE (not a cached child); reads
 │     self.workspace.{tabs, active_tab}, self.sessions, theme directly
 │  jump_panel_scroll: ScrollHandle        ← only retained UI state
 └─ workspace: Workspace<App>
       tabs: Vec<Tab>          Tab::ephemeral: bool
       active_tab: usize       (switch funnels through set_active_tab → teardown)
```

- Panel render: `[ JumpPanel(JUMP_PANEL_WIDTH) | screen_view ]` wrapping the
  existing root content, built inline in `YaldaGpuiView::render`.
- **Inline, not cached** (see spec "Rendering"): the panel is O(workspaces +
  sessions) — cheap — so caching buys nothing, and inline sidesteps the
  unreliable dirty-tracking + double-lease of a root-embedded, root-reading
  cached child. The perf guarantee that matters (don't bloat the *expensive*
  cached surfaces) is held by the existing transcript/linear render-flat tests.
- Selection: workspace → `set_active_tab`; bound session → focus its tile;
  free session → create/replace ephemeral virtual workspace (`jump_to_session`).
- A one-time bounds settle (the panel insets content) is absorbed before the
  baseline in the `*_is_render_flat` harness tests.

## Invariants

INV-JP1 single root-level instance, never per-tile · INV-JP2 panel mutates
nothing it reads (selection via existing APIs) · INV-JP3 ephemeral workspaces
invisible to Workspaces section + persistence.

## Tickets

| Ticket | Subtasks | Status |
|---|---|---|
| 001-ticket-jump-panel-mvp | #1 inline panel + embed · #2 sections render · #3 ephemeral virtual workspace + teardown · #4 selection wiring · #5 filters (persist + `?` menu) · #6 tests (lifecycle + settle) | ✅ code-complete; runtime check owed |
