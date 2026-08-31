# bug-0064: restart-discards-or-partially-replays-wal

**Status:** FIXED
**First seen:** 2026-08-30
**Component:** `docs/components/agent-tile/session-binding.md` (`UXI-AgentTile-44`)

## Symptom

After a Yalda/session-server restart, some sessions disappear and some surviving
sessions replay only a suffix of their durable transcript. Manual repair backups
exist for earlier session-identity damage, but production recovery does not
discover them and must not guess how to merge divergent histories.

A concurrent-launch race also allowed several servers to unlink and bind the
same socket path in succession. Each recovered and resumed the same durable
sessions, producing multiple live agents with one ACP identity and multiple
processes holding the same WALs open for append.

## Context / root cause

Two independent production policies violated the durability contract:

- `session_wal::recover_one` returned `Ok(None)` for every header version other
  than the current v3, intentionally dropping the complete session on upgrade.
- `restore_seed_from_disk` called `event_log_from_recovery`, which applied the
  live in-memory cap before any client could attach. A real session with 159,874
  WAL lines was published with 37,501 events while the server logged
  `recovered_dropped=122296`.

Additional destructive risks found in the same audit are fresh creation using
`truncate(true)` on an existing identity and recovery silently skipping malformed
interior records. The v1→v2 control rename never affected WAL-appended records;
the v2→v3 event transition was additive and the current reader retains the legacy
variants, so discarding those known versions had no technical necessity.

The existing single-instance check used socket existence plus a trial connect
as a lock. That check and the later unlink/bind were not atomic. A transient
connect failure or two simultaneous launchers could both decide the pathname
was stale, unlink a live server's socket, recover the same roster, and open the
same WALs. No OS lease protected either server ownership or individual writers.

## Planned solution

Make restart recovery exact and read-only: replay every valid record from known
v1/v2/v3 WALs, remove pre-attach compaction, refuse create collisions, distinguish
only a torn final line from interior corruption, retain unsupported/corrupt files,
and prove the contract through real recovery/server-start paths plus mutation
controls. Never touch the live GUI, server, agents, or user WAL files while fixing.

Acquire a non-blocking OS lifetime lease before inspecting or unlinking the
socket and before WAL recovery or agent spawn. Keep the lease inode persistent
across shutdown. Independently lock every open WAL writer so even a future
server-ownership regression cannot create concurrent appenders.

## Approaches already tried (do NOT repeat)

- Replacing a durable prefix with `CompactedSummary` is not complete recovery;
  keeping bytes on disk while hiding them from the session is still data loss.
- Provider `session/load` is not a substitute for the exact Yalda WAL.
- Automatically selecting or merging manual backups is unsafe when active and
  backup histories may diverge.
- A socket pathname and PID text are observations, not mutual exclusion. Never
  use connect-then-unlink as the single-instance primitive.

---

## Log

### 2026-08-30 — loss paths localized; adversarial repair in progress

- Read-only live evidence confirmed 122,296 valid events omitted from one
  restart replay; no live process or WAL was modified.
- Historical-version preservation guard observed RED because v1 returned
  `Ok(None)`.
- Complete active/archived replay guard observed RED with 14 of 20 fixture
  events dropped by startup compaction, then passed with compaction removed.
- Remaining implementation and the expanded durability proof matrix are tracked
  in Cog graph `xtt`.

### 2026-08-30 — split-brain reproduced and structurally blocked

- Read-only process evidence found three Claude processes sharing ACP identity
  `9be662f6-9b47-41b8-b927-47599cb58148` and three session servers holding the
  same Fable WAL open concurrently.
- At explicit operator direction, older server processes were terminated; the
  current GUI-connected server was not restarted.
- The server now acquires an OS lifetime lease before socket/WAL/agent access.
- Every `SessionWal` create/reopen takes an exclusive writer lock.
- A real two-process regression proves the losing server cannot replace the
  socket inode and the owner continues serving; the full WAL unit matrix proves
  the second writer is refused without changing bytes.
