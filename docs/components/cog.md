# Component: Cog

**Status:** living
**Component token:** `Cog` (⇒ `UXI-Cog-N`)

## Description

`App::Cog(CogTile)` — a read-only tile that explores a [Cog](../../../cog) planning
graph (opened with `Cmd-G` / `Ctrl-G`, the `.` → new → cog menu, or the macOS File
menu). It shells out to the local `cog` CLI (which talks to `cogd` over HTTP,
honoring `COG_ADDR`) and renders the returned JSON. There is no writing — it only
reads.

The tile has two panes:

- **Left (selector).** First a **graph explorer** — the list of every graph
  (`cog graph list`), one selectable row each (name + id + sealed/prototype marks).
  Opening a graph swaps the left pane for that graph's **node list** — one
  selectable row per node (name + a coloured status badge). `j`/`k` (or ↑/↓) move
  the selection, which always stays in view; `Enter`/`o`/`l` opens the highlighted
  graph; `Esc`/`h` returns to the graph list; `r` refreshes.
- **Right (detail, scrollable).** For a highlighted graph: its id, sealed/prototype,
  omega, and description. For a highlighted node: its name + id + status badge, its
  **content**, its **output** (when present), its **status-transition timeline** (from
  `cog node log`, one row per `status_changed`, showing the new status + actor +
  time), and its **notes** (from `cog graph read-node-notes`, topic + prose + actor +
  time). Scrolls with `d`/`u` (half-page) and PageDown/PageUp.

Every node carries an **effective status**: the stored status (`done`, `claimed`,
`failed`, `abandoned`) as-is, but an `open` node is split into `ready` (all
predecessors done) vs `blocked` — computed locally from the edge set. Each status
has a distinct colour in both the left-list badge and the right-pane header.

The body is a cached child entity (`CogView`, built on **yux**), so it re-renders
only when its own payload / selection / scroll changes, not on unrelated typing.
The cheap `CogTile` (title + monotonic `req` guard + the view handle) holds no
payload; the loaded graph list / bundle lives in `CogView`. Fetches run on the
background executor, never the paint thread; a monotonic `req` discards a stale
response.

Primary code home: `cog.rs` (subprocess client + data model + `CogTile`),
`cog_ui.rs` (view-layer open/load/select/scroll/key methods), `cog_view.rs` (the
cached two-pane body `CogView`).

## References

- [../../../cog](../../../cog) — the Cog repo (CLI + data model reference).
- `yux/CLAUDE.md` — the cached-view component layer the body is built on.
- `docs/components/linear.md` — the sibling read-only App tile this is modelled on.

## UX invariants

### UXI-Cog-1 — A Cog tile opens on the graph explorer

**Statement.** Opening a Cog tile (`OpenCog` / `Cmd-G` / new-cog-tile / File menu)
creates an `App::Cog` tile whose left pane is the graph explorer: the list of all
graphs from `cog graph list`, first row selected. It never opens directly into a
graph.

**Applies to.** `open_cog` / `open_cog_inner` (`cog_ui.rs`), `cog_load_graphs`,
`CogViewState::Graphs`. Render dispatch `chrome.rs` `App::Cog`.

**Why.** The requirement: "A new tile should have a file explorer view that lets me
pick which graph I want to explore." Without this the tile has no entry point.

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_opens_on_graph_explorer` (drives the real
`open_cog_inner` + `cog_apply` with a synthetic graph list, asserts the view state is
`Graphs` with the graphs present).

### UXI-Cog-2 — Selecting a node shows its detail in the right pane

**Statement.** In a loaded graph, moving the left selection to a node makes the right
pane show that node's content, output (if any), status, status-transition timeline,
and notes. The selected node and the rendered detail are always the same node.

**Applies to.** `CogView::select_move`, `CogView::right_pane` / `node_detail`
(`cog_view.rs`). `cog_select` (`cog_ui.rs`).

**Why.** The requirement: "In left pane … select nodes. In right pane … see contents
and notes … status and status transitions of everything."

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_node_selection_resets_right_scroll` (real
reducer `cog_apply` + `cog_select`: selection advances and the right pane resets to
top — negative-control verified RED) and `cog_detail_paints_and_overflows`
(layout-probe: the right-pane content paints taller than its viewport — non-vacuous).

### UXI-Cog-3 — The right detail pane is scrollable and independent of selection

**Statement.** The right detail pane scrolls (`d`/`u`, PageUp/PageDown) independently
of the left selection; changing the selected node resets the right pane to the top.
The left selector also keeps the selection in view.

**Applies to.** `CogView::scroll_right`, the `right_scroll` reset in
`CogView::select_move`, `left_scroll.scroll_to_item` (`cog_view.rs`). `cog_scroll`
(`cog_ui.rs`).

**Why.** The requirement: "Right pane should be scrollable." A node's detail
(content + output + transitions + notes) routinely exceeds the viewport.

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_right_pane_scrolls_and_clamps` (the right
scroll offset moves on `d` and clamps at the top on `u`) + the render-count guard
`cog_body_is_cached` (a root-only notify leaves `CogView`'s render count flat; a
payload change busts it once — negative-control verified RED).
