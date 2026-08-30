//! Per-branch reviewed-hash persistence for the Diff Review tile.
//!
//! Scaffold stub — implemented by cog node `review-state` (m7xl).
//! Stores `{ reviewed_hashes: [u64] }` at
//! `$(git rev-parse --git-common-dir)/yalda-review/<branch>.json`, joins
//! reviewed flags into a `DiffModel`, GCs dead hashes on write, and takes a
//! `*_PATH_OVERRIDE` seam under `cfg(test)`.
//! See docs/specs/spec-diff-review.md § Data Model / C5.
#![allow(dead_code)]

use super::*;
