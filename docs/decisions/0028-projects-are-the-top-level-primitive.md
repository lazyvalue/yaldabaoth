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

- **Name AND cwd are both unique.** Name is the human key (renameable, the
  persistence reference); cwd is *additionally* enforced unique so `by_cwd` is
  **total-or-none** — one project per directory, no ambiguous "first match."
  (The user wants name-uniqueness and "not more than one project per cwd";
  enforcing both now is free — you can *relax* a uniqueness constraint later with
  zero migration, but you cannot *add* one once duplicates exist. Fable advisory.)
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

### 3. Workspaces and sessions hold a `ProjectId` foreign key; cwd is NEVER stored on them

The cwd **leaves the layout tree and the session entirely** — it is normalized
onto the project and resolved at each point of use. This is not "thread the store
everywhere": `workspace.rs` is a generic layout container (`Workspace<C>`) that
never actually *consumes* a cwd; every real consumer (agent-subprocess spawn,
jump-panel grouping, persistence key, new-workspace inheritance) already sits at
the view layer holding the `Projects` store. So the cwd read moves **up** to the
consumers; nothing is pushed **down** into the layout tree.

- `Workspace` (renamed `Tab`, §5) drops `cwd: WorkspaceCwd` and holds
  `project: ProjectId` — a **required, private** field, exactly the ADR-0023
  pattern with the type swapped (a workspace without a project is
  unrepresentable). `WorkspaceCwd`, `Tab::cwd`, `default_cwd`/`inherited_cwd`'s
  cwd form are deleted; the new-workspace inheritance copies the active
  workspace's `ProjectId`.
- `AgentSession` **keeps** its `cwd: PathBuf` — but reframed: this is the
  **immutable spawn directory** the subprocess actually runs in (server-side
  ground truth), NOT a cached copy of the project's cwd. It is legitimately owned
  by the session (deriving it from the project would be *wrong* the instant the
  project repoints — the running agent is still in the old dir). The session's
  **project membership** is what's derived, via `Membership` (below): `Inferred`
  from `projects.by_cwd(session.cwd)` today, upgradeable to a stored `Assigned`
  `ProjectId` when the server-metadata endgame lands. So T003 changes **no
  `AgentSession` field**; only the workspace loses its cwd.
- **Server/roster sessions are project-agnostic** (the session server knows
  nothing about projects — a self-imposed constraint, see Consequences). Their
  membership is *inferred* from cwd, never stored as authority. This is modeled
  as a three-valued `Membership` resolved at the roster boundary:
  - `Assigned(ProjectId)` — the stored foreign key (authoritative).
  - `Inferred(ProjectId)` — `Projects::by_cwd(session.cwd)` for a foreign
    session with no assignment (recomputed every render, **never persisted** as
    an assignment — persisting a guess turns it into fake authority that
    survives a rename/repoint).
  - `Unfiled(cwd)` — the honest "no project roots this cwd" state (rendered by
    shortened path); creation is deliberate, so a jump never auto-creates.

There is **no cached cwd anywhere**, so there is nothing to keep in sync. A
project cwd repoint therefore affects **new** spawns/pickers/grouping only; an
already-running agent subprocess keeps its original spawn cwd (server-side,
immutable) — see Consequences.

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

- **Denormalized cwd cache + single-writer resync** (workspace/session keep a
  cached `cwd` refreshed by a `resync_project_cwds` walk on every project-cwd
  change). **Rejected by name** — it reintroduces the cached-derivable-data /
  drift bug class this entire effort exists to eliminate (the same class the
  typed `WorkspaceCwd` field was created to kill). A foreign key on a row is
  normalization; caching the joined column is not. (Fable advisory: "Option B
  should be rejected in the ADR by name.")
- **Thread `cwd(&projects)` through `workspace.rs`** (pure derivation, store
  pushed down). Rejected as a *false premise*: `workspace.rs` never consumes a
  cwd, so there is nothing to thread — §3's "read at the consumer" achieves pure
  derivation without touching a single layout-tree signature.
- **A view-side `TabId → ProjectId` side map** instead of a field on the tab.
  Rejected — it makes "a workspace without a project" representable again (the
  exact failure ADR-0023 fixed) and must be hand-maintained at every
  workspace-creation path; someone forgets one. The FK belongs *on* the tab.
- **cwd as the project key** (a project *is* its directory). Rejected by the user
  in favor of a renameable name key; cwd is enforced as a unique *attribute*
  (§1), which gives deterministic `by_cwd` without making cwd the identity.
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
  `params` is the stringly-bag reborn (ADR-0023's lesson): promote each param to
  a typed field the moment it gains a consumer; the map is for opaque passthrough
  only.
- **Persisted references use the project NAME, not the runtime `ProjectId`** (a
  memory-only counter). Load order is `projects.json` → `workspace.json` /
  `acp_sessions.json`, and load is **self-healing**: an unresolved name resolves
  via `ensure_at_cwd(cwd, name)` using the cwd the record already carries (the
  server needs a session cwd regardless, so it is kept in `acp_sessions.json`), so
  nothing dangles on a partial write or hand-edit.
- **A project cwd repoint affects new spawns only.** Everything resolves live, so
  new agents/pickers/grouping follow immediately; an already-running agent
  subprocess keeps its original (server-side, immutable) spawn cwd. This is the
  bug class the FK model prevents and the rejected cache would have created.
- **Delete = confirm-then-cascade** (the user's choice): a project with
  workspaces or live sessions prompts, then on confirm closes its workspaces and
  kills its sessions. The confirm makes the session kills a *deliberate user
  gesture*, not a silent cascade (reconciles with the "never silently touch
  server sessions" principle). An empty project deletes directly.
- **Server-side session→project metadata is the durable endgame** (a follow-up
  ticket): `yalda-session-server` is ours, so a future opaque per-session
  metadata bag (`project=<name>` written at create) lets any client recover
  `Assigned` membership with no cwd inference — demoting `by_cwd` to a
  migration-era fallback. Not blocking this pass.
- Behavior is UX-visible; ships behind the `UXI-Project-N` invariants in
  `docs/components/project.md` with headless guards, and the pixel/gesture bits
  flagged `NEEDS-RUNTIME`.
