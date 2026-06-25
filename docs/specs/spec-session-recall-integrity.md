# Spec: Session Recall Integrity

**Status:** DRAFT (revised 2026-06-24 per adversarial review)
**Last updated:** 2026-06-24

## Builds On

- **`spec-multi-session.md`** — defines the session lifecycle (`session/new`,
  `session/load`, detach, attach, prompt). WHY/HOW: this spec tightens what a
  *prompt* guarantees (no silent drop) and what `/clear` does at the lifecycle
  level; it does not change the lifecycle states.
- **`spec-session-server-actor.md`** — the actor owns `ManagedSession`,
  `pending_prompts`, the WAL, and the marker-based `replay_fence`. HOW: this spec
  makes the `pending_prompts` drain *accountable* (it currently only logs) and
  notes the actor spec's UPGRADE-HAZARD banner for any new wire frame.
- **`spec-agent-cwd.md`** — the GUI session reset (`claude-clear` →
  `clear_agent_session`) already mints a fresh session carrying label+cwd+mode.
  HOW: the `/clear` boundary *reuses that existing reset*; this spec only adds a
  path so a **typed** `/clear` reaches it.

## Overview

Two integrity failures around resume / `/clear`, both verified against code +
the user's WAL on 2026-06-24:

- **Failure A — resume reads past `/clear`.** Claude Code's `/clear` resets the
  agent's own context but is **invisible over ACP** (empirically: `acp_session_id`
  unchanged, turn counter does not reset, no event — `~/.yalda/wal/1ae49f84….log`).
  The yalda **menu** clear (`claude-clear`) already handles this safely (it
  creates a *new* server session, so resume can't reach back). The residual gap
  is the **typed** escape hatch: a user typing the literal string `/clear` into
  the compose buffer — that text is sent verbatim to the agent
  (`agent_ui.rs:3547`), Claude Code clears invisibly, and on resume yalda's
  untrimmed WAL replays + `session/load`s the pre-`/clear` context.
- **Failure B — echoed-but-undelivered prompt (the reported data loss).** A user
  prompt is optimistically echoed once a fire-and-forget send "succeeds" at the
  socket layer. The **synchronous** reject paths (no-such-session, immediate
  `channel.send` failure) **already** surface `Notification::PromptRejected`
  (`main.rs:2026–2052`) and the GUI handler already restores the text
  (`agent_ui.rs:2070–2097`). The **silent** seam is the
  `pending_prompts` **drain on channel respawn**: a prompt queued while the
  channel is down is re-sent on reattach, and on failure
  (`main.rs:629–631`) the code only `tracing::error!`s — no `PromptRejected`, no
  reconcile. The user sees the echo; the agent never receives it; nothing
  surfaces the loss. ("I see the messages, but the agent does not.")

Named entity: `PromptRejected` (the EXISTING notification + GUI handler this spec
reuses for the drain seam). No new wire frame is introduced.

## Behaviors

### Prompt-Delivery Integrity (Failure B) — ACTIVE target

- **B1. No silent drain drop.** When `pending_prompts` is drained on channel
  (re)attach, a `send` failure for a queued prompt MUST surface as a
  `PromptRejected` to that session's subscribers — not be logged-and-dropped. It
  reuses the existing notification + GUI reconcile (system notice + the prompt
  text restored to compose). This is THE fix for the reported "agent never got
  my message" on resume/reconnect.
- **B2. The drain is total.** Every queued prompt drained reaches a terminal
  state: delivered to the live channel, or surfaced as `PromptRejected`. No
  "log and continue."
- **B3. Replay fence — already satisfied, assert it.** The `replay_fence` is
  **marker-based** (`ReplayComplete` + `ForceClear`, `src/replay_fence.rs`), NOT
  turn-number-keyed, so a post-resume live turn already cannot be suppressed as
  replay. This spec adds **no fence change**; it only pins the property with a
  regression test so a future change can't reintroduce a turn-number fence.

> **Deferred (separate ticket), explicitly out of scope here:**
> a **success** `PromptReceipt` + bounded-window "ack didn't arrive ⇒
> undelivered" timer for the *fire-and-forget frame lost mid-socket-death* seam
> (`session_client.rs` writer breaks before flush). That seam has no current
> signal and closing it needs a new acked-prompt protocol — which collides with
> the actor spec's **UPGRADE-HAZARD** (a new GUI against an old running server:
> the timer would false-fire "undelivered" on every prompt). Deferred until a
> wire-version handshake exists; the timer MUST degrade to assume-delivered, not
> assume-failed, against a server that doesn't speak the receipt.

