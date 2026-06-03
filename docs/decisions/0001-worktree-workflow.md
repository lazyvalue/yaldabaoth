# ADR-0001: Worktrees under .claude/worktrees as the default for substantial work

**Status:** Accepted
**Date:** 2026-06-02
**Related:** dev-system.md, CLAUDE.md

## Context

The project grew to multiple concurrent workstreams (ACP, rail, perf,
workspaces) run partly by parallel subagents. Doing this in the main checkout
meant a muddy working dir and collision risk. Sibling worktrees had already
sprung up cluttering `~/ws/` (`sketch-agent-beauty`, `sketch-bugfix`), and the
harness already used `.claude/worktrees/` for its own agent isolation.

## Decision

Substantial / multi-file / agent-run work happens in a git worktree under
`./.claude/worktrees/<slug>` on its own branch. `.claude/worktrees/` is
gitignored. Trivial one-file edits and conversational answers don't need one.

## Rationale

Keeps the main checkout clean, isolates parallel work so it can't collide, and
reuses the directory the harness already uses — no new clutter in `~/ws/`.

## Alternatives rejected

- **Sibling worktrees in `~/ws/`** — clutters the workspace dir (the thing we're avoiding).
- **One shared tree, branch-switching** — serializes work and loses uncommitted state on switch.

## Consequences

- A worktree branches from `HEAD`, so **uncommitted work is invisible to a new
  worktree**. Commit a stable *base* first (we did: `f282130`) before spinning
  up worktrees that need that work.
- Skills must live in the real `.claude/` (not a worktree) to be usable in the
  main session — so dev-system infra is authored in the main tree, not isolated.
- Convergence is now a separate step (see ADR-0004 and `/integrate`).
