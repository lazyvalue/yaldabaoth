# bug-0036: codex-message-does-not-interrupt

**Status:** FIXED
**First seen:** 2026-08-12
**Component:** `docs/components/agent-tile/compose.md` (UXI-AgentTile-13)

## Symptom

While a Codex session is working, submitting an ordinary message does not
interrupt or redirect the running turn. The message waits behind that turn, so
the user must click Stop separately before Codex changes course. A normal
mid-turn Codex message should perform that graceful interruption itself and then
be delivered as the next prompt.

## Context / root cause

The real UI route is `submit_agent` → `submit_compose` →
`send_prompt_to_session`. UXI-AgentTile-13 currently treats every provider as a
Claude-style steer: it sends another `session/prompt` while preserving the clean
`Awaiting` phase. Claude's adapter advertises `promptQueueing`, so that is useful
there; Codex needs the current ACP turn cancelled first. The provider is already
durable on `AgentState`, and both the server-managed and direct channel paths
already expose the same graceful cancel transport used by the Stop button.

This is not a missing keyboard binding and is not fixed by changing Esc or the
Stop button. The missing behavior sits on normal submit itself.

## Solution

On a normal submit, snapshot whether the bound session is both Codex and
currently awaiting. If so, send one graceful ACP `session/cancel` through the
active transport immediately before sending the new prompt. Reuse a factored
transport helper shared with Stop, but do not enter `StopRequested` or invoke the
second-Stop force-restart policy: the new prompt deliberately supersedes the old
turn. Leave idle Codex submits and all Claude submits unchanged.

The real path is guarded with an in-process `AcpChannelClient`: first establish
`Awaiting` through a real submit, then type and submit a normal Codex follow-up,
asserting that both a cancel and the new prompt reach the transport. In the same
guard, prove an idle Codex submit and a mid-turn Claude submit emit no cancel.

## Approaches already tried (do NOT repeat)

- Treating every provider as Claude-style prompt queueing. Codex accepts the
  later prompt, but it does not use that prompt to interrupt the active turn.
- Rebinding Esc or changing the explicit Stop action. Neither changes what a
  normal message submission does, which is the behavior reported here.

---

## Log

### 2026-08-12 — localized

Localized the omission to the normal submit path above and added the provider-
specific contract plus a real-path regression guard.

### 2026-08-12 — fixed and verified

Factored the Stop button's graceful transport operation into
`cancel_session_transport` and reused it from normal submit only when the bound
session is Codex in a clean `Awaiting` phase. The cancel is sent before the
replacement prompt; normal submit does not enter `StopRequested`. Idle Codex,
Claude steering, and a Codex turn already in `StopRequested` remain non-cancel
controls.

The mandatory negative control removed the Codex-awaiting interrupt call. The
new real-path guard failed exactly at `mid-turn Codex submit must interrupt the
running turn` while the preceding prompt assertions remained green. Restoring
the call passed the focused guard, steering and Stop regressions, and the full
GUI suite (569 passed, 1 ignored). Both mutations of the provider/phase predicate
were caught. `cargo build --release --bin yalda-gpui` passed.

Runtime gap: no live Codex subprocess was driven during this repair. The GUI's
production submit path and cancellation/prompt transport queues are covered
headlessly; restart the running Yalda app to load the rebuilt binary, then the
reported live interaction is the remaining confirmation.
