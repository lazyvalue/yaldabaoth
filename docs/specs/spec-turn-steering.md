# Agent Turn Steering

**Status:** DRAFT (implemented on branch `agent-steering`)

**Last updated:** 2026-08-12

> **SCOPE NOTE (supersedes the queue/chips described below).** The shipped design
> is **provider-aware delivery**: Claude sends at once (mid-turn via the
> worker's `promptQueueing` concurrent driver), while capable Codex adapters use
> their advertised native `_session/steering` FIFO. Older Codex adapters
> gracefully cancel an in-flight turn before sending the normal message as a
> replacement prompt;
> both commit the user turn on a successful send. On send
> failure the draft is left in the compose for retry. The earlier **client-side
> steering queue + cancelable chips + `flush_idle_steering`** were removed (an
> unrequested addition that, once its chips UI was reverted, hid the user's text
> on a failed send). Sections mentioning `AgentState.steering`, chips, or
> at-boundary flush are HISTORICAL; the queue no longer exists. See INV-UX-7.

## Builds On

- **ADR-0024 / Model C** — the transcript is read-only + append-only and the
  draft lives in a separate `Compose` buffer; the only cross-buffer transfer is
  text, never a position. Steering reuses the compose as the authoring surface
  and only ever *appends* committed turns to the transcript — a queued steer is
  never written into the transcript until it is actually sent.
- **`spec-agent-render-pipeline.md`** — the `TranscriptSeqs` fingerprint
  discipline for the **cached** transcript. The steering chips render in the
  inline compose region (re-drawn with the root on notify), NOT in the cached
  transcript, so the queue is deliberately **not** a `TranscriptSeqs` input —
  adding it would needlessly bust the transcript cache on every queue change.
- **`spec-agent-session-ownership.md`** — strict 1:1 tile↔session; session state
  is owned by the `AgentSession` model. The steering queue is session-owned so it
  survives unbind/rebind and a turn ending in a non-focused session still flushes.
- **`ux-invariants.md`** — this spec adds **INV-UX-7** (steering-queue
  affordance + delivery semantics + Esc-interrupts-in-flight).
- **`src/acp_channel.rs`** — the worker driver loop serializes prompts: it fires
  `session/prompt` and awaits the response (turn end) before draining the next.
  This is the v1 transport reality the at-boundary delivery is built on.

## Overview

Users want to interrupt or redirect the agent mid-task with a normal message.
The TUI does this **in-process** via the Claude Agent SDK's *streaming-input*
mode — it injects a user message straight into the running agent loop. yalda
talks to Claude over **ACP**, a protocol boundary, so the TUI's mechanism is not
directly available:

- **ACP v1** (the negotiated protocol): a prompt **is** a turn — `session/prompt`
  resolves with a `stopReason`. The spec defines no mid-turn input message; only
  `session/cancel`.
- **ACP v2** (the `unstable_protocol_v2` draft in crate 1.0.0): decouples turns
  from prompts. **NOT usable today** — probed against `claude-agent-acp@0.44.0`,
  which negotiates **down to v1** when offered v2. A v2 protocol implementation
  would be dead code until the agent speaks it.
- **The real mechanism — `promptQueueing` (v1 vendor capability).** The same probe
  showed `claude-agent-acp` advertises `agentCapabilities._meta.claudeCode.promptQueueing`
  and **accepts a `session/prompt` sent while a turn is in flight**, queueing it and
  processing it the instant the current turn finishes (verified: a mid-turn prompt's
  output streamed right after the in-flight turn, same session, no interruption).

**The blocker was yalda, not the protocol.** The worker driver *serialized* — it
awaited each turn's response before forwarding the next prompt — so a steer could
not reach the agent until the boundary. Turn Steering makes the worker forward
prompts **concurrently** when `promptQueueing` is advertised, so a submit reaches
the agent **immediately, mid-turn**.

Delivery model:

- **Immediate (IMPLEMENTED, default for capable agents):** a submit is sent at
  once — even mid-turn — and committed as a user turn. The agent queues it and
  processes it after the current turn. This is the v2-ready shape: if the agent
  ever negotiates v2, the same path flips to true generation-time injection with
  no UI change.
- **Native steering (Codex):** when initialize advertises root
  `_meta.steering.supported`, a normal message submitted while Codex is awaiting
  is sent as `_session/steering`. The adapter serializes successive requests in
  FIFO order and injects them into the active turn. Yalda places the initial
  capable-Codex prompt, native steering requests, and explicit Stop on one
  ordered worker-control stream, then emits Stop as `session/cancel`; neither a
  fast follow-up nor Stop can overtake an earlier submission.
  Older adapters fall back to cancel-then-prompt. Neither path enters the Stop
  button's `StopRequested` / force-restart state; idle Codex submits are ordinary
  prompts.
- **Offline queue (IMPLEMENTED, fallback):** if the send fails (disconnected), the
  message is held in `AgentState.steering` and retried at the next turn boundary
  (`flush_idle_steering`) — FIFO, never dropped. Surfaced as cancelable chips.

Named artifacts:

- **`PendingSteer`** — one queued message: `{ id, text }` (`agent.rs`).
- The **steering queue** — `AgentState.steering: VecDeque<PendingSteer>` +
  `steering_seq` (stable-id source).
- **`flush_idle_steering`** — the view method that drains the queue at the
  boundary (`agent_ui.rs`).
- **`StopAgent`** — the existing interrupt action (`session/cancel`); this spec
  adds an Esc trigger.

## Behaviors

### Submit / immediate delivery (IMPLEMENTED)

1. `submit_compose` always attempts an **immediate send** via
   `send_prompt_to_session`, regardless of turn state. On a successful write it
   commits the user turn (`insert_user_turn`) and begins a turn **only if one
   isn't already running** (a mid-turn steer rides the in-flight turn; the
   elapsed/quiet clocks are not reset). The compose resets, preserving placement
   (Model C §4.1).
