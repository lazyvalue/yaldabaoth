# Worklog: Waiting and Working tab counts

**Date:** 2026-07-28
**Branches touched:** `jump-tab-counts`

## Built

- Added an always-visible number indicator to each project's Waiting and
  Working tabs, including `0`.
- Derived both totals from the same deduplicated project projection as the tab
  contents, excluding archived and unavailable sessions.
- Kept All and Archived unnumbered and preserved the neutral gray selected-tab
  treatment.
- Added compact green/orange count pills through the reusable
  `yux::compact_count_indicator` primitive.
- Added and reconciled `UXI-JumpPanel-17`.

## Verification

- Observed `jump_waiting_working_tabs_paint_live_counts` fail on the unchanged
  renderer because the Waiting total was absent, then return green after the
  implementation.
- The guard proves exact derived totals, archive/unavailable exclusion, a
  Working→Waiting transition, persistent zero-state paint, and containment
  inside the matching tab targets.
- `cargo build`: passed.
- `cargo test --bin yalda-gpui`: 495 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test`: all repository test targets passed.
- `git diff --check`: passed.

## Open / unresolved

- Human-check the pill scale and green/orange balance in Folio and Nightfox
  (runtime harness gap #1).

## Decisions

- No ADR: this is a presentation extension of the existing live-tab projection.
  Counts are derived view data, not separately persisted state.

## Next

- Review the tab strip with both zero and multi-digit totals in Folio and
  Nightfox.
