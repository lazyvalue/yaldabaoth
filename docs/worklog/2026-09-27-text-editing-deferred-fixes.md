# Worklog: text-editing-deferred-fixes

**Date:** 2026-09-27 · **Graph:** `ls2` · **Branch:** `te-q1..q5` → `text-edit-review` → `main` (`7c0cb75`)

## Shipped
- Q3 doc tiles (C3): only painted Doc tiles re-parse; `c3_hidden_doc_does_not_reparse_on_sibling_edit_and_is_fresh_when_shown` → `a2281df`.
- Q5 pickers + diff (A8, A9, E3):
  - the recursive file-filter search runs in the background;
  - `KeyedMemo` caches the palette and filter rankings;
  - diff comments are grouped by path once.

  Landed as `11eae4d`.
- Q1 engine (B2, B12, B8, B9, B14, B17, compose undo): undo reapplies the frozen-line shifts; ropey counts only `\n` as a line break, with CRLF round-tripped; the undo stack is capped at 1000 groups; the tree-sitter parse is lazy; a failed key prefix replays its first key; compose typing can be undone. Landed as `9455fb7`.
- Q2 edit view (C8–C11, tab-line click):
  - cached `EditBodyView`;
  - a sibling tile's edits no longer reset this tile's scroll;
  - one list-marker parser (`src/md_line.rs`);
  - the gutter sizes to the line count and scales with zoom.

  Landed as `cd688ad`.
- Q4 compose (D11–D15):
  - the You-block is its own cached view (`you_block_view.rs`);
  - a width change re-wraps the compose;
  - the end-of-line caret stays inside the box;
  - wrapping counts terminal cells (`unicode-width`), and pasted CRLF becomes LF;
  - there is one `reset_to`;
  - the diff comment compose uses the shared renderer.

  E4 was already bounded by kfa; I added a guard for it. Landed as `7c0cb75`.
- Every bugfix guard was observed RED with its fix reverted (names are in the review's Outcome). On `main` `7c0cb75`, `cargo test` passes 1475 with 0 failures and `cargo build --bins` is clean.

## Caveats
- NEEDS-RUNTIME, gap 1 (pixels): the You-block overlay look, the caret and selection fonts, and the zoomed gutter. Scott rebuilds and restarts; nothing was restarted.
- The Edit view and chatbox have no mouse hit-testing, so the tab-line click fix exists only in the transcript. In a mixed-ending file, Backspace can leave a `\r`. The jump palette still rebuilds its items on every render.
- The `tests/session_resilience_test.rs` flake predates this work (it also failed 1 in 6 runs on `main` without these changes). The land script retries that suite.

## Decisions
- D11 draws the cached You-block over a placeholder rather than nesting it, because GPUI dirties every parent of a notified view. C8 reveals the caret only while the tile is focused. B8 normalizes only files that are CRLF throughout.
- Resilience flake: Cog bulletin `yaldabaoth/session-server::follow-ups` (`an7`, entry `yx4`).

## Cog
- Status: `complete`

```text
graph text-editing-deferred-fixes (frontiers)
frontier 0: q1-engine [done], q4-compose [done], q2-edit-view [done], q3-doc-tiles [done], q5-pickers-diff [done]
frontier 1: verify [done]
frontier 2: report [done]
frontier 3: omega [done] (omega)
```
