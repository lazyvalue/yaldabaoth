# Backlog

Open, deferred, and flagged work. Higher fidelity than git: each item says what,
why it's here, and its status. Updated at session end (`/worklog`) and as items
move. Past work lives in `docs/worklog/`; the *why* of choices lives in
`docs/decisions/`.

Status legend: `IN-FLIGHT` (agent/branch active) · `READY` (scoped, not started)
· `DEFERRED` (deliberately not now, reason given) · `NEEDS-DECISION` (waiting on
the user) · `NEEDS-RUNTIME` (built, awaiting human runtime verification).

---

## Top priority

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
