# Component: Project

**Status:** draft
**Component token:** `Project` (⇒ invariants are `UXI-Project-N`)

## Description

A **Project** is the top-level organizational primitive. It owns a **single cwd**
(the project root), a **unique name** (its key), and an extensible **params** map
(configuration; empty for now). Workspaces and agent sessions **belong to** a
project and inherit its cwd — the cwd no longer lives on a workspace or a session
(ADR-0028). A project can hold any mix, including none, of workspaces and
sessions, and an **empty project persists**.

The runtime object lives in a dedicated **`project` module** (`project::Project`,
`project::ProjectId`, `project::Projects`) to disambiguate from Linear's
`Project*` types. The `Projects` store is the single owner of the name-uniqueness
invariant (the same ownership pattern as `AgentSessions`, ADR-0026): its
`by_name` index is private and only its own methods write it.

```rust
struct ProjectId(u64);                 // stable, monotonic, never reused
struct Project { name: String, cwd: PathBuf, params: BTreeMap<String,String> }
struct Projects { by_id, by_name (private), next_id }
```

**The hierarchy the app now reads:** `Frame` → `Project`s (ownership axis) with
each project owning `Workspace`s (planes of `Window` tiles) and `AgentSession`s.
See ADR-0028 §5 for the accompanying `Tab`→`Workspace`, `Workspace`→`Frame` type
rename (the `tab` vocabulary is eradicated).

## References

- ADR-0028 — the model + naming decision (this component is its home).
- `spec-agent-session-ownership.md` — the `AgentSessions` store pattern this
  mirrors; sessions now also carry a `ProjectId`.
- `docs/components/jump-panel.md` — the jump panel renders this hierarchy
  (`UXI-Project-3` extends / reframes `UXI-JumpPanel-1..6`).
- `docs/components/workspace.md` — a `Workspace` belongs to a project; cwd derived.
- ADR-0021 (ephemeral virtual workspace), ADR-0022 (universal roster), ADR-0023
  (cwd was a required typed field — now lifted to the project), ADR-0010
  (canonical cwd key, used to resolve roster sessions → project).
- `spec-agent-cwd.md` — "a workspace is one project" (now mechanically enforced).

## UX invariants

### UXI-Project-1 — A Project is a first-class, name-keyed object owning one cwd + a params bag

**Statement.** A `Project { name, cwd, params }` exists as a real, persisted
object. Its **name is unique** — the `Projects` store refuses a second project
with an existing name — and it owns **exactly one cwd** and an extensible
(empty-for-now) `params` map. The store's `by_name` index is private; creation is
the only path to a project, so two projects sharing a name is unrepresentable.

**Applies to.** `project.rs` (new): `ProjectId`, `Project`, `Projects`
(`create(name, cwd) -> Result<ProjectId, DuplicateName>`, `get`, `by_name`,
`by_cwd`, `rename`, `iter`, `close`). Persisted via `persist.rs`
(`~/.yalda/projects.json`).

**Why.** The cwd was a display string, never an identity, so nothing could carry
per-project configuration; objectifying it is what makes per-project settings
possible.

**Status.** `not implemented`.

**Enforcement.** `tests.rs`: `projects_store_enforces_unique_name` (create twice
with one name → the second is refused; `by_cwd` resolves a path) — pure unit test,
negative-control by removing the `by_name` check.

### UXI-Project-2 — Every workspace and session belongs to exactly one project; cwd is derived

**Statement.** A `Workspace` carries `project: ProjectId` (not its own cwd); an
`AgentSession` carries a project (locally-created sessions store the `ProjectId`;
roster-only sessions resolve theirs by `Projects::by_cwd(session.cwd)`). The cwd
used anywhere — the workspace's inherited cwd, the cwd spawned into an agent
subprocess — is read **from the project**, never from a cwd field on the
workspace/session. There is one source of truth for a project's directory.

**Applies to.** `workspace.rs` (`Workspace::cwd` reads through the store),
`agent.rs`/`agent_ui.rs` (`AgentSession.project`; `agent_base_cwd` resolves via
the active workspace's project), `jump_panel_view.rs` (rows resolve project by
id/cwd). Replaces `WorkspaceCwd` on the workspace and `AgentSession.cwd` as the
source of truth (ADR-0023 lifted to the project).

