# bug-0056: session-settings-and-prompts-not-durable

**Status:** FIXED
**First seen:** 2026-08-22
**Component:** session WAL / crash recovery

## Symptom

After a daemon crash or restart, a session can revert its selected permission
mode or model. A prompt accepted while the agent is spawning can disappear,
including pasted image attachments. Conversely, a live send that failed can
recover as a phantom retry or transcript turn.

## Context / root cause

The WAL persisted creation metadata and transcript notifications but not later
permission/model changes or the delivery boundary around queued prompts.
`UserPrompt` was also recorded before live delivery succeeded, so event history
could claim a user turn the worker never received.

## Planned solution

Add fsynced, last-write-wins permission/model records and write-ahead prompt
intent plus terminal outcome records. Recover only intents without a terminal
record, including full image payloads. Record the optimistic transcript fact
only after delivery or durable admission, and reapply recovered settings to
every replacement channel.

## Approaches already tried (do NOT repeat)

- Reconstructing settings from transcript events is impossible: model and
  permission selection are metadata, not canonical turn events.
- Logging `UserPrompt` first cannot distinguish admitted work from a failed live
  channel send.

---

## Log

### 2026-08-22 — durable metadata and prompt transactions shipped

- WAL recovery now applies the latest permission/model selection and rebuilds
  unsettled prompt intents in FIFO order with text and images.
- Delivery, rejection, and cancellation append terminal records; replacement
  channels reapply settings before flushing pending prompts.
- A recovered session without an ACP resume id now performs a fresh spawn rather
  than deleting its valid WAL identity.
- Negative controls proved permission override, terminal filtering, and failed
  live-send behavior are constrained. Thirteen WAL tests and the full combined
  suite passed.
- Implemented in `b62f421`; merged to main in `c354664`.
