# bug-0053: recovered-sessions-drop-live-replies

**Status:** FIXED
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

## Fix as shipped

`recovered_stream_position` now derives one canonical recovery seed from the
durable Agent stream: `max(event.generation) + 1` with per-generation sequence
zero, or `(0, 0)` when no Agent history exists. `restore_seed_from_disk` uses
that value for `ManagedSession.channel_generation`, `LogSnapshot`, `gen_watch`,
and `ResumeJob.expected_generation`; `spawn_resume_worker` publishes against
the same seed instead of a hard-coded zero.

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

### 2026-08-22 — monotonic recovery shipped and activated

- Required negative control: the WAL-backed guard failed on the old recovery
  behavior with `(0, 42)` instead of `(2, 0)`.
- Fixed guard: green; all 51 `yalda-session-server` tests and the server build
  passed.
- Mutation gate: 5 generated mutations, 4 caught, 1 unviable, 0 missed.
- Merged to `main` as `46746fd`, built the release server, and restarted the
  runtime without deleting any WALs.
- Runtime verification: 142 durable sessions = 121 archived + 21 live; all 21
  live sessions connected and subscribed. `integration meta planner` retained
  145 turns and recovered at channel generation 2, strictly above its durable
  generation-1 history.
