# Worklog — 2026-06-06 — Phase B batch (R + 9′ landed; 8b/5c/11′ held)

**Branch:** `phase-b-batch` → fast-forwarded to `master`.
**Method:** ADR-grounded map workflow (5 agents) → implement the safe subset →
adversarial-verify workflow → CI-gate. Map deliberately asked each agent whether
its item was `safely_autonomous`; **4 of 5 came back false/high-risk**, which
drove the cut below.

## Shipped

| Item | Commit | One-liner |
|---|---|---|
| R | `eca7759` | `HighlightCache::reset()` — owner owns its reset (ADR-0011); `reset_for_replay` delegates instead of reconstructing |
| 9′ | `74c4f73` | `apply_channel_state()` chokepoint — **restart now drains `pending_prompts`** (was: prompts queued mid-restart were lost) |

Both adversarially verified **behavior-preserving-as-intended, high confidence,
zero findings**. CI green (build + full test). 9′ ⚠️ owes a runtime check.

## Held (with reasons) — NOT blind-merged

- **8b** (delete turn-end inference) — gated D1/ADR-0006. The direct ACP path
  has **no worker-side `ReplyEvent::TurnEnded`** (A.8a only added `generation`
  to the *server* `Notification::TurnEnded`), and the worker doesn't know its
  channel generation (that's a server-side counter). ADR-0006 mandates an
  emit-additively → observe-agreement-across-real-sessions → delete rollout; the
  observe step is inherently a runtime job. Not a clean one-pass blind change
  (an enum variant forces match-arm edits at every consumer, unlike A.8a's field).
- **5c** (Doc/Edit shared rope) — gated D2/ADR-0007. 49-site rewrite of core
  content ownership (retire `edit_cache`, pool-gc for two-view files, unified
  undo); ADR explicitly stages it (5a→5b→5c). 10 owed runtime checks. Reckless
  to land blind; deserves its own focused session.
- **11′** (`ChannelAttachState` enum) — no functional benefit, and it refactors
  the **exact reconnect/attach path as the active reconnect-storm bug** found
  this session. Verification is compromised while that path flaps; stabilize
  first. Faithful design is mapped (`Attaching { prev: Option<Channel>, rx }`).

## Found this session — new bug

- **Session-server reconnect storm** (`docs/backlog.md` → Bugs, memory
  `project_reconnect_storm`). 489 "client reconnected" vs 5 "client connected"
  across runs; user hit `close_session(...) failed (connection): session server
  disconnected` while creating a session during signoff. `request()` errors
  whenever the liveness flag is false → a round-trip in a down window fails.
  Pre-existing; NOT from the Phase A/B refactors. Clean-room reset unblocked.
  **Recommended next focus** — it blocks signoff and gates a clean 11′.

## Design notes (9′)

- `apply_channel_state` returns the OLD channel via `#[must_use]`; each caller
  `drop(old)` AFTER releasing the sessions lock — `AcpChannelClient::Drop` joins
  the worker / kills the child and must not run under the global mutex (preserves
  `restart_session`'s original ordering).
- Draining under the lock is safe (`send()` is a non-blocking `prompt_tx.send`)
  and actually closes a pre-existing create_session race (a concurrent `prompt()`
  could re-queue onto a `pending_prompts` we'd already taken).
- `set_permission_mode` is an atomic store, so its order vs the prompt drain is
  immaterial.

## Next

1. **Reconnect-storm root cause** (blocks signoff; gates 11′).
2. Runtime-check 9′ (prompt-during-restart; restart-in-Plan).
3. 5c and 8b as their own focused, staged sessions with the user in the loop.
