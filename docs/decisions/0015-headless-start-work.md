# ADR-0015: "Run with no GUI" includes *starting* work headlessly

**Status:** Accepted (decision); mechanism is a follow-up spec
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

Decision only. The verb/CLI design, ownership-interaction rules, and durability
ordering go in a follow-up spec before implementation. Tracked in the backlog.
