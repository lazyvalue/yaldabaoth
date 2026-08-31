# Worklog: Jump declutter and Cog segment completion

**Date:** 2026-08-30
**Branch:** `codex/declutter-jump-cog-completion`

## Cog execution evidence

- Graph id: `k2z`

### Initial render

```text
graph declutter-jump-and-segment-cog-completion (frontiers)
frontier 0: reconcile-contracts [open]
frontier 1: add-guards [open]
frontier 2: implement-jump [open], implement-completion [open]
frontier 3: verify-integrate [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `slp` `reconcile-contracts`: claimed → closed; output: specified `UXI-JumpPanel-32`, amended
  `UXI-AgentTile-43`, and reconciled the backlog.
- `wor` `add-guards`: claimed → closed; output: added real Jump paint and Agent compose-dispatch guards.
- `3xc` `implement-jump`: claimed → closed; output: removed the four-state segmented widget and made the
  production sidebar paint the ordinary All projection directly.
- `dn2` `implement-completion`: claimed → closed; output: made completion advance through the next `/` or
  `::` boundary, retaining `%` until the final segment.
- `bnl` `verify-integrate`: claimed → closed; output: focused/wider verification, release build, worklog,
  commit, and main integration.
- `vsz` `omega`: claimed → closed; output: final aggregate acceptance verified.

### Notes

- Graph seq 17, deviation: `cargo fmt --all -- --check` reports extensive
  pre-existing formatting drift in unrelated files; the changed diff passes
  `git diff --check` and was not bulk-reformatted.
- Graph seq 18, deviation: `cargo-mutants` is not installed; both changed
  behaviors instead have observed-RED controls on their exact production paths.

### Final status

- Status: `complete`

```text
graph declutter-jump-and-segment-cog-completion (frontiers)
frontier 0: reconcile-contracts [done]
frontier 1: add-guards [done]
frontier 2: implement-jump [done], implement-completion [done]
frontier 3: verify-integrate [done]
frontier 4: omega [done] (omega)
```

## Built

- `SHIPPED`: expanded Jump projects no longer paint Waiting / Working / All /
  Archived controls or count badges.
- `SHIPPED`: the Jump panel always shows ordinary non-archived content; Cmd-P
  and compatibility projections keep their activity ordering/filtering helpers.
- `SHIPPED`: Cog Topic acceptance walks one separator boundary per Tab/Enter,
  keeps `%` while partial, removes it on the final segment, and does not submit.

## Decisions and deviations

- Partial Topic completions retain the explicit `%` trigger. Removing it before
  the final segment would make the next Tab indistinguishable from ordinary path
  prose and prevent repeated shell-style completion.
- Archived-only browsing leaves the Jump panel with the removed widget. Durable
  archive state and archive/unarchive session actions remain intact.
- No processes were restarted; activation remains at the build boundary.

## Negative controls

- Jump paint guard failed against the prior implementation because
  `jump-agent-tabs-<project>` was present; removal returned it GREEN.
- Topic pure guard failed against the prior implementation because it inserted
  `projects/cog/mail::chat` instead of stopping at `%projects/cog/`; incremental
  acceptance returned it GREEN.

## Verification

- `cargo test --bin yalda-gpui jump_ -- --test-threads=1`: 59 passed.
- `cargo test --bin yalda-gpui topic_ -- --test-threads=1`: 9 passed.
- `cargo build --release --bin yalda-gpui`: passed.
- `git diff --check`: passed.
- `cargo fmt --all -- --check`: blocked by pre-existing repository-wide drift;
  recorded as graph deviation seq 17.
- `cargo mutants --version`: unavailable (`cargo-mutants` is not installed);
  recorded as graph deviation seq 18.
- Commit `f3d0165` fast-forwarded to `main` (worklog finalization amended it).

## Runtime boundary

- Exact native visual feel is harness gap #1 and needs the user's eye after they
  choose to restart the GUI. This task did not restart `yalda-gpui` or
  `yalda-session-server`.
