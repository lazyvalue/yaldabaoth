# Worklog: Diff Review Tile (`App::Diff`)

**Date:** 2026-08-29
**Branch:** `main`
**Spec:** `docs/specs/spec-diff-review.md` (DRAFT → ACTIVE)
**Component:** `docs/components/diff.md` (`UXI-Diff-1..9`)

## What shipped

A new read-only review App, `App::Diff`, implementing the whole
`spec-diff-review.md` (B1–B9, Data Model, C1–C6) via Cog graph `ec3`:

- **Pure diff parser** (`diff_model.rs`) — `parse_diff` → `DiffModel`; `hunk_hash`
  = hash(path + per-file occurrence index + content lines), excluding `@@`
  positions, so identical hunks in one file review independently and a
  position-only shift keeps the hash.
- **Async git boundary** (`diff_git.rs`) — `collect_raw_diff` (merge-base, diff,
  status, untracked via `ls-files`+`diff --no-index` — never `git add -N`),
  errors as values, off the paint path. Plus `execute_merge_no_ff` /
  `install_merge_gate_hook`.
- **ReviewState** (`review_state.rs`) — reviewed hashes at
  `<git-common-dir>/yalda-review/<branch>.json`, GC on write, path-override seam.
- **Tile + view** (`diff.rs`, `diff_view.rs`, `diff_ui.rs`) — `DiffSource`
  (Session|Path), selector, cumulative diff body as a yux `cached_child`
  (root-observed, monospace add/remove coloring, focus bar), j/k/[/]/z/r/v/V/c/o
  keys, leaders, text-zoom, the async derive pipeline, review marks, comment→
  steering, open-in-Zed.
- **Refresh triggers** — re-derive on bound-session turn completion + debounced
  file-mutating tool-call completion (reducer chokepoints), focus survives by
  hash.
- **Unreviewed badge** — root-owned `DiffProjections`, projected onto jump-panel
  session rows.
- **Two-layer merge gate** — tile `merge` (all-reviewed + clean, `--no-ff`,
  abort-on-conflict) + installable `pre-merge-commit` hook recomputing the same
  predicate via the hidden `yalda-gpui --hash-diff` (single normalization, C6),
  `merge.ff false`, `MERGE_HEAD` pre-commit fragment, fail-closed. Script:
  `scripts/yalda-pre-merge-hook`.
- **Open gesture** — `OpenDiff` (`cmd-d` / `ctrl-shift-d` / File menu / `.`
  new-tile) opens an unbound selector tile (spec B1).

## Verification

- `cargo test --bin yalda-gpui` → **826 passed, 0 failed, 2 ignored**
  (the 2 ignored are pre-existing live-agent integration tests).
- `cargo test --lib` → **213 passed, 0 failed**.
- Every node shipped a headless guard test exercising the real path
  (layout probe / real keystrokes / real reducer / real snapshot-restore / real
  git fixtures + shell hook) and an **observed-RED negative control**. No
  `NEEDS-RUNTIME` flags. yux perf rules honored (`DiffView` cached child +
  `diff_view_unrelated_root_notify_is_render_flat`).

## Open / follow-ups

- The jump panel has no per-row **unread** glyph today (only the workspace-folder
  aggregate is tinted — `docs/components/jump-panel.md`); B6 "alongside the unread
  badge" is realized as the unreviewed chip painting while the row's `unread`
  flag is independently true. A visible per-row unread mark would be a separate
  jump-panel change.
- `git merge/commit --no-verify` bypasses the hook — documented residual hole
  (defense in depth, not access control), per spec B7.
- Real GUI paint/colors and the live GUI↔server↔agent loop remain the standard
  human-runtime gaps; not exercised here.

## Cog execution evidence

- Graph id: `ec3`
- Name: `diff-review-tile`
- Actor: `claude-code`

### Initial render

```
graph diff-review-tile (frontiers)
frontier 0: diff-parser [open], git-boundary [open]
frontier 1: review-state [open], app-diff-tile [open]
frontier 2: refresh-triggers [open], review-marks-ui [open], comment-steering [open], open-in-zed [open], diff-persistence [open]
frontier 3: badge-projection [open], merge-gate [open]
frontier 4: integrate-verify [open]
frontier 5: omega [open] (omega)
```

### Node execution

Each node was claimed with `cog node claim-next --with-inputs`, executed
(fanned out to Sonnet subagents where file-independent), verified on `main`
(build + test + observed-RED negative control), then closed with
`--resolution done` and a JSON `output`. Frontier 0 + review-state ran in
parallel; the tile-internal nodes (f2, f3) ran serially because they share
`diff_ui.rs` / `main.rs` / `verify_harness.rs`.

- `diff-parser` (5md2) claimed → closed done, output: parser + `hunk_hash`, 9
  tests, commit `7579667`.
- `git-boundary` (ln8z) claimed → closed done, output: `collect_raw_diff`, 4
  tests, commit `d80a17b`.
- `review-state` (m7xl) claimed → closed done, output: `ReviewState` store, 6
  tests, commit `d80a17b`.
- `app-diff-tile` (nd0e) claimed → closed done, output: `App::Diff` + `DiffView`,
  3 tests, commit `2b631b3`.
- `refresh-triggers` (7ods) claimed → closed done, output: B3 triggers, 4 tests,
  commit `8e70352`.
- `review-marks-ui` (fb5x) claimed → closed done, output: v/V marks, 3 tests,
  commit `febaa52`.
- `comment-steering` (hk81) claimed → closed done, output: hunk comment → send,
  6 tests, commit `df0e1f5`.
- `open-in-zed` (oc72) claimed → closed done, output: `o` → zed, 4 tests, commit
  `1a17501`.
- `diff-persistence` (w5a4) claimed → closed done, output: persist round-trip, 2
  tests, commit `ea6f3ed`.
- `badge-projection` (1cxd) claimed → closed done, output: `DiffProjections` +
  jump badge, 3 tests, commit `b5ea347`.
- `merge-gate` (v5tg) claimed → closed done, output: merge gate + hook +
  `--hash-diff`, ~25 tests, commit `b428225`.
- `integrate-verify` (arms) claimed → closed done, output: `OpenDiff` + docs,
  commit `d89b5f7`.
- `omega` (fhoz) claimed → closed done, output: full-feature aggregate.

### Notes

- Graph note (deviation): `app-diff-tile` added the minimal exhaustive
  `PersistedKind::Diff { worktree }` arm so `persist.rs` compiled;
  `diff-persistence` therefore verified it with a round-trip test rather than
  re-introducing the arm.
- Execution note: Agent-tool worktree isolation branches from pre-scaffold `main`
  (`9b9fa0c`), which lacks the new modules, so every node needing
  `diff_model`/`diff_git`/`review_state`/`App::Diff` ran in the **main checkout**;
  frontier-0 pure modules ran in isolated worktrees and were copied forward onto
  the committed scaffold.

### Final status

- Status: `complete`
- Render:

```
graph diff-review-tile (frontiers)
frontier 0: diff-parser [done], git-boundary [done]
frontier 1: review-state [done], app-diff-tile [done]
frontier 2: refresh-triggers [done], review-marks-ui [done], comment-steering [done], open-in-zed [done], diff-persistence [done]
frontier 3: badge-projection [done], merge-gate [done]
frontier 4: integrate-verify [done]
frontier 5: omega [done] (omega)
```
