# Component: Diff Review Tile

**Status:** living
**Component token:** `Diff` (⇒ invariants are `UXI-Diff-N`)

## Description

`App::Diff` is a read-only **review tile** for one git **worktree**: it shows the
branch's cumulative changes (`merge-base(base, HEAD) → working tree`), lets Scott
tick files off as **Viewed**, write **line/range comments** that are saved to a
per-branch review file, and **send** those comments to any agent session. It owns
no git logic — it shells out to `git` and parses unified-diff text. Design:
`docs/specs/spec-diff-review.md` (rev 2); why: ADR-0039.

Primary code homes:

- **`diff_model.rs`** — pure parser. `parse_diff(...) -> DiffModel`;
  `FileDiff{path, status, hunks, added, removed, file_hash}`; `Hunk{header,
  lines}`; `DiffLine{Context|Added|Removed}`. `file_hash` = hash(path + content
  lines, no `@@` positions) — a file's review identity.
- **`diff_git.rs`** — async git boundary: `collect_raw_diff(worktree, base)`,
  `list_worktrees(repo_dir)`. Errors are values.
- **`review_state.rs`** — the `Review` JSON (`viewed`, `comments`,
  `last_sent_session`) at `<primary-checkout-root>/.yaldabaoth/reviews/<branch>.json`,
  `info/exclude` upkeep, pure ops (toggle viewed, add/edit/delete comment,
  `recompute_outdated`, `unsent_ids`, `record_sent`, `build_send_prompt`),
  `*_PATH_OVERRIDE` test seam.
- **`diff.rs`** — `DiffTile` (worktree, picker, model, review, cursor, range,
  collapse, compose, send picker) + pure nav helpers + `zed_open_arg`.
- **`diff_view.rs`** — the yux cached child `DiffView` (picker, header, file rows,
  diff lines, inline comment cards, hint footer); self-notifies on `DiffSeqs`.
- **`diff_ui.rs`** — view methods: open/bind/unbind, refresh + apply, viewed,
  comments, send picker + send, open in Zed, `handle_diff_key`.

**States.** **Unbound** (`worktree: None`) ⇒ worktree picker. **Bound** ⇒ diff
with line cursor. Overlays within bound: comment compose, send picker (the only
text-input surfaces).

**Keys (bound).** `j`/`k` line · `}`/`{` hunk · `]`/`[` file · `z` collapse ·
`v` Viewed · `V` range · `c` comment · `e` edit · `x` delete · `s` send unsent ·
`S` send all · `o` Zed · `r` refresh. Space = tile verbs, `.` = shell verbs.

## References

- `docs/specs/spec-diff-review.md` — design (B1–B8, Data Model, C1–C6).
- ADR-0039 — why worktree-bound, file-level Viewed, review JSON location, removals.
- `docs/components/common/*` — yux cached-view rules the body obeys.
- `jump_palette.rs` — the fuzzy list the send picker reuses.

## UX invariants

### UXI-Diff-10 — Worktree picker binds by keyboard or click

**Statement.** An unbound tile lists every `git worktree list` entry of the active
repo (branch prominent, path dimmed, primary labelled) plus "Pick a folder…";
`j`/`k` + `Enter` or a mouse click binds the tile to that worktree and derives its
diff. Outside a git repo the picker says so and offers only the folder row.
`space → Switch worktree` returns to the picker.

**Status.** `implemented` (graph 8g7 node worktree-picker)

**Enforcement.** `verify_harness.rs::{diff_picker_lists_and_paints_worktrees,
diff_picker_j_enter_binds_second_worktree_and_derives,
diff_picker_click_row_binds_worktree, diff_picker_not_a_repo_offers_only_folder_row,
diff_tile_bound_persists_and_restores_bound}`; `diff_git.rs::list_worktrees_*`.

### UXI-Diff-11 — Diff paints with a line cursor; nav never strands it

**Statement.** A bound tile paints a header (branch, base, `N/M files viewed`,
unsent count), per-file header rows and monospace diff lines with old/new gutters.
The line cursor is always on a visible row; `j`/`k`, `}`/`{`, `]`/`[`, `z`, and
click move it predictably across files, skipping collapsed content. An invalid
worktree renders an inline error, never a panic; an empty diff renders an
explicit "No changes" message. A key-hint footer is always visible.

**Status.** `target`

### UXI-Diff-12 — Diff body is O(changed)

