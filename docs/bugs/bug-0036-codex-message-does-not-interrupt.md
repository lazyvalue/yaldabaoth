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

The real UI route is `submit_agent` → `submit_compose` → transport. The original
repair composed `session/cancel` with a replacement `session/prompt`, but those
messages traveled through independent worker queues. Codex's non-
`promptQueueing` worker drains extra cancel signals before starting the next
queued prompt, so successive questions plus Stop had timing-dependent outcomes.

The installed Codex adapter advertises `_meta.steering.supported` and provides
FIFO `_session/steering`. Yalda did not negotiate or use that extension.

## Solution

Negotiate native steering from the root initialize metadata. For capable Codex
adapters, put the initial `session/prompt`, each `_session/steering` request, and
explicit `session/cancel` on one ordered worker-control stream. Await prompt and
steering responses outside that stream so control delivery remains responsive.
Older adapters retain cancel-then-prompt compatibility; Claude's advertised
`promptQueueing` behavior is unchanged.

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

### 2026-08-20 — recurred with successive questions + explicit Stop

The one-follow-up repair is nondeterministic when the user submits more than one
question and then presses ⌘. The normal prompt and cancel travel over independent
transport queues. In the non-`promptQueueing` worker used by Codex, the active
prompt consumes one cancel and the worker deliberately drains every additional
cancel before it starts the next queued prompt. Depending on which prompt has
become active when those queues are observed, the explicit Stop is discarded,
one queued question runs, or a later question is cancelled. The original guard
covered exactly one follow-up, so it could not expose this ordering failure.

The installed `@agentclientprotocol/codex-acp` 1.1.7 advertises root capability
`_meta.steering.supported: true` and implements `_session/steering` with a
per-session FIFO queue. That is the correct transport for already-submitted
mid-turn questions. Yalda places the initial capable-Codex prompt, native
requests, and explicit Stop on one local ordered control stream, so a rapid
follow-up cannot start ahead of the initial prompt and each question is accepted
before the later `session/cancel` can reach the adapter. Yalda will use native
steering when advertised and retain cancel-then-prompt only for older adapters. The new
real-path guard submits two questions during one live Codex turn, presses the
actual Stop handler, and requires two FIFO steering payloads plus exactly one
explicit cancel—never the old three-cancel race.

### 2026-08-20 — fixed and verified

Capability negotiation now recognizes root `_meta.steering.supported`. Direct
and session-server transports expose native steering, and capable Codex sessions
send the initial prompt, every steer, and Stop through one ordered control
stream. Older adapters still receive exactly one compatibility cancel followed
by the replacement prompt.

The exact GUI sequence—initial submit, two mid-turn questions, then the real
Stop handler—was observed RED before routing changed: the first question never
reached native steering. A production fake-ACP subprocess then exposed a deeper
wire race where steering could overtake the separately queued initial prompt;
unifying all capable-Codex control fixed the asserted wire order to prompt,
first steer, second steer, cancel.

Verification passed with 177 library tests (2 ignored), 51 session-server tests,
and 688 GUI tests (2 ignored). Focused GUI, server actor, capability, wire-shape,
and subprocess wire-order guards also passed. Mutation controls caught 3/3 GUI
routing mutants and, after adding a direct legacy-fallback guard for the initial
survivor, 10/10 expanded ACP capability/routing mutants. Both release binaries
built successfully. `git diff --check` passed; repository-wide
`cargo fmt --all -- --check` still reports unrelated pre-existing drift.

Runtime gap: the installed, authenticated Codex adapter was not driven live.
The production worker and JSON-RPC wire are exercised through a subprocess fake;
restart Yalda and its session server to load the rebuilt binaries.
