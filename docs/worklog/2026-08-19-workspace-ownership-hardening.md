# Worklog: Workspace ownership hardening

**Date:** 2026-08-19
**Branches touched:** `codex/fix-close-agent-unbound` (`1a0d12f`), then `main`
(`e462d81` merge)

## Cog execution evidence

- Graph id: `90f`

### Initial render

```text
graph fix-close-agent-unbound (frontiers)
frontier 0: reproduce-contract [open]
frontier 1: audit-core [open], audit-surfaces [open], audit-persist [open]
frontier 2: typed-core [open]
frontier 3: fix-close-path [open], fix-project-session [open]
frontier 4: sequence-guards [open]
frontier 5: verify-ship [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `jlh0` `reproduce-contract`: claimed → closed; output: production
  `close-window` observed RED by dropping a bound Agent tile entirely, and the
  pre-attach roster race observed two stable owners for one durable session.
- `3627` `audit-core`: claimed → closed; output: inventoried constructors,
  placement transitions, mutable identity fields, duplicate WindowId states,
  stale direct-focus/scratchpad indices, and split project ownership.
- `ortc` `audit-surfaces`: claimed → closed; output: traced Close Tile,
  Stash/Summon, Send/Bind, picker, roster, Cmd-P, and jump-panel paths and
  identified duplicated close semantics and the missing bind reconciliation.
- `z2ew` `audit-persist`: claimed → closed; output: traced snapshot/restore and
  reproduced file-order retention of cross-project duplicate Agent identities.
- `nnc4` `typed-core`: claimed → closed; output: made stable Window id/project
  private and immutable, removed the parallel Unbound project field, and added
  validated restore and ownership boundaries.
- `pc08` `fix-close-path`: claimed → closed; output: keyboard and menu now use
  one exhaustive `CloseTileOutcome`; bound Agent close stashes the exact tile,
  including the sole-workspace replacement branch.
- `w8kg` `fix-project-session`: claimed → closed; output: the bind choke retires
  roster-race duplicates, merges tags, and restore selects the canonical tile
  by authoritative session cwd/project before rewriting healed state.
- `j0gn` `sequence-guards`: claimed → closed; output: operation sequences,
  negative invariant guards, 656 GUI tests, and 41 focused mutations with 36
  caught, 5 compile-invalid, and no survivors.
- `z4y4` `verify-ship`: claimed → closed; output: full GUI/library/all-target
  tests, feature and merged release builds, feature commit `1a0d12f`, and merge
  `e462d81` passed without staging the pre-existing Cargo or scheduled-task edits.
- `z62o` `omega`: claimed → closed; output: aggregate lifecycle, persistence,
  test, mutation, integration, and rebuild evidence complete.

### Notes

- Node `j0gn`, seq `2`, topic `deviation`: mutation baseline exposed three stale
  `AgentSpawner::spawn` test calls; the test-only callers now pass the required
  provider so the all-target suite is executable again.
- Node `j0gn`, seq `4`, topic `decision`: stable Window id/project are private,
  Unbound derives project from its Window, and persisted Agent canonicalization
  keys the concrete occurrence rather than a possibly duplicated WindowId.
- The final worklog validation occurs immediately after omega closes because the
  repository checker requires both graph status `complete` and an omega-done
  frontier; no final state is fabricated in advance.

### Final status

- Status: `complete`

```text
graph fix-close-agent-unbound (frontiers)
frontier 0: reproduce-contract [done]
frontier 1: audit-core [done], audit-surfaces [done], audit-persist [done]
frontier 2: typed-core [done]
frontier 3: fix-close-path [done], fix-project-session [done]
frontier 4: sequence-guards [done]
frontier 5: verify-ship [done]
frontier 6: omega [done] (omega)
```

## Built (with status)

- **DONE — immutable stable tile identity.** `WindowId` and `ProjectId` cannot be
  rewritten by callers; project travels with the complete tile through every
  bound/Unbound transition.
- **DONE — exclusive ownership gates.** Duplicate ids, cross-project workspace
  leaves, invalid direct Unbound focus, stale scratchpad entries, and duplicate
  local/durable Agent identities are rejected before persistence. Invalid
  snapshots cannot become live state.
- **DONE — Agent close semantics.** Closing a bound Agent is equivalent to
  Stash: the same tile/session/tags/project moves immediately to Unbound and
  scratchpad. Closing an already-Unbound empty picker remains destructive.
- **DONE — duplicate-session repair.** Bind-time races are reconciled into the
  live canonical tile; existing persisted duplicates self-heal deterministically
  using authoritative session cwd/project and are rewritten on startup.
- **DONE — suite repair.** Stale fake-transport calls were updated for the typed
  provider argument, restoring repository-wide all-target verification.

## Open / unresolved

- No known workspace ownership regression remains. Existing compiler warnings
  and environment-backed ignored live tests predate this change.

## Decisions

- No new ADR. This implements ADR-0033's exclusive placement model by putting
  stable identity on the tile and enforcing it at mutation/persistence borders.

## Verification status

- Observed RED: bound Agent `close-window` produced no membership instead of
  `TileMembership::Unbound`; green after the shared typed close transition.
- Observed RED: the provisional-create/roster/bind race produced two stable
  owners for one server SID; green after bind reconciliation.
- `cargo test --features test-support --bin yalda-gpui`: 654 passed, 0 failed,
  2 ignored on feature and merged main.
- `cargo test --lib`: 173 passed, 0 failed, 2 ignored on feature and merged main.
- `cargo test --all-targets --features test-support --no-fail-fast`: passed all
  non-live targets; three credential/network-backed live tests remained ignored.
- Focused `cargo mutants --in-diff`: 41 tested, 36 caught, 5 unviable, zero
  survivors.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed on
  feature and merged main.
- `git diff --check`: passed before commit.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-workspace-ownership-hardening.md`: passes.

## Next

- Restart the GUI to load the rebuilt release executable. The first startup
  automatically repairs and rewrites any persisted duplicate Agent ownership.
