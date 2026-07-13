# Component: Text Zoom (common)

**Status:** implemented
**Component token:** `TextZoom` (⇒ `UXI-TextZoom-N`)

## Description

Document text-zoom scales the body and heading text sizes of the reading
surfaces by a global `text_scale`: the buffer doc/edit views AND the agent
transcript (conversation prose + markdown blocks) grow and shrink together with
`Cmd-=` / `Cmd-+` / `Cmd--`. Chrome stays at native size — gutter labels, tool-card
status glyphs, the right sidepanel, the status footer, and the compose input do
not scale. The zoom is GLOBAL app state, not per-session, and is pushed to every
live transcript view rather than carried as a per-session seq.

## References

- INV-UX-13 in `docs/ux-invariants.md` → migrated here.
- `docs/components/agent-tile/README.md` — the transcript facet consuming this.

## UX invariants

### UXI-TextZoom-1 — Document text zoom scales the agent transcript, like a buffer

**Statement.** `Cmd-=` / `Cmd-+` (in) and `Cmd--` (out) — the document text-zoom
`text_scale` — scale the **agent transcript** the same way they scale the buffer
doc view: the conversation **prose** and the transcript's **markdown blocks**
(headings / code / tables) multiply by `text_scale`. Zoom is GLOBAL (not session
state): its action handler pushes `notify_transcript_views(TextStyle)` so every live
`TranscriptView` re-renders and re-reads `text_scale` off the root (via
`RootSnapshot`) — it is NOT a per-session `TranscriptSeqs` seq. As with buffers,
**chrome stays at native size**: the turn/tool gutter labels, tool-card status
glyphs, the right sidepanel (Plan/Subagents), the status footer, and the **compose input** (its caret
and line-box are pixel-pinned for caret-containment — INV-UX-1 — so its font is held
fixed; scaling it would require scaling the caret + `CHATBOX_CHAR_W` in lockstep, a
separate change). `Cmd-0` resets zoom everywhere EXCEPT agent tiles, where it is
panel-focus (INV-UX-12) — zoom-out then back is the reset there.

**Applies to.** `transcript_view.rs`: `RootSnapshot.text_scale` (read from
`root.text_scale`), the per-line `text_size(px(13.0 * text_scale))` on the
`FlatItem::Line` **row wrapper** (NOT just `claude-body` — `gpui::list` items do
not inherit the list's ambient text size, so the size must live on the item, the
same way the doc/WP views set it on each line wrapper), and the `FlatItem::Block`
`RenderCtx { text_scale }`. `main.rs`: `set_text_scale` → `notify_transcript_views`.

**Why.** Reading the agent conversation at a comfortable size should work exactly
like reading a document — the transcript is the agent tile's primary reading
surface.

**Status.** `implemented` (headless — the painted line height is probed).

**Enforcement.** `verify_harness.rs`: `transcript_prose_scales_with_zoom` (probes
a prose line's PAINTED height at 1× vs 2× — it must grow, so the font actually
scales, not just the cache busting) + `transcript_021_theme_and_zoom_bust_cache`
(zoom re-renders the transcript once — the invalidation path). Exact glyph shape
remains the harness's pixel gap (#1), but the size change is now guarded.
