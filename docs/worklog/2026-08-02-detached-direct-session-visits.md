# Worklog: Detached direct session visits

**Date:** 2026-08-02
**Branch touched:** `main` (pre-existing dirty checkout)

## Built

- Added `UXI-JumpPanel-19`: selecting an agent session from the jump panel or
  `Cmd-P` always opens a bare ephemeral view.
- Kept a single `AgentTile::Bound` reference shape for workspace and direct
  viewports. The project/session domain owns identity and runtime state once;
  workspaces own tiles, and tiles reference the shared `SessionId`.
- Derived direct detachment from the containing workspace's `ephemeral` flag.
  Durable placement/free projections and persistence skip ephemeral workspaces,
  while the direct tile retains normal transcript and command behavior.
- Hardened close and `/clear` from a detached view so the durable owner never
  keeps a dangling session key and `/clear` carries the workspace placement to
  the replacement session.
- Reconciled the jump-panel, session-ownership, and ephemeral-workspace specs.

## Verification

- `cargo check --bin yalda-gpui`: passed.
- `cargo test --bin yalda-gpui`: 518 passed, 1 ignored.
- `cargo test --lib`: 162 passed, 2 ignored.
- `cargo test --bin yalda-gpui direct_session_visits_add_a_reference_and_keep_workspace_placement -- --nocapture`: passed.
- `cargo test --bin yalda-gpui jump_to_ -- --nocapture`: passed.
- `cargo test --bin yalda-gpui archive_unbinds_tiles_but_direct_jump_reopens_the_transcript -- --nocapture`: passed.
- Negative control: temporarily restored the former `jump_to_window(owner)`
  branch; the new guard failed because the expected ephemeral view was absent.
  Restoring the implementation returned the guard to green.
- Ownership negative control: temporarily counted ephemeral workspaces in
  `agent_tile_id_bound_to`; `jump_to_free_session_opens_then_tears_down_ephemeral`
  failed at "an ephemeral reference is not durable workspace placement."
  Restoring the workspace filter returned the guard to green.
- `git diff --check`: passed. Repository-wide `cargo fmt --check` remains noisy
  on pre-existing, unrelated formatting drift in the dirty checkout; no bulk
  formatting rewrite was applied.

## Open / unresolved

- None. This is state/navigation behavior and is fully headless-guarded; no
  pixel-level runtime check is required.

## Decisions

- No new ADR. This extends ADR-0021's existing ephemeral-workspace mechanism.
  The final model deliberately has no detached tile variant: a direct tile is an
  ordinary reference, and durable placement is a property of its workspace.
