# bug-0010: transcript-selection-grows-on-streamed-text

**Status:** FIXED
**First seen:** 2026-07-17
**Component:** docs/components/AgentTile (transcript selection, UXI-Selection-1)

## Symptom

In an agent tile, sometimes when new agent text arrives it "automatically gets
selected" — a highlight the user never dragged appears and grows to cover the
new content. The user's guess: clicking on the tile once made it a "leader for
selection." (X11-style copy-on-select: a click / drag leaves an anchor set.)

## Context / root cause

The transcript selection is derived LIVE from `anchor..cursor`
(`editor.rs:930 selection_range`) — the transcript cursor doubles as the
selection head. In a read-only transcript the cursor **auto-advances on its
own** as the agent streams (F2 caret-tracks-streaming) and is force-jumped to
the editable tail at turn end / on reopen (`AgentState::move_cursor_to_tail`,
`agent.rs:4199`). Neither mover touches the `selection_anchor`:

- `Editor::splice_insert` (`editor.rs:1467`) — the programmatic (agent-streamed)
  insert path — shifts the **cursor** when text lands at/before it, but leaves
  `selection_anchor` at its stale absolute `(line, col)`. So an insert above the
  caret slides the caret down while the anchor stays put → `anchor..cursor`
  grows.
- `move_cursor_to_tail` jumps the caret to the tail without clearing the anchor.
  A persisted anchor from an earlier click/select up in the transcript then
  spans anchor..tail — everything from the old click to the bottom lights up.
  This is the dominant trigger of the reported symptom.

A persisted anchor is normal: copy-on-select (UXI-Selection-1) intentionally
leaves the selection highlighted after copy, and even a bare click can leave an
anchor when a streamed chunk moves the caret between mouse-down and mouse-up
(the up-time selection is non-empty → copies + keeps the anchor).

Invariant we want: **programmatic caret movement in the transcript must never
grow a selection over content the user never dragged.** The selection either
stays pinned to its original characters or collapses on a caret jump.

## Planned solution

1. `splice_insert` / `splice_delete`: shift `selection_anchor` by the SAME rule
   already applied to the cursor, so a live selection stays pinned to the same
   characters across agent-streamed edits (can only preserve or legitimately
   include an insert INSIDE the selection — never spuriously grow it).
2. `move_cursor_to_tail`: `clear_selection()` before the jump — a forced caret
   jump is a navigation and collapses any selection (standard editor behavior).

## Approaches already tried (do NOT repeat)

- <none yet — first attempt>

---

## Log

### 2026-07-17 — first fix (anchor-shift in splice + collapse on tail-jump)

**Changed:**
- `src/editor.rs` — new `EditorView::move_selection_anchor(line, col)` (no-op
  when no selection). `splice_insert` / `splice_delete` now capture the anchor's
  char offset before the mutation and remap it by the SAME rule already applied
  to the cursor, so a live selection stays pinned to its characters across
  agent-streamed edits instead of ballooning.
- `src/bin/yalda-gpui/agent.rs` — `AgentState::move_cursor_to_tail` now
  `clear_selection()`s before the caret jump (turn-finalize / reopen is a
  navigation, so it collapses any lingering selection).

**Verified:**
- `editor::tests::append_llm_chunk_shifts_selection_anchor_with_cursor` — a
  selection ("draft") pinned while a chunk streams above it; drives the REAL
  `append_llm_chunk` → `splice_insert`. Negative control: `&& false` on the
  anchor-shift branch → `Some(((1,5),(2,10)))` (anchor stranded on the streamed
  line) → RED.
- `verify_harness::transcript_tail_jump_collapses_stale_selection` — a REAL
  mouse drag leaves a persisted anchor, then the REAL `move_cursor_to_tail`
  collapses it. Negative control: comment out the `clear_selection()` →
  `Some(((0,0),(3,0)))` (ballooned anchor..tail) → RED.
- Full suite green: `cargo test --lib` 155 passed; `cargo test --bin yalda-gpui`
  386 passed (incl. `agent_tile_statemachine_fuzz_holds_invariants`).

**Outcome:** FIXED (on `main`, pending commit + binary rebuild). Runtime spot-
check by the user still worthwhile since the trigger was a live interaction
race, but both real code paths are now guarded.
