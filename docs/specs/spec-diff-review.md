# Diff Review Tile (`App::Diff`)

**Status:** ACTIVE (implemented — see `docs/components/diff.md` for the shipped
`UXI-Diff-N` invariants)
**Last updated:** 2026-08-29

## Builds On

- **`spec-tiles-and-apps.md` / ADR-0019** — Tiles hold exactly one App. WHY:
  `App::Diff` is a new peer App variant (precedent: `App::Linear`); HOW: this
  spec adds the variant and its tile payload without touching the split tree,
  which stays generic over the content type (C1 there).
- **`spec-agent-session-ownership.md`** — sessions are store-owned with an
  immutable spawn `cwd`. WHY: a Diff tile's primary binding is *a session*, and
  the session's `cwd` is the ground truth for which worktree to diff; HOW: the
  tile holds a `SessionId` reference and reads session state via the store; it
  never owns session state.
- **`spec-turn-steering.md`** — mid-turn prompt delivery via `promptQueueing`.
  WHY: review comments are feedback to the *authoring* agent; HOW: a hunk
  comment is submitted through the existing `send_prompt_to_session` path, so it
  steers mid-turn and prompts normally when idle. No new transport.
- **`spec-yux.md` + `yux/CLAUDE.md`** — cached-view rules. WHY: the diff body is
  an expensive, mostly-stable surface; HOW: it renders as its own cached child
  entity (`cached_child`), invalidated at mutation sites, with a render-count
  test.
- **`docs/components/jump-panel.md`** — the session-list navigator and its
  unread badge model. WHY: "N unreviewed hunks" is the review analogue of
  unread; HOW: the jump panel reads a per-session unreviewed count projected
  from the tile's review model.

## Overview

**Problem.** Agents author code in worktrees faster than Scott reviews it.
There is no surface to see a branch's cumulative changes, no low-friction way to
send feedback about a specific change back to the authoring agent, and nothing
stops an unreviewed branch from merging.

**`App::Diff`** is a read-only review App. Like lazygit, it owns no git logic:
it shells out to `git` and parses unified-diff output. Named entities:

- **`DiffTile`** — the tile payload: an optional **`DiffSource`** binding plus
  view state (focus, collapse, comment compose).
