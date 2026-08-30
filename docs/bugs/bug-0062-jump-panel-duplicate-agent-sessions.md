# bug-0062: jump-panel-duplicate-agent-sessions

**Status:** FIXED  
**First seen:** 2026-08-30  
**Component:** `docs/components/jump-panel.md` (`UXI-JumpPanel-25`)  
**Cog:** `1z0`

## Symptom

The jump panel showed multiple entries for the same durable Agent session. Two
copies could appear together under Detached, or one under its workspace and one
under Detached. Activating either copy opened the same conversation.

## Root cause

The live frame could contain more than one stable Agent tile remembering the same
server sid. `materialize_roster_detached_tiles` stopped as soon as
`agent_tile_id_for_server_sid` found any owner. That prevented creation of a new
copy but did not reconcile copies already admitted by an earlier race or stale
live state. The tile-native jump panel then honestly projected both owners.

## Fix

Roster reconciliation now examines every tile remembering each listed sid before
deciding whether materialization is needed. Attached ownership has precedence;
otherwise the oldest Detached tile is retained. Redundant Detached tiles have
their tags merged into the retained tile and are removed. A second Attached owner
is deliberately left to the stronger restore-time ownership repair because
silently deleting a workspace member would mutate layout.

The renderer remains a pure ownership projection and does not conceal corrupt
state with row-level deduplication.

## Attempts

### 2026-08-30 — ownership repair at roster reconciliation

- Added `roster_reconciliation_retires_duplicate_detached_session_tiles`, which
  constructs the reported Attached+Detached collision plus two Detached owners,
  drives the production roster materializer, validates Agent identity, and counts
  destinations through the real jump-panel section projection.
- Negative control: before the repair, the guard failed because materialization
  returned unchanged and left both collisions intact.
- Implemented `reconcile_roster_agent_identity` at the roster boundary; duplicate
  tags survive on the canonical tile.