### Clear Boundary (Failure A) — ACTIVE target

- **A1. The menu clear already works (SHIPPED).** `claude-clear` →
  `clear_agent_session` closes the old server session and creates a fresh one
  (`agent_ui.rs:2793–2877`); resume of the new session cannot reach pre-`/clear`
  context. No change.
- **A2. Typed `/clear` routes to the same reset.** When the compose buffer's
  submitted text is exactly `/clear` (the typed escape hatch), yalda performs the
  `claude-clear` reset instead of forwarding the literal text to the agent — so
  the typed gesture establishes the same durable boundary as the menu command,
  and resume cannot read past it.
- **A3. Resume cannot cross a reset.** Because a reset creates a new
  `server_session_id`, a resumed session never `session/load`s or replays
  anything from before the reset. (History retention of the *old* session is the
  existing close-behavior — `clear_agent_session` closes + deletes the old WAL —
  and is NOT changed here; the integrity guarantee is "resume doesn't reach
  back," not "old history is kept." Whether `/clear` should retire-rather-than-
  delete the old session is a separate question, not in this spec.)

## Data Model

- **`PromptRejected`** (EXISTING — `session_proto.rs`): `{ session_id, reason,
  text }`. Reused unchanged. The drain-failure path (B1) constructs and pushes it
  exactly as the synchronous-reject path already does (`main.rs:2042–2051`).

No new types, no new wire frames.

## Interfaces

- **Server, module-internal:** the `pending_prompts` drain
  (`apply_channel_state`, `main.rs:629–631`) returns/raises the failed prompt so
  the actor pushes a `PromptRejected` to the session's notification subscribers,
  mirroring the existing reject path. (Today the drain swallows the error.)
- **GUI, external:** the submit path (`submit_chatbox` / `submit_worksheet`)
  intercepts a compose body equal to `/clear` and dispatches the existing
  `clear_agent_session` instead of sending the text (A2). The existing
  `PromptRejected` handler (`agent_ui.rs:2070–2097`) is unchanged and now also
  fires for the drain seam.

## Constraints

- **C1. No new silent-drop seam for prompt delivery.** Any prompt-drop path
  reachable after an optimistic echo MUST emit `PromptRejected`. "Log and
  continue" is disallowed for prompt delivery (B1/B2).
- **C2. Additive only / upgrade-safe.** This spec adds no new wire frame (it
  reuses `PromptRejected`), so it is safe against an old running server. The
  deferred success-receipt MUST be gated on a wire-version handshake and degrade
  to assume-delivered.
- **C3. No fence regression.** The marker-based `replay_fence` MUST NOT be
  re-keyed on the turn counter (B3); a regression test pins this.
- **C4. WAL append-only preserved.** No fix trims or rewrites the WAL.

## Revision History

- 2026-06-24 (2) — REVISED per adversarial review. Corrected: B5/C4 (old
  turn-number fence) removed — the fence is already marker-based (`replay_fence.rs`),
  so B3 is now "assert, don't fix." Dropped the invented submit-token /
  per-turn `UndeliveredNotice` — Failure B reuses the existing `PromptRejected`
  notification + GUI handler. Narrowed B4: the success-receipt + timeout is
  DEFERRED (bigger, UPGRADE-HAZARD), and the in-scope silent seam is the
  `pending_prompts` drain (`main.rs:629–631`). Reframed A1: the `claude-clear`
  menu command already does the safe reset; the gap is the **typed** `/clear`
  (A2). Removed the WAL-retain claim that contradicted close-deletes-WAL (A3).
- 2026-06-24 (1) — DRAFT from the verified investigation (superseded).
