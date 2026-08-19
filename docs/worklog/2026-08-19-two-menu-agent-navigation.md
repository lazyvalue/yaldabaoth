# Worklog: Two-menu command surface and direct turn navigation

**Date:** 2026-08-19
**Branches touched:** `codex/simplify-two-menus` (`3f4a6f9`), then `main`
(`7429feb` merge)

## Cog execution evidence

- Graph id: `p9g`

### Initial render

```text
graph simplify-two-menus-direct-turn-motion (frontiers)
frontier 0: contract-menu-surface [open]
frontier 1: implement-menus-motion [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `hh2g` `contract-menu-surface`: claimed → closed; output recorded the exact
  Agent and system menu roots, direct uppercase `J`/`K` contract, and observed
  RED failures for the legacy menu surfaces and missing direct turn movement.
- `un6v` `implement-menus-motion`: claimed → closed; output recorded the two
  reduced menus, numbered model submenu, explicit Agents/Tasks views, generic
  bound-or-unbound send picker, and direct turn navigation implementation.
- `iptx` `verify-integrate`: claimed → closed; output recorded passing full GUI
  and library suites, all viable targeted mutants caught, release builds, the
  feature commit, merge, worklog validation, and the rebuilt main executable.
- `p1bw` `omega`: claimed → closed; output aggregated the shipped menus,
  navigation behavior, verification, and integration results.

### Notes

- The Agent menu intentionally keeps only worksheet/chatbox, model, session,
  clear, and view. Existing command handlers excluded from the menu remain
  available internally.
- Buffer-specific menus remain unchanged until their command design is supplied.
- Model choices use `1` through `9`, then `0`, so changing provider labels do
  not create key collisions.
- Repository-wide `cargo fmt --all -- --check` reports broad pre-existing
  formatting drift. The retained patch passes `git diff --check`; no unrelated
  formatter rewrite was retained.
- The first mutation attempts exercised an unrelated `test-support` integration
  build failure. The final run scoped Cargo Mutants to `yalda-gpui`; the
  temporary mutation configuration was removed.
- The feature worktree temporarily mirrored the main checkout's uncommitted
  `raw-window-handle` dependency solely to verify the release build. It was
  removed from the feature diff, leaving the main checkout's existing Cargo and
  scheduler-lock edits untouched.

### Final status

- Status: `complete`

```text
graph simplify-two-menus-direct-turn-motion (frontiers)
frontier 0: contract-menu-surface [done]
frontier 1: implement-menus-motion [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — Agent tile menu.** Its only roots are switch worksheet/chatbox,
  numbered switch-model choices, select session, clear, and a View submenu for
  mutually exclusive Agents and Tasks panels. The active model and active view
  are marked.
- **DONE — system menu.** Its only roots are New Tile, uppercase `X` Close Tile,
  Send Tile to Workspace, Theme, Toggle Jump Panel, System, and Workspace.
- **DONE — workspace sending.** Send Tile to Workspace always opens the same
  destination picker for bound and unbound tiles.
- **DONE — turn movement.** Uppercase `J` moves to the newer user turn and
  uppercase `K` to the older turn directly, with no command-menu mode.

## Open / unresolved

- Buffer tile command design is deliberately deferred. No matching backlog item
  was needed for this scoped change.

## Decisions

- No ADR was required; this is a command-surface simplification within the
  existing menu and workspace architecture.

## Verification status

- Observed RED: the system menu exposed ten legacy roots, used lowercase close,
  and lacked root-level send; the Agent menu exposed extra lifecycle,
  permission, send, and turn-jump commands; uppercase `J` did not move turns.
- `cargo test --bin yalda-gpui`: 634 passed, 0 failed, 2 ignored on the feature
  branch and again on merged `main`.
- `cargo test --lib`: 173 passed, 0 failed, 2 ignored on the feature branch and
  again on merged `main`.
- Targeted `cargo mutants`: 18 tested; 15 caught and 3 unviable, with every
  viable mutation caught.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed on
  the feature branch and merged `main`; both main release executables are
  rebuilt.
- `git diff --check`: passed before commit.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-two-menu-agent-navigation.md`: passes.

## Next

- Define the Buffer tile-specific command menu when its workflows are settled.
