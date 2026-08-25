# Component: Agent Stats

**Status:** living
**Component token:** `AgentStats` (⇒ invariants are `UXI-AgentStats-N`)

## Description

Agent Stats is Yalda's singleton system tile for operational evidence about
agents and source repositories. It has three read-only pages:

- **Agents** presents the live Working and Ready fleet without cold or
  disconnected sessions crowding current work.
- **Inactive** presents Archived and Unavailable sessions as distinct lifecycle
  states.
- **Repository** presents a bounded, read-only static scan selected from Yalda's
  registered projects and previously analyzed repositories.

The v1 source is deliberately honest. Agent facts come from the universal
roster and from session state that this GUI has loaded. Repository facts come
from bounded filesystem and Git metadata commands. Timestamped snapshots and
repository analyses are durably retained across Yalda reboots with bounded
retention; the UI distinguishes restored observations from current live facts.
V1 still does not claim exact lifecycle phase durations, provider token classes,
task outcomes, or empirical repository friction. Missing provider or
unloaded-session facts render as unavailable, never as zero.

Primary code homes are `telemetry/`, `agent_stats_view.rs`, and
`agent_stats_ui.rs`.

## References

- [jump-panel.md](jump-panel.md) — the System navigator and Cmd-P projection.
- [system-console.md](system-console.md) — the adjacent global operational row.
- `src/agent_event.rs` — canonical agent fact identity and ordering.
- `src/bin/yalda-gpui/yux/CLAUDE.md` — cached-view and reusable-detail rules.
- Cog roadmap `b68` at `%projects/cog/telemetry::roadmap` — historical telemetry,
  deeper repository traces, optimization, and failure analysis.
- Cog v1 graph `p62` at `%projects/cog/telemetry::v1` — implementation record.
- Cog follow-up graph `9ku` — inactive-agent partition and multi-repository
  selection.

## UX invariants

### UXI-AgentStats-1 — System navigation opens one stateful tile

**Statement.** The jump panel's System section contains an **Agent stats** row
directly beside **System console**, and Cmd-P contains one matching **Agent
stats** target. Both entry points call the same open-or-focus transition. The
first activation creates one Detached Agent Stats tile and presents it solo;
later activations focus that tile wherever it is attached, hidden, or Detached.
No activation creates a duplicate or changes the active workspace layout.

**Applies to.** `App::AgentStats`, `open_agent_stats`,
`render_jump_panel`, `PaletteTarget::AgentStats`, and
`activate_jump_palette_selection`.

