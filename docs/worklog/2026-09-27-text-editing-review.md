# Worklog: text-editing-review

**Date:** 2026-09-27 · **Graph:** `exa` · **Branch:** `text-edit-review` → `main` (`074b166`)

## Shipped
- Review of every text-input surface: 54 findings with stable IDs in
  `docs/research/2026-09-27-text-editing-review.md`.
- P1: one shared `LineInput` (`src/line_input.rs`, `yux/line_input.rs`) for all 14
  single-line fields; `UXI-TextEditing-5`. Guard:
  `line_input_overlay_rejects_chords_and_edits_at_caret` (NC: `"/tmp/ws/projyy"`).
- P2 engine: B1, B3–B7, B10, B11, B13, B15, B16, D1. P3 render: C1, C2, C4, C5, D2, D5,
  D6. P4 compose: D3, D4, D7–D10. P5 clipboard/diff: C6, C7, E1, E2. Each bugfix has
  a guard observed RED without the fix (names in the review's Outcome table).
- `cargo test` green on `main` `074b166` (1410 passed).

## Caveats
- NEEDS-RUNTIME, gap 1 (pixels): caret and selection fonts. The Restart-path draft
  save ordering is untested because it spawns a real process.
- The deferred items were then fixed in graph `ls2`
  ([worklog](2026-09-27-text-editing-deferred-fixes.md)).

## Decisions
- The `LineInput` model lives in the lib crate, because the lib file browser
  shares it. Alt types only OS-composed characters.

## Cog
- Status: `complete`

```text
graph text-editing-code-review (frontiers)
frontier 0: survey [done]
frontier 1: p5-clip-diff [done], line-input [done], p4-compose [done], p3-render [done], p2-engine [done]
frontier 2: verify [done]
frontier 3: report [done]
frontier 4: omega [done] (omega)
```
