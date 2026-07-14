# bug-0001: created-session-not-persisted

**Status:** FIXED
**First seen:** 2026-07-14
**Component:** `docs/components/agent-tile/session-binding.md` (UXI-AgentTile-18)

## Symptom

After restart, agent tiles show a **session picker** instead of auto-resuming the
session they held — despite the identity-based auto-resume feature (UXI-AgentTile-18)
being "implemented" and its tests green. "Still being prompted with a picker."

## Context / root cause

`save_agent_ring` resolved a bound tile's server session id as
`session.resume_id.or_else(|| channel.session_id())`. For a **freshly-CREATED**
server-managed session **both are None**:
- `resume_id` is only set when a session is *resumed* (from a persisted slot), never
  for one created this run.
- `channel` is `None` for server-managed sessions — the daemon owns the ACP channel,
  not the GUI.

So a created session resolved `None` → it was NOT written to `acp_sessions.json`, and
its tile's `resume_sid` stayed `None` → `workspace.json` persisted `session_id: null`
→ restore had no id to bind → picker. Only *resumed* sessions were ever persisted.
Disk confirmed it: 8 agent leaves, exactly 1 with an id (a resumed one), 7 null;
`acp_sessions.json` had 1 session while the server had 4 live.

The server sid IS known — it lives in the store's `by_sid` map
(`AgentSessions::sid_of(id)`), bound at create/attach time. `save_agent_ring` just
wasn't reading it.

## Planned solution

Resolve the id via the store's authoritative binding first:
`self.sessions.sid_of(id)` → fall back to `resume_id` / `channel.session_id()` only
if absent. Covers created AND resumed sessions uniformly.

## Approaches already tried (do NOT repeat)

- **Identity persistence via `resume_id`/`channel.session_id()` (the initial
  UXI-AgentTile-18 impl).** Green tests, broken app — the snapshot-layer guard
  (`agent_tile_persists_session_identity_not_index`) set `resume_sid` BY HAND, so it
  never exercised `save_agent_ring` resolving the id for a created session. Classic
  anti-circling failure: the test bypassed the real save path. Any future fix MUST
  drive `save_agent_ring` with a created (resume_id None, channel None) session.

---

## Log

### 2026-07-14 — resolve the sid from the store, add the missing save-path guard

- Fix: `agent_ui.rs::save_agent_ring` now resolves the persisted id via
  `self.sessions.sid_of(id)` first (then `resume_id`, then `channel.session_id()`),
  so a created server-managed session is persisted like a resumed one.
- Guard: `verify_harness.rs::created_server_session_persists_its_id_for_restore`
  drives the REAL `save_agent_ring` for a created session (resume_id None, channel
  None, sid bound in the store) and asserts the tile's `resume_sid` + the persisted
  layout leaf carry the id. Negative-controlled: reverting to the
  `resume_id`/`channel` chain fires RED ("a created session's id must be cached").
- Full suite 368 green. Runtime check still needed (harness gap #2): create a fresh
  session, restart yalda, confirm it auto-resumes in its tile with no picker.
