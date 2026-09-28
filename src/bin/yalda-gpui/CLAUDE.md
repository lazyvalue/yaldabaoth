# yalda-gpui — UX architecture (read before adding/altering any view)

> **Behavioral contract: `docs/ux-patterns/` (universal `UXP-N` laws) + the owning
> component's `UXI-<Component>-N` list in `docs/components/` are authoritative and
> mandatory.** A change MUST NOT violate one; if it seems to need to, reconcile the
> spec first. New UX ⇒ `/new-ux`. The rules below are the *performance* contract;
> the specs are the *behavior* contract.

This module is the GPUI surface. GPUI re-renders the **root every frame**
(`window.rs draw_roots`), and its *only* render-skip lever is
`AnyView::cached`. So the cost of an indelicately-built view is **O(whole tree)
per keystroke** — that is the performance trap this module is organized to make
hard to fall into. Background + the six verified GPUI 0.2.2 facts:
`docs/reference/gpui-render-model.md`.

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

Use **yux** (yalda-ux) — yalda's reusable UX layer over GPUI, in the `yux/`
module (`yux/cached.rs` = `cached_child` + the `record_*` accounting +
`MissReason`; `yux/detail.rs` = `DetailStyle` + reusable view primitives). Read
`yux/CLAUDE.md`: it states the component rules (state encapsulation, the
never-notify-in-render law, the render-count test) and the contribution mandate
— **all reusable UX lives in or is built from `yux/`.** Reference components:
`transcript_view.rs` and `linear_view.rs`. Hand-rolling a `Render` impl with a
large inline element tree is the mistake the whole module is shaped to prevent;
if you think you need to, that is a signal to compose from (or extend) yux.

## Instrumentation

`record_render(label)` / `record_notify(label, MissReason)` (`yux/cached.rs`),
read in tests via `perf_render_count` / `perf_last_notify`. Run the live app
with `YALDA_PERF=1` to watch counts. Render *count* is a proxy, not frame time —
GPUI can't be driven headlessly for paint, so a real perf read is still a human
`sample` under `--release` (debug masks all wins).

## Module layout

`src/bin/yalda-gpui/` is a module-per-concern split (modules glob-import the
root via `use super::*;` and the root re-exports them with `pub(crate) use`,
so items stay crate-visible regardless of file):

- `main.rs` (~6.5k) — `YaldaGpuiView` struct, the `Render` impl, app/tab/
  split/doc methods, marks/layout-modes/tags, menus + overlays + pickers,
  key bindings + `main()`. A Tile (`Window<App>`) holds one `App`
  (`spec-tiles-and-apps.md`, ADR-0019): `App::Buffer(BufferApp)` —
  `BufferApp::{Picking(file browser), Viewing(rendered doc), Editing(raw)}`
  — or `App::Agent(AgentTile)` (a viewport bound to one session in the
  `AgentSessions` store; see `spec-agent-session-ownership.md`). The render path
  branches on that, each screen with its own `key_context` (`YaldaView`,
  `EditView`, `BrowserView`, `AgentView`) and its own `on_action` wiring.
- `screens.rs` — the screen render bodies: `render_doc`, `render_edit`
  (Code + WP), `render_agent`, `render_browser`.
- `agent.rs` — agent-tile data layer: tool-call model, `FlatItem` view model
  + S1 cache + `rebuild_agent_view_model`, `TurnPhase`, `AgentState`,
  `AgentSession`, `AgentTile`.
- `agent_sessions.rs` — the `SessionStore`/`AgentSessions` owner: the private
  `SessionId → AgentSession` registry that enforces the 1:1 binding invariant
  (`open_or_focus`, `bind_sid`, `locate`, `close`).
- `you_block_view.rs` — `YouBlockView`: the worksheet's ACTIVE inline
  You-block as its own cached view, painted by `render_agent` over the
  transcript's placeholder item (`slot_overlay`) so typing never re-renders
  the transcript (D11); plus the shared `you_block_element` builder.
