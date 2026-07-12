# 2026-07-11 — Event-log O(n²) append stall (the "FoF spooling" bug)

## Symptom (user report)
A live session ("FoF") appeared to spool agent messages that only released in
bursts when the user sent new prompts; the agent seemed idle until poked.

## Diagnosis (live server probe)
Queried the running `yalda-session-server` over its Unix socket (read-only
`admin_status`/`list_sessions`):

- `ping` hung 12s and `list_sessions` took 5s **during** FoF activity, but both
  were instant when FoF was idle → the single-writer actor stalls for multi-
  second stretches, starving all forwarders; events then flush in a burst.
- FoF `event_log_len = 28,461` — 5–7× any other session (next: 5,549), `gen=1`
  (force-restarted once), `log_base=0` (never trimmed; cap is 50k so it hadn't
  crossed the high-water yet).

## Root cause
`ManagedSession::push_event` (runs per event) ends with `publish_snapshot()`,
which clones the whole log onto the forwarder watch. The log was
`Arc<Vec<Notification>>`, so that clone left an outstanding `Arc` ref — and the
**next** push's `Arc::make_mut` copy-on-write **deep-cloned the entire Vec**.
O(n) per push → **O(n²) over a session**. A ~1,900-event FoF turn cloned a 28k
Vec ~1,900 times ≈ tens of millions of `Notification` clones → seconds of actor
stall per turn. Pre-existing; FoF was just the first log big enough to trip it.

## Fix (`bf6bbe8`, on `main`)
Back the log with `imbl::Vector` (persistent, structurally-shared RRB tree):
`clone()` O(1), `push_back` O(log n) even while a snapshot is shared. The
publish-snapshot-every-append design is now cheap. Seq/trim/cursor semantics
unchanged — all 9 existing `EventLog` tests pass. `snapshot()`/`entries()`
replaced by `tail_from(offset)` (clones only the new tail the forwarder flushes
anyway); forwarder updated to match.

## Verification
- `cargo test --lib event_log` → 11 pass (9 existing + 2 new).
- New guards: `snapshot_clone_is_unaffected_by_later_pushes` (immutable-snapshot
  contract); `append_stays_cheap_with_outstanding_snapshot` (20k shared appends
  < 5s). **Negative control:** injecting the old CoW (full clone per push) makes
  the perf guard take **33.2s** and fail — restored → 0.08s.
- Full suite green (lib 154 / gpui 359 / server 2); release server bin builds.
- Perf is genuine-gap #3 (a coarse timing proxy, not a precise gate), but the
  asymptotic gap is ~1000× so the guard cleanly catches a CoW reversion.

## To pick up in the running app
`rebuild and restart all` (rebuilds + restarts the session server). FoF resumes
from the WAL with its full log, but appends are now O(log n) so the stalls stop.
