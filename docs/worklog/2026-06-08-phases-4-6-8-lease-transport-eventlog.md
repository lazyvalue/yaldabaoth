# Worklog: session-server phases 4 / 6 / 8 — lease, transport seam, eventlog

**Date:** 2026-06-08
**Branches touched:**
- `master` (`f882317`) — merged phase 4 (`4beebe6`) + phase 6 (`9818517`); base commits `ba12d5d`, `b0375e9`, `1f80296`.
- `phase8-eventlog` (`922aa84`) — phase 8, **NOT merged** (awaiting v2→v3 WAL cutover).

Three phases of `spec-session-server-actor.md` / `spec-event-stream.md`, each run as
Workflow (map → design judge-panel → staged implement → verify) → adversarial review
→ fix pass(es) → **independent** re-review. Pattern paid off repeatedly: green build +
green tests but a `BLOCKING`/`MAJOR` review nearly every time — the reviews caught real
bugs the tests structurally missed.

## Built (with status)

- **Phase 4 — lease ownership** — ✅ MERGED `master` (`4beebe6`, base `ba12d5d`).
  `owner: conn_id` → `Lease{client_id, expires_at: Instant}` + 5s client heartbeat /
  15s TTL (dual-clock: actor owns monotonic `Instant`, wire carries display millis);
  stable per-install `client_id` (`~/.cache/sketch/client_id`, `SKETCH_CLIENT_ID`
  override for blue-green); `attach_owner_with_retry`→`attach_for_role` (deterministic
  same-`client_id` reclaim); wire `OwnerChanged→LeaseChanged`; WAL 1→2, **discard** v1.
  Workflow `wf_c45c440b-aac` → review `BLOCKING` (2 client races: owner-gap-after-promote;
  observer heartbeat steal/churn) → fixed (unconditional beater + per-tick `is_driver`
  self-gate; `is_driver` on `AgentSlot`) → re-review `MINOR`, both closed, found a leaked
  (non-singleton) beater → fixed (singleton `_lease_heartbeat` Task). 17 + 8 headless pass.
  **RUNTIME-VERIFIED in-app** (see Verification).

- **Phase 6 — `AgentTransport` seam** — ✅ MERGED `master` (`b0375e9` + `1f80296`).
  Object-safe sync `AgentTransport` trait + `AgentSpawner`/`RealAgentSpawner`; feature-gated
  (`test-support`) in-process `FakeTransport`/`FakeAgentControls`/`FakeAgentSpawner`; new
  `tests/agent_transport_fake_test.rs`. Real subprocess path byte-identical;
  crash/WAL/socket/back-pressure tests kept subprocess-backed. Workflow `wf_6ead8955-d04`
  → review `MINOR` (fake's `complete_turn` emitted a `TurnEnded` record the *default* worker
  doesn't — gated behind `SKETCH_EMIT_TURN_ENDED=1`) → fixed (`complete_turn` counter-only;
  opt-in `emit_turn_ended_event`). `1f80296` is an incidental pre-existing `tree_test` API-drift
  fix surfaced by running the full suite. Behavior-preserving. Substrate for phase-8 headless
  reducer tests.

- **Phase 8 — full eventlog refactor** — ✅ COMPLETE on `phase8-eventlog`, **not merged**
  (`b740386` refactor + `e19b9d7` stuck-turn fix + `922aa84` tests). Collapse
  `Notification::{ReplyEvent,TurnEnded,UserPrompt}` + `WorkerEvent::Reply` → one `AgentEvent`
  (`src/agent_event.rs`, byte-preserving `Unknown{tag,raw}`); emit chokepoint (worker stamps
  gen/turn, server `record()` assigns durable seq); generation-on-`ChannelOpened`; WAL 2→3
  **discard**; **ringbuffer compaction** (`log_base` logical offset, §6 epoch predicate into
  phase-5 cursor seq-space, `CompactedSummary` trim marker, owner hard-ceiling + spec §6
  high-water disconnect-before-gap, on-disk WAL append-only); GUI **total reducer** over
  `AgentEventKind` + idempotent finalize, **additive** per §9 (old inference behind a gate).
  Workflow `wf_73656668-97f` → review `BLOCKING` (live forwarder gapped owner across a trim;
  marker prepend shifted seq +1; finalize ledger keyed 0-vs-1-based) → fixed → re-review
  `SOLID`, found `MAJOR` (no high-water bound → App-Napped owner pins in-memory growth) →
  fixed → **live runtime check found a real §9 bug** → fixed → re-review `SOLID`.
  Full `--features test-support` suite green.

