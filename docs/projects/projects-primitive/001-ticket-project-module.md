# 001 — `project` module: `Project` / `ProjectId` / `Projects` store

**Goal.** Introduce the runtime Project object + its owning store, with the
name-uniqueness invariant enforced by construction. No wiring into workspaces/
sessions yet — types + unit tests only. (`UXI-Project-1`.)

## Subtasks

- [x] New `src/bin/yalda-gpui/project.rs`: `ProjectId(u64)`, `Project { name, cwd,
      params: BTreeMap<String,String> }`, `Projects { by_id, by_name (private),
      next_id }`.
- [x] Store API: `create(name, cwd) -> Result<ProjectId, DuplicateName>`,
      `get(id)`, `get_mut(id)`, `by_name(&str)`, `by_cwd(&Path)` (first cwd match,
      via `cwd_match_key`), `rename(id, name) -> Result<(), DuplicateName>`,
      `iter()`, `close(id)`.
- [x] Wire the module into `main.rs` (`pub(crate) use project::*;`) — matches the
      module-per-concern glob pattern.
- [x] Unit tests in `tests.rs`: `projects_store_enforces_unique_name` (dup create
      refused; `by_cwd` resolves; `rename` collision refused). Negative-control:
      drop the `by_name` check → dup succeeds (RED).

## Verification

`cargo test --bin yalda-gpui projects_store` green; NC observed RED. No `~/.yalda`
touch (pure in-memory).

## Links

ADR-0028 §1,§2,§6 · `docs/components/project.md` UXI-Project-1.
