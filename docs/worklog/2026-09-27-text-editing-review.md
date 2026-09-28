# Worklog: text-editing code review + fixes

**Date:** 2026-09-27
**Branches touched:** `text-edit-review` (+ package branches `te-p2-engine`,
`te-p3-render`, `te-p4-compose`, `te-p5-clip-diff` merged into it). **Not merged
to `main`** — Scott asked for no merge.

## Cog execution evidence

- Graph id: `exa`

### Initial render

```text
graph text-editing-code-review (frontiers)
frontier 0: survey [open]
frontier 1: line-input [open], perf-fixes [open]
frontier 2: verify [open]
frontier 3: report [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `cyb4` `survey`: claimed → closed; output: `{"doc":"docs/research/2026-09-27-text-editing-review.md","findings":{"A":7,"B":17,"C":11,"D":15,"E":4}}`
- `bdi3` `line-input`: claimed → closed; output: `{"commit":"5e3c77b","ids_fixed":["A1","A2","A3","A4","A5","A6","A7"],"negative_control":"left: \"/tmp/ws/projyy\""}`
- `mziw` `p2-engine`: claimed → closed; output: `{"fixed":["B1","B3","B4","B5","B6","B7","B10","B11","B13","B15","B16","D1"]}`
- `cmu7` `p3-render`: claimed → closed; output: `{"fixed":["C1","C2","C4","C5","D2","D5","D6"]}`
- `cbtt` `p4-compose`: claimed → closed; output: `{"fixed":["D3","D4","D7","D8","D9","D10"]}`
- `1z91` `p5-clip-diff`: claimed → closed; output: `{"fixed":["C6","C7","E1","E2"]}`
- `p11a` `verify`: claimed → closed; output: `{"tests":{"lib":"257 passed","yalda-gpui":"856 passed","session-server":"69 passed"}}`
- `xtwh` `report`: claimed → closed; output: review Outcome + worklog + backlog
- omega `fkqp`: claimed → closed; output: `{"complete":true}`

### Notes

- Graph note `decision`: `perf-fixes` was replaced by four package nodes (P2–P5),
  run by parallel agents in sub-worktrees branched from `text-edit-review` and
  merged back into it (never `main`).

### Final status

- Status: `complete`

```text
graph text-editing-code-review (frontiers)
frontier 0: survey [done]
frontier 1: p5-clip-diff [done], line-input [done], p4-compose [done], p3-render [done], p2-engine [done]
frontier 2: verify [done]
frontier 3: report [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

Full findings + per-ID outcome: `docs/research/2026-09-27-text-editing-review.md`.

- **P1: one single-line input** (`src/line_input.rs`, `yux/line_input.rs`). All 14 ad-hoc
  `String` push/pop fields now share it. It adds a movable caret, word and line
  delete, and one typed-char policy (`KeyPress::typed_char`). This fixes three
  things: Cmd/Ctrl chords typed their bare letter in six fields, Option-composed
  characters were blocked in three, and prefilled paths could only be edited by
  backspacing from the end. The send picker now caches its ranking. Recorded as
  `UXI-TextEditing-5`.
- **P2: engine.** Fixes the undo-group clobber on Insert-mode Delete, two
  frozen-line guard bugs (the off-by-one and `dd` on the last line), the backspace
  range, the modified flag vs the save point, and the `e`/`b` motions. Typed runs
  now merge into one splice, and inserts are bulk (paste, seed, restore).
  Frozen-line checks use binary search and range overlap. Anchors shift only the
  affected tail. Word motions share one CharClass. Compose paste is now one undo
  step.
- **P3: render.** The highlight cache now aligns on shared prefix and suffix, so
  inserting near the top re-highlights 1 line instead of 1000. There is one
  `display_text` helper (it was duplicated 5×), and the WP line kinds update
  incrementally. Four caret fixes: the caret no longer drifts after tabs (Code,
  WP, compose, transcript), the WP selection no longer switches to the code font,
  the caret on markdown-stripped transcript lines is placed correctly, and
  building a chatbox row is no longer quadratic. The compose lines are cached on
  `(edit_seq, visible_cols)`.
- **P4: compose.** Adds `Document::is_blank`. The slash query checks only the
  first char, and the topic query reads only the caret line. The topic catalog is
  an `Rc`, and the TEMP diagnostics only run when enabled. Popups are only
  computed when visible. A rejected prompt is restored in Worksheet mode too. The
  draft is saved on quit and before restart, and the sessions file is written
  atomically.
- **P5: clipboard + diff.** Yank and put go through the GPUI clipboard; the
  blocking `wl-copy`/`xclip` subprocesses are removed. Counted `x` is now one
  delete and one undo step. There is one `focused_text_input()` resolver, which
  makes Cmd-V work in a Diff comment. The Diff card snapshot is shared and keyed
  on `review_gen`.

## Open / unresolved
- The branch is not merged: Scott reviews it and decides on the merge.
- The deferred findings are listed in the review's Outcome section. The largest are
  C3 (Doc tiles re-parse on sibling edits) and C10/D11 (Edit body and You-block are
  not cached entities). Also: B2 (undo frozen snapshot), B8 (ropey line breaks),
  B14 (unused tree-sitter reparse), and A8 (a synchronous filesystem walk on every
  file-filter keystroke).
- Found while fixing: text typed in the agent compose can't be undone; the mouse
  hit-test on lines with tabs still uses display columns.

## Decisions
- The `LineInput` model lives in the lib crate, because the lib `FileBrowser`
  shares it. The Alt policy is: type only an OS-composed character, never a bare
  ASCII alphanumeric.
- The Cog graph filter keeps its clamp-on-edit behavior; only the doc comment was
  wrong. Every other field resets its selection on edit.

## Verification status
- `cargo test`: all targets green on `text-edit-review` — lib 257, yalda-gpui 856
  (the main baseline was 833), session-server 69, and every `tests/*.rs`. `cargo build --bins` is clean.
- Every bugfix guard was observed RED with the fix reverted (the evidence is in the
  node outputs and the commit messages).
- Nothing was restarted, and no release binary was built for activation.
  NEEDS-RUNTIME after a merge: gap 1 (caret and font pixels), and the Restart-path
  draft save ordering (which spawns a real process).
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-27-text-editing-review.md` passes.

## Next
- Scott: review `text-edit-review` and merge. Then pick deferred items from the review.
