# bug-0001: created-session-not-persisted

**Status:** RECURRED (2nd mechanism found + fixed 2026-07-14)
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
- **Resolve the sid via `sid_of` in `save_agent_ring` (attempt 2).** Correct but
  INSUFFICIENT alone: it fixed the in-memory `resume_sid` + `acp_sessions.json`, but
  the guard (`created_server_session_persists_its_id_for_restore`) only checked the
  IN-MEMORY `snapshot_content`, not the `workspace.json` FILE restore reads. The two
  persistence files were out of sync on disk (acp fresh, workspace stale) because
  `save_agent_ring` never wrote `workspace.json`. Lesson: for a persistence bug, the
  guard must LOAD THE ACTUAL FILE the restore path reads, not snapshot in memory.

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

### 2026-07-14 (2) — RECURRED: the two persistence files were out of sync on disk

- Symptom persisted after attempt 2. Ground-truth from the user's disk (NOT a theory
  this time): `acp_sessions.json` had the CURRENT session (`7a2d8254`, written by
  `save_agent_ring`) but `workspace.json` still showed a GONE session (`153b565c`) on
  one leaf + `null` on 7 others. The two files were out of sync — and `workspace.json`
  is the file restore reads for the per-tile id.
- Root cause: `save_agent_ring` stamps `resume_sid` in memory and writes
  `acp_sessions.json`, but NEVER writes `workspace.json`. `save_workspace_state`
  (which serializes the layout incl. `resume_sid`) only fires on STRUCTURAL changes
  (split/close/move). So a session you create-and-use, without restructuring, never
  gets its id into `workspace.json` → restart → picker.
- Fix: call `self.save_workspace_state()` at the end of `save_agent_ring` so the two
  files stay in sync (`agent_ui.rs`). Safe in tests — `workspace_persist_path()` is
  `None` under `cfg(test)` without an override, so it no-ops.
- Guard: `verify_harness.rs::save_agent_ring_persists_session_id_to_workspace_json`
  drives the REAL `save_agent_ring`, then LOADS `workspace.json` from disk
  (`load_persisted_workspace`) and asserts an agent leaf carries the id. This closes
  the gap in attempt 2's guard (checks the FILE, not memory). Negative-controlled:
  removing the `save_workspace_state()` call → `workspace.json` never written → RED.
- Also added boot-time restore diagnostics (`main.rs::restore_agent_leaves` eprintln
  per leaf: BOUND+resume / PICKER(duplicate) / PICKER(no id)) so a future recurrence
  produces the log directly instead of another guess-round. 369 suite green.
- STILL runtime-only (harness gap #2): the server-path restore ATTACH can't be driven
  headlessly (no mock session-server). If it still shows a picker after rebuild, the
  restore diagnostics will say which arm fired — paste them.
