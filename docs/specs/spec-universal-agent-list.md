# Spec: Universal agent-session list

Status: Draft (2026-06-18) — Phase 1 implemented.
Related: ADR-0022, spec-jump-panel.md, spec-agent-session-ownership.md, ADR-0020.

## Problem

Two surfaces list agent sessions from **different sources** that don't update
each other:

- The **jump panel** read the local `AgentSessions` store (`self.sessions`) —
  only sessions *this GUI has opened*. A session running on the server but never
  opened here never appeared. → "active sessions should always be visible" bug.
- The per-tile **session selector** fires its own per-tile `list_sessions` RPC
  and caches the result in `SessionPicker`. It doesn't react to renames, to
  sessions added/closed elsewhere, or to a session being selected in another
  tile.

## Model — one roster, read-only projections

A single **`AgentRoster`** on the root view (`agent_roster.rs`): a live cache of
**every** session the session-server knows about, keyed by server sid, holding
`SessionInfo` (label, cwd, connected, turns, permission_mode).

- **Seeded** by one `list_sessions` at boot/connect (`refresh_roster`).
- **Kept live** by the broadcasts the server *already* pushes
  (`apply_server_batch`): `SessionCreated` → upsert, `SessionClosed` → remove,
  `SessionRenamed` → relabel. (`SessionCreated` was previously a no-op hook
  explicitly "for a future available-sessions view" — this is that view.)
- Distinct from the `AgentSessions` store: the store holds the *live
  conversations* this GUI has bound to a tile (`Entity<AgentSession>`); the
  roster holds *metadata about every session that exists*. A session can be in
  the roster but not the store (running elsewhere, never opened here).

Both the jump panel **and** the selector render as **read-only projections** of
this one roster. Because both re-render from the same root state that
`apply_server_batch` already `cx.notify()`s, a rename / add / close / selection
updates both at once.

## Phase 1 (done) — jump panel reads the roster

- `AgentRoster` + API (`upsert`/`remove`/`rename`/`replace_all`/
  `entries_by_label`).
- Wire the three broadcasts into `apply_server_batch`; seed via `refresh_roster`
  at boot (also starts the server pump at boot so the list stays live without an
  agent tile open).
- Jump panel "Agent sessions" renders `jump_panel_agent_rows`: the roster
  (every server session) **unioned** with local-only sessions not yet in the
  roster (mid-create placeholders), deduped by sid. Status: `●` in-use (a tile
  binds it) / `○` free; disconnected sessions dimmed.
- Selecting a row: opened-here → focus/ephemeral (`jump_to_session`);
  roster-only free → open an ephemeral virtual workspace and attach via the
  picker's bind path (`jump_to_roster_session` → `picker_attach_existing`).

## Phase 2 (planned) — selector is a roster projection

Make `SessionPicker` derive its free/bound rows from the shared roster (filtered
by the tile's cwd, partitioned by `bound_sid_set`) instead of its own per-tile
`list_sessions` cache, so the selector auto-updates on rename, add/close, and on
a session being selected in another tile. This retires the per-tile async
`spawn_list_sessions_for_picker` / `apply_picker_sessions` round-trip and its
INV-PR `WindowId` routing (ADR-0020): a single shared roster has no per-tile
async race to misroute. See ADR-0022 "Consequences" for the migration care.

## Invariants

- INV-UAL1 — the roster is a read-only mirror of server truth; UI never mutates
  session state *through* it (selection goes through the existing bind/attach
  APIs).
- INV-UAL2 — the roster and the `AgentSessions` store are distinct and may
  diverge; the roster is the superset (all server sessions), the store is what's
  bound to tiles here.
- INV-UAL3 — every surface that lists sessions projects from the one roster (no
  second source).

## Out of scope

Cross-cwd visibility policy changes in the selector (keeps today's cwd filter);
roster persistence (it's a live cache, re-seeded each boot).
