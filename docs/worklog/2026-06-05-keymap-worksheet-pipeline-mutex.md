# Worklog: keymap extraction, worksheet dedup, pipelined-turn crash, mutex-poison

**Date:** 2026-06-05
**Branch:** `master` — `fb1919a` (base) → `d4cce77`, via four squash-merged task
branches (each CI-green, fast-forwarded, worktree removed):
- `phase02-keymap` (`704e13d`) — Phase 0.2 keymap extraction + first action smoke
- `worksheet-dedup` (`a6f2829`) — worksheet double-render fix (Phase A.1 worksheet half)
- `worksheet-kcollide` (`50021fc`) — pipelined-submit turn-numbering crash fix
- `mutex-poison` (`d4cce77`) — poison-tolerant session-server lock

## Built (with status)

- **Phase 0.2 — `register_keymap` extraction** (`704e13d`). Pulled the four
  `bind_keys` blocks (96 bindings, verbatim) out of `main()`'s run-closure into
  `fn register_keymap(app)`, callable from the headless harness. Landed the
  **first end-to-end action smoke** (`cmd_b_toggles_file_browser_rail`) driving
  the full keymap→action→handler chain. **This unblocks `vcx.simulate_keystrokes`
  for every future GPUI migration step.** CI-green.

- **Worksheet double-render fix** (`a6f2829`, Phase A.1 worksheet half).
  `submit_worksheet` hand-computed `turn_k = last_seen_turns + 1` and froze lines
  inline, bypassing the reconciler chokepoint — so the server/agent echo
  double-rendered. Extracted `register_user_turn() -> Option<k>` (reconcile +
  `current_turn()` k-derivation + `user_turn_ks` tripwire) as the shared core;
  `insert_user_turn` appends, new `commit_worksheet_turn` freezes in place.
  Send-first/commit-on-success (mirrors chatbox; closes a freeze-on-failed-send
  phantom). Tests: GUI seam (suppress + non-vacuous control) + pure multi-line
  reconciler. CI-green. ⚠️ **send-FAILURE behavior change owes a runtime check.**

- **Pipelined-submit crash fix** (`50021fc`) — *reported live by the user:*
  `double user turn: TurnId::User(2) inserted twice`. The worksheet fix armed
  the M3 tripwire on a pre-existing turn-numbering gap: `current_turn() =
  last_seen + 1` only advances on `TurnEnded`, so a second submit made while the
  previous turn is in flight (natural in worksheet: type, send, type, send)
  reuses the in-flight `k`. Fix: every non-replay insert takes
  `max(current_turn(), next_unused_user_turn())` — distinct, monotonic turns; a
  no-op in the common case; covers both pipelined `LocalSubmit` and a
  content-mismatched `Echo`. Tests: exact repro (panicked before → k=2,3 after)
  + unsuppressed-echo sibling. CI-green. ⚠️ **runtime check: two pipelined
  worksheet prompts render as separate turns.**

- **Session-server mutex-poison cascade** (`d4cce77`). 24 `.lock().unwrap()` on
  the shared `sessions: Mutex<HashMap>` → one panic-while-holding poisons the
  lock and every later access cascades, killing all sessions. Centralized all
  access through `SessionManager::lock_sessions()` (recovers via `into_inner()`,
  surfaces the recovery on stderr). Test: poison-then-recover. CI-green.

## Open / unresolved (remaining backlog, triaged this session)

- **Phase A.1 field-ownership half — `replay_turns` owns its fields.** Pure
  refactor (delete loose `last_seen_turns`/`replay_turn`, store one
  `ReplayTurns`, add `on_turn_ended`). Reaches into the pump's turn-end
  detection (`main.rs:12229–12266`) — **delicate; touches the turn-numbering
  just stabilized. Deserves a focused pass, not a rushed one.**
- **GUI debug overlay + `report_error` sink + no-silent-drop pump.** The
  observability gap (handover §8). High ceiling, **multi-session build; wants
  its own spec first.**
- **clippy/fmt CI tightening — NOT a quick win (correcting the stale "8
  warnings").** Reality: **164 clippy lints** + **531 `cargo fmt` diffs**.
  Enabling `-D warnings` is a large, behavior-risky cleanup; `cargo fmt --all`
  churns a massive diff that collides with the live `yalda-*` worktrees. Defer;
  if pursued, do fmt and clippy as separate, dedicated passes.
- **D4 durable-log subsystem** needs its own impl spec. **Full D1 event-stream
  refactor** is a phased effort (design solid).

## Verification status

All four merges build + unit-test green (`scripts/ci.sh`). Headless coverage is
real now (keymap smoke + agent seam tests). **Two owed human runtime checks**
(both flagged above): the worksheet send-failure behavior and pipelined-turn
rendering — the GPUI app still can't be driven headlessly for layout.

## Next

Recommended order: (1) the two owed runtime checks (quick, needs the user);
(2) `replay_turns` field-ownership as a focused pure-refactor pass; (3) spec +
build the GUI debug overlay. Hold clippy/fmt for dedicated passes.
