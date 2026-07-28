# Component: Project

**Status:** living
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
object. **Both its name and its cwd are unique** — the `Projects` store refuses a
second project with an existing name *or* cwd — and it owns an extensible
(empty-for-now) `params` map. The store's `by_name`/`by_cwd` indices are private;
creation is the only path to a project, so two projects sharing a name — or a cwd
— is unrepresentable. cwd-uniqueness makes `by_cwd` **total-or-none** (no
ambiguous "first match"), which is what lets a project-agnostic server session
infer its project (`UXI-Project-2`).

**Applies to.** `project.rs` (new): `ProjectId`, `Project`, `Projects`
(`create(name, cwd) -> Result<ProjectId, CreateError>` where `CreateError =
DuplicateName | DuplicateCwd(ProjectId)`; `ensure_at_cwd`, `get`, `by_name`,
`by_cwd`, `membership_for_cwd`, `rename`, `set_cwd`, `close`, `first`, `iter`).
Persisted via `persist.rs` (`~/.yalda/projects.json`).

**Why.** The cwd was a display string, never an identity, so nothing could carry
per-project configuration; objectifying it is what makes per-project settings
possible.

**Status.** `implemented` (T001, commit `e786c23`).

**Enforcement.** `project.rs::projects_store_enforces_unique_name` (create twice
with one name → refused; a second with an existing cwd → `DuplicateCwd`; `by_cwd`
resolves) — pure unit test; NC observed RED by removing the `by_name` check.
Plus `ensure_at_cwd_dedups_cwd_and_uniquifies_name`,
`rename_and_repoint_preserve_uniqueness`, `membership_infers_or_unfiles_by_cwd`.

### UXI-Project-2 — Every workspace and session belongs to exactly one project; cwd is derived

**Statement.** A `Workspace` carries a required, private `project: ProjectId`
**foreign key** (not its own cwd — the ADR-0023 pattern, type swapped). An
`AgentSession` likewise holds a `ProjectId` when locally created. The cwd used
anywhere — the workspace's inherited cwd, the cwd spawned into an agent
subprocess, the persistence/grouping key — is resolved **from the project at the
point of use** (`projects.cwd_of(id)`), **never cached** on the workspace/session,
so there is one source of truth and nothing to drift. A project-agnostic
roster/server session (no stored assignment) has its project **inferred** from
cwd via the three-valued `Membership::{Assigned | Inferred | Unfiled}` resolved at
the roster boundary — an inference is recomputed every render, never persisted as
authority.

**Applies to.** `workspace.rs` (`Workspace::cwd` reads through the store),
`agent.rs`/`agent_ui.rs` (`AgentSession.project`; `agent_base_cwd` resolves via
the active workspace's project), `jump_panel_view.rs` (rows resolve project by
id/cwd). Replaces `WorkspaceCwd` on the workspace and `AgentSession.cwd` as the
source of truth (ADR-0023 lifted to the project).

**Why.** Two independent cwd fields (`WorkspaceCwd`, `AgentSession.cwd`) linked
only at creation time is exactly the drift the project object removes.

**Status.** `implemented` (T003, commit `da833be`).

**Deviation from plan.** Only the **workspace** dropped its cwd for the FK.
`AgentSession` **keeps** its `cwd: PathBuf`, reframed as the **immutable spawn
directory** (server-side ground truth) — deriving a *running* agent's cwd from
its project would be wrong the instant the project repoints (the agent is still
in the old dir). The session's *project membership* is derived (`Membership`),
not its cwd. So T003 changed no `AgentSession` field; ADR-0028 §3 was corrected to
match.

**Enforcement.** `verify_harness.rs::workspace_and_session_cwd_derive_from_project`
(point the active workspace at a project rooted at A → `agent_base_cwd()` == A;
repoint the project's cwd to B → it follows live, proving derived-not-cached; NC:
disable `p.cwd = cwd` in `Projects::set_cwd` → stays A, observed RED). The FK swap
is additionally covered by the pre-existing `workspace_cwd_inheritance` /
`workspace_cwd_persists_across_restart` passing on the new path.

### UXI-Project-3 — The jump panel renders the project hierarchy

> **Chrome updated by `UXI-JumpPanel-7/-8`.** The inline create affordances
> described below moved OFF the panel: the top-level **＋ New project** row → the
> global menu; the per-project **＋ New workspace / ＋ New agent session** rows and
> the dim **cwd subtext** → gone, with create/delete now in the project **context
> menu** (click the name). The per-project *grouping* (one section per project,
> WORKSPACES + AGENT SESSIONS sublists, global `ctrl-<n>` numbering) is unchanged —
> only the create/delete entry points and the visual treatment moved.

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

**Status.** `implemented` (T004 core `caadfc9`; T004-tail T005). The jump panel
now renders one section per project (`jump_panel_sections` → `render_jump_panel`):
project header (name + dim cwd) + a WORKSPACES sublist (filtered by
`tab.project()`, global `idx+1` badge) + AGENT SESSIONS + inline ＋New workspace /
＋New agent session rows, plus a top-level ＋New project row. Empty projects still
render a section; individual tiles are not listed.

