# ADR-0036 — Embed the Cog runtime adapter in the session server

**Status:** accepted
**Date:** 2026-08-25
**Related:** `spec-cog-runtime-adapter.md`; ADR-0015; ADR-0009;
`spec-session-server-actor.md`; Cog runtime-delivery v9 (Chat event 141)

## Context

Cog runtime delivery must inject an exact two-block untrusted user message into
an existing Codex or Claude session, keep the claim renewed, and acknowledge Cog
only after a correlated provider turn completes. The GUI cannot own this: it may
be closed, and tiles are viewports rather than session owners. A second adapter
process could call `admin_prompt`, but it cannot safely observe exact terminal
turns without attaching another forwarder or duplicating provider ownership.

## Decision

Run the adapter as a supervised child task inside `yalda-session-server`.

- The adapter coordinator owns only Cog protocol state, host/attempt leases, and
  its recovery journal.
- The existing Manager remains the sole owner of ACP transports, session WALs,
  busy state, permission mode, steering capability, and turn outcomes.
- A private Manager command admits a Cog delivery, submits or serializes it, and
  resolves only at the correlated canonical terminal event.
- Production Cog HTTP/SSE sits behind a typed transport trait; tests use a mock.
- Configuration is opt-in and activation is capability-gated before any Cog
  mutation or provider input.

## Alternatives rejected

**GUI-owned adapter.** It would disappear on a GUI reboot and conflate a durable
session concern with a tile/view concern.

**Separate binary using the public session socket.** It adds supervision and an
observer/forwarder problem, and `admin_prompt` acknowledges admission rather than
terminal provider success. Extending that public socket to stream delivery
receipts would duplicate an in-process fact already owned by Manager.

**New ACP process per Cog address.** It violates the one-transport/session
invariant and can concurrently resume the same provider session.

## Consequences

The session server gains a bounded optional subsystem and a private correlated
delivery command. It remains usable with no Cog config and stays alive if the
adapter is inert or failing. Provider completion truth has one owner, and GUI
transcripts naturally show Cog-delivered user inputs and resulting turns through
the existing event log.

