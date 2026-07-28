# bug-0024: codex-restores-as-claude

**Status:** FIXED
**First seen:** 2026-07-28
**Component:** `docs/components/agent-tile/providers.md`

## Symptom

After restarting Yaldabaoth, a Codex session's tile identifies itself as Claude
in the top status strip, and its agent turns are headed `Claude` rather than
`Codex`. The server session remains a real Codex session.

## Context / root cause

`restore_agent_leaves` installs bound session entities synchronously, before the
asynchronous `refresh_roster` request has seeded `agent_roster`. It tries to infer
the restored session's provider from that empty roster and falls back to
`AgentProvider::Claude`. The later WAL-backed roster response carries
`provider: Codex`, but roster reconciliation repairs only lost labels
(`recover_labels_from_roster`); it never repairs the already-open session's
provider. Both user-visible labels correctly read `AgentState.provider`, so they
faithfully render the wrong restored state. This violates `UXI-AgentTile-30`.

The local `acp_sessions.json` side-channel also omits provider identity, so it
cannot bridge the startup race even though the session-server WAL is correct.

## Fix

Persisted the provider beside each local session snapshot and used it directly on
restore. For existing snapshots without that additive field, every open session's
provider is reconciled from the authoritative roster whenever a full roster seed
or `SessionCreated` notification arrives; the session entity is notified so the
cached transcript invalidates, and the repaired identity is persisted. Added a
headless regression through the real server-notification reducer and real paint
probes for both the top provider badge and transcript turn header.

## Approaches already tried (do NOT repeat)

- Provider-aware render helpers alone. They were correct, but their unit test
  hand-set `AgentState.provider = Codex` and never exercised restart/roster
  reconciliation, so it could not catch the wrong upstream state.

---

## Log

### 2026-07-28 — root cause localized

Confirmed live WAL headers carry `provider: codex` while the local
`acp_sessions.json` entries have no provider field. Located the race in
`restore_agent_leaves`: it reads the roster before the async startup seed lands,
defaults to Claude, and no later path reconciles provider identity.

### 2026-07-28 — fixed and regression-pinned

Added provider to the additive local session snapshot, used it in both
server-managed and direct-spawn restore, and added roster reconciliation for old
snapshots and startup races. The reconciliation notifies the session entity, so
the cached transcript invalidates alongside the tile.

The headless regression binds a Claude-fallback fixture to `S1`, applies a real
Codex `SessionCreated` notification plus reply replay, and asserts painted
`Codex` probes in both the status strip and transcript with no painted `Claude`
probe. Negative control was observed RED by removing the `SessionCreated`
provider reconciliation: state remained `Claude`; restoring it returned GREEN.
