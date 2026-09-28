# Worklog: Diff Review rework (rev 2 — worktree-bound, Viewed files, stored comments)

**Date:** 2026-09-27
**Branch:** `diff-rework` → `main`
**Spec:** `docs/specs/spec-diff-review.md` rev 2 · **ADR:** 0039 ·
**Component:** `docs/components/diff.md` (`UXI-Diff-8..17`; 1–7 retired)

## Why

Scott found the rev-1 Diff tile's session binding broken and unintuitive: agent
sessions start in the primary checkout and work in `.claude/worktrees/<slug>`, so
a session-bound tile diffed `main` against itself. He redesigned it in
conversation: pick a worktree, mark files reviewed, keep comments in a JSON file,
send them to any session.

## What shipped

- **Worktree picker** — unbound tile lists `git worktree list` (branch, dimmed
  path, primary tag) + "Pick a folder…"; `j`/`k`/Enter or click binds. Session
  binding, session-driven refresh triggers, and the jump-panel unreviewed badge
  are gone. Refresh on tile focus gain (`diff_reconcile` focus edge) + `r`.
- **Line cursor + virtualized body** — uniform-height rows (file header / hunk
  header / line / comment-card rows); `j`/`k`, `}`/`{`, `]`/`[`, `G`, `z`, click;
  exact scroll-to-cursor; old/new gutters; add/remove tints; key-hint footer.
- **Per-file Viewed** — `v` / checkbox; keyed `path → file_hash` so any change to
  the file's diff clears it; viewed files fold; cursor advances to next unviewed;
  header `N/M files viewed` + progress bar / "All files viewed ✓".
- **Comments** — `c` (line) / `V…c` (range) → bottom-pinned compose with anchor
  caption; saved immediately to the review JSON as unsent drafts with snippet;
  inline cards under the anchor; `e` edit, `x x` delete; outdated comments flagged
  (never moved/deleted) and shown after the file header.
- **Send picker** — `s` (unsent) / `S` (all) / space → Send comments…: reuses the
  jump palette's items + fuzzy filter; preselects the review's last-sent session;
  writes the review file first, then sends one short prompt naming the absolute
  JSON path + comment ids; records `sent` + `last_sent_session` on success only.
  Lists palette agent tiles, then free loaded sessions, then server-roster
  sessions (non-archived); roster-only sessions are prompted by server id without
  attaching or stealing focus.
- **Review file** — `<primary-checkout-root>/.yaldabaoth/reviews/<branch>.json`
  (`/`→`__`), atomic writes off the render path, serialized latest-wins saves,
  `/.yaldabaoth/` added once to `<git-common-dir>/info/exclude`.
- **Removed** — merge gate, `pre-merge-commit` hook + `scripts/yalda-pre-merge-hook`,
  hidden `--hash-diff` subcommand, rev-1 hunk-hash `ReviewState`.

## Verification

- `cargo test --bin yalda-gpui` → **843 passed, 0 failed, 1 ignored**.
- `cargo test --bin yalda-gpui --features test-support` → **855 passed, 0 failed, 1 ignored**.
- `cargo test --lib` → **226 passed, 0 failed, 2 ignored**.
- `cargo build --release --bin yalda-gpui` → ok. **Running GUI/server not touched.**
- Every node shipped headless guards on the real path (real keystrokes, real
  async derive over tempdir git fixtures with linked worktrees, layout probes,
  render-count tests, real delivered `PromptPayload` under `test-support`), each
  with **observed-RED negative controls** (listed in the Cog node outputs).
- `cargo mutants` **not run** — `cargo-mutants` is not installed on this machine;
  CI `mutation-gate` covers `--in-diff`.

## Open / follow-ups

- `NEEDS-RUNTIME` (gap 1, pixels): tints, card styling, checkbox/progress bar,
  "last sent" tag are layout-verified only.
- `NEEDS-RUNTIME` (gap 2, live loop): the server-route prompt to a roster-only
  (not GUI-loaded) session is fire-and-forget; a later server-side rejection is
  only logged and the comments stay marked sent.
- Long diff lines are truncated (uniform-row virtualization), not wrapped or
  horizontally scrollable. `o` opens the line in Zed.
- Comments on a file that has left the diff stay in the JSON but aren't shown in
  the tile.
- Out of scope per spec: suggestion comments, resolved threads, review-level note.

## Cog execution evidence

- Graph id: `8g7`
- Name: `diff-review-rework`
- Actor: `claude-code`

### Initial render

```
graph diff-review-rework (frontiers)
frontier 0: remove-merge-gate [open], spec-revise [open]
frontier 1: worktree-picker [open], review-store [open]
frontier 2: file-viewed-ui [open]
frontier 3: comments-ui [open]
frontier 4: send-picker [open]
frontier 5: polish-verify [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `spec-revise` (le5b) claimed → closed done, output: spec rev 2 + UXI-Diff-10..17 + ADR-0039, commit `c95eaf1`.
- `remove-merge-gate` (4gj0) claimed → closed done, output: gate/hook/`--hash-diff` deleted, commit `29e16ac`.
- `review-store` (ozld) claimed → closed done, output: `Review` JSON API + `file_hash`, 7 negative controls, commit `55bd6be`.
- `worktree-picker` (j73t) claimed → closed done, output: picker + focus refresh + session-binding removal, commit `2298fd6` (merged `2090daf`).
- `file-viewed-ui` (ipwu) claimed → closed done, output: Viewed + progress + line cursor, 6 negative controls, commit `567e6b4`.
- `comments-ui` (p6ys) claimed → closed done, output: draft comments + inline cards, 6 negative controls, commit `b77e5cb`.
- `send-picker` (fy7d) claimed → closed done, output: send picker + delivery recording, 6 negative controls, commit `ce15a43`.
- `polish-verify` (qwiq) claimed → closed done, output: full suites green, release build, docs reconciled, this worklog, merge to `main`.
- `omega` (8l3g) claimed → closed done, output: aggregate.

### Notes

- Graph note (deviation): frontier-1 ran in separate worktrees
  (`diff-rework-store`, `diff-rework-picker`) to avoid concurrent cargo builds
  seeing half-edits; merged back before `file-viewed-ui`.
- Graph note (deviation): virtualized uniform rows ⇒ long lines truncated; range
  select moved from `file-viewed-ui` to `comments-ui`; comment cards are split
  into fixed-height rows.
- Comment compose is bottom-pinned (not inline) so typing never re-renders the
  cached body; spec B5 updated.

### Final status

- Status: `complete`
- Render:

```
graph diff-review-rework (frontiers)
frontier 0: remove-merge-gate [done], spec-revise [done]
frontier 1: worktree-picker [done], review-store [done]
frontier 2: file-viewed-ui [done]
frontier 3: comments-ui [done]
frontier 4: send-picker [done]
frontier 5: polish-verify [done]
frontier 6: omega [done] (omega)
```
