# ADR-0034 — Tile attachment is independent of visibility

**Status:** accepted
**Date:** 2026-08-19
**Supersedes:** ADR-0033's Bound/Unbound vocabulary and direct-Unbound-only
presentation model; `UXI-Workspace-18` (Scratchpad); `UXI-Workspace-23`
(Close-as-Stash).

## Context

ADR-0033 correctly made a tile a stable shell whose workspace ownership could
change without recreating its App. It conflated two independent questions,
however:

1. Is the tile attached to a workspace?
2. Is an attached tile visible in that workspace's current arrangement?

The scratchpad modeled “hidden” as an MRU subset of Unbound. Consequently Hide
silently detached a tile, Close silently stashed some Agent tiles, and the jump
surfaces could not say “this tile belongs to workspace X but is currently
hidden.” Those are different operations and must not share a transition.

## Decision

### 1. Placement has three legal states

Every stable tile is in exactly one state:

- **Attached + visible** — owned by exactly one workspace and present in its
  active layout.
- **Attached + hidden** — owned by exactly one workspace but absent from its
  active layout.
- **Detached** — owned by the frame's Detached collection and associated with
  no workspace.

There is no Detached + hidden state. Hidden is meaningful only relative to the
workspace that continues to own the tile.

Attach moves a Detached tile to Attached + visible. Detach moves either attached
state to Detached and clears hidden state. Hide moves Attached + visible to
Attached + hidden. Unhide moves Attached + hidden to Attached + visible, selects
the owning workspace, and focuses the tile. None of these transitions recreates
the tile, changes its `WindowId`, App state, project, tags, or Agent session.

### 2. Hidden placement is a restoration preference, not reserved geometry

Hiding records the tile's last complete plane footprint and ordering context.
The footprint is not a reservation: visible tiles may move, resize, change
arrangement, or occupy it while the tile is hidden.

On Unhide, the workspace first restores the saved footprint when it is still
valid and unoccupied. Otherwise the current layout manager inserts the tile by
its ordinary new-tile rules near the current focus. In Columns, the resulting
column order is the reading order of the restored or newly assigned plane
footprint. This matches ordinary i3/dwm expectations: remove and reinsert a
client without freezing the rest of the layout around a ghost slot.

### 3. Solo presentation is focus, not ownership or visibility

The frame may temporarily present exactly one tile alone when that tile is
Detached or Attached + hidden. Presentation is a typed navigation target, not a
third owner and not a visibility mutation.

Selecting a hidden tile in the jump panel or Cmd-P presents it alone. Leaving
that presentation keeps it hidden. Selecting a Detached tile likewise presents
it alone and keeps it Detached. Selecting an attached visible tile selects its
workspace and focuses it normally.

### 4. Workspace navigation includes hidden attachment

Each workspace folder in the jump panel contains every tile attached to that
workspace, both visible and hidden. Hidden rows are distinguishable but retain
ordinary tile metadata and workspace grouping. The separate **Detached** list
contains only Detached tiles and retains tag-folder organization.

A workspace is allowed to have no visible tiles when all of its tiles are
hidden. Its canvas renders an explicit all-tiles-hidden empty state; it does not
manufacture a replacement tile. The workspace and its hidden attachments remain
durable and navigable.

### 5. Close is independent

Close retires the focused tile shell. It never aliases Attach, Detach, Hide, or
Unhide. Closing a tile does not terminate an Agent session; Agent-session
lifecycle remains an explicit Agent command. If the universal session roster
subsequently requires a viewport for a live session with no tile, normal roster
reconciliation may materialize a new Detached Agent tile. That is a new shell,
not the closed tile changing placement.

Closing the last visible tile is permitted when the workspace still owns hidden
tiles. The durable one-workspace floor remains, but it no longer requires every
workspace to manufacture a visible leaf.

### 6. Compatibility migration is additive

Legacy Bound tiles load as Attached + visible. Legacy Unbound tiles load as
Detached. A legacy scratchpad id that names a legacy Unbound tile remains
Detached; scratchpad membership and MRU cycling are discarded because they
cannot infer a workspace attachment without inventing ownership. Direct-Unbound
focus migrates to a Detached presentation target. No migration duplicates a
tile, changes a tile's project, or changes an Agent session binding.

## Consequences

- “Attached” / “Detached” describe workspace association. “Visible” / “Hidden”
  describe only an attached tile's participation in its workspace layout.
- Workspace ownership validation covers visible leaves, hidden attached tiles,
  Detached tiles, and typed presentation references as one exclusive graph.
- Hidden tiles require workspace-owned storage plus best-effort placement
  metadata; they do not live in Detached and do not need a second MRU index.
- Attach, Detach, Hide, Unhide, and Close have separate commands and typed model
  transitions.

## Alternatives rejected

- **Keep Scratchpad as an alias for Hide.** This preserves the conflation: the
  tile leaves its workspace even though the user asked only to hide it.
- **Reserve a hidden tile's geometry with an invisible layout leaf.** This makes
  hidden tiles constrain visible rearrangement and turns a restoration
  preference into a ghost obstacle.
- **Store one boolean on every tile.** A Detached hidden tile becomes
  representable, and callers must manually keep ownership and the flag in sync.
