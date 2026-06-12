# 021 — Transcript → cached child entity (flagship)

Closes audit finding #1 (chatbox keystroke re-lays-out the static transcript).
The first real consumer of the `CachedPanel` keystone (ticket 020). Build on the
`transcript-entity` branch. **Needs a human runtime `sample` profile** before
integration — the GUI can't be driven headlessly for paint.

## Goal

A chatbox keystroke re-renders only the root chrome + compose box; the
transcript subtree's layout/paint is **reused** (its `render()` not invoked).
Proven headlessly; confirmed smooth by runtime profile.

## The seam (already clean — not a blind 1600-line move)

The transcript list is `gpui::list(c.list_state.clone(), render_fn).flex_1().w_full()`
at ~`screens.rs:1489`, where `render_fn` (~`screens.rs:1016`) is already a closure
over **snapshots** built at ~`screens.rs:890–1015` (`lines_rc`/`lines_snap`, `hl_snap`,
`frozen_lines_snap`, `tool_calls_snap`, `expanded_snap`, cursor, selection, theme).
The extraction unit = {snapshot construction + `render_fn` + the `list` element}.
**Compose box, status strip, headers stay inline in `render_agent`** (they become
their own cached children in 022/023).

## Design — snapshot-push (AgentState stays source of truth)

`TranscriptView` entity implements `FingerprintedPanel`; holds the snapshot it
renders from (the `_snap` bundle + `list_state`). `AgentState` is NOT moved into
an entity (keeps the `AgentSessions` 1:1 store untouched).

Flow in `render_agent`, per bound session:
1. Compute the **render fingerprint** cheaply (no snapshot build).
2. If it changed since last push: rebuild the snapshot, push it into the
   `TranscriptView` via `entity.update(..)`, and `CachedPanel::notify_if_changed`
   (dirties the child). If unchanged (chatbox typing): do neither → child stays
   out of `dirty_views` → `cached()` reuses its laid-out subtree.
3. Embed `panel.element(size_full_style())` (or `flex_1`) in the transcript slot.
   `notify_if_changed` must run BEFORE the element is laid out (helper contract).

### Render fingerprint (correctness crux — must cover everything rows depend on)

`transcript edit_seq` + frozen-ranges gen + tool-structure fp (`calls_snapshot` +
`expanded_snapshot`) + **transcript** cursor line/col + selection range + theme id
+ `text_scale`. NOT viewport width — `cached()`'s bounds cache-key already
invalidates on resize, and the snapshot segments don't depend on width (wrap is
layout-time).

- Chatbox mode while typing: caret is in the *chatbox* editor, transcript
  `edit_seq`/cursor/selection all stable → fp stable → cache hit. ✓
- Worksheet mode: typing bumps transcript `edit_seq` → fp moves → re-render. ✓ (correct)

### Lifecycle

`HashMap<SessionId, CachedPanel<TranscriptView>>` on `YaldaGpuiView`. Created
lazily on first render of a bound tile; dropped on session close (hook
`AgentSessions::close`). 1:1 invariant ⇒ one panel per session ⇒ splits with
multiple agent tiles work without extra logic.

## Subtasks

- [ ] `TranscriptView` entity + `FingerprintedPanel` impl (render_fp as above)
- [ ] Relocate snapshot build + `render_fn` + `list` into `TranscriptView::render`
- [ ] `transcript_panels: HashMap<SessionId, CachedPanel<TranscriptView>>` on root + lazy create + close hook
- [ ] `render_agent` gates snapshot rebuild/push on render_fp; embeds `panel.element(..)`; compose/strip stay inline
- [ ] Headless regression test: chatbox keystroke ⇒ `TranscriptView` render-count flat; transcript content change ⇒ bumps. (counter in `TranscriptView::render`, idiom from `VIEW_MODEL_REBUILDS` / `edit_view_keystroke_is_o_changed`)
- [ ] Build + full test suite
- [ ] **Human runtime:** `sample` the live process while typing in a large
      transcript; confirm no per-keystroke transcript layout. Adversarial review
      for regressions (cursor blink, streaming, resize, multi-tile, follow-tail).

## Risks

Largest blast radius so far. Watch: follow-tail/scroll on new content (must still
notify when content appends), selection/cursor rendering parity, theme/zoom
changes invalidating correctly, the snapshot raw-pointer borrow idiom currently in
`render_agent` (the entity owns the snapshot instead — should remove the unsafe).

## Links

`docs/projects/gpui-responsiveness/project.md`, `audit-report.md` §3, ticket 020
(`cached_panel.rs`), `spec-agent-session-ownership.md`.
