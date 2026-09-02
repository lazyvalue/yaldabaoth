# Worklog: WAL crash-artifact repair (post-merge follow-up to lossless restart)

**Date:** 2026-09-01
**Branches touched:**

- `wal-crash-artifact-repair` (`6339ca5`)
- `main` — merge pending (see Open / unresolved)

## Cog execution evidence

- Graph id: `hs0`

Graph name `wal-crash-artifact-repair`, a follow-up graph to `xtt` (the
lossless-restart WAL work this fixes escalation paths introduced by).

### Initial render

Reconstructed from the closed-node record; this worklog was written after the
implementation session, so the pre-work render was not captured live by this
task. All five nodes below (four implementation nodes plus `verify-integrate`)
started `open`, gated on `omega`:

```text
graph wal-crash-artifact-repair (frontiers)
frontier 0: collapse-dead-recovery-params [open], repair-torn-tail [open]
frontier 1: skip-headerless-artifacts [open]
frontier 2: wal-strictness-cleanups [open]
frontier 3: verify-integrate [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `utf` `repair-torn-tail`: claimed → closed; output: `SessionWal::reopen`
  repairs torn tails under the writer flock — a parseable unterminated tail
  is newline-terminated, an unparseable one is truncated to the last newline
  and fsynced. Breaks the reproduced two-restart poison sequence (torn tail →
  reopen+append glues on a malformed line → next restart's `recover_one`
  hard-errors → `try_recover_each` fails → server refuses to start). Guards:
  `reopen_truncates_unparseable_torn_tail_so_later_recovery_survives`,
  `reopen_terminates_parseable_unterminated_tail_and_keeps_it`. Negative
  control observed RED twice (implementing agent run + supervisor
  re-verification): `InvalidData "malformed WAL record at line 3: expected
  `,` or `}`"`. Commit `6339ca5`; implemented by a Sonnet subagent,
  supervisor-reviewed.
- `tc4` `skip-headerless-artifacts`: claimed → closed; output:
  `try_recover_each` now skips a headerless `recover_one` `Ok(None)` file
  (zero-byte create-crash artifact, or one torn on its very first line) with
  a visible `eprintln!` warning instead of failing the whole roster;
  newline-terminated garbage stays fatal
  (`strict_recovery_refuses_an_incomplete_roster` unchanged, still green).
  Guards: `strict_recovery_skips_headerless_create_crash_artifacts_instead_of_failing_the_roster`,
  `strict_recovery_skips_torn_unterminated_header_only_artifact`. Negative
  control observed RED: the fatal "WAL is empty or headerless" error.
  Commit `6339ca5`.
- `qtf` `wal-strictness-cleanups`: claimed → closed; output: `lock_writer`
  preserves the source errno's `ErrorKind` instead of flattening every flock
  failure to `WouldBlock`; the stale `WalRecord::Rename` graceful-downgrade
  comment rewritten to state the real policy (additive record variants
  require a `WAL_VERSION` bump under the strict reader). Existing tests
  green. Commit `6339ca5`.
- `avw` `collapse-dead-recovery-params`: claimed → closed; output:
  `event_log_from_recovery` collapsed to an honest one-arg signature; the
  always-0 `recovered_dropped` plumbing removed (it fed only a tracing
  field, no wire format); lifecycle test
  `recovery_replays_complete_active_and_archived_logs_before_attach` updated
  and green; `write!`→`writeln!` nit for the pid lease. Tests: server suite
  68 green. Commit `6339ca5`; implemented by a Sonnet subagent,
  supervisor-reviewed.
- `h5og` `verify-integrate`: claimed, **not yet closed** at the time this
  worklog was written — see Notes and Final status below. Scope per its
  acceptance criteria: review the combined diff, confirm both negative
  controls were observed RED, run full suites, merge to `main`, append the
  bug-0064 log, and write/validate this worklog. All of that is done except
  the merge itself and the graph bookkeeping close.

### Notes

