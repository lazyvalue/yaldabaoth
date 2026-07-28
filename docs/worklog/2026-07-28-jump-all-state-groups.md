# Worklog: All-tab activity groups

**Date:** 2026-07-28
**Branches touched:** `jump-all-state-groups`

## Built

- Partitioned each project's All tab into headed Working, Waiting, and
  conditional Unavailable sections.
- Kept persisted custom order stable within each activity section; activity
  changes move a row between sections without rewriting its durable slot.
- Added compact colored glyph, uppercase label, count, and hairline headings;
  empty sections render no chrome.
- Kept empty-query `Cmd-P` aligned with the All tab's grouped presentation
  order; fuzzy queries retain their normal score ordering.
- Promoted the repeated heading shape to
  `yux::compact_list_group_heading`.
- Extended and reconciled `UXI-JumpPanel-2` and `UXI-JumpPanel-14`.

## Verification

- Observed `jump_all_tab_groups_activity_with_headers` fail on the unchanged
  renderer because the nonempty Working heading was absent, then restored green
  with the real headed partition.
- `cargo check --bin yalda-gpui`: passed.
- `cargo test --bin yalda-gpui --no-fail-fast`: 494 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test --features test-support --bin yalda-gpui`: 501 passed, 1 ignored.
- The headless render guard proves section order, row order within each section,
  conditional Unavailable placement, empty-section omission, and matching
  empty-query `Cmd-P` order.

## Open / unresolved

- Exact heading density and color balance remain runtime harness gap #1.

## Decisions

- No ADR: this extends the existing All-tab presentation contract. Unavailable
  is an exceptional conditional section, not another selectable activity tab.

## Next

- Human-check the new headings in Folio and Nightfox with both short and long
  session lists.
