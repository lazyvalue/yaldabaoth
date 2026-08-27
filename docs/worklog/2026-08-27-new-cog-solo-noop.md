# Worklog: new-cog-solo-noop

**Date:** 2026-08-27
**Branches touched:** `fix-new-cog-solo-noop`

## Cog execution evidence

- Graph id: `1dx`

### Initial render

```text
graph fix-new-cog-solo-noop (frontiers)
frontier 0: reproduce-guard [open]
frontier 1: implement-fix [open]
frontier 2: verify-document [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `sc2` `reproduce-guard`: claimed → closed; output: real dispatcher guard
  observed RED because solo New Cog preserved id 2 instead of creating a tile.
- `pxq` `implement-fix`: claimed → closed; output: solo New Cog creates,
  presents, loads, and persists a detached Cog tile; attached split path passes.
- `4fc` `verify-document`: claimed → closed; output: targeted guard, library
  suite, and GUI check passed; bug/worklog artifacts recorded.
- `wu8` `omega`: claimed → closed; output: aggregate acceptance verified.

### Notes

- Node `4fc`, seq `4`, topic `deviation`: repository-wide formatting drift and
  two unrelated session-server-fallback steering failures were recorded with the
  passing targeted, library, check, and diff evidence.

### Final status

- Status: `complete`

```text
graph fix-new-cog-solo-noop (frontiers)
frontier 0: reproduce-guard [done]
frontier 1: implement-fix [done]
frontier 2: verify-document [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- `new-cog-tile` now handles solo-presented ownership by creating a detached
  Cog tile and loading it by stable id; normal workspace creation still splits.
- A real dispatch-path harness guard covers both focus domains and preservation
  of the original detached tile.
- Bug record `bug-0061` captures the root cause and observed RED control.

## Open / unresolved

- The full GUI suite reached 757 passing tests but has two pre-existing
  environment-sensitive steering failures after the test harness could not start
  its session server and fell back to direct spawn. One failure reproduced alone;
  neither code path overlaps this Cog/Workspace-only change.
- Repository-wide `cargo fmt --check` reports extensive formatting drift in
  unchanged files. The changed regions match current rustfmt output and
  `git diff --check` passes.
- Live mouse/keyboard smoke testing in a displayed Linux GPUI window was not run;
  the headless test drives the exact menu dispatcher and production ownership path.

## Decisions

- Match New Agent's contextual ownership transition: direct/solo creation stays
  detached instead of manufacturing or mutating a workspace.

## Verification status

- Negative control: pre-fix real dispatcher test failed at the distinct-tile
  assertion (`left: 2`, `right: 2`).
- Targeted fixed guard: 1 passed.
- `cargo test --lib` outside the loopback sandbox: 213 passed, 2 ignored.
- `cargo check --bin yalda-gpui`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-27-new-cog-solo-noop.md`
  passes.

## Next

- Merge the verified branch to `main` and rebuild/restart Yalda for the Linux box.
