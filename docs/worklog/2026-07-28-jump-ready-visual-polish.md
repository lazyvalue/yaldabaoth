# Worklog: Jump ready-state visual polish

**Date:** 2026-07-28
**Branch:** `jump-ready-visual-polish`

## Built

- Unified Waiting admission and presentation: every connected non-working agent
  now carries the green `your turn` treatment, regardless of unread state.
- Replaced status chips with subtle background washes: orange for Working and
  green for ready input, without outlines, rounded boxes, or italic labels.
- Reserved the overlay's neutral gray palette for selected rows and tabs.
- Increased the workspace-to-tab gap to 10px while retaining the segmented
  control boundary and internal dividers.

## Verification

- Observed RED on the real per-project Waiting projection: the read idle row
  produced no `your turn` hint while the unread row did.
- The real projection now asserts that every Waiting row carries the hint and
  that the tabs paint at least 8px below the final workspace row.
- Pure palette coverage guards orange Working, green ready input, and neutral
  gray selection.
- `cargo test --bin yalda-gpui --no-fail-fast`: 491 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test --features test-support --bin yalda-gpui`: 498 passed, 1 ignored.

## Runtime note

- Exact wash alpha and subjective density remain visual verification gap #1.
