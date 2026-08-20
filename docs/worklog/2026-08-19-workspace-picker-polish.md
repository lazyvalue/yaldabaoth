# Worklog: Workspace destination picker polish

**Date:** 2026-08-19
**Branches touched:** `codex/workspace-picker-polish` (`5ed9168`, `d27367d`,
`ae1f577`), then `main` (`983a774` merge)

## Cog execution evidence

- Graph id: `z83`

### Initial render

```text
graph polish-send-workspace-picker (frontiers)
frontier 0: diagnose-contract [open]
frontier 1: implement-picker [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `iscu` `diagnose-contract`: claimed → closed; output: bug-0048 and
  UXI-Workspace-25 define stable workspace-name destination rows, and the real
  Ctrl-W picker guard reproduced `Claude (Research)`.
- `mclx` `implement-picker`: claimed → closed; output: destination labels use
  `Workspace::display_label`; the selector is a compact yux-built card with
  structured header, selected/current/create states, fixed chrome, scrolling,
  and click dispatch.
- `j30p` `verify-integrate`: claimed → closed; output: focused and broad
  verification, mutation proof, branch integration, release build, and worklog
  completed while preserving the pre-existing user changes.
- `ehsd` `omega`: claimed → closed; output: identity, aesthetics, interaction
  parity, regression coverage, documentation, integration, and release build
  are complete.

### Notes

- No Cog deviation or failure notes were recorded.

### Final status

- Status: `complete`

```text
graph polish-send-workspace-picker (frontiers)
frontier 0: diagnose-contract [done]
frontier 1: implement-picker [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — stable destination names.** Picker rows identify only the workspace;
  focused Agent, Browser, document, or provider state cannot rewrite the label.
- **DONE — compact visual hierarchy.** A responsive 480px centered card uses a
  title and follow-policy subtitle, standard system-font labels, accent-rail
  selection, Current/Create badges, separated creation action, and secondary
  key hints.
- **DONE — reusable picker row.** `yux::picker_option_row` owns the common row
  geometry, typography, badge, selection, and hover styling.
- **DONE — mouse and keyboard parity.** Existing `j`/`k`, arrows, `g`/`G`,
  Enter/`l`, and Esc/`q` behavior is unchanged; existing and new-workspace rows
  are now clickable.

## Open / unresolved

- Pixel-color/golden-image comparison remains a general verification-harness
  gap. The real headless paint path verifies card/row geometry and mouse hit
  dispatch. No GUI process was launched or restarted during implementation.

## Verification status

- Initial negative control: focused identity guard failed with
  `Claude (Research)` before the fix.
- Focused picker guards: 2 passed, 0 failed.
- Painted guard: compact card bounds, header/list order, fixed 42px rows,
  separated creation action, and real selected-row click all passed.
- Required mutation: restoring Agent-derived `Claude (<workspace>)` made the
  identity guard fail at the reported symptom; restoration returned green.
- Deterministic GUI suite: 659 passed, 0 failed, 2 ignored.
- `cargo check --bin yalda-gpui`: passed.
- `cargo build --release --bin yalda-gpui`: passed after merge.
- `git diff --check`: passed.
- Post-merge focused picker guards: 2 passed, 0 failed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-workspace-picker-polish.md`: passed after omega.

## Next

- Restart the GUI to load the rebuilt release executable.
