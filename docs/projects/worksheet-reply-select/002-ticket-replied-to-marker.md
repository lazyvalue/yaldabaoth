# 002 — The `>` replied-to source marker (when not editing)

Implements **UXI-AgentTile-37**. The source agent line(s) a pending worksheet
reply quotes render with a beautiful blockquote marker (`>` / left bar + italic)
in the transcript, so it is obvious what a pending reply refers to. Shown while
NOT typing in the reply block; pending-scoped (clears on submit / abandon).

## Goal

A reply that quotes agent text (via `r` / selection, UXI-AgentTile-21/-35) marks
its source in the transcript with a `>` blockquote, visible whenever the reply's
compose is not the active typing surface.

## Subtasks

- [ ] Capture the quoted source range in `AgentState` (`reply_source_range:
      Option<(usize, usize)>` or a small `Vec`) inside `reply_quote_at_cursor`
      (both the selection and sentence branches).
- [ ] Clear it on submit (`freeze_you_blocks` path), on `close_you_block`, and on
      the `u`-pop back-out (UXI-AgentTile-24).
- [ ] Thread it into the transcript render: add to the `TranscriptView` snapshot
      + a `TranscriptSeqs` seq (per the cached-surface rule), and add a
      `transcript_021_*` render-count test.
- [ ] Frozen-line render branch: when the marker is active AND the reply is not
      being typed (`!(focus==Compose && compose.mode==Insert)`), style each source
      line as a blockquote (left bar + italic, theme `blockquote_bar` /
      `blockquote_text`).
- [ ] Decide "not typing" gate precisely and keep it consistent with
      `inline_you_block_active`.

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
