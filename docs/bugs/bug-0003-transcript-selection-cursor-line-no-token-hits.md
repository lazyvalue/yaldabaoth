# bug-0003: transcript-selection-cursor-line-no-token-hits

**Status:** FIXED
**First seen:** 2026-07-15
**Component:** docs/components (Selection / transcript mouse select-to-clipboard, INV-UX-14 / UXI-Selection-1)

## Symptom

Mouse selection in the agent transcript "behaves strangely" and copy "doesn't
always copy what is selected." Intermittent: sometimes a drag-select copies the
wrong text, or a click/drag that starts on one line anchors somewhere else.

## Context / root cause

Transcript mouse hit-testing maps a window point → `(line, char)` through a
paint-time **token sink** (`token_hits`): every painted text token registers its
bounds + covered char range via `register_token_on_paint`, and
`hit_test_tokens` picks the nearest token. `transcript_mouse_down` reads the sink
that was painted on the PREVIOUS frame to place the drag anchor.

The non-cursor render path (`build_wrapped_line`, `!is_cursor_line`) registers
every token. But the **cursor line** — rendered when `focus == Transcript` and
not mid-drag (`cursor_line = cursor.line`) — takes the caret-injection path
(agent.rs ~1179-1244) which splits tokens around the caret and **never calls
`register_token_on_paint`**. So the caret's line contributes ZERO token hits.

Consequence: once the transcript is focused (which it becomes after the first
select, and stays), the one line holding the caret has no hits. A `mouse_down`
that lands on that line hit-tests to the nearest OTHER line → the anchor is
dropped on the wrong line → the selection (and the copied text) is wrong.
Intermittent because it depends on where the caret currently sits and where the
drag starts. During the drag itself the caret is suppressed (`dragging`) so all
lines register — only the stale mouse-down anchor frame is corrupt, which is
enough to corrupt the whole selection.

Empty (blank) lines are likewise never registered (the empty-line placeholder
branch skips the sink), so a click on a blank line clears the selection and a
drag can't anchor there.

## Planned solution

Make the token sink **caret-independent**: register token hits on the cursor
line too. Refactor `build_wrapped_line` so every emitted piece (before-caret,
the caret cell, after-caret, whole non-owner tokens, the EOL caret, and the
empty-line placeholder) registers into the sink with its correct
`start_char`/`char_count`, via a shared `reg` helper. Hit-testing then has full
line coverage regardless of focus/caret state.

## Approaches already tried (do NOT repeat)

- <none yet>

---

## Log

### 2026-07-15 — cursor line now registers hit-test tokens

**Root cause confirmed.** `build_wrapped_line` (agent.rs) registered token hits
only on the non-cursor path. When `focus == Transcript` and not mid-drag, the
caret's line renders via the caret-injection path, which emitted its
before/caret/after pieces with NO `register_token_on_paint`. So the focused
caret line contributed zero entries to `token_hits`, and `transcript_mouse_down`
(which reads the previous frame's sink to place the anchor) snapped a click on
that line to the nearest OTHER line → wrong anchor → wrong copied text.
Intermittent because it depends on where the caret sits and where the drag
starts. (During the drag the caret is suppressed via `dragging`, so only the
stale mouse-down anchor frame is corrupt — but that alone poisons the whole
selection.)

**Fix (agent.rs `build_wrapped_line`).** Added a shared `reg(el, start_char,
char_count)` closure that wraps an element in `register_token_on_paint` when a
`token_sink` is present, and routed EVERY emitted piece through it:
- non-cursor tokens (was already registered — now via `reg`),
- cursor-line before-caret / after-caret slices, at correct char offsets,
- the caret cell itself (count 1 in Normal, 0 in Insert),
- the trailing EOL caret (zero-width at line end),
- the empty-line placeholder (zero-width at col 0, so blank lines are anchorable).
Hit-testing is now caret-independent: full per-line coverage regardless of
focus/caret state.

**Verified.** New headless guard
`transcript_drag_on_focused_caret_line_copies_that_line` (verify_harness.rs):
focuses the transcript, parks the caret on line 1, drags across line 1, asserts
the clipboard holds line 1's text and does NOT leak line 0's. Drives the REAL
mouse handlers (`simulate_mouse_down/move/up`) + real paint sink + clipboard
round-trip.
- **Negative control (observed RED):** temporarily added `if is_cursor_line {
  return el; }` to `reg` (reproducing the pre-fix state) → test failed with
  "focused caret line (line 1) registered no token hits", the exact bug.
  Restored the fix → green.
- Full suite: `cargo test --bin yalda-gpui` → 371 passed, 0 failed.

Not committed (awaiting user). Not yet human-verified with a real macOS drag
(runtime gap #1: pixels + real OS mouse delivery), but the geometry + copy path
are covered headlessly. Doc-view mouse selection was inspected and is NOT
affected — it registers per-line `TextLayout`s independent of the caret.
</content>
</invoke>
