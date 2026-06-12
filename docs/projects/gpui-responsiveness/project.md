# Project: GPUI responsiveness — "super fast GUI"

Umbrella project to make `yalda-gpui` consistently fast on the hot path
(typing, scrolling, dragging). Born from a whole-surface multi-agent audit
(20 verified findings) — see `audit-report.md` (ranked roadmap; §2 rankings
still valid, §3 helper API superseded by rev 2 below) and `audit-findings.json`.

**Rev 2 (2026-06-11).** The invalidation design changed after a first-principles
review against the gpui 0.2.2 source plus external review (`codex-feedback.md`).
Rev 1's fingerprint-polled `CachedPanel` protocol (ticket 020, merged) is
retired: polling a fingerprint from inside `render()` violates GPUI's notify
timing (fact 4 below) and hand-rolls dependency tracking the framework provides
event-driven. Rev 2 works with the framework: **state in entities, invalidation
by observation, `cached()` as a thin render-skip wrapper.** See "Design
history" for what was kept/rejected and why.

## Root cause (the one model every ticket assumes — unchanged)

`YaldaGpuiView` is the **only** GPUI entity, so **any `cx.notify()` dirties the
root and re-runs the entire render + layout tree — there is no subtree skip.**
Every "typing/interacting in surface A re-lays-out unrelated subtree B" symptom
is this one cause. Prior virtualization + S1 memoization capped most costs at
O(visible), but they still fire on the most-touched surface (typing).
The durable fix is NOT to teach the root to diff its own state per frame
(rev 1); it is to **move per-surface state into entities so the framework's own
invalidation provides the granularity** — then the mega-entity problem
dissolves instead of being managed.

## GPUI 0.2.2 facts (verified against framework source — load-bearing)

1. **The root always re-renders.** `draw_roots` prepaints the root view
   unconditionally every draw (`window.rs:2013`). You cannot make the root
   skip; you make root render *cheap* and put the expensive subtrees behind
   `cached()` children.
2. **`AnyView::cached(style)` is the only render-skip lever** (`view.rs:102`).
   A cached child's `render()` is skipped and its prepaint reused iff its
   entity is NOT in `window.dirty_views` AND bounds/content-mask/text-style are
   unchanged (`view.rs:209-218`). The cached slot is sized **from the style**
   (`view.rs:170-176`) — it must carry `size_full`/`flex_1` or it collapses.
