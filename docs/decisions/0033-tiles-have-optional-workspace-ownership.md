# ADR-0033 — Tiles have optional workspace ownership

**Status:** superseded by ADR-0034
**Date:** 2026-08-18
**Supersedes:** ADR-0021 (ephemeral virtual workspaces), the free-session
placement model in `spec-agent-session-ownership.md`, and the words “free
session” / “bare agent view” in the component specs.

## Context

Yalda currently puts every tile in a workspace layout. Agent sessions are the
exception: the normalized session store can keep a session alive without a tile,
and direct navigation fabricates an ephemeral one-tile workspace as a temporary
viewport. That creates three competing concepts:

- a durable workspace tile;
- a “free” session with no durable tile;
- an ephemeral workspace/tile pair used only to view a session.

The exception leaks into persistence, workspace numbering, project navigation,
close behavior, Cmd-P, and the jump panel. It also makes a workspace look like
the owner of application state even though users expect a tile to keep its state
when it leaves a workspace.

## Decision (historical)

### 1. A tile is the durable shell object

`WindowId` identifies a tile. A tile owns one `App`, its project, and its tag
set. Moving a tile never recreates its `App`, changes its `WindowId`, or drops
its tags.

### 2. Workspace ownership is optional and exclusive

The frame partitions all tiles into exactly two ownership domains:

- **Bound** — the tile is a leaf in exactly one durable workspace layout.
- **Unbound** — the tile is in the frame's unbound collection and in no
  workspace.

A tile cannot be in both domains or neither. Binding and unbinding move the same
tile value between them. Binding is project-local: a tile may bind only to a
workspace in its project.

### 3. Direct view is focus, not ownership

The frame may directly focus one unbound tile. This temporarily replaces the
workspace canvas in the content area, but it does not create a workspace, insert
the tile into a layout, or change membership. Selecting any workspace clears
direct-unbound focus and returns to ordinary workspace rendering.

Ephemeral virtual workspaces and their origin bookkeeping are removed.

### 4. Tags belong to tiles

Tags are tile metadata and survive bind, unbind, direct view, restart, and moves
between same-project workspaces. The unbound list groups tiles by these tags in
the same order and folder UI used by the former session list.

Migration seeds tile tags from existing durable metadata:

- an Agent tile inherits the tags stored for its server session id;
- a Buffer tile inherits the tags stored for its canonical file buffer.

After migration the tile is authoritative. Two tiles viewing the same underlying
session or file may be tagged independently.

### 5. Navigation projects ownership

The jump panel renders each workspace as a collapsible folder containing its
bound tiles. A separate **Unbound** list contains only unbound tiles and retains
the existing tag folders and ordering. Cmd-P (“Jump to…”) is the keyboard
projection of the same destinations:

- a bound-tile result selects its workspace and focuses the tile;
- an unbound-tile result directly focuses the tile without binding it.

The period shell menu is not an unbound-tile picker.

### 6. Session lifetime remains independent

An Agent tile refers to normalized project-owned session state. Unbinding or
closing a tile never kills its session; killing a session remains an explicit
Agent command. Server-roster sessions that have no bound tile are materialized
as unbound Agent tiles so there is exactly one navigation object and one place
for tile metadata.

### 7. Compatibility migration is additive

Old `workspace.json` snapshots have no unbound collection or tile-level tags.
They load every persisted leaf as bound, import existing session/buffer tags,
materialize roster-only sessions as unbound Agent tiles, and write the new shape
on the next save. No old leaf is duplicated into Unbound.

## Consequences

- Closing a workspace unbinds its tiles instead of deleting them. It remains a
  no-op for the sole durable workspace.
- “Unbound Agent tile” no longer means “picker with no session.” That state is
  called an **empty Agent tile**. “Unbound” describes workspace membership only.
- Workspace-number shortcuts still address durable workspaces. Direct-unbound
  focus has no workspace number.
- Layout code continues to own bound placement geometry. The frame owns
  unbound ordering and direct focus.

## Supersession

ADR-0034 separates workspace attachment from workspace visibility. Its
**Attached / Detached** vocabulary replaces **Bound / Unbound**, and its hidden
attached state replaces the scratchpad-as-Unbound model added after this ADR.
This document remains the historical record for stable tile identity, exclusive
ownership, project-local attachment, and the removal of ephemeral workspaces.

## Alternatives rejected

- **Keep sessions free and fabricate direct viewports.** This preserves the
  exception and cannot retain generic tile state outside a workspace.
- **Treat unbound as a hidden workspace.** That makes binding a layout-to-layout
  move and reintroduces fake workspace semantics, numbering, and persistence
  filters.
- **Centralize every tile in a map and make layouts store ids only.** Clean in
  isolation, but it expands this refactor across every content borrow and render
  path. Moving complete `Window<App>` values between the two owners enforces
  the same invariant with a smaller migration.