**Why.** Two independent cwd fields (`WorkspaceCwd`, `AgentSession.cwd`) linked
only at creation time is exactly the drift the project object removes.

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `workspace_and_session_cwd_derive_from_project`
(change a project's cwd → the workspace's inherited cwd and a session's spawn cwd
both follow; NC: revert the derivation to a stored field → the change doesn't
propagate).

### UXI-Project-3 — The jump panel renders the project hierarchy

**Statement.** The jump panel is grouped by project: a top-level **＋ New
project** row, then one section per project (header = project name, dim cwd
subtext), each expanding into a **WORKSPACES** sublist and an **AGENT SESSIONS**
sublist plus inline **＋ New workspace** / **＋ New agent session** rows. Depth
stops there — **individual tiles are not listed**. Workspace numbering
(`ctrl-<n>`) stays **global and sequential across all projects** (the badge digit
under any project equals the flat position; "for now" — revisit for per-project).
The `PINNED` placeholder stays. The active-workspace and focused-session accent
marks (`UXI-JumpPanel-5`) and the per-session status dot (`UXI-JumpPanel-1`,
`-6`) render within their project's section.

**Applies to.** `jump_panel_view.rs`: `group_agent_rows_by_cwd` becomes
`group_rows_by_project` (keys on `ProjectId`, not `shorten_cwd_for_display`);
`render_jump_panel` grows the project-header + per-project create rows + a
workspaces-per-project sublist; the workspace-number badge stays global.

**Why.** The user wants to see the whole hierarchy — which workspaces and agents
belong to which project — at a glance, and to create either scoped to a project.

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `jump_panel_groups_workspaces_and_sessions_by_project`
(two projects each with a workspace + a session → the real `render_jump_panel`
row model shows two project sections, each listing its own workspace and session,
tiles absent; NC: collapse to a flat list → the section structure disappears).

### UXI-Project-4 — Creating a project asks for a name + cwd and starts empty