- **`scripts/rebuild-server.sh`** — ✅ dev tool, verified working. Rebuilds + relaunches the
  daemon; GUI-aware (a running GUI respawns the binary next to *it*, so the script builds the
  running GUI's checkout and lets it respawn — avoids the v2/v3 mismatch). Modes: `$SKETCH_REPO`
  override / auto-detect running GUI / standalone.

## Open / unresolved (see `docs/backlog.md`)

- **Phase 8 merge = the v2→v3 WAL cutover** — `NEEDS-DECISION` (timing). It discards v2 sessions
  (same as the v1→v2 wipe); do at a quiet moment. WAL is socket-scoped (`session_wal_dir()`
  follows `SKETCH_SESSION_SOCKET`), which is what let the runtime check run isolated.
- **Phase 8 owed runtime checks** — `NEEDS-RUNTIME`: (1) GPUI paint confirm — the now-`Idle`
  spinner visibly clears on screen after a resume+live-prompt (fold/`turn_phase` proven correct
  headlessly; only the paint is unverifiable); (2) App-Nap-paused-owner high-water eviction →
  clean reconnect + lease reclaim in the live app.
- **Phase 8 deferred follow-ups** — delete the §9 gated old-inference after real-session soak;
  latent `event.seq` vs `seq_of` divergence (only bites when phase-5 cursor is wired client-side
  — commented in code).
- **App Nap lease limitation** — accepted/documented. Two windows of the same install on one Mac:
  the backgrounded owner's heartbeat is App-Nap-throttled → lease lapses (~15s) → ownership
  follows focus. Fails safe (no double-drive). The blue-green `:promote` loop (the real case) is
  unaffected. Memory: `lease-app-nap-limitation`.
- **Worktree cleanup** — `phase4-lease` + `phase6-transport` worktrees are merged and can be
  removed; `phase8-eventlog` stays until its merge.

## Decisions
- No new ADRs written this session; the design lives in `spec-event-stream.md` (§1–§12) and
  `spec-session-server-actor.md`. Candidates worth an ADR if we want the *why* pinned:
  **(a)** ringbuffer compaction replacing ADR-0009's deferred snapshot-compaction; **(b)** the
  WAL-version-bump **discard** migration policy (no converter). Offer `/decision` next session.

## Verification status
- **Phase 4:** RUNTIME-VERIFIED in the real app (2026-06-08) — clean v2-daemon spawn + live
  v1-WAL discard; heartbeats accepted (no `bad frame`); idle-then-prompt holds the lease (no
  false expiry); `:promote` self-hosting handoff textbook (observe → handoff-on-close → promote
  → drives past >15s). Confirmed via daemon log + user drive.
- **Phase 6:** headless only (behavior-preserving; real path byte-identical).
- **Phase 8:** headless-complete; the §9 stuck-turn reproduced + fixed + re-reviewed at the fold
  level. **Residual is paint/runtime only** — GPUI not headless-drivable, so the two owed checks
  above are the harness gap, not a correctness gap.
- The **live runtime check earned its keep**: it found the §9 stuck-turn bug (a live prompt after
  a resume hung in "thinking" — `ReplayEnd`'s server-stamped `turn` aliased the next live turn's
  finalize key, so finalize no-op'd and `turn_phase` never returned to `Idle`) that *no headless
  test had*. Now a permanent regression guard.

## Next
1. **Phase 8 runtime confirm** (~minutes, quiet machine): resume a session + send a prompt →
   spinner clears; then the App-Nap-owner reconnect check.
2. **v2→v3 cutover + merge** phase 8 to master once confirmed (nothing precious open).
3. Post-soak: **delete the §9 gated old-inference** (the additive dual-path was always meant to
   be temporary).
4. Housekeeping: remove the merged `phase4-lease` / `phase6-transport` worktrees; optionally
   `/decision` for the two design choices above.
