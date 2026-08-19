# bug-0047: agent-session-duplicated-across-projects

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Workspace / Agent session placement

## Symptom

The same active Agent session can appear under two projects in the jump panel.

## Evidence and root cause

The live persisted snapshot contains repeated server session IDs on distinct
stable Agent tiles, including a duplicate pair assigned to Fulcrum and
Yaldabaoth. The runtime permits this race:

1. a newly opened tile owns a local pre-attach `SessionId`, which has no durable
   server ID yet;
2. roster reconciliation cannot identify that tile and materializes a dormant
   Unbound tile for the newly listed server session;
3. the later `bind_sid` transition gives the original tile the same server ID,
   leaving two stable tiles for one session.

Restore rejects later duplicates by file order, but does not know which tile's
project matches the session's immutable CWD. It can therefore retain the wrong
project assignment. The ownership APIs contain no typed transition that can
atomically reconcile the provisional and roster identities.

## Required fix

Introduce one typed Agent-placement choke that atomically adopts or merges a
roster tile when a provisional session receives its server ID. Enforce one
stable tile per session identity at runtime, restore, and snapshot boundaries;
project membership must derive from and remain consistent with the session's
project. Existing corrupt snapshots must heal deterministically.

## Verification

- A deterministic pre-attach/roster/bind race guard.
- A corrupt cross-project snapshot migration guard.
- A command-sequence invariant oracle checking every Agent server ID has exactly
  one stable tile and one project after each transition.
- `verify_harness::provisional_bind_reconciles_racing_roster_tile` observed two
  owners before the fix and one canonical owner afterward.
- `tests::persisted_duplicate_agent_identity_keeps_session_cwd_project` proves
  corrupt cross-project records retain the occurrence matching authoritative
  session cwd and retire the other record even when corruption also duplicated
  the stable tile ID.
- `verify_harness::agent_identity_guard_rejects_duplicate_local_and_durable_owners`
  proves both local and durable identity collisions are rejected explicitly.
- Persistence refuses ownership graphs with duplicate window or Agent identity;
  restore heals old Agent duplicates before constructing live state.