3. **Notify dirties the entity AND its ancestor views** (`mark_view_dirty`,
   `window.rs:1304`) — propagation is up-only. Notifying the parent/root never
   dirties a cached child. (Rev 1's "marks only that entity" was wrong.)
4. **The timing law: a `cx.notify` issued DURING a draw is parked.**
   `invalidate_view` (`window.rs:116`) under `draw_phase != None` inserts into
   the invalidator's *pending* set, does NOT set the window dirty flag, and
   does NOT push `Effect::Notify`. Three consequences: it cannot affect the
   current frame (`window.dirty_views` was drained at draw start,
   `window.rs:1915`); it does not schedule a next frame (the frame loop draws
   only when `is_dirty()`, `window.rs:1018`) — the stale frame persists until
   an unrelated event; and observers are skipped. ⇒ **Never notify from
   render.** Notify from event handlers, observe callbacks, timers, or
   `cx.defer` — all run at `phase == None` and land in `dirty_views` for the
   very frame their triggering event scheduled (zero frames late).
5. **Observation is timing-correct by construction.** `cx.observe`
   (`app.rs:780`) callbacks fire in effect flush (`apply_notify_effect`,
   `app.rs:1301`) — outside the draw — so `observe(model) → cx.notify()` on
   the view is the canonical cache-busting path.
6. **Accessed-entity tracking schedules redraws but does not bust caches.** A
   cached view's render records the entities it read (`view.rs`
   `detect_accessed_entities`; `window.rs:1983`); notifying a read entity
   schedules a redraw, but only a notify on the **view entity itself** lands in
   `dirty_views` (a non-view's `view_path` is empty). Hence rule 5: cached
   views invalidate themselves via observation.

## The component model (replaces rev 1's fingerprint helper)

One pattern for every surface; design new widgets to it:

- **State lives in entities.** Per-session domain state becomes
  `Entity<AgentSession>` (`SessionStore<P>` is payload-generic — the 1:1
  sid-binding invariant is untouched by the payload swap). Mutators run
  `session.update(cx, |s, cx| { …; cx.notify() })` — the notify happens at the
  mutation site (timing-correct, fact 4) and granularity is per-entity for free.
- **Views observe, filtered by version counters.** A surface view holds its
  model handle + `cx.observe`; the callback compares the monotonic seqs its
  render reads (`edit_seq`, frozen-gen, tools-gen, …) against what it last
  rendered and notifies itself only when *its slice* moved, logging the reason.
  This is the legitimate descendant of rev 1's `render_fp`: explicit typed
  counters compared at event time — not an opaque hash polled per frame.
- **Widgets own their UI state; models own domain state.** Scroll/list/focus
  state (e.g. `ListState`, `follow_output`) lives in the view entity; the
  transcript document/tools/turn state lives in the session entity.
- **Events out.** Widgets emit typed events (`EventEmitter` + `cx.subscribe`)
  instead of closing over root fields (compose emits Submit, etc.).
- **`.cached(size_full)` selectively.** Only expensive, usually-stable
  subtrees: transcript, App leaves. Cheap chrome (status strip) just renders —
  caching everything buys lifecycle complexity for nothing.
- **Time-varying visuals notify from their timer** (cursor blink, thinking
  tick): the timer task notifies the owning view — timers run outside the
  draw, so this is sound and scoped.

What survives from ticket 020: the `.cached()` embedding, the size-from-style
guard, and the headless proof that `cached()` skips render. What's deleted
(ticket 024): `FingerprintedPanel`, `notify_if_changed`, the parent-held
`last_fp` — the per-frame poll from render violates fact 4, duplicates state,
and its "call before layout" ordering contract is enforced by nothing.

## Orthogonal fixes (unchanged — do NOT use entities/caching for these)

Blocking I/O off the paint thread (clipboard in-process — shipped; browser
recursive `fs` walk → debounce + background executor + cancellation; it is an
I/O problem, not a render-cache problem); local memo guards; list
virtualization.

## Instrumentation (the definition of "verified fast")

`YALDA_PERF` counters: root render count, per-panel render count, cached
hit/miss with miss reason (dirtied / bounds / text-style / refresh), and
notify reason per panel (which seq moved). Headless tests assert render
counts (the `VIEW_MODEL_REBUILDS` / `PROBE_RENDERS` idiom); human `sample`
remains the paint-thread ground truth. Without the counters it is too easy to
finish a refactor while silently missing the cache path.

## Phases / tickets

| #   | Ticket                                                          | Phase | Risk | Status |
|-----|-----------------------------------------------------------------|-------|------|--------|
| 010 | Cheap wins: cwd OnceLock + frozen to_vec dedup + thinking-tick gate | 0 | low | **done** (merged `0dc2b97`) |
| 011 | Clipboard in-process for the 4 main.rs handlers (#4/#5)         | 1 | low | **done** (merged `0dc2b97`) |
| 012 | Deferred: WP-classify cache; browser filter debounce + bg walk (#2); vim yank/put clipboard | 0/1 | low-med | todo (`012-ticket-deferred.md`) |
| 020 | `CachedPanel` + render-skip proof test                          | 2 | med | **done** (merged `66c63b9`); fingerprint half superseded → 024 |
| 024 | Rework `cached_panel`: delete fingerprint layer; `cached_child` embed + counters + timing-law tests | 2 | low | **done** (merged `851f207`) |
| 025 | `Entity<AgentSession>` hoist: store payload swap, mutation-site notify; no behavior change | 2 | med | **done** (merged `851f207`) |
| 021 | `TranscriptView` entity: observes session, slice-filtered self-notify, cached embed (flagship; closes #1, #7, #8) | 2 | med-high | **done** — owner runtime-checked 2026-06-11 (merged `851f207`) |
| 022 | Compose widget: own entity owning the draft, emits Submit/mode events (#1/#7) | 2 | med | todo |
| 023 | Strip + thinking indicator: measure with 024 counters first; cache only if counters say so (#14) | 2 | low | todo |
| 030 | Each App leaf a view entity owning its screen state — dismantle the mega-entity (#8, #9); resize coalesce | 3 | high | todo |
| 040 | Opportunistic: expanded tool-group virtualization (#3), agent-picker window (#11), workspace-save off-thread (#15) | 4 | low | todo |

Order: 024 → 025 → 021 → 022 → 023; 012/040 anytime in parallel.

## Verification

- Headless: render-count assertions per panel (024 counters); the canonical
  protocol test (mutate model in update → observer notifies view → next frame
  re-renders fresh); the timing-law pin (a mid-draw notify does NOT invalidate
  — if a gpui upgrade changes this, we want a loud signal).
- Runtime (human, cannot self-verify): `sample` while typing in a large
  transcript (no per-keystroke transcript layout) and **streaming-tail
  freshness** — the last append of a turn must appear without a mouse wiggle
  (the rev-1 hazard). Phase 0/1 still owes its runtime copy/paste check.

## Design history (why rev 2)

Rev 1 (ticket 020) gated a cached child on a `render_fp` polled from inside the
parent's render. Source review showed that protocol is unsound per fact 4: the
mid-draw notify lands this frame's change one-or-more frames late and can
strand a stale frame indefinitely (worst case: the final streaming append of an
agent reply staying invisible until the next unrelated event). The proof test
passed because it notified from outside the draw — the one calling pattern the
contract forbade. The deeper issue: a hand-maintained fingerprint is a manual
dep-array with the classic missed-dependency failure mode, compensating for
state the framework can't observe because it lives on the root mega-entity.
External review (`codex-feedback.md`) independently caught the notify-wording
error and contributed the version-counter, selectivity, and miss-reason
instrumentation points (adopted); its recommendation to keep update+notify in
the render path is rejected as unsound, and its compose-long-draft concern is
stale (fixed by the shipped compose virtualization, `a72d583`/`aa2ae08`).

## Links

- `audit-report.md`, `audit-findings.json`, `codex-feedback.md` (this dir)
- ADR-0019 (Tiles & Apps), ADR-0020 (INV-PR/INV-RV), `spec-agent-session-ownership.md`
- gpui 0.2.2 source: `~/.cargo/registry/src/…/gpui-0.2.2/src/{window.rs,view.rs,app.rs}`
- Memory: input-latency-profiling, durable-architecture
