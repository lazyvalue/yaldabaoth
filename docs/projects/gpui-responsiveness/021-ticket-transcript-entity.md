# 021 — `TranscriptView` entity (flagship) — rev 2

Closes audit finding #1 (chatbox keystroke re-lays-out the static transcript)
and transitively #7/#8. **Rewritten for rev 2** (`project.md` "Design
history"): the rev-1 snapshot-push + render-fingerprint design is replaced by
the component model — the view observes its session entity and notifies
itself, with `cached()` as the render-skip wrapper. Depends on 024 (helper +
counters) and 025 (`Entity<AgentSession>`). **Needs a human runtime `sample`
profile** before integration.

## Goal

A chatbox keystroke re-renders only the root chrome + compose; the
transcript's `render()` is skipped and its prepaint reused. A transcript
change (stream chunk, worksheet edit, tool expand) re-renders the transcript
**on the same frame its event scheduled** — zero frames late, no stale tail
(the rev-1 hazard). Proven headlessly via render counters; confirmed by
runtime profile.

## The seam (unchanged from rev 1 — still clean)

The transcript list is `gpui::list(list_state, render_fn)` at
~`screens.rs:1489`; `render_fn` (~`screens.rs:1016`) closes over data built at
~`screens.rs:890–1015`. The extraction unit = {that data construction +
`render_fn` + the `list` element} → `TranscriptView::render`. Compose, status
strip, headers stay inline in `render_agent` (tickets 022/023).

## Design — observe + self-notify (no snapshots, no fingerprints)

`TranscriptView` is a view entity, one per session:

- **Owns (UI state):** `list_state`, `list_item_count`,
  `last_reconciled_edit_seq`, `last_scrolled_edit_seq`, follow-tail intent —
  the scroll/list cluster moves here from `AgentState` (widgets own UI state;
  models own domain state).
- **Reads (domain state):** `Entity<AgentSession>` directly in `render()` via
  `.read(cx)` — no snapshot build, no push protocol, no raw-pointer borrows.
  Renders only when notified, so reads are O(visible) exactly when needed.
- **Invalidation:** `cx.observe(&session, …)` registered at construction. The
  callback compares the seqs this render reads — transcript `edit_seq`,
  frozen-ranges gen, tool-structure gen (`calls`/`expanded`), transcript
  cursor/selection — against what was last rendered, and calls `cx.notify()`
  (on itself) only when a slice moved, recording the reason for the
  `YALDA_PERF` notify-reason counter. Observe callbacks run in effect flush —
  timing-correct by construction (`project.md` facts 4–5).
- **Theme / text-zoom:** global, not session state. The zoom/theme *action
  handlers* (event context) notify each live `TranscriptView` directly. (If a
  global Settings entity appears later, observe that instead — same pattern.)
- **Cursor blink (worksheet mode):** the blink timer notifies the
  `TranscriptView` it animates — timers are event context, sound and scoped.
- **Embed:** `render_agent` emits `cached_child(transcript_view)` (024 helper,
  `size_full` baked in) in the transcript slot. No per-frame calls of any kind.

Chatbox typing mutates compose state only → session seqs the observer checks
are stable → no self-notify → entity stays out of `dirty_views` → `cached()`
reuses the subtree. Worksheet typing bumps `edit_seq` → observer fires in the
same event's effect flush → fresh render on that event's frame. ✓ both modes.

## Lifecycle

`HashMap<SessionId, Entity<TranscriptView>>` on `YaldaGpuiView`. Created
lazily on first render of a bound tile (constructor registers the observe
subscription); dropped on `AgentSessions::close`. 1:1 invariant ⇒ one view per
session ⇒ multi-tile splits need no extra logic.

## Subtasks

- [ ] `TranscriptView` entity: UI-state fields moved in from `AgentState`;
      `render()` relocates the row build + `render_fn` + `list` element;
      render counter for tests.
- [ ] Seq plumbing: ensure each observed slice has a monotonic counter on
      `AgentSession` (`edit_seq` exists; add frozen-gen / tools-gen where only
      collections exist today).
- [ ] Observe subscription with slice filter + notify-reason logging; zoom and
      theme action handlers notify live transcript views.
- [ ] `transcript_views: HashMap<SessionId, Entity<TranscriptView>>` + lazy
      create + close hook; `render_agent` embeds via `cached_child`.
- [ ] Headless regression tests: chatbox keystroke ⇒ transcript render count
      flat; worksheet edit / stream chunk (session.update) ⇒ +1 on the next
      frame **including the final append of a burst** (the rev-1 stale-tail
      case); tool expand ⇒ +1; theme/zoom ⇒ +1; follow-tail still reveals
      grown content.
- [ ] Build + full test suite.
- [ ] **Human runtime:** `sample` while typing in a large transcript (no
      per-keystroke transcript layout); stream a long reply and confirm the
      tail lands without input wiggling; adversarial pass: cursor blink,
      selection parity, resize, multi-tile splits, follow-tail, session
      close/rebind.

## Risks

Largest blast radius so far. Watch: the moved scroll/list state (every
`render_agent`/reducer touch-point of `list_state` must move or read through
the view); stale captures — the reused prepaint holds listeners whose closures
captured the *previous* render's data, so interactive rows (tool expand, links)
must act through ids/indices resolved at event time, never cached row data;
seq coverage — any render input without a counter is a stale-UI bug (the
adversarial review should hunt for exactly these, with the notify-reason log
as the audit trail).

## Links

`project.md` (facts, component model), tickets 024/025, `audit-report.md` §2
finding #1, `spec-agent-session-ownership.md`, ADR-0020 (INV-PR/INV-RV).
