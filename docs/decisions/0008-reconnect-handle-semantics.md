# ADR-0008: Reconnect — surface failures now, swap-in-place deferred (D3)

**Status:** Accepted
**Date:** 2026-06-05
**Related:** spec-state-architecture.md (D3), agent_transport module, ADR-0003

## Context

The GUI talks to the persistent session server over a socket that can drop and
reconnect while the server and its sessions keep living. Two bug classes on the
reconnect path: (1) a request sent through a `SessionServerHandle` captured
*before* a reconnect silently goes nowhere (stranded handle → dead writer); (2) a
session that fails to re-attach after reconnect sits in "reconnecting…" forever
(reconnect_session_server ~main.rs:11545 logs to stderr and gives up). Both live
on a rare path — a local unix socket rarely drops.

## Decision

**Split by value and risk.**
- **Now (cheap, low-risk):** surface reconnect re-attach failures — flip the slot
  to a visible error instead of permanent "reconnecting…", mirroring the
  open-path fix (`spawn_attach_sessions`). A handle's identity is "a session on
  the server, by id"; if the session is *gone* after reconnect, that surfaces as
  a visible slot error (the loud failure that matters).
- **Defer:** the `Arc<Core>` swap-in-place refactor (keeps handles valid across
  reconnect). Trigger: *do it if we observe a stranded-handle vanish after a
  reconnect.* The critique rated it HIGH risk; not worth taking on spec for a
  rare path.

## Rationale

The transparent-vs-fail-loud framing is mostly academic (one persistent server,
reconnect to the same one). The substance is "don't strand handles, surface
reattach failure." Only the second is worth doing now.

## Consequences

- Reconnect path mirrors the open path's failure surfacing.
- Depends on D4 durability for *how often* "session gone after reconnect" fires.
