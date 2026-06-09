# Backlog

Open, deferred, and flagged work. Higher fidelity than git: each item says what,
why it's here, and its status. Updated at session end (`/worklog`) and as items
move. Past work lives in `docs/worklog/`; the *why* of choices lives in
`docs/decisions/`.

Status legend: `IN-FLIGHT` (agent/branch active) · `READY` (scoped, not started)
· `DEFERRED` (deliberately not now, reason given) · `NEEDS-DECISION` (waiting on
the user) · `NEEDS-RUNTIME` (built, awaiting human runtime verification).

---

## Bugs

- **Edit-view typing crash + latency** — `FIXED` (2026-06-06). `reparse` fed
  tree-sitter a stale (never-`edit()`'d) tree → nondeterministic SIGSEGV
  (`d32edf9`); then full-parse-per-keystroke was slow → proper incremental
  reparse, fuzz-guarded, 10–20× faster (`413da19`). See worklog
  `2026-06-06-reparse-segfault-and-incremental.md`.
- **Reparse may be wasted work** — `READY` (small). The adversarial verifier
  noted the tree-sitter tree might be consumed "only in tests, not GPUI
  rendering." If true, the per-keystroke `reparse` could be made lazy/skipped
  for an even bigger win. Quick code-read, no runtime needed. (Incremental
  reparse already cut the cost 10–20×, so lower priority now.)

- **Session-server reconnect storm** — `ROOT-CAUSED + FIXED on branch
  session-resilience` (2026-06-07); `NEEDS-RUNTIME` for the GUI reconnect path.
  **Root cause:** `SessionServerClient` had no socket shutdown on drop. Its
  reader thread is detached and blocks forever on `lines()`, so dropping the
  client (notably `reconnect()`'s `*self = fresh`) leaked the thread AND kept
  the socket fd open — the **server never saw the disconnect, so it never
  released session ownership**. Every in-place `reconnect()` orphaned a zombie
  owner; the next re-attach was rejected with "another GUI already owns this
  session", and the connection only truly closed at process exit. That is the
  489-reconnects / few-closes pattern and the `close/create … disconnected`
  round-trip failures. **Fix (4 files):** (1) `Drop` now `shutdown(Both)`s the
  socket so the reader unblocks and the server releases ownership at once;
  (2) reconnect re-attach moved off the paint thread via the existing
  `spawn_attach_sessions` (Owner-reclaim retry) instead of raw inline blocking
  attaches that also froze rendering; (3) `attach_owner_with_retry` added to the
  client lib for the residual teardown-vs-reattach race; (4) single-instance
  guard — a 2nd server on a live socket exits instead of stealing it and
  orphaning sessions; (5) `pid_file_path`/persist path now follow
  `SKETCH_SESSION_SOCKET` (enables isolated instances + the guard). **Verified:**
  new headless harness `tests/session_resilience_test.rs` drives the REAL server
  binary (no agent needed) — reproduces the storm without the fix; with it, 30
  sequential restarts + in-place reconnect + duplicate-server guard all pass,
  every connection closes (no zombies). **Still owed:** human runtime check that
  the GPUI app reconnects seamlessly after the server reader thread sees EOF
  (GPUI can't be driven headlessly). Was suspected to be the attach-replay
  broadcast lag — that path was already self-healing; the real cause was the
  missing shutdown.

## Session-server hardening + actor extraction (all MERGED to `master`)

Phase-3 + phase-7 of `spec-session-server-actor.md`. All landed on `master`
(`bd796d4`,`1e2c881`,`03e8d10`,`23747a0`,`a70ef74`); branches deleted. Headlessly
verified via the resilience+transcript harness. Worklogs:
`2026-06-07-session-server-hardening.md`, `2026-06-07-actor-extraction-and-perm-ux.md`.

- **Permission default + `0600` socket** — `DONE` (ADR-0014 + addendum). Now
  **Yolo, config-driven** (`default-permission-mode` in config.kdl; server loads
  config in `create_session`); `DEFAULT_PERMISSION_MODE` is the no-config
  fallback. Socket `0600` (TOCTOU-closed via `umask`-around-`bind`). Owner-gated
  escalation. Runtime-confirmed by user.
- **Permission mode visible + cyclable in the server model** — `DONE` /
  runtime-confirmed. `AgentState.permission_mode` from `SessionInfo`; status-strip
  badge always renders; `<space> c m` cycles via the `SetPermissionMode` wire verb.
- **Structured tracing + `admin_status` verb** — `DONE`. Runtime-confirmed.
- **Actor extraction (phase 3, ADR-0012)** — `DONE` (`23747a0`). `Mutex<HashMap>`
  → single `run_manager` task (mpsc `Command` + oneshot); lock-free watch-based
  forwarder; pump owns the channel + forwards generation-stamped Commands.
  Behavior-preserving (conn_id ownership kept, no wire change); harness green 5×,
  test files unchanged, two adversarial reviews SOLID. Kills the shared-mutex race
  class + poison-tolerant lock.
- **Slow-subscriber disconnect** — `DONE` (`a70ef74`). All server→client writes
  bounded by a timeout (60s default; `SKETCH_SLOW_SUB_TIMEOUT_MS`); stuck peer
  dropped → reconnects + replays; owner never gapped.
- **Headless start-work verb** — `DONE` (`f3585b0`, ADR-0015). `Request::AdminPrompt`
  + `sketch-session-server prompt <sid> <text>` CLI + `SessionServerClient::
  {admin_prompt,connect_existing}`; ungated `enqueue_prompt` core shared with the
  owner-gated `do_prompt`. Headless prompt takes no lease; WAL-durable; runs under
  the session's stored permission mode. Test: `admin_prompt_drives_turn_without_owner`.
- **Cursor reconnect (phase 5, additive)** — `DONE` (`a3650a4`). Optional
  `cursor:(generation,index)` on `Request::Attach`; forwarder tails `[index..]` on
  generation-match+in-range, else full replay (additive; GUI untouched). Test:
  `cursor_reconnect_streams_only_tail`. **GUI cursor-wiring is NEEDS-RUNTIME** (have
  the GUI send its last cursor on reconnect; the transcript reconciler must be
  checked under tail-only streams — GPUI not headless-drivable).
- **Lease ownership (phase 4)** — `DONE` — runtime-verified, merging to master
  (branch `phase4-lease` → `ba12d5d`, 2026-06-08). `owner: conn_id` → `Lease{
  client_id, expires_at: Instant}` + 5s client heartbeat / 15s TTL; dual-clock
  (actor owns monotonic `Instant`, wire carries display-only millis); stable
  per-install `client_id` (`~/.cache/sketch/client_id`, `SKETCH_CLIENT_ID` override
  for blue-green candidates); `attach_owner_with_retry`→`attach_for_role`
  (deterministic same-`client_id` reclaim, retry/observer-fallback retired); wire
  `OwnerChanged→LeaseChanged`; WAL 1→2 with **discard** of v1. STAGED, not bundled
  with the eventlog collapse. **Verification:** workflow `wf_c45c440b-aac` (build +
  15/8 headless) → race review found 2 BLOCKING client races (owner-gap after
  promote; observer heartbeat steal/churn) → fixed (unconditional beater +
  per-tick `is_driver` self-gate; `is_driver` persisted on `AgentSlot`) → indep.
  re-review `MINOR`, both closed, found a leaked-beater → fixed (singleton
  `_lease_heartbeat` Task). Final: build clean, **17 + 8 headless pass**. **Runtime-verified
  (2026-06-08):** clean v2 daemon spawn + live v1-WAL discard; heartbeats accepted
  (no `bad frame`); idle-then-prompt holds the lease (no false expiry); `:promote`
  self-hosting handoff textbook (candidate observer-attach → original close →
  candidate promote → drives past >15s — the bug-1 owner-gap fix, confirmed in-app
  via daemon log + user drive). **Known limitation (App Nap):** two windows of the
  *same* install on one Mac — the backgrounded owner's heartbeat (collect step on
  GPUI's foreground executor) is throttled by macOS App Nap, so its lease lapses
  (~15s) and ownership follows focus. Fails safe (no double-drive / corruption).
  Acceptable per user — the self-hosting / blue-green loop is the real case and
  works; same-machine multi-window is the edge. Follow-up only if that matters
  (heartbeat off an App-Nap-immune timer / disable App Nap / longer TTL).
- **`AgentTransport` seam (phase 6)** — `DONE` / merging to master (branch
  `phase6-transport` → `b0375e9` + `1f80296`, 2026-06-08). `AgentTransport` trait
  (object-safe, sync, pump-facing) + `AgentSpawner` factory + `RealAgentSpawner`;
  `FakeTransport`/`FakeAgentControls`/`FakeAgentSpawner` in-process fake (gated
  `feature = "test-support"`); new `tests/agent_transport_fake_test.rs`. Real
  subprocess path byte-identical; crash/WAL/socket/back-pressure tests kept
  subprocess-backed. Workflow `wf_6ead8955-d04` → review `MINOR` (fake's
  `complete_turn` wrongly emitted a `TurnEnded` record the default worker doesn't)
  → fixed: `complete_turn` is counter-only, opt-in `emit_turn_ended_event` covers
  `SKETCH_EMIT_TURN_ENDED=1`. Build + full suite + 8/8 fake tests green.
  Behavior-preserving → foldable after build-check. Overlaps
  `tests/session_resilience_test.rs` with phase 4 at integrate (kept additive).
  Unblocks the phase-8 eventlog reducer/forwarder headless tests.
- **GUI projection + full eventlog end-to-end (phase 8)** — `MERGED` to master
  (`f0710fc`, 2026-06-08; v3 WAL cutover landed). Post-merge runtime confirms still
  owed (see end of entry) but non-blocking — headless + reviews are green.
  Producer collapse `Notification::{ReplyEvent,TurnEnded,UserPrompt}` +
  `WorkerEvent::Reply` → one `AgentEvent` (`src/agent_event.rs`, byte-preserving
  `Unknown{tag,raw}`); emit chokepoint (worker stamps gen/turn, server `record()`
  assigns durable seq); generation-on-`ChannelOpened`; WAL 2→3 **discard**;
  **ringbuffer compaction** (`log_base` logical offset, §6 epoch predicate wired
  into phase-5 cursor seq-space, `CompactedSummary` trim marker, on-disk WAL
  append-only); GUI **total reducer** over `AgentEventKind` + idempotent finalize,
  **additive** per §9 (old inference kept behind a gate — deleting it is a
  post-soak follow-up). **Verification:** workflow `wf_73656668-97f` (build + all
  suites green: new `event_log`/`agent_event_stream`/`agent_reducer_*`/ringbuffer
  tests) → adversarial review `BLOCKING` (live forwarder gapped the owner across a
  trim; marker prepend shifted seq +1; finalize ledger keyed 0-vs-1-based so dedup
  never fired) → **fixed** (log_base-aware live forwarder + owner hard-ceiling;
  prepend decrements `log_base`; aligned finalize keys) with fail-before/pass-after
  tests → re-review `SOLID`, found a MAJOR (no high-water bound → an App-Napped
  owner pins in-memory growth) → **fixed** (spec §6 disconnect-before-gap:
  `enforce_high_water` evicts the slowest forwarder — owner included, lease-safe —
  before the trim) → eviction race self-checked clean (immutable `LogSnapshot` +
  evicted-check-first). Final: build clean, **full `--features test-support` suite
  green**. **Runtime check (2026-06-08, isolated v3 sandbox):** replay idempotency
  passed (a daemon-bounce full replay caused NO visible re-render — the reducer
  refolded identical state); WAL v3 reload + re-adopt clean. **Found + FIXED a real
  §9 bug:** a live prompt AFTER a resume stuck in "thinking" forever — `ReplayEnd`'s
  server-stamped envelope `turn` (`self.turns`) aliases the next live turn's
  finalize key (`completed_turn = turns-1`), so routing `ReplayEnd` through the
  per-turn idempotency ledger pre-occupied the live turn's `(gen,turn)` slot →
  live `TurnEnded` no-op'd finalize → `turn_phase` never returned to `Idle`. Fix
  (`e19b9d7`): `ReplayEnd` is a replay-PREFIX marker, routed through a one-shot
  `replay_prefix_finalized` (re-armed on `reset_for_replay`), never taking a
  per-turn slot. Reproduced + fixed headlessly (verify_harness), independently
  re-reviewed `SOLID` (multi-resume re-arm + no-other-aliasing-pair + no §9
  regression confirmed). **Post-merge confirms (NEEDS-RUNTIME, non-blocking):** (1) one-shot GPUI paint confirm —
  the now-`Idle` spinner visibly clears on screen (fold/`turn_phase` proven correct
  headlessly; only the paint is unverifiable without a GPUI run); (2) App-Nap-paused-owner
  high-water eviction → clean reconnect + lease reclaim in the live app; (3) the
  merge is the **v2→v3 WAL cutover** (discards v2 sessions — do at a quiet moment).
  **Deferred follow-ups:** delete the §9 gated old-inference after real-session
  soak; latent `event.seq` vs `seq_of` divergence (only bites when phase-5 cursor
  is wired client-side — commented).
- **GUI stale-session robustness** — `DONE` (`b0f1eb2`) / NEEDS-RUNTIME. GUI drops
  the slot + scrubs the persisted id (by id, across all cwd keys) on a permanent
  `no such session` attach error; transient errors keep the recoverable status.
  Compile-verified; runtime check owed (silent drop, no recur next launch,
  transient survives, last-slot restores underlying, multi-tab/pane).
- **In-app rebuild + reconnect-badge** — `NEEDS-RUNTIME`. `dev_rebuild_restart_gui`
  (`<space> c g`) and the permission badge after a sid-only reconnect (shows
  default until re-synced) need a human runtime check.

## Top priority

- **State-first architecture overhaul** — `PHASE-A-DONE / PHASE-B-GATED`
  (updated 2026-06-08). Root-cause fix for the constant-regression class (30% of
  state is hand-synced caches/copies). Full state→owner map (162 items),
  20-module state-first decomposition, 6 gating decisions, and a phased plan in
  `docs/specs/spec-state-architecture.md` (+ Appendix A inventory).
  - **Phase A (pure extractions) — essentially complete.** Landed: `replay_turns`
    field-ownership (`6168157`), `overlay` 5-Options→`ActiveOverlay` enum
    (`e5be921`), `settings`/text-zoom persist (`e66a54c`), canonical cwd key
    (`c46f023`), `tool_calls`→owner (`f10486e`), `agent_view_model`→owner
    (`9253139`), additive `TurnEnded{generation}` (`8cdbdd1`), server `record()`
    fusion + `apply_channel_state` unify (`74c4f73`), `InputSurface` enum
    (`761dfe6`), dead-code removal (`15fe390`), `reset_for_replay` delegation
    (`eca7759`). Deferred-on-purpose: `buffer_pool` (5a, folded into D2) and
    `DocState` auto-derive (5b, memo half already done).
  - **Decisions D1–D6 — written.** ADRs 0006–0011 cover them (0007 doc/edit rope
    = D2, 0008 reconnect semantics = D3, 0009 durability = D4, 0010 cwd = D5,
    0006/0011 turn-end + crate boundary ≈ D1/D6). No longer a blocker on the user.
  - **Stop-the-bleeding — done:** CI gate ✅, keymap extraction + headless action
    smokes ✅, worksheet double-render ✅, `clippy -D warnings` + `fmt --check`
    quality CI gate ✅ (2026-06-08).
  - **Phase B (behavior-changing, GPUI-runtime-gated) — HELD, by design.** Not
    blocked on a decision; blocked on the **verification harness** (GPUI can't be
    driven headlessly) and on stabilizing the active reconnect path. Remaining:
    `5c` Doc/Edit single pooled rope (49-site staged rewrite, ADR-0007);
    `8b` delete turn-end inference in the pumps (needs the worker to actually emit
    `ReplyEvent::TurnEnded` first — the default worker still doesn't, see
    `acp_channel.rs:541`; partly superseded by the phase-8 `AgentEvent` stream);
    `10` reconnect `Arc<Core>` swap (trigger-deferred per ADR-0008);
    `ChannelAttachState` faithful enum (refactors the live reconnect-storm path —
    stabilize that first).

- **CI gate** — `DONE` (2026-06-08). Minimal `build --bins + test` on push/PR
  (`.github/workflows/ci.yml`) landed; the `quality` job (`clippy -D warnings` +
  `fmt --all --check`) is now enabled too — the whole tree is clippy-clean and
  fmt-clean. Turns the human from the only oracle into the fallback.

- **Verification harness** — `READY`. Highest leverage: agents can't drive the
  GPUI app, so everything is human-verified. Build a headless/scripted render +
  golden screenshots, a realistic-size perf bench as a gate, and a scripted-input
  driver. See `docs/dev-system.md` § Verification harness. Until this lands,
  every branch below is `NEEDS-RUNTIME`.

## State (2026-06-02)

`master` fast-forwarded `f282130` → `8036ccf` (= `integration`): base ACP + rail
+ perf + workspaces are now on `master`. Rail is **runtime-confirmed by the
user**; the rest is `NEEDS-RUNTIME`. Follow-ups below are off `integration`,
**not yet folded**.

## Follow-ups (branches off `integration`)

- **`ff-buffer-pool`** — `IN-FLIGHT`. Wire the dead buffer pool into the live app
  so docs are shared by reference across views (fixes workspaces "also-show"
  sharing unsaved edits). See ADR-0005. Behavior-changing → human review.
- **`ff-ui-threading`** — `DONE` (`c7b138f`). Move `open_agent`/`attach`/`close`
  socket round-trips off the paint thread (tachyon S4); open is now instant.
  Removes the last ~30s freeze path. Behavior-changing → **runtime review before fold**.
- **`ff-editor-perf`** — `DONE` (`42b4507`). Delta-based undo (refactor #4) + O(1)
  LLM insertion-point cache (#9), +10 tests. Behavior-preserving → foldable after build check.
- **`ff-server-perf`** — `DONE` (`7a352ea`). `Arc` event_log snapshots (#6).
  Behavior-preserving → foldable. `#7` (lock sharding) deferred below.

## Ready

- **Fold the perf/cleanup follow-ups into `integration`** after build-check:
  `ff-server-perf` (done), `ff-editor-perf` (when done). Hold the behavior-
  changing ones (`ff-buffer-pool`, `ff-ui-threading`) for runtime review.
- **Retarget `/refactor` to sketch** — `READY`. Its `workflow.js` PHILOSOPHY
  preamble is Fulcrum-specific (Python/PyO3/pytest/EARS). Replace with a Rust /
  GPUI philosophy (Result-typed errors, newtypes for invariants, `#[test]` /
  `debug_assert!` as enforcement hooks, no migration framing).

## Deferred (with reason)

- **`/refactor` net-new findings not yet taken** — see
  `docs/research/refactor-review-perf-hot-path.md`. `#4`/`#9` are being done in
  `ff-editor-perf`; `#6` in `ff-server-perf`. Remaining: nothing critical.
- **Server lock sharding / forwarder-consumes-broadcast (refactor #7)** —
  `DEFERRED` (needs-human). The "event_log is the single source of truth,
  broadcast is only a wake signal" design is load-bearing (fixes `Lagged`
  merge artifacts). Changing it risks ordering/dup regressions. `#6` already
  removed the dominant cost (whole-log clone). Revisit only if profiling shows
  the per-event global lock is a real bottleneck under many sessions.
- **event_log compaction/capping** — `DEFERRED`. Interacts with the resumable-
  tail `sent`-index replay protocol; risky. `Arc` snapshots (`#6`) bought the
  cheap win without it.
- **Tachyon R1/R2 (speculative pre-tokenize, frame-budgeted replay)** —
  `DEFERRED`. Marginal after the memoization (S1) landed; measure before building.
- **tool_calls deep-clone per frame → `Rc<HashMap>`** — `DEFERRED`. Touches
  ~5 mutation sites with `Rc::make_mut`; outside the memoized boundary, so
  orthogonal. Low risk but not yet worth the churn.

## Needs decision (you)

- **Workspaces multi-membership for agents** — `NEEDS-DECISION`. Needs the
  multi-subscriber session core/view split (see `spec-workspaces-tagging.md`).
  Bigger lift; confirm it's wanted before building.
- **Merge order to `master`/`main`** — `NEEDS-DECISION`. `integration` is the
  combined buildable branch; none of it is runtime-verified yet.

## Needs runtime verification

All 2026-06-02 branches: `rail-fixes` (placement/contrast/chords), `perf` /
`perf-tachyon` (feels-fast + tokens/tool-expand/thinking-indicator correct),
`workspaces` (Ctrl-W m/M chords, dot, focus after move), `integration` (all of
the above together).
