# yux — yalda's reusable UX layer (read before building ANY UI)

**All UX work in `yalda-gpui` is built from yux, and contributes back to it.**
If you reach for a raw GPUI element tree in a feature module, stop: either a yux
primitive already exists, or the thing you're about to write *is* the next yux
primitive. The goal is reuse and DRY — one cached-panel implementation, one set
of detail primitives, one place the responsiveness rules are enforced.

## What lives here

### `cached.rs` — the render-skip infrastructure
GPUI re-renders the **root view every frame**, so without intervention every
surface is O(whole tree) per keystroke. The one lever that breaks that coupling:

- **`cached_child(view)`** — embed a GPUI view entity as a cached element
  (bakes in `size_full`). Its `render()` is skipped unless the entity notifies
  *itself*. `cached_child_styled(view, style)` for non-fill sizes (style MUST
  carry a size or the panel collapses).
- **`record_render(label)` / `record_notify(label, MissReason)`** — the perf
  counters. `record_render` at the top of every cached `render()`;
  `record_notify` at every notify site. Read in tests via `perf_render_count` /
  `perf_last_notify` / `perf_reset`; watch live with `YALDA_PERF=1`.
- **`MissReason`** — why a cached panel invalidated (`Dirtied` / `Bounds` /
  `TextStyle` / `Refresh`).

### `detail.rs` — reusable view primitives
Domain-free building blocks for any read-only detail surface, all driven by one
style bundle so a caller themes once:

- **`DetailStyle`** — `{ fg, dim, accent, err, mono, prose, base, pt }`, the
  resolved colors/fonts/size snapshotted once per render.
- **`multiline_text(text, color, font, base)`** — render `\n`-separated text as
  one child per line (a bare `SharedString` collapses newlines). Empty → `—`.
- **`kv_row(label, value, &DetailStyle)`** — fixed-label / value row.
- **`section_heading(text, &DetailStyle)`** — underlined section header.
- **`compact_tab(id, label, indicator, selected, selected_bg, &DetailStyle)`** —
  equal-width compact tab chrome with an optional inline indicator; selection
  is carried by its background and every label keeps normal foreground
  contrast. The caller owns the enclosing tab group.
- **`compact_count_indicator(id, count, tint, &DetailStyle)`** — compact semantic
  number pill for tabs and other narrow chrome.
- **`compact_bounded_group(id, header, body, outline, separator)`** — flat,
  compact hierarchy card whose optional body is spatially enclosed beneath its
  header with shared outline, spacing, clipping, and separator.
- **`compact_list_group_heading(id, glyph, label, count, tint, &DetailStyle)`**
  — small uppercase label + count + trailing hairline for headed dense lists.
- **`context_menu_item(id, glyph, colors, label, font)`** — shared compact
  popup-row chrome for cursor context menus. The caller owns dispatch and the
  popup shell.
- **`picker_option_row(id, glyph, label, badge, selected, colors, fonts)`** —
  shared accent-rail row chrome for keyboard-first picker cards. The caller
  owns the option model and dispatch.
- **`completion_popup(id, rows, selected, colors, mono)`** — shared compact
  completion shell + primary/secondary rows for keyboard-owned input
  suggestions. The caller owns query/filter state and key dispatch.
- **`note_block(author, when, body, &DetailStyle)`** — author · timestamp over a
  multiline body (comments, updates, log entries).
- **`fmt_iso_datetime(&Option<String>)`** — ISO-8601 → `YYYY-MM-DD HH:MM`.

### `list.rs` — virtualized scroll surfaces
- **`ScrollAnchoredList<T>`** — a `gpui::list` that re-syncs to a new item
  sequence by SPLICING the changed range (shared prefix/suffix trimmed), never
  `reset()`. `reset()` nulls the scroll offset + unmeasures every row, so a
  same-frame `scroll_to_reveal_item` lands at item 0 — the "viewport jumps to
  the top on every newline" bug. One per scroll surface (Edit view, Doc view,
  compose box). All methods take `&self` (interior-mutable), so a `&self` render
  path reconciles fine. `reconcile(items, seq)` is gated on `seq` (idle frames
  are no-ops). Consume via `.state()` (paint/reveal/scroll) + `.len()`.
- **`splice_list_to_items(&ListState, old, new)`** — the bare splice primitive,
  unit-testable against a raw `ListState`. The agent transcript's
  `TranscriptScroll` reconciles by item COUNT (streaming tail + follow-output),
  not a content diff, so it stays separate — don't force it onto this.

## Efficiency practices (non-negotiable)

1. **O(changed), never O(whole tree).** An expensive surface that is usually
   stable while you interact elsewhere is its own cached view entity embedded
   with `cached_child`. The reference consumers are `transcript_view.rs`
   (`TranscriptView`) and `linear_view.rs` (`LinearView`).
2. **Never call `cx.notify()` inside a `render()`/build path.** A notify issued
   mid-draw is *parked* — no effect that frame, no scheduled redraw. Notify only
   from event handlers, `cx.observe` callbacks, timers, or `cx.defer`.
3. **A cached view busts its own cache, and only its own.** Notifying the parent
   never dirties a cached child (dirty propagates *up*). So a child invalidates
   by being notified itself — at a mutation site (`view.update(cx, |v, cx| { …;
   cx.notify() })`) or in its `cx.observe(&model)` callback.
4. **Global inputs are pushed, not polled.** Theme / zoom aren't per-component
   state; their action handlers walk the live components and notify each
   (`notify_transcript_views` / `notify_linear_views`) with the right
   `MissReason`. Don't read a global inside a cached render without a push path
   to invalidate it.
5. **Every new cached surface ships a render-count test.** Mirror
   `transcript_021_*` / the linear render-count test in `verify_harness.rs`:
   interacting with an *unrelated* surface must leave this surface's
   `perf_render_count` **flat**. No test ⇒ zero perf coverage ⇒ the regression
   is only a matter of time.

## State encapsulation (the component model)

A yux component is a GPUI **view entity** with a strict ownership split:

- **It owns its UI state** (scroll position, expand flags, the loaded
  payload) — nothing outside reaches in and mutates it. Callers interact through
  methods/`update`, never by poking fields from the parent's render.
- **It reads, doesn't own, the global chrome.** Theme/fonts/zoom come from the
  root view via a `WeakEntity<YaldaGpuiView>` read in `render()` (snapshot into
  owned locals, release the borrow, then build). It never copies the theme into
  itself.
- **It self-invalidates.** The owner of a piece of state is the only one that
  notifies for it (rule 3). This is what makes the component composable: drop it
  anywhere, and it stays correct and fast without the parent knowing its
  internals.
- **Interactive rows resolve state at event time.** A cache hit reuses prepaint
  whose closures captured the *previous* render's data — so click handlers must
  act through ids/indices resolved in the handler via the weak root handle,
  never through row data closed over at build time.

## The contribution mandate

Building UX = extending yux. When you build a surface:
- **Compose from existing primitives first.** Need labels+prose+sections? That's
  `kv_row` + `section_heading` + `multiline_text` — don't hand-roll divs.
- **Promote anything reused twice.** The moment a shape appears in a second
  surface, it becomes a `detail.rs` primitive (decoupled from any domain type,
  parameterised by `DetailStyle`). DRY is the point.
- **Domain views live in their feature module but are built ON yux** and follow
  every rule above. `LinearView` is Linear-specific so it lives in
  `linear_view.rs`, but its body is `detail.rs` primitives and its caching is
  `cached.rs` — the *reusable* parts are here, the *domain glue* is there.
