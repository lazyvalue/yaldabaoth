# ADR-0016: Ring-buffer the in-memory event_log instead of snapshot compaction

**Status:** Accepted
**Date:** 2026-06-08
**Related:** ADR-0009 (durable log; deferred snapshot-compaction), spec-event-stream.md §6, phase 8 (`phase8-eventlog`)

## Context

ADR-0009 deferred `event_log` compaction "until a long session measurably hurts
memory or re-attach latency," and noted that when it lands `seq`/`replay_fence`
become logical offsets. spec-event-stream.md §6 specified the `epoch =
(generation, log_base)` predicate for exactly that. The in-memory `event_log` is
an `Arc<Vec<Notification>>` that grows unbounded per session: a long session
pins memory, and every re-attach replays the whole thing. Phase 8 needed a bound,
and the user asked "can we just ring-buffer this?"

## Decision

Bound the **in-memory** `event_log` with a ring buffer (`EVENT_LOG_CAP`,
env-overridable), **not** a snapshot+tail file-compaction. `log_base` is a
logical `seq` offset — the entry at Vec index `i` has `seq = log_base + i`;
front-trimming advances `log_base`. A trim **prepends a `CompactedSummary`
marker** (not a silent drop) and decrements `log_base` so survivor seqs stay
stable. The §6 epoch predicate routes a client whose `acked_seq < log_base` to a
from-base rebuild. The **owner (lease holder) is a hard ceiling** — never trim
past its forwarded position; a forwarder past a **high-water bound** is
disconnected (spec §6 disconnect-before-gap) so a wedged/App-Napped owner can't
pin growth. The **on-disk WAL stays append-only/unbounded** — ADR-0009's
durability contract is unchanged; disk compaction stays deferred.

## Rationale

A ring is the simplest bound that solves the *actual* cost (memory + re-attach
replay), and it composes with the §6 epoch predicate and the just-shipped phase-5
cursor reconnect because `log_base` lives in the same `seq` space the cursor
already speaks. Disk isn't the pressure (NDJSON transcripts are tiny), so the
heavier snapshot+tail file-compaction buys nothing now. The `CompactedSummary`
marker keeps a trim honest — the reducer renders "history compacted," never a
silent hole.

## Alternatives rejected

- **Snapshot + tail file compaction** (ADR-0009's original yalda) — heavier;
  solves disk growth, which isn't the pressure. Still deferred.
- **Drop-oldest with no marker** — silent history holes; violates the reducer's
  "no silent gap" contract (§6/§7).
- **Leave it unbounded** (status quo) — pins memory and makes re-attach O(n) in
  the full transcript for long sessions.

## Consequences

Bounds memory and re-attach cost. `log_base` is now a logical offset everywhere
— the forwarding/reconnect cursor is `log_base + vec_index` via `seq_of` /
`resolve_cursor`, never a raw Vec index (an in-code comment warns that
per-generation `event.seq` is NOT the cursor). New invariant: **never trim past
the owner or the high-water-protected slowest forwarder.** The full
`min(acked_seq)` floor (subscribers reporting `acked_seq` upstream) is a
measured §11 follow-up; the shipped version is cap-trim + owner-ceiling +
high-water disconnect. On-disk WAL compaction remains deferred (ADR-0009).