- `agent_ui.rs` — agent/session methods on the view: open/attach/create/
  close flows, server pump + reducers (`apply_server_batch`
  / `apply_reply_events` / `apply_agent_event`), submit paths, Claude key
  handler.
- `chrome.rs` — focused-window/layout render, tab strip, tag bar, rails.
- `edit_ui.rs` / `browser_ui.rs` — per-screen methods (edit entry/exit + key
  dispatch; browser nav + rail).
- `render_blocks.rs` — free render helpers for the markdown doc/transcript
  path: colors/fonts, styled-line/block/table elements, wiki links.
- `linear.rs` / `linear_ui.rs` / `linear_view.rs` — `App::Linear`: the Linear
  GraphQL client + data model, the view-layer methods, and the cached body
  component (built on **yux**).
- `diff.rs` / `diff_ui.rs` / `diff_view.rs` + `diff_model.rs` / `diff_git.rs` /
  `review_state.rs` — `App::Diff`: the read-only, worktree-bound review tile
  (`docs/specs/spec-diff-review.md` rev 2, `docs/components/diff.md`, ADR-0040).
  `diff.rs` = tile data model + pure nav helpers; `diff_ui.rs` = view methods
  (worktree bind/refresh/apply, Viewed, comments, send picker, open);
  `diff_view.rs` = the yux cached body (`DiffView`, root-observed).
  `diff_model.rs` = the pure unified-diff parser + `file_hash`; `diff_git.rs` =
  the async `git` subprocess boundary (diff, worktree list); `review_state.rs` =
  the per-branch review JSON (viewed files + comments) at
  `<primary-checkout-root>/.yaldabaoth/reviews/<branch>.json`.
- `yux/` — the reusable UX component layer (cached-view infra + view
  primitives). See **"yux" below** and `yux/CLAUDE.md`.
- `persist.rs` — paths, preferences, workspace + ACP-session persistence,
  server launch helpers.
- `workspace.rs` — tab strip + n-ary split tree (`Workspace<C>`,
  `FocusedWindow`, etc.). See `docs/specs/spec-tabs-and-splits.md`.
- `tests.rs` / `verify_harness.rs` — unit tests + headless render harness.

Keep the split honest: new agent-tile logic goes in `agent.rs`/`agent_ui.rs`,
markdown-block render helpers in `render_blocks.rs`, **all reusable UX in
`yux/`** — don't let `main.rs` re-accrete.

## Key conventions

Per-screen vim-style bindings live with `Some("YaldaView")` etc. contexts.
Global Cmd shortcuts (Quit, OpenBrowser, OpenClaude, tab/split management,
zoom) are registered with `None` context and **must** have a matching
`on_action(Self::handler)` on every screen's root so the dispatch lands.

## Document text zoom (extending it)

`Cmd-=` / `Cmd-+` zoom in, `Cmd--` zooms out, `Cmd-0` resets. Implementation
is a `text_scale: f32` on `YaldaGpuiView` (clamped `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`,
step `TEXT_SCALE_STEP = 1.1`) that multiplies the body `text_size(px(14.0))`
and every heading size. Threaded into `RenderCtx::text_scale` for block
rendering — for the buffer doc/edit views AND the **agent transcript**
(conversation prose + markdown blocks scale; UXI-TextZoom-1). **Chrome stays fixed** —
status bars, tab strip, browser rows, the agent gutter/labels + bottom panels,
and the pixel-pinned compose input all render at their native sizes. To extend
the zoom to a new surface, multiply that surface's base `text_size` by
`self.text_scale` and add `on_action(Self::zoom_in/out/reset)` to its root; for a
cached surface (the transcript) read `text_scale` off the root and invalidate via
`notify_transcript_views`.

## Tests never touch `~/.yalda`

`acp_session_persist_path` / `preferences_path` / `workspace_persist_path` return
`None` (or a tempdir override) under `cfg(test)`. A test that triggers `set_theme`,
`set_text_scale`, or `save_workspace_state` must NOT write the user's real state —
a new persisted path gets the same `*_PATH_OVERRIDE` seam (bug-0016).
