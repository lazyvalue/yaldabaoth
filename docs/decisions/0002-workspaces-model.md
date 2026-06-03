# ADR-0002: Workspaces model (renamed tabs, move-default, doc-only also-show)

**Status:** Accepted
**Date:** 2026-06-02
**Related:** spec-workspaces-tagging.md, spec-tabs-and-splits.md, ADR-0005

## Context

We wanted dwm-style "tag panels into desktops, a panel can live in several." A
panel is three fused things — content, per-view state (cursor/scroll), and a
slot in one layout tree — and only the *content* is shareable. Research
(`spec-workspaces-tagging.md`) showed every editor that does this (vim
tabpages, perspective.el, VS Code groups) shares the document by reference and
gives each space its own geometry.

## Decision

- **Workspace = today's `Tab`, renamed (user-facing strings only).** No new
  container layer. `Tab<C>` type kept internally (the container type is already
  `Workspace<C>` — name collision avoided by not renaming the type yet).
- **"Send panel elsewhere" defaults to MOVE** (leaves the source); **also-show**
  is a separate explicit verb (`Ctrl-W M`).
- **Close = remove this view**; destroying content stays the existing close/
  refcount behavior.
- **Multi-home indicator** (a dot) when a doc is shown in >1 workspace.
- **No union views** (showing two workspaces at once).
- **v1: only documents are multi-home.** Agents/browser are single-home (one
  subprocess); also-show on them shows a toast.

## Rationale

Matches the proven prior art; reuses ~all of `workspace.rs` unchanged. Move-
default prevents accidental multi-homing. Union views have no clean tree-merge
for hand-arranged layouts and no editor offers them — dropped. The literal "tag
the *panel*" idea (same layout node in two trees) needs `Rc<RefCell>` graphs and
an unanswerable "is the cursor shared?" — rejected; tag the *content* instead.

## Alternatives rejected

- **Shared layout nodes** (same `Window` leaf in two trees) — graph data model, cursor ambiguity.
- **Workspaces-of-tabs (2-level)** — muddy mental model (which level owns focus/rail).
- **Full `Tab`→`Workspace` type rename** — large mechanical churn + name collision; deferred to strings-only.

## Consequences

- Also-show for docs needs the shared buffer pool to share *unsaved* edits;
  the pool is unwired, so v1 reads from disk like splits (see ADR-0005).
- Agent multi-membership is deferred; it needs the session Core/View split
  (ADR-0005) and is `NEEDS-DECISION` in the backlog.
