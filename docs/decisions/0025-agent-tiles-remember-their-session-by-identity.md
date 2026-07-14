# ADR-0025: Agent tiles remember their session by identity and auto-resume on restart

**Status:** Accepted
**Date:** 2026-07-13
**Related:** ADR-0019 (Tiles contain Apps), `spec-agent-session-ownership.md`,
`spec-tabs-and-splits.md` (Behavior 23–24, workspace persistence),
`docs/components/agent-tile/session-binding.md` (UXI-AgentTile-18/19)

## Context

On restart, agent tiles were rebound to sessions **positionally**: the layout
(`workspace.json`) stored every agent leaf as `Agent { session_id: None }`, and
`restore_agent_leaves` zipped the flat per-cwd session list to the layout's agent
leaves **by index** (`persisted.get(i)`). A tile therefore did not durably remember
*which* session it held; the mapping was inferred from traversal order. Whenever the
zip broke — persisted list empty (cwd mismatch / nothing saved), more leaves than
sessions, a duplicate sid (`AlreadyOpen`), or a session gone server-side — the tile
fell to the free-session **picker**. Users hit the picker on restart and had to
re-select sessions, which they explicitly do not want.

## Decision

**Persist the binding by identity, and auto-resume — never a picker on restart.**

1. Each agent leaf stores its bound session's durable server id **in the layout
   leaf** (`PersistedKind::Agent { session_id: Some(id) }`). The id is cached on the
   `AgentTile` as `resume_sid` by `save_agent_ring` (which already resolves it), so
   the cx-free `snapshot_content` can write it.
2. On restore, each tile rebinds to **its own** stored id (details — mode / draft /
   cwd — looked up in the id-keyed `acp_sessions.json` side-channel; the leaf's id is
   authoritative for the binding). No positional zip.
3. **Unresumable session** (the daemon GC'd it; `session/load` fails): the tile shows
   a small inline "session unavailable — start fresh" affordance — one click, **not**
   the picker. (Deferred — see Consequences.)

Back-compat: an old `workspace.json` with no per-leaf ids (all `None`) falls back to
the positional zip once; the next save writes ids.

## Alternatives rejected

- **Resume-prompt in-tile** ("Resume this session?" per tile): the user resolved the
  fork to fully automatic — a prompt is still friction.
- **Keep positional, just fix the picker triggers**: doesn't address the root — the
  binding isn't remembered, so any layout/order change re-breaks it.
- **Silent fresh session on unresumable** (option (a)): rejected by the user in
  favor of an explicit inline notice (b) so a lost session is acknowledged, not
  silently replaced.

## Consequences

- Restart rebinds each tile to the same session it held, regardless of session-list
  order, cwd drift, or layout changes — the picker no longer appears on restart.
- `AgentTile` gains a `resume_sid` cache; `save_agent_ring` is now `&mut self` (it
  stamps the tiles in a second pass).
- **Part 2 (the unavailable inline notice, UXI-AgentTile-19) is built.** No worker
  plumbing was needed after all: the permanent "no such session" error already
  reaches the GUI in `spawn_attach_sessions`. It now takes a `resuming` flag —
  a gone session on an auto-resume routes to `reconcile_session_unavailable` (inline
  notice, keeps identity for a later re-attempt), while a gone session on a live
  re-attach keeps the existing close→picker path. The live "session gone" attach
  result driving it is the only runtime-checked seam (harness gap #2).
