# 024 — Rework `cached_panel`: thin embed helper, counters, timing-law tests

Retires the rev-1 fingerprint protocol (see `project.md` "Design history").
`src/bin/yalda-gpui/cached_panel.rs` becomes a small, misuse-proof wrapper
around GPUI's one render-skip lever, plus the instrumentation every later
ticket verifies against. No consumer exists yet besides the proof test, so
this is low-risk and unblocks 025/021.

## Goal

- `FingerprintedPanel`, `CachedPanel::notify_if_changed`, and the parent-held
  `last_fp` are gone (they violate the timing law — `project.md` fact 4 — and
  duplicate state).
- One embed helper remains: `cached_child(view) -> AnyElement` that bakes in
  `size_full` (an explicit-style variant for non-fill cases). Sizing stays part
  of the visible contract; misuse (sizeless style) stays impossible by default.
- Module docs state the corrected facts: notify dirties entity **and
  ancestors** (up-only); never notify from render; invalidate via
  `cx.observe(model) → cx.notify()` on the view, or at mutation sites.
- `YALDA_PERF` counters exist for later tickets to assert against.

## Subtasks

- [x] Delete `FingerprintedPanel` + `notify_if_changed` + `last_fp`; add
      `cached_child(view)` / `cached_child_styled(view, style)`.
- [x] Rewrite module docs: facts 2–6 from `project.md` with gpui file:line
      (`view.rs:103/170-176/209-212`, `window.rs:116/1304/1926/128`,
      `app.rs:780/1301`).
- [x] Counters (debug/`YALDA_PERF`): per-entity render count + cached hit/miss
      with miss reason (dirtied / bounds / text-style / refresh — infer: we
      can't see inside gpui, so count renders + record last-notify reason at
      our notify sites; document the inference limits).
- [x] Tests in `verify_harness.rs`:
  - [x] Keep the render-skip proof (parent notify does not re-render the
        cached child) — rewritten to the rev-2 API
        (`cached_panel_skips_render_until_child_is_notified`), green.
  - [x] **Canonical protocol:** mutate a model entity inside `update` →
        `cx.observe` callback notifies the view → next frame re-renders fresh
        (`cached_observe_protocol_busts_cache_fresh`).
  - [x] **Timing-law pin:** a `cx.notify` issued from *inside* a render does
        NOT invalidate that frame and does NOT schedule a redraw on its own
        (`cached_notify_from_render_is_parked`). Pins the gpui behavior rev 1
        tripped over; a gpui upgrade that changes it fails loudly here.
- [x] Build + full test suite (177 bin tests + full workspace suite green).

## Verification

`cargo test` green; run the GUI with `YALDA_PERF=1` and confirm counters tick.
No runtime behavior change expected (helper has no live consumers).

## Links

`project.md` (facts 2–6, component model), ticket 020 (what shipped),
`codex-feedback.md` (instrumentation + miss-reason points adopted here).
