# Worklog: Jump panel readability states

**Date:** 2026-08-19
**Branches touched:** `codex/jump-panel-readability` (`e25828e`), then
`main` (`dace4ea` merge)

## Cog execution evidence

- Graph id: `ddd`

### Initial render

```text
graph jump-panel-readability (frontiers)
frontier 0: lock-contract [open]
frontier 1: implement-styles [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `k4d9` `lock-contract`: claimed → closed; output: reproduced the Folio active
  workspace foreground regression, multi-line long-title row, and missing hidden
  marker through the real headless jump-panel paint path; documented bugs
  0049–0051 and UXI-JumpPanel-26.
- `mp7j` `implement-styles`: claimed → closed; output: added a neutral active
  workspace treatment with foreground label and accent rail, a reusable
  single-line ellipsis primitive, and a typed attached-visible / attached-hidden
  / detached projection with compact hidden pills.
- `2csq` `verify-integrate`: claimed → closed; output: focused, adjacent,
  mutation, all-target, and release verification passed; branch commit
  `e25828e` was integrated by merge commit `dace4ea`, with user-owned changes
  preserved.
- `dzit` `omega`: claimed → closed; output: the readability, truncation,
  hidden-state, documentation, integration, and verification requirements are
  complete.

### Notes

- Cog deviation note sequence 16 records two infrastructure observations: the
  sandbox denied the Apple Metal compiler cache during the first mutation run,
  so the mutation check was rerun with approved host access; and two unrelated
  steering tests are non-hermetic when `yalda-session-server` is absent from
  `PATH`. Both had passed in the earlier full-bin run and the final all-targets
  run.
- No GUI process was launched or restarted.

### Final status

- Status: `complete`

```text
graph jump-panel-readability (frontiers)
frontier 0: lock-contract [done]
frontier 1: implement-styles [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — readable active workspace.** Active workspace labels retain the
  theme foreground; a neutral selected background and 2px accent rail carry
  active state without sacrificing contrast.
- **DONE — single-line names.** Workspace and tile names use the shared
  `single_line_ellipsis` primitive, so long names truncate instead of wrapping
  and changing row height.
- **DONE — hidden-state indicator.** Hidden attached tiles, including Agent
  tiles, paint a compact trailing `hidden` pill. Detached rows cannot acquire
  the marker because the placement projection makes detached-and-hidden an
  unrepresentable state.
- **DONE — regression documentation.** UXI-JumpPanel-26 and bug records
  0049–0051 capture the production contract and its regression guards.

## Open / unresolved

- **NEEDS-RUNTIME:** exact pixel aesthetics and color perception remain a
  visual verification gap beyond the tested paint bounds and exact theme-token
  assertions. The user can inspect the rebuilt release after restarting the
  GUI.

## Verification status

- Initial RED evidence: the Folio active label used pale selection-mark color
  instead of foreground; a long title expanded its row to 92.5px versus 29px;
  and a hidden attached tile painted without an indicator.
- Focused guards passed for active workspace contrast, single-line long names,
  and hidden indicators.
- Adjacent workspace-folder, tagged-item sizing, hidden-tile navigation, and
  destination-picker guards passed.
- Manual negative controls made each focused guard fail when its production
  behavior was reverted, then return green after restoration.
- `cargo mutants`: 8 mutants tested; 2 viable placement mutants caught and 6
  default-return structural mutants unviable; no survivors.
- Branch GUI suite: 662 passed, 0 failed, 2 ignored.
- Library suite: 173 passed, 0 failed, 2 ignored.
- Post-merge `cargo test --all-targets --features test-support
  --no-fail-fast`: passed, with 3 live tests ignored.
- Post-merge `cargo build --release --bin yalda-gpui
  --bin yalda-session-server`: passed.
- `git diff --check`: passed. Repository-wide `cargo fmt --all -- --check`
  continues to report pre-existing formatting drift in unrelated files; no bulk
  rewrite was retained.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-jump-panel-readability.md`: passed after omega.

## Next

- Restart the GUI to load the rebuilt release executable and visually inspect
  the final theme treatment.
