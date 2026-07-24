# 002 — Persistence + migration (`projects.json`, cwd → named project)

**Goal.** Persist the `Projects` store and, on first load without a
`projects.json`, migrate existing cwds to named projects — total, panic-proof,
lossless. (`UXI-Project-8`.)

## Subtasks

- [x] `persist.rs`: `PersistedProject { name, cwd, params }`, `PersistedProjects`
      root; `projects_persist_path()` → `~/.yalda/projects.json` with the
      `*_PATH_OVERRIDE` / `None`-under-`cfg(test)` seam.
- [x] `save_persisted_projects` / `load_persisted_projects`; hand-rolled
      unknown-field tolerance so an old/newer file never resets the store.
- [x] Migration `migrate_cwds_to_projects(workspace_cwds, session_cwds) ->
      Projects`: distinct cwds → projects; `~/ws/yaldabaoth`→**Yaldabaoth**,
      `~/ws/fulcrum`→**Fulcrum**, else basename title-cased. Runs only when
      `projects.json` is absent.
- [x] Tests (`tests.rs`, no `~/.yalda`): round-trip, and
      `migration_maps_known_cwds_and_basename_fallback` (two known cwds + one
      other → three named projects, nothing dropped). NC: drop the fallback → the
      third cwd's items orphaned (RED).

## Verification

`cargo test --bin yalda-gpui` migration + round-trip green; NC RED. Tests never
touch `~/.yalda`.

## Links

ADR-0028 §7 · `docs/components/project.md` UXI-Project-8 · ADR-0010 (cwd key),
`UXI-Workspace-7` (migration discipline).
