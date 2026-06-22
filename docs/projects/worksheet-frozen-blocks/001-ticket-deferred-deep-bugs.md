# 001 — Deferred deep worksheet bugs (streaming / undo / fingerprint)

Higher-scope, pre-existing issues surfaced by the four analysis subagents that
are NOT the reported insert/render repro. Each needs runtime repro + careful,
separately-tested fixes.

> **2026-06-21 — surfaced live: reopening a multiturn session in Worksheet mode
> ("can't find cursor / undo erased it / tool calls at the bottom").** Data was
> safe (server WAL intact); the break was the worksheet's editor rebuild hitting
> these deferred bugs. **Fixed: streaming cursor drift (F2) + undo wipes metadata
> (C3)** — see checkboxes. The remaining two (floor-only-EOF, fingerprint) are
> edge/perf-sensitive and intentionally NOT blind-patched (see notes).

- [x] **Streaming cursor drift — FIXED.** The cursor lived on `EditorView` while
  `EditorCore::programmatic_insert` shifted only frozen ranges/anchors, so a
  chunk streamed above the caret stranded it on agent content ("can't find my
  cursor" on resume). Fix: `Editor::splice_insert/_delete` (the wrapper holds
  view+core) shift the view cursor on EVERY programmatic splice; all
  append/freeze paths route through them. Test
  `append_llm_chunk_keeps_caret_on_draft_pushed_down` (`editor.rs`).

- [x] **Undo/redo wipes TurnId/tool metadata — FIXED.** `undo`/`redo` called
  `reset_line_anchors`, dropping every tag → gutter blanked + continuation
  appended at EOF ("undo erased it / tool calls at the bottom"). Fix:
  `Document::undo/redo` now return line-level `AnchorShift`s; `EditorCore::
  apply_anchor_shifts` SHIFTS the anchors (metadata is keyed by stable anchor id,
  so it survives) instead of resetting — only delete-consumed anchors drop. Test
  `undo_preserves_frozen_line_metadata` (`editor.rs`).

- [ ] **Floor only covers EOF tail.** `agent_tail_floor_char` walks up from EOF
  and stops at the first frozen/tagged line, so a user draft *between* two frozen
  blocks is invisible and a streamed chunk can land below/inside it. Fix: protect
  every editable region the turn's insertion point could fall into (or make the
  splice anchor-relative to the turn's own frozen lines). **NOT fixed yet —**
  edge case (mid-document draft + live stream), not in the live report; the fix
  touches the delicate `append_llm_chunk_floored` path so it needs its own repro.

- [ ] **`view_model_fingerprint` excludes cursor + `edit_seq`.** The memoized
  flat build's blank-collapse `protect_line` and `build_nav_stops` read the
  cursor line and per-line blank classification, neither of which is in the
  fingerprint, so a cursor-only move (no `line_count` change) can reuse a flat
  list built for a different cursor → caret on a collapsed line / stale nav-stops.
  Fix: fold `cursor_line` (worksheet only) into the fingerprint, OR move the
  cursor/content-sensitive passes out of the memoized build into the per-frame
  closure. Mind the O(changed) typing-latency invariant (ADR-0020 / INV-RV).
