# Worklog: Jump tabs and summary polish

**Date:** 2026-07-28
**Branches touched:** `jump-tabs-summary-polish`

## Built

- Made every connected non-working agent a Waiting-tab member; unread remains a
  stronger attention treatment rather than a separate activity state.
- Rebuilt the three tabs as a bounded cool-accent segmented control.
- Replaced gold/low-contrast supporting copy with readable cool prose colors in
  Folio and Nightfox.
- Shortened autoname summaries to topic/goal only, made the first user turn
  produce an immediate excerpt, and added visible in-flight feedback, an
  eight-second request bound, and a persisted opening-topic fallback.

## Verification

- Observed RED with an idle/read agent omitted from Waiting, then restored the
  operational two-state filter.
- Observed RED with missing credentials leaving autoname state `Requested`, then
  restored fallback settlement.
- Focused state, paint-containment, palette, prompt/fallback, and persistence
  tests pass.
- `cargo check --bin yalda-gpui` passes.
- `cargo test --bin yalda-gpui --no-fail-fast` passes: 490 passed, 1 ignored.
- `cargo test --lib` passes: 160 passed, 2 ignored.
- `cargo test --features test-support --bin yalda-gpui` passes: 497 passed,
  1 ignored.

## Open / unresolved

- Exact pixel colors and subjective visual balance remain runtime harness gap #1.
- The live naming HTTP request remains network verification gap #2; its timeout,
  parser, fallback, and settlement paths are headless-covered.

## Next

- None for this revision.
