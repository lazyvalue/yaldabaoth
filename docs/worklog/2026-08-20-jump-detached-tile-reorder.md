# Worklog: Jump-panel Detached tile reorder

**Date:** 2026-08-20
**Branches touched:** `codex/jump-detached-reorder` (`0eecbd8`), then `main`
(`b3a3bf8` merge)

## Cog execution evidence

- Graph id: `gx4`

### Initial render

```text
graph jump-detached-tile-reorder (frontiers)
frontier 0: contract-red-guard [open]
frontier 1: persist-detached-order [open]
frontier 2: wire-verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `5kes` `contract-red-guard`: claimed → closed; output: extended
  UXI-JumpPanel-28, introduced the typed drag-group contract, and added a real
  production-projection guard. Observed RED against the no-op scaffold:
  `[alpha, beta, gamma]` remained unchanged instead of becoming
  `[gamma, alpha, beta]`.
- `inho` `persist-detached-order`: claimed → closed; output: added independent
  preference save/load, stable alphabetical-plus-rank projection, a total order
  across every activity tab, and a live exact-project/tag state guard.
- `8x2u` `wire-verify-integrate`: claimed → closed; output: wired buffer and
  agent rows in tagged and untagged Detached groups through the shared tile drag
  abstraction, completed negative-control and mutation verification, passed the
  full suite and release builds, and merged the feature to main while preserving
  existing dirty files.
- `qly4` `omega`: claimed → closed; output: aggregated the shipped behavior,
  persistence, invariant proof, release verification, and main integration.

### Notes

- The first copied-tree mutation baseline could not write Cargo/Apple Metal
  caches under the filesystem sandbox. It was rerun with the required cache
  access.
- The first mutation pass caught 9 of 12 reducer mutants. The test was
  strengthened with foreign-project/same-tag, same-project/wrong-tag, complete
  total-order, project-order, durable-no-op, and prior-rank assertions; the
  focused rerun caught all three survivors.
- Repository-wide `cargo fmt --check` reports broad pre-existing formatting
  drift. Every changed hunk was matched to rustfmt output, and `git diff --check`
  passes.
- Main remained at the feature base (`fc082f4`) throughout development, so no
  main-into-feature sync merge was necessary.

### Final status

- Status: `complete`

```text
graph jump-detached-tile-reorder (frontiers)
frontier 0: contract-red-guard [done]
frontier 1: persist-detached-order [done]
frontier 2: wire-verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — Detached row drag/drop.** Both agent-backed and non-agent tile rows
  drag with the same preview, hover, and drop behavior.
- **DONE — exact visible-group boundaries.** Untagged rows reorder only within
  their project’s untagged group. Tagged rows reorder only inside the exact
  project/tag folder. Workspace and Detached drag variants cannot cross-fire.
- **DONE — presentation-only semantics.** Reordering cannot alter tile identity,
  attachment, project ownership, or tags.
- **DONE — durable order.** `jump_detached_tile_order` persists independently
  from attached workspace tile order. Missing and newly discovered tiles retain
  the existing alphabetical default after ranked tiles.
- **DONE — filter safety.** Reordering rebuilds from all Detached tiles, so a
  Waiting/Working/All/Archived filter cannot erase hidden identities from the
  stored order.

## Open / unresolved

- None.

## Decisions

- No ADR required. This is jump-panel presentation state and does not change the
  workspace attachment model or operational tile ownership.
- A multi-tag tile has one durable rank, reflected consistently in every tag
  folder in which it appears.

## Verification status

- Initial RED and restored negative control both produced alphabetical order
  instead of the requested reordered projection when the stable rank sort was
  absent.
- Scoped mutation testing: 12/12 `reorder_detached_tile` mutants caught.
- Focused production projection, preference compatibility, and existing
  Detached paint/click guards passed.
- `cargo check --features test-support --bin yalda-gpui`: passed.
- `cargo test --all-targets --features test-support`: passed; only explicitly
  live/credential-dependent tests were ignored.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-20-jump-detached-tile-reorder.md`: passes.

## Next

- Rebuild/restart Fulcrum and drag Detached rows within an untagged group or tag
  folder to confirm the pointer feel with the active theme.
