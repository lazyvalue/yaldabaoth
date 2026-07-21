# Bug Manifest

The index of every bug we've worked on. **Read this first** (via `/bug`) before
attacking a bug — if it's here, the linked file holds the history of what was already
tried, so we don't attempt the same failed fix twice (that's the definition of
insanity). Higher fidelity than git: each bug file carries context, the planned fix,
and a **timestamped log** of every actual attempt.

## How this works

- Each bug is `docs/bugs/bug-<NNNN>-<slug>.md` (zero-padded, sequential). Template:
  `docs/bugs/_template.md`.
- A bug addressed **more than once** does NOT get a new file — it gets a new
  timestamped entry appended to the bottom of its existing file. One bug, one file,
  a growing log.
- Status: `OPEN` (known, not fixed) · `IN-PROGRESS` · `FIXED` (fix landed +
  verified) · `RECURRED` (came back after a FIXED — see its latest log entry) ·
  `WONTFIX`.

## Bugs

| Id | Slug | Status | First seen | Times addressed | One-line |
|----|------|--------|------------|-----------------|----------|
| bug-0001 | created-session-not-persisted | RECURRED→FIXED | 2026-07-14 | 3 | created server-managed sessions weren't persisted (resume_id/channel both None) → picker on restart; fixed by resolving the sid via the store's `sid_of` |
| bug-0002 | restore-drops-replayed-history | FIXED | 2026-07-15 | 1 | on restore, replayed history vanished when a respawn (gen bump) happened while the §9 gate was closed — the deferred rebaseline's `reset_for_replay` wiped legacy-rendered replay the gated reducer had skipped; fixed by applying the rebaseline eagerly at `ChannelOpened` |
| bug-0003 | transcript-selection-cursor-line-no-token-hits | FIXED | 2026-07-15 | 1 | transcript mouse-select copied the wrong text when the transcript was focused: the caret line rendered via the caret-injection path which registered NO hit-test tokens, so a mouse-down there snapped the anchor to the nearest other line; fixed by routing every emitted piece (incl. caret cell + empty lines) through `register_token_on_paint` in `build_wrapped_line` |
| bug-0004 | two-adjacent-you-blocks | FIXED | 2026-07-16 | 1 | two You-blocks rendered adjacent (next to each other) when the blank line between their anchors collapsed — `open_you_block_at_cursor` keyed on raw anchor equality, not render slot; fixed with `you_blocks_would_be_adjacent` (all-blank-between ⇒ same slot ⇒ resume, don't spawn); separated multi-insertion preserved |
| bug-0005 | duplicate-claude-labels-on-restore | FIXED | 2026-07-16 | 1 | after GUI restore, two sessions in one cwd both named bare 'claude' — three fallbacks default to bare "claude" and the restore loop never dedupes labels; fixed with `unique_label`/`dedupe_slot_labels` in `load_persisted_acp_sessions` + running dedup in `restore_agent_leaves` (missing label → numbered claude-N) |
| bug-0006 | transcript-select-copies-wrong-span-on-stripped-lines | FIXED | 2026-07-16 | 1 | transcript drag-select copied a shifted raw slice (`:** scott+…`) on markdown lines — `build_wrapped_line` registered `TokenHit.start_char` in STRIPPED render space but the editor/`selection_text` slice RAW; fixed by registering raw offsets via `stripped_to_raw_cols` (+ raw→rendered band conversion). Localized by a Fable subagent |
| bug-0007 | jump-panel-sessions-spontaneously-reorder | FIXED | 2026-07-16 | 1 | jump-panel agent sessions reorder on their own — `jump_panel_agent_rows` sorted roster sessions by label but APPENDED local-only ones last, so a fresh local session hopped into its label slot once the async roster refresh caught up; fixed by sorting the combined roster+local list by label |
| bug-0008 | cannot-select-parsed-blocks-in-transcript | FIXED | 2026-07-16 | 1 | can't mouse-select tables / bullet lists / code in the transcript — `FlatItem::Block` renders via `block_inner` whose `RenderCtx` has NO token-hit sink, so parsed blocks register zero `TokenHit`s; fixed with `register_block_hits_on_paint` (per-line bands for lists/code, PER-CELL bands for tables via `parse_table_cell_ranges`); runtime-unverified |
| bug-0009 | camera-pan-does-not-snap-to-slots | FIXED | 2026-07-17 | 1 | Cmd+Shift free-pan rests on a fractional slot — `desktop_drop`'s `pan_drag` branch was the one branch that saved+returned WITHOUT `snap_camera_to_slots()` (UXI-Workspace-8 left free-pan out of scope). Fixed by snapping on pan release; UXI-Workspace-8 reconciled to include free-pan |
| bug-0010 | transcript-selection-grows-on-streamed-text | FIXED | 2026-07-17 | 1 | agent-streamed text auto-selects — transcript `selection_range = anchor..cursor` with an auto-advancing caret; `splice_insert` shifts the cursor but not the anchor, and `move_cursor_to_tail` jumps the caret without clearing the anchor, so a persisted click/select balloons over new content. Fix: shift the anchor in splice + collapse selection on the tail-jump |

<!-- Example row once populated:
| bug-0001 | chatbox-caret-offscreen | RECURRED | 2026-05-02 | 16 | caret + text scroll out of the visible chatbox; no single owner of caret-in-viewport |
-->
