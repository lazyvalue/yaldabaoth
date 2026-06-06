# Handoff — state-architecture overhaul (single source of truth)

**Date:** 2026-06-06 · **Branch:** `master` @ `0c25869` (Phase A done; Phase B R/9′/8b-stage1 landed; reparse crash+latency fixed) · Tree clean.
Read `CLAUDE.md` + auto-memory first; this is the consolidated item list that
spec §7, the worklogs, and the original `handover.md` only covered in pieces.

This note is the **complete enumeration** — every planned item (spec Phases
0/A/B), every reactive fix shipped along the way, every deferral with its
reason, and every owed runtime check. Status keys: ✅ done · ⬜ open · ⏸ deferred
(reason) · ⚠️ needs human runtime check.

---

## How to read the supporting docs
- `docs/specs/spec-state-architecture.md` — the plan. §3 module map, §7 ordered
  backlog (now matches this note), Phase A/B detail above §7.
- `docs/worklog/2026-06-05-keymap-worksheet-pipeline-mutex.md` — session 1.
- `docs/worklog/2026-06-05-phase-a-autonomous.md` — session 2 (A.2–A.6) +
  deferral reasons + the A.7 gotcha.
- ADRs `0006`–`0011` = decisions D1–D6 (all Accepted).

---

## Phase 0 — enablers
| Item | Status | Ref |
|---|---|---|
| 0.1 CI gate (`scripts/ci.sh`, pre-push + GH Actions) | ✅ | earlier |
| 0.2 `register_keymap` extraction + first headless action smoke | ✅ | `704e13d` |

## Phase A — pure / front-loaded (CI-gated, no behavior change)
| # | Item | Status | Ref / note |
|---|---|---|---|
| 1 / A.1 | `replay_turns` owns its fields | ✅ | `6168157` (field-ownership) |
| — | ↳ worksheet double-render fix (A.1 worksheet half) | ✅ ⚠️ | `a6f2829` |
| 2 / A.2 | 5 overlay `Option`s → `ActiveOverlay` enum | ✅ ⚠️ | `e5be921` |
| 3 / A.3 | `settings`: persist text zoom + one `save_settings()` | ✅ ⚠️ | `e66a54c` |
| 4 / A.4 | canonical cwd key (D5) + save-all-tabs | ✅ | `c46f023` |
| 5a | `buffer_pool` extraction (single liveness) | ⏸ | dead/unwired code; do **with** D2 (5c), not before |
| 5b | `DocState.blocks` → `edit_seq` auto-derivation | ⏸ | memoization half already done (quick-win-2); rest is a murky restructure, low payoff |
| 6 / A.6 | `tool_calls` → `ToolCalls` owner + atomic `register` | ✅ | `f10486e` |
| 7 / A.7 | `agent_view_model` memoizer extraction → `AgentViewModel` owner | ✅ | `9253139` (6 fields moved; `lines_cache` kept put per the gotcha) + `1c939f0` (owner owns the decision: split `memoize_view_model` into `cached`/`store` on `AgentViewModel`, rebuild stays at the call site) |
| 8a | emit `TurnEnded{count,generation}` **additively** | ✅ | `8cdbdd1` (added `generation` w/ `#[serde(default)]`, populated from server `channel_generation`; consumer ignores it; +2 serde back-compat tests). Direct-channel explicit emit left to 8b (behavior-changing) |
| 9 | session-server fusions (`record()` = log+broadcast, etc.) | ✅ | `record()` already fused; only writes outside it are 2 intentional carve-outs. `apply_channel_state()` unification is **behavior-changing** (fixes restart prompt-loss + perm-mode revert) → Phase B / runtime-check track |
| 11 | sum-type cleanups | ✅ (this slice) | `InputSurface` ✅ `761dfe6`; `has_unseen_activity` dead-code **removed** `15fe390`. `ChannelAttachState` **deferred**: `channel`+`attach_pending` reach a "both `Some`" re-attach transient (4-state, not a clean enum) |

