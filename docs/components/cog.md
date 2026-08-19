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

### UXI-Cog-3 — The right detail pane is scrollable, keyboard-focusable, and independent of selection

**Statement.** The right detail pane scrolls (`d`/`u`, PageUp/PageDown) independently
of the left selection; changing the selected node resets it to the top; the left
selector keeps its selection in view. Keyboard **focus** can move to the detail pane
(`Enter`/`o`/`l`/`→`/`Tab`) so `j`/`k`/arrows scroll it instead of moving the left
selection; `Esc`/`h`/`←`/`Tab` returns focus to the selector. The focused pane shows
a faint accent wash. Focus resets to the selector on every state change.

**Applies to.** `CogView::scroll_right`, `CogFocus` + `focus_right`/`focus_left`/
`toggle_focus`/`focused_right`, the `right_scroll` reset in `CogView::select_move`,
`left_scroll.scroll_to_item`, `focus_tint` (`cog_view.rs`). `handle_cog_press`,
`cog_scroll`, `cog_set_focus`, `cog_toggle_focus` (`cog_ui.rs`).

**Why.** The requirement: "Right pane should be scrollable" and "Need to be able to
move focus to the right pane so I can scroll with keyboard." A node's detail
(content + output + transitions + notes) routinely exceeds the viewport.

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_right_pane_scrolls_and_clamps` (the right
scroll offset moves on `d` and clamps at the top on `u`);
`cog_right_focus_scrolls_with_jk` (real `handle_cog_press`: `l` focuses the detail
pane, then `j` scrolls it and does NOT move selection; `h` returns focus and `j`
selects again — negative-control verified RED); and the render-count guard
`cog_body_is_cached`.

### UXI-Cog-4 — Detail is presented as cards + pretty-printed JSON

**Statement.** In the node detail pane, each status transition and each note renders
as its own rounded, hairline-bordered card (visually separated for scanning).
Structured `content` / `output` JSON is pretty-printed (2-space indent) in a
monospace code block; a bare-string content renders as prose. The left selector
uses a smaller font and truncates long graph / node names with an ellipsis rather
than wrapping.

**Applies to.** `card` / `note_card` / `transition_card` / `json_block` /
`truncating_label` (`cog_view.rs`); `json_prose` / `json_is_structured`
(`cog.rs`).

**Why.** The `/new-ux` refinement: "Each update should have a small very stylish
bounding box (like a card)"; "JSON needs to be pretty printed"; "Left pane is
actually too narrow. Long graph names word wrap awkwardly … make font a bit smaller."

**Status.** implemented

**Enforcement.** Card/JSON/ellipsis styling is paint-only (genuine runtime gap #1 —
exact glyphs/colours need a human eye); the structural pieces are unit-guarded by
the pretty-print branch of `json_prose` and the existing
`cog_detail_paints_and_overflows` (the detail column, now cards + code block, still
paints and overflows its viewport).

### UXI-Cog-5 — The tile is mouse-clickable

**Statement.** Clicking a graph row in the explorer opens that graph (the
keyboard-Enter equivalent). Clicking a node row selects it (its detail fills the
right pane) and returns keyboard focus to the selector. Clicking the right detail
pane moves keyboard focus there so `j`/`k` scroll it. Mouse-wheel scrolling of the
detail pane works natively (`overflow_y_scroll`).

**Applies to.** `CogView::click_graph` / `click_node` / `click_focus_right`, the
`.on_click(cx.listener(…))` wiring on graph/node rows and the right pane
(`cog_view.rs`); `cog_open_graph_for` / `cog_fetch_graph` (`cog_ui.rs`). The
click handler sets its own loading state and hands id/label to the root so the
root never re-updates the still-borrowed `CogView` (a reentrant-borrow panic).

**Why.** The `/new-ux` refinement: "Should be able to click on things." Keyboard-only
navigation is not discoverable.

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_click_node_selects` (real `click_node`:
selects row 2, returns focus to the selector — negative-control verified RED),
`cog_click_right_pane_focuses_it` (real `click_focus_right`), and
`cog_click_graph_row_opens` (real `click_graph` routes through `cog_open_graph_for`
→ the tile enters loading; the live `cog` fetch is runtime gap #2, not pumped).

### UXI-Cog-6 — A live-events strip streams the graph's event feed across the bottom

**Statement.** While a graph is open, a full-width strip ACROSS THE BOTTOM streams
the graph's live event feed from `cog graph watch <id>`. Each event is an
aesthetically formatted, syntax-highlighted, pretty-printed JSON card, newest first
(bounded to the last 300). The watcher starts when a graph loads, restarts when a
different graph loads, and is killed when leaving the graph or closing the tile (no
orphaned subprocess). **Every live event auto-refreshes the graph** — the bundle
reloads in place (coalesced: one reload in flight + one queued) so the node list /
detail / stats track the change — and the **events feed persists across that refresh**
(and manual `r`); only a graph change clears it. Keyboard focus reaches the strip via
`Tab` (Selector → Detail → Events → Selector); `j`/`k`/`d`/`u`/PageUp/Down scroll the
focused pane; clicking it focuses it. The strip exists only in a loaded graph.

**Applies to.** `CogView::events` / `push_event` / `events_pane` / `event_card` /
`scroll_events` / `focus_events` / `toggle_focus`, `CogFocus::Events` (`cog_view.rs`);
`cog::spawn_watch`, `CogTile::{watch, watch_gen}` + `Drop` (`cog.rs`);
`cog_start_watch` / `cog_stop_watch` / `cog_push_event` / `cog_scroll_events`,
the `cog_apply` watch hooks, `handle_cog_press` focus/scroll routing (`cog_ui.rs`).

**Why.** The `/new-ux` requirement: "When I am reviewing a graph I should be able to
see the live (sse) events streaming."

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_events_stream_into_pane` (real
`cog_push_event`: events fold in newest-first, generation-guarded, invalid JSON
dropped — negative-control verified RED on the newest-first insert);
`cog_events_pane_paints_and_focus_cycles` (layout-probe: no `cog-events` pane in the
explorer, a real-sized one in a graph; `Tab` cycles Selector → Detail → Events →
Selector via real `handle_cog_press`); `cog_event_auto_refreshes_and_preserves_events`
(a live event sets the coalescing `refreshing` flag; `cog_apply_refresh` →
`update_bundle` updates the node set while KEEPING the feed — negative-control
verified RED by clearing events in `update_bundle`). The live `cog graph watch`
subprocess ↔ strip loop is runtime gap #2 (`cfg(test)` skips the spawn); confirm
against `cogd` at runtime.

### UXI-Cog-8 — The left panel has an Overview tab (graph render + stats)

**Statement.** In a loaded graph the left panel lists `[Overview, nodes…]` — an
**Overview** row at the top. Selecting it (click, or `k`/↑ up from the first node)
shows, in the detail pane, the graph's rendered structure (`cog graph render`) plus
aggregate **stats**: node count, counts by status, and claimed→done completion
times (count, quickest, longest, average).

**Applies to.** `CogViewState::Graph { overview }`, `overview_row` / `overview_body`
/ `click_overview` / `showing_overview`, `select_move` (Overview at linear index 0)
(`cog_view.rs`); `CogBundle::stats` / `completion_ns` / `fmt_duration_ns`,
`CogBundle.render` (`cog.rs`).

**Why.** The `/new-ux` request: "on the left panel, create an Overview listing (tab)
at the top … render the graph and give stats: number nodes, quickest / longest /
average completion time."

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_stats_completion_times` (real
`bundle.stats()` math: counts + quickest/longest/avg from node logs) and
`cog_overview_reachable_and_toc_jumps` (real `cog_select`: `k` up from node 0 reaches
the Overview and its body paints — layout-probe). Exact graph-render glyphs are
runtime gap #1.

### UXI-Cog-9 — Node detail has a Table of Contents, State transitions first

**Statement.** The node detail pane opens with a **Table of Contents** of clickable
section chips; clicking one jumps the pane to that section. The sections are ordered
**State transitions first**, then Content, Output (when present), Notes.

**Applies to.** `node_header` / `node_sections` (transitions pushed first) / the TOC
chips + `scroll_node_section` / `click_node_section` in `right_pane` (`cog_view.rs`).

**Why.** The `/new-ux` request: "On node detail put a Table of Contents at the top so
I can jump down to different sections. Always make State transitions the first item."

**Status.** implemented

**Enforcement.** `verify_harness.rs::cog_node_sections_state_transitions_first` (real
`node_sections`: section[0] is Status transitions, then Content/Output/Notes —
negative-control verified RED by reordering) and the TOC-jump tail of
`cog_overview_reachable_and_toc_jumps` (a chip click scrolls the detail pane).

### UXI-Cog-7 — JSON is syntax-highlighted everywhere in the tile

**Statement.** Every JSON surface in the tile — node content, node output, and each
live-event card — renders pretty-printed with syntect syntax highlighting (keys,
strings, numbers, literals, punctuation each coloured), theme-aware (the theme's
syntect theme), in a monospace code block.

**Applies to.** `highlighted_json` / `json_highlighter` (cached per syntect theme) /
`json_block` / `event_card` (`cog_view.rs`), built on
`yalda::highlight::Highlighter` with the `"json"` syntax.

**Why.** The `/new-ux` requirement: "add syntax highlighting to JSON everywhere in
this tile. All JSON pretty printed."

**Status.** implemented

**Enforcement.** Exact token colours are paint-only (runtime gap #1 — a human eye);
the highlighter path is exercised structurally by
`cog_detail_paints_and_overflows` (content renders through `json_block` →
`highlighted_json`) and `cog_events_pane_paints_and_focus_cycles`.
