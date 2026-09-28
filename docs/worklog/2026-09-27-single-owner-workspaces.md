# Worklog: single-owner-workspaces

**Date:** 2026-09-27
**Branches touched:** single-owner-workspaces (e2c82d1, 80e31fb, 9c6cb7e, 07375c1) → merged to `main`

## Cog execution evidence

- Graph id: `kqs`

### Initial render

```text
graph opus-default-and-single-owner-workspaces (frontiers)
frontier 0: design-no-detached [open], default-opus-5-5 [open]
frontier 1: omega [open] (omega)
```

Expanded after Scott chose option A (before any Detached code edits):

```text
frontier 0: design-no-detached [claimed], default-opus-5-5 [done]
frontier 1: adr-and-specs [open], remove-detached-model [open]
frontier 2: rewrite-tests [open]
frontier 3: worklog-merge [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `x71l` `default-opus-5-5`: claimed → closed; output: `{"commit":"e2c82d1","negative_control":"pin disabled -> first ModelChanged claude-opus-4-8 (RED)"}`
- `5xxu` `design-no-detached`: claimed → closed; output: `{"adr":"0039","decision":"option A (Scott)"}`
- `glyk` `remove-detached-model`: claimed → closed; output: `{"commits":["9c6cb7e","07375c1"],"build":"clean, no new warnings vs main"}`
- `ckoj` `adr-and-specs`: claimed → closed; output: `{"docs":["ADR-0039","UXI-Workspace-30","jump-panel.md",...]}`
- `s0mj` `rewrite-tests`: claimed → closed; output: `{"yalda_gpui":"826 passed","lib":"228 passed","deleted_tests":10,"new_guards":6}`
- `tzac` `worklog-merge`: claimed → closed; output: `{"merged":"main","release":"built; processes untouched"}`
- `z2g8` `omega`: claimed → closed; output: `{"complete":true}`

### Notes

- `graph`, topic `decision`: Option A — tile-less server sessions get no tile; Cmd-P + session selector open them into the project's workspace. Close workspace retires tiles; archive returns tiles to picker; legacy detached_tiles dropped on load; multiple workspaces remain.

### Final status

- Status: `complete`

```text
graph opus-default-and-single-owner-workspaces (frontiers)
frontier 0: design-no-detached [done], default-opus-5-5 [done]
frontier 1: adr-and-specs [done], remove-detached-model [done]
frontier 2: rewrite-tests [done]
frontier 3: worklog-merge [done]
frontier 4: omega [done] (omega)
```

## Built (with status)
- **Opus 5.5 default** (`acp_channel.rs`): fresh Claude `session/new` is pinned to
  `claude-opus-5-5` via `session/set_config_option` before the model selector is
  emitted; resumed sessions and Codex untouched. Real-worker fake-adapter guard,
  negative control RED. UXI-AgentTile-16 updated.
- **Detached tiles removed** (ADR-0039, UXI-Workspace-30): no `detached_tiles`,
  attach/detach actions + `ctrl-w b` / `ctrl-w shift-b` + tile-menu "detach tile",
  roster tile materialization, Detached duplicate repair, jump-panel Detached
  section, tag folders and their prefs. New `Frame::open_tile_in_project`
  (active → project's first workspace → new workspace). Cmd-P lists live,
  non-archived tile-less sessions (`PaletteTarget::Session`). Project-menu New
  agent session opens a visible tile (previously an invisible free session).
  Close workspace retires tiles; archive → picker. Legacy snapshot keys ignored.
  Also fixed `all_window_ids` missing hidden tiles of all-hidden workspaces.
- Net −1.4k lines.

## Open / unresolved
- On first launch of the new binary, the 56 persisted Detached tiles in
  `~/.yalda/workspace.json` are dropped (sessions remain on the server; reach via Cmd-P).
- Ephemeral virtual workspaces look test-only now (backlog entry).
- Legacy Waiting/Working/Archived tab projection code is still kept for tests only.

## Decisions
- ADR-0039: every tile belongs to a workspace — Scott's simplification request.

## Verification status
- Headless: `yalda-gpui` 826 passed, lib 228 passed on `main`.
- `cargo-mutants` not installed locally → mutation gate left to CI.
- `NEEDS-RUNTIME` (gap 2): Opus pin against the real `claude-agent-acp`; activation of
  the new release binaries (GUI + session server) is Scott's call — nothing restarted.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-27-single-owner-workspaces.md` passes.

## Next
- Restart GUI + session server when convenient; confirm new sessions show Opus 5.5 and Cmd-P lists tile-less sessions.
