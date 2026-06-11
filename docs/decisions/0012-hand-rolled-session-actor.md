# ADR-0012: Hand-roll the session-server actor (don't adopt an actor framework)

**Status:** Accepted
**Date:** 2026-06-07
**Related:** spec-session-server-actor.md (the actor model this implements), ADR-0009 (durable session log — the actor is its single writer), spec-event-stream.md (the emit chokepoint the actor owns)

## Context

spec-session-server-actor.md proposes replacing the session server's shared
`Mutex<HashMap<SessionId, Session>>` with a single-writer **actor**: one task owns
the session map; all mutations arrive as messages. Before building it we surveyed
the Rust actor-framework landscape (four parallel evaluations against our actual
requirements) to decide adopt-vs-hand-roll.

Our shape is narrow and specific:
- **One** long-lived Manager actor + N *dumb* per-session source threads. Low
  actor count.
- Already on a multi-thread `#[tokio::main]` runtime.
- A **sync→async bridge**: each per-session OS thread owns a `!Sync`
  `std::sync::mpsc::Receiver` and forwards agent events into the actor.
- The actor is the **single writer of the durable WAL** (ADR-0009); turn-boundary
  `fsync` must complete in-actor before the ack.
- **Process** supervision is launchd's job (see spec § Single-instance), not the
  framework's.

## Options considered

- **actix (0.13.5)** — REJECTED. Requires `actix-rt`/`LocalSet`; does not coexist
  cleanly with our multi-thread `#[tokio::main]` (the documented
  "`spawn_local` outside a `LocalSet`" failure). Would force a second runtime or
  surrender our top-level runtime. Core actor crate is in low-activity maintenance.
- **ractor (0.15.x)** — capable, tokio-native, active (Meta-backed, single
  maintainer). But its value is supervision trees + clustering + many-actor
  ergonomics; restart policy is hand-written anyway; unbounded mailbox, no
  backpressure. Overkill at one-actor scale.
- **kameo (0.20)** — the best framework fit and the rising tokio-native option
  (typed `ask`/`tell`, `ActorRef::blocking_send` for our bridge, built-in
  supervision). Downsides at our scale: pre-1.0 breaking churn (~bi-monthly),
  default-64 mailbox footgun, and most of its value (supervision/distribution)
  unused.
- **xtra (0.6)** — lightweight, mature/quiet, but does NOT solve our hardest
  point (the `!Sync` blocking-thread bridge needs `block_on` glue), so marginal
  over hand-rolled.
- **coerce / riker** — distributed-first / abandoned (riker last release 2020).

## Decision

**Hand-roll the actor** with the bare tokio pattern the spec already yaldaes: a
`Command` enum, a `tokio::sync::mpsc` inlet, a `tokio::spawn`ed `loop`, and
`oneshot` for request/response replies. ~150 LoC, no new dependency.

Rationale:
- The pattern fits our topology exactly; a framework's per-actor savings amortize
  over *many* actors and *declarative restart policies* — we have neither.
- Our hardest integration point is *better* hand-rolled: `tokio::sync::mpsc::Sender`
  is `Send + Clone`, so each bridge thread holds a clone and calls
  `blocking_send()` — no async context, no glue. Frameworks either fight this
  (xtra) or it's the one idea we'd lift anyway (kameo).
- The WAL's fsync-before-ack constraint gets no help from any framework — same
  `spawn_blocking`-and-await (or dedicated WAL lane) design either way.
- Hand-rolled is the most headlessly testable (construct the handle in a
  `#[tokio::test]`, drive it; pure `apply_event(&mut state, …)` handlers unit-test
  with zero async) — aligns with the verification-harness priority.

## Consequences

- We own ~150 LoC of actor scaffolding instead of a dependency. The things we
  forgo (supervision trees, links, registries, ask-timeouts) are either unneeded
  at one-actor scale or trivially re-added (`tokio::time::timeout` around the
  reply `oneshot`).
- **Backpressure is now our responsibility:** use a bounded inlet and a deliberate
  overflow policy (the spec's slow-subscriber-disconnect rule), since we don't get
  a framework's mailbox semantics.
- **Revisit trigger:** if the design grows to many supervised per-session *actors*
  with real restart policies, adopt **kameo** (tokio-native, rising, supervision
  built-in) — not actix (runtime conflict). Until then, no framework clears the bar.
