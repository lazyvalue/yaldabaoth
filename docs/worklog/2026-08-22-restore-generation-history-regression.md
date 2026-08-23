# Restore generation history regression

**Date:** 2026-08-22
**Branch:** `codex/restore-generation-history`
**Main merges:** `8d39a01`, `e20446e`

## Cog execution evidence

- Graph id: `od4`

### Initial render

```text
graph restore-generation-history-regression (frontiers)
frontier 0: setup-repro [open]
frontier 1: fix-history [open]
frontier 2: record-regression [open], verify-activate [open]
frontier 3: omega [open] (omega)
```

### Node execution

Every node was claimed and closed with output using actor `claude-code`:

- `budf` `setup-repro`: created the dedicated worktree and observed the exact
  production-sequence regression RED with an empty transcript.
- `7lim` `fix-history`: separated generation bookkeeping from full transcript
  reset; commit `f0e34e4`; focused and full GUI suites passed.
- `x9e5` `verify-activate`: merged the GUI fix, rebuilt release, restored all 30
  leaves, and verified the live server roster and example session.
- `d1ep` `restart-script`: added after activation revealed a surviving old GUI;
  commit `2685397`; hardened restart then left exactly one GUI process.
- `28ub` `record-regression`: appended bug-0002, added bug-0058, updated the
  lifecycle record, and validated both Cog worklogs.
- `y9l4` `omega`: confirms merges, tests, activation, records, and data safety.

### Notes

- Plan deviation: activation revealed a distinct restart-script defect, so
  `restart-script` was added after `fix-history` and connected to omega.
- Runtime visual-capture limitation: the Yalda window was assigned to another
  macOS Space; normal capture showed the current desktop and direct
  ScreenCaptureKit access was denied by TCC. Substituted evidence is the exact
  real-reducer regression, GUI `BOUND+resume` logs, one-process assertion, and
  read-only server admin status. Human visual confirmation was requested while
  records were completed.
- `cargo fmt --all -- --check` under host rustfmt 1.8.0 reports broad
  repository-wide drift in untouched files. No unrelated rewrite was made;
  `git diff --check` passed for the repair.

### Final status

- Status: `complete`

```text
graph restore-generation-history-regression (frontiers)
frontier 0: setup-repro [done]
frontier 1: fix-history [done]
frontier 2: record-regression [done], restart-script [done], verify-activate [done]
frontier 3: omega [done] (omega)
```

## Root cause and repair

The session server correctly recovered and streamed the durable WAL. On every
resume it recorded a newer-generation `ChannelOpened`, while its replay fence
discarded the agent's duplicate history. The GUI interpreted that channel event
as an instruction to clear the editor and wait for replacement replay that could
never arrive, leaving every restored tile empty.

`begin_server_generation` now resets only per-generation reconciliation,
finalization, turn, and stream-gate state. The explicit reconnect path remains
the sole owner of clearing the editor immediately before full WAL replay.

Activation also exposed that `dev-gui.sh` did not require the old process to
exit. It now performs a bounded TERM/wait/KILL/verify sequence over only
repo-built GUI executables before launching the replacement.

## Verification

- Negative control: the exact
  `history → ChannelOpened(new generation) → ReplayEnd` guard failed with an
  empty transcript before the fix.
- `restore_keeps_durable_history_when_resume_duplicates_are_fenced`: passed;
  later canonical + legacy live output rendered exactly once.
- Focused restore, generation rebaseline, and real `/clear` reducer tests:
  passed.
- `cargo test --bin yalda-gpui`: **691 passed, 0 failed, 2 ignored**.
- `bash -n dev-gui.sh`: passed.
- Release `./dev-gui.sh`: passed; exactly one release GUI remained.
- Read-only runtime: 153 stored = 123 archived + 30 live; 30/30 live connected;
  30/30 subscribed. `integration meta planner`: connected, 158 turns, 37,509
  resident events, subscriber count 1, generation 3.
- No WAL deletion, server restart, or server-state mutation was used for this
  GUI-only repair.
- `scripts/check-cog-worklog.sh` on this worklog and the parent lifecycle
  worklog: passes.
