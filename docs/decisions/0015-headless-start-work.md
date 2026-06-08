# ADR-0015: "Run with no GUI" includes *starting* work headlessly

**Status:** Accepted — mechanism implemented (see "Status of mechanism")
**Date:** 2026-06-07
**Related:** spec-session-server-actor.md (lease ownership, admin surface), handover2.md § Open decision, ADR-0009 (durable WAL — makes an enqueued prompt survivable), ADR-0013 (launchd — the always-present host), ADR-0014 (permission mode gates what a headless agent may auto-do)

## Context

The resilience/durability/supervision arc made the session server *always
present* (launchd), crash-survivable (WAL), and able to *finish* turns with no
GUI attached. But **starting** work still requires an owner GUI: a prompt is an
owner-only verb sent over an attached connection. So "run with no GUI" today
means *finish what was started*, not *start autonomously*. The open question
(handover2.md): should it also mean **start** work — e.g. cron/automation
enqueuing a prompt to a session that has no attached owner?

## Decision

**Yes.** "Run with no GUI" includes initiating work headlessly. A non-GUI
caller (CLI subcommand and/or admin verb over the socket) can enqueue a prompt
to an existing, unowned session and have the agent run it to completion with no
GUI ever attaching.

## Consequences / implied work (follow-up spec, not yet built)

- **An admin/CLI "enqueue prompt" path** distinct from the owner-only `Prompt`
  verb. Likely shapes: a `sketch-session-server prompt <session_id> <text>`
  subcommand and/or an `admin_prompt` socket verb. Reuses the existing
  pending-prompt queue + WAL durability (the prompt must be persisted before
  ack, per ADR-0009's "never lose a sent prompt").
- **Ownership semantics.** Enqueuing without owning must not fight the lease
  model (spec § Lease). Cleanest: a headless prompt does not take a lease; it
  appends to the session's input queue and the server drives the turn. A later
  attaching GUI observes/owns normally. Define what happens if an owner *is*
  attached when a headless prompt arrives (queue behind owner input, or reject).
- **Authorization.** Gated by ADR-0014's permission mode: a headless-started
  turn runs under the session's stored mode. Since there is no human to escalate
  mid-turn, a session intended for autonomous runs must be pre-escalated
  (explicitly, by its creator) — the safe default means an un-escalated session
  started headlessly will decline tools, which is the intended fail-safe.
- **Lifecycle.** Pairs naturally with creating a session headlessly too (a CLI
  `create` already exists in spirit via the wire `CreateSession`); the full
  "cron kicks off an agent" story = create (or target existing) + set mode +
  enqueue prompt, all without a GUI.

## Status of mechanism

**Implemented.** The headless enqueue-prompt path is built and tested:

- **Socket verb:** `Request::AdminPrompt { session_id, text }` (wire tag
  `admin_prompt`), additive to `session_proto.rs`. It maps to a new
  `Command::AdminPrompt` in the actor, whose handler calls `enqueue_prompt`
  with NO owner gate.
- **Core refactor:** `do_prompt` was split into a thin owner-gate check plus a
  shared `enqueue_prompt(session_id, text)` core (`log_only(UserPrompt)` →
  WAL fsync at the turn boundary → send to the live channel, or push to
  `pending_prompts` if the agent is still spawning). The owner path is
  behaviorally identical; `AdminPrompt` reuses the exact same core, so the
  prompt is just as durable (ADR-0009's "never lose a sent prompt" holds).
- **CLI:** `sketch-session-server prompt <session_id> <text>` connects to an
  ALREADY-RUNNING server (via the new `SessionServerClient::connect_existing`,
  which never auto-launches a throwaway daemon — unlike the GUI's `connect`),
  calls `admin_prompt` (a round-trip so the CLI gets a definitive Ack/Error,
  since it has no notification stream to infer delivery from), and prints
  `ok` / `error: …`. If no server is listening it prints an error suggesting
  the server be started (`sketch-session-server` or `… install`) and exits 1.
- **Client method:** `SessionServerClient::admin_prompt(session_id, text)`
  mirrors `prompt` but uses the round-trip `request` rather than fire-and-
  forget.

**Lease-interaction decision (taken here):** a headless prompt does NOT take
ownership / a lease. It only enqueues onto the session's input queue and the
server drives the turn. The agent processes its input queue regardless of which
connection sent the prompt, so:

- An *unowned* session: the headless prompt drives a turn to completion with no
  GUI ever attaching (proven by `admin_prompt_drives_turn_without_owner`).
- An *owned* session: the prompt still enqueues and the turn still runs — there
  is no rejection and no fight over the lease. The current owner keeps its
  lease; the headless input simply joins the queue. (We deliberately chose
  "enqueue alongside" over "reject when owned": the queue is already the single
  serialization point, so there's no correctness reason to reject, and it keeps
  the headless path identical to the owner path modulo the gate.)

Authorization (ADR-0014 permission mode) is unchanged: a headless-started turn
runs under the session's stored mode; an un-escalated session declines tools,
which is the intended fail-safe.

Tested headlessly in `tests/session_transcript_test.rs`
(`admin_prompt_drives_turn_without_owner`): create an unowned session, assert
`admin_status` reports `has_owner == false`, call `admin_prompt`, then attach a
fresh read-only observer and confirm the durable transcript contains the
enqueued `UserPrompt`, the agent's streamed reply chunks, and a `TurnEnded`
(turns >= 1) — proving the NON-owner enqueue actually drove the agent.
