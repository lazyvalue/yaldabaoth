# bug-0033 — a stray ` / ``` bleeds and ruins the rest of the transcript

**Status:** FIXED
**First seen:** 2026-08-07
**Component:** AgentTile / transcript rendering (`highlight_cache.rs`,
`agent.rs::detect_block_ranges`).

## Symptom

"Sometimes ` or ``` bleeds from agent text and ruins the rest of the transcript."
An agent message containing an unbalanced code fence (a `` ``` `` that never closes
in that message) makes everything AFTER it — later agent turns, user turns, the live
draft — render as code (code background + monospace), or get swallowed into one giant
code block.

## Context / root cause

A code fence is parsed as spanning the WHOLE transcript document, not one agent
message. Two independent places carry the fence state across turn boundaries:

1. **`highlight_cache.rs::snapshot_inner`** folds a single running `FenceState` over
   ALL lines of the document (`fence = advance_fence(&lines[i], &fence)`), with no
   turn awareness. A stray `` ``` `` opens the fence and every subsequent line — in
   any later turn — is highlighted with `code_block_bg`.
2. **`agent.rs::detect_block_ranges`** searches for the closing fence with an
   UNBOUNDED `while i < lines.len()` loop that ignores frozen-range boundaries. A
   stray `` ``` `` in one turn pairs with the next `` ``` `` anywhere later and emits
   one `FlatItem::Block` covering everything between (including user turns).

## Planned solution

A code fence is bounded to the contiguous **frozen range** (one committed agent
message) it opens in:

- **Highlighter:** pass `frozen_ranges` into `snapshot_inner`; reset
  `FenceState` to closed at every frozen-span boundary (a line whose frozen-region
  membership differs from the previous line's). A real, balanced code block lives
  inside one turn, so it is unaffected; only cross-turn bleed is reset. Composes with
  the `fence_before[i]` cache (the reset is reflected in each line's entry fence fp).
- **Block detector:** bound the closing-fence search in `detect_block_ranges` to the
  opening fence's frozen range; if no close is found within the turn, emit no block
  and leave the lines as plain Lines (matching the existing streaming behavior).

No change to balanced code blocks. The single-backtick inline case shares root #1
(a `` `code` `` span never crosses lines in the highlighter, but a stray `` ``` `` is
the catastrophic one; inline handling is per-line in `highlight_source_line`).

## Log

### 2026-08-07 — FIXED (fence bounded to the agent turn, in both subsystems)

- **Change.**
  - `highlight_cache.rs::snapshot_inner` now takes `frozen_ranges` and resets
    `FenceState` at every frozen-span boundary (a pointer walks the sorted ranges;
    `region` = containing range start or -1; a change resets the fence). Composes
    with the `fence_before[i]` cache (the reset shows up in each line's entry fp).
    `snapshot_syn` grew the `frozen_ranges` param; the transcript passes the real
    ranges, the buffer/doc path + tests pass `&[]` (whole-doc fences, unchanged).
  - `agent.rs::detect_block_ranges` bounds the closing-fence search to the opening
    fence's frozen range (`turn_end`); on no close it resumes from `start+1`
    (rest of the turn parsed normally) instead of swallowing to EOF.
- **Verified.** `highlight_cache.rs::fence_resets_at_frozen_turn_boundary_no_bleed`
  (a stray `` ``` `` in turn 1 → turn-2 line equals a FRESH-fence highlight, and,
  non-vacuously, turn-1's own line is still code-styled). `tests.rs::detect_block_ranges_bounds_fence_to_turn`
  (no cross-turn block; a balanced in-turn block still detected). **Both NCs
  observed RED:** unbound the detector search → block `(0,4)` bleeds; drop the fence
  reset → the turn-2 line highlights as in-fence code. Full suite 541 green.
- **Not covered.** The `hl_cache_enabled()==false` bypass path
  (`highlight_markdown_lines_syn`, YALDA_HL_CACHE off — a debug A/B path) still folds
  the fence document-wide; the default cache path is fixed. Exact painted code
  background is gap #1, but the segment style equality is asserted headlessly.