2. The **worker** (`acp_channel.rs`) makes mid-turn delivery real: when the agent
   advertises `promptQueueing`, its driver forwards each prompt concurrently
   (`FuturesUnordered`, no wait on the in-flight turn), bumping the turn counter
   per settled prompt. Non-capable agents use the unchanged sequential driver.
3. For **Codex**, `submit_compose` detects `provider == Codex && awaiting` and
   asks the transport to steer. An adapter advertising root
   `_meta.steering.supported` receives the initial `session/prompt`, subsequent
   `_session/steering` requests, and later explicit Stop on the same ordered
   worker-control stream;
   otherwise the transport sends one graceful cancel before the ordinary
   replacement prompt. Claude never takes this branch. Both Codex routes retain
   the ordinary commit-on-success UI contract.

### Offline queue / retry (IMPLEMENTED)

3. If the immediate send **fails** (channel down), `submit_compose` enqueues the
   message in `AgentState.steering` (FIFO) and shows an "offline — queued" status.
4. `flush_idle_steering` runs after each pump batch; for a session that is Idle
   with a non-empty queue it pops the **front** steer and retries the send. If the
   send still fails it is requeued at the **front** (`requeue_steer_front`) —
   never dropped. The queue surfaces as cancelable chips above the compose.

### Edit / cancel (IMPLEMENTED)

5. Each pending steer renders as a **chip** in the compose region (just above the
   compose box), in FIFO order, showing a one-line snippet + a ✕.
6. **Cancel** (✕) removes that steer (`cancel_steering_chip` → `cancel_steer`).
7. **Edit** (click the chip body) pulls the steer's text back into the compose
   (caret at end, INV-UX-1), drops it from the queue, focuses the compose
   (`edit_steering_chip` → `take_steer` + `Compose::set_text`). Re-submitting
   re-queues it at the back. No per-chip mini-editor — the compose is the single
   authoring surface.

### Interrupt via Esc (IMPLEMENTED)

8. In the agent view, when a turn is **in flight**, bare `Esc` triggers
   `stop_agent_inner` (`session/cancel`; second press force-restarts) — the
   primary interrupt. Checked after the focused-subagent Esc (which unfocuses).
9. When **no** turn is in flight, `Esc` keeps its current meaning
   (transcript→compose, per-mode toggle). "Esc never quits / never closes" holds.
   `cmd-.` remains bound to `StopAgent`.

### Reset / clear (IMPLEMENTED via construction)

10. A fresh/cleared session starts with an empty queue; pending steers belong to
    the conversation they were authored against.

## Data Model

On `AgentState` (session-owned):

```
struct PendingSteer { id: u64, text: String }
steering:     VecDeque<PendingSteer>   // FIFO; front flushes first
steering_seq: u64                      // monotonic; source of stable PendingSteer ids
```

- A message is **either** in `steering` (pending) **or** committed to the
  transcript (sent) — never both. The transcript stays append-only and truthful.
- `steering_seq` mints stable ids and is bumped on every queue mutation. It is
  **not** a `TranscriptSeqs` input (chips render in the inline compose region).

## Interfaces

`AgentState` (module-internal, called by `agent_ui.rs` + the render path):

- `enqueue_steer(text) -> u64`, `pop_steer_front() -> Option<PendingSteer>`,
  `requeue_steer_front(steer)`, `cancel_steer(id)`, `take_steer(id) -> Option<String>`.
- `Compose::set_text(text)` — load text for chip editing (caret at end).

`YaldaGpuiView` (`agent_ui.rs`):

- `send_prompt_to_session(id, text, cx) -> bool` — shared send core (live submit +
  boundary flush).
- `flush_idle_steering(cx)` — boundary drain.
- `cancel_steering_chip(id, steer_id, cx)`, `edit_steering_chip(id, steer_id, cx)`.

Delivery selection (FUTURE): a `SteeringDelivery::{AtBoundary, Immediate}` derived
from the agent's advertised capabilities; v1 is hard-wired `AtBoundary`.

## Constraints

- **Forward-compatible by construction.** Queue, chips, edit/cancel, Esc, and the
  FIFO contract are identical across v1 and v2; only the flush *trigger* differs.
  v2 must not require touching the UI layer.
- **No optimistic transcript writes.** A steer enters the transcript only when
  sent (fixes the prior `submit_compose` phantom echo).
- **Model C.** Only text crosses compose↔transcript; chips live in the compose
  region, not the transcript.
- **Tests never touch `~/.yalda`** (standard harness seam).

## Revision History

- 2026-06-26 — Created (DRAFT); v1 implemented on `agent-steering`. Steering queue
  with capability-gated delivery (v1 at-boundary / v2 immediate), multiple FIFO
  messages, compose-based chip editing, and Esc-interrupts-in-flight. Adds
  INV-UX-7. Guards `steering_*` in `verify_harness.rs`.