**Statement.** The top-level **＋ New project** opens an overlay prompting for a
**name** and a **cwd** (resolved by `resolve_agent_cwd_arg`). On commit a new,
**empty** project (zero workspaces, zero sessions) is created and persisted; a
**duplicate name** is refused with a transient error and creates nothing; an
empty name cancels. The per-project **＋ New workspace / ＋ New agent session**
create into *that* project (no cwd prompt — the cwd is the project's).

**Applies to.** `main.rs`: a new `RenameTarget::NewProject`-style overlay (name +
cwd), its open + commit routing → `Projects::create`; `jump_panel_view.rs` the
＋ rows. The per-project create rows call `new_workspace_in(project)` /
`new_agent_session_in(project)`.

**Why.** A project is the create scope; making name+cwd explicit at birth is what
lets everything below inherit the cwd.

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `new_project_overlay_creates_empty_project_and_rejects_dup`
(commit with a name+cwd → store gains an empty project; commit a dup name → error
note, no new project; NC: no-op the dup guard → a second project appears).

### UXI-Project-5 — Deleting a project confirms when non-empty, then cascades

**Statement.** A **delete project** action removes the project. If it still holds
workspaces or live sessions, it first shows a **confirmation prompt**; on confirm
it **cascades** (closes every workspace, kills every session), then removes the
project. An **empty** project deletes directly. Empty projects otherwise
**persist** — a project that loses its last workspace/session is **not**
auto-deleted.

**Applies to.** `main.rs` (delete action + confirm overlay), `agent_ui.rs`
(cascade: close workspaces, `AgentSessions::close` each session + server close),
`project.rs` (`Projects::close`), `jump_panel_view.rs` (the project-header
affordance). Persistence drops the project from `projects.json`.

**Why.** Cascade-on-confirm prevents both accidental data loss and orphaned
workspaces/sessions pointing at a dead project; persisting empty projects lets a
project be a durable place you set up before filling.

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `delete_nonempty_project_confirms_then_cascades`
(delete a project with a workspace + session → confirm required → on confirm both
are gone and the project is removed; an empty project deletes without a workspace/
session left behind; NC: skip the cascade → an orphaned session survives).

### UXI-Project-6 — Session↔tile binding is intra-project only

**Statement.** A session may be bound only to a tile whose workspace shares the
session's project. An unbound agent tile's **selector** and a workspace's
free-session list offer only **free sessions of that tile's project**. A
free-session jump (ADR-0021) opens its ephemeral workspace **under the session's
own project**. A cross-project bind is refused.

**Applies to.** `agent_ui.rs` / `agent_sessions.rs` (bind path gated on
project-match; the selector's free-session query filtered by project),
`jump_panel_view.rs` (free-session jump resolves the session's project for the
ephemeral workspace). Extends `spec-agent-session-ownership.md` INV-2 with a
project predicate.

**Why.** A session's cwd is its project's cwd; binding it into a foreign-project
workspace (different cwd) would misrepresent where the agent runs — the same
honesty rule as the jump-panel cwd-gate (`UXI-JumpPanel-2`).

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `bind_refused_across_projects_allowed_within`
(a free session of project A binds to a tile in an A-workspace; the same bind into
a B-workspace tile is refused and the selector doesn't list it; NC: drop the
project predicate → the cross-project bind succeeds).

### UXI-Project-7 — The active project is derived; create entry points scope to it

**Statement.** There is **no stored "current project"**. The active project is
derived: the project of the **focused workspace**, else the **focused session's**
project, else the **first** project. The former global/`?`-menu "new agent
session" with its cwd overlay (`UXI-JumpPanel-3`, `-4`) is **removed** — a session
is created only via a project's ＋ row (into that project). Keyboard/global create
paths, if kept, target the *active* project (no cwd prompt).

**Applies to.** `main.rs` (`active_project()` derivation replacing
`agent_base_cwd`'s active-tab-cwd lookup; removal of `FreeAgentSessionCwd` /
`AgentNewSessionCwd` cwd overlays), `agent_ui.rs` (`agent_base_cwd` → active
project's cwd), `jump_panel_view.rs` (the removed global ＋ row).

**Why.** With cwd on the project, there is no free-floating cwd to prompt for;
the create scope is always a project, so the active project is a pure derivation.

**Status.** `not implemented`.

**Enforcement.** `verify_harness.rs`: `active_project_derives_from_focus`
(focus an A-workspace → active project A; focus a B-session tile → B; NC: hardcode
the first project → focus changes don't move it) and
`global_cwd_session_overlay_is_gone` (the removed entry point no longer opens a
cwd overlay).

### UXI-Project-8 — Migration maps existing cwds to named projects, losslessly

**Statement.** On first load without a `projects.json`, the distinct cwds across
persisted workspaces and sessions become projects: `~/ws/yaldabaoth` →
**Yaldabaoth**, `~/ws/fulcrum` → **Fulcrum**, any other cwd → a project named from
its basename, title-cased. Every existing workspace and session is re-pointed at
its project by cwd match. The migration is **total and panic-proof** (an
unexpected cwd never drops the snapshot or panics) and **never loses data** — same
discipline as `UXI-Workspace-7`. Empty projects that result persist.

**Applies to.** `persist.rs` (`projects.json` load/save; the migration scan over
`PersistedWorkspace` cwds + `acp_sessions.json` per-session cwds; the two named
mappings + basename fallback), `main.rs` (run migration when `projects.json` is
absent, then bind workspaces/sessions to projects). Uses `cwd_match_key`
(ADR-0010) for the cwd→project resolution.

**Why.** Existing live state must land in named projects with zero loss on the
first run of the new model; the two known cwds get the user's chosen names.

**Status.** `not implemented`.

**Enforcement.** `tests.rs` (pure serde + migration, no `~/.yalda`):
`migration_maps_known_cwds_and_basename_fallback` (a synthetic snapshot with the
two known cwds + one other → three projects with the right names, every workspace/
session re-pointed, nothing dropped; NC: drop the fallback → the third cwd's items
are orphaned/lost, observed RED).
