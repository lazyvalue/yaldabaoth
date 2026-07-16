# bug-0004: two-adjacent-you-blocks

**Status:** FIXED
**First seen:** 2026-07-16
**Component:** docs/components/agent-tile/compose.md (`UXI-AgentTile-11` rule 6)

## Symptom

In a worksheet agent tile, under certain conditions two You-blocks render **adjacent
— right next to each other** — instead of one. That must never happen. Repro: open a
tail You-block (the div at the bottom), type text, Esc-Esc to nav, move the caret UP
one line onto the last agent line, press `o` (or any insert-entry key). A second,
empty You-block appears immediately beside the first.

## Context / root cause

`agent_ui.rs:4728` routes `i/a/o/I/A/O` (worksheet transcript-nav) to
`AgentState::open_you_block_at_cursor` (`agent.rs`), which implements `UXI-AgentTile-11`
rule 6 "multiple insertion points": a block open + caret at a DIFFERENT anchor parks
the current block and opens a fresh one.

The decision keyed on **raw anchor equality** (`snapped == you_block_anchor`), not on
where the blocks actually RENDER. Two blocks land in the SAME render slot when the
lines between their anchors collapse. Concretely (repro): the tail block is anchored
at the empty tail line; pressing `o` one line up parks it and opens a new block at the
agent line just above. The empty tail line between them then collapses (nothing
anchors it), so both blocks resolve to the same insertion position → two adjacent
`YouBlock` items. Reproduced headlessly: after the gesture, `flat_items` =
`[…, L3, YOU(Some(0)), YOU(None)]` — two consecutive YouBlocks (exploratory probe,
since removed).

Multiple insertion points at GENUINELY separated anchors (agent content surviving
between them, e.g. anchors 0 and 2 of `alpha/beta/gamma/delta`) are NOT adjacent and
remain valid — that's the intended feature, not the bug.

## Planned solution

Resolve insertion points by **render slot**, not raw anchor. Add
`you_blocks_would_be_adjacent(a, b)`: true when every doc line strictly between the
two anchors is blank (so they collapse into one slot; same effective line is trivially
adjacent). In `open_you_block_at_cursor`, match the caret's snapped anchor against the
active block and each parked block by adjacency: adjacent ⇒ resume that block; only a
genuinely separated legal anchor spawns a new block. Keeps the multi-insertion feature
for separated points; forbids adjacency.

## Approaches already tried (do NOT repeat)

- **"One open block at a time" (full retirement of multi-insertion)** — over-reached.
  Reverted at the user's direction: the invariant is "no two *adjacent* you-blocks,"
  NOT "only ever one block." Separated insertion points must still work.

---

## Log

### 2026-07-16 — resolve insertion points by render slot; forbid adjacency (FIXED)

**Status:** FIXED.

**What changed.** Added `AgentState::you_block_effective_line` +
`you_blocks_would_be_adjacent` (`agent.rs`) and rewrote `open_you_block_at_cursor` to
match the caret's snapped anchor against the active/parked blocks by ADJACENCY
(render-slot collision) rather than raw anchor equality: adjacent ⇒ resume that block;
a separated legal anchor still parks the active and opens a fresh block. Spec rule 6
(compose.md) amended to state "multiple insertion points, but never two adjacent."

**How verified.** Localized with an exploratory probe on the real key path
(`handle_claude_key(o)`): old code produced `flat_items` with two consecutive
`YouBlock`s (`ADJACENT=true`, `parked=1`); probe removed. New guard
`worksheet_you_blocks_never_render_adjacent` drives the exact gesture and asserts the
RENDERED `flat_items` contain NO two consecutive YouBlock items, exactly one You-block,
and that the existing "hi" reply is resumed (not orphaned). **Negative control
(observed RED):** revert the active check to `snapped == self.you_block_anchor` →
two adjacent YouBlocks → the adjacency assert fires RED; restored → green. The three
existing multi-insertion tests (`worksheet_multiple_insertion_points`,
`worksheet_cursor_on_existing_block_resumes_it`, `worksheet_reentering_insert_keeps_block_anchor`)
still pass unchanged — genuine separated insertion points are preserved. Full suite:
375 passed incl. the state-machine fuzz oracle.

**Outcome.** `o`/`i`/… near an existing You-block resumes it instead of spawning an
adjacent duplicate; separated multi-insertion still works. Runtime-unverified
(headless-green); fix is on `main`'s working tree — rebuild the binary to confirm.
