# bug-0015: code-block-shifts-under-the-pointer-on-click

**Status:** RECURRED → FIXED + SHIPPED (2026-07-22; the fix was stranded uncommitted — see the latest log entry)
**First seen:** 2026-07-21 ("Still can't select in a multiline code block")
**Component:** `docs/components/common/selection.md` (`UXI-Selection-1`) + agent-tile transcript

Related: [bug-0008](bug-0008-cannot-select-parsed-blocks-in-transcript.md) (FIXED —
parsed blocks registered zero hit-test tokens). That fix landed and is NOT the
problem here: code blocks *do* register bands now.

## Symptom (as reported)

"Still can't select in a multiline code block."

## What was ruled OUT (measured, this session)

A headless probe (frozen ```` ```rust ```` block, 2 code lines, in the real transcript
view) established:

1. **Hit bands exist.** The block registers one band per raw line —
   `[(0,0,7),(1,0,10),(2,0,10),(3,0,3)]` — so bug-0008's fix is live. Not a
   zero-tokens problem.
2. **The model path works.** Driving the REAL handlers
   (`transcript_mouse_down/move/up`) with correct painted coordinates selects across
   lines and copies: `sel=((1,0),(2,10))`, clipboard `"let a = 1;\nlet b = 2;"`.
   So selection, `selection_text`, and the auto-copy are all fine for code blocks.

## What was found (the real defect, reproducible)

**Clicking inside a code block moves the block ~25px under the pointer.** Measured
band tops for the block's 4 raw lines:

| state | band tops |
|-------|-----------|
| base (cursor outside the block) | 950, 970, 990, 1010, 1032 |
| cursor set INSIDE the block | **975, 995, 1015, 1035** (last band gone) |
| cursor moved back outside | 950, 970, 990, 1010, 1032 |

Isolated by mutating one thing at a time: `focus = Transcript` alone → no shift;
`dragging = true` alone → no shift; **setting the editor cursor onto a line inside the
block → +25px**, reversible. `pending_reveal_cursor` is `false`, so this is not the
reveal path.

**Mechanism.** The flat-item build is **cursor-dependent**: the blank-line collapse
keeps the blank line the cursor sits on. With the cursor at the tail blank line the
items are `["Block", "Line"]`; move the cursor into the block and the trailing blank
collapses away — `["Block"]`. One fewer list item ⇒ the list reflows ⇒ everything
repaints 25px lower.

`transcript_mouse_down` sets the editor cursor, so the press itself triggers the
reflow: the anchor is captured against the OLD layout, and every subsequent
`hit_test_tokens` runs against the NEW one. 25px is larger than the 20px line height,
so the drag maps a full line off — you press on `let b = 2;` and end up selecting from
`let a = 1;`. On a short block the pointer can leave the block entirely.

## Planned solution (option 1 was implemented — see the Log)

Candidates, in order of preference:

1. **Make the item list stable across a click** — the blank-line collapse should not
   change item COUNT based on the transcript cursor (or should be frozen for the
   duration of a drag). Smallest change, kills the reflow at the source.
2. **Decouple selection from the editor cursor** — keep the drag anchor/head in
   `TranscriptView` instead of mutating `editor.cursor`, so a press has no
   render-visible side effect. Cleaner, wider.

Guard seam: the probe above, promoted — assert the block's painted band tops are
IDENTICAL before and after `transcript_mouse_down` lands inside it (negative control:
restore the cursor-dependent collapse ⇒ the +25px shift returns).

## Surface (answered)

The user confirmed: **the agent transcript**, so the `line_layouts`/doc-view branch
below was NOT the reported bug and remains unprobed.

## Open question (why the fix was held until the surface was confirmed)

It is not established that the shift is what the user is reporting. Two surfaces have
completely different selection mechanisms and only one was probed here:

- the **agent transcript** (paint-time token sink, `hit_test_tokens`) — probed above;
- the **doc view** (`line_layouts` / `doc_selection` in `RenderCtx`) — NOT probed.
  Note `block_element` passes `line_layouts: None` for NESTED blocks (blockquote
  children, list items), so a code block inside a list item would register no layouts
  at all in the doc view — a plausible *separate* "can't select" for that surface.

Per the anti-circling rules the fix was held until the surface was confirmed rather
than shipped as a guess. The transcript was confirmed; the doc-view branch was never
implicated and stays open as a separate candidate if it is ever reported.

## Approaches already tried (do NOT repeat)

- **Registering per-raw-line hit bands** (bug-0008, 2026-07-16) — done, still working,
  necessary but not sufficient. Re-doing it will not help.

---

## Log

### 2026-07-21 — localized, not fixed (first pass)

Probed the real transcript view headlessly (probe removed after measuring; results
recorded above). Established that bands and the selection/copy model path are both
healthy for code blocks, and that the press itself reflows the transcript by 25px via
the cursor-dependent blank-line collapse. No code changed. Awaiting the user's answer
on surface (transcript vs doc view) and exact symptom before implementing, so the fix
targets what they actually see.

### 2026-07-21 — froze the collapse's protected line for the duration of a drag (FIXED)

**What changed.**
- `agent.rs`: new `AgentState.drag_protect_line: Option<usize>`. The blank-collapse
  pass now protects `drag_protect_line.unwrap_or(cursor.line)`, and the field is
  folded into the view-model memo key (it is a genuine build input).
- `transcript_view.rs`: `transcript_mouse_down` captures the PRE-press cursor line
  into `drag_protect_line` before moving the cursor; `transcript_mouse_up` and the
  no-hit early-out clear it. So the flat-item COUNT cannot change for the duration of
  the gesture, and the transcript cannot reflow under the pointer.

Chosen over the alternative (decoupling selection from the editor cursor entirely):
same observable fix, far smaller blast radius. The decoupling remains the cleaner
long-term shape if this class recurs.

**How verified.** `verify_harness.rs::code_block_does_not_shift_when_clicked` asserts
on PAINTED geometry — the block's four band tops are identical before and after the
REAL `transcript_mouse_down` lands inside it — then drags to the second code line and
asserts the clipboard holds BOTH lines, and that the protection clears on mouse-up.
Non-vacuity: it first asserts the block actually painted ≥4 per-line bands.

**Negative control observed RED:** reverting `protect_line` to the bare cursor line
fails with `left: [(0,950),(1,970),(2,990),(3,1010)]` vs
`right: [(0,975),(1,995),(2,1015),(3,1035)]` — the exact +25px shift. Restored, green.
Suites: 401 bin + 157 lib.

**Unverified.** Not yet seen in the live app (rebuild + restart needed). If selection
in a code block STILL feels broken after this, the remaining suspects are the caret
suppression path and the doc-view `line_layouts: None` branch for nested blocks —
neither is implicated by any measurement yet, so do not pre-emptively "fix" them.

**Note.** During this work a parallel session's in-flight `AgentRow.order_sid` change
left the shared tree uncompilable; one stale test initializer in `verify_harness.rs`
got `order_sid: None` from me so the suite could run.

### 2026-07-22 — RECURRED report: the fix was never SHIPPED (committed + rebuilt)

**Symptom (user).** "I still can't select in multiline code blocks. This is
bug-0015. It's higher priority and EXTREMELY frustrating. Fix it first. Ensure it
never, ever happens again. I am sick and tired of repeating this."

**Diagnosis — this was a SHIPPING failure, not a fix-quality failure.** The
2026-07-21 `drag_protect_line` fix was correct and verified (the guard passes on
the REAL handler path; negative control fires the exact +25px shift). But
`git log -S drag_protect_line` shows it was **never committed** — `agent.rs` +
`transcript_view.rs` sat dirty in the working tree the whole time. The user runs
`main` (release, via `./dev-gui.sh`), so they never once had the fix. This is
anti-circling rule 5 verbatim: "Fixes stranded on feature branches never reach the
user… 'Tests pass on the branch' is not shipped." Worse: the guard
`code_block_does_not_shift_when_clicked` HAD been committed (folded into an
unrelated whole-file `verify_harness.rs` stage), so HEAD referenced
`AgentState.drag_protect_line` without the field — HEAD did not even compile.

**What was done this time (the missing step).**
1. Re-verified the fix on the real path: `code_block_does_not_shift_when_clicked`
   green; negative control (revert `protect_line` to the bare `cursor().line`)
   observed RED with `left:[(0,950)…] vs right:[(0,975)…]` — the +25px shift.
2. **Committed** `agent.rs` (`drag_protect_line` field + collapse `protect_line`)
   and `transcript_view.rs` (capture at mouse-down / clear at mouse-up) to `main`,
   making HEAD compile + the guard non-vacuous.
3. Promoted the behavior to a durable invariant: **`UXI-Selection-2`** — a
   drag-selection never moves the content under the pointer (transcript flat-item
   count is frozen for the gesture). So a future edit that reintroduces the reflow
   trips a named invariant, not just a lone test.
4. Rebuilt the release binary so the fix is in the RUNNING app (rule 5's second
   half), and told the user to restart.

**Durable follow-up (not done, recorded).** The band-aid freezes the item count
only during a drag; a bare click that parks the caret in frozen content still
statically reflows the block once. The class dies for good only by decoupling the
transcript selection anchor/head from the document caret (option 2 above) so a
press has zero layout side effect. Left as the preferred shape if this recurs; the
reported *selection* symptom is fixed and shipped.
