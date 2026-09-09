# ADR-0038: Recovered history streams from the WAL; the resident event log holds only post-boot events

**Date:** 2026-09-09
**Status:** accepted
**Related:** ADR-0009/-0018 (WAL durability), bug-0064 (restart fidelity:
no startup compaction), Cog graph `aop`, Cog bulletin
`yaldabaoth/session-server::follow-ups` (deferred alternatives)

## Context

On 2026-09-09 the session server's first restart in a week replayed 55
sessions (1.1 GB of WAL, 29 archived) and peaked at **7.2 GB RSS**, settling
at **1.9 GB**. `restore_seed_from_disk` decoded every WAL line into a
`Notification` (2–4× the JSON size once every string, `Vec`, and
`serde_json::Value` map is a separate heap object), collected each file into
a `Vec`, copied it into the `imbl::Vector` behind `EventLog`, and kept all of
it resident — for archived sessions too, which never append and therefore
never reach the live-path trim. The two largest files (320 MB live, 180 MB
archived) were half the total on their own.

Two contract constraints bound the fix:

1. **Restart fidelity is absolute** (bug-0064, 08-30). The first attaching
   client must receive every durable event in order. Startup compaction is
   off the table — it dropped 122k events once.
2. **The on-disk WAL is never trimmed.** It is already the complete,
   append-only source of truth; the resident log is a cache over it.

Given (2), holding a decoded copy of history in RAM buys nothing that the
file does not already provide.

## Decision

1. **Boot folds a summary, never a transcript.** `session_wal::recover_one`
   walks each file once through `walk_wal` and retains only metadata: identity,
   settings, prompt state, `turns`, the resume id, `durable_events` (the count
   of transcript records on disk) and `next_generation` (max durable `Agent`
   generation + 1). Memory is O(1) per file.
2. **The resident `EventLog` starts empty at `log_base = durable_events`.**
   Logical positions `[0, durable_events)` are the session's *durable prefix*
   and live on disk (`ManagedSession::durable_prefix`); live events append
   after it exactly as before. `tip_seq`, cursors, the trim floor, and the
   high-water eviction are unchanged — they were always defined over logical
   seqs, not `Vec` indices.
3. **A from-base attach streams the durable prefix from the file.** When
   `do_attach` resolves the cursor to `FromBase`, the forwarder first streams
   `[0, len)` through `stream_durable_prefix` — one record decoded at a time,
   serialised into ~1 MiB batches over a bounded channel from a blocking
   reader — then continues tailing the resident log from `sent_seq = len`.
   The appender only ever writes past `len`, so the read is race-free against
   a live writer. A file shorter than `len` streams what exists and logs an
   error naming the shortfall: loud, never a silent gap.
4. **Archive folds the resident tail into the durable prefix.** Once the
   archive marker is durable and the WAL handle was open for every push,
   `durable_prefix.len = tip_seq` and the resident log is emptied: an archived
   session holds zero events. Unarchive reopens the WAL and is served by (3).
   A WAL-degraded session (`wal == None`) keeps its resident log — its events
   never reached disk.

## Consequences

- Boot memory no longer scales with transcript size; steady-state memory
  scales with the *post-boot* tail (bounded by `event_log_cap`) of live
  sessions only.
- The decode cost moves from boot to first attach per session and is paid
  again on every from-base reattach (a GUI restart re-streams its open
  sessions). This is the same work the old boot did, spread out and without a
  resident copy; it was already paid on the wire by the GUI.
- `recover_one` and `stream_durable_prefix` share one reader, so the
  fidelity property "prefix streamed == events that would have been loaded"
  is pinned by a test over the same walker rather than two parsers.
- `AdminSessionInfo` reports `durable_events` alongside the resident
  `event_log_len` / `log_base`, so an operator can see the split.
- A future trim of the resident tail after a from-base stream is consistent:
  the `CompactedSummary` marker covers `[len, new_base)`, never the prefix.

## Alternatives rejected

- **Materialise on attach into RAM** (read the file into the resident log at
  first attach). Restores the 1.9 GB steady state as soon as the GUI attaches
  its open sessions, and blocks the actor for the read. Streaming through the
  forwarder costs the same decode with O(batch) memory and no actor stall.
- **Metadata-only for archived sessions alone.** Halves the problem but leaves
  every live session's full history resident. Subsumed by (2)–(4).
- **Startup compaction.** Forbidden by bug-0064's fidelity contract.
- **Resident events as wire bytes** and **bounding / not replaying into the
  Cog bridge** — deferred, recorded as Cog bulletin
  `yaldabaoth/session-server::follow-ups` (items 3 and 4).
