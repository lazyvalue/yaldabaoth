# 2026-06-24 — Worksheet Model C: uncorruptible ordering

## What

Re-derived **Model C** onto `main`: the worksheet is now a **read-only,
append-only transcript + a separate `Compose` draft buffer**, with one ordering
invariant (INV-ORDER) pinned by headless tests. This structurally eliminates the
recurring worksheet ordering corruption (a newer turn rendering above an older
exchange; the draft stranded mid-history).

Branch `worksheet-modelc` → merged to `main`.

## Why

Root cause was one mutable rope holding BOTH the immutable ordered log AND the
live draft, reconciled by position-based heuristics with no invariant and no
test. See ADR-0024 + `docs/projects/worksheet-modelc/project.md`. A complete
Model C had been built on `worksheet-redesign` but never merged and went 27
commits stale.

## How

- **Approach:** a 3-way `git merge` of the stale branch surfaced every divergence
  as a conflict (vs hand-retyping, which risks silent drops). 11 conflict hunks,
  resolved to PRESERVE recent orthogonal work and TAKE Model C for the core:
  - kept main's caret-containment compose render + added `Compose::seeded`
  - kept the typed `/clear` intercept (session-recall A2) in `submit_compose`
  - kept strict-1:1 + roster restore handling; added Model C `compose_draft`
    restore (`InputSurface::with_draft`)
  - kept jump-panel `pending_jump` seq; added Model C `transcript_focused` seq
- **Read-only transcript:** `composing = false` in `rebuild_agent_view_model`
  (the transcript never hosts the inline-compose divider); paste/copy target the
  compose (INV-1); `should_follow_tail` collapsed to `follow_output`.
- **Ordering verified structural:** `agent_tail_floor_char` returns EOF whenever
  there's no untagged user text in the transcript — and the draft now lives in
  the separate compose — so all agent streaming appends at the bottom.
- **"You" divider** re-homed from a transcript flat-item to the inline compose's
  top-edge label (the user's explicit ask).
- **Deleted** Model-A machinery (`submit_worksheet`, live per-line freeze,
  worksheet→transcript routing, transcript divider, `snap_nav`). Removed 2
  Model-A tests that asserted the in-transcript divider/interjection.

## Evidence

```
cargo build --bin yalda-gpui   # OK (7 dead-code warnings from deleted machinery)
cargo test  --bin yalda-gpui   # 256 passed; 0 failed; 1 ignored
cargo test  --lib              # 142 passed; 0 failed; 2 ignored
```

Keystone tests (would FAIL against the old shared-rope model):
- `inv_order_streaming_with_draft_appends_at_eof` — with a non-empty compose
  draft, `agent_tail_floor_char == EOF` and a newly streamed turn appends at the
  bottom; the draft never enters the transcript.
- `inv_order_interleaved_turns_stay_chronological` — U/A turns land in
  occurrence order regardless of a held draft.

## Runtime-unverified (human check needed — GPUI not headless)

Enter worksheet; resume a multiturn session; type a draft and confirm it does NOT
appear in history; toggle Worksheet⇄Chatbox (draft preserved); send and confirm
the new turn appends at the bottom; confirm the inline "You" label renders.

## Follow-ups

- The inert floor machinery (`agent_tail_floor_char` /
  `append_llm_chunk_floored`) can be simplified to a plain EOF append now that no
  draft lives in the transcript — cosmetic cleanup, deferred.
- A render-side presence test for the inline "You" label (verify_harness) —
  deferred (GPUI render assertions are limited headlessly).
- Closes the worksheet-frozen-blocks F1/floor backlog items as **superseded**
  (the machinery they patched is gone).
