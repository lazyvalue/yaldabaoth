# ADR-0028: Projects are the top-level organizational primitive

**Status:** Accepted
**Date:** 2026-07-23
**Related:** ADR-0002 (workspaces model — this **supersedes** its "defer the
`Tab`→`Workspace` rename" call), ADR-0010 (canonical cwd key), ADR-0021
(ephemeral virtual workspace), ADR-0022 (universal agent roster), ADR-0023
(workspace cwd is a required typed field), ADR-0025 (agent tiles remember their
session by identity), ADR-0026 (make impossible states unrepresentable),
spec-agent-session-ownership.md, `docs/components/project.md`.

## Context

Today the app's organizational hierarchy tops out at the **workspace** (the code
type is still `Tab<C>`; ADR-0002 renamed it in user-facing strings only). A
**cwd** is carried per-workspace (`Tab::cwd: WorkspaceCwd`) and per-session
(`AgentSession.cwd: PathBuf`), and the *cwd string* is the **implicit** grouping
axis — the jump panel groups agent sessions by `shorten_cwd_for_display(cwd)`
(`group_agent_rows_by_cwd`), and multiple specs already gesture at "the cwd is
the project axis" (`components/jump-panel.md`, `spec-agent-cwd.md`: "a yalda
workspace is one project"). But there is **no object** that owns "a set of
workspaces + a set of sessions + a shared cwd + configuration." The cwd is a
display string, not an identity, so nothing can carry per-project configuration.

The user wants that object made first-class, with a named hierarchy:

> Projects are at the top. Projects have a CWD and a map of other parameters.
> Workspaces belong to projects; Tiles are in workspaces. Agent sessions belong
> to projects; they can also be bound to tiles. A Project can have no workspaces
> (all sessions) or no sessions (all workspaces). The jump panel represents this.

## Decision

### 1. `Project` is a first-class, name-keyed object owning one cwd

```rust
struct ProjectId(u64);                 // stable local identity, monotonic
struct Project {
    name: String,                      // the UNIQUE key (user-facing)
    cwd: PathBuf,                       // exactly one; the project root
    params: BTreeMap<String, String>,  // extensible config bag — empty for now
}
```

- **Name is the unique key.** One `Project` per name. cwd is a *property*, not
  the key (in practice one project per cwd, but that is not enforced — the user
  chose name-uniqueness).