**Deviation from plan.** (1) Section ORDER reuses the existing `jump_cwd_order`
drag order keyed on the project's cwd display (the project header stays a
`CwdDrag` source/target), falling back to project-id order — the `order_grouped_rows`
"cwd order → project order" reframe was kept as a cwd-keyed order rather than a
new project-id order list, so empty projects can't yet be drag-reordered (they
sort by id). (2) Unfiled sessions (a cwd no project roots) still render under
electric-blue path headers in a trailing **Unfiled** section, preserving the
prior behavior for free roster sessions in dir-less projects.

**Enforcement.** `verify_harness.rs::jump_panel_renders_per_project_sections`
(two projects each with a workspace + one session at A's cwd → A's section lists
its own workspace by GLOBAL index and NOT B's, B renders an empty section, the
session groups under A, badges are distinct global numbers; NC: drop the
`t.project() == id` workspace filter → A lists B's workspace, observed RED). The
sections key on `ProjectId`, so the per-project header IS the project name by
construction. (The earlier stand-alone `jump_group_header` helper + its
`jump_panel_groups_sessions_by_project` test were removed once T005's
`jump_panel_sections` became the sole render path — single-sourced.)

### UXI-Project-4 — Creating a project asks for its cwd and starts empty

> **Simplified entry.** `?` → `p` now asks only for the cwd. The project name
> is derived from the directory basename using `project_name_for_cwd`; a name
> collision is uniquified with ` (2)`, ` (3)`, and so on. The two-field
> name/cwd overlay described in the historical notes is superseded.

**Statement.** The global **new project** command (`?` → `p`) opens an overlay
prompting for a **cwd** (resolved by `resolve_agent_cwd_arg`). On commit a new,
**empty** project (zero workspaces, zero sessions) is created and persisted with
a basename-derived unique name; a **duplicate cwd** is refused with a transient
error and creates nothing; an empty cwd cancels. The per-project **＋ New
workspace / ＋ New agent session**
create into *that* project (no cwd prompt — the cwd is the project's).

> **Entry points moved (`UXI-JumpPanel-7/-8`).** **New project** lives in the
> global menu; **New workspace / New agent session** live in the project name's
> context menu.

**Applies to.** `main.rs`: `ActiveOverlay::NewProject` with one cwd field, its
open + commit routing → `Projects::ensure_at_cwd`; `jump_panel_view.rs` the
per-project context-menu entry point. The project-scoped creates call
`new_workspace_in(project)` / `new_agent_session_in(project)`.

**Why.** A project is the create scope; making the cwd explicit at birth is what
lets everything below inherit it, while deriving the name keeps creation quick.

**Status.** `implemented` (simplified from the original T005 flow).
`ActiveOverlay::NewProject` (one cwd field) → `commit_new_project_overlay` →
`Projects::ensure_at_cwd`; a duplicate cwd or bad cwd surfaces a transient error
and creates nothing; an empty cwd cancels. The jump panel's per-project ＋New
workspace / ＋New agent session rows call `new_workspace_in(pid)` /
`new_agent_session_in(pid)` (cwd = the project's, no prompt).

**Enforcement.**
`verify_harness.rs::new_project_overlay_creates_from_cwd_and_rejects_duplicate_cwd`
drives the real overlay/commit path, proves basename derivation, uniquification
for two distinct directories with the same basename, empty-project creation,
and duplicate-cwd refusal.

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

**Status.** `implemented` (T005). The delete entry point is now the project
**context menu's** "✕ Delete project" item (`UXI-JumpPanel-8`), not a ✕ glyph on
the header (`UXI-JumpPanel-7` removed that); it still calls the same
`request_delete_project`: a non-empty project arms
`ActiveOverlay::ConfirmProjectDelete(pid)`, an empty one deletes directly.
`perform_delete_project` kills the project's sessions (local via
`AgentSessions::close` + off-thread `spawn_close_session`; roster-only sids
dropped + server-closed), closes its workspaces descending, seeds a placeholder
under a surviving project if that would empty the frame (Behavior 2), then
`Projects::close` + `save_persisted_projects`. A focused agent tile whose session
was killed falls back to its selector.

**Never zero projects.** Deleting the **last** project mints a fresh default
(rooted at the process dir, named from it) and seeds the replacement workspace
under THAT — the "never zero projects" twin of "never zero workspaces". The
delete closes the project *first*, then derives the survivor from what remains, so
a workspace can never point at a deleted project id (adversarial-review-caught bug,
commit `e25a43b`).

**Deviation from plan.** The cascade lives in `main.rs::perform_delete_project`
(not split into `agent_ui.rs`) since it orchestrates workspaces + projects +
sessions together. The live-server `close_session` round-trip is off-thread
against the daemon (harness gap #2); the headless guard asserts the store /
overlay / workspace state transitions (the reducer side), not the subprocess.

**Enforcement.** `verify_harness.rs::delete_nonempty_project_confirms_then_cascades`
(a project with a workspace + a session: `request_delete_project` arms the confirm
and removes NOTHING; `perform_delete_project` then drops the project, kills the
session (`sessions.close`), closes the workspace, and leaves ≥1 workspace; an
empty project deletes with no confirm; NC: skip the session-kill loop → the
orphaned session survives, observed RED). Plus
`delete_last_project_mints_a_fresh_default` (deleting the sole project leaves a
non-empty store + a workspace pointing at a LIVE project + a resolvable
`active_project`; NC: restore the `unwrap_or(pid)` survivor computed before close →
orphaned workspace, observed RED).

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

**Status.** `implemented` (T006). Three layers enforce intra-project binding:
(1) `picker_projection` (`agent_ui.rs`) filters the selector/free-session list on
the tile's project (`projects.by_cwd(cwd)`), not a raw cwd; (2) `jump_to_session`
(free branch) and `jump_to_roster_session` open the ephemeral virtual workspace
via `Workspace::open_ephemeral_tab_in(content, project)` pinned to the SESSION's
own project (resolved from its spawn cwd), so the subsequent bind is intra-project
by construction; (3) `picker_attach_existing` — the shared attach choke both the
picker and the roster-jump funnel through — carries a hard cross-project guard
(session project via `projects.by_cwd(session.cwd)` vs the active project) that
aborts before minting a placeholder and sets a transient note.

**Deviation from plan.** The bind gate lives at `picker_attach_existing`
(upstream of `bind_session_sid`/`apply_open_agent_resolution`, which lack
tile/project context), not in `agent_sessions.rs` — those stay the store-side 1:1
choke. Because (1) filters the selector and (2) redirects the free-session jump to
the session's own project, no normal UI path reaches the (3) hard refusal with a
cross-project session; it is defense-in-depth. The `OpenResolution::Created`
(brand-new session) path is same-project by construction (created at
`agent_base_cwd`) and is not gated.

**Enforcement.** `verify_harness.rs::bind_refused_across_projects_allowed_within`
(a free ROSTER session of project B is omitted from an A-tile's selector; a direct
`picker_attach_existing(A tile ← B session)` is refused — nothing binds, a
transient note is set; pointing the workspace at B and attaching the same session
succeeds). NCs observed RED: disabling the Part-4 guard → the A tile binds the B
session; removing the Part-3 filter → B's session lists in A's selector.

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

**Status.** `implemented` (removal half in T005; derivation in T006). Both global
cwd overlays are gone: `RenameTarget::FreeAgentSessionCwd` + `AgentNewSessionCwd`
and their open / commit / render arms are deleted, the `?`-menu "new agent
session" entry and its `new-free-agent-session` dispatch arm are removed, and
`claude-new-here` is retired — a session is created only via a project's ＋ row.
The `active_project()` derivation now exists (`agent_ui.rs`): focused workspace's
project → focused (bound) session's project → `projects.first()`. `agent_base_cwd`
was already the active project's cwd via `active_workspace_cwd` (T003), so it is
the cwd twin of `active_project` and needed no change.

**Deviation from plan.** In practice the workspace branch of `active_project`
dominates — an active tab always carries a project, so the session + `first()`
fallbacks are only reachable in the transient no-tab state; they are implemented
for spec fidelity and totality. The derivation lives in `agent_ui.rs` (beside
`agent_base_cwd`), not `main.rs`.

**Enforcement.** `verify_harness.rs::active_project_derives_from_focus`
(point the active workspace at project A → `active_project()` == A; jump a free
B-session — its ephemeral workspace opens under B per UXI-Project-6 — → it follows
to B; NC: hardcode `active_project` to `projects.first()` → focus changes don't
move it, observed RED) plus `global_cwd_session_overlay_is_gone` (the removed
entry point no longer opens a cwd overlay).

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

**Status.** `implemented` (T002, commit `e786c23`; wired into boot in T003 via
`boot_projects`). The store-level migration + naming + self-heal are done; the
persisted round-trip is guarded. The two named cwds fall out of the general
basename rule (`ws/yaldabaoth`→Yaldabaoth, `ws/fulcrum`→Fulcrum).

**Deviation from plan.** Realized as `migrate_cwds_to_projects` +
`project_name_for_cwd` (basename, first-letter-capitalized) + `ensure_at_cwd`
(dedups by canonical cwd, uniquifies a clashing name). Boot resolves via
`boot_projects` (`persist.rs`): load `projects.json` if present, else migrate.

**Enforcement.** `tests.rs` (pure serde + migration, no `~/.yalda`):
`migration_maps_known_cwds_and_basename_fallback` (two known cwds + one other →
three named projects, dup folded, nothing dropped; NC: replace the naming
derivation with a constant → all fold to one, observed RED),
`project_name_for_cwd_capitalizes_basename`, `projects_persist_round_trips_via_disk`
(names + cwds + params round-trip through the `cfg(test)` path seam).
