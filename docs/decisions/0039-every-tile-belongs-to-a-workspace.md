# ADR-0039 — Every tile belongs to a workspace

**Status:** accepted
**Date:** 2026-09-27
**Supersedes:** ADR-0034's **Detached** placement state (and, through it,
ADR-0033's Unbound collection and roster materialization). ADR-0034's
Attached-visible / Attached-hidden split, typed solo presentation of hidden
tiles, and independent Close remain in force.

## Context

ADR-0033/0034 made a tile the durable shell and let it live outside every
workspace (“Detached”). To give every server session exactly one navigation
object, roster reconciliation then minted a Detached Agent tile for every
session with no tile. In practice this produced a second, parallel navigator:
on 2026-09-27 Scott's saved frame held **56 Detached tiles** for 57 server
sessions, 52 of them archived — almost all invisible, yet all persisted,
deduplicated, healed, and tag-folded. The Detached list, attach/detach
commands, duplicate-identity repair, and solo presentation of Detached tiles
exist only to support that state.

Scott: “Let's simplify the UX substantially. Remove the notion of detached
tiles. We're just going to have workspaces. Every tile belongs to one
workspace.”

## Decision

### 1. A tile exists only inside a workspace

Every tile is owned by exactly one workspace, visible or hidden. There is no
frame-level tile collection. `TileMembership` is Attached-only;
`SoloPresentation` names only hidden attached tiles.

### 2. Sessions without a tile get no tile (option A)

The server roster is not a tile source. A server session no tile shows stays a
server session: reachable from Cmd-P and from an Agent tile's session selector.
Opening one places a **new visible tile** in a workspace of the session's
project — the active workspace when it belongs to that project, else that
project's first workspace, else a new workspace for the project — and focuses
it. The jump panel lists workspaces and their tiles only.

Option B (materialize each live session as a hidden tile in its project's
workspace) was rejected: it keeps roster materialization and duplicate repair
alive and turns workspace folders into session lists.

### 3. Transitions

- **Detach / Attach are removed** (`ctrl-w shift-b` / `ctrl-w b`, tile menu
  “detach tile”). Send-to-workspace and Hide/Unhide remain.
- **Close workspace retires its tiles** (visible and hidden). Agent sessions
  keep running on the server; the one-workspace floor remains.
- **Archive** returns every tile showing the session to its session picker;
  it never moves the tile.
- **New tile while a hidden tile is solo-presented** lands in that tile's
  owning workspace.

### 4. Migration

`workspace.json` `detached_tiles` (and legacy `unbound_tiles`,
`direct_unbound`, Detached solo presentation) are ignored on load and never
written. Nothing is lost: an Agent tile only referenced a server session, which
remains reachable; Detached Buffer tiles referenced files still on disk.

### 5. Future multi-membership

“Owned by exactly one workspace” is the current rule, not a structural
promise. Nothing here should make a later tile-in-several-workspaces model
harder than today's single-owner validator.

## Consequences

- Removed: `DetachedTile`, `Frame::detached_tiles`, roster materialization,
  Detached duplicate-identity repair, the jump panel's Detached section (and its
  tag folders and ordering preference), the attach/detach actions.
- Tile tags remain tile metadata but no longer drive a sidebar folder view.
- The session selector and Cmd-P become the paths to a session with no tile.

## Alternatives rejected

- **Option B — hidden tiles per session.** See §2.
- **Collapse to a single workspace.** Not requested; multiple workspaces stay.
