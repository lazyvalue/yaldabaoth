# Worklog: Fix crowded jump-panel workspace cards

**Date:** 2026-08-19
**Branches touched:** `codex/fix-jump-workspace-card-shrink` (`7cb10bd`),
then `main` (`c60ebc1` merge)

## Cog execution evidence

- Graph id: `d0t`

### Initial render

```text
graph fix-jump-workspace-card-shrink (frontiers)
frontier 0: reproduce-regression [open]
frontier 1: fix-card-sizing [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `e4ps` `reproduce-regression`: claimed → closed; output: the crowded
  production-paint path reproduced a 4px workspace group clipping a 29px
  header and 29px tile row in a 900×360 viewport.
- `8czd` `fix-card-sizing`: claimed → closed; output:
  `compact_bounded_group` became a non-shrinking flex item and the component
  contract, regression record, and manifest were updated.
- `fkw7` `verify-integrate`: claimed → closed; output: focused, adjacent,
  full GUI, all-target, negative-control, release-build, integration, and
  worklog checks completed while preserving the pre-existing user changes.
- `mp90` `omega`: claimed → closed; output: reproduction, root-cause fix,
  regression protection, documentation, integration, and verification are
  complete.

### Notes

- The first guard used the default test viewport and stayed green because the
  content fit. Constraining the real renderer to the reported crowded case at
  900×360 produced the exact failure; this was test refinement before the fix,
  not a product-code attempt.
- `cargo mutants --list` offered only structural default-return mutations for
  this helper, not deletion of the relevant method call. The required negative
  control was therefore performed manually by removing `flex_none`; it restored
  the exact 4px-band RED before the fix was reapplied.
- No GUI process was launched or restarted.

### Final status

- Status: `complete`

```text
graph fix-jump-workspace-card-shrink (frontiers)
frontier 0: reproduce-regression [done]
frontier 1: fix-card-sizing [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — workspace cards retain intrinsic height.** The shared bounded-group
  primitive now opts out of flex shrink, so the scrolling jump-panel column
  overflows instead of compressing cards into border-only bands.
- **DONE — crowded production-path guard.** Sixteen mixed expanded and
  collapsed workspaces render in a constrained viewport; expanded cards must
  retain a header and tile row, while collapsed cards must retain their header.
- **DONE — durable contract and bug record.** UXI-JumpPanel-27 and bug-0052
  document the sizing invariant, root cause, failed test shape, and fix.

## Open / unresolved

- The rebuilt release executable was not installed into or restarted in the
  running GUI. A restart is required to observe the fix in the live app.

## Verification status

- Observed RED: 4px group around a 29px header and 29px member row at 900×360.
- Required negative control: removing `flex_none` restored the exact 4px-band
  failure; reapplying it returned the guard to green.
- Focused crowded-card guard: passed.
- Adjacent jump-panel suite: 19 passed, 0 failed.
- Full GUI suite on the task branch: 667 passed, 0 failed, 2 ignored.
- Post-merge `cargo test --all-targets --features test-support
  --no-fail-fast`: passed; the three network/auth-dependent live tests were
  ignored as declared.
- Post-merge `cargo build --release --bin yalda-gpui
  --bin yalda-session-server`: passed (existing warnings only).
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-fix-jump-workspace-card-shrink.md`: passed after
  omega closure.

## Preserved user state

- Existing user changes in `.claude/scheduled_tasks.lock`, `Cargo.lock`, and
  `Cargo.toml` were neither staged nor modified by this fix.

## Next

- Restart the GUI to load the rebuilt release executable.
