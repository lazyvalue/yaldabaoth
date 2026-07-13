# Component: Rail

**Status:** living
**Component token:** `Rail` (⇒ `UXI-Rail-N`)

## Description

A persistent **per-tab** side column (distinct from the root-level jump panel). Its
kinds: the **file-browser rail** (`Cmd-B` / `ToggleFileBrowserRail`) and the
**outline rail** (`ToggleOutlineRail`). Features: side flip (`FlipRailSide`);
rail-focused navigation (`RailDown` / `RailUp` / `RailSelect` / `RailClose` /
`RailParent`), hidden-file toggle, sort cycle, worktrees, and a filter input (under
the `RailView` key context). Primary code home: `chrome.rs`.

## References

- `docs/specs/spec-rail.md` — the rail's design and behavior.

## UX invariants

_(none migrated yet — add via /new-ux as behavior is specified.)_