## Phase B — GPUI / behavior-changing / gated (⚠️ all need human runtime check)
| # | Item | Status | Gate |
|---|---|---|---|
| R | `reset_for_replay` delegation (each module owns its `reset()`) | ✅ | `eca7759` — `HighlightCache::reset()`; value-identical, adversarially verified |
| 9′ | `apply_channel_state()` unification — drain `pending_prompts` + bump generation consistently across create/restore/restart | ✅ ⚠️ | `74c4f73` — restart now drains prompts (was: lost). Adversarially verified behavior-as-intended. **Owes runtime check**: prompt-during-restart reaches new channel; restart-in-Plan stays Plan |
| 5c | Doc/Edit single pooled `SharedCore` | 🔶 step 1 on branch | On `doc-edit-5c` (off master, +incremental reparse merged). **Done:** `DocState` binds a pooled `SharedCore` via `source: Option<DocSource>`; `edit_cache` retired; toggle shares the core; `refresh_blocks` re-derives Doc blocks live (read-only borrow — panic-safe); **ALL Doc-open paths pool-bound** (open_file, clone-for-split, restore, wiki-link, reload-in-place, startup). **Awaiting runtime check:** two-pane live tracking (refresh fires) + no data loss on toggle. **Step 2:** cursor-stash on Doc→Edit, save-clears-dirty-in-both, persist/restore, gc/refcount hygiene; strip the temp `[5c]` diag logs. Carries a small per-keystroke Doc-side cost (`render_with_wiki` re-parse) — separate from the Edit-side incremental reparse |
| 8b | delete turn-end inference in all 3 pumps | 🔶 stage 1 done | **Additive emit landed** (`8b-additive`, on master): `ReplyEvent::TurnEnded{count}` from the worker, gated `SKETCH_EMIT_TURN_ENDED=1` (default off); GUI arm inert + logs agreement. **Stage 2 (delete inference) splits by risk** — investigated 2026-06-06: (i) **GUI-consumer half** — `apply_reply_events` finalizes on the explicit signal; delete the legacy direct (`gpui:12429`) + TUI (`claude.rs:326`) inferences. **Seam-test verifiable** (verify_harness drives `apply_server_batch`). Lower risk. (ii) **Server-pump half** — delete the inference at `session-server/main.rs:794`, rewire to the worker's explicit `TurnEnded`, **preserving the replay-fence (`:808-838`) + generation emit (`:856-871`)**. **NO headless coverage** (seam tests don't drive the server pump), storm-muddied resume path → needs a live **resume** runtime check; a fence mistake = transcript loss on reconnect. Do (i) anytime; do (ii) with the app + after/with the storm root-cause |
| 11′ | `ChannelAttachState` enum — fold `channel`+`attach_pending` into `Attaching{prev,rx}` | ⏸ HELD | No functional benefit; refactors the **same reconnect path as the active reconnect-storm bug**. Stabilize/understand that first — verification is compromised while the path flaps |
| 10 | reconnect `Arc<Core>` swap-in-place | ⏸ | D3 — explicit trigger-deferral (wait until you observe a stranded-handle vanish) |

## Reactive fixes shipped (were NOT in the original §7 backlog)
| Item | Status | Ref |
|---|---|---|
| Pipelined-submit turn-numbering crash (`User(2) inserted twice`) | ✅ ⚠️ | `50021fc` |
| Session-server mutex-poison cascade | ✅ | `d4cce77` |
| **Edit-view typing SIGSEGV** (stale-tree incremental parse) | ✅ | `d32edf9` — fresh parse; found via 5c testing |
| **Edit-view typing latency** (full parse/keystroke after the segfault fix) | ✅ | `413da19` — proper incremental reparse, fuzz-guarded (~27k edits, sexp-equal to full parse), **10–20× faster** |
| **Reconnect-storm diagnostics** | ✅ | `6eb9660` — every disconnect now names its cause (server EOF/read-err/write-fail; client reader exit reason). Storm itself still NEEDS-RUNTIME-REPRO (backlog → Bugs) |