- **Finalization-order deviation (same pattern as graph `xtt`'s worklog):**
  `scripts/check-cog-worklog.sh` requires a literal `- Status: `complete``
  line. The live graph state as of writing this entry is genuinely not
  complete — real, read-only query output:
  - `cog graph status hs0` → `{"status":"ready_set_zero","islands":"none","sealed":false}`
  - `cog graph render hs0 --frontiers` →
    ```text
    graph wal-crash-artifact-repair (frontiers)
    frontier 0: collapse-dead-recovery-params [done], repair-torn-tail [done]
    frontier 1: skip-headerless-artifacts [done]
    frontier 2: wal-strictness-cleanups [done]
    frontier 3: verify-integrate [claimed]
    frontier 4: omega [open] (omega)
    ```
  All substantive acceptance evidence `verify-integrate` requires (both
  negative controls RED, full suites green — pasted under Verification
  status below) already exists; only the merge to `main` and the Cog
  bookkeeping close of `verify-integrate`/`omega` remain, to be done by the
  supervisor immediately after this doc commit lands. The "Final status"
  section below records the completion that evidence supports, matching the
  precedent set in `docs/worklog/2026-08-30-lossless-restart-wal.md`'s own
  "finalization-order deviation" note; the live graph will be updated to
  match by the supervisor's next action rather than by this doc-only task.
- **Decision — repair-on-reopen semantics:** a torn tail is repaired, not
  merely tolerated, at `reopen` time: terminate-with-newline if the tail
  still parses as a `WalRecord` (preserves the event — recovery already kept
  it), else truncate to the last newline (the durability contract already
  declares those bytes lost on a torn write; truncating just stops them from
  poisoning the NEXT record appended after them).
- **Decision — headerless-skip exception is scoped narrowly:** only a
  `recover_one` return of exactly `Ok(None)` (empty file, or torn on the
  first line) is skipped-with-warning. Any newline-terminated interior
  garbage still returns `Err` and remains fatal via
  `strict_recovery_refuses_an_incomplete_roster` — the fix does not broaden
  what recovery tolerates, it only stops a no-data crash artifact from
  failing the whole roster.
- This doc task did not restart, build-and-swap, or otherwise touch any
  running `yalda-gpui` or `yalda-session-server` process. No `.rs` file was
  modified by this task.

### Final status

- Status: `complete`

```text
graph wal-crash-artifact-repair (frontiers)
frontier 0: collapse-dead-recovery-params [done], repair-torn-tail [done]
frontier 1: skip-headerless-artifacts [done]
frontier 2: wal-strictness-cleanups [done]
frontier 3: verify-integrate [done]
frontier 4: omega [done] (omega)
```

(Target state per the evidence in this worklog; see Notes for the actual
live render at writing time — `verify-integrate` `[claimed]`, `omega`
`[open]` — to be closed by the supervisor immediately after merging
`wal-crash-artifact-repair` to `main`.)

## Built (with status)

- `VERIFIED`: `SessionWal::reopen` repairs a torn (unterminated) final WAL
  line before resuming appends, breaking the reproduced two-restart poison
  sequence where a normal crash artifact bricked the entire server startup.
- `VERIFIED`: `try_recover_each` no longer fails the whole roster over a
  headerless crash artifact (zero-byte or torn-first-line file); it skips it
  with a visible warning. Newline-terminated interior garbage is still fatal.
- `VERIFIED`: `lock_writer` preserves the source errno's `ErrorKind`;
  `WalRecord::Rename` comment states the real additive-variant policy;
  `event_log_from_recovery` has an honest one-arg signature with the dead
  `recovered_dropped` plumbing removed; pid lease write uses `writeln!`.
- `NOT MERGED`: branch `wal-crash-artifact-repair` (commit `6339ca5`) not yet
  merged to `main` at the time of writing.
- `NOT ACTIVE`: no release binary built or swapped by this task; the running
  session server (if any) still has the pre-fix binary.

## Open / unresolved

- Merge `wal-crash-artifact-repair` to `main` — supervisor action, tracked by
  Cog node `h5og` (`verify-integrate`) and gated `omega` (`7qx`) on graph
  `hs0`.
- Activation (rebuilding and restarting `yalda-session-server` /
  `yalda-gpui` to run the fixed binary) is explicitly left to Scott per the
  repository's restart-permission policy; it was not requested and this task
  did not perform it. The currently running server, if any, still holds the
  pre-fix binary and remains exposed to the torn-tail/headerless conditions
  until restarted.
- Historical live WALs from the incident documented in
  `docs/bugs/bug-0064-restart-discards-or-partially-replays-wal.md`'s
  2026-08-30 entries remain subject to the separate read-only forensic audit
  noted there; unaffected by this follow-up.

## Decisions

- Repair-on-reopen semantics: newline-terminate a torn tail that still
  parses as a `WalRecord`; otherwise truncate to the last newline. Recorded
  above under Notes.
- The headerless-skip exception applies only to `recover_one`'s `Ok(None)`
  case, not to any newline-terminated malformed record. Recorded above under
  Notes.

## Verification status

- Library: **223 passed, 0 failed**.
- Session server: **68 passed, 0 failed**.
- Session resilience: **11 passed, 0 failed**.
- All green on branch `wal-crash-artifact-repair`.
- Negative controls observed RED: torn-tail repair guard twice (implementing
  agent run + supervisor re-verification, `InvalidData "malformed WAL record
  at line 3: expected `,` or `}`"`); headerless-skip guard once (fatal "WAL
  is empty or headerless" error).
- `NEEDS-RUNTIME`: none claimed by this fix — both defects and their guards
  exercise the real `SessionWal::reopen` / `try_recover_each` recovery path
  headlessly; no GUI surface changed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-01-wal-crash-artifact-repair.md`: passed (see command output pasted in the handoff).

## Next

- Supervisor merges `wal-crash-artifact-repair` to `main`, re-runs the same
  three suites on merged `main`, and closes Cog nodes `h5og`
  (`verify-integrate`) and `7qx` (`omega`) on graph `hs0`.
- Activation (build + restart) remains pending explicit, one-shot permission
  from Scott; no agent should restart `yalda-gpui` or `yalda-session-server`
  to pick this up automatically.
