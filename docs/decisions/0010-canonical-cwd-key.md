# ADR-0010: Canonical cwd key + lazy fallback-read (D5)

**Status:** Accepted
**Date:** 2026-06-05
**Related:** spec-state-architecture.md (D5), persistence module

## Context

App-side persistence and the resume-match filter key on the cwd *string*, with
three different spellings in play (`current_dir()`, `process_cwd()`, raw
`cwd.display()`). When the launch dir isn't byte-identical to the saved one —
symlinks, trailing slash, the macOS `/tmp` → `/private/tmp` symlink — resume
silently misses and you get the "session turned into a new empty one" symptom.

## Decision

One **canonical cwd-key function** used everywhere on-disk: canonicalize both
sides, fall back to raw-vs-raw if `canonicalize` fails (so a deleted path doesn't
regress to never-matching). For the transition (existing entries are under the
old key), **lazy fallback-read**: look up the canonical key; on a miss, try the
old raw key once and adopt it; the next save rewrites it canonical. No migration
pass, no silent drop.

## Rationale

Fixes the silent resume-miss; contained to the `persistence` module. Lazy
fallback makes the transition invisible at ~3 lines. A full migration pass is
over-engineering for a one-time single-user transition; silently dropping stale
entries would reintroduce the very "sessions vanished" symptom we're killing.

## Consequences

- Part of the `persistence` module work (migration step 4).
- Already applied to the resume *filter* (`cwd_match_key`); this generalizes it
  to the on-disk keys.
