# Restore command menus in an empty workspace

**Date:** 2026-08-29
**Cog graph:** `vfl` (fix-empty-workspace-command-menus) — status `complete`
**Branch:** `fix-empty-workspace-command-menus` → merged to `main`

## Cog execution evidence

- Graph id: `vfl`

### Initial render

```text
graph fix-empty-workspace-command-menus (frontiers)
frontier 0: reproduce-contract [open]
frontier 1: implement-fix [open]
frontier 2: verify-fix [open]
frontier 3: record-integrate [open]
frontier 4: omega [open] (omega)
```

### Node execution

Each node was claimed and closed with output (actor `codex`):

- `rlt9` `reproduce-contract`: claimed → closed; output: documented the empty
  shell-surface contract and observed the production close/paint/keystroke guard
  fail before the fix.
- `g5nm` `implement-fix`: claimed → closed; output: made the empty surface own
  focus and key routing, represented a no-tile menu origin, and added the
  `space`-to-shell fallback.
- `8y2m` `verify-fix`: claimed → closed; output: targeted RED/GREEN guard, full
  GUI and library suites, both runtime binary builds, clean diff check, and all
  six changed-line mutants accounted for.
- `joji` `record-integrate`: claimed → closed; output: reconciled the component
  contracts and bug records, recorded the worklog, committed the scoped branch,
  merged it to main without disturbing existing user changes, and reran the
  integrated guard.
- `vjzo` `omega`: claimed → closed; output: implementation, specifications,
  verification, mutation evidence, bug record, worklog, and main integration
  reconciled.

### Notes

- The chosen empty-state behavior is deliberate: without a focused App,
  `space` opens the shell menu while retaining `space` as the displayed leader;
  `.` continues to open the same shell menu with its normal trail.
- The first targeted command mistakenly used `--exact` and selected zero tests;
  it was immediately corrected to the uniquely named filter before taking RED
  or GREEN evidence.
- A repository-wide formatter check exposed pre-existing drift. An accidental
  formatter spill was fully restored, the intended patch was reapplied, and
  only scoped changes remain with `git diff --check` clean.
- The first sandboxed mutation baseline could not write Clang's Metal module
  cache. The escalated baseline then exposed one unrelated guard that requires
  `.git`, which cargo-mutants omits from its temporary copy. The final invocation
  skipped only that guard and tested six changed-line mutants: five caught, one
  rejected by the compiler, zero survivors.
- No backlog item was added: the reported defect is fixed and its durable state
  is recorded in bug-0060 and the amended component contracts.

### Final status

- Status: `complete`

```text
graph fix-empty-workspace-command-menus (frontiers)
frontier 0: reproduce-contract [done]
frontier 1: implement-fix [done]
frontier 2: verify-fix [done]
frontier 3: record-integrate [done]
frontier 4: omega [done] (omega)
```

## What shipped

- The valid empty-workspace view is now a focused shell input root with raw
  leader interception and the global action listeners normally inherited from
  a visible tile root.
- `MenuOverlay` supports a legitimate no-tile origin, preserving stale-menu
  dismissal when focus changes without inventing a placeholder `WindowId`.
- On an empty workspace, `.` opens the shell menu and `space` falls back to that
  menu. On a focused App, `space` retains its existing tile-local behavior.
- `UXI-Workspace-1`, `UXI-Menu-6`, and bug-0060 now specify and guard the edge.

## Verification

- Negative control: before the fix,
  `empty_workspace_keeps_both_command_leaders_live` failed because a real
  `space` keystroke opened no overlay after closing the sole tile.
- Targeted guard after the fix: passed through the production close command,
  painted empty root, shared-focus assertion, and real `space`/`.` keystrokes.
- `cargo test --bin yalda-gpui`: **758 passed, 0 failed, 2 ignored**.
- `cargo test --lib`: **213 passed, 0 failed, 2 ignored**.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed (existing
  warnings only).
- Changed-line mutation gate: **6 tested; 5 caught, 1 unviable, 0 missed**.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-29-empty-workspace-command-menus.md`: passed.

## Open / caveats

- Repository-wide `cargo fmt --all -- --check` remains red on unrelated existing
  files; no broad formatting changes were retained.
- The debug runtime binary was rebuilt. A running GUI must be restarted before
  it can use the new binary.