- **cwd lives ONLY on the project.** Workspaces and sessions no longer carry
  their own cwd; they reference a project and read the cwd *from* it. This
  enforces mechanically what `spec-agent-cwd.md` always intended ("a workspace is
  one project").
- **`params` is scaffolded, not populated.** No configuration keys are defined or
  surfaced in this pass; the bag exists so future `/new-ux` passes add settings
  (per-project model, permission mode, env, etc.) without a schema change.

### 2. One owner: a `Projects` store (mirrors `AgentSessions`, ADR-0026)

```rust
struct Projects {
    by_id:   BTreeMap<ProjectId, Project>,
    by_name: HashMap<String, ProjectId>,   // private; the uniqueness invariant
    next_id: u64,
}
```

The store is the **only** writer of `by_name`; illegal states (two projects with
one name) are unrepresentable. This is the same ownership pattern that fixed the
agent-session 1:1 bugs (`spec-agent-session-ownership.md`). Creation
(`create(name, cwd)`) rejects a duplicate name; `by_cwd(path)` resolves a cwd to
a project (first match — the practical-1:1 case is unambiguous).

### 3. Workspaces and sessions reference a project by id; cwd is derived

- `Workspace` (renamed `Tab`, see §5) replaces `cwd: WorkspaceCwd` with
  `project: ProjectId`. Its cwd is `projects.get(project).cwd`.
- `AgentSession` replaces `cwd: PathBuf` with `project: ProjectId`. The cwd
  spawned into the subprocess is read from the project.
- **Server/roster sessions are project-agnostic** (the session server knows
  nothing about projects). A roster-only session is mapped to a project by
  `Projects::by_cwd(session.cwd)` at render time — exactly how the jump panel
  groups by cwd today, but resolving to a stable `ProjectId` instead of a display
  string. Locally-created sessions carry their `ProjectId` directly.

### 4. Binding is intra-project only

A session bound to a tile must share the tile's workspace's project. Selectors
and free-session lists are project-scoped; a free-session jump (ADR-0021) opens
its ephemeral workspace **under the session's own project**. Binding a session
into a foreign-project workspace would misrepresent where the agent runs (its cwd
= its project's cwd), so it is refused — the same honesty rule as the jump
panel's cwd-gate (UXI-JumpPanel-2).

### 5. Eradicate the `tab` vocabulary — full type rename (supersedes ADR-0002 §Alternatives)

ADR-0002 deferred the `Tab`→`Workspace` type rename because the container struct
already owned the name `Workspace<C>`. The introduction of `Project` changes that
calculus, and the user has directed eradicating `tab` outright. So:

- `Tab<C>`  → **`Workspace<C>`** (the user-facing workspace: a plane of tiles).
- the old container `Workspace<C>` (one per OS frame, owns the workspace strip +
  buffer pool) → **`Frame<C>`** (matches the docs' existing informal term "one
  per OS-level Frame"; disambiguates from both the workspace and GPUI's window).
- `PersistedTab`→`PersistedWorkspace*` naming, `active_tab`→`active_workspace`,
  `next_tab`/`prev_tab`/`new_tab`/`close_tab`/`select_tab`→`*_workspace`, the
  `NewTab`/`CloseTab`/`NextTab`/`PrevTab`/`RenameTab` actions →`*Workspace`, and
  the ~340 call-sites move with them. The physical **Tab key** (`ctrl-tab`
  keystroke, `Key::Tab`, tab-character/indentation, markdown tables) is **NOT**
  touched — only the workspace-concept "tab".

The final code hierarchy reads cleanly: **`Frame` → `Workspace`s → `Window`s
(tiles)**, with **`Project`s** as an orthogonal ownership axis over workspaces and
sessions.

### 6. Naming vs Linear's `Project`

`Project` is already used by the Linear app (`linear.rs`: `ProjectDetail`,
`ProjectCandidate`, `LinearThing::Project`). None is a bare `Project` struct, so
there is no hard type collision, but to keep the read unambiguous the org
primitive lives in its own **`project` module** (`project::Project`,
`project::ProjectId`, `project::Projects`). Linear's types keep their `Linear`/
`…Detail` qualifiers.

### 7. Migration: cwd → named project (total, panic-proof)

On first load without a `projects.json`, scan every persisted workspace cwd and
persisted session cwd, and create projects for the distinct cwds:

- `~/ws/yaldabaoth` → project **Yaldabaoth**
- `~/ws/fulcrum` → project **Fulcrum**
- any other cwd → auto-create a project named from the directory basename,
  title-cased (**total + panic-proof**; not expected to fire — the user confirms
  live state is under the two known cwds).

Every existing workspace and session is then re-pointed at its project by cwd
match. Empty projects persist. An old snapshot with no `projects.json` never
drops data (same discipline as `UXI-Workspace-7`).

## Alternatives rejected

- **Nest `Vec<Project>` inside the frame, each owning `Vec<Workspace>`.** Rejected
  — it would re-thread focus/active-workspace/persistence through a second
  container level and fight the flat `Vec<Workspace>` the code already has. The
  store + derived grouping (§2/§3) mirrors the working `AgentSessions` pattern and
  keeps grouping a *derivation*, exactly as the cwd grouping is today.
- **Keep cwd the project key (a project *is* its directory).** Rejected by the
  user in favor of a name key ("probably has its own name"), so a project can be
  renamed and — in principle — two could share a cwd.
- **Keep `Tab<C>` internally (honor ADR-0002's deferral).** Rejected — the user
  explicitly wants `tab` eradicated "so we don't make that mistake again," and the
  `Project`/`Frame`/`Workspace` renaming resolves the collision that forced the
  deferral.
- **Populate `params` with a first setting now.** Rejected — no concrete setting
  was requested; scaffolding the bag keeps this pass focused on the hierarchy.

## Consequences

- **A large mechanical rename** (`Tab`→`Workspace`, `Workspace`→`Frame`; ~35
  defs + ~340 call-sites + docs). Sequenced as its own ticket, guarded by the
  existing suite; reverses ADR-0002's parked rename.
- **A new persisted artifact** (`~/.yalda/projects.json`) and a one-time
  migration; the cwd-string keys in `workspace.json` / `acp_sessions.json`
  (`persist_cwd_key`) stay as the server-facing cwd, now derived from the
  project.
- **The global cwd-overlay session-create is removed** (UXI-JumpPanel-3/4): a
  session is only ever created *inside* a project, so its cwd is the project's.
  The change-cwd flow (`AgentChangeCwd`) is reframed as "move session to another
  project" (or retired) — resolved in the component spec.
- **Per-project configuration becomes possible** — the point of the whole change.
- Behavior is UX-visible; ships behind the `UXI-Project-N` invariants in
  `docs/components/project.md` with headless guards, and the pixel/gesture bits
  flagged `NEEDS-RUNTIME`.
