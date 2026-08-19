# 001 — Four workspace command families

**Status:** complete
**Depends on:** ADR-0033, UXI-Workspace-17..20

## Subtasks

- [x] Evolve the move picker into send-without-follow and send-and-follow with
      same-project destinations; keep also-show menu-only.
- [x] Add persisted Unbound-backed scratchpad MRU membership and stash/summon
      commands.
- [x] Add stable previous-workspace history and back-and-forth navigation.
- [x] Activate persisted master ratio/count in Columns and add clamped controls.
- [x] Add real key/menu/picker/paint tests, observed-RED controls, mutation
      coverage, full builds, documentation, and integration.

## Non-goals

- Cross-project tile moves.
- Floating window geometry or a second scratchpad tile store.
- Replacing `Ctrl-W s` horizontal split.
- Reintroducing master-stack as a third workspace layout mode.
