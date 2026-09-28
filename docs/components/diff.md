# Component: Diff Review Tile

**Status:** living
**Component token:** `Diff` (⇒ invariants are `UXI-Diff-N`)

## Description

`App::Diff` is a read-only **review tile** for one git **worktree**: it shows the
branch's cumulative changes (`merge-base(base, HEAD) → working tree`), lets Scott
tick files off as **Viewed**, write **line/range comments** that are saved to a
per-branch review file, and **send** those comments to any agent session. It owns
no git logic — it shells out to `git` and parses unified-diff text. Design:
`docs/specs/spec-diff-review.md` (rev 2); why: ADR-0040.

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

**Keys (bound).** `j`/`k` (and ↓/↑) line · `}`/`{` hunk · `]`/`[` file · `G` last
row · `z` fold · `v` Viewed · `o` Zed · `r` refresh · `V` range (j/k extend
within the file, `Esc` clears) · `c` comment · `e` edit · `x` `x` delete
(implemented) · `s` send unsent · `S` send all (send node). **Compose keys:**
typing, `Enter` newline, `Ctrl-Enter`/`Cmd-Enter` save, `Esc` closes an empty
draft; on a non-empty one the first `Esc` warns and the second discards;
leaders are suppressed while composing. Space = tile verbs, `.` = shell verbs. The footer lists the live
keys (`DIFF_KEY_HINTS`).

