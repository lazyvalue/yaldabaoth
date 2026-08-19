# Worklog: Jump panel and shell routing regressions

**Date:** 2026-08-19
**Branches touched:** `codex/fix-jump-tag-row-sizing` (`f3face0`), then `main`
(`75fffa5` merge)

## Cog execution evidence

- Graph id: `3qs`

### Initial render

```text
graph fix-jump-tag-row-sizing (frontiers)
frontier 0: reproduce-tag-sizing [open]
frontier 1: fix-tag-sizing [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `4wi4` `reproduce-tag-sizing`: claimed → closed; output: real painted RED:
  Unbound tag-folder height 34px versus a standard jump row at 29px.
- `6v2p` `reproduce-unbound-close`: claimed → closed; output: a real `close-window` command
  failure proving directly focused Unbound Buffer/Agent picker tiles remained.
- `2u35` `audit-ctrl-w-routing`: claimed → closed; output: silent action loss localized to
  manually incomplete ancestry listeners spread across App roots.
- `soh8` `centralize-ctrl-w-routing`: claimed → closed; output: one generated shell
  router, deleting App/rail/arrangement duplicates, and passing the real
  `Ctrl-W h/j/k/l` App-state matrix plus registry exact-set guard.
- `6ekb` `fix-unbound-close`: claimed → closed; output: model and real command guards
  proved exact Unbound removal, scratchpad pruning, focus clearing, and workspace
  reveal for Buffer and Agent pickers.
- `vzpr` `fix-tag-sizing`: claimed → closed; output: the production folder was pinned to
  compact fixed monospace typography and the painted zoom-invariance guard passed.
- `nu49` `verify-integrate`: claimed → closed; output: full feature/main tests, targeted
  mutation testing, release builds, commit `f3face0`, and merge `75fffa5` passed
  while unrelated main edits remained intact.
- `6nhn` `omega`: claimed → closed; output: aggregate shipped/verified result.

### Notes

- Graph note seq 7 added the directly focused Unbound close regression reported
  while the typography graph was active.
- Graph note seq 15 added the central Ctrl-W routing architecture after the
  reporter identified intermittent tile-state capture as unacceptable.
- Node `soh8` note seq 3 records the clarification that the critical failure is
  specifically `Ctrl-W` followed by a focus direction; split was not accepted
  as a test proxy.

### Final status

- Status: `complete`

```text
graph fix-jump-tag-row-sizing (frontiers)
frontier 0: audit-ctrl-w-routing [done], reproduce-tag-sizing [done], reproduce-unbound-close [done]
frontier 1: fix-unbound-close [done], centralize-ctrl-w-routing [done], fix-tag-sizing [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — directional focus routing.** `Ctrl-W h/j/k/l` is owned by one shell
  ancestor for bound and directly focused Unbound tile surfaces. App renderers
  no longer duplicate or selectively omit shell workspace actions.
- **DONE — Unbound Close Tile.** Directly focused Unbound Buffer and Agent
  picker tiles are removable through the system command; stable identity,
  scratchpad, and workspace-floor invariants are enforced.
- **DONE — tagged jump typography.** Unbound tag folders use fixed compact
  monospace chrome; tagged and loose child rows share the ordinary fixed row.

## Open / unresolved

- None for the reported regressions. Existing compiler warnings and ignored
  environment/runtime tests predate this change.

## Decisions

- No ADR added. The existing shell/App ownership boundary already determines
  the design: global workspace commands belong above App renderers.

## Verification status

- Observed RED: removing the central common-ancestor router made real
  `Ctrl-W h` leave focus on the center Buffer picker.
- Observed RED: removing `FocusRight` from the generated router failed the exact
  registry coverage guard.
- Observed RED: the pre-fix close command left the Unbound Buffer picker live;
  reversing the fixed scratchpad predicate left the stale id and failed.
- Observed RED: the pre-fix tag folder painted at 34px versus a 29px standard row.
- `cargo test --bin yalda-gpui`: 639 passed, 0 failed, 2 ignored on the feature
  branch.
- `cargo test --lib`: 173 passed, 0 failed, 2 ignored on the feature branch.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed on
  the feature branch.
- Targeted `cargo mutants --in-diff`: 7 mutants tested, 6 caught and 1 unviable,
  with no survivors.
- On merged `main`, `cargo test --bin yalda-gpui` again passed 639 tests with 2
  ignored, and `cargo test --lib` again passed 173 with 2 ignored.
- On merged `main`, both release binaries rebuilt successfully.
- `git diff --check`: passed before commit. Repository-wide `cargo fmt --check`
  still reports broad pre-existing formatting drift, so no unrelated rewrite was
  retained.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-jump-panel-shell-routing-regressions.md`: passes.

## Next

- Restart the GUI to load the rebuilt release executable.
