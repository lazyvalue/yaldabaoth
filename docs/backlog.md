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

## Session-server hardening (branch `session-hardening`, off `master`)

Phase-7 of `spec-session-server-actor.md`. All on the **`session-hardening`**
branch (`bd796d4`, `1e2c881`), **unmerged + unpushed**. Headlessly verified;
see worklog `2026-06-07-session-server-hardening.md`.

- **Safe-default permission mode + `0600` socket** — `DONE on branch` /
  `NEEDS-RUNTIME` for GUI UX (2026-06-07, ADR-0014). New sessions default to
  `AskEachTime` (not `Yolo`); `DEFAULT_PERMISSION_MODE` is the single revert point.
  Owner-gated escalation. Socket forced `0600` via `umask` around `bind()`
  (TOCTOU-free). **Runtime check owed:** with no inline-approval UI yet, a fresh
  GUI session declines tools until the user cycles the mode — confirm this is
  acceptable / add a chrome hint, or flip the constant.
- **Structured tracing + `admin_status` verb** — `DONE on branch` (2026-06-07).
  Server-binary `eprintln`→`tracing` (stderr, ANSI off, `RUST_LOG`/env-filter);
  additive `admin_status` returns `AdminSnapshot` (session/owner/subscriber/log
  state). Behavior-preserving; transcript test is the log-grep regression guard.
- **Slow-subscriber disconnect** — `DEFERRED (refined)`. The shared-log forwarder
  already defuses the original "slow subscriber pins unbounded growth" worry
  (subscribers re-derive their tail from the shared `event_log`; no per-subscriber
  queue; `Lagged` is graceful). Residual is a minor liveness issue only: a
  forwarder parked on a permanently-dead socket. Low-risk fix = a bounded
  write-timeout reaper, but it touches the load-bearing forwarder and its GUI
  reconnect-after-forced-disconnect can't be runtime-verified headlessly. Do
  supervised. (Distinct from the deferred event_log *compaction* item below, which
  is the actual unbounded-memory concern.)
- **Actor extraction (phase 3, ADR-0012)** — `READY` (large). Scoped: 21 lock
  sites, 8 mutated `ManagedSession` fields, crux is the per-session pump
  sync↔async bridge (non-`Sync` `std::sync::mpsc::Receiver` → forward `Record`
  into the actor inlet). Mechanical but pervasive (~1–2 wk); kills the shared-mutex
  race class + poison-tolerant lock. Independent of the two `DONE` items above.
- **Merge decision** — `NEEDS-DECISION`. Fold `session-hardening` to `master`?
  Tracing + admin_status are foldable now; the permission change is `NEEDS-RUNTIME`.
  Split the branch if you want the first two ahead of the permission sign-off.

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
