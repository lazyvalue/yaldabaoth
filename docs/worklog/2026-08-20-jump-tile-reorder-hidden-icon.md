# Jump panel: tile drag-reorder + hidden-icon polish

**Date:** 2026-08-20
**Cog graph:** `0uc` (jump-tile-reorder-and-icons) — status `complete`
**Branch:** `jump-tile-reorder` → merged to `main` (`b1cb982`)

## Cog execution evidence

- Graph id: `0uc`

### Initial render

```
graph jump-tile-reorder-and-icons (frontiers)
frontier 0: icons: enlarge + hidden→icon [open], reorder: state + persistence [open]
frontier 1: reorder: drag wiring [open]
frontier 2: UXI + headless tests + negctrl [open]
frontier 3: omega [open] (omega)
```

### Node execution

Each node was claimed and closed with output (actor `claude-code`):

- **`ak1h` — icons: enlarge + hidden→icon** → `done`. Output: badge glyph
  `pt*1.15`; hidden text pill replaced by dim `⊘` fixed-width far-trailing cell.
- **`q1o9` — reorder: state + persistence** → `done`. Output:
  `Preferences::jump_tile_order Option<Vec<WindowId>>` + view field +
  init/save/load; per-folder stable rank sort; `reorder_tile` + `reorder_move_win`.
- **`fyd6` — reorder: drag wiring** → `done`. Output: `TileDrag` + `attach_tile_drag`
  wired on workspace-folder tile rows (agent + non-agent).
- **`fm3e` — UXI + headless tests + negctrl** → `done`. Output: `UXI-JumpPanel-28`;
  two guard tests; negative control observed RED; hidden-marker geometry test
  updated; row-height regression fixed via badge line-height clamp; 671 tests pass.
- **`tqhr` — omega** → `done`. Output: builds + 671 tests green; negative control RED.

### Notes

- (node `fm3e`, topic `decision`) Invariant numbered `UXI-JumpPanel-28` (next free;
  23–27 taken). Hidden glyph `⊘` shares the Unicode block of the already-rendering
  `⊞`, so font coverage is safe. Reorder is display-order-only within a workspace
  folder, folder-gated, persisted by durable `WindowId` — analog of the
  session-level `UXI-JumpPanel-2`. Detached/tag-folder tiles intentionally NOT
  draggable this pass.

### Final status

- Status: `complete`

```
{"status":"complete","islands":"none","sealed":false}
```

```
graph jump-tile-reorder-and-icons (frontiers)
frontier 0: icons: enlarge + hidden→icon [done], reorder: state + persistence [done]
frontier 1: reorder: drag wiring [done]
frontier 2: UXI + headless tests + negctrl [done]
frontier 3: omega [done] (omega)
```

## What shipped

Three user-requested jump-panel changes (`UXI-JumpPanel-28`):

1. **Tiles reorder by drag within their workspace folder.** New `TileDrag`
   payload + `attach_tile_drag` helper wire `on_drag`/`can_drop`/`drag_over`/
   `on_drop` on both non-agent (`◇`) and agent-backed tile rows. The drop calls
   `reorder_tile`, which is folder-gated (a tile never crosses folders) and
   rebuilds a global `jump_tile_order: Vec<WindowId>`, persisted in
   `Preferences::jump_tile_order`. The order applies as a **stable** per-folder
   sort by rank in `jump_panel_sections_with_tab` (empty = layout order). Direct
   analog of the session-level reorder (`UXI-JumpPanel-2`).
2. **Hidden marker is now an icon.** The ugly, inconsistently-placed `"hidden"`
   text pill (`compact_status_mark`) became a single dim `⊘` glyph in a
   fixed-width cell at the row's FAR trailing edge — identical placement for tile
   and agent rows. Achieved by moving the `status_mark` slot after the hint in
   `jump_nav_row_hinted`.
3. **Leading nav glyphs are slightly larger.** Badge `text_size` bumped to
   `pt*1.15` in an 18px cell, with `line_height` pinned to the base row line box
   (`pt*1.618`) so the bigger glyph does NOT grow row height — fixed chrome holds
   (`UXI-JumpPanel-24/26`).

## Verification

- `cargo test --bin yalda-gpui`: **671 passed, 0 failed, 2 ignored.**
- New guards: `jump_tile_reorder_move_semantics` (WindowId list surgery) and
  `jump_tile_reorder_applies_within_folder_and_gates_by_folder` (REAL
  `reorder_tile`: default = layout order, within-folder drag reorders + persists,
  cross-folder refused). Prefs round-trip field asserted.
- **Negative control observed RED:** deleting `tiles.sort_by_key(rank)` makes the
  reorder assertion fail (`left: [1,2,3]` layout order vs `right: [3,1,2]`).
- Updated `jump_panel_hidden_tiles_paint_indicator` to the new icon geometry;
  fixed a row-height regression the badge bump first introduced by clamping the
  badge line-height.

## Open / caveats

- The GPUI mouse-drag GESTURE that dispatches the drop is `NEEDS-RUNTIME`
  (harness gap #2 — no headless drag-dispatch seam); the state change the drop
  runs IS headlessly tested. The exact `⊘` glyph + enlarged badge are a
  paint/human-eye detail (gap #1). Not yet exercised in the live binary.
- Detached and tag-folder tile rows intentionally do NOT drag this pass — only
  workspace-folder tiles.
