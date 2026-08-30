# Component: Diff Review Tile

**Status:** living
**Component token:** `Diff` (⇒ invariants are `UXI-Diff-N`)

## Description

`App::Diff` is a read-only **review tile**: it shows a worktree's cumulative
changes (`merge-base(base, HEAD) → working tree`, so committed *and* uncommitted
changes both appear), lets Scott mark hunks reviewed, send a hunk-anchored
comment back to the authoring agent, open a file in Zed, and gate a merge on
"everything reviewed". Like lazygit it owns **no** git logic — it shells out to
`git` and parses unified-diff text (Constraint C1).

Primary code homes:

- **`diff_model.rs`** — the pure, unit-testable parser. `parse_diff(raw, worktree,
  branch, base, merge_base) -> DiffModel`. `DiffModel{worktree, branch, base,
  merge_base, dirty, files}`; `FileDiff{path, status, hunks, added, removed}`;
  `Hunk{header, lines, hunk_hash, reviewed}`; `DiffLine{Context|Added|Removed}`.
  **`hunk_hash`** = `hash(repo-relative path + per-file occurrence index +
  content lines)`, excluding the `@@` position numbers — its review identity. No
  filesystem/subprocess in this module.
- **`diff_git.rs`** — the async git subprocess boundary. `collect_raw_diff(worktree,
  base) -> Result<RawGitDiff, GitDiffError>` (merge-base, diff, status/dirty,
  untracked via `ls-files` + `diff --no-index` — never `git add -N`, worktree
  list, branch). Errors are values, never panics. Runs off the paint path.
- **`review_state.rs`** — `ReviewState` persisted at
  `<git-common-dir>/yalda-review/<branch>.json` (`{reviewed_hashes:[u64]}`), the
  common dir so the merge hook (running in the primary checkout) reads the same
  marks the tile wrote from a feature worktree. `load_review_state` /
  `save_review_state` (GCs dead hashes on write) / `resolve_git_common_dir` /
  `join_reviewed_flags`. `*_PATH_OVERRIDE` test seam.
- **`diff.rs`** — the tile data model + pure helpers. `DiffSource{Session(SessionId)
  | Path(PathBuf)}`, `DiffTile{source, model, focus, collapsed, compose,
  refreshing, …}`, nav (`move_hunk_focus`/`jump_file`/`toggle_collapsed`),
  `merge_gate_decision(model, feature_clean, primary_clean) -> Result<(),
  MergeRefusal>`, `zed_open_arg`, `build_hunk_comment_prompt`, and the
  `DiffProjections` type (`HashMap<PathBuf, usize>`).
- **`diff_view.rs`** — the **yux cached child** (`DiffView`) that renders the diff
  body: a file list with `+/-` counts and per-file monospace hunk blocks
  (add=green / remove=red / context=default), a left focus bar on the focused
  hunk, reviewed hunks visibly distinct, body text scaled by `text_scale`. It
  observes the **root** entity and self-notifies only when its `DiffSeqs`
  fingerprint moves.
- **`diff_ui.rs`** — the `YaldaGpuiView` methods: `bind_diff_source`/`diff_unbind`,
  `refresh_diff`/`diff_apply` (the async derive pipeline), `open_diff_inner`
  (open a new selector tile), review-mark toggles, comment compose open/submit,
  `open_hunk_in_zed`, `diff_merge_focused`/`diff_install_hook_focused`, and
  `handle_diff_key`.

