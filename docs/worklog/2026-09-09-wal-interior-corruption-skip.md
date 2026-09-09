# Worklog: wal-interior-corruption-skip

**Date:** 2026-09-09
**Branches touched:** `bug-0064-skip-corrupt-wal` (a0af02c, worktree) → fast-forwarded into `main`

Session opened with "Is the yalda server running?" — it wasn't. bug-0064's
third recurrence: the systemd service was crash-looping on a WAL the strict
reader refuses, all 55 sessions offline. Diagnosis, an approved hand repair,
then a code fix so one corrupt file can never take the roster down again.

## Cog execution evidence

- Graph id: `fgb`
- An accidental duplicate import `ba0` was noted `superseded` and sealed; no
  work ran on it.

### Initial render

```text
graph bug-0064-skip-interior-corrupt-wal (frontiers)
frontier 0: bug-record [open], unit-guard [open]
frontier 1: reader-fix [open]
frontier 2: server-guard [open]
frontier 3: verify-integrate [open]
frontier 4: worklog [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `dzx9` `bug-record`: claimed → closed; output: bug-0064 file status RECURRED
  + 2026-09-09 log entry (symptom, timeline, root cause, hand repair, planned
  policy, how it differs from 08-30/09-01); manifest row RECURRED, times 4.
- `y1o1` `unit-guard`: claimed → closed; output:
  `strict_recovery_skips_interior_corrupt_file_and_recovers_the_rest` replaces
  `strict_recovery_refuses_an_incomplete_roster`; production shape (complete
  record / verbatim `a1204e30` orphan fragment / complete record); observed
  RED pre-fix: `InvalidData malformed WAL record at line 3` aborted the roster.
- `077q` `reader-fix`: claimed → closed; output: `try_recover_each` skips a
  `recover_one` `Err(InvalidData)` file with a loud `SKIPPING CORRUPT WAL`
  warning, retains it, continues; other I/O and `visit` errors still
  propagate; doc comment rewritten. `cargo test --lib session_wal` 22 passed.
- `8qf4` `server-guard`: claimed → closed; output:
  `tests/session_resilience_test.rs::interior_corrupt_wal_is_skipped_and_the_server_still_boots`
  boots the REAL binary against a seeded WAL dir (corrupt file sorts first);
  asserts up, `list_sessions` has valid and NOT corrupt, bytes untouched, log
  names file + line 3. Negative control (skip arm disabled): `server exited
  during recovery (exit status: 1)` with the production log line verbatim.
- `vqg9` `verify-integrate`: claimed → closed; output: see Built / Verification.
- `963n` `worklog`: claimed → closed; output: this file + backlog, evidence
  check passed.
- `dhdc` `omega`: claimed → closed; output: fix on `main`, release server
  binary built, NOT activated.

### Notes

- `graph`, seq `10`, topic `decision`: reverse the 08-30 whole-roster
  fail-closed clause for durable content corruption only (`InvalidData` →
  skip per file, loud, retained); real I/O and reopen errors still fatal;
  `recover_one` stays strict per record. Deviation: the test asserting the old
  policy was deleted, not kept.

### Final status

- Status: `complete`

```text
graph bug-0064-skip-interior-corrupt-wal (frontiers)
frontier 0: bug-record [done], unit-guard [done]
frontier 1: reader-fix [done]
frontier 2: server-guard [done]
frontier 3: verify-integrate [done]
frontier 4: worklog [done]
frontier 5: omega [done] (omega)
```

## What happened (diagnosis)

- `yalda-session-server.service` `failed` — exit 1 in ~140 ms, five restarts,
  "Start request repeated too quickly". Its stderr goes to
  `~/.yalda/session-server.log` (not the journal): ten times
  `Error: Custom { kind: InvalidData, error: "malformed WAL record at line
  10820: expected value at line 1 column 1" }` after `recovering session
  0a6af469` — i.e. the NEXT file in sort order, `0b319323-…log`.
- Line 10820 is a 248-byte **orphan tail fragment**; its neighbours are
  complete valid records and it rejoins with neither. Whole-dir scan found
  exactly one more: `a1204e30-…log` line 11811 (38 bytes,
  `pected":false},"toolName":"Bash"}}}}}`). Both files were last written
  during the Sep 1–2 twin-writer split-brain (bug-0064 recurrence #2): the
  head of each record was clobbered by the concurrent appender.
- Why only now: the resident server booted Sep 1 21:22 PDT — *before* the
  damage — and ran 7 days. Sep 8 21:22 the GUI disconnected; 21:24 a fresh
  server refused to boot (Highlander: 6 `(deleted)`-exe zombies); 21:24:43 all
  six SIGTERMed. **Server down from 21:24 Sep 8**, not 00:46. 00:18 Sep 9
  first start of the strict reader → the wall; 00:46 `deploy-server.sh` →
  same wall.

## Built (with status)

- **Hand repair of production data** (Scott approved "3 — both"): both files
  copied byte-for-byte to `~/.yalda/wal-backup-torn-20260909T005200/`; the
  single orphan line removed by a verify-every-line python pass + atomic
  rename (48322 / 78946 lines kept, all parse; whole-dir rescan clean). Loss
  bound: at most one `tool_call_updated` per file, both week-old fulcrum
  sessions (one archived).
- **Service started** with explicit approval (`reset-failed` + `start`):
  `active (running)` 00:52:12 PDT, 55 sessions recovered, 0 errors.
  Observation: 7.2 GB peak RSS replaying 1.1 GB of WALs → backlog.
- **Code fix** `a0af02c` on `main`: `src/session_wal.rs` `try_recover_each`
  per-file skip for `InvalidData`; guards above; bug-0064 file + manifest
  (`RECURRED→FIXED`, times 4).
- Suites on the branch: `--lib` 223 · `--bin yalda-session-server` 68 ·
  `--test session_resilience_test` 15 (14 + new). On `main` after the
  fast-forward: see Verification status.
- Release `yalda-session-server` built in the main checkout — **not installed,
  not restarted**.

## Open / unresolved

- Activation of the fix (`./deploy-server.sh`) — Scott's call; the running
  server is the pre-fix binary and would crash-loop again on any future
  interior-corrupt WAL. See backlog.
- Replay memory (7.2 GB peak) — backlog, `READY`.
- `cargo-mutants` not installed on this host (same as 09-08); the changed
  predicate got a direct negative control instead of a mutation run.
- Pre-existing, untouched: `M src/bin/yalda-gpui/keymap_registry.rs` in the
  main checkout (someone else's in-flight edit); rustfmt drift in
  `benches/render_bench.rs` and three older hunks of
  `session_resilience_test.rs`.

## Decisions

- Recorded as graph note `fgb`/seq 10 and in the bug-0064 log; not ADR-sized.
  If the fail-closed roster contract is ever written up as an ADR, this is
  the carve-out: content corruption skips per file, environment errors fail
  the boot.

## Verification status

- Real-path guards on the actual startup path (`restore_seed_from_disk` →
  `try_recover_each` → `recover_one`), both observed RED with the fix
  disabled and green with it; the server guard reproduces the production
  exit-1 log line verbatim.
- Suites on `main` @ a0af02c (main checkout, fresh debug build): `--lib` 223
  passed / 2 ignored · `--bin yalda-session-server` 68 · `--test
  session_resilience_test` 15. `cargo build --release --bin
  yalda-session-server` → `target/release/yalda-session-server` 01:01:29 PDT;
  `~/.local/bin/yalda-session-server` is still the 00:46 pre-fix binary and
  the running service was not touched.
- Not runtime-verified: the fixed binary against the real `~/.yalda/wal`
  (gap 2 — needs the deploy Scott owns). The hand-repaired WALs are already
  proven by the running (old) server's clean 55-session recovery.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-09-wal-interior-corruption-skip.md` passes.

## Next

- Scott: `./deploy-server.sh` when convenient (restarts the service; every
  attached GUI session reconnects; WAL replay lossless). Then delete
  `~/.yalda/wal-backup-torn-20260909T005200/` (488 MB) once satisfied.
- Investigate replay memory: 1.1 GB of WAL → 7.2 GB RSS at boot.
