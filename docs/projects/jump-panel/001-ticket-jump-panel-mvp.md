# 001 — Jump panel MVP

**Goal:** Ship the always-visible root-level jump panel with Pinned (placeholder)
/ Workspaces / Agent-sessions sections, and the ephemeral-virtual-workspace
behavior for opening free sessions.

**Spec:** `docs/specs/spec-jump-panel.md` · **ADR:** `0021`.

## Subtasks

- [x] **#1 Panel surface + embed.** `jump_panel_view.rs` —
  `YaldaGpuiView::render_jump_panel(cx)` builds the sidebar **inline** (see
  decision note below), retaining only `jump_panel_scroll: ScrollHandle` on the
  root. Embedded in `YaldaGpuiView::render`, wrapping `screen_view` in a flex
  row at `JUMP_PANEL_WIDTH`. `record_render("jump_panel")` label.
  - **Decision change:** initially built as a `cached_child` view entity with a
    `JumpPanelSeqs` fingerprint + `cx.observe(&root)`. Abandoned: a root-embedded
    cached view that READS the root double-leases at construction and its
    notify-dirtying didn't propagate reliably (gpui accessed-entity tracking for
    a mid-render-created, parent-reading view). Inline is correct for an
    O(workspaces+sessions) surface — caching buys nothing. See spec "Rendering".
- [x] **#2 Sections render.** Titled section list: Pinned (empty placeholder),
  Workspaces (non-ephemeral tabs, active highlighted), Agent sessions (store
  ids, bound/free `●`/`○` indicator + label). Composed from `yux/detail.rs`
  `section_heading` + a local `jump_nav_row`.
- [x] **#3 Ephemeral virtual workspace + teardown.** `Tab::ephemeral: bool` +
  `Workspace::is_ephemeral`/`active_is_ephemeral`. `Workspace::open_ephemeral_tab`
  pushes a single-leaf agent tab bound to `sid` and activates it (replacing any
  existing ephemeral). `Workspace::set_active_tab` is the single switch chokepoint
  that tears down a departing ephemeral tab (remove tab → tile dropped → session
  free); `next_tab`/`prev_tab` + the view-level switches route through it.
- [x] **#4 Selection wiring.** `cx.listener` row clicks: workspace →
  `select_tab`; agent session → `jump_to_session` (bound → focus its tile via
  `agent_tile_id_bound_to` + `jump_to_window`; free → `open_ephemeral_tab`).
- [x] **#5 Filters.** Ephemeral tabs excluded from: Workspaces section, the `?`
  workspace-switch menu (`global_menu`), and persistence (`snapshot_workspace`
  filters + clamps `active_tab`).
- [x] **#6 Tests.** Lifecycle (verify_harness): free-session jump opens an
  ephemeral workspace then tears it down on switch-away (session back to free);
  a second free-session jump replaces (no accumulation); bound-session jump
  focuses the existing tile (no new tile). Plus `jump_panel_renders_with_sessions`
  (panel is live in the tree) and the one-time bounds settle absorbed in the
  `transcript_021_* / linear_*_is_render_flat / doc_selection_drag` harness tests.

## Verification

`cargo check` + `cargo test` green with pasted evidence. Runtime check is a
human pass (GPUI can't be driven headlessly for paint): panel visible across
workspace switches, active workspace highlighted, opening a free session shows
it transiently and it vanishes on jump-away. Flag the runtime items explicitly
in the worklog.

## Links

project.md · spec-jump-panel.md · ADR-0021 · ADR-0019 (tiles/apps) ·
spec-agent-session-ownership.md · yux/CLAUDE.md.
