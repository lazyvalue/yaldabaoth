# ADR-0023: A workspace's cwd is a required, typed field — "no cwd" is unrepresentable

Status: Accepted (2026-06-18)
Related: spec-agent-cwd.md, ADR-0021 (ephemeral virtual workspace), ADR-0019.

## Context

Agent sessions inherit their working directory from the active workspace
(`agent_base_cwd`). That cwd lived as a **stringly, optional** entry in a
general per-`Tab` registry: `kv: HashMap<String, String>`, read as
`kv["cwd"]`. "No cwd" was a representable state (empty/absent key), handled by a
silent `unwrap_or_else(process_cwd)` at read time.

The jump-panel work (ADR-0021) added a new kind of `Tab` — the ephemeral virtual
workspace — built with a fresh `Tab { …, kv: HashMap::new() }` literal. It
**silently omitted the cwd key**. Nothing forced it to carry one, so an agent
created while an ephemeral workspace was active inherited the process/launch dir
instead of the spawning workspace's cwd. The inheritance *chain* was intact and
tested; the regression was that a new construction path produced a workspace
with nothing to inherit, and the type system permitted it.

This is the recurring shape: an invariant ("every workspace has a cwd") kept as
a *convention* over an optional bag, which a new call site can violate without a
compile error.

## Decision

Make a cwd-less workspace **unrepresentable**.

- Introduce `WorkspaceCwd(PathBuf)` — a typed, always-present working directory.
- Replace `Tab.kv` (whose only consumer was `"cwd"`) with a **required, private**
  field `cwd: WorkspaceCwd`. Private to `workspace.rs`, so no other module can
  write a `Tab { … }` literal; all construction goes through `Tab::with_layout(…,
  cwd)`, which **demands** a cwd. Omitting it is a compile error — the exact
  regression can no longer be written.
- New tabs get the right cwd *by default*: `Workspace::open_ephemeral_tab` and
  the new-workspace paths call `Workspace::inherited_cwd()` (the active tab's
  cwd, else the root `default_cwd`). The process-dir default is chosen **once**,
  by the binary, at root-workspace creation (`Workspace::with_initial(content,
  cwd)`) — never silently at read time. `workspace.rs` stays free of any
  process-dir knowledge.
- `agent_base_cwd` / `active_workspace_cwd` become **total**: a workspace that
  exists always yields a cwd; the only `process_cwd` fallback left covers the
  degenerate "no active tab at all" state.

## Alternatives rejected

- **Keep `kv["cwd"]`, just patch `open_ephemeral_tab` to copy it.** Fixes the
  instance, not the class — the next new `Tab` literal can omit it again.
- **Required but public field.** Makes omission a compile error (good) but still
  lets a literal pass a bogus value; private + constructors also makes the
  *correct* value (inherit) the path of least resistance.
- **Fully privatize every `Tab` field + accessors.** Maximal encapsulation but
  disproportionate churn — `Tab`'s other fields (layout, focused, rail, …) are
  read pervasively. Only `cwd` carried the bug, so only `cwd` is private; the
  rest stay public.
- **Keep the general `kv` registry for future keys.** Its sole consumer was
  `"cwd"`; a stringly bag is precisely the shape that let a key be forgotten.
  Dropped — reintroduce a typed field if a future need appears.

## Consequences

- The cwd-inheritance regression (and its whole class — "a new workspace kind
  forgets cwd") is a **compile error**, not a runtime surprise.
- Persistence: `PersistedTab` gains a typed `cwd: Option<String>`; old snapshots
  (cwd in the legacy `kv`) are migrated on restore (`legacy_kv["cwd"]` → typed
  cwd, else process dir). The legacy field is read but never written again, so it
  ages out of snapshots.
- `WorkspaceCwd` is intentionally a thin wrapper (no path validation yet) — it is
  the *home* for "always absolute/validated" if we want it later.
- Backstop test: `workspace_cwd_inheritance` asserts an agent created in an
  ephemeral virtual workspace inherits the spawning workspace's cwd — belt on top
  of the type guarantee.
