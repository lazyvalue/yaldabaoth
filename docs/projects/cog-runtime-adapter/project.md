# Project: Cog runtime-delivery adapter

**Status:** in progress — canonical protocol v9 accepted; live activation gated
by the currently-404 capability endpoint.
**Cog graph:** `vkf`
**Spec:** `docs/specs/spec-cog-runtime-adapter.md`
**Decision:** ADR-0036

## Problem / why

Cog durably stores addressed Mail and Chat, but its embedded provider broker and
Yalda can otherwise compete to resume the same registered provider session.
Delivery also needs crash-safe completion semantics: a cursor must not move when
a prompt is merely accepted, dispatched, disconnected, or timed out.

Cog v9 supplies a fenced external-runtime protocol. Yalda now needs the other
half: negotiate it, claim only explicitly selected routes, submit the exact
untrusted payload through the existing session owner, recover after crashes, and
complete Cog only after a real successful provider turn.

## Goals

- One Yalda runtime host covers multiple selected Cog Agent Addresses.
- Reuse the existing session-server ACP owners; never spawn or resume a competing
  provider.
- Preserve Cog's source-vector cursor semantics and stable attempt identity.
- Persist enough journal state to recover every dispatch/completion ambiguity.
- Fail closed and remain inert while capabilities is 404 or incompatible.

## Scope

**In:** typed v9 HTTP/SSE client, exact codecs, optional config, host/owner/claim
lifecycle, durable journal, session-manager provider bridge, supervision,
headless/mock verification, docs and runtime activation check.

**Out:** Cog server implementation/deployment; GUI controls; automatic address
registration; automatic return of external ownership; interpreting peer content;
new permissions or tool authority.

## Model

```text
Cog v9 HTTP/SSE
      │ typed + capability-gated
      ▼
Runtime coordinator ── fsync journal
      │ private correlated delivery command
      ▼
yalda-session-server Manager
      │ existing one ACP transport + WAL
      ▼
Codex / Claude provider session
```

## Tickets

| Ticket | Status |
|---|---|
| `001-ticket-runtime-adapter-v1.md` | in progress |

