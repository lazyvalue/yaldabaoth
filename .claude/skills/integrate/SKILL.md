---
name: integrate
description: Combine feature branches/worktrees into one buildable integration branch — merge in dependency order, resolve conflicts, build + test, and flag behavior-changing branches for human runtime review before folding to master. Use when several parallel branches/worktrees are done and need to converge, or the user asks to integrate, combine, or merge the work.
---

# Integrate

Converge parallel work into one buildable branch. Parallel fan-out has a hidden
cost — convergence — and this is where it's paid. (Best avoided up front by
decomposing by file/module ownership, not by concern — see ADR-0004.)

## Process

1. **Inventory.** `git worktree list` + `git log --oneline -1` per branch. For
   each branch decide: behavior-**preserving** (perf, cleanup, refactor — safe to
   fold after a build check) or behavior-**changing** (new UX, new interaction,
   different output — needs human runtime review before folding).
2. **Pick a base + order.** Start from the branch that contains the most/lowest
   layer (e.g. a synthesis branch that already stacks others). Merge the rest in
   dependency order. Prefer the branch with the heaviest shared-file edits first
   so later merges conflict against settled code.
3. **Create the integration worktree.** `git worktree add .claude/worktrees/integration -b integration <base>`.
4. **Merge one at a time, building after each.** `git merge --no-edit <branch>`;
   resolve conflicts by hand (never leave markers); then
   `cargo build --bin sketch-gpui --bin sketch-session-server` + `cargo test`.
   Don't merge the next branch until the current one is green. A clean text-merge
   does NOT imply it compiles — always build.
5. **Report** per branch: merged cleanly / conflicts resolved (what), build +
   test result, and the behavior-preserving vs behavior-changing classification.
6. **Folding to master** is a separate, user-gated step. Behavior-preserving
   branches can fast-forward/merge after the build check; **hold behavior-
   changing ones for the user's runtime verification** — say so, don't auto-fold.

## After integrating

- `/worklog` to record what landed and what's still unverified.
- Offer cleanup: remove superseded branches + their worktrees
  (`git worktree remove <path>` then `git branch -D <branch>`).

## Constraints

- Build + test after every merge; report real command output, not claims.
- Never silently auto-fold behavior-changing work to master.
- Don't push. Don't fold to master/main without an explicit ask.
- Remember the verification gap: integration being green means it *builds and
  unit-tests*, not that it's runtime-correct. Flag what still needs a human run.
