# Diff Review Tile (`App::Diff`)

**Status:** DRAFT (rev 2 — worktree-bound review with stored comments; supersedes
the rev-1 session-bound design. See ADR-0039.)
**Last updated:** 2026-09-27

## Builds On

- **`spec-tiles-and-apps.md` / ADR-0019** — Tiles hold exactly one App. WHY:
  `App::Diff` is a peer App variant (precedent: `App::Linear`); HOW: the variant
  and its tile payload live beside the others; the split tree stays generic.
- **`spec-turn-steering.md`** — prompt delivery via `send_prompt_to_session`.
  WHY: sending review comments to an agent is just a prompt; HOW: the send
  command delivers one short prompt through the existing path (mid-turn it
  steers, idle it prompts). No new transport.
- **`spec-yux.md` + `yux/CLAUDE.md`** — cached-view rules. WHY: the diff body is
  an expensive, mostly-stable surface; HOW: it renders as a cached child entity
  (`cached_child`) with a render-count test.
- **Jump palette (`jump_palette.rs`)** — the `cmd-p` fuzzy session list. WHY:
  choosing which session receives comments should feel exactly like jumping to
  a session; HOW: the send picker reuses the palette's item model and fuzzy
  filter.

## Overview

**Problem.** Agents write code in worktrees faster than Scott reviews it. He
needs one calm place to read a branch's changes, tick files off as he reads
them, jot comments against specific lines, and hand those comments to *any*
agent session to act on.

**Why rev 2.** Rev 1 bound the tile to an agent *session* and diffed the
session's `cwd`. Sessions start in the primary checkout and do their work in
`.claude/worktrees/<slug>`, so a session-bound tile usually diffed `main`
against itself and showed nothing. It also coupled review to one conversation,
reviewed at the git-hunk grain (whose boundaries git chooses, not the reader),
and threw comments away after sending. Rev 2 makes the **worktree** the thing
under review, the **file** the thing you mark, and the **review file** (JSON)
the durable home for comments, which can be sent to any session.

`App::Diff` stays read-only with respect to code: it shells out to `git` and
parses unified-diff text (C1). Named entities:

- **`DiffTile`** — tile payload: an optional bound worktree + view state
  (cursor, visual range, collapse, comment compose, send picker).
- **`DiffModel`** — the parsed diff: branch, base, merge-base, files → hunks →
  lines. Each file carries a **`file_hash`** (its review identity).
- **`Review`** — the persisted review record for one branch: viewed files,
  comments, last session sent to. One JSON file per branch (Data Model).
- **Hunk** — a contiguous block of changed lines plus ~3 lines of unchanged
  context, as produced by `git diff` (`@@ -40,6 +40,8 @@`). In rev 2 hunks are
  purely a **navigation** unit (`{`/`}`); nothing is marked per hunk.

The diff shown is always **merge-base(base, HEAD) → working tree** (committed +
uncommitted + untracked), base = the repo's default branch.

## Behaviors

- **B1. Pick a worktree. [DRAFT]** An unbound tile renders the **worktree
  picker**: every entry of `git worktree list --porcelain` for the repo
  containing the active workspace's cwd (fallback: process cwd), one row each
  — branch name prominent, path dimmed (home-relative), primary checkout
  labelled. Rows are selectable with `j`/`k`/arrows + `Enter`, or a mouse click.
  A final row "Pick a folder…" binds an arbitrary path. Not in a git repo ⇒ the
  picker says so plainly and offers only the folder row. `space → Switch
  worktree` returns a bound tile to the picker. A deleted/invalid worktree
  renders an inline error with a "Pick another worktree" hint, never a panic.

- **B2. Read the diff. [DRAFT]** A bound tile shows a header (branch, base,
  `N/M files viewed` progress, `K unsent comments`) and, per file, a file header
  row (status glyph, path, `+a −r`, Viewed checkbox) followed by its diff lines
  in monospace with add/remove backgrounds and old/new line-number gutters. A
  **line cursor** (left accent bar + subtle row highlight) moves with `j`/`k`/
  arrows across every visible row; `}`/`{` jump to next/prev hunk, `]`/`[` to
  next/prev file, `z` collapses/expands the file under the cursor, mouse click
  on a line moves the cursor there. A keyboard-hint footer always lists the
  current keys. Untracked files appear as all-added (non-mutating listing, no
  `git add -N`). Text zoom scales the diff body; chrome stays fixed. Empty diff
  ⇒ "No changes on <branch> vs <base>."

- **B3. Refresh. [DRAFT]** The diff re-derives by re-running git (async, off
  the paint path) when the tile **gains focus** and on `r`. The previous model
  stays on screen until the new one lands (a quiet "refreshing" indicator in
  the header). The cursor stays on the same file + nearest line after a
  refresh. There are no session-driven triggers.

