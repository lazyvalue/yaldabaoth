# Worklog: flatten-shell-workspace-submenu

**Date:** 2026-09-27
**Branches touched:** flatten-shell-menu → merged to `main`

## Cog execution evidence

- Graph id: `zgd`

### Initial render

```text
graph flatten-shell-workspace-submenu (frontiers)
frontier 0: flatten-menu [open]
frontier 1: omega [open] (omega)
```

### Node execution

- `fli1` `flatten-menu`: claimed → closed; output: `{"main_tests":"833 passed","negative_control":"old tree FAILED 2 contract tests"}`
- omega: claimed → closed; output: `{"complete":true}`

### Notes

- None

### Final status

- Status: `complete`

```text
graph flatten-shell-workspace-submenu (frontiers)
frontier 0: flatten-menu [done]
frontier 1: omega [done] (omega)
```

## Built (with status)
- `.` shell menu: removed the `w workspace` submenu; root now `n t j s l N r x b p S \``.
  `x` close workspace, `b` back and forth, `p` new project, `S` system (moved from `s`,
  which is Show at root). Contract tests updated; RED against the old tree.

## Open / unresolved
- Close workspace is now one keystroke after `.` (was `. w x`).

## Decisions
- System submenu key `S` (root `s` is Show hidden tile).

## Verification status
- `yalda-gpui` 833 passed on `main`; release GUI built, not restarted (NEEDS-RUNTIME activation by Scott).
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-27-flatten-shell-workspace-submenu.md` passes.

## Next
- None.
