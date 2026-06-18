# Worksheet frozen-block model

## Problem / why

Agent Worksheet mode lets the user edit in a vim-like buffer where Claude's
content is "frozen" (read-only) and the user types in editable lines. Two
reported failures:

1. **Enter+exit insert creates a phantom empty "You" region and butchers
   existing Claude text.**
2. **No way to insert *between* lines of frozen interior text** — the user wants
   a "frozen block" model.

## The model (user's definition, 2026-06-16)

- A **frozen block** is a *single line of frozen text terminated by a newline*.
  Each such line is its own block.
- The user must be able to **insert between any two frozen blocks** (i.e.,
  between any two adjacent frozen prose lines, or above/below them).
- **Tool groups, fenced code blocks, and tables are ATOMIC** — each is treated
  as a *single* frozen block: crossed in one navigation keystroke, and never
  split by an insert. ("The rest of the logic is correct" — the existing
  one-stroke crossing of these is kept.)

## Root cause

`EditorView::frozen_lines: Vec<(start,end)>` models frozen content as flat,
merged, half-open *line* ranges with **no block identity**. The "atomic
structural block" concept exists only at render time (`detect_block_ranges` in
`agent.rs`), so every insert/delete boundary predicate reasons about *lines*,
never *blocks*. Consequences:

- `can_insert_char_at` permits a `\n` at col 0 / end-of-line of **any** frozen
  line, including the interior of a fenced code block or table → splitting a
  structural block ("butchers Claude text"). For frozen *prose* lines this same
  rule is exactly the desired "insert between blocks" gesture; the two cases are
  indistinguishable without block identity.
- The render header pass and `build_nav_stops` make prose-run / whole-doc
  assumptions that contradict the per-line model.

## Subagent analysis (4 personalities, read-only)

Invariant Theorist, Chaos Monkey, Render-Pipeline Skeptic, Streaming Realist
each swept the code. Convergent ranked findings → fixes:

| Fix | Bug | Severity | Status |
|-----|-----|----------|--------|
| 1 | Insert splits atomic code/table block (CRIT-1/2, C1) | critical | this branch |
| 2 | Submit freezes blank lines → empty "You" turn (HIGH-1/2) | high | this branch |
| 3 | Phantom "You" header from whole-rest-of-doc scan (Finding C) | medium | this branch |
| 4 | `build_nav_stops` coalesces prose run → can't insert between lines (Finding D) | medium | this branch |
| 5 | `snap_nav_stop` strands caret on unrenderable block interior (Finding E) | medium | this branch |

### Deferred (real, higher-scope, NOT the reported repro) → ticket 001

- **Streaming cursor drift** (Streaming Realist F2): `programmatic_insert` shifts
  frozen ranges/anchors but NOT the `EditorView` cursor, so a chunk streamed
  above the caret detaches it onto agent content.
- **Floor only covers EOF tail** (F1): `agent_tail_floor_char` can't protect a
  user draft wedged *between* two frozen blocks.
- **Undo wipes metadata** (C3): `reset_line_anchors` drops all `TurnId`/tool tags
  on undo/redo → continuation streams to EOF, gutter blanks.
- **Fingerprint excludes cursor/content** (Skeptic A/B): `view_model_fingerprint`
  omits cursor line + `edit_seq`, so the cursor-sensitive blank-collapse /
  nav-stops passes can be reused stale on a cursor-only move.

## Definition of done

builds + tests + headless regression tests for each fix + runtime check flagged
(GPUI can't run headless). Live on branch `worksheet-frozen-blocks`.
