# Worklog: Agent picker Enter consistency

**Date:** 2026-08-19
**Branches touched:** `codex/fix-agent-picker-enter-consistency` (`0c00933`),
then `main` (`b69a100` merge)

## Cog execution evidence

- Graph id: `ipz`

### Initial render

```text
graph fix-agent-picker-enter-consistency (frontiers)
frontier 0: real-enter-matrix [open]
frontier 1: fix-enter-placement [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `spba` `real-enter-matrix`: claimed → closed with output recording a truthful
  real-key RED after Down navigation, a live tagged-roster shrink, repaint, and
  Enter; focus incorrectly remained on picker tile 4 instead of stable tile 3.
- `pzts` `fix-enter-placement`: claimed → closed with output recording keyboard
  cursor normalization against the current live projection and passing real-key
  dormant, already-local, roster-shrink, click, and command guards.
- `l2e3` `verify-integrate`: claimed → closed with output recording 630 passing
  GUI tests and passing release builds on branch and merged main, feature commit
  `0c00933`, and merge commit `b69a100`.
- `1ktl` `omega`: claimed → closed with output aggregating the root cause, fix,
  verification, and integration evidence.

### Notes

- The previous handoff overclaimed keyboard coverage: its already-local guard
  called activation directly. This pass converted that guard to real Enter and
  added the missing live-projection transition.
- Node `l2e3`, topic `deviation`: the main checkout contained unrelated scheduler
  lock, titlebar, and `raw-window-handle` dependency edits. Autostash `c885186`
  preserved and reapplied all three around merge `b69a100`.

### Final status

- Status: `complete`

```text
graph fix-agent-picker-enter-consistency (frontiers)
frontier 0: real-enter-matrix [done]
frontier 1: fix-enter-placement [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — real Enter consistency.** The picker roster is live. When rows shrink
  while open, keyboard movement and Enter now normalize the stored cursor to the
  same valid row the current frame visibly highlights.
- **DONE — state matrix.** Real Enter is covered for roster-only dormant Agents,
  already-local unbound Agents without duplication, and a tagged roster whose
  selected row shifts during a focused repaint.

## Open / unresolved

- None for this recurrence. The two ignored GUI tests and existing compiler
  warnings predate this change.

## Verification status

- Observed RED: after real `down down down`, removing the first tagged roster
  session visually moved the highlight from row 3 to row 2, but real Enter
  submitted stale row 3 and left focus on empty picker tile 4.
- `cargo test --bin yalda-gpui`: 630 passed, 0 failed, 2 ignored on the feature
  branch and merged `main`.
- `cargo build --release --bin yalda-gpui`: passed on the feature branch and
  merged `main`; the main release executable is rebuilt.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-agent-picker-enter-consistency.md`: passes.

## Next

- None required.
