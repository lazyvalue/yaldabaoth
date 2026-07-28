# Worklog: Jump-panel agent state tabs

**Date:** 2026-07-28
**Branches touched:** `jump-agent-state-tabs`

## Built

- Added independent Waiting, Working, and All agent tabs below every expanded
  project's workspace list.
- Waiting and Working filter by live agent state and sort by state-entry time,
  oldest first; All preserves durable custom order and appends new sessions.
- Added reusable compact tab chrome in `yux`.
- Captured and reconciled the behavior as `UXI-JumpPanel-14`.

## Verification

- Observed the chronology guard fail after temporarily disabling the state-time
  sort, then restored the implementation.
- `cargo check --bin yalda-gpui`: passed.
- `cargo test --bin yalda-gpui --no-fail-fast`: 485 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test --features test-support --bin yalda-gpui`: 492 passed, 1 ignored.
- Headless GPUI coverage exercises independent project selection, painted tab
  presence, filtering, state transitions, stable All order, and append behavior.

## Open / unresolved

- Exact visual color judgment remains runtime harness gap #1.
- The repository-wide `scripts/ci.sh` feature-test step is currently blocked by
  three unrelated, pre-existing `Spawner::spawn` call sites in
  `tests/agent_transport_fake_test.rs` that lack the new provider argument.

## Next

- None for this UX change.