- **B4. Viewed files. [DRAFT]** `v` (or clicking the checkbox) toggles
  **Viewed** on the file under the cursor. A viewed file collapses to its
  header row, dimmed, with a check. Viewed-ness is keyed by `path + file_hash`
  (hash of the file's diff content lines, position-independent), so **any
  change to that file's diff clears Viewed automatically** — no timestamps.
  Marking a file viewed moves the cursor to the next unviewed file. When every
  file is viewed the header reads "All files viewed ✓".

- **B5. Comments. [DRAFT]** `c` on the cursor line opens a comment compose
  pinned at the bottom of the tile, captioned with its anchor
  ("commenting on src/foo.rs:40–46"), while the anchored lines stay highlighted
  in the diff; `V` starts a line-range selection (extend with `j`/`k`, confined
  to one file; `Esc` cancels) and `c` comments on the range. `Enter` inserts a
  newline, `Ctrl-Enter` / `Cmd-Enter` saves, `Esc` closes an empty draft — on a
  non-empty draft the first `Esc` only warns ("Esc again to discard") and a
  second discards. A saved comment is written to the review file immediately as
  **unsent** and renders inline under its last anchor line as a card (body,
  "unsent"/"sent to <session> · <time>"/"outdated" badge). `e` edits the
  comment under the cursor; `x` deletes it after a confirming second `x`.
  Comments anchor to the new-side line numbers (old side only when every
  anchored line is removed) and store a **snippet** (the anchored lines' text)
  so they can be relocated. When the anchored content no longer appears in that
  file's diff, the comment is marked **outdated** (kept, never deleted or
  silently moved) and listed at the top of its file. Comments do not require a
  session.

- **B6. Send comments.** `s` (and `space → Send comments…`) opens the
  **send picker**, a `cmd-p`-style fuzzy session list (the jump palette's item
  model, ranking and panel) over every agent session the GUI knows — the
  palette's agent tiles first, then free loaded sessions, then the rest of the
  universal roster; archived sessions hidden — whose initial selection
  is this review's `last_sent_session` when that session still exists (tagged
  "last sent"; otherwise the first row). Typing edits the query; ↑/↓ and
  ctrl-n/ctrl-p move. A session this GUI has not attached is prompted by
  server sid — no attach, no tile bind, and the Diff tile keeps focus. The
  review file is written before the prompt is delivered. `Enter` sends one short prompt
  via `send_prompt_to_session` naming the review file's **absolute path** and
  the ids of every **unsent** comment, with the instruction to read the file
  and address those comments (line numbers may have drifted — use `snippet`;
  do not edit the review file). On success each sent comment gains a
  `sent: [{session, at}]` entry and `last_sent_session` is updated; on failure
  nothing is marked and a status line explains. With zero unsent comments the
  picker does not open; the status line says "No unsent comments." Already-sent
  comments can be re-sent explicitly (`S` = send all, including sent; with no
  comments at all: "No comments.").

- **B7. Open in Zed. [DRAFT]** `o` spawns `zed <abs-path>:<cursor line>`
  fire-and-forget; a missing `zed` shows a status hint.

- **B8. Leaders. [DRAFT]** Space = tile verbs (Switch worktree, Refresh, Send
  comments…, Open in Zed); `.` = shell verbs (ADR-0032). The comment compose and
  the send picker are the tile's only text-input surfaces.

## Data Model

```rust
struct DiffTile {
    worktree: Option<PathBuf>,       // None ⇒ worktree picker
    picker: WorktreePickerState,     // rows + selection (unbound only)
    model: Option<DiffModel>,        // last derived diff (kept during refresh)
    review: Option<Review>,          // loaded review for model.branch
    rows: Rc<Vec<RowRef>>,           // visible rows: File / Hunk / Line{old,new}
    cursor: usize,                   // flat index into rows
    range_anchor: Option<usize>,     // V-selection start
    folds: Folds,                    // user collapse / expand (viewed ⇒ folded unless expanded)
    compose: Option<CommentCompose>, // new or editing comment
    send_picker: Option<SendPicker>,
    refreshing: bool, error: Option<String>,
}

struct FileDiff { path, status, hunks: Vec<Hunk>, added, removed, file_hash: u64 }
```

**`Review`** — persisted at
`<primary-checkout-root>/.yaldabaoth/reviews/<sanitized-branch>.json`, where the
primary checkout root is the parent of `git rev-parse --git-common-dir`. Every
worktree of a repo therefore shares one reviews directory, it survives the
feature worktree's deletion, and agents can read it without touching `.git/`.
On first write, `/.yaldabaoth/` is appended (idempotently) to
`<git-common-dir>/info/exclude` so it never shows as untracked. Branch names
are sanitized (`/` → `__`); a detached HEAD uses `detached-<worktree-dir-name>`.

