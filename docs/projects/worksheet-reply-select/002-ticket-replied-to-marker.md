# 002 — The `>` replied-to source marker (when not editing) — DONE

Implements **UXI-AgentTile-37** (DONE). The source agent line(s) a pending worksheet
reply quotes render with a beautiful blockquote marker (`>` / left bar + italic)
in the transcript, so it is obvious what a pending reply refers to. Shown while
NOT typing in the reply block; pending-scoped (clears on submit / abandon).

## Goal

A reply that quotes agent text (via `r` / selection, UXI-AgentTile-21/-35) marks
its source in the transcript with a `>` blockquote, visible whenever the reply's
compose is not the active typing surface.

## Subtasks

- [x] Capture the quoted source range in `AgentState`
      (`reply_source_range: Option<(usize, usize)>`) inside `reply_quote_at_cursor`
      (both the selection and sentence branches).
- [x] Clear it on submit (`close_you_block`, called from the submit path), the
      empty-Esc discard, and the `u`-pop back-out (UXI-AgentTile-24).
- [x] Thread it into the transcript render: `TranscriptSeqs::reply_marker` +
      `TranscriptPrep::reply_marker_snap`. (The `transcript_021_*` count test was
      not added separately — see the deviation in UXI-AgentTile-37.)
- [x] Frozen-line render branch: when the marker is active AND not typing
      (`reply_marker_range()` gate), draw a `>` gutter glyph + blockquote-colored
      left bar. (Gutter `>`, not an inline prefix; italic deferred — see deviation.)
- [x] "Not typing" gate = `reply_marker_range()`: `Some` while a reply is pending
      and NOT `focus==Compose && compose.mode==Insert`.

## Verification

- Headless paint tap (like `code_block_selection_is_painted_and_aligned`): assert
  the source range renders the blockquote style while the reply is pending + not
  typed, and is ABSENT after submit/abandon. NC: disable the render branch → no
  marker paints.
- Exact glyphs/colors are harness gap #1 (human eye) — the "beautiful" bar.

## Notes

- Source line indices are stable while idle (no streaming) and the reply freezes
  at the tail (below the source), so line indices don't shift before clear.
  Consider editor anchors if a resume/replay could move them.
- Persistence: pending-scoped by decision (2026-08-06). If the user later wants a
  permanent "replied to this" mark, widen the clear rule.
