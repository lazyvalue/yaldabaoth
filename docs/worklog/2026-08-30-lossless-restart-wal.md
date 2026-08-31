# Worklog: lossless restart WAL and split-brain prevention

**Date:** 2026-08-30
**Branches touched:**

- `fix/lossless-restart-wal` (`8eef3b1`)
- `main` (`077b5e1` merge)

## Cog execution evidence

- Graph id: `xtt`

### Initial render

Shown before tracked implementation edits. The graph was expanded when live
evidence exposed backup/replay and concurrent-server failure modes.

```text
graph make-restarts-session-wal-lossless (frontiers)
frontier 0: audit-restart-loss [open]
frontier 1: audit-backup-replay [open], spec-lossless-invariant [open]
frontier 2: guard-old-wal [open], harden-restart-entrypoints [open]
frontier 3: implement-wal-migration [open]
frontier 4: verify-lossless-restarts [open]
frontier 5: integrate-document [open]
frontier 6: final-worklog [open]
frontier 7: omega [open] (omega)
```

### Node execution

- `lsd` `audit-restart-loss`: claimed → closed; localized exact-version WAL
  discard and pre-attach replay compaction, including live evidence of 122,296
  durable events omitted from publication.
- `ozy` `audit-backup-replay`: claimed → closed; proved only active `*.log`
  files are discovered and rejected unsafe automatic backup merging.
- `wus` `spec-lossless-invariant`: claimed → closed; added
  `UXI-AgentTile-44` and bug-0064.
- `3vu` `guard-old-wal`: claimed → closed; historical v1/v2 production-path
  fixtures observed RED under exact-version discard.
- `9gx` `harden-restart-entrypoints`: claimed → closed; removed startup replay
  compaction and proved all 20 active/archived fixture events publish.
- `ptp` `implement-wal-migration`: claimed → closed; known WAL versions recover,
  collisions refuse, corrupt/future inputs fail without changing bytes, and
  production recovery cannot publish a partial roster.
- `i8g` `verify-lossless-restarts`: claimed → closed; full relevant suites,
  release builds, and four manual mutation controls passed.
- `vdr` `prevent-split-brain`: added after emergency process evidence, claimed
  → closed; OS lifetime server lease and exclusive per-WAL writer locks passed
  real multi-process and byte-preservation guards.
- `f5u` `integrate-document`: claimed → closed; verified commit `8eef3b1`
  merged to local `main` as `077b5e1`, then the competing-server and WAL
  matrices passed again from merged main.
- `3nk` `final-worklog`: claimed → closed; complete graph and merged-main
  evidence captured in this artifact.
- `ccd` `omega`: claimed → closed; output: all durability, exclusivity,
  documentation, integration, and no-activation requirements were confirmed.

### Notes

- Emergency scope expansion was recorded on graph `xtt`: three servers held the
  same Fable WAL and three Claude processes shared one ACP identity. At explicit
  operator direction, older servers were terminated; the GUI-connected server
  was not restarted.
- `cargo fmt --all --check` exposes broad pre-existing formatting drift. No
  unrelated bulk formatting was applied; `git diff --check` passes.
- `cargo-mutants` is unavailable. Four manual production-predicate mutations
  were observed RED and restored before the green gate.
- No fixed binary was activated and no surviving live Yalda process was
  restarted.
- Finalization-order deviation: the validator requires a complete graph while
  final-worklog/omega require the final artifact. Cog note `finalization-order`
  records that both nodes were closed from established merged-main evidence,
  followed immediately by this truthful snapshot and validation.

### Final status

- Status: `complete`

```text
graph make-restarts-session-wal-lossless (frontiers)
frontier 0: audit-restart-loss [done], prevent-split-brain [done]
frontier 1: audit-backup-replay [done], spec-lossless-invariant [done]
frontier 2: guard-old-wal [done], harden-restart-entrypoints [done]
frontier 3: implement-wal-migration [done]
frontier 4: verify-lossless-restarts [done]
frontier 5: integrate-document [done]
frontier 6: final-worklog [done]
frontier 7: omega [done] (omega)
```

## Built (with status)

- `VERIFIED`: v1/v2/v3 WALs recover without losing identity or valid events.
- `VERIFIED`: startup publishes every recovered active and archived event.
- `VERIFIED`: unknown versions and malformed interior records fail closed while
  preserving original bytes; only a torn unterminated final record is bounded.
- `VERIFIED`: create collisions cannot truncate an existing WAL.
- `VERIFIED`: a server lifetime lease is acquired before socket/WAL/agent work.
- `VERIFIED`: every WAL create/reopen holds one exclusive live-writer lock.
- `NOT ACTIVE`: release binaries were built but never launched.

## Open / unresolved

- Existing live WALs exposed to the historical multi-writer incident require a
  separate read-only forensic audit before any operator-controlled restart.
- Manual repair backups remain untouched and are never auto-merged.
- Repository-wide rustfmt drift and unavailable `cargo-mutants` predate this
  change.

## Decisions

- Durable WAL bytes are sacred: recovery is read-only and never substitutes a
  summary for an exact known-format replay.
- Ambiguous/corrupt recovery fails closed rather than publishing a guessed or
  incomplete roster.
- Server ownership uses a persistent-inode OS lease, not socket/PID observation.
- Per-WAL locking remains defense in depth even with the server lease.

## Verification status

- Library: **218 passed, 0 failed, 2 ignored**.
- Session server: **68 passed, 0 failed**.
- Session resilience: **11 passed, 0 failed**.
- Session transcript: **14 passed, 0 failed**.
- WAL matrix: **18 passed, 0 failed**.
- Competing real server: loser stopped at lifetime lease; owner socket inode and
  service remained intact.
- Merged-main critical rerun: competing-server guard and all **18** WAL tests
  passed.
- Release build: `yalda-session-server` and `yalda-gpui` passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-30-lossless-restart-wal.md`: passed.

## Next

- Activation remains pending explicit user-controlled restart permission. Do not
  restart the live GUI or server automatically.
