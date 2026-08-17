# bug-0040: worksheet-reply-wraps-narrow-first-paint

**Status:** FIXED
**First seen:** 2026-08-17
**Component:** docs/components/agent-tile/compose (UXI-AgentTile-9)

## Symptom

Immediately after pressing `r` to quote a long agent sentence, the inline
worksheet reply sometimes wrapped into a narrow column even though the agent
tile was wide. The quote looked like a roughly 40-character-wide strip until a
later edit or unrelated repaint corrected it.

## Root cause

The active `FlatItem::YouBlock` derives its wrapping columns from the compose
surface's `CaptureBounds` cell. A newly opened reply necessarily renders before
that block has painted, so the cell is still zero and the renderer used a
hard-coded 40-column fallback. `CaptureBounds` records geometry during paint but
does not schedule another render (and notifying during render is forbidden), so
the cached transcript could keep that first narrow layout until another event
invalidated the You-block.

The italic type in the screenshot was not the defect: `> ` reply quotations are
intentionally italic on every surface under UXI-Blockquote-1. The incorrect
40-column wrapping was the formatting failure.

## Fix

`inline_you_block_wrap_cols` now preserves the exact compose measurement when it
exists, but derives the first-paint width from the already-painted transcript
viewport (minus conservative inline chrome) when it does not. The 40-column
value remains only an emergency fallback before either surface has layout.

This avoids render-time notification, keeps cached-view ownership intact, and
uses the same resolved width for both You-block rendering and caret reveal math.

## Verification

- `verify_harness.rs::worksheet_r_first_paint_uses_transcript_width` drives the
  real `handle_claude_key(r)` path, begins the layout probe before opening the
  reply, and checks its first settled paint. With the fix removed, a 1574px
  transcript produced a 142px-tall reply block and failed the `<115px` guard.
- `tests.rs::inline_you_block_wrap_width_prefers_measurement_then_viewport`
  constrains exact-measurement precedence, viewport derivation, the pre-layout
  emergency fallback, and minimum progress.
- Targeted `cargo mutants` over `inline_you_block_wrap_cols`: 12/12 mutants
  caught. The first viable run caught 11 and exposed the exact `> 1.0` boundary;
  adding the one-pixel sentinel assertion caught the remaining mutant.

## Log

### 2026-08-17 — localized and fixed

The screenshot's line breaks matched the literal `40` fallback in
`transcript_view.rs`. A first attempt at probing after `probe_dirty` false-passed
because that helper deliberately invalidates the cached transcript multiple
times, allowing `CaptureBounds` to populate before the assertion. Moving the
probe start before the real `r` dispatch captured the faulty first paint and
made the guard RED for the reported reason. The implementation then replaced
only the unmeasured branch with transcript-viewport-derived columns.

The first mutation run was sandbox-unviable because GPUI's Metal build tried to
write clang's module cache outside the workspace. Re-running with the required
filesystem permission produced viable results (11 caught / 1 missed); the missed
boundary mutant led to the additional one-pixel assertion, after which the
targeted rerun caught both `>=` mutants and the helper reached 12/12 coverage.
