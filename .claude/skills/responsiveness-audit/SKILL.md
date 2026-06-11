---
name: responsiveness-audit
description: Whole-surface, symptom-agnostic audit of the GPUI app for UI-responsiveness invariant violations — O(changed) work, virtualization, no blocking I/O on the paint thread, no per-frame re-derivation, coalesced input. Use when the user wants to hunt latency/jank/stalls broadly, "audit responsiveness", or "run the tachyon reviewer". Report-only; the user picks what to fix.
---

# Responsiveness audit (the "tachyon reviewer")

Audit yalda for violations of UI-responsiveness invariants. You are a
responsiveness savant: zero-blocking, zero-jank interfaces are the standard.
**Report only** — the user picks what to implement afterward.

## The one rule that makes this skill worth running: be SYMPTOM-AGNOSTIC

This skill exists because of a real failure (see `docs/decisions/0004-*` and
`docs/dev-system.md` § Parallel work): a perf fan-out of dozens of agents missed
a textbook O(document)-per-keystroke bug because **every prompt inherited the
reported symptom's framing** and aimed every agent at the same code region.
Diverse lenses over identical scope = one search run N times, with a shared
blind spot.

So this audit is **invariant-driven, not symptom-driven**. Do NOT anchor on any
reported symptom or any one feature. Sweep EVERY surface against EVERY invariant,
with no prior about where the problem is. Breadth of *surface* is the whole point.

## The invariants (check every surface against all five)

1. **O(changed/visible), not O(total).** Per-frame and per-event work scales with
   what changed or what's visible — never with total content (doc length, list
   size, file count, transcript length).
2. **Virtualization.** Large/long content builds elements only for visible rows.
3. **No blocking I/O on the paint/UI thread.** No synchronous disk read/write,
   directory scan, or socket round-trip in render or in a key/mouse/scroll handler.
4. **No redundant per-frame re-derivation.** Re-parse / re-highlight / re-read /
   re-derive every frame when inputs are unchanged is a violation — cache behind a
   cheap change-key (e.g. `edit_seq`).
5. **Input never blocks; work is coalesced.**

## Process

1. **Enumerate every surface — don't anchor.** Grep the render/input/persistence
   entry points: every `fn render_*`, every action/key/mouse/scroll handler, every
   `save_*`/`snapshot_*`/persistence call, every file/dir/socket access. The floor
   (extend it): Doc view, Edit (code+WP), Browser file list, rail/outline,
   overlays, tab/workspace strip, status bars, multi-home dot, mouse/selection/
   scroll handlers, persistence, file-open/reload paths.
2. **Gather "already handled"** from `docs/backlog.md` + recent branches, and pass
   it to the reviewers so they hunt the *others*, not known-fixed paths.
3. **Pick run mode — prefer surface fan-out.** For true surface diversity, dispatch
   **one agent per surface (or surface group)** so each gets independent deep
   attention — not one agent over everything (that reintroduces a single blind
   spot). Use a Workflow if the surface count is large and the user has opted into
   workflows; otherwise a few parallel `Agent` calls. Each reviewer checks ALL five
   invariants against its surface.
4. **Verify before trusting.** Re-check each finding against the cited code; drop
   the ones that don't hold; the real input magnitude must actually make it bite.
5. **Synthesize** one ranked report; the user picks what to fix.

## Finding contract

Each confirmed finding: **location** (file:line) · **invariant violated** (#1–5) ·
**cost & trigger** (the O() and the realistic input size that makes it bite) ·
**fix** (concrete, citing the in-repo pattern to copy — `HighlightCache`,
`ListState` virtualization, off-thread `background_executor().spawn`, `edit_seq`
memoization, debounced/async persistence) · **UX/correctness risk** ·
**behavior-preserving?** · **effort** S/M/L · **confidence** high/med/low.
Plus a short speculative/low-confidence section. A clean surface is useful signal —
say so.

## Constraints

- Report-only; never edit code in this skill.
- Every finding cites code; raise only where the real magnitude matters.
- The **verification harness** (`src/bin/yalda-gpui/verify_harness.rs`) is the
  framing-proof empirical backstop — prefer turning a confirmed finding into a
  harness latency/recompute-count gate when it's implemented.
