# ADR-0022: A single universal agent-session roster, projected by every list

Status: Accepted (2026-06-18) — Phase 1 + Phase 2 landed (jump panel + selector both project from the roster).
Related: spec-universal-agent-list.md, spec-jump-panel.md, ADR-0020, spec-agent-session-ownership.md.

## Context

Agent sessions were listed by two surfaces from two sources that never
reconciled: the jump panel read the local `AgentSessions` store (only sessions
opened in this GUI), and the per-tile selector fired its own `list_sessions` RPC
into a per-tile `SessionPicker` cache. Result: sessions running on the server
but not opened here were invisible in the jump panel, and neither surface
reacted to renames, to sessions created/closed elsewhere, or to a session being
selected in another tile.

The server already broadcasts `SessionCreated` / `SessionClosed` /
`SessionRenamed` to every connection — and `SessionCreated` was a deliberate
no-op hook in `apply_server_batch` annotated "for a future available-sessions
view." The infrastructure for a live shared list already existed; nothing
consumed it.

## Decision

Introduce **one `AgentRoster`** on the root view: a live, read-only cache of
every session the server knows about, keyed by server sid (`SessionInfo`).
Seed it with one `list_sessions` at boot/connect; keep it live by wiring the
three broadcasts into it. **Every surface that lists sessions projects from this
one roster** — the jump panel and (Phase 2) the per-tile selector — so a rename
/ add / close / selection updates all of them at once (they re-render from the
same notified root state).

The roster is intentionally **separate from the `AgentSessions` store**: the
store owns the live conversations bound to tiles (`Entity<AgentSession>`,
transcript/channel); the roster owns metadata about every session that exists.
The store is a subset of the roster (what's opened here).

## Alternatives rejected

- **Fold server sessions into the `AgentSessions` store.** The store's whole job
  is the 1:1 tile↔session binding of *live conversations* (ADR-0019); stuffing
  metadata-only placeholders for sessions no tile binds would blur that
  invariant and create half-real `AgentSession` entities. A separate read-only
  cache keeps the store's meaning intact.
- **Keep two sources, sync them on each change.** That's the bug — two caches
  that drift. One source, many projections, is the fix.
- **Make the jump panel poll `list_sessions`.** Wasteful and laggy; the server
  already pushes deltas. Seed once, then ride the broadcasts.

## Consequences

- The server pump now starts at **boot** (not only when an agent tile opens) so
  the roster stays live regardless of whether an agent tile is open. The pump is
  a cheap idle singleton.
- A session can appear in the roster (and jump panel) before it's in the store;
  selecting such a row opens it (ephemeral workspace + attach), reusing the
  picker's bind path. No new attach machinery.
- **Phase 2 migration care:** making the selector a roster projection retires
  the per-tile async `spawn_list_sessions_for_picker` / `apply_picker_sessions`
  round-trip whose INV-PR `WindowId` routing (ADR-0020) fixed a real
  "wrong-tile-fills, real-one-hangs" bug. That bug was a property of *per-tile
  async results racing onto the focused tile*; a single shared roster has no
  per-tile async result to misroute, so the failure mode is designed out rather
  than re-risked. The migration must keep the selector's selection-by-index and
  `next_agent_label` dedup reading the same projected list the render shows.
- The roster is not persisted — it's re-seeded each boot from server truth.
