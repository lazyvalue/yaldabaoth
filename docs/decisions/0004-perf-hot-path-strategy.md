# ADR-0004: Perf strategy — the O(changed) contract, and synthesis over parallel fan-out

**Status:** Accepted
**Date:** 2026-06-02
**Related:** highlight_cache.rs, render_agent (main.rs), editor.rs, docs/research/refactor-review-perf-hot-path.md

## Context

The app slowed badly once an agent session was streaming — the symptom was
super-linear cost as the transcript grew. Investigation found the per-frame /
per-chunk work scaled with transcript length (O(n²) over a turn): full-transcript
clones on every chunk, the highlight cache re-tokenizing every line, an anchor
map rebuilt per newline, the whole `render_agent` view-model re-derived every
frame, and a 120ms animation tick forcing full re-renders.

## Decision

- **Guiding invariant: the agent render/stream path is O(changed), not
  O(transcript).** Per-frame work must be O(visible + changed lines); per-chunk
  append must be O(chunk). Any new code on this path is held to that contract.
- Concrete fixes (landed on `perf` + `perf-tachyon`): O(1) rope tail probes;
  byte-scan `advance_fence`; in-place O(shifted) anchor shift; coalesced
  same-session reply application + once-per-drain follow-scroll; ~1Hz-gated
  animation tick; **memoize the render_agent view-model** behind a fingerprint
  of `(edit_seq, frozen_count, tool_call_order, expanded set, awaiting_reply)`.
- **Convergence:** when parallel agents must touch the same code, plan an
  explicit *synthesis* step that takes the best implementation of each.

## Rationale

A single O(changed) contract, enforced by `edit_seq`-style fingerprints, is one
sharp abstraction replacing many independent O(n) passes — and it's testable
(rebuild-counter guard tests). Memoization is behavior-preserving *if the
fingerprint is complete*; the structural inputs were verified (cursor/selection/
theme/tool-content are read later, not baked into the flat list).

## Alternatives rejected

- **Merge all three parallel perf branches** — they overlapped (two rewrote
  `LineAnchorStore`); a blind merge would double-apply/conflict. Synthesized instead.
- **Speculative pre-tokenize / frame-budgeted replay (tachyon R1/R2)** — marginal
  after memoization; deferred until measured.

## Consequences

- **Lesson: decompose parallel work by file/module ownership, not by concern.**
  Splitting perf by concern (event-loop/threads/render) put all three in the
  same hot path and forced the synthesis tax. Now a documented rule (dev-system.md).
- None of the perf gains are runtime-verified (no headless GPUI) → the
  verification harness is the top backlog item.
