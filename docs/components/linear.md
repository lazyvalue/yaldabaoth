# Component: Linear

**Status:** living
**Component token:** `Linear` (⇒ `UXI-Linear-N`)

## Description

`App::Linear(LinearTile)` — a tile that views Linear issues and projects by tag via
the Linear GraphQL API (opened with `Cmd-L`). It presents an issue/project list and
a cached detail body, built on **yux** (the reusable UX component layer). It reads
`LINEAR_API_KEY` from the environment. Primary code home: `linear.rs` (GraphQL client
+ data model), `linear_ui.rs` (view-layer methods), `linear_view.rs` (the cached body
component).

## References

- `docs/components/common/text-editing.md` — text surfaces obey `TextEditing` where applicable.
- `yux/CLAUDE.md` — the cached-view component layer the detail body is built on.

## UX invariants

_(none migrated yet — add via /new-ux as behavior is specified.)_
