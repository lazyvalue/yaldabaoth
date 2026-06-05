# ADR-0009: Durable per-session event-log subsystem (D4)

**Status:** Accepted
**Date:** 2026-06-05
**Related:** ADR-0006 (the stream), spec-event-stream.md (durable instance), spec-state-architecture.md (D4)

## Context

The session server checkpoints `session_server.json` **only on clean shutdown**
(SIGINT/SIGTERM) and deletes-on-restore. So a crash — power loss, OOM, `kill -9`,
or a panic (and the 25 un-poison-guarded `.lock().unwrap()` make the mutex-poison
cascade one panic away) — loses **every session and its entire transcript since
the last clean exit**, presenting as "resumed into an empty session."

## Decision

Build a **durable per-session event-log subsystem**, treating the `event_log`
(already the source-of-truth, ordered, append-only stream) as what it is:
- **Append-only write-ahead log** via the fused `record()` (quick win #4), with
  **periodic atomic snapshots** (temp + `rename`) for compaction. Logical offsets
  (`seq`) are first-class (not Vec indices).
- **Durability contract:** every event is `write()`-n immediately (a *process*
  crash loses nothing — the OS still flushes); `fsync` at turn boundaries
  (UserPrompt, TurnEnded). **Guarantee: never lose a completed turn or a sent
  prompt; the worst case is an in-flight stream tail truncating on power loss.**
  No fsync-per-token.
- **Resume = latest snapshot + log-tail replay** (idempotent); removes the
  delete-on-restore hack.
- **Log compaction/bounding is deferred** until a long session measurably hurts
  memory or re-attach latency — keeps `seq`/`replay_fence` as simple absolute
  offsets until then.

## Rationale

Event-sourcing persistence (durable log + snapshot) is the correct shape for an
append-only event stream and aligns with ADR-0006 and the event-stream spec
(this is its durable instance). Pairs with app-side persistence (D5/ADR-0010):
the app records *which sessions were open where* by id, the server durably owns
the *transcripts* — together, resume-anytime by identity.

## Consequences

- Gets its own implementation spec. Defense-in-depth with the mutex-poison fix
  (a separate error-handling item that *reduces* crash frequency; the log
  survives crashes regardless of cause).
- When compaction lands, `seq`/`replay_fence` become logical offsets (epoch
  predicate in spec-event-stream.md §6).
