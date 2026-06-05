# ADR-0007: Doc and Edit views share one rope (D2)

**Status:** Accepted
**Date:** 2026-06-05
**Related:** ADR-0005 (panel = view onto a shared Core), spec-state-architecture.md (D2), buffer_pool module

## Context

The Doc view (rendered) and Edit view (raw) of the *same file* each construct
their own `Document` (rope) — two independent ropes for one file, a
`duplicated-copy` hazard. `DocState.edit_cache` (a stashed parallel editor) is
the scar tissue from shuttling state between them.

## Decision

One `SharedCore` (rope + undo) per canonical file path; Doc and Edit are **views**
of that one core. **All views are live** — editing in one (e.g. an Edit pane)
updates the other (a Doc pane) immediately — and **undo is unified** per file.
`edit_cache` is retired. The one observable behavior change (Doc tracks live Edit
edits) is gated behind this decision and lands as migration step 5c, *after* the
no-behavior-change parts (5a buffer_pool, 5b blocks auto-derived on `edit_seq`).

## Rationale

This is ADR-0005's "panel is a view onto a refcounted shared Core" applied to the
Doc/Edit case. It deletes a duplicated-copy and lets us retire the stash. Live
re-render cost is O(changed) because blocks are memoized on `edit_seq` (quick
win #2), so the live path is cheap. Rejected: a "frozen preview until reload"
semantic — not desired; the rendered view tracking edits is what users expect.

## Consequences

- 5a/5b land first (no behavior change, independently verifiable); 5c flips Doc
  onto the shared core.
- Undo becomes one history per file, shared across its views.