- **`DiffSource`** — what to diff: `Session(SessionId)` (worktree derived from
  the session's `cwd`) or `Path(PathBuf)` (an explicit worktree, no session).
- **`DiffModel`** — the parsed diff: base/head SHAs, dirty flag, files, hunks.
  A **hunk** carries a content hash (`hunk_hash`) that is its review identity.
- **`ReviewState`** — the per-repo persisted record of reviewed hunk hashes,
  keyed by branch. Sidecar file, not in git history.
- **Merge gate** — two layers: the tile's merge command, and a
  `pre-merge-commit` git hook that reads `ReviewState`.
- **Comment→steering** — hunk-anchored feedback sent into the bound session.
- **Open-in-Zed** — `zed <path>:<line>` escape hatch for deep exploration.

The diff shown is always **merge-base(base, HEAD) → working tree**: committed
and uncommitted changes both appear, because agents edit long before they
commit. Base defaults to the repo's default branch.

## Behaviors

- **B1. Binding. [DRAFT]** An unbound `DiffTile` renders a selector (sessions
  whose `cwd` is inside a git repo, plus "pick a path"). Binding to a session
  derives the worktree from the session's `cwd` at bind time. Closing the
  session does not close the tile; the tile falls back to `Path` binding on the
  same worktree (the diff outlives the conversation). A deleted worktree renders
  an inline error state, never a panic.

- **B2. Cumulative diff view. [DRAFT]** The tile shows a file list (with
  add/remove counts) and, per file, hunk blocks in monospace with add/remove
  line coloring. No syntax highlighting in v1. Untracked files appear as
  all-added hunks via a non-mutating listing (`git ls-files --others` +
  `git diff --no-index /dev/null <file>`); the tile never writes the worktree's
  index (no `git add -N`). Navigation is vim-style: j/k move hunk focus,
  file-level collapse/expand, `[`/`]` jump files. Text zoom (INV-UX-13 pattern)
  scales the diff body; chrome stays fixed.

- **B3. Refresh. [DRAFT]** The diff re-derives by re-running git: (a) when a
  bound session's turn completes, (b) debounced after any tool-call completion
  in the bound session that reports file changes, (c) on manual refresh (`r`).
  Git runs asynchronously off the paint path; the tile shows the previous model
  until the new one lands. Hunk focus survives refresh when the focused hunk's
  hash still exists; otherwise focus moves to the nearest hunk.

- **B4. Comment → steering. [DRAFT]** With a hunk focused, one keypress (`c`)
  opens a comment compose (the standard Compose primitive, pinned in the tile).
  Submit sends to the bound session via `send_prompt_to_session` with the
  comment text prefixed by machine-readable context: repo-relative path, new
  line range, and the hunk patch text. Mid-turn it steers (per
  spec-turn-steering); idle it prompts. On send failure the draft stays in the
  compose (no silent drop). A `Path`-bound tile with no session cannot comment;
  the affordance is absent, not erroring.

- **B5. Review marks. [DRAFT]** A keypress (`v`) toggles the focused hunk
  reviewed. Reviewed-ness is keyed by `hunk_hash` (hash of the hunk's
  repo-relative path + content lines, excluding `@@` positions). Any edit that
  changes a hunk's content changes its hash, so it reverts to unreviewed
  automatically — staleness needs no timestamps or SHA comparison. File-level
  "mark all" exists. Marks persist in `ReviewState` across restarts.

- **B6. Unreviewed badge. [DRAFT]** The jump panel shows the count of
  unreviewed hunks next to a session whose worktree has any, rendered alongside
  (not replacing) the unread badge when both are present. The count lives in a
  root-owned worktree-keyed projection (`DiffProjections`, Data Model) updated
  whenever any `DiffModel` derives, so it survives tile close and is shared by
  multiple tiles on one worktree. It is a projection of derived state, not a
  background scan: a worktree never opened in a Diff tile shows no count, and a
  count goes stale-frozen once nothing refreshes that worktree.

- **B7. Merge gate. [DRAFT]** The tile's `merge` command merges the branch into
  base only when every hunk in the current `DiffModel` is reviewed and the
  **feature worktree** is clean; otherwise it refuses with the unreviewed
  count. The merge executes in the primary checkout (`git -C <primary> merge
  --no-ff <branch>`); it refuses if the primary checkout is dirty, and on
  conflict it runs `git merge --abort` and reports — the tile never leaves
  conflict markers in a live checkout. Independently, an installable git-hook
  layer (installed per-repo by an explicit tile command, never automatically)
  recomputes the same predicate from `ReviewState` + `git diff` and aborts the
  merge — catching merges by agents or at the CLI. Because `pre-merge-commit`
  does not fire on fast-forward merges, the installer also sets
  `git config merge.ff false`, and installs a `pre-commit` fragment that runs
  the same check when `MERGE_HEAD` exists (covering conflicted merges committed
  via `git commit`). Residual holes are documented, not defended: `--no-verify`
  bypasses hooks; the gate is defense-in-depth, not access control. Repos
  without the hook simply lack the second layer.

- **B8. Open in Zed. [DRAFT]** A keypress (`o`) on a focused hunk spawns
  `zed <abs-path>:<first-new-line>`. Fire-and-forget; a missing `zed` binary
  surfaces a status hint.

- **B9. Leaders. [DRAFT]** The tile implements `leader_intercept` (space = tile
  verbs: bind, refresh, merge, install hook; `.` = shell verbs) per the
  universal-leaders contract. The comment compose is the only insert-mode
  surface in the tile.

## Data Model

```rust
enum DiffSource { Session(SessionId), Path(PathBuf) }

struct DiffTile {
    source: Option<DiffSource>,      // None ⇒ selector
    model: Option<DiffModel>,        // last derived diff (kept during refresh)
    focus: DiffFocus,                // file index + hunk index
    collapsed: HashSet<PathBuf>,
    compose: Option<Compose>,        // open comment compose, hunk-anchored
    refreshing: bool,
}

struct DiffModel {
    worktree: PathBuf,
    branch: String,
    base: String,                    // e.g. "main"
    merge_base: String,              // SHA
    dirty: bool,
    files: Vec<FileDiff>,            // path, status, Vec<Hunk>
}

struct Hunk {
    header: String,                  // "@@ -a,b +c,d @@"
    lines: Vec<DiffLine>,            // Context | Added | Removed
    hunk_hash: u64,                  // hash(path + occurrence index + content lines)
    reviewed: bool,                  // joined from ReviewState at derive time
}
```

`hunk_hash` excludes `@@` positions (stable across unrelated edits) but is
salted with the hunk's per-file occurrence index, so two identical hunks in one
file review independently.

**`ReviewState`** persists in the repo's **git common dir**
(`$(git rev-parse --git-common-dir)/yalda-review/<branch>.json`, one file per
branch): `{ reviewed_hashes: [u64] }`. The common dir is shared by the primary
checkout and every linked worktree, so marks written while reviewing the
feature worktree are visible to a hook running a merge in the primary checkout
— and it is never inside the tracked tree. Hashes of hunks that no longer exist
are garbage-collected on write. Under `cfg(test)` the location takes the
standard `*_PATH_OVERRIDE` seam.

**`DiffProjections`** — root-owned map `worktree → unreviewed_count`, updated
at `DiffModel` derive time; read by the jump panel (B6). Not persisted.

Data ownership: `DiffTile` owns its view state and derived `DiffModel`; the
root view owns `DiffProjections`; `ReviewState` is owned by the diff subsystem
(read by the hook script as a plain JSON consumer). Session state stays in
`AgentSessions`; the tile only holds a `SessionId`.

**Persistence.** The workspace layout persists a Diff tile as its worktree path
only (a new `PersistedKind` arm in `persist.rs`). `SessionId` is runtime-local
and meaningless across restarts, so a session-bound tile restores as
`Path`-bound to the same worktree; re-binding to a session is a manual act.

## Interfaces

View methods on `YaldaGpuiView` (module-internal):

- `bind_diff_source(source, cx)` / selector flow — B1.
- `refresh_diff(cx)` — async git spawn → parse → swap `DiffModel`, notify. B3.
- `toggle_hunk_reviewed(cx)` / `mark_file_reviewed(cx)` — B5; writes
  `ReviewState`, notifies the jump panel projection.
- `submit_hunk_comment(cx)` — B4; delegates to `send_prompt_to_session`.
- `merge_reviewed_branch(cx)` / `install_merge_hook(cx)` — B7.
- `open_hunk_in_zed(cx)` — B8.

Subprocess boundary: one async helper runs
`git -C <worktree> diff <merge-base> --no-color` (+ `status`/`merge-base`
queries) and returns raw output; a pure parser (`diff_model.rs`) turns it into
`DiffModel`. The parser is the unit-testable core and never touches the
filesystem.

Hook: `scripts/yalda-pre-merge-hook` (shell), reads the branch's `ReviewState`
file from the git common dir, resolves the branch's worktree via
`git worktree list` to evaluate its dirty state, and recomputes hunk hashes via
the same normalization — exposed as a hidden `yalda-gpui --hash-diff`
subcommand so hook and GUI cannot drift. The installer bakes the resolved
absolute binary path into the hook script; if the binary is missing at merge
time the hook **fails closed** with an explanatory message. Exits non-zero on
any unreviewed hunk or dirty feature worktree.

Events/messages: none new — refresh triggers ride the existing session pump
(turn/tool-call completion observed in the reducer path).

## State Machine

Hunk review lifecycle (per hunk identity):

```
 unreviewed ──v──► reviewed ──(content edit ⇒ new hash)──► unreviewed
     ▲                │
     └──────v─────────┘        merge allowed ⇔ all current hunks reviewed
```

## Constraints

- **C1. No in-process git.** All diff/merge-base/status data comes from the git
  CLI; yalda parses, never computes. Parsing lives in a pure module.
- **C2. Paint-path purity.** Git subprocesses and `ReviewState` I/O never run
  on the render path; the diff body is a `cached_child` with a render-count
  test; no `cx.notify()` in render (yux rules).
- **C3. Read-only.** No staging, no `git apply`, no file editing from this
  tile. The only write surfaces are `ReviewState`, the hook installer, and the
  merge command.
- **C4. Session decoupling.** The tile references sessions by id only; every
  behavior except B4 works `Path`-bound with no session.
- **C5. Test hygiene.** Tests use tempdir git fixtures; never the user's repos
  or `~/.yalda`; `ReviewState` gets a path-override seam. Guard tests live in
  `verify_harness.rs` (parser unit tests beside `diff_model.rs`).
- **C6. Gate honesty.** The hook and the tile must evaluate the same predicate
  from the same normalization (shared via `--hash-diff`); a drift between them
  is a bug, not a fallback.

## Revision History

- 2026-08-29 — Initial DRAFT. Design settled conversationally: session-bound
  cumulative diff, hash-keyed review marks, comment→steering over the existing
  prompt path, two-layer merge gate, Zed escape hatch. Exploration (LSP, symbol
  nav, highlighting) deliberately out of scope — Zed covers it.
- 2026-08-29 — Adversarial-review pass (verdict RETHINK, both blocking items +
  all lesser items folded in): merge gate hardened (`merge.ff false` +
  `MERGE_HEAD` pre-commit fragment for the fast-forward / conflicted-merge
  holes, `--no-verify` documented as a hole); `ReviewState` moved from the
  worktree sidecar to the shared git common dir so hook and tile read the same
  file; merge execution site pinned to the primary checkout with abort-on-
  conflict; `DiffProjections` added as the owned home of the B6 badge count;
  restart persistence defined (worktree path, restore `Path`-bound);
  `hunk_hash` salted with occurrence index; `git add -N` dropped for a
  non-mutating untracked listing; hook binary path baked at install, fail
  closed.
- 2026-08-29 — Implemented via Cog graph `ec3` (13 nodes). Status DRAFT→ACTIVE.
  Behaviors B1–B9 + Data Model + C1–C6 shipped on `main` and reconciled to
  `docs/components/diff.md` (`UXI-Diff-1..9`). Deviations from the draft:
  - **`DiffView` observes the root entity**, not a separate model entity —
    `DiffTile` owns `model`/`focus`/`collapsed` inline (unlike Linear/Cog), so
    the cached body reads the tile back through the root and self-notifies on a
    `DiffSeqs` change; text-zoom rides the same fingerprint (no separate notify
    walk).
  - **Persist arm** stores `tile.worktree()` (cx-free) rather than resolving a
    live session `cwd` at snapshot time; a `Session`-bound tile's derived
    worktree *is* that session's cwd at derive time.
  - **Comment compose renders at the screen level** (uncached, like the agent
    compose), so typing never touches the cached `DiffView` — no new `DiffSeqs`
    input was needed.
  - **Jump panel has no per-row unread glyph today** (`docs/components/jump-panel.md`
    — unread only tints the workspace-folder aggregate), so B6 "alongside the
    unread badge" is realized as the unreviewed chip painting while the row's
    `unread` flag is independently true.
  - **Open gesture added** (`OpenDiff`, global `cmd-*` + menu) so an unbound
    tile (the selector) is reachable from the GUI — the draft named no open key.
  - **`--no-verify`** documented in-code as the residual gate hole (defense in
    depth, not access control).
