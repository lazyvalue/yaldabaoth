# Worklog: Jump-panel workspace reorder

**Date:** 2026-08-20
**Branches touched:** `codex/jump-workspace-reorder` (`9c25e1e` feature,
`8ad6539` main-sync merge), then `main` (`b8fb3ac` merge)

## Cog execution evidence

- Graph id: `3rj`

### Initial render

```text
graph jump-workspace-reorder (frontiers)
frontier 0: spec-red-guard [open]
frontier 1: persist-order-model [open]
frontier 2: wire-drag-verify [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `7lxk` `spec-red-guard`: claimed → closed; output: specified UXI-JumpPanel-29 and
  added the production-view behavioral guard. Observed RED: actual workspace
  indices `[1, 2, 3]`, expected `[3, 1, 2]` after the reorder transition.
- `25ag` `persist-order-model`: claimed → closed; output: added durable preference
  save/load, stable presentation sorting, same-project state gating, and
  legacy-preference compatibility coverage.
- `ajvb` `wire-drag-verify`: claimed → closed; output: added the typed workspace drag
  payload and header wiring, proved the negative control, caught all seven
  scoped mutants, merged current main, passed GUI/all-target tests, built both
  release binaries, and integrated the result into main.
- `qzry` `omega`: claimed → closed; output: aggregated the shipped drag behavior,
  persistence and invariants, verification, and main integration.

### Notes

- Node `ajvb`, seq `14`, topic `deviation`: main advanced from `911e0bb` to
  `171313a` during the task and added UXI-JumpPanel-30 at the same documentation
  tail. The documentation-only conflict was resolved by retaining both UXI-29
  workspace reordering and UXI-30 working-agent indication; their guards pass
  together.
- The first copied-tree mutation baseline could not write Apple Metal's module
  cache under the sandbox. It was rerun with the required cache access. The
  first pass caught five of seven mutants; strengthening the rejected-drop
  assertion caught the remaining two on rerun.
- Repository-wide `cargo fmt --all -- --check` reports broad pre-existing
  formatting drift. The new/changed blocks were checked against rustfmt output,
  and no unrelated mechanical rewrite was retained.

### Final status

- Status: `complete`

```text
graph jump-workspace-reorder (frontiers)
frontier 0: spec-red-guard [done]
frontier 1: persist-order-model [done]
frontier 2: wire-drag-verify [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — workspace drag reorder.** A workspace identity label is a typed drag
  source and each workspace header is a drop target. Gesture and state gates
  both reject cross-project drops.
- **DONE — presentation-only semantics.** Dragging changes the jump-panel folder
  projection only. It cannot reorder `Frame::workspaces`, renumber `Ctrl-<n>`,
  move tiles, or change project ownership.
- **DONE — durable order.** Composite project/immutable-workspace keys persist
  in preferences; absent or newly discovered workspaces retain frame order.
- **DONE — gesture independence.** The disclosure chevron still folds, clicking
  the label still selects, and dragging the label previews the workspace name.

## Open / unresolved

- The GPUI mouse-drag dispatch itself remains `NEEDS-RUNTIME` under documented
  harness gap #2; the compiled typed wiring and exact state transition are
  covered headlessly. Exact pointer feel should be confirmed on next app launch.

## Decisions

- No ADR required. This is jump-panel presentation state, deliberately separate
  from operational workspace order and identity.

## Verification status

- Initial RED and restored negative control both returned `[1, 2, 3]` instead
  of `[3, 1, 2]` when the production projection sort was absent.
- Scoped mutation testing: 7/7 mutants caught after strengthening the durable
  rejected-drop state assertion.
- `cargo test --bin yalda-gpui`: 676 passed, 0 failed, 2 ignored before main
  sync; focused UXI-29, UXI-30, and preferences guards passed after main sync.
- `cargo test --all-targets --features test-support --no-fail-fast`: passed;
  only credential-dependent live tests were ignored.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-20-jump-workspace-reorder.md`: passes.

## Next

- Restart/use the rebuilt app and drag a workspace label onto another workspace
  in the same project to confirm pointer feel and drop-highlight aesthetics.
