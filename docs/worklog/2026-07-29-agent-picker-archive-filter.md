# Worklog: Agent picker archive filter

**Date:** 2026-07-29

## Built

- Captured `UXI-AgentTile-32`: archived sessions never appear in an unbound
  Agent Tile's session picker.
- Filtered the shared `picker_projection` before its free/bound partition, so an
  archived session is absent whether selectable or already in use.
- Kept both create-new rows, project scoping, 1:1 binding, and archive surfaces
  elsewhere unchanged.

## Verification

- Added
  `agent_tile_picker_excludes_free_and_bound_archived_sessions`, which builds two
  bound tiles and a third focused picker, then proves archived free and bound
  identities are absent while live equivalents remain.
- Observed the guard fail against the unchanged projection when
  `S-free-archived` appeared in the selectable list.
- `cargo test --features test-support --bin yalda-gpui --no-fail-fast`: 513
  passed, 1 ignored.
- `cargo test --lib --no-fail-fast`: 161 passed, 2 ignored.
- `cargo check --bins`: passed.

## Deviation

- None.
