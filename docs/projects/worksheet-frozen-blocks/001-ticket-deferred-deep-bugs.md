# 001 — Deferred deep worksheet bugs (streaming / undo / fingerprint)

Higher-scope, pre-existing issues surfaced by the four analysis subagents that
are NOT the reported insert/render repro. Each needs runtime repro + careful,
separately-tested fixes.

> **2026-06-21 — surfaced live: reopening a multiturn session in Worksheet mode
> ("can't find cursor / undo erased it / tool calls at the bottom").** Data was
> safe (server WAL intact); the break was the worksheet's editor rebuild hitting
> these deferred bugs. **Fixed: streaming cursor drift (F2) + undo wipes metadata
> (C3)** — see checkboxes. **2026-06-22: found + fixed the ACTUAL "undo erased
> the buffer" cause (undo-group pollution, below) after a headless repro.** The
> remaining two (floor-only-EOF, fingerprint) are edge/perf-sensitive and
> intentionally NOT blind-patched (see notes).

- [x] **Undo erases the whole transcript (THE "undo erased the buffer" repro) —
  FIXED.** `begin_insert` opens ONE undo group for the entire insert session;
  while it's open, an agent chunk's `programmatic_insert` → `record_splice`
  folded the streamed content INTO the user's group (`pending_undo` was `Some`),
  so exiting insert committed a group holding user text + agent turns and one
  undo inverted ALL of it. Fix: programmatic (agent) splices go through
  `Document::insert_str_at_char_no_undo` / `delete_range_no_undo` (NOT recorded
  on the undo stack) and `shift_recorded_splices` keeps the user's own splices
  position-correct across the interleave. Undo now reverts only the user's
  edits, never agent content. Test
  `worksheet_resume_undo_does_not_erase_transcript` (`verify_harness.rs`).

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

- [x] **`view_model_fingerprint` excludes cursor + `edit_seq` — FIXED**
  (2026-06-22). The memoized flat build's blank-collapse `protect_line` is
  mode-and-cursor-sensitive, but the fingerprint folded in neither, so toggling
  Chatbox→Worksheet (which moves the caret to the trailing blank compose tail)
  OR a worksheet cursor-only move onto a collapsible blank reused a flat list
  built for the other state → caret on a stripped line, rendering **below the
  visible buffer** (the live "cursor can go below the end of the buffer" report).
  Fix (option 1, worksheet-scoped): `view_model_fingerprint` now folds in the
  `InputSurface::Worksheet` discriminant + the worksheet caret line. Insert-mode
  typing already busts via `edit_seq` (no added cost; `transcript_021_chatbox_
  keystroke_is_render_flat` still flat), so this only adds an O(changed) S1
  rebuild on Normal-mode worksheet navigation. Also: `finish_replay` now snaps
  the caret to the editable tail in Worksheet mode so REOPENING a session lands
  it on the last line explicitly (not via stream-shift ordering). Tests:
  `view_model_fingerprint_busts_on_input_surface_and_worksheet_cursor` (tests.rs),
  `worksheet_already_active_during_replay_lands_caret_on_tail` (verify_harness.rs).
  ⚠️ NEEDS-RUNTIME: a `--release` `sample` while holding `j` in a huge worksheet
  to confirm the per-nav rebuild is imperceptible (debug masks the real cost).
