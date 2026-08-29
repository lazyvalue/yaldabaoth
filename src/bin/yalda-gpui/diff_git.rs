//! Async git subprocess helper for the Diff Review tile.
//!
//! Scaffold stub — implemented by cog node `git-boundary` (ln8z).
//! Runs `git` for a worktree off the paint path and returns raw output
//! (merge-base, diff, status, untracked listing, worktree list, branch).
//! See docs/specs/spec-diff-review.md § Interfaces / Constraint C1.
#![allow(dead_code)]

use super::*;