**States.** A tile is either **unbound** (`source: None` ⇒ renders the selector:
sessions whose cwd is a git repo, plus "pick a path") or **bound** to a
`Session` (worktree = the session's cwd) or a `Path`. The comment compose is the
tile's only insert-mode surface. Keys (bound, not composing): `j`/`k` hunk focus,
`[`/`]` file jump, `z` collapse file, `r` refresh, `v` toggle hunk reviewed, `V`
mark file reviewed, `c` comment (session-bound only), `o` open in Zed. Space =
tile verbs (refresh/bind/merge/install-hook), `.` = shell verbs (B9 leaders).

## References

- `docs/specs/spec-diff-review.md` — the design doc (behaviors B1–B9, Data Model,
  Constraints C1–C6).
- `docs/components/common/*` — the yux cached-view rules the body obeys.
- `docs/components/jump-panel.md` — the unreviewed badge rides the jump panel
  (`UXI-Diff-6`).
- ADR-0019 / `spec-tiles-and-apps.md` — `App::Diff` is a peer App variant.
- `spec-turn-steering.md` — comment→steering rides `send_prompt_to_session`.

## UX invariants

### UXI-Diff-1 — Cumulative diff paints; nav moves hunk focus

**Statement.** A bound tile derives and paints a file list plus per-file
monospace hunk blocks (add/remove colored); `j`/`k` move the focused hunk (across
files), `[`/`]` jump files, `z` collapses a file. A deleted/invalid worktree
renders an inline error, never a panic.

**Applies to.** `diff_view.rs::DiffView`, `diff_ui.rs::{refresh_diff, diff_apply,
handle_diff_key}`, `diff.rs::{move_hunk_focus, jump_file, toggle_collapsed}`.

**Why.** The tile is worthless if changes don't render or navigation strands
focus; a missing worktree must not crash the app (spec B1/B2).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::diff_tile_paints_files_and_hunks_and_jk_moves_focus`
(layout probe, non-vacuous), `diff_tile_invalid_worktree_is_inline_error_not_panic`.

### UXI-Diff-2 — Diff body is O(changed): typing elsewhere doesn't re-render it

**Statement.** The diff body is a cached child that re-renders only when its own
inputs (`DiffSeqs`: source/model/focus/collapse/refreshing/error/zoom) change; an
unrelated root notify leaves its render count flat. No `cx.notify()` runs on the
render path.

**Applies to.** `diff_view.rs::{DiffView, DiffSeqs}`, embedded via `cached_child`.

**Why.** Per-keystroke O(whole tree) render is the module's central perf trap
(yux rules).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::diff_view_unrelated_root_notify_is_render_flat`.

### UXI-Diff-3 — Refresh on session activity; focus survives by hash

**Statement.** The diff re-derives when the bound session's turn completes and
(debounced) after a file-mutating tool-call completes, plus manual `r`. The old
model shows until the new one lands. The focused hunk stays put when its
`hunk_hash` still exists, else focus moves to the nearest hunk.

**Applies to.** `agent_ui.rs` reducer chokepoints (`drain_diff_*`), `agent.rs`
(`finalize_agent_turn_idem`, file-change gen), `diff_ui.rs::{refresh_diff,
diff_apply}`, `diff.rs::restore_focus_by_hash`.

**Why.** Agents edit continuously; a stale or focus-jumping diff is unusable
(spec B3).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_rederives_on_bound_session_turn_completion,
diff_tile_rederives_debounced_after_file_changing_tool_call,
diff_tile_triggered_refresh_preserves_focus_when_hunk_unchanged,
diff_tile_triggered_refresh_moves_focus_to_nearest_when_hunk_hash_gone}`.

### UXI-Diff-4 — Review marks are hash-keyed and persist

**Statement.** `v` toggles the focused hunk reviewed; `V` marks the whole focused
file. Marks are keyed by `hunk_hash`, persisted to `ReviewState` in the git
common dir, and re-joined on every derive — so a content edit (new hash) reverts
a hunk to unreviewed automatically, with no timestamps. `ReviewState` I/O never
runs on the render path.

**Applies to.** `diff_ui.rs::{toggle_hunk_reviewed, mark_file_reviewed,
set_hunks_reviewed}`, `review_state.rs`, `diff_view.rs` (reviewed styling covered
by `model_gen`).

**Why.** Staleness must need no manual bookkeeping (spec B5).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_v_toggles_hunk_reviewed_and_persists,
diff_tile_content_edit_reverts_hunk_to_unreviewed, diff_tile_shift_v_marks_whole_file_reviewed}`.

### UXI-Diff-5 — Hunk comment steers the authoring session

**Statement.** On a **session-bound** tile, `c` on a focused hunk opens a pinned
Compose; submit sends, via `send_prompt_to_session`, a machine-readable prefix
(repo-relative path + new-line range + hunk patch) followed by the comment — mid
turn it steers, idle it prompts. A send failure keeps the draft. A `Path`-bound
tile has no comment affordance.

**Applies to.** `diff_ui.rs::{open_hunk_comment, submit_hunk_comment,
cancel_hunk_comment}`, `diff.rs::build_hunk_comment_prompt`, `screens.rs`
compose render.

**Why.** Review feedback must reach the agent with enough context to act, without
a new transport (spec B4).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_c_opens_comment_compose_on_session_bound_tile,
diff_tile_path_bound_c_opens_no_comment_compose, diff_tile_esc_cancels_comment_compose,
diff_tile_comment_submit_failure_keeps_draft,
diff_hunk_comment_open_type_submit_delivers_prefixed_prompt}` (last is
`#[cfg(feature = "test-support")]` — asserts the real delivered `PromptPayload`).

