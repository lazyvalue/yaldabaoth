# Worklog: empty-workspace-menu-leaders

**Date:** 2026-08-30
**Branches touched:** `fix-empty-workspace-menus`

## Cog execution evidence

- Graph id: `nod`

### Initial render

```text
graph fix-empty-workspace-menu-leaders (frontiers)
frontier 0: reproduce [open]
frontier 1: implement [open]
frontier 2: verify [open]
frontier 3: worklog [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `qo3` `reproduce`: claimed → closed; output: a real production-root/keymap
  guard failed because `.` left the menu overlay unset on an empty workspace.
- `roz` `implement`: claimed → closed; output: the empty shell root now owns
  global routing, menu origins permit no focused tile, and Space falls back to
  shell scope when no local App exists.
- `rpz` `verify`: claimed → closed; output: focused leader and workspace-nav
  guards pass; the full suite and repository hygiene checks were recorded.
- `inv` `worklog`: claimed → closed; output: bug, component, graph, decisions,
  negative control, and verification evidence captured and validated.
- `skx` `omega`: claimed → closed; output: aggregate acceptance verified.

### Notes

- Architectural decision: an empty workspace is a shell input surface, not a
  special App. Dot opens the shell menu; Space falls back to that menu because
  there is no focused App command scope.
- Verification deviation: `cargo fmt --all` exposed extensive pre-existing
  formatting drift. Its unrelated rewrite was removed; `git diff --check`
  passes and the changed Rust is formatted locally.
- Verification deviation: `cargo-mutants` is not installed. The production-path
  pre-fix RED and the intermediate Space RED provide manual negative controls.

### Final status

- Status: `complete`

```text
graph fix-empty-workspace-menu-leaders (frontiers)
frontier 0: reproduce [done]
frontier 1: implement [done]
frontier 2: verify [done]
frontier 3: worklog [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

- Empty-layout chrome installs the same shell navigation, actions, and leader
  interception expected of other focused screen roots.
- Menu overlay focus provenance supports the legitimate no-tile state while
  retaining stale-focus dismissal.
- `bug-0063`, `UXI-Menu-6`, and `UXI-Workspace-1` document the behavior.

## Open / unresolved

- The full GUI suite reached 759 passing tests but two pre-existing steering
  tests failed after the harness could not start its session server and fell
  back to direct spawn.
- `cargo-mutants` is unavailable on the host.

## Decisions

- Keep command ownership at the shell root; do not fabricate a tile or special
  App merely to receive keys.
- Space retains its universal-menu affordance by falling back to shell scope
  only when no focused App exists.

## Verification status

- Negative controls: pre-fix `.` failed with no overlay; after enabling the
  empty-root handler but before Space fallback, Space still failed.
- Focused leader guard: 1 passed.
- Workspace numeric navigation guard: 1 passed.
- Full `cargo test --bin yalda-gpui`: 759 passed, 2 environment-sensitive
  steering failures, 1 ignored.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-30-empty-workspace-menu-leaders.md`: passed.

## Next

- Merge to `main`, rebuild Yalda, and restart the GUI.
