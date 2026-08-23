# bug-0054: terminal-session-stays-thinking

**Status:** FIXED
**First seen:** 2026-08-22
**Component:** session server lifecycle / Agent Tile compose state

## Symptom

An older unarchived session can accept text and remain visibly “Thinking” even
after its agent has disconnected or its spawn failed. Detach and prompt
rejection notifications can likewise leave the local turn phase in-flight, and
submitting to an archived tile can optimistically paint a user turn that the
server will never accept.

## Context / root cause

The server inferred lifecycle from `Option<TransportHandle>` and treated every
missing channel as a temporary spawn window. Permanent disconnects therefore
queued prompts forever. Spawn failure and cancellation did not consistently
clear `busy` or drain queued prompts. In the GUI, `SessionDetached` and
`PromptRejected` updated status text but did not settle `TurnPhase`, while the
archived guard ran after the optimistic compose commit.

## Planned solution

Make lifecycle explicit, queue only during `Spawning`/`Restarting`, and make
every terminal transition clear busy state and reject/cancel admitted work.
Set the GUI phase to Idle on terminal notifications and reject archived submits
before modifying the transcript.

## Approaches already tried (do NOT repeat)

- Treating `channel == None` as “still starting” cannot distinguish a recoverable
  spawn window from a terminal disconnect.
- Clearing only the status string does not settle the compose state machine.

---

## Log

### 2026-08-22 — explicit lifecycle and terminal UI settlement shipped

- Added `SessionLifecycle::{Spawning, Live, Restarting, Disconnected, Archived}`
  and constrained prompt queuing to the two transient states.
- Spawn failure, agent disconnect, and cancel now clear `busy`, terminalize every
  durable pending prompt, and emit `PromptRejected` where user recovery is needed.
- `SessionDetached` and `PromptRejected` now force `TurnPhase::Idle`; archived
  submit is rejected before the optimistic worksheet commit.
- Required negative controls failed when each terminal transition was removed.
  Server lifecycle tests, three real GPUI-path guards, full GUI/server suites,
  and real-wire resilience/transcript suites passed.
- Implemented in `39406ab` and `4fed80c`; merged to main in `c354664`.
