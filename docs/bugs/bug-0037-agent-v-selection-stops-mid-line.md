# bug-0037 — Agent Tile V selection stops in the middle of the next line

**Status:** FIXED
**First seen:** 2026-08-13
**Component:** AgentTile transcript selection (`UXI-AgentTile-34`)

## Symptom

In an idle Agent Tile worksheet, press `V` over an agent line and then `j`. The
first line is selected whole, but the selection on the next line stops at a
character column—often near the middle—instead of including the complete line.
`V` therefore does not behave like true linewise visual mode.

## Context / root cause

The original `select-line` action called `extend_by_line` once and turned the
editor's generic `extend_mode` on. That made the initial range line-aligned, but
the following `j`/`k` still ran ordinary characterwise cursor motion. Vertical
motion retained the end column of the first line through `CursorPos::desired_col`,
so a longer destination line was selected only through that inherited column.

The prior real-path guard checked only that the selection's ending **line index**
changed. It did not assert the ending column or exact selected text, so the broken
partial-line selection passed.

## Planned solution

Represent linewise selection explicitly in `EditorView`. `V` enters that state;
after every shared normal-mode motion, normalize the fixed anchor line and active
cursor end to whole logical-line boundaries. Keep lowercase `v` characterwise and
retain the existing Esc/reply exit paths. Strengthen the Agent Tile harness with
unequal line lengths and exact selected-text assertions in both directions.

## Approaches already tried (do NOT repeat)

- Generic `extend_mode` plus a one-time `extend_by_line` is insufficient: it
  preserves the selection anchor but not linewise endpoint semantics after motion.

---

## Log

### 2026-08-13 — FIXED (distinct linewise state + endpoint normalization)

- **Change.** `EditorView` now tracks `linewise_extend_mode`; `select_linewise`
  enters/grows it and `normalize_linewise_selection` keeps forward and backward
  motion on full-line boundaries. Shared normal motions call the normalizer, and
  Agent Tile tool-anchor hops re-normalize their actual visible destination.
  Lowercase `v` explicitly remains characterwise.
- **Verified.** `worksheet_v_then_j_extends_selection` now uses a 3-character
  first line and a much longer second line and asserts their exact full text;
  `worksheet_v_then_k_selects_whole_previous_line` covers the reverse direction.
  Both drive the real `handle_claude_key` worksheet path and pass. The strengthened
  down-motion guard was observed RED before the fix with actual `"one\ntwo"` versus
  the expected full second line.
