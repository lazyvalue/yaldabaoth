# yalda-gpui — UX architecture (read before adding/altering any view)

This module is the GPUI surface. GPUI re-renders the **root every frame**
(`window.rs draw_roots`), and its *only* render-skip lever is
`AnyView::cached`. So the cost of an indelicately-built view is **O(whole tree)
per keystroke** — that is the performance trap this module is organized to make
hard to fall into. Background + the six verified GPUI 0.2.2 facts:
`docs/projects/gpui-responsiveness/project.md`.

## The one pattern: expensive surfaces are cached child entities

A surface that is expensive to render and usually stable while you type
elsewhere (the transcript; later: compose, status strip, split leaves) is its
own GPUI **view entity**, embedded in its parent via `cached_child(view)`. It
re-renders **only** when its own inputs change. The reference implementation is
`transcript_view.rs` (`TranscriptView`) — read it as the worked example.

Anatomy (what every cached surface has):

- **A model handle it reads, not owns.** `TranscriptView` holds
  `session: Entity<AgentSession>` and reads it in `render` via `.read(cx)`.
  Domain state lives in the model; only *UI* state (scroll/list/focus —
  `TranscriptScroll`) lives in the view.
- **An observe subscription that self-notifies on a slice change.** Registered
  in the constructor (`TranscriptView::new` → `cx.observe(&session, …)`). The
  callback computes a cheap fingerprint (`TranscriptSeqs::of`), diffs it against
  `last_rendered` (`diff_reason`), and calls `cx.notify()` **on itself** only
  when its slice moved. Observe callbacks run in effect flush — outside the draw
  — which is why this is timing-correct (facts 4–5).
- **A cached embed.** The parent (`render_agent`, `screens.rs`) does
  `cached_child(self.transcript_view_for(id, session, cx))`. Lazy-created in
  `transcript_view_for` (`main.rs`), dropped on `AgentSessions::close`.

## The rules (each maps to a real bug we shipped and fixed)

1. **Never `cx.notify()` inside a `render`/`build_body` path.** A notify issued
   mid-draw is *parked*: no effect that frame, no scheduled redraw — a stale
   frame until something unrelated happens (the rev-1 stale-tail bug). Notify
   from event handlers, `cx.observe` callbacks, timers, or `cx.defer` only.
   Pinned by `cached_notify_from_render_is_parked`. The incoming `CachedView`
   framework (see the spec) removes the `cx` from the build path so this can't
   be written at all — prefer it over hand-rolling `impl Render`.
2. **Every input `build` reads must be in the fingerprint.** A field read in
   `build_body` but missing from `TranscriptSeqs` ⇒ stale UI (this is exactly
   how the caret-glyph and stall-clock bugs happened). When you add a render
   input: add its seq to `TranscriptSeqs::of`, AND add a `transcript_021_*`
   regression test in `verify_harness.rs` asserting that input busts the cache.
   Globals (theme, zoom) aren't seqs — their action handlers push via
   `notify_transcript_views`.
3. **Embed via `cached_child(view)`** (size baked in). Never hand-roll
   `view.into_any().cached(style)` — a sizeless style collapses the panel.
4. **Interactive rows resolve state at event time, not capture it.** A cache
   hit reuses prepaint, whose listener closures captured the *previous* render's
   data. Tool-group expand / wiki links must act through ids/indices resolved in
   the handler (`cx.listener`), never through row data closed over in `build`.
5. **A new cached surface ships a render-count test.** Mirror
   `transcript_021_chatbox_keystroke_is_render_flat`: typing on an unrelated
   surface ⇒ this surface's `record_render` count stays **flat**. This is the
   enforced guard (CI runs `cargo test`); without it the surface has zero perf
   coverage.

## Don't hand-roll `impl Render` for an expensive surface

Use the `CachedView`/`Panel` abstraction (`cached_panel.rs`; framework spec:
`docs/specs/spec-gpui-panel-framework.md`). It owns the observe wiring, the
fingerprint diff, the `record_*` accounting, and the cached embed, and its
`build` method is handed no `Context` — so rule 1 is structural, not a thing you
remember. Hand-rolling a `Render` impl with a large inline element tree is the
mistake the whole module is shaped to prevent; if you think you need to, that is
a signal to extend the framework instead.

## Instrumentation

`record_render(label)` / `record_notify(label, MissReason)` (`cached_panel.rs`),
read in tests via `perf_render_count` / `perf_last_notify`. Run the live app
with `YALDA_PERF=1` to watch counts. Render *count* is a proxy, not frame time —
GPUI can't be driven headlessly for paint, so a real perf read is still a human
`sample` under `--release` (debug masks all wins).
