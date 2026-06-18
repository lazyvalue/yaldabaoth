# Project: universal-agent-list

**Status:** 🟢 Phase 1 + Phase 2 landed — jump panel AND the per-tile selector
both project from one universal roster; builds + 223 tests green. Built on the
`universal-agent-list` worktree/branch (off `main` @ 720b7a0).
**Spec:** `docs/specs/spec-universal-agent-list.md`. **ADR:** `0022`.

## Problem / Why

Two surfaces listed agent sessions from two sources that never reconciled: the
jump panel read the local `AgentSessions` store (only sessions opened in THIS
GUI), and the per-tile selector fired its own `list_sessions` RPC into a per-tile
`SessionPicker` cache. So a session running on the server but not opened here was
invisible in the jump panel, and neither surface reacted to renames, to sessions
added/closed elsewhere, or to a selection in another tile.

## Goals

- One shared, live **roster** of every server-known session; every list projects
  from it (jump panel + selector), so rename/add/close/selection update all at
  once.
- Active sessions always visible in the jump panel, even if never opened here.
- No second source of truth; the roster is a read-only mirror of server truth.

## Model

```
YaldaGpuiView
 ├─ sessions: AgentSessions        ← live conversations BOUND to tiles (store)
 └─ agent_roster: AgentRoster      ← ALL server sessions, keyed by sid (cache)
        seed: refresh_roster (list_sessions @ boot)
        live: apply_server_batch → SessionCreated/Closed/Renamed
        ▲
        ├─ jump panel: jump_panel_agent_rows = roster ∪ local-only, deduped
        └─ selector (Phase 2): picker_projection(cwd) = roster − bound, cwd-filtered
```

## Phases

- **Phase 1 (done):** `AgentRoster` + API; wire the 3 broadcasts; seed at boot;
  start the pump at boot; jump panel reads the roster union; `jump_to_roster_session`
  opens a roster-only free session (ephemeral workspace + attach).
- **Phase 2 (done):** `SessionPicker` reduced to `{selected, cwd}`; selector
  renders/selects via `picker_projection(cwd)` from the roster; retired
  `spawn_list_sessions_for_picker` / `apply_picker_sessions` (+ INV-PR routing);
  `next_agent_label` dedups against the roster. See ADR-0022 migration care.

## Tickets

| Ticket | Subtasks | Status |
|---|---|---|
| (P1, in project.md) | roster model · wire events+seed · jump panel reads roster · end-to-end test | ✅ done |
| (P2, in project.md) | render from roster · select from roster · next_agent_label from roster · migrate picker tests · both-on-selection test | ✅ done |
