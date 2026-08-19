# Worklog: unbound-tile-workspaces

**Date:** 2026-08-18
**Branches touched:** `codex/unbound-tile-workspaces` (feature commit), `main`

## Cog execution evidence

- Graph id: `9k2`

### Initial render

```text
graph unbound-tile-workspaces (frontiers)
frontier 0: contract-architecture [open]
frontier 1: unbound-core [open]
frontier 2: persistence-migration [open]
frontier 3: direct-access [open]
frontier 4: jump-panel-tree [open]
frontier 5: real-path-verification [open]
frontier 6: document-integrate [open]
frontier 7: omega [open] (omega)
```

### Node execution

- `4edc` `contract-architecture`: claimed → closed; output: `{"decision":"ADR-0033","model":"each stable tile is exclusively bound or unbound; Cmd-P is the p-menu"}`.
- `5enx` `unbound-core`: claimed → closed; output: `{"model":"Frame.unbound_tiles + direct_unbound","invariants":"stable id/state/tags; same-project bind; workspace close unbinds"}`.
- `rczp` `persistence-migration`: claimed → closed; output: `{"migration":"additive unbound/direct/tags fields","dedup":"window and Agent server identities"}`.
- `jr0y` `direct-access`: claimed → closed; output: `{"cmd_p":"ownership-ordered stable tiles","commands":["bind","unbind"]}`.
- `4d9y` `jump-panel-tree`: claimed → closed; output: `{"ui":"collapsible workspace folders + tile-native Unbound","metadata":"tags/status/provider/archive retained"}`.
- `kfs9` `real-path-verification`: claimed → closed, reopened for a same-workspace direct-focus regression, then claimed → closed; output: `{"gui":"609 passed","lib":"171 passed","build":"passed","observed_red":["project predicate inversion failed real paint test","same-workspace selection failed to leave direct Unbound focus"],"mutants":"29/29 caught"}`.
- `vm1a` `document-integrate`: claimed → closed; output: `{"docs":"contracts/project/worklog reconciled","integration":"feature branch committed and merged to main; focused main verification passed"}`.
- `sjyp` `omega`: claimed → closed; output: `{"result":"complete","graph":"9k2"}`.

### Notes

- Graph, seq `24`, topic `decision`: p-menu means Cmd-P Jump to; the period shell menu is out of scope.
- Graph, seq `25`, topic `deviation`: parallel copy-mode mutation testing exhausted the available disk; the low-footprint in-place rerun caught all 15 targeted mutants and restored the source tree.
- Node `kfs9`, seq `5`, topic `deviation`: converting the superseded
  ephemeral tests exposed and scoped the same-workspace direct-focus bug before
  integration.

### Final status

- Status: `complete`

```text
graph unbound-tile-workspaces (frontiers)
frontier 0: contract-architecture [done]
frontier 1: unbound-core [done]
frontier 2: persistence-migration [done]
frontier 3: direct-access [done]
frontier 4: jump-panel-tree [done]
frontier 5: real-path-verification [done]
frontier 6: document-integrate [done]
frontier 7: omega [done] (omega)
```

## Built (with status)

- Added exclusive bound/unbound ownership for stable tiles, direct unbound focus, state-preserving bind/unbind, workspace-close-to-Unbound, and additive persistence migration.
- Cmd-P and the jump panel now navigate stable tiles. The jump panel renders independently collapsible workspace folders and a tag-grouped Unbound list with Agent status/provider/archive metadata.
- Added real keyboard, menu, paint, click, fold, persistence, migration, tag, archive, and deletion coverage.

## Open / unresolved

- No runtime-only verification gap. Seven obsolete ephemeral-workspace tests
  were converted into stable unbound-tile guards; the two remaining ignored GUI
  tests are pre-existing platform/cache checks. Compatibility fields remain
  readable for old snapshots.
- Repository-wide `cargo fmt --all -- --check` remains red on broad pre-existing formatting drift, so this change did not rewrite unrelated files.

## Decisions

- ADR-0033: tiles have optional workspace ownership; direct focus is a non-owning pointer, and bind/unbind moves the complete tile.

## Verification status

- `cargo test --bin yalda-gpui --quiet`: 609 passed, 2 ignored.
- `cargo test --lib --quiet`: 171 passed, 2 ignored.
- `cargo build --bin yalda-gpui --bin yalda-session-server --quiet`: passed.
- Observed RED: changing the Unbound project predicate from `tile.project == id` to `!=` failed `jump_panel_workspace_folders_and_unbound_rows_are_tile_native` at `unbound projection`; restoring it passed.
- `cargo mutants --in-place ...`: 29 targeted ownership, Cmd-P,
  jump-panel, and same-workspace-selection predicates tested; 29 caught.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-18-unbound-tile-workspaces.md` passes.

## Next

- Use the stable tile ownership API for future workspace layout commands; do not reintroduce ephemeral workspaces or a parallel session-only navigation model.
