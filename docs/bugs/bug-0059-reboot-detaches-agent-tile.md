# bug-0059: reboot-detaches-agent-tile

**Status:** FIXED
**First seen:** 2026-08-25
**Component:** Workspace / Agent tile durable membership

## Symptom

An Agent tile that belonged to a named workspace can reappear as Detached after
a Yalda reboot. In the reported instance, `outlook lead` belonged to the
`Outlook` workspace before reboot; afterward `Outlook` was empty and the same
session appeared under Detached.

## Context / root cause

GUI boot starts the universal session-roster seed before it restores
`workspace.json`. The roster callback treats every session without a tile in the
current live `Frame` as roster-only, materializes it as a dormant Detached tile,
and immediately saves the frame. If that callback wins while the frame is still
the pre-restore default, the save overwrites the authoritative attached
membership that has not been loaded yet.

The corruption can be latent: the current process may finish restoring from an
already-loaded snapshot while the disk copy has been replaced, so the tile only
looks Detached on a later reboot. The timing dependence explains why the failure
is intermittent.

Read-only corroboration from the reported state: `~/.yalda/workspace.json` has
`Outlook` with an empty layout and session
`533c635d-3ce5-4391-afb5-7e8a40caf26e` (`outlook lead`, tile `1175`) in
`detached_tiles`; `acp_sessions.json` retains the same label and Fulcrum cwd.

## Solution

Make durable workspace restore a hard prerequisite of starting universal roster
materialization at the production GUI boot entry point. Cover that exact boot
initializer hermetically with a persisted named workspace containing an Agent
tile plus an immediately available roster entry. The stable tile must remain
Attached to the named workspace and the rewritten snapshot must retain that
ownership, with no Detached duplicate.

## Approaches already tried (do NOT repeat)

- Duplicate-session healing is not the ownership source here. It runs after the
  snapshot is loaded and prefers a correct-project attached candidate, but it
  cannot recover membership after an earlier pre-restore save has replaced the
  attached leaf entirely.
- Roster materialization cannot infer a named workspace from session cwd. A
  project may contain many workspaces; the persisted tile membership must be
  restored before roster-only sessions are synthesized.

---

## Log

### 2026-08-25 12:01 — localized boot-order overwrite

- Production order: `main.rs` starts `start_server_pump` / `refresh_roster`
  before calling `restore_workspace_from_disk`.
- Fast-result path: `refresh_roster` calls
  `materialize_roster_detached_tiles`, then `save_workspace_state` whenever it
  creates a tile.
- Cog graph `d14`, node `prove-race`, records the causal sequence and the
  reported on-disk end state.

### 2026-08-25 12:38 — restored ownership before roster startup

- Added `initialize_workspace_before_roster` at the production GUI boot seam.
  It restores `workspace.json` before the server pump or universal roster seed
  can start, so even an immediate roster result sees the durable ownership
  graph before it may materialize or save Detached tiles.
- Added the real-path guard
  `boot_restores_attached_agent_before_fast_roster_save`. It prepares Outlook
  with stable Agent tile `1175`, drives the production initializer with an
  immediate matching roster result, and checks both the live `Frame` and the
  persisted snapshot for exact Attached membership and no Detached duplicate.
- Required negative control: temporarily reversed the helper order. The guard
  failed at `Outlook remains present after boot`, reproducing the destructive
  pre-restore save; restoring the fix returned it to green.
- Verification: focused guard passed; full `yalda-gpui` suite passed with 714
  tests, 0 failures, and 2 ignored; the in-diff production-helper mutant was
  caught; `git diff --check` passed. Repository-wide `cargo fmt --all --
  --check` remains red on extensive pre-existing formatting drift outside this
  patch and is recorded as a Cog deviation.

### Repair boundary

The fix preserves membership that is still present in `workspace.json`. It does
not infer a named workspace for a tile whose saved ownership was already
overwritten: the roster contains project cwd but no workspace identity. The
reported `outlook lead` tile therefore needs a one-time explicit Send to the
`Outlook` workspace; subsequent reboots will preserve that choice.
