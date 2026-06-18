# 001 — Deferred deep worksheet bugs (streaming / undo / fingerprint)

Higher-scope, pre-existing issues surfaced by the four analysis subagents that
are NOT the reported insert/render repro. Each needs runtime repro + careful,
separately-tested fixes.

- [ ] **Streaming cursor drift.** `EditorCore::programmatic_insert` shifts
  `frozen_lines`, `lockable_through_line`, `line_anchors`, `last_llm_line` for an
  insert above a position — but the cursor lives on `EditorView`, a struct the
  core can't reach, and no pump caller compensates. A chunk streamed above the
  caret leaves the cursor on what is now agent content; the next keystroke
  edits/splits frozen text. Fix: a `shift_cursor_for_insert/_delete` applied by
  the `Editor` wrapper (which holds both view+core) on every programmatic splice.
  Verify in `verify_harness.rs`: set cursor mid-draft, run a chunk through
  `apply_reply_events`, assert `editor.cursor().line` tracked the user's text.

- [ ] **Floor only covers EOF tail.** `agent_tail_floor_char` walks up from EOF
  and stops at the first frozen/tagged line, so a user draft *between* two frozen
  blocks is invisible and a streamed chunk can land below/inside it. Fix: protect
  every editable region the turn's insertion point could fall into (or make the
  splice anchor-relative to the turn's own frozen lines).

- [ ] **Undo/redo wipes TurnId/tool metadata.** `undo`/`redo` call
  `reset_line_anchors`, dropping every `TurnId`/tool tag; a continuing stream then
  `find_llm_insertion_point` → `None` → appends at EOF, and the gutter blanks.
  Fix: snapshot+restore anchor metadata with the undo group, or re-derive tags
  from the restored frozen ranges.

- [ ] **`view_model_fingerprint` excludes cursor + `edit_seq`.** The memoized
  flat build's blank-collapse `protect_line` and `build_nav_stops` read the
  cursor line and per-line blank classification, neither of which is in the
  fingerprint, so a cursor-only move (no `line_count` change) can reuse a flat
  list built for a different cursor → caret on a collapsed line / stale nav-stops.
  Fix: fold `cursor_line` (worksheet only) into the fingerprint, OR move the
  cursor/content-sensitive passes out of the memoized build into the per-frame
  closure. Mind the O(changed) typing-latency invariant (ADR-0020 / INV-RV).