**Statement.** The body is a cached child re-rendering only when its `DiffSeqs`
inputs change; an unrelated root notify leaves its render count flat; typing in
the comment compose does not re-render the body. No `cx.notify()` on the render
path.

**Status.** `target`

### UXI-Diff-13 — Refresh on focus and `r`; cursor survives

**Statement.** The diff re-derives when the tile gains focus and on `r`, async,
keeping the old model painted until the new one lands. The cursor stays on the
same file (nearest line) when that file still exists. No session activity
triggers a refresh.

**Status.** `partial` — focus-gain + `r` refresh implemented (focus edge detected
in the per-frame `diff_reconcile`, the one point every focus mutator funnels
through); cursor survival is still hunk-grain until the line cursor lands
(UXI-Diff-11).

**Enforcement.** `verify_harness.rs::{diff_tile_rederives_on_focus_gain,
diff_tile_refresh_preserves_focus_when_hunk_unchanged,
diff_tile_refresh_moves_focus_to_nearest_when_hunk_hash_gone}`.

### UXI-Diff-14 — Viewed is per file, persisted, and self-clearing

**Statement.** `v` / checkbox click toggles Viewed on the cursor's file; a viewed
file collapses to a dimmed header with a check, and the cursor advances to the
next unviewed file. Viewed is stored as `path → file_hash` in the review file, so
any change to that file's diff clears it on the next derive. Progress
`N/M files viewed` is always accurate.

**Status.** `target`

### UXI-Diff-15 — Comments are saved drafts, shown inline, marked outdated

**Statement.** `c` (line) or `V…c` (range) opens a compose; saving writes the
comment to the review file immediately as unsent and paints it inline under its
anchor. `e` edits, `x` deletes. When the anchored snippet no longer appears in
the file's diff the comment is flagged outdated and listed at the file's top —
never silently deleted or moved. No session is required.

**Status.** `target`

### UXI-Diff-16 — Send picker defaults to the last session and records delivery

**Statement.** `s` opens a fuzzy session picker preselecting this review's
`last_sent_session` (if it still exists). `Enter` delivers one prompt via
`send_prompt_to_session` containing the absolute review-file path and exactly
the unsent comment ids; on success those comments gain a `sent` entry and
`last_sent_session` updates; on failure nothing changes and the status line
explains. Zero unsent ⇒ no picker, status "No unsent comments." `S` sends all.

**Status.** `target`

### UXI-Diff-17 — Review file location and hygiene

**Statement.** The review file lives at
`<primary-checkout-root>/.yaldabaoth/reviews/<sanitized-branch>.json`, shared by
all worktrees of the repo; the first write adds `/.yaldabaoth/` to
`<git-common-dir>/info/exclude`. Writes are atomic and never on the render path;
tests never write outside a tempdir override.

**Status.** `target`

### UXI-Diff-8 — Open in Zed; open an unbound Diff tile

**Statement.** `o` spawns `zed <abs-path>:<cursor line>` fire-and-forget; a
missing `zed` surfaces a status hint, no panic. `OpenDiff` (`cmd-d` /
`ctrl-shift-d` / File menu / `.` new tile) opens a new **unbound** Diff tile.

**Status.** `implemented` (cursor-line target updates with UXI-Diff-11)

**Enforcement.** `verify_harness.rs::{diff_tile_o_key_missing_zed_binary_sets_status_hint_no_panic,
diff_tile_o_key_with_no_model_is_noop_no_panic}` + the `open_diff_inner` open test.

### UXI-Diff-9 — Restart restores the same worktree

**Statement.** The workspace persists a Diff tile as its worktree path only
(`PersistedKind::Diff{worktree}`); a bound tile restores bound to the same
worktree; an unbound tile restores unbound.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_unbound_persists_and_restores_unbound,
diff_tile_bound_persists_and_restores_bound}`.

## Retired (rev 1, ADR-0039)

- **UXI-Diff-1** (hunk-focus nav) → superseded by UXI-Diff-11.
- **UXI-Diff-2** (O(changed)) → superseded by UXI-Diff-12.
- **UXI-Diff-3** (session-driven refresh) → superseded by UXI-Diff-13.
- **UXI-Diff-4** (hunk-hash review marks) → superseded by UXI-Diff-14.
- **UXI-Diff-5** (hunk comment steers the bound session) → superseded by UXI-Diff-15/16.
- **UXI-Diff-6** (jump-panel unreviewed badge) → removed.
- **UXI-Diff-7** (two-layer merge gate) → removed.
