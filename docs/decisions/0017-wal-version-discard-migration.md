# ADR-0017: WAL schema migration by version-bump discard, not a converter

**Status:** Accepted
**Date:** 2026-06-08
**Related:** ADR-0009 (durable WAL), spec-event-stream.md §12, phases 4 + 8

## Context

The durable per-session WAL (ADR-0009, `src/session_wal.rs`) carries a `version`
header from day one. Phase 4 (lease state) and phase 8 (the `AgentEvent`
collapse) each change the on-disk WAL shape — breaking, lockstep wire+WAL
changes. spec-event-stream.md §12 required either a one-time reader for the old
shape **or** an explicit discard. We needed a migration policy that doesn't
become a per-phase tax.

## Decision

On a WAL `version` bump, **discard pre-version logs on read** — a daemon at
version N drops any WAL with version < N, and that session resumes empty (the
agent re-loads its own history via `session/load`). **No converter is written.**
The cutover is done at a quiet moment (nothing precious live). Applied: 1→2
(phase 4 lease), 2→3 (phase 8 `AgentEvent`).

## Rationale

This is a self-hosting dev tool, not a product holding users' irreplaceable
data; the WAL is a transcript **cache**, and the agent can re-load history. A
converter is real code to write and test for a format still churning across
phases — cost with little payoff when the worst case is "resume empty." Discard
is zero migration code, and it was verified **live**: the v1→v2 discard fired
correctly during the phase-4 runtime check (`discarding pre-v2 WAL … session
resumes empty`), logged, no corruption.

## Alternatives rejected

- **One-time converter per version** — real code + tests for a churning format;
  the recovered data (a transcript cache the agent can rebuild) isn't worth it
  yet.
- **Keep multi-version readers indefinitely** — accumulates a dead code path for
  every historical shape; the format is still moving.

## Consequences

Each breaking WAL change is a **deliberate cutover** that wipes existing sessions
to empty — it must be done at a quiet moment and flagged loudly, not shipped as a
silent deploy. New discipline: a WAL-bumping merge is a "cutover" item in the
backlog (e.g. phase 8's pending v2→v3). The header `version` gate is the only
migration machinery. **Revisit this** the day the WAL holds irreplaceable state
(not a re-loadable transcript) — at that point a converter becomes worth writing.
