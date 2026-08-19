# Worklog: Jump panel and shell routing regressions

**Date:** 2026-08-19
**Branches touched:** `codex/fix-jump-tag-row-sizing` (pending), then `main`
(pending)

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

- `4wi4` `reproduce-tag-sizing`: closed with the real painted RED result:
  Unbound tag-folder height 34px versus a standard jump row at 29px.
- `6v2p` `reproduce-unbound-close`: closed with a real `close-window` command
  failure proving directly focused Unbound Buffer/Agent picker tiles remained.
- `2u35` `audit-ctrl-w-routing`: closed after localizing silent action loss to
  manually incomplete ancestry listeners spread across App roots.
- `soh8` `centralize-ctrl-w-routing`: closed after adding one generated shell
  router, deleting App/rail/arrangement duplicates, and passing the real
  `Ctrl-W h/j/k/l` App-state matrix plus registry exact-set guard.
- `6ekb` `fix-unbound-close`: closed after the model and real command guards
  proved exact Unbound removal, scratchpad pruning, focus clearing, and workspace
  reveal for Buffer and Agent pickers.
- `vzpr` `fix-tag-sizing`: closed after the production folder was pinned to
  compact fixed monospace typography and the painted zoom-invariance guard passed.
- `nu49` `verify-integrate`: claimed; integration results pending.
- `6nhn` `omega`: pending.

### Notes

- Graph note seq 7 added the directly focused Unbound close regression reported
  while the typography graph was active.
- Graph note seq 15 added the central Ctrl-W routing architecture after the
  reporter identified intermittent tile-state capture as unacceptable.
- Node `soh8` note seq 3 records the clarification that the critical failure is
  specifically `Ctrl-W` followed by a focus direction; split was not accepted
  as a test proxy.

### Final status

- Status: `open` (integration in progress)

```text
graph fix-jump-tag-row-sizing (frontiers)
frontier 0: audit-ctrl-w-routing [done], reproduce-tag-sizing [done], reproduce-unbound-close [done]
frontier 1: fix-unbound-close [done], centralize-ctrl-w-routing [done], fix-tag-sizing [done]
frontier 2: verify-integrate [claimed]
frontier 3: omega [open] (omega)
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

- None in product scope; integration verification is still running.

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
- `git diff --check`: passed. Merged-main and final worklog validation remain.

## Next

- Finish full verification, merge to `main`, rebuild release artifacts, and
  close Cog integration and omega.