```json
{
  "version": 2,
  "branch": "diff-rework",
  "base": "main",
  "worktree": "/abs/path/to/worktree",
  "viewed": { "src/foo.rs": 1234567890 },
  "next_comment": 4,
  "comments": [
    { "id": "c3", "path": "src/foo.rs", "side": "new", "lines": [40, 46],
      "snippet": "fn foo() {\n    bar();\n}", "body": "rename this",
      "created": "2026-09-27T14:03:00Z",
      "sent": [{ "session": "<session id>", "label": "<session title>", "at": "…" }],
      "outdated": false }
  ],
  "last_sent_session": "<session id>"
}
```

The GUI is the only writer (atomic tmp + rename). Writes happen off the render
path. `viewed` entries whose `file_hash` no longer matches are dropped on write.
Comments are never garbage-collected (outdated ones stay until deleted).
Under `cfg(test)` the root takes the standard `*_PATH_OVERRIDE` seam.

**Persistence (workspace).** A Diff tile persists as its worktree path only
(`PersistedKind::Diff { worktree }`); an unbound tile restores unbound.

## Interfaces

View methods on `YaldaGpuiView` (module-internal):

- `open_diff_inner(cx)` — new unbound tile (picker). B1.
- `bind_diff_worktree(id, path, cx)` / `diff_unbind(id, cx)` — B1.
- `refresh_diff(id, cx)` → async `collect_raw_diff` + `load_review` → `diff_apply`. B3.
- `toggle_file_viewed(id, cx)` — B4.
- `open_comment_compose` / `submit_comment` / `edit_comment` / `delete_comment` — B5.
- `open_send_picker(id, include_sent, cx)` / `send_review_comments(id, sid, cx)` — B6.
- `open_in_zed(id, cx)` — B7.

Subprocess boundary (`diff_git.rs`): `collect_raw_diff(worktree, base)` and
`list_worktrees(repo_dir)`; errors are values. Pure core: `diff_model.rs`
(parser, `file_hash`) and `review_state.rs` (schema, viewed/comment ops,
`recompute_outdated`, `build_send_prompt`).

The send prompt (exact shape, one message):

```
Review comments for branch `<branch>` (worktree <abs worktree path>).
Read <abs review json path> and address comments <c3, c5, c7>.
Each comment has `path`, `lines` (may have drifted since — locate by `snippet`)
and `body`. Do not edit the review file.
```

## State Machines

File viewed (per path):

```
 unviewed ──v──► viewed ──(file diff content changes ⇒ new file_hash)──► unviewed
     ▲             │
     └─────v───────┘
```

Comment:

```
 draft-compose ──save──► unsent ──send──► sent ──send (S)──► sent (+entry)
                           │                │
                           └──(anchor content gone)──► outdated (flag; still unsent/sent)
 any ──x──► deleted
```

## Constraints

- **C1. No in-process git.** All diff/worktree data comes from the git CLI; yalda
  parses, never computes.
- **C2. Paint-path purity.** Git subprocesses and review-file I/O never run on
  the render path; the diff body is a `cached_child` with a render-count test;
  no `cx.notify()` in render.
- **C3. Read-only code.** No staging, applying, or editing from this tile. The
  only write surface is the review file (and the one-time `info/exclude` line).
- **C4. Session-free review.** Every behavior except B6 works with no agent
  session at all.
- **C5. Test hygiene.** Tests use tempdir git fixtures; never the user's repos or
  `~/.yalda`; the review root has a path-override seam.
- **C6. Idiot-proof.** Every state has visible guidance: picker empty/not-a-repo
  text, key-hint footer, empty-diff text, status line for every refusal.

## Out of scope (later)

Suggestion comments (replacement code), resolved/unresolved threads, a
review-level summary note, syntax highlighting, side-by-side view, file watcher
refresh. The merge gate, pre-merge hook, `--hash-diff` subcommand, session
binding, session-driven refresh, and the jump-panel unreviewed badge from rev 1
are **removed** (ADR-0039).

## Revision History

- 2026-08-29 — rev 1 DRAFT → ACTIVE via Cog graph `ec3`: session-bound
  cumulative diff, hunk-hash review marks in the git common dir, comment →
  steering, two-layer merge gate, jump-panel unreviewed badge, open in Zed.
- 2026-09-27 — rev 2 DRAFT (Cog graph `8g7`, ADR-0039). Scott found session
  binding broken in practice (sessions' cwd is the primary checkout, not their
  worktree) and unintuitive. Redesign: worktree picker; file-level Viewed
  (GitHub/Gerrit convention) with hunks as navigation only; line/range comments
  stored as drafts in `<primary>/.yaldabaoth/reviews/<branch>.json`; send to any
  session via a `cmd-p`-style picker defaulting to the last session sent to,
  pointing the agent at the file + comment ids; refresh on focus + `r`. Merge
  gate, hook, `--hash-diff`, session triggers, and unreviewed badge deleted.
