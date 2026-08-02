# Agent Tile — Session picker

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-32`.

## Description

An unbound Agent Tile renders a session picker. It always offers explicit New
Claude and New Codex rows, followed by existing sessions in the tile's project:
free sessions are selectable, while sessions already bound to another tile are
shown read-only as in use.

## References

- `docs/components/README.md` § Terminology — a session with no durable
  workspace-tile reference is **free**.
- `docs/components/jump-panel.md` — `UXI-JumpPanel-16` owns the durable archived
  visibility flag and the Archived tab.
- `docs/specs/spec-agent-session-ownership.md` — the 1:1 binding invariant.
- Code: `agent_ui.rs::picker_projection`,
  `screens.rs::render_agent_picker`, and
  `agent_ui.rs::{agent_picker_move, agent_picker_activate}`.

## UX invariants

### UXI-AgentTile-32 — Archived sessions never appear in the session picker

**Statement.** The Agent Tile session picker does not show an archived session,
whether that session is free or already bound to another tile. Archiving is a
visibility boundary for the entire existing-session portion of the picker. The
two create-new rows remain present, and unarchived sessions retain the existing
project scoping, free/selectable, and bound/read-only behavior.

**Applies to.** The shared picker projection in
`agent_ui.rs::picker_projection`; its consumers
`screens.rs::render_agent_picker`,
`agent_ui.rs::agent_picker_move`, and
`agent_ui.rs::agent_picker_activate`; the durable sid-keyed archive set
`YaldaGpuiView::jump_archived_sessions`.

**Why.** Archived sessions are deliberately removed from ordinary navigation
surfaces. Showing one in an unbound tile's picker makes it look active and
attachable, bypassing the user's archive choice.

**Status.** `implemented` (headless, at the shared projection consumed by every
picker interaction path).

**Enforcement.**
`verify_harness.rs::agent_tile_picker_excludes_free_and_bound_archived_sessions`
at the real shared projection seam. It builds two bound tiles plus a third,
focused unbound picker, seeds equivalent archived/unarchived free and bound
roster sessions, then proves only the unarchived identities reach the picker's
selectable and in-use lists. Negative control observed RED with the archive
guard absent: the archived free identity reappeared in the selectable list.

**Deviation from plan.** None.
