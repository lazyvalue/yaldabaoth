# Worklog — 2026-06-07 — Durable per-session WAL (crash survival)

The #1 gap for "agents keep running with no GUI attached": the server persisted
sessions only on a *graceful* shutdown (JSON snapshot on SIGINT/SIGTERM), so a
crash — `kill -9`, OOM, panic, power loss, reboot — lost every session and its
transcript since the last clean exit. This implements ADR-0009's durable
write-ahead log so a crash no longer loses completed work. Branch: `durable-wal`.

## What shipped

- **`src/session_wal.rs`** (new lib module, unit-tested): a per-session
  append-only NDJSON WAL. First line is a versioned `Header` (the metadata not in
  the event stream — label, cwd, permission_mode); every later line is one
  `Notification` in `event_log` order.
  - **Durability contract (ADR-0009):** `write()` each event immediately (no
    userspace buffering → a *process* crash loses nothing, the OS flushes);
    `sync_data` only at **turn boundaries** (`UserPrompt`, `TurnEnded`) — never
    per streamed chunk. Guarantee: never lose a completed turn or a sent prompt;
    worst case on power loss is an in-flight stream tail truncating.
  - **Recovery** replays the file; a torn final line (interrupted write) is
    skipped, not fatal. `acp_session_id` (to `--resume` the agent) is re-derived
    from the last `SessionAttached`; `turns` from `TurnEnded` count.
- **Server integration** (`sketch-session-server/main.rs`):
  - `ManagedSession` gains a `wal` handle; `record()` and the user-prompt path
    (`log_only`) append durably, fsync at boundaries.
  - `create_session` opens the WAL (header fsync'd up front). `close_session`
    deletes it (explicit close = don't recover). Crash/disconnect leave it.
  - `restore_from_disk` now recovers from the WAL dir (replaces the JSON
    snapshot + the delete-on-restore hack); re-spawns each agent via
    `session/load`. Graceful shutdown no longer needs a special save — the WAL
    is always current.
  - WAL dir follows `SKETCH_SESSION_SOCKET` (`session_wal_dir()`), so test and
    alternate instances never share durable state.

## Decision recorded first

Before building, surveyed the Rust actor-framework landscape (ADR-0012): adopt
none — hand-roll the planned actor with bare `tokio::mpsc` + `oneshot`. actix is
disqualified by an `actix-rt` vs multi-thread-tokio runtime conflict; ractor/kameo
are capable but their value (supervision trees, clustering) is unused at our
one-manager scale, and our hardest point (the `!Sync` sync→async bridge) is better
hand-rolled via `Sender::blocking_send`. The WAL here is the single-writer's durable
substrate regardless of how the actor lands.

## Verification (headless)

- `session_wal` unit tests: roundtrip, **torn-final-line tolerance**, reopen,
  remove (4 passed).
- `session_recovered_after_server_crash` (new, in `session_transcript_test.rs`):
  complete a 6-chunk turn → **SIGKILL the server** (no graceful shutdown) →
  start a fresh server on the same socket+WAL → assert the session reappears and
  the full transcript (6 chunks + UserPrompt + TurnEnded) replays from the WAL.
  Stable across repeated runs.
- Full suites green: transcript (5), resilience (4), lib (86). All bins build.

## Notes / what's next

- This is a plain append-only log replayed in full on recovery. **Snapshot +
  compaction is deferred** (ADR-0009) until a long session measurably hurts
  memory or recovery latency.
- Remaining "no-GUI" gaps (spec-session-server-actor § Rollout): **launchd
  supervision + start-at-boot** (phase 7 — so the server is always-present and
  crash-restarted independent of any GUI) and the **actor extraction** (phase 3).
  Mid-turn crash recovery (turns==0 at crash, fence==0) is untested — the
  agent's `session/load` replay could double-log without a fence; worth a test
  when the actor/event-stream `seq` work lands.
