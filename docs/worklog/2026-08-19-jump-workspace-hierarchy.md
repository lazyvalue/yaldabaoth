# Worklog: Jump-panel workspace hierarchy

**Date:** 2026-08-19
**Branches touched:** `codex/jump-workspace-hierarchy` (`cdd32cd`), then
`main` (`0248e89` merge)

## Cog execution evidence

- Graph id: `b0m`

### Initial render

```text
graph jump-workspace-hierarchy (frontiers)
frontier 0: design-contract [open]
frontier 1: implement-cards [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `7gtr` `design-contract`: claimed → closed; output: UXI-JumpPanel-27
  defines a flat cool-blue bounded workspace group, membership count, folded
  state, and Detached separation; the production paint guard was observed RED
  because the prior renderer had no shared group boundary.
- `4f0c` `implement-cards`: claimed → closed; output: workspace folders use the
  reusable yux `compact_bounded_group`, a blue-derived style projection, and an
  attached-tile count while preserving ellipsis, hidden markers, folding,
  selection, and click dispatch.
- `udmw` `verify-integrate`: claimed → closed; output: focused, adjacent,
  mutation, full GUI, library, all-target, integration, release-build, and
  worklog checks passed; branch `cdd32cd` was merged as `0248e89` with
  unrelated user changes preserved.
- `8ksz` `omega`: claimed → closed; output: visual hierarchy, ownership
  clarity, cool-blue palette, reusable primitive, regression coverage,
  documentation, integration, and release build are complete.

### Notes

- Design-node deviation note sequence 4 records that the sandbox denied the
  Apple Metal module cache under `~/.cache/clang`; the focused test was rerun
  with approved host access.
- Verification-node deviation note sequence 4 records that `cargo fmt` touched
  pre-existing formatting drift in 42 unrelated files inside the isolated
  worktree. Every unrelated file and hunk was explicitly restored before the
  feature commit; only the six intended files landed.
- No GUI process was launched or restarted.

### Final status

- Status: `complete`

```text
graph jump-workspace-hierarchy (frontiers)
frontier 0: design-contract [done]
frontier 1: implement-cards [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — spatial ownership.** Each workspace header and its attached visible
  or hidden tile rows occupy one subtle rounded outline; child rows are inset
  below a shared separator. Detached tiles remain outside every card.
- **DONE — restrained blue hierarchy.** The outline, workspace glyph,
  membership pill, separator, active rail, and quiet header wash are alpha-only
  derivatives of the existing cool-blue `DETACHED` token. No brown/gold accent
  participates in workspace chrome.
- **DONE — compact membership count.** Every header reports its attached-tile
  count, including hidden members, and retains it when collapsed.
- **DONE — reusable yux primitive.** `compact_bounded_group` owns the common
  outline, spacing, clipping, separator, and optional body treatment.

## Open / unresolved

- **NEEDS-RUNTIME (verification gap 1):** the harness proves painted geometry
  and exact color-token derivation but cannot inspect rasterized pixels. Final
  optical weight and spacing should be judged after restarting the GUI; the
  values are intentionally subdued and centralized for easy tuning.

## Verification status

- Initial RED: the focused production paint guard failed at
  `jump-workspace-group-*` because the old header/tree-guide layout had no
  shared boundary.
- Focused/adjacent jump-panel suite: 18 passed, 0 failed.
- Full deterministic GUI suite: 666 passed, 0 failed, 2 ignored.
- Library suite: 173 passed, 0 failed, 2 ignored.
- Targeted mutation gate: 5 mutants tested; 4 caught, 1 structurally unviable,
  0 survivors.
- Post-merge `cargo test --all-targets --features test-support
  --no-fail-fast`: passed; live external-service tests remained ignored.
- Post-merge `cargo build --release --bin yalda-gpui
  --bin yalda-session-server`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-jump-workspace-hierarchy.md`: passed after omega.

## Next

- Restart the GUI to load the rebuilt release and judge the final optical
  balance in the real panel.