### UXI-Diff-6 — Unreviewed count projects to the jump panel

**Statement.** Each derive updates a root-owned `DiffProjections` map
(`worktree → unreviewed_count`); the jump panel shows that count on a session
row whose cwd maps to a counted worktree, alongside (not replacing) other row
marks. The projection survives tile close and is not persisted; a worktree never
opened shows nothing.

**Applies to.** `main.rs` (`diff_projections` field), `diff_ui.rs::diff_apply`,
`jump_panel_view.rs` (`AgentRow::unreviewed_hunks`, `unreviewed_badge_label`),
`diff_model.rs::unreviewed_hunk_count`.

**Why.** Review debt must be visible where sessions are navigated (spec B6).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{jump_panel_paints_unreviewed_badge_after_diff_derive,
jump_panel_unread_and_unreviewed_badge_coexist, diff_projection_survives_diff_tile_close}`.

### UXI-Diff-7 — Two-layer merge gate refuses unreviewed / dirty merges

**Statement.** The tile `merge` verb refuses unless every current hunk is
reviewed and both the feature worktree and the primary checkout are clean; it
merges in the primary checkout (`git -C <primary> merge --no-ff`) and aborts on
conflict, never leaving markers. An installable `pre-merge-commit` hook recomputes
the **same** predicate from `ReviewState` + `git diff` via the hidden
`yalda-gpui --hash-diff` subcommand (single normalization, C6), sets
`merge.ff false`, adds a `MERGE_HEAD`-gated `pre-commit` fragment, and fails
closed if the binary is missing.

**Applies to.** `diff.rs::{merge_gate_decision, MergeRefusal}`,
`diff_git.rs::{execute_merge_no_ff, install_merge_gate_hook, worktree_is_clean}`,
`diff_ui.rs::{diff_merge_focused, diff_install_hook_focused}`, `main.rs`
`--hash-diff`, `scripts/yalda-pre-merge-hook`, `diff_model.rs::hunk_hashes`.

**Why.** An unreviewed branch must not merge; the hook catches merges by agents
or at the CLI (spec B7). `--no-verify` is a documented residual hole (defense in
depth, not access control).

**Status.** `implemented`

**Enforcement.** `diff_git.rs::tests::{execute_merge_no_ff_merges_cleanly_when_no_conflict,
execute_merge_no_ff_conflict_aborts_and_leaves_no_markers, pre_merge_hook_refuses_unreviewed_merge,
pre_merge_hook_allows_fully_reviewed_clean_merge, pre_merge_hook_fails_closed_when_binary_missing,
hash_diff_subcommand_output_matches_diff_model_hashes,
installer_merge_ff_false_prevents_fast_forward_merge_commit}`,
`diff.rs::merge_gate_decision_tests`.

### UXI-Diff-8 — Open in Zed; open an unbound Diff tile

**Statement.** `o` on a focused hunk spawns `zed <abs-path>:<first-new-line>`
fire-and-forget; a missing `zed` surfaces a status hint, no panic. `OpenDiff`
(global `cmd-*` action + menu) opens a new **unbound** Diff tile (the selector).

**Applies to.** `diff.rs::zed_open_arg`, `diff_ui.rs::{open_hunk_in_zed,
open_diff_inner}`, `keymap_registry.rs`, the `on_action(Self::open_diff)` wiring.

**Why.** Deep exploration belongs in Zed, not re-built here; and the tile must be
reachable from the running GUI (spec B8 / B1).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_o_key_missing_zed_binary_sets_status_hint_no_panic,
diff_tile_o_key_with_no_model_is_noop_no_panic}` + the `open_diff_inner` open test.

### UXI-Diff-9 — Restart restores Path-bound to the same worktree

**Statement.** The workspace persists a Diff tile as its worktree path only
(`PersistedKind::Diff{worktree}`); a session-bound tile restores `Path`-bound to
the same worktree (a `SessionId` is runtime-local); an unbound tile restores
unbound.

**Applies to.** `persist.rs` (`PersistedKind::Diff` snapshot/restore arms),
`diff.rs::DiffTile::worktree`.

**Why.** The diff outlives the conversation (spec Persistence).

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_session_bound_persists_and_restores_as_path_bound,
diff_tile_unbound_persists_and_restores_unbound}`.
