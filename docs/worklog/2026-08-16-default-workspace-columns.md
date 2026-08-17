# Worklog: Default workspace arrangement to columns

**Date:** 2026-08-16
**Branches touched:** `default-workspace-columns` (`11579c8` — feature/spec/tests),
then `main` (`302ba19` — merge and release rebuild; worklog commit follows)

## Cog execution evidence

- Graph id: `1i9`

### Initial render

```text
graph default-workspace-columns (frontiers)
frontier 0: implement-default-columns [open]
frontier 1: verify-and-ship [open]
frontier 2: omega [open] (omega)
```

### Node execution

- `9jo5` `implement-default-columns`: claimed → closed; output:
  `{"summary":"Columns is now the WorkspaceView default for production-created, field-absent, and unknown-value workspaces; explicit plane and columns preferences still round-trip, and the lossless toggle remains unchanged.","verification":["5 focused columns/default/persistence/render tests passed","the manual Plane-default negative control failed for the intended reason","git diff --check passed"]}`
- `l7cx` `verify-and-ship`: claimed → closed; output:
  `{"summary":"Verified and shipped the columns-default workspace change on main.","commits":{"feature":"11579c8","merge":"302ba19"},"verification":["3 WorkspaceView mutants tested: 2 caught, 1 unviable","574 passed, 0 failed, 1 ignored in the worktree and on main","release build passed in the worktree and on main"]}`
- `9rxc` `omega`: claimed → closed; output:
  `{"summary":"Columns is the default workspace arrangement on main; infinite plane remains a lossless persisted alternate via Ctrl-W a or the workspace menu."}`

### Notes

- Node `9jo5`, seq `8`, topic `deviation`: the existing columns render guard
  assumed `Plane` was the product default; its fixture now establishes `Plane`
  explicitly before driving the real plane→columns toggle.
- Node `l7cx`, seq `12`, topic `deviation`: the first full suite run exposed
  four plane camera/placement guards sharing the same implicit-default fixture.
  `boot_desktop_two_tiles` now opts into `WorkspaceView::Plane`; focused reruns
  and the full suite then passed.

### Final status

- Status: `complete`

```text
graph default-workspace-columns (frontiers)
frontier 0: implement-default-columns [done]
frontier 1: verify-and-ship [done]
frontier 2: omega [done] (omega)
```

## Built (with status)

- **SHIPPED — UXI-Workspace-14 follow-up.** Fresh workspaces now start in
  equal-width columns, including snapshots predating the `view` field.
- Explicit persisted `"plane"` and `"columns"` choices still reopen unchanged.
  `Ctrl-W a` and the workspace menu continue to toggle losslessly between them.
- Unknown future `view` strings degrade to the current `Columns` default without
  dropping the workspace snapshot.
- Plane-specific headless fixtures explicitly select `Plane`, keeping their
  camera, culling, semantic-zoom, and pan coverage independent of the default.

## Open / unresolved

- None. The changed behavior and both arrangements are covered headlessly; this
  does not depend on one of the documented runtime-only gaps.

## Decisions

- An explicit persisted arrangement is a user preference and wins over the new
  default. Only fresh, field-absent, and unknown-value workspaces use `Columns`.
- No ADR was needed: the existing two-view architecture and lossless toggle are
  unchanged; this is a default-policy adjustment within UXI-Workspace-14.

## Verification status

- `cargo test --bin yalda-gpui columns -- --nocapture`: 5 passed.
- Negative control: moving `#[default]` back to `Plane` made
  `new_workspace_defaults_to_columns` fail with `left: Plane`, `right: Columns`.
- `cargo mutants --no-config --features test-support --file
  src/bin/yalda-gpui/workspace.rs --re 'WorkspaceView' --in-place --baseline skip
  --timeout 120 --cargo-arg=--bin=yalda-gpui -- workspace_view_`: 3 tested,
  2 caught, 1 unviable.
- `cargo test --bin yalda-gpui --features test-support`: 574 passed, 0 failed,
  1 ignored in the feature worktree and again on `main`.
- `cargo build --release --bin yalda-gpui`: passed in the feature worktree and
  again on `main`; the release binary contains merge `302ba19`.
- `git diff --check`: passed; `main` was clean after the merge and verification.
- Repository-wide `cargo fmt --all -- --check` reports broad pre-existing drift
  under the current formatter; no bulk reformat was included in this feature.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-16-default-workspace-columns.md` passes.

## Next

- None required. Use `Ctrl-W a` (or the workspace menu) whenever a workspace
  should return to the infinite plane.
