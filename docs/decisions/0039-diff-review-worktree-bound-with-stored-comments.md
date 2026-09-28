# ADR-0039: Diff review is worktree-bound, file-level Viewed, with comments stored in a per-branch review file

**Date:** 2026-09-27
**Status:** accepted
**Related:** `docs/specs/spec-diff-review.md` (rev 2), `docs/components/diff.md`,
Cog graph `8g7`; supersedes the rev-1 design shipped via graph `ec3`.

## Context

Rev 1 of `App::Diff` bound a tile to an agent **session** and diffed the
session's `cwd`. In practice agent sessions start in the primary checkout and do
their work in `.claude/worktrees/<slug>`, so a session-bound tile diffed `main`
against itself and showed nothing. Scott found the binding "doesn't seem to
work" and unintuitive. Rev 1 also marked review per **git hunk** — a unit whose
boundaries git chooses (a nearby edit merges two hunks and resets both marks) —
sent comments straight into the bound session without keeping them, and carried
a two-layer merge gate Scott doesn't want.

## Options considered

**Review target.** (a) session → cwd (rev 1); (b) worktree picked from
`git worktree list`; (c) branch without worktree. Chose **(b)**: the worktree is
what actually holds the changes; it names its branch; no session is needed to
review.

**Review unit.** (a) hunk (rev 1; Cursor/Zed use per-hunk accept/reject, but
for *applying* edits, not reviewing); (b) **file "Viewed"** (GitHub, GitLab,
Gerrit, Critique, Graphite); (c) commit (agents commit in arbitrary chunks);
(d) per-file-per-revision (Reviewable — powerful, famously confusing). Chose
**(b)**, keyed by `path + file_hash` so it self-clears when the file's diff
changes. Hunks remain a navigation unit only.

**Comment home.** (a) sent immediately, not stored (rev 1); (b) stored as drafts
in a review file, sent on demand to any session by pointing the agent at the
file + comment ids. Chose **(b)** (Scott's call): comments outlive a
conversation, can go to any session, and the prompt stays short while the agent
reads current content.

**Review file location.** (a) `<git-common-dir>/yalda-review/` (rev 1 — shared,
but inside `.git/`, which agents avoid/permission-prompt on); (b)
`~/.yalda/reviews/<repo>/`; (c) **`<primary-checkout-root>/.yaldabaoth/reviews/<branch>.json`**
(Scott's call). (c) is shared by every worktree, survives worktree deletion,
is plainly readable by agents, and is kept out of `git status` by an automatic
`info/exclude` entry (local, never committed).

## Decision

Adopt the rev-2 spec: worktree picker; file-level Viewed; line/range draft
comments with snippet + outdated flag in the review JSON; `cmd-p`-style send
picker defaulting to the review's last session, sending the absolute JSON path
and unsent comment ids; refresh on focus + `r`. **Delete** the merge gate,
`pre-merge-commit` hook, `--hash-diff` subcommand, session binding,
session-driven refresh triggers, and the jump-panel unreviewed badge (it was
keyed by session cwd and so shared rev 1's flaw).

## Consequences

- No merge enforcement anywhere; merging is Scott's manual act.
- The GUI is the review file's only writer; agents are told not to edit it.
  Concurrent GUI instances could race — acceptable (single-user, atomic writes).
- Rev-1 review marks in `.git/yalda-review/` are abandoned, not migrated (hunk
  hashes don't map to file hashes).
- The jump panel loses the unreviewed count; a worktree-aware badge can return
  later if wanted.