**Why.** A global monitor must stay reachable without consuming a workspace
slot, and two global navigation surfaces must not create two copies of the same
stateful monitor.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::agent_stats_system_row_and_cmd_p_open_one_singleton`.

### UXI-AgentStats-2 — Active, inactive, and repository pages are honest

**Statement.** The tile has exactly three top-level pages, **Agents**,
**Inactive**, and **Repository**. Agents contains only Working and Ready root
sessions. Its totals and averages use that active population, with each metric's
known-value denominator still explicit. Inactive contains every Archived and
Unavailable root session and no active session. Archived is server-authoritative
cold storage; Unavailable means an unarchived session whose live transport is
not currently connected. Both states have distinct labels and counts.

Both agent pages show provider, model, availability, settled turns, current
tool counts and failures, context occupancy, optional cumulative cost, and
current-turn elapsed time when each source exists. Loading and empty states are
explicit. Context occupancy is never labeled as consumed tokens, and absent
facts are never displayed as numeric zero.

Repository offers a bounded selector containing all registered project roots
plus roots with retained analyses. Entries use project names when available and
otherwise an honest path-derived label. Opening Agent Stats selects the project
that was active at the entry point; choosing another entry keeps that selection
while refreshing exactly that root. This path is generic: Yalda, Fulcrum, and
every other repository use the same catalog, selection, scan, and persistence
logic. Repository shows the selected root, tracked file totals, bounded
top-level and extension distributions, instruction and workspace-manifest
counts, large source files, and recent Git churn. Loading, non-Git, and
command-error states use explicit text.

The Repository page scans the selected catalog entry. A refresh retains that
selection rather than silently switching back to the currently focused project.
The scan reads repository metadata and transiently counts bounded source-file
lines. It never retains source content, runs builds or tests, or mutates the
repository.

**Applies to.** `AgentMetricSnapshot`, `FleetMetricSnapshot`,
`RepositorySnapshot`, `AgentStatsView`, and the Agent Stats refresh methods.

**Why.** Current work must remain legible even when the durable roster contains
many cold or disconnected sessions, and repository evidence must be comparable
across the codebases Yalda's agents actually develop.

**Status.** `implemented`

**Enforcement.** `telemetry::agent::tests::roster_only_sessions_remain_visible_with_unloaded_facts_unknown`,
`telemetry::agent::tests::every_average_exposes_its_actual_denominator`,
`telemetry::agent::tests::disconnected_and_archived_are_distinct_and_cannot_be_resurrected_locally`,
`telemetry::repository::tests::tracked_projection_is_deterministic_and_bounded`,
`telemetry::repository::tests::scan_never_copies_file_content_into_snapshot`,
`verify_harness.rs::agent_stats_partitions_active_and_inactive_agents`, and
`verify_harness.rs::agent_stats_selects_another_registered_repository`.

### UXI-AgentStats-3 — Collection and rendering never block the frame

**Statement.** Agent Stats renders through a yux cached child. The component
owns its page and snapshot state and self-invalidates at mutation sites. An
unrelated root notification does not rebuild it. Repository commands run away
from the render path; their completion updates the owned snapshot and schedules
one redraw. Theme and text-style changes explicitly invalidate the cached view.

**Applies to.** `AgentStatsView`, `cached_child`,
`record_render("agent_stats")`, `refresh_agent_stats`, and
`notify_agent_stats_view`.

**Why.** Monitoring must not add frame latency to the agent and editor surfaces
that it measures.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::agent_stats_cached_body_only_rerenders_for_owned_inputs`
and `verify_harness.rs::agent_stats_repository_refresh_is_explicit_and_generation_gated`.

### UXI-AgentStats-4 — The singleton survives workspace persistence

**Statement.** Workspace persistence stores Agent Stats as a distinct stateless
App kind. Restore recreates its tile and page shell. Opening Agent Stats after
restore focuses the restored tile instead of creating another one.

**Applies to.** `PersistedKind::AgentStats`, `snapshot_content`,
`restore_content`, and `open_agent_stats`.

**Why.** A restored system tile must retain shell identity independently from
the telemetry store's data lifecycle.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::agent_stats_system_row_and_cmd_p_open_one_singleton`.

### UXI-AgentStats-5 — Telemetry and analysis survive Yalda reboots

**Statement.** Agent observations and repository analyses are stored as
versioned, timestamped, privacy-safe records under Yalda's data directory.
Retention is bounded. Repository records are keyed by normalized repository
root and never retain source content. Startup restores the last accepted
records before fresh collection resumes; restored data is labeled with its
observation time and is never presented as current live state. Missing,
corrupt, or unknown-version stores fail to an explicit empty state without
blocking startup. Collection persists observations even when the Agent Stats
tile is closed.

**Applies to.** `TelemetryStore`, `TelemetryStore::load`,
`TelemetryStore::save`, agent collection mutation sites, repository scan
completion, and `AgentStatsView`.

**Why.** Lifecycle and optimization evidence is only useful if a Yalda restart
does not erase it.

**Status.** `implemented`

**Enforcement.** `telemetry::store::tests::save_load_reconstructs_timestamped_agent_and_repository_observations`,
`telemetry::store::tests::agent_history_is_coalesced_and_retained_at_a_fixed_bound`,
`telemetry::store::tests::repository_entries_are_latest_by_normalized_root_and_bounded`,
`telemetry::store::tests::missing_corrupt_and_unknown_versions_fail_safe`, and
`verify_harness.rs::agent_stats_restores_durable_observations_before_fresh_collection`.