**Row model.** The bound body is a virtualized `gpui::list` over the tile's
cached `rows: Rc<Vec<RowRef>>` (`RowRef::{File, Hunk, Line{old,new},
Comment{comment,part,parts}, ComposeSlot{part,parts}}`, from the pure
`visible_rows(model, review, folds)` plus the compose slots spliced in by
`DiffTile::rebuild_rows`); the cursor is a flat index into it. A comment card is
`parts` fixed-height `Comment` rows forming ONE bordered box (`CardLine::Header`
— id pill + status pill; then the body wrapped at `COMMENT_WRAP_COLS`
characters, capped at `COMMENT_MAX_BODY_ROWS`; an outdated card appends its
snippet, dimmed; then `CardLine::Footer`), placed after the last line of its
snippet's nearest match (`place_comment`), or right after the file header when
outdated / unplaceable. The ring is a 1px frame (outer fill = border color,
opaque inner fill) so contiguous rows join into one outline.
While the comment compose is open, `ComposeSlot` rows (`compose_slot_rows`:
draft visual lines clamped to 3..=12, + 3 chrome rows) sit directly under the
anchor's last line (after existing cards there), or replace the edited card's
rows. The body paints them empty; the root paints the editor over them with
`yux::list_rows_overlay` (uncached, after the body in tree order, placed by
uniform-row arithmetic over the list's scroll top, clipped to the viewport).
**Any future inline row kind (e.g. context Expander rows) must keep the
uniform row height** — both `reveal_cursor` and the overlay placement assume
it — and should be a new `RowRef` variant that `is_inline_insert`/`host_row`
and `compose_slot_place` classify correctly.
Every row is one fixed height (`diff_row_h`), so keeping the cursor in view is
exact arithmetic (`compose_first_visible_line`), never gpui's unmeasured-row
estimate. A viewed file folds unless `z`-expanded (`Folds`).

## References

- `docs/specs/spec-diff-review.md` — design (B1–B8, Data Model, C1–C6).
- ADR-0040 — why worktree-bound, file-level Viewed, review JSON location, removals.
- `docs/components/common/*` — yux cached-view rules the body obeys.
- `jump_palette.rs` — the fuzzy list the send picker reuses.

## UX invariants

### UXI-Diff-10 — Worktree picker binds by keyboard or click

**Statement.** An unbound tile lists every `git worktree list` entry of the active
repo — two lines per row: line 1 the branch (primary labelled), line 2 a dimmed
description `<HEAD commit subject> · <relative age> · <~/path>` (just the path when
the commit is unknown), each line one line with an ellipsis, never blank — plus
"Pick a folder…";
`j`/`k` + `Enter` or a mouse click binds the tile to that worktree and derives its
diff. Outside a git repo the picker says so and offers only the folder row.
`space → Switch worktree` returns to the picker.

**Status.** `implemented` (graph 8g7 node worktree-picker)

**Enforcement.** `verify_harness.rs::{diff_picker_lists_and_paints_worktrees,
diff_picker_rows_paint_label_and_description,
diff_picker_long_branch_label_ellipsizes_with_visible_prefix,
diff_picker_j_enter_binds_second_worktree_and_derives,
diff_picker_click_row_binds_worktree, diff_picker_not_a_repo_offers_only_folder_row,
diff_tile_bound_persists_and_restores_bound}`; `diff_git.rs::{list_worktrees_*,
parse_head_commits_reads_batched_log}`; `diff_view.rs::picker_description_tests`.
The label/description guards read the SHAPED text through the `probe_text` seam
(bug-0072: rows painted a bare "…" while their boxes had full width).

### UXI-Diff-11 — Diff paints with a line cursor; nav never strands it

**Statement.** A bound tile paints a header (branch, base, `N/M files viewed`,
unsent count), per-file header rows and monospace diff lines with old/new gutters.
The line cursor is always on a visible row; `j`/`k`, `}`/`{`, `]`/`[`, `z`, and
click move it predictably across files, skipping collapsed content. An invalid
worktree renders an inline error, never a panic; an empty diff renders an
explicit "No changes" message. A key-hint footer is always visible.

**Status.** `implemented` (graph 8g7 nodes file-viewed-ui, comments-ui — the
header paints `K unsent` when K > 0).

**Enforcement.** `verify_harness.rs::{diff_tile_paints_rows_and_line_cursor_keys_move_it,
diff_tile_click_row_moves_cursor, diff_tile_cursor_stays_painted_in_view_after_many_j,
diff_empty_diff_paints_no_changes, diff_tile_invalid_worktree_is_inline_error_not_panic}`;
`diff.rs::row_model_tests::*` (pure nav, folds, anchors).

### UXI-Diff-12 — Diff body is O(changed)

**Statement.** The body is a cached child re-rendering only when its `DiffSeqs`
inputs change; an unrelated root notify leaves its render count flat; typing in
the comment compose does not re-render the body. No `cx.notify()` on the render
path.

**Status.** `implemented` — the body is a cached child whose `DiffSeqs` covers
`model_gen`, `rows_gen`, `cursor`, `review_gen`, `range_anchor`, `compose_gen`
(open/close only), refreshing/error, picker, zoom; rows are virtualized
(O(visible)). The inline comment compose is painted by the root OVER its slot
rows (`yux::list_rows_overlay`), outside the cached body, so typing in it
leaves the body's render count flat; only a change in the draft's visual line
count (a slot-row rebuild, `rows_gen`) re-renders the body.

**Enforcement.** `verify_harness.rs::{diff_view_unrelated_root_notify_is_render_flat,
diff_view_v_and_j_rerender_the_cached_body, diff_view_v_range_rerenders_the_cached_body,
diff_compose_typing_is_render_flat}`.

### UXI-Diff-13 — Refresh on focus and `r`; cursor survives

**Statement.** The diff re-derives when the tile gains focus and on `r`, async,
keeping the old model painted until the new one lands. The cursor stays on the
same file (nearest line) when that file still exists. No session activity
triggers a refresh.

**Status.** `implemented` — focus-gain + `r` refresh (focus edge detected in the
per-frame `diff_reconcile`); the line cursor re-resolves by `CursorAnchor`
(same path, nearest new-side line, else old-side, else the file header; file
gone ⇒ clamped).

**Enforcement.** `verify_harness.rs::{diff_tile_rederives_on_focus_gain,
diff_cursor_survives_refresh_on_same_file}`;
`diff.rs::row_model_tests::anchor_survives_a_shift_and_falls_back_when_file_gone`.

### UXI-Diff-14 — Viewed is per file, persisted, and self-clearing

**Statement.** `v` / checkbox click toggles Viewed on the cursor's file; a viewed
file collapses to a dimmed header with a check, and the cursor advances to the
next unviewed file. Viewed is stored as `path → file_hash` in the review file, so
any change to that file's diff clears it on the next derive. Progress
`N/M files viewed` is always accurate.

**Status.** `implemented` (graph 8g7 node file-viewed-ui). The derive prunes
stale entries and re-saves; a `v` during an in-flight derive wins over the
derive's older load.

**Enforcement.** `verify_harness.rs::{diff_v_marks_file_viewed_persists_folds_and_advances,
diff_edit_clears_viewed_and_prunes_review_json, diff_checkbox_click_toggles_viewed}`;
`diff.rs::row_model_tests::tile_toggle_viewed_advances_then_unmark_reexpands`.

### UXI-Diff-15 — Comments are authored inline, saved as drafts, boxed, marked outdated

**Statement.** `c` (line) or `V…c` (range) opens the compose **inline,
GitHub-style: directly under the commented line** (the range's last line;
below any cards already there) — a bordered, rounded box whose header names
what is being commented (`Comment on a.txt:40–46` / `Editing c3 on a.txt:40`)
and whose footer hints `ctrl-enter save · esc cancel`. It moves with the diff
when it scrolls and grows with the draft. Saving writes the comment to the
review file immediately as unsent and paints it inline under its anchor as a
card — one bordered box (full ring, rounded corners, padding) with id + status
pills and the body in the prose font; no emoji anywhere. `e` edits in the same
inline compose, opened in place of the card; `x` deletes. When the anchored
snippet no longer appears in the file's diff the comment is flagged outdated
and listed at the file's top — never silently deleted or moved. No session is
required.

**Status.** `implemented` (graph 8g7 node comments-ui; placement + box revised
in graph kfa node comment-inline — the compose was previously pinned at the
tile's bottom). Deviations/decisions: the editor is a root-level overlay over
`ComposeSlot` rows rather than an element inside the cached list (typing must
not re-render the body, UXI-Diff-12); the draft is monospace and hard-wraps at
`COMPOSE_WRAP_COLS` characters (fixed-height rows), showing at most 12 lines
with the caret line kept in view; the compose text and cards scale with the
text zoom. A focused card (cursor on any of its rows) shows an accent ring and
`e edit · x delete` in its header instead of a row tint. `x` needs a second `x`
on the same card (the first shows "x again to delete c3"; any other key
disarms). A range spanning removed and added lines anchors to the new side
(new-side lines only); old side only when every line is removed. `c`/`e`/`x`/`V`
off their target rows show a hint. Cards of a folded (e.g. viewed) file are
hidden with it; comments on files no longer in the diff stay in the JSON but
have no row to render under.

**Enforcement.** `verify_harness.rs::{diff_comment_c_saves_json_and_paints_card_below_anchor,
diff_compose_inline_tracks_scroll, diff_comment_card_paints_bordered_box_without_emoji,
diff_comment_v_range_saves_span_and_snippet, diff_comment_edit_and_confirmed_delete,
diff_comment_outdated_after_change_paints_after_file_header,
diff_comment_esc_needs_two_presses_on_nonempty_draft, diff_compose_typing_is_render_flat}`;
`diff.rs::row_model_tests::{comment_placement_anchor_and_card_lines,
compose_layout_wraps_places_caret_and_sizes_slots, range_selection_is_clamped_to_one_file}`;
`yux::list::tests::uniform_rows_rect_tracks_the_scroll_top`. Negative controls
observed RED (graph kfa): slots at the end of the rows (compose not under the
anchor); overlay ignoring the list scroll top; slot rows rebuilt per keystroke
(10 body renders for 10 keys); card ring padding removed (inner == outer); `💬`
restored on the id pill; `e` not replacing the card.

### UXI-Diff-16 — Send picker defaults to the last session and records delivery

**Statement.** `s` opens a fuzzy session picker preselecting this review's
`last_sent_session` (if it still exists). `Enter` delivers one prompt via
`send_prompt_to_session` containing the absolute review-file path and exactly
the unsent comment ids; on success those comments gain a `sent` entry and
`last_sent_session` updates; on failure nothing changes and the status line
explains. Zero unsent ⇒ no picker, status "No unsent comments." `S` sends all.

**Status.** `implemented` (graph 8g7 node send-picker). Deviations/decisions:
the picker lists every **agent session** the GUI knows — the agent tiles
`Cmd-P` lists (palette order, so live tile-bound sessions come first), then
sessions loaded in the store that no tile binds, then every other non-archived
session in the universal roster (by label) — not only tile-bound ones. A
session loaded here is delivered through `send_prompt_to_session` (transcript
echo + turn start; busy Codex ⇒ steer); a roster-only session is prompted by
server sid directly — **no attach, no tile bind, no focus change** (the Diff
tile keeps focus). "Success" is the prompt being accepted for delivery (the
server prompt is fire-and-forget; an async `PromptRejected` for an unattached
session only logs). The review file is written (background executor) **before**
the delivery attempt; a failed write aborts the send. The picker reuses the
jump palette's `PaletteItem` / `rank_palette_items` / panel render
(`render_palette_panel`), rendered at screen level over the tile (not a
`DiffSeqs` input). Typing goes to the query (j/k are letters); ↑/↓ and
ctrl-n / ctrl-p move; the last-sent row is tagged "last sent"; with no
last-sent match the first row is selected. The recorded session key is the
server sid (`local-<n>` for a sid-less local session). `S` with zero comments
hints "No comments."; also `space → send comments…`.

**Enforcement.** `verify_harness.rs::{diff_send_s_delivers_unsent_ids_to_last_sent_default,
diff_send_failure_records_nothing_and_file_written_first}` (feature
`test-support`: assert the real delivered `PromptPayload`) and
`diff_send_picker_query_is_render_flat_and_filters`.

### UXI-Diff-17 — Review file location and hygiene

**Statement.** The review file lives at
`<primary-checkout-root>/.yaldabaoth/reviews/<sanitized-branch>.json`, shared by
all worktrees of the repo; the first write adds `/.yaldabaoth/` to
`<git-common-dir>/info/exclude`. Writes are atomic and never on the render path;
tests never write outside a tempdir override.

**Status.** `implemented` — every write is spawned on the background executor
through `save_review_latest` (process-wide lock + per-tile generation, so an
out-of-order stale snapshot never overwrites a newer one). Harness tests write
only inside their tempdir fixture (the fixture IS the primary checkout).

**Enforcement.** `review_state.rs::review_v2_tests::*` (layout, atomic write,
exclude idempotence, override root, stale-save skip);
`verify_harness.rs::diff_v_marks_file_viewed_persists_folds_and_advances`
(the review file never shows up as an untracked change after a re-derive).

### UXI-Diff-8 — Open in Zed; open an unbound Diff tile

**Statement.** `o` spawns `zed <abs-path>:<cursor line>` fire-and-forget; a
missing `zed` surfaces a status hint, no panic. `OpenDiff` (`cmd-d` /
`ctrl-shift-d` / File menu / `.` new tile) opens a new **unbound** Diff tile.

**Status.** `implemented` — targets the cursor row (`zed_target`: a line's
new-side number; a removed line's new-file position; a header's first new line).

**Enforcement.** `verify_harness.rs::{diff_tile_o_key_missing_zed_binary_sets_status_hint_no_panic,
diff_tile_o_key_with_no_model_is_noop_no_panic}` + the `open_diff_inner` open test.

### UXI-Diff-9 — Restart restores the same worktree

**Statement.** The workspace persists a Diff tile as its worktree path only
(`PersistedKind::Diff{worktree}`); a bound tile restores bound to the same
worktree; an unbound tile restores unbound.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::{diff_tile_unbound_persists_and_restores_unbound,
diff_tile_bound_persists_and_restores_bound}`.

## Retired (rev 1, ADR-0040)

- **UXI-Diff-1** (hunk-focus nav) → superseded by UXI-Diff-11.
- **UXI-Diff-2** (O(changed)) → superseded by UXI-Diff-12.
- **UXI-Diff-3** (session-driven refresh) → superseded by UXI-Diff-13.
- **UXI-Diff-4** (hunk-hash review marks) → superseded by UXI-Diff-14.
- **UXI-Diff-5** (hunk comment steers the bound session) → superseded by UXI-Diff-15/16.
- **UXI-Diff-6** (jump-panel unreviewed badge) → removed.
- **UXI-Diff-7** (two-layer merge gate) → removed.
