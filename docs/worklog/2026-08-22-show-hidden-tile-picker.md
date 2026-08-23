# Workspace Show hidden-tile picker

**Date:** 2026-08-22
**Cog graph:** `srb` (show-hidden-tile-picker) — status `complete`
**Branch:** `codex/show-hidden-tile-picker` → merged to `main`

## Cog execution evidence

- Graph id: `srb`

### Initial render

```text
graph show-hidden-tile-picker (frontiers)
frontier 0: spec-show-picker [open]
frontier 1: implement-show-picker [open]
frontier 2: verify-show-picker [open]
frontier 3: omega [open] (omega)
```

### Node execution

Each node was claimed and closed with output (actor `codex`):

- `rufo` `spec-show-picker`: claimed → closed; output: documented
  `UXI-Workspace-27` and amended `UXI-Menu-8` with command availability,
  empty-state, stable-id activation, and interaction semantics.
- `o2ur` `implement-show-picker`: claimed → closed; output: added Workspace →
  Show, the context predicate, hidden-tile picker overlay, keyboard/mouse
  interaction, and typed Unhide dispatch with persistence.
- `r6o1` `verify-show-picker`: claimed → closed; output: real menu-keystroke,
  paint, empty/non-empty scope, keyboard/click activation, and disabled-context
  coverage; full GPUI suite green; changed predicate mutants caught.
- `la3q` `omega`: claimed → closed; output: specification, implementation,
  verification, mutation evidence, and worklog reconciled.

### Notes

- Node `r6o1`, seq `5`, topic `deviation`: sandboxed `cargo-mutants` could not
  write the Metal clang module cache, so it was rerun with approved escalation.
  The function filter emitted ten unrelated field-deletion mutants; the two
  behavior-changing `focused_on_active_workspace` constant mutants were both
  caught, while unrelated misses were recorded as outside this feature guard's
  scope.

### Final status

- Status: `complete`

```text
graph show-hidden-tile-picker (frontiers)
frontier 0: spec-show-picker [done]
frontier 1: implement-show-picker [done]
frontier 2: verify-show-picker [done]
frontier 3: omega [done] (omega)
```

## What shipped

- The shell menu now exposes `.` → Workspace → Show (`w`, then `s`). Show is
  dimmed and non-dispatching unless an ordinary visible tile in the active
  workspace is focused.
- Show opens a Cmd-P-style picker over only the active workspace's hidden
  stable tile ids. It has a deliberate empty state, keyboard navigation,
  hover selection, and click activation.
- Selecting a row uses the existing `unhide_window` transition, follows and
  focuses the owning workspace/tile, restores its best-effort footprint, and
  persists the updated workspace state.
- `UXI-Workspace-27` and `UXI-Menu-8` now specify and enforce the behavior.

## Verification

- `cargo check --bin yalda-gpui`: passed (existing warnings only).
- `cargo test --bin yalda-gpui`: **686 passed, 0 failed, 2 ignored**.
- `workspace_show_picker_unhides_active_hidden_tile_and_disables_outside_workspace`:
  passed through real `.` → `w` → `s` keystrokes, painted empty and populated
  picker states, Enter and click activation, cross-workspace exclusion, and the
  Detached/solo disabled state.
- `cargo test --bin yalda-gpui shell_menu -- --nocapture`: passed (3 tests).
- `cargo test --bin yalda-gpui local_menus_have_no_duplicate_keys_per_level --
  --nocapture`: passed.
- Mutation run caught both constant-return mutants for the changed
  `focused_on_active_workspace` predicate. The narrowly requested filter also
  generated ten unrelated field-deletion mutants, which survived this focused
  feature test and are recorded above.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-22-show-hidden-tile-picker.md`:
  passed.

## Open / caveats

- Exact visual resemblance to Cmd-P remains `NEEDS-RUNTIME`: the harness proves
  the card, empty state, rows, and hit targets paint, but pixel-level visual
  judgment is the documented human-eye gap.
- Repository-wide `cargo fmt --all -- --check` reports pre-existing formatting
  drift well outside this change; no bulk formatting rewrite was applied.

## Decisions

- Show remains visible but dimmed when unavailable so the Workspace submenu is
  spatially stable and the context rule is discoverable.
- Picker targets are captured as stable `WindowId`s and revalidated at commit
  time, preventing stale entries from unhiding the wrong tile.

## Next

- Human-eye check the picker against Cmd-P after restarting the rebuilt GUI.
