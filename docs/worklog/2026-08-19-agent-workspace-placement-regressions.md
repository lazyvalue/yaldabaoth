# Worklog: Agent workspace placement regressions

**Date:** 2026-08-19
**Branches touched:** `codex/fix-agent-picker-placement` (`1b04249`), then
`main` (`51e09fd` merge)

## Cog execution evidence

- Graph id: `hcy`

### Initial render

```text
graph fix-agent-picker-placement (frontiers)
frontier 0: reproduce-contract [claimed]
frontier 1: implement-placement [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `rkn7` `reproduce-contract`: claimed → closed with output recording observed
  RED failures for painted-row click, Enter, the missing Agent command, and the
  33.5px-vs-29px jump-folder typography mismatch.
- `gzxk` `implement-placement`: claimed → closed with output recording the
  stable-tile replacement primitive, already-local no-duplicate handling,
  bound/unbound Agent send command, font correction, contracts, and guards.
- `5suk` `verify-integrate`: claimed → closed with output recording 629 passing
  GUI tests on both branch and merged main, passing release builds, feature
  commit `1b04249`, and merge commit `51e09fd`.
- `86i4` `omega`: claimed → closed with output aggregating the shipped picker,
  command, font, verification, and integration results.

### Notes

- Graph note seq 8 expanded the diagnosis after the reporter confirmed the
  bounce could affect keyboard as well as mouse: deterministic stable-tile
  placement, an explicit Agent send command, and the jump font fix became one
  regression bundle.
- Node `5suk`, topic `deviation`: main already contained unrelated scheduler
  lock and titlebar changes. `git merge --autostash` preserved and reapplied
  both; they remain the only dirty paths after integration.
- Repository-wide `cargo fmt --all -- --check` reports broad pre-existing
  formatting drift, so no unrelated mechanical rewrite was retained.

### Final status

- Status: `complete`

```text
graph fix-agent-picker-placement (frontiers)
frontier 0: reproduce-contract [done]
frontier 1: implement-placement [done]
frontier 2: verify-integrate [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **DONE — existing Agent placement.** Selecting an existing session from an
  empty workspace Agent tile now moves the session's stable unbound tile into
  that exact layout slot and retires the temporary picker tile. Click and Enter
  share the operation; already-local sessions are reused without duplication.
- **DONE — Agent command.** The Agent local menu exposes `p` → **send to
  workspace**. The destination picker accepts bound or unbound Agent tiles,
  preserves stable identity/state/tags, and follows the chosen same-project
  workspace.
- **DONE — jump font.** Cmd-P workspace folder rows explicitly use the standard
  base jump typography.

## Open / unresolved

- None for the reported regressions. The two ignored GUI tests and existing
  compiler warnings predate this change.

## Verification status

- Observed RED: click and Enter kept focus on temporary tile 3 instead of stable
  tile 2 before the placement fix.
- Observed RED: `agent-send-workspace` failed to open the workspace picker.
- Observed RED: workspace folder row measured 33.5px while the standard jump row
  measured 29px.
- Focused guards pass for real click, real Enter, already-local/no-duplicate
  placement, command dispatch/binding, stable layout-slot replacement, menu key,
  and exact jump-row typography.
- `cargo test --bin yalda-gpui`: 629 passed, 0 failed, 2 ignored on the feature
  branch and again on merged `main`.
- `cargo build --release --bin yalda-gpui`: passed on the feature branch and
  merged `main`; the main release executable is rebuilt.
- `git diff --check`: passed before commit.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-19-agent-workspace-placement-regressions.md`: passes.

## Next

- None required.
