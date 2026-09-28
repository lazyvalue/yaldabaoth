# UXP-3 — A view re-renders whenever any input it reads changes (no stale render)

**Statement.** Every value a view reads while rendering has an invalidation path:
when it changes, the view is notified and repaints. Cached views track each input
by a monotonic seq or an explicit notify at the mutation site; global inputs
(theme, zoom) are pushed by a `notify_*_views` walk. `cx.notify()` is never called
from inside a render path (it is parked and has no effect).

**Applies to.** Every GPUI view, and especially cached child entities
(`yux/cached.rs::cached_child`, `TranscriptView`, `LinearView`, `DiffView`).

**Why.** Stale tail, stale caret glyph, stall clock: each shipped because a render
input had no invalidation (legacy INV-UX-23; `src/bin/yalda-gpui/yux/CLAUDE.md`).

**Status.** `implemented` for the existing cached surfaces.

**Enforcement.** Each cached surface ships a bust test per input, e.g.
`verify_harness.rs::transcript_021_session_edit_busts_cache`,
`transcript_021_theme_and_zoom_bust_cache`,
`transcript_dropped_notify_id_forces_render`; the render-path rule is pinned by
`cached_notify_from_render_is_parked`.

**Realized by.** `UXI-AgentTile-7`.
