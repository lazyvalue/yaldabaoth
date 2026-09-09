# Worklog: wal-history-streaming

**Date:** 2026-09-09
**Branches touched:** `wal-history-streaming` (worktree) → fast-forwarded into `main`

Follow-on to the same-day
[bug-0064 recurrence session](2026-09-09-wal-interior-corruption-skip.md):
Scott asked what the 7.2 GB recovery peak was and picked "do 1 and 2"
(metadata-only archived sessions; lazy history for live sessions). Both land
as one model — history streams from the WAL, nothing recovered is resident —
recorded as ADR-0038.

## Cog execution evidence

- Graph id: `aop`
- Deferred alternatives 3 and 4 live OUTSIDE the graph, per Scott: Cog
  bulletin `an7` at `yaldabaoth/session-server::follow-ups` (author address
  `yhq`, registered this session as `claude-code`).

### Initial render

```text
graph wal-history-streaming (frontiers)
frontier 0: wal-summary-scan [open], adr [open]
frontier 1: lazy-seed [open]
frontier 2: prefix-streaming-attach [open], archive-fold [open]
frontier 3: guards [open]
frontier 4: verify-integrate [open]
frontier 5: worklog [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `k1pi` `wal-summary-scan`: claimed → closed; output: `walk_wal` (one
  line-walker: metadata fold + per-Event callback with early stop);
  `recover_one` folds `durable_events` / `next_generation` / `turns` / acp id
  with no `Vec` retained; `stream_durable_prefix(path, len, sink)`;
  test-only `read_all_events`; `RecoveredSession.event_log` removed;
  fidelity test `summary_and_prefix_stream_agree_with_a_full_decode`.
  `cargo test --lib session_wal` 23 passed.
- `qom2` `adr`: claimed → closed; output: ADR-0038 written.
- `oxzt` `lazy-seed`: claimed → closed; output: `restore_seed_from_dir(dir)`
  seam; resident log seeded `from_recovered(vec![], durable_events)` with
  `durable_prefix`; generation from `next_generation`;
  `recovered_stream_position` + `event_log_from_recovery` deleted;
  `AdminSessionInfo.durable_events`; recovery tests rewritten to drive the
  real seed path over a tempdir. Server bin 69 passed.
- `0j3p` `prefix-streaming-attach`: claimed → closed; output: `AttachGrant`
  carries the prefix on a `FromBase` resolution; `stream_prefix_to_client`
  (blocking reader → ~1 MiB batches over a 2-slot channel → `write_frames`
  under the slow-sub timeout, `apply_stop_action` between batches, loud
  ERROR on a short file). NC1 (grant forced `None`): restart replay `[]` vs
  six chunks, archived `0` of `20000`, bin grant test RED.
- `5b7l` `archive-fold`: claimed → closed; output: archive sets
  `durable_prefix.len = tip_seq` and empties the resident log; archive test
  extended (resident 0, prefix len 1, admin `durable_events` 1).
- `ggfk` `guards`: claimed → closed; output: two real-binary guards (below),
  stable 3/3; NC1 + NC2 RED with exact text recorded on the node.
- `13cl` `verify-integrate`: claimed → closed; output: see Built /
  Verification.
- `57ex` `worklog`: claimed → closed; output: this file + backlog; evidence
  check passed.
- `bjp1` `omega`: claimed → closed; output: on `main`, release server binary
  built, NOT activated.

### Notes

- None on the graph — the one cross-cutting decision is ADR-0038; the
  deferred items are the bulletin above (by Scott's instruction, not a graph
  note).

### Final status

- Status: `complete`

```text
graph wal-history-streaming (frontiers)
frontier 0: wal-summary-scan [done], adr [done]
frontier 1: lazy-seed [done]
frontier 2: prefix-streaming-attach [done], archive-fold [done]
frontier 3: guards [done]
frontier 4: verify-integrate [done]
frontier 5: worklog [done]
frontier 6: omega [done] (omega)
```

## The model (ADR-0038, one paragraph)

Boot folds a summary per WAL (O(1) memory); the resident `EventLog` starts
empty at `log_base = durable_events`; logical positions `[0, len)` are the
session's *durable prefix* on disk. A from-base attach streams that prefix
through the forwarder straight from the file, then tails the resident log
from `sent_seq = len`. Archive folds the resident tail into the prefix, so an
archived session holds zero events. Cursors, trim floor, high-water eviction
and `tip_seq` are untouched — they were always over logical seqs.

## Built (with status)

- `src/session_wal.rs` — `walk_wal`, summary-only `recover_one`,
  `stream_durable_prefix`, `RecoveredSession { durable_events,
  next_generation }` (no `event_log`).
- `src/bin/yalda-session-server/main.rs` — `DurablePrefix`, `AttachGrant`,
  lazy seed, `restore_seed_from_dir`, prefix streaming in the forwarder
  (`stream_prefix_to_client`, `write_frames`, `apply_stop_action`), archive
  fold, admin `durable_events`.
- `src/session_proto.rs` — `AdminSessionInfo.durable_events` (serde default).
- `docs/decisions/0038-wal-history-streams-from-disk.md`.
- Guards: `session_wal::summary_and_prefix_stream_agree_with_a_full_decode`;
  bin `from_base_attach_grants_the_durable_prefix_and_tail_attach_does_not`,
  `recovery_keeps_history_on_disk_and_seeds_an_empty_resident_log_at_the_durable_base`,
  archive test extended; real binary
  `restart_replays_full_history_from_disk_without_a_resident_copy`
  (stub-agent turn → kill → reboot on the same WAL dir → admin shows
  `log_base == durable_events`, resident < CHUNKS → attach → identical chunk
  texts in order → still nothing materialised → log line `attach: durable
  prefix streamed from disk`) and
  `archived_session_stays_cold_across_restart_and_still_replays_in_full`
  (20k-event archived WAL → resident 0 / durable 20001 → attach streams all
  20000 in order, no dups → resident still 0).
- Suites on the branch: `--lib` 224 · `--bin yalda-session-server` 69 ·
  `--test session_resilience_test` 17. GUI: `cargo check --bin yalda-gpui
  --tests` clean.
- On `main` after the fast-forward + release build: see Verification.

## Open / unresolved

- **Deploy** — `./deploy-server.sh`; backlog `NEEDS-RUNTIME`. One deploy
  covers this and the bug-0064 skip fix.
- Gap 3 (wall-clock/RSS): the memory win is proven structurally (resident
  count 0 through a 20k-event stream, real binary) but the production number
  (peak RSS on the 55-session roster) needs the deploy — check
  `systemctl --user status yalda-session-server` afterwards.
- Attach cost moved to first attach per session and is paid again on every
  from-base reattach (e.g. a GUI restart re-streams its open sessions). Same
  work the old boot did, spread out, O(batch) memory. If it ever shows as
  latency, item 3 in the bulletin (raw wire bytes) is the lever.
- `admin_prompt_works_with_no_client_attached` flaked once in a full
  resilience run (passes alone on the branch and on `main`; two later full
  runs green). Not touched by this change; noted, not chased.
- `cargo-mutants` still not installed on this host; two hand negative
  controls instead.

## Decisions

- ADR-0038 — recovered history streams from the WAL; the resident event log
  holds only post-boot events.

## Verification status

- Real-path guards on the actual boot (`restore_seed_from_dir` → real
  binary), attach (`do_attach` → `forward_notifications`) and archive paths;
  both negative controls observed RED with the production symptom.
- Suites on `main` @ 456fc3e (main checkout): `--lib` 224 passed / 2
  ignored · `--bin yalda-session-server` 69 · `--test
  session_resilience_test` 17. `cargo build --release --bin
  yalda-session-server` → `target/release/yalda-session-server` 01:31:59 PDT
  (15.3 MB); `~/.local/bin/yalda-session-server` is still the 00:46 pre-fix
  binary and the running service was not touched.
- Not runtime-verified: the fixed binary against the real `~/.yalda/wal`
  (gap 2/3 — the deploy Scott owns).
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-09-wal-history-streaming.md` passes.

## Next

- Scott: `./deploy-server.sh`, then watch peak RSS and the first attach of
  the 320 MB session.
- If a GUI restart's re-stream of ~26 sessions feels slow: bulletin item 3.
