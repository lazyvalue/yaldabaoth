# ADR-0006: Agent interaction is a sourced-once, folded event stream

**Status:** Accepted
**Date:** 2026-06-05
**Related:** spec-state-architecture.md (decision D1), ADR-0003 (ACP durability), the constant-regression diagnosis

## Context

Agent interaction is *already* an event stream — `ReplyEvent` (acp_channel.rs:147),
wrapped as `ServerNotification`, durably logged in the server's `event_log`, and
folded by `apply_reply_events` / `ReplayTurns` / `UserTurnReconciler`. The defect
is not the absence of a stream; it's that **the same fact is re-synthesized at
each layer instead of sourced once from its authoritative origin and folded.**

Turn-end is the exemplar. The ACP worker awaits a single `session/prompt` RPC
that resolves exactly once, at the real turn boundary, with a
`PromptResponse { stopReason }` (acp_channel.rs:1388). The worker stands on that
boundary, discards it (`turns.fetch_add` at :1435), and then **three** separate
pumps re-derive "a turn ended" from the heuristic *"queue went momentarily empty
AND the counter climbed"* — including the server, which re-synthesizes its own
`ServerNotification::TurnEnded` from the counter rather than forwarding the
worker's. That inference caused the mid-replay false-finalize (patched with the
`ReplayComplete` band-aid) and a post-respawn wedge on the GUI direct path. More
broadly, ~30% of app state is hand-synced copies/derived-caches — the same
disease in many costumes.

## Decision

**Principle.** Agent interaction is a typed event stream. Each state transition
is exactly one variant in a canonical vocabulary, **emitted at one authoritative
source**, **forwarded verbatim** across transport hops (worker → server → GUI),
and consumed by **total reducers** (a compile-time exhaustive `match`, e.g.
`apply_reply_events` and the pure sub-reducers). No consumer infers a transition
outside the fold. "Listeners" means these typed reducer functions — *not* a
runtime pub/sub bus; the win is exhaustiveness (a new variant forces every
consumer to handle it), the opposite of three sites quietly inferring and
drifting.

**First application (D1) — turn-end.** Emit `ReplyEvent::TurnEnded { count,
generation }` at acp_channel.rs:1435, where the worker already observes the
`session/prompt` resolution. The server **forwards** it (stops re-inferring its
own `TurnEnded` from the counter); the GUI reducer consumes it; the
"queue-empty + counter-climbed" inference is deleted from all three pumps. The
`generation` token lets every consumer rebaseline uniformly after a channel
respawn (closing the wedge). Rollout is **additive**: emit alongside the existing
inference, assert/log that they agree across a few real sessions (including a
resume and a tool-only turn), then delete the inference.

ACP guarantees **one `session/prompt` = one turn = one response** — tool calls
stream as `session/update` notifications *while the request is pending* — so
tool-only and compaction turns map to a single clean boundary by construction.
The "does turn-end map cleanly?" open risk is therefore retired, and the additive
phase is belt-and-suspenders rather than genuine risk.

## Rationale

The authoritative boundary already exists in our own code; we were paying to
throw it away and re-derive it lossily three times. Sourcing once and folding
fixes both observed bugs at the root (not per-pump) and makes the event
vocabulary exhaustive. The server's `event_log` is already the durable instance
of this exact model, so this is also principle #1 of the state overhaul
("transcript is a projection of the log, not a copy") applied to the live stream.

## Consequences

- **The same pattern is adopted next at these sites (quick wins, ranked):**
  1. `subagents` becomes a derived projection (fold) of `tool_calls`, deleting
     the hand-synced mirror — pure, local, unit-testable.
  2. `DocState.blocks` derived from `Document.edit_seq` (memoized fold), deleting
     manual `invalidate_blocks_snapshot` — pure-ish, no behavior change.
  3. Server `record()` fuses `event_log` push + broadcast so "an event happened"
     has one mutator — server-local.
  4. Channel permission policy re-applied from its single owner on every channel
     swap (`spawn_channel_then_apply_state`) — fixes the silent-revert-on-restart
     bug; server-local.
  5. `FileBrowser` `filtered_indices`/`search_results` rebuilt by one method from
     `(entries, filter_text)` — small, local.
  These are tracked against the spec's migration steps.
- **Not** a pub/sub / observer framework: compile-time total reducers only.
- Later steps (`tool_calls`, `agent_view_model`, reconciler extraction) inherit
  this principle rather than re-litigating it.
- `ReplyEvent` gains a `TurnEnded` variant; the server forwards rather than
  synthesizes; the three inference sites are removed once the additive phase
  confirms agreement.
