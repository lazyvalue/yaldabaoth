# Worklog — 2026-06-06 — Finish Phase A (A.7 / A.8a / A.9 / item-11)

**Branch:** `finish-phase-a` → fast-forwarded to `master`.
**Method:** map → implement → adversarial-verify, two workflows + serial
hand-edits (all four items share files, so implementation was serial in one
worktree, not parallel).

## What shipped

| Item | Commit | One-liner |
|---|---|---|
| A.7 | `9253139` | Extract `AgentViewModel` owner (6 memoization fields) from `AgentState` |
| item-11 | `15fe390` | Remove dead `has_unseen_activity` field + orphaned `is_active` plumbing |
| A.8a | `8cdbdd1` | Carry `generation` on `TurnEnded` additively (serde-default, +2 tests) |
| A.9 | — (no code) | `record()` fusion already done; remainder is behavior-changing → deferred |

CI green after each (`cargo build --bins` + `cargo test`, all suites 0 failed).
A 3-agent adversarial-verify workflow returned **behavior-preserving = true,
high confidence, zero findings** for all three code commits.

## Decisions / deviations from the map

- **A.7 — method stays on `AgentState`.** The map plan said move
  `memoize_view_model` onto `AgentViewModel`. That's borrow-infeasible: the
  `rebuild: FnOnce(&mut Self)` closure needs the whole `AgentState` (it mutates
  `block_cache`, reads `tools`), so the method on the sub-struct would require
  `&mut self.view_model` and `&mut AgentState` simultaneously. Resolution: move
  only the 6 fields into a nested `view_model: AgentViewModel`; the method reads
  through `self.view_model.*`. Same god-struct shrink, no borrow hazard.
  `lines_cache`/`lines_cache_seq` left on `AgentState` (the name-collision
  gotcha — an identical pair lives on `EditState`).

- **item-11 — `has_unseen_activity` was write-only dead code**, not a
  scoping problem. Set in 5 places, read in zero (no badge/render ever consumed
  it). Honest "scoping" cleanup = deletion. The cross-tab marking loop in
  `pump_session` existed only to set it, and the `is_active`/`is_active_in_ring`
  plumbing fed only that loop — all removed.

- **item-11 — `ChannelAttachState` enum NOT done (deferred).** The map flagged
  `channel: Option` + `attach_pending: Option` as a 3-state machine to fold into
  an enum. They are not: `main.rs:12767` sets `attach_pending = Some(rx)` WITHOUT
  clearing `channel` (a re-attach while the old channel is still live), and the
  pump polls `attach_pending` regardless of `channel`. That's a real 4th "both
  `Some`" state. A naive 3-variant enum would change reconnect behavior. A
  faithful version needs an `Attaching { old_channel, rx }` variant + a runtime
  check → Phase B (logged as 11′).

- **A.8a — minimal, not the map's over-build.** The map proposed a new
  `Arc<AtomicU64>` generation on `AcpChannelClient`. Redundant: the server
  already owns `channel_generation` (`session-server/main.rs:38`, bumped on
  restart, read by the pump's `synced_gen` rebaseline). A.8a is just: add
  `generation` to the proto variant (serde-default for old-log back-compat),
  populate it from `session.channel_generation` at the one emit site, consumer
  ignores it. No new infrastructure, no second emit (a worker-side emit would
  double-fire the consumer = behavior change; that's 8b's job).

- **A.9 — Phase-A portion already complete.** `record()` already fuses
  log+broadcast. The only writes outside it are two intentional, documented
  carve-outs (`prompt()` append-without-broadcast — the live GUI already has the
  text; `broadcast_owner_changed` broadcast-without-log — transient state). The
  `apply_channel_state()` unification the map wants is behavior-changing (today
  `restart_session` drops prompts queued mid-restart and bumps generation only
  on restart) → logged as 9′ for the runtime-check track.

## Open / unresolved

- **9′** `apply_channel_state()` prompt-drain + generation consistency (fixes
  latent restart prompt-loss + perm-mode revert) — behavior-changing, needs the
  app.
- **11′** `ChannelAttachState` faithful enum — behavior-sensitive reconnect path.
- One **flaky** test observed: `tests/sketch_channel_test.rs` (legacy
  sketch-channel, subprocess/timing) failed once under concurrent build load,
  passed on every clean re-run. Unrelated to these changes. Candidate for a
  timing-tolerance pass if it recurs.

## Next

Phase A is done. The two highest-value untouched items remain: the **5 owed
runtime checks** and **GUI observability** (`report_error` sink + no-silent-drop
pump) — see `HANDOFF.md`.
