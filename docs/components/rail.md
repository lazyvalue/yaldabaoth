# Component: Rail

**Status:** living
**Component token:** `Rail` (⇒ `UXI-Rail-N`)

## Description

A persistent **per-tab** side column (distinct from the root-level jump panel). Its
kinds: the **file-browser rail** (`Cmd-B` / `ToggleFileBrowserRail`) and the
**outline rail** (`ToggleOutlineRail`). Features: side flip (`FlipRailSide`);
rail-focused navigation (`RailDown` / `RailUp` / `RailSelect` / `RailClose` /
`RailParent`), hidden-file toggle, sort cycle, worktrees, and a filter input (under
the `RailView` key context). Primary code home: `chrome.rs`.

## References

- `docs/specs/spec-rail.md` — the rail's design and behavior.
- `docs/components/buffer.md` `UXI-Buffer-14` — the outline marks a heading folded in
  the focused Doc with `▸`.

## UX invariants

### UXI-Rail-1 — The outline lists the buffer's real headings, identically in both views

**Statement.** The outline rail lists every heading of the focused buffer (ATX and
setext, at any nesting depth — inside lists and quotes too) and nothing else: a
`#` line inside a fenced or indented code block is code, not a heading. The
rendered (Doc) and raw (Edit) views of the same text list the same headings,
because both derive from one parse (`yalda::render::outline`). Indentation is
relative to the shallowest heading present. The list re-derives whenever the
buffer's content version changes (Doc: `blocks_seq`; Edit: `edit_seq`).

**Applies to.** `chrome.rs` `derive_outline` / `outline_change_key`;
`src/render.rs` `outline`; `workspace.rs` `OutlineState`.

**Why.** The Edit outline scanned raw lines (picking up shell comments in code
fences) while the Doc outline saw only top-level heading blocks, so the two views
disagreed; the Doc change-key was the block count, so a same-count edit left it
stale.

**Status.** `implemented`

**Enforcement.** `md_harness.rs::outline_rail_selection_tracks_and_jumps` (fenced
`#` excluded); `render.rs::source_map_tests::outline_excludes_code_and_includes_setext_and_nested`.

### UXI-Rail-2 — "You are here" follows the document cursor

**Statement.** While the rail does not have focus, the outline highlights the
section containing the focused buffer's cursor (the last heading at or above it)
with an accent bar, and the rail's own selection follows it. Opening the outline
puts its selection on that section, never on row 0 by default.

**Applies to.** `chrome.rs` `refresh_outline_rail` / `outline_cursor_position`;
`OutlineState::track_current`.

**Why.** The selection was an index that never tracked the document, so the
outline opened at the top and kept stale indices across documents and focus
changes — "selection was always weird".

**Status.** `implemented`

**Enforcement.** `md_harness.rs::outline_rail_selection_tracks_and_jumps`.

### UXI-Rail-3 — Moving through the focused outline previews

**Statement.** With the rail focused, `j`/`k` move the rail selection (clamped at
the ends, no wrap-around) and the buffer follows: its cursor moves to that heading
and the heading scrolls to the top of the view. Focus stays in the rail.

**Applies to.** `browser_ui.rs` `rail_down` / `rail_up` / `outline_preview_selected`.

**Why.** Browsing an outline should show you where each heading leads; wrap-around
made a long outline jump unexpectedly from the bottom to the top.

**Status.** `implemented`

**Enforcement.** `md_harness.rs::outline_rail_selection_tracks_and_jumps`.

### UXI-Rail-4 — Enter or click jumps and hands focus back

**Statement.** Enter on an outline row, or clicking it, puts the buffer's cursor on
that heading, scrolls the heading to the top of the view (not merely into view at
the bottom), and returns focus to the buffer. The rail stays open.

**Applies to.** `browser_ui.rs` `rail_select` / `outline_activate` / `outline_jump_to`;
row `on_mouse_down` in `chrome.rs` `render_rail`.

**Why.** Enter previously left focus in the rail and revealed the heading at the
bottom edge; rows weren't clickable.

**Status.** `implemented` (click shares `outline_activate` with Enter; the mouse
dispatch itself is not separately guarded).

**Enforcement.** `md_harness.rs::outline_rail_selection_tracks_and_jumps`.

### UXI-Rail-5 — The highlighted outline row is always painted inside the rail

**Statement.** However long the outline and however short the window, the
highlighted row (selection when focused, "you are here" otherwise) is scrolled
into the rail's visible area. The row list is virtualized to the rail's real
height, and it reveals only when the highlighted row changes, so a manual
wheel-scroll of the rail isn't yanked back.

**Applies to.** `chrome.rs` `render_rail` (`uniform_list` + `outline_scroll`,
`outline_revealed`).

**Why.** A fixed 40-row window ignored the rail's height, so in a short window the
selection was clipped out of view and the list re-centered in jumps.

**Status.** `implemented`

**Enforcement.** `md_harness.rs::outline_rail_selected_row_stays_painted_in_short_window`
(layout probe). Exact colors: human check (gap #1).
