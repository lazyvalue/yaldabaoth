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

- **Session-server reconnect storm** — `NEEDS-RUNTIME-REPRO`. The GUI client
  flaps its connection to the session server in a tight loop: a single
  `session-server.log` accumulated **489 "client reconnected" vs 5 "client
  connected"** across runs. Symptom hit 2026-06-06 during signoff: creating a
  new Claude session failed with `close_session(<id>) failed (connection):
  session server disconnected` — `SessionServerClient::request()` returns that
  error whenever the liveness flag is false, so a round-trip (close/create) that
  lands in a "down" window fails. Suspected: ownership flapping in the
  owner/observer model (`ManagedSession.owner`, `Promote`, `attach_with_owner_retry`
  at main.rs:1884) and/or a disconnect during the large attach-replay (109/29/11
  events) re-dropping the socket → reconnect → repeat. Pre-existing; NOT from the
  Phase A/B refactors (none touch connect/reconnect/ownership). Adjacent to the
  reconnect-handle work (ADR-0008 / item 10). Clean-room reset (clear
  `session_server.json` + `acp_sessions.json` + stale socket) unblocks. Needs a
  runtime repro to root-cause: log every connect/disconnect with reason + a
  backoff/jitter on the GUI reconnect, then watch one launch.

## Top priority

- **State-first architecture overhaul** — `NEEDS-DECISION`. Root-cause fix for
  the constant-regression class (30% of state is hand-synced caches/copies). Full
  state→owner map (162 items), 20-module state-first decomposition, 6 gating
  decisions (ADRs), and a phased, individually-verifiable migration plan in
  `docs/specs/spec-state-architecture.md` (+ Appendix A inventory). Immediate
  items: CI gate (done), keymap-extraction → headless action smokes, worksheet
  double-render fix. Blocked on D1–D6 for Phase B.

- **CI gate** — `READY`. Minimal `build --bins + test` on push/PR
  (`.github/workflows/ci.yml`), merging via the `arch-overhaul` branch. Turns the
  human from the only oracle into the fallback. Next: `clippy -D warnings` +
  `fmt --check` once the 8 existing warnings clear.

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
