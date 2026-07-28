# Worklog: Archived agent sessions

**Date:** 2026-07-28
**Branches touched:** `jump-session-archive`

## Built

- Added a durable sid-keyed archive flag orthogonal to live Waiting/Working
  activity, with preference persistence and `/clear` identity succession.
- Added a fourth Archived project tab. Archived rows are absent from Waiting,
  Working, All, and `Cmd-P`; unarchiving restores the existing custom All slot
  and current live-state projection.
- Added contextual archive/unarchive to the focused agent's `<space>` menu and
  to a cursor-anchored session-row right-click menu.
- Promoted the repeated popup row chrome to `yux::context_menu_item`.
- Captured and reconciled the behavior as `UXI-JumpPanel-16`.

## Verification

- Observed the archive projection guard fail after temporarily removing the All
  filter, then restored the implementation.
- `cargo check --bin yalda-gpui`: passed.
- `cargo test --bin yalda-gpui --no-fail-fast`: 493 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test --features test-support --bin yalda-gpui`: 500 passed, 1 ignored.
- Headless GPUI coverage drives all four projections, preference persistence,
  `Cmd-P`, the dynamic local command, actual right-click/click events on painted
  rows and popup actions, sid-less refusal, and `/clear` succession.

## Open / unresolved

- Exact visual balance of the four-tab strip and context popup remains runtime
  harness gap #1.

## Decisions

- No ADR: archive is a presentation/persistence flag on the existing stable
  session identity, not a new operational activity state.

## Next

- Human-check the four-tab density and cursor-popup placement in Folio and
  Nightfox.
