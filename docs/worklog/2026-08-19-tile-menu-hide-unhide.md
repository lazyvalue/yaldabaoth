# Worklog: Tile menu Hide and Unhide

**Date:** 2026-08-19
**Branches touched:** `codex/tile-menu-hide-unhide` (`<pending>`), then `main`
(`<pending>` merge)

## Cog execution evidence

- Graph id: `yda`

### Initial render

```text
graph tile-menu-hide-unhide (frontiers)
frontier 0: implement [open]
frontier 1: verify-integrate [open]
frontier 2: omega [open] (omega)
```

### Node execution

- `6qyj` `implement`: claimed → closed; output: all seven tile-specific menu
  builders share one Hide/Unhide suffix, applicability comes from typed tile
  membership, UXI-Menu-6/-8 were reconciled, and UXI-Menu-9 plus real-path
  guards were added.
- `0ol3` `verify-integrate`: claimed → pending close; output: focused and broad
  tests, changed-diff mutation testing, isolated commit, merge, release rebuild,
  and worklog finalization.
- `ux56` `omega`: pending.

### Notes

- Node `0ol3`, seq `11`, topic `deviation`: two initial mutation runs were
  interrupted because isolated GPUI workers rebuilt every dependency. The
  authoritative in-place changed-diff run completed with 20 mutants tested,
  12 caught, 8 unviable, and zero survivors.
- Final worklog validation occurs after omega closes because the repository
  checker requires a complete graph and omega-done evidence.

### Final status

- Status: `pending integration`

```text
graph tile-menu-hide-unhide (frontiers)
frontier 0: implement [done]
frontier 1: verify-integrate [claimed]
frontier 2: omega [open] (omega)
```

## Built (with status)

- **DONE — universal tile-menu visibility suffix.** Every App-specific Space
  menu ends with `h` Hide and `H` Unhide after a separator, sourced from one
  shared builder.
- **DONE — typed conditional enablement.** Attached-visible enables Hide and
  dims Unhide; attached-hidden enables Unhide and dims Hide; Detached dims both.
- **DONE — inert disabled commands.** Disabled visibility keys neither dispatch
  nor close the menu, using the menu system's shared disabled-command path.
- **DONE — UX contract and guards.** UXI-Menu-9 specifies the behavior; unit and
  real-keystroke harness tests cover menu shape and membership transitions.

## Open / unresolved

- Exact disabled-row pixels are verification-harness gap 1; the existing shared
  renderer maps every command in `MenuOverlay.disabled` to its dimmed palette.
  No GUI process was launched or restarted during implementation.

## Verification status

- Focused shared-suffix, exact Agent menu, duplicate-key, and real membership
  tests: passed.
- Negative control: deleting the visible-state `tile-unhide` disable made the
  real keystroke guard fail RED at the expected assertion; restoration passed.
- Deterministic GUI suite: 655 passed, 0 failed, 2 ignored, 2 pre-existing
  steering tests filtered.
- `cargo check --bin yalda-gpui --bin yalda-session-server`: passed.
- Changed-diff mutations: 20 tested, 12 caught, 8 unviable, zero survivors.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-tile-menu-hide-unhide.md`: pending omega.

## Next

- Merge to `main`, rebuild the release GUI, close omega, and validate this
  worklog.
