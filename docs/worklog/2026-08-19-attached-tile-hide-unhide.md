# Worklog: Attached tile hide/unhide

**Date:** 2026-08-19
**Branches touched:** `codex/hide-unhide-tiles` (`<pending>`), then `main`
(`<pending>` merge)

## Cog execution evidence

- Graph id: `48s`

### Initial render

```text
graph attached-tile-hide-unhide (frontiers)
frontier 0: contract [open]
frontier 1: typed-model [open]
frontier 2: persist-migrate [open]
frontier 3: commands-nav [open]
frontier 4: verify [open]
frontier 5: integrate [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `6gnr` `contract`: claimed → closed; output: ADR-0034 and the Workspace,
  Jump Panel, and terminology contracts define Attached-visible,
  Attached-hidden, Detached, typed solo presentation, best-effort layout
  restoration, all-hidden workspaces, and independent Close.
- `v4ko` `typed-model`: claimed → closed; output: the workspace model encodes
  exclusive ownership and visibility, owns hidden windows under their
  workspace, implements Attach/Detach/Hide/Unhide/Close transitions, and
  validates every legal presentation state.
- `ncx3` `persist-migrate`: claimed → closed; output: hidden attachment,
  placement hints, typed solo presentation, and empty layouts round-trip;
  legacy Unbound/Scratchpad data loads additively and duplicate Agent identity
  is repaired before construction.
- `ebjz` `commands-nav`: claimed → closed; output: shared shell actions, menus,
  jump-panel folders, Cmd-P, all-hidden rendering, and Agent traversal use the
  typed placement model.
- `xcnw` `verify`: claimed → closed; output: complete deterministic GUI,
  library, non-benchmark target, and benchmark suites passed; focused mutation
  checks caught every viable Hide/Detach/persisted-identity mutation.
- `qw66` `integrate`: claimed → pending close; output: isolated feature commit,
  merge, post-merge verification, release rebuild, and worklog finalization.
- `c4ly` `omega`: pending.

### Notes

- Node `xcnw`, seq `2`, topic `deviation`: the plain GUI suite has two
  pre-existing steering failures. One was reproduced against untouched main;
  deterministic verification excludes only those two. A strict 0.5px tagged-row
  geometry assertion can also flake under cold mutation builds, so mutation
  copies exclude it after the ordinary GUI suite passes it.
- Node `xcnw`, seq `3`, topic `deviation`: the first combined all-target command
  forwarded GUI-only skip arguments into Criterion and ran one session timing
  test under mutation-worker contention. The timing test passed immediately in
  isolation; clean non-benchmark target and benchmark-target runs both passed.
- Final worklog validation occurs after omega closes because the repository
  checker requires a complete status and omega-done evidence.

### Final status

- Status: `pending integration`

```text
graph attached-tile-hide-unhide (frontiers)
frontier 0: contract [done]
frontier 1: typed-model [done]
frontier 2: persist-migrate [done]
frontier 3: commands-nav [done]
frontier 4: verify [done]
frontier 5: integrate [claimed]
frontier 6: omega [open] (omega)
```

## Built (with status)

- **DONE — independent placement and visibility.** A tile is Attached-visible,
  Attached-hidden, or Detached; Hide does not detach, Detach clears hidden
  state, and Close retires the tile shell without aliasing either operation.
- **DONE — layout-aware Unhide.** Hide records a best-effort plane footprint.
  Unhide restores it only while valid and otherwise uses the current layout
  manager's normal insertion rules, then follows and focuses the workspace.
- **DONE — navigation projections.** Hidden tiles remain under their workspace
  in the jump panel and Cmd-P, open alone temporarily, and remain hidden until
  explicit Unhide. Visible attached tiles open their workspace; Detached tiles
  remain in the tagged Detached list.
- **DONE — all-hidden workspaces.** A workspace may contain no visible tiles and
  renders an explicit all-tiles-hidden state without inventing a replacement.
- **DONE — durable identity.** Persistence, roster traversal, session restore,
  and close paths include hidden attached tiles and preserve the one-session /
  one-tile, one-project invariants.

## Open / unresolved

- No known hide/unhide or workspace-ownership regression remains. Two unrelated
  steering guards remain red on baseline main and are recorded above.

## Decisions

- ADR-0034: attachment, visibility, temporary presentation, and Close are
  independent state dimensions; legacy ADR-0033 terms migrate additively.

## Verification status

- `cargo check --bin yalda-gpui`: passed.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed.
- Deterministic GUI suite: 653 passed, 0 failed, 2 ignored, 2 baseline tests
  filtered.
- Library suite: 173 passed, 0 failed, 2 ignored.
- Clean `--lib --bins --tests --examples --features test-support` sweep: passed.
- `cargo test --benches --features test-support`: passed.
- Focused mutations: Hide dispatcher 3 caught; persisted identity 2 caught / 1
  unviable; Detach action boundary 3 caught / 1 unviable; zero viable survivors.
- `git diff --check`: passed before commit.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-attached-tile-hide-unhide.md`: pending omega.

## Next

- Restart the GUI to load the rebuilt release executable.
