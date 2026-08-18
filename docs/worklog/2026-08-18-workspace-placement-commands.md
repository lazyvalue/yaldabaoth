# Worklog: Dwm-style workspace placement commands

**Date:** 2026-08-18
**Branches touched:** `codex/workspace-placement-commands` (`0cc2bff` —
feature/spec/tests), then `main` (merge commit follows; worklog commit is
separate)

## Cog execution evidence

- Graph id: `av4`

### Initial render

```text
graph workspace-placement-commands (frontiers)
frontier 0: spec-contract [open]
frontier 1: placement-model [open]
frontier 2: command-surface [open]
frontier 3: real-path-guards [open]
frontier 4: document-and-log [open]
frontier 5: integrate-verify [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `fmqi` `spec-contract`: claimed → closed; output defines complete
  `(Slot, Span)` footprint exchange, stable focus, Plane/Columns targeting, and
  bounded workspace-local undo in `UXI-Workspace-15`.
- `pgys` `placement-model`: claimed → closed; output records the model,
  32-entry undo history, restore behavior, and 53 passing workspace tests.
- `43b2` `command-surface`: claimed → closed; output records nine actions,
  collision-free `Ctrl-W` bindings, arrangement-root wiring, persistence, and
  the stale-safe tile picker.
- `g2yq` `real-path-guards`: claimed → closed; output records 591 passing GPUI
  tests, 171 passing library tests, the two-binary build, and six observed-RED
  production-path negative controls.
- `9uiq` `document-and-log`: claimed → closed; output records the README command
  reference, implemented UX invariant, this worklog, and its validation.
- `yrh0` `integrate-verify`: claimed → closed; output records the feature and
  merge commits plus focused verification on `main`.
- `sog1` `omega`: claimed → closed with output aggregating the shipped UX,
  implementation, verification, documentation, and integration evidence.

### Notes

- Node `pgys`, topic `deviation`: persisted `DesktopState` could no longer use
  a struct literal after the private undo history was added, so restore now goes
  through `DesktopState::restored`, which deliberately resets transients and
  command history.
- Node `g2yq`, topic `deviation`: repository-wide `cargo fmt --all` rewrote
  thousands of unrelated lines under rustfmt 1.8.0. That mechanical noise was
  removed; the retained seven-file implementation has a clean
  `git diff --check`, consistent with recent repository worklogs.
- Node `yrh0`, topic `deviation`: the first targeted mutation run caught 30 of
  34 mutants and exposed missing direct assertions for restored slots/spans/
  camera plus two-tile rotation. A restore contract and two-tile cycle case
  were added; the four survivors were then caught on rerun.
- No backlog entry matched this approved feature, so `docs/backlog.md` required
  no change.

### Final status

- Status: `complete`

```text
graph workspace-placement-commands (frontiers)
frontier 0: spec-contract [done]
frontier 1: placement-model [done]
frontier 2: command-surface [done]
frontier 3: real-path-guards [done]
frontier 4: document-and-log [done]
frontier 5: integrate-verify [done]
frontier 6: omega [done] (omega)
```

## Built (with status)

- **DONE — UXI-Workspace-15.** `Ctrl-W H/J/K/L` swaps the focused tile with a
  visible directional neighbor; Columns intentionally supports only `H`/`L`.
- `Ctrl-W Enter` promotes the focused tile, `Ctrl-W x` opens a keyboard tile
  picker, and `Ctrl-W r/R` rotates all complete placement footprints.
- `Ctrl-W u` walks a bounded workspace-local history of successful placement
  commands. Drag, resize, and structural placement changes invalidate that
  history instead of being undone accidentally.
- Complete `(anchor, span)` footprints move between stable window ids. Focus,
  app/session identity, marks, and camera semantics are preserved, and every
  successful command is persisted immediately.

## Open / unresolved

- None. The command grammar, state mutations, picker interaction, Plane and
  Columns behavior, persistence path, and stable-focus behavior are covered
  headlessly. Pixel-level picker aesthetics remain part of the repository's
  general visual-runtime gap, not a functional gap for this feature.

## Decisions

- Swaps exchange complete footprints rather than Apps or anchors alone. This
  matches dwm-style slot movement without introducing overlap when sizes differ.
- Columns vertical swaps are no-ops because there is no visible tile above or
  below; hidden plane geometry never overrides the visible arrangement.
- Arrangement undo is deliberately separate from document undo and is not
  persisted across restart.
- No ADR was needed: the change extends the existing workspace placement model
  and command grammar without changing an architectural boundary.

## Verification status

- `cargo test --bin yalda-gpui`: 592 passed, 0 failed, 2 ignored.
- `cargo test --lib`: 171 passed, 0 failed, 2 ignored.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed.
- `cargo test --bin yalda-gpui ctrl_w_ -- --nocapture`: 5 passed, including
  the three new production-keymap guards.
- `cargo test --bin yalda-gpui workspace::desktop_tests::placement_`: 4 passed.
- Targeted `cargo mutants` run over the new placement model: 34 tested; the
  initial 30 caught / 4 missed result strengthened the guards, then all four
  surviving mutants were caught on the focused rerun.
- Observed-RED negative controls independently disabled directional swap, undo,
  promote, forward rotation, backward rotation, and picker commit. Each guard
  failed at its command-specific assertion before the production path was
  restored.
- `git diff --check`: passed. No repository-wide formatting rewrite is retained.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-18-workspace-placement-commands.md`: passes.

## Next

- None required.
