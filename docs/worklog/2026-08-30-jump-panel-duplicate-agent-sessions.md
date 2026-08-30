# Worklog: jump-panel-duplicate-agent-sessions

**Date:** 2026-08-30
**Branches touched:** `fix-jump-session-duplicates`

## Cog execution evidence

- Graph id: `1z0`

### Initial render

```text
graph fix-jump-panel-session-duplicates (frontiers)
frontier 0: localize [open]
frontier 1: fix [open]
frontier 2: verify [open]
frontier 3: worklog [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `9px` `localize`: claimed → closed; output: a real production-path guard
  reproduced stale Attached+Detached and Detached+Detached owners sharing one
  server sid, and observed the current roster materializer leave them intact.
- `rdd` `fix`: claimed → closed; output: roster reconciliation now preserves
  the Attached/oldest canonical tile, merges tags, and retires duplicate Detached
  owners before materialization or jump projection.
- `fde` `verify`: claimed → closed; output: focused RED/green guard, full-suite
  run, diff check, formatting-baseline audit, and mutation-tool availability
  were recorded.
- `h3m` `worklog`: claimed → closed; output: bug, component, graph, and
  verification evidence captured and validated.
- `n1r` `omega`: claimed → closed; output: aggregate acceptance verified.

### Notes

- Node `9px`, seq `1`, topic `investigation`: the persisted workspace contained
  no duplicate durable ids, isolating the defect to live tile ownership rather
  than duplicate disk records.
- Verification deviation: `cargo fmt --all -- --check` reports extensive
  pre-existing formatting drift in unrelated files; `git diff --check` passes.
- Verification deviation: `cargo-mutants` is not installed on this host, so the
  changed predicate could not use the mutation runner. The mandatory manual
  negative control was observed RED on the exact production path.

### Final status

- Status: `complete`

```text
graph fix-jump-panel-session-duplicates (frontiers)
frontier 0: localize [done]
frontier 1: fix [done]
frontier 2: verify [done]
frontier 3: worklog [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

- The universal-roster reconciliation repairs duplicate stable Agent ownership
  before the jump panel sees it.
- An Attached tile wins over stale Detached copies. With only Detached owners,
  the oldest stable tile wins. Duplicate tile tags are retained on the winner.
- The renderer remains a pure ownership projection rather than concealing
  contradictory owners with row-level deduplication.
- `bug-0062` and `UXI-JumpPanel-25` document the behavior and architectural
  boundary.

## Open / unresolved

- The full GUI suite reached 758 passing tests but two pre-existing steering
  tests failed after the harness could not start its session server and fell
  back to direct spawn. One reproduced alone with the same startup timeout; the
  changed roster ownership path is not involved.
- `cargo-mutants` is unavailable on the host.
- The running GUI must be rebuilt/restarted to pick up the fix and trigger a
  fresh roster reconciliation against current live state.

## Decisions

- Repair the authoritative ownership graph at roster reconciliation. Do not
  hide duplicate rows in the jump-panel renderer.
- Never silently delete a second Attached owner during live roster refresh;
  workspace-layout conflicts remain the restore-time repair's responsibility.

## Verification status

- Negative control: the pre-fix focused guard failed at `healing duplicate
  ownership is a material roster change` before reaching projection assertions.
- Focused fixed guard: 1 passed.
- Full `cargo test --bin yalda-gpui`: 758 passed, 2 environment-sensitive
  steering failures, 1 ignored.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-30-jump-panel-duplicate-agent-sessions.md`: passed.

## Next

- Merge to `main`, rebuild Yalda, and restart the GUI so the first roster refresh
  repairs the currently duplicated Detached owners.