## Standing / cross-cutting (from spec §7 items 4, 8 + handover §8)
| Item | Status | Note |
|---|---|---|
| clippy `-D warnings` + `fmt --check` in CI | ⏸ | reality is **164 clippy + 531 fmt** diffs (not the stale "8"); dedicated passes only, and fmt would collide with the live `sketch-*` worktrees |
| Regression→prevention loop (fix = failing-test-first; new derived field = +fingerprint +reset) | ◾ ongoing rule | being followed |
| **GUI debug overlay + `report_error` sink + no-silent-drop pump** | ⬜ | the observability gap = root-cause #4 ("failures invisible"); **highest-value untouched work**; wants a short spec |
| D4 durable session-log subsystem (WAL + snapshot) | ⬜ | needs its own impl spec |
| Full D1 event-stream refactor | ⬜ | `spec-event-stream.md`; phased; design solid |

---

## ⚠️ Owed human runtime checks (GUI not headless-drivable — need you at the app)
1. Pipelined worksheet prompts render as **separate** turns (no crash). *(`50021fc`)*
2. Worksheet **send-failure** keeps authored lines editable (was: froze them). *(`a6f2829`)*
3. Tab-double-click rename while the menu is open **drops the menu** (intended). *(`e5be921`)*
4. Text **zoom restores** on relaunch; theme/status-bar still persist. *(`e66a54c`)*
5. Chatbox↔Worksheet **toggle + session restore** still behave. *(`761dfe6`)*

## A.7 gotcha (so the next attempt doesn't trip on it)
`lines_cache`/`lines_cache_seq` exist on **two** structs (AgentState ~`5111` AND
the cache struct ~`4823`), so a blind `.lines_cache` rename is unsafe. The other
6 fields (`flat_items_cache`, `gutter_cache`, `view_model_fp`, `view_model_seq`,
`block_cache`, `block_cache_frozen_count`) are AgentState-unique. Extract those 6
into an `AgentViewModel` owner (move `memoize_view_model` + the fingerprint onto
it); decide whether `lines_cache` moves too (per-site disambiguation) or stays.
`memoize_view_model` couples both + sits in the render hot path → watch the
borrows. Memoization is already implemented + tested, so this is **god-struct
shrink only** — lower priority than the observability gap.

## Recommended next moves
**Phase A is done; the pure/no-behavior-change refactors are exhausted.** Every
remaining item is behavior-changing and runtime-gated — nothing left to grind
solo. The keystone is the **reconnect storm**: root-causing it unblocks both 11′
and 8b's server-pump half. Ranked:
1. **Verify 5c live-tracking** at the app (5 min) → merge `doc-edit-5c` → step 2.
   Closest to done; just needs your eyes. App was last built+running for this.
2. **Reconnect-storm root cause** — fully instrumented now; trigger a repro and
   read the labeled log (server conn-closed reason + client reader exit). Fixing
   it unblocks 11′ AND 8b-server-half. Highest leverage.
3. **8b stage 2** — do the GUI-consumer half anytime (seam-test verifiable); the
   server-pump half needs a live **resume** check (see 8b row) — do it with the
   app, after the storm.
4. **11′** `ChannelAttachState` — after the storm is understood (same path).
5. **GUI observability** (`report_error` sink + no-silent-drop pump) — still the
   highest-value *untouched* feature; would make every runtime check above
   cheaper. Spec it first.

## Working conventions (unchanged)
Worktree per task under `.claude/worktrees/`; map→design→adversarial-verify
**workflow** for high-blast-radius items; `scripts/ci.sh` green before merge;
fast-forward to `master`; update spec §7 + a worklog after each. Never use the
AskUserQuestion tool. `master` is the trunk (no `main` branch exists).
