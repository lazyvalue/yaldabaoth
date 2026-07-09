# 003 — One wrapped-line renderer; delete the chatbox grid stack

**Unification target 1 (the prize — kills the largest bug family).**

Two renderers draw a wrapped line with a caret:

- `build_wrapped_line` (`agent.rs`) — GPUI flex-wrap; caret split via
  `caret_token_split` (added in batch 1); used by buffer Code/WP + transcript.
- `build_chatbox_line` / `wrap_line_cols` / `caret_visual_row` /
  `build_chatbox_wrapped_line` (`agent.rs`) — hand-computed monospace column
  wrap; a SECOND copy of the caret-split logic and a SECOND selection painter
  (`emit_chunk`); used by the compose + inline You-block.

The chatbox renderer exists only to make the caret's visual row computable for
the compose caret-containment window (`ComposeWindow` / `compose_window`,
`CHATBOX_CHAR_W`, `compose_visual_metrics`, the windowing in `screens.rs`). That
whole grid apparatus is the root of the recurring "cursor off-screen in the
chatbox" bug (see `project_chatbox_offscreen_recurring`).

## Goal

Converge the compose onto `build_wrapped_line` + `scroll_to_reveal_item` (the
mechanism buffers already use), then delete the grid-window machinery
(~450–600 LOC): both chatbox builders, `wrap_line_cols`, `caret_visual_row`,
`compose_visual_metrics`, `compose_item_for_visual_row`, `ComposeWindow`,
`CHATBOX_CHAR_W`, and the bounds-capture plumbing on `Compose`. The compose gets
buffer typography for free.

## Subtasks (incremental)

- [ ] Fold `emit_chunk` selection painting into `apply_selection_bg` (pure, low
      risk).
- [ ] Render the inline You-block through `build_wrapped_line`.
- [ ] Render the compose panel through `build_wrapped_line`.
- [ ] Replace the caret-containment window with `scroll_to_reveal_item` (note the
      documented caveat: it reveals a logical line, not a visual row — for a
      single wrapped line taller than the viewport, compute the caret's wrap row;
      see INV-UX-1 subtlety note).
- [ ] Delete the grid-window machinery.

## Verification

INV-UX-1 / INV-UX-2 have paint-level probes (`probe_bounds("compose-cursor-row")`)
— assert the caret paints inside the viewport, non-vacuously (content taller than
the viewport). Keep the render-count guards green. This is the one that finally
makes "the compose is a small buffer" true.
</content>
