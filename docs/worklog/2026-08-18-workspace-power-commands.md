# Worklog: workspace power commands

**Date:** 2026-08-18
**Branches touched:** `codex/workspace-power-commands` (`98b5712`) — implementation; `main` (`82d5261`) — merge

## Cog execution evidence

- Graph id: `dgn`

### Initial render

```text
graph workspace-power-commands (frontiers)
frontier 0: contract [open]
frontier 1: core [open]
frontier 2: commands [open]
frontier 3: verify [open]
frontier 4: integrate [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `gk5v` `contract`: claimed → closed; output: UXI-Workspace-17..20 and the durable project ticket fix command semantics, ownership, persistence, and keys.
- `kzef` `core`: claimed → closed; output: stable same-project relocation, persisted Unbound-backed scratchpad MRU, stable-name workspace history, and clamped Columns controls; 38 model tests green.
- `reaf` `commands`: claimed → closed; output: production actions, menus, picker, keymap, central wiring, and Columns master/stack rendering.
- `jo7t` `verify`: claimed → closed; output: 623 GPUI tests and 173 library tests passed (2 intentional ignores each), both required binaries built, five observed-RED controls restored, and all viable changed-method mutants caught.
- `r08q` `integrate`: claimed → closed; output: UX contract, README, ticket, and worklog reconciled; feature commit merged to `main`; all three focused production-path guards passed on merged main.
- `kui8` `omega`: claimed → closed; output: all four command families specified, implemented, persistence-safe, verified, documented, committed, and merged.

### Notes

- Graph, seq `18`, topic `deviation`: scratchpad uses `Ctrl-W d/D` because existing Vim-compatible `Ctrl-W s` remains horizontal split.
- Verification output: the surviving `source < target` → `source <= target` mutant is equivalent because `source == target` is rejected before that comparison. Three unrelated `DesktopState::restored` field-deletion mutants bypassed cargo-mutants' function filter.

### Final status

- Status: `complete`

```text
graph workspace-power-commands (frontiers)
frontier 0: contract [done]
frontier 1: core [done]
frontier 2: commands [done]
frontier 3: verify [done]
frontier 4: integrate [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- Added send-without-follow and send-and-follow through the existing same-project workspace picker.
- Added a persisted scratchpad MRU over ordinary Unbound tiles and global stash/summon commands.
- Added stable previous-workspace toggling with per-workspace focus restoration.
- Activated persisted Columns master ratio/count geometry and added four clamped controls.
- Merged feature commit `98b5712` to `main` in merge commit `82d5261`.

## Open / unresolved

- No feature work remains. Existing compiler warnings and the three unrelated restore-field mutation survivors are unchanged baseline issues.

## Decisions

- No new ADR. UXI-Workspace-17..20 record the feature contract; scratchpad reuses Unbound instead of adding another ownership domain.

## Verification status

- `cargo test --bin yalda-gpui`: 623 passed, 2 ignored.
- `cargo test --lib`: 173 passed, 2 ignored.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed.
- Real GPUI key/action/picker/paint guards passed on the feature branch; the three focused production-path guards passed again on merged `main`.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-18-workspace-power-commands.md` passes.

## Next

- Restart the development GUI to use the newly built command surface.
