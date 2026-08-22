# bug-0053: recovered-sessions-drop-live-replies

**Status:** IN PROGRESS
**First seen:** 2026-08-22
**Component:** session-server durable recovery / Agent Tile generation rebaseline

## Symptom

Older, unarchived sessions accept a message and remain visibly “Thinking,” with
no reply appearing. The session-server reports them connected and eventually
idle, and their WAL contains the complete user prompt, streamed answer, usage,
and successful `TurnEnded`.

The live report identified `integration meta planner` as one example. Its
265 MB WAL recorded the submitted `ok next` prompt and a completed turn 145 at
10:39:09 PDT, while the GUI remained stuck. A one-session hard restart made it
usable again without discarding its transcript or ACP context.

## Root cause

`restore_seed_from_disk` reset every recovered `ManagedSession` to
`channel_generation = 0`, seeded both generation watch channels at zero, and
made `spawn_resume_worker` publish against generation zero. That is only valid
for a WAL with no prior generation history.

This WAL already retained generation-1 `AgentEvent`s from an earlier hard
session restart. On full replay the GUI correctly rebaselined to generation 1.
The freshly resumed server channel then emitted its live turn as generation 0,
so the GUI's monotonic generation guard correctly classified every new chunk
and boundary as stale. The backend nevertheless executed and durably recorded
the turn, producing the misleading split: server idle/completed, tile still
Thinking with no visible answer.

The manual recovery worked because `restart_session` incremented the running
server's channel from generation 0 to generation 1, bringing it back to the
GUI's replayed high-water generation.

## Fix contract

- Derive recovery's channel generation from the maximum durable `AgentEvent`
  generation and choose the next value; use generation 0 only when the WAL has
  no `AgentEvent` history.
- Seed `ManagedSession`, `LogSnapshot`, `gen_watch`, and the resume worker's
  expected generation from that one value.
- Restart the per-generation agent sequence at zero.
- Guard the real WAL recovery boundary with a generation-1 transcript whose
  first post-restart stream position must be `(generation 2, seq 0)`.

## Approaches already tried

- Restarting only the affected session is a valid immediate recovery but not a
  systemic fix; the alias returns after a whole-server restart unless durable
  generation history seeds the recovered channel.
- Clearing `turn_phase` on `PromptRejected` is a separate UI bug. This reported
  turn was accepted and completed, so no prompt rejection occurred.

---

## Log

### 2026-08-22 — runtime localization and immediate recovery

- Confirmed 21 live, unarchived, subscribed sessions and only the diagnostic
  session busy at the server.
- Confirmed the example session's complete turn 145 in its WAL despite the
  stale GUI state.
- Correlated retained generation-1 history with the post-server-restart
  generation-0 live turn.
- Force-restarted only `integration meta planner`; it reattached at generation
  1, remained at 145 turns, and preserved its ACP session.

