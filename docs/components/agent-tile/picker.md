# Agent Tile — Session picker

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-32`,
`UXI-AgentTile-39`.

## Description

An unbound Agent Tile renders a session picker. It always offers explicit New
Claude and New Codex rows, followed by existing sessions in the tile's project:
free sessions are selectable, while sessions already bound to another tile are
shown read-only as in use. Free sessions are organized into tag folders using the
same sid-keyed tag data the jump panel shows (`UXI-AgentTile-39`).

The Agent local menu exposes **send to workspace**. It opens the workspace
destination picker for the focused Agent tile whether that tile is currently
bound or unbound; choosing a same-project workspace moves the stable tile and
follows it.

## References

- `docs/components/README.md` § Terminology — a session with no durable
  workspace-tile reference is **free**.
- `docs/components/jump-panel.md` — `UXI-JumpPanel-16` owns the durable archived
  visibility flag and the Archived tab.
- `docs/specs/spec-agent-session-ownership.md` — the 1:1 binding invariant.
- Code: `agent_ui.rs::picker_projection`,
  `screens.rs::render_agent_picker`, and
  `agent_ui.rs::{agent_picker_move, agent_picker_activate}`.
- `docs/components/jump-panel.md` — `session_tags` (sid → `[tag]`) and the jump
  panel's own tag-folder grouping (`partition_rows_by_tag`), the source of the tag
  data this picker reuses.

## UX invariants

### UXI-AgentTile-32 — Archived sessions never appear in the session picker

**Statement.** The Agent Tile session picker does not show an archived session,
whether that session is free or already bound to another tile. Archiving is a
visibility boundary for the entire existing-session portion of the picker. The
two create-new rows remain present, and unarchived sessions retain the existing
project scoping, free/selectable, and bound/read-only behavior.

Archiving a bound session also moves its workspace tile to this picker
immediately (`UXI-JumpPanel-16`). The archived session is still intentionally
reachable by selecting it from the jump panel's Archived tab; that explicit
direct visit opens its transcript read-only in a bare ephemeral view and does
not route through or add the session to this picker. Unarchive is the explicit
transition that recreates its ACP transport.

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

### UXI-AgentTile-39 — The session picker groups free sessions into tag folders

**Statement.** In the Agent Tile session picker, the free (selectable) existing
sessions are grouped into tag folders. A session's tags are the ones the jump
panel shows for it (the sid-keyed `YaldaGpuiView::session_tags` map). Each free
session appears exactly once, placed under one group: the alphabetically-first of
its tags, or the untagged group when it carries no tag. Tag folders are ordered
alphabetically by tag; the untagged group is last. Each folder shows a
non-interactive header (the tag name, or `UNTAGGED`) above its sessions; a header
is a visual row only — it is never selectable and never consumes a navigation or
activation index. When no free session carries any tag, no folder headers are
rendered and the flat single-list layout is unchanged.

The two create-new rows (`New Claude` at index 0, `New Codex` at index 1), the
project scoping, and the read-only IN USE block are unchanged. The free-session
activation and navigation indices (`2..=N+1`) continue to map to the free-session
list in its rendered order, so grouping reorders that list but preserves the
one-to-one row↔session mapping every interaction path depends on.

**Applies to.** The shared free-list ordering in
`agent_ui.rs::picker_projection` (which now returns free sessions in grouped
order); the header emission in `screens.rs::render_agent_picker`; the index math
in `agent_ui.rs::{agent_picker_move, agent_picker_activate}`, which read the same
projection order; the sid-keyed tag source `YaldaGpuiView::session_tags`.

**Why.** A session's tag is the primary way the user thinks about which agent is
which (the jump panel already folds by tag). A flat, label-sorted picker forces
the user to re-scan for the tagged group they want; grouping the picker by the
same tags removes that mismatch and makes the two surfaces consistent.

**Status.** `implemented` (headless, at the shared projection order consumed by
render, navigation, and activation).

**Enforcement.**
`verify_harness.rs::session_picker_groups_free_sessions_by_tag` at the real
`picker_projection` seam plus a rendered frame. It seeds free roster sessions
across two tags and one untagged session, sets `session_tags`, and asserts the
projected free order is grouped by tag (alphabetical, untagged last) so that
`agent_picker_activate` row indices still resolve to the intended session.
Negative control observed RED with the grouping sort removed: the free order fell
back to plain label order and the grouped-order assertion failed.

**Deviation from plan.** Sessions with multiple tags are filed under a single
group (their alphabetically-first tag), not duplicated under every tag as the
jump panel does. Duplication is impossible here because the picker's activation
and navigation math require each free session to occupy exactly one selectable
index. Grouping order is derived (alphabetical) rather than reusing the jump
panel's manual `jump_tag_order`, keeping the picker projection free of jump-panel
UI state.
