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
- Cog follow-up graph `k2k` — readable-width tables and per-agent observed
  timelines.

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

### UXI-AgentStats-6 — Dense evidence keeps a readable measure

**Statement.** The tabs remain tile-width, while the scroll body's metric cards,
tables, repository projections, and detail content share a centered readable
measure capped at 1,280 logical pixels. Below that cap the body remains fluid and
uses the available width. Flexible agent labels and repository count labels grow
only inside that measure; they never push their trailing values to opposite edges
of an ultrawide tile.

**Applies to.** `AgentStatsView::render`, active/inactive agent tables,
`repository_ready`, and per-agent timeline detail.

**Why.** Operational tables are read by scanning across rows. Letting a flexible
cell absorb an ultrawide monitor makes related values visually unrelated and
forces excessive eye travel.

**Status.** `implemented`

**Enforcement.**
`verify_harness.rs::agent_stats_content_keeps_a_readable_measure_on_wide_tiles`.

### UXI-AgentStats-7 — Agent rows open an honest durable timeline

**Statement.** Clicking any row on Agents or Inactive opens an in-place detail
view for that row's stable telemetry identity. The detail header names the agent
and exposes a visible **Back** control; Back or Escape returns to the originating
list without changing the top-level tab. Switching top-level tabs closes the
detail.

The detail derives a bounded chronological timeline from the same retained fleet
observations used by Agent Stats. It shows first/last observation, state changes,
and known deltas for settled turns, tools, tool failures, context occupancy, and
cost. Consecutive fleet samples that do not change the selected agent are
collapsed. The current live observation may appear as an unsaved trailing point.
The view calls this an **observed timeline** and states that lifecycle boundaries
are captured immediately while ordinary metric churn is sampled at most every 30
seconds; it never claims exact phase durations or historical token consumption.

Restored history opens before fresh collection after a Yalda reboot. If retention
contains no point for the selected identity, the detail shows an explicit empty
state. Timeline persistence remains telemetry-store v1 and retains only the
existing privacy-safe snapshots: never transcripts, prompts, source content, or
tool inputs/outputs.

**Applies to.** `AgentStatsView`, `TelemetryStore::agent_history`, agent-row click
handlers, Agent Stats key handling, and the cached render body.

**Why.** Fleet aggregates reveal which agents are expensive or stalled; the
timeline is the first drill-down needed to see when their lifecycle and activity
changed without overstating the precision of v1 telemetry.

**Status.** `implemented`

**Enforcement.**
`agent_stats_view::tests::agent_timeline_collapses_unrelated_fleet_churn_and_reports_known_deltas`
and `verify_harness.rs::agent_stats_row_click_opens_durable_observed_timeline`.

### UXI-AgentStats-8 — Tool usage is attributable without retaining payloads

**Statement.** An agent timeline shows which tools changed at each retained
observation and includes one deterministic aggregate table for that agent. Each
aggregate row names the tool and reports observed calls, failures, and failure
rate. Failures are grouped by tool rather than shown only as one session total.
Timeline entries describe per-tool changes as observed deltas at the sample time;
they do not claim an exact call start, completion, or failure timestamp.

Tool coverage is optional. A locally loaded session supplies a bounded set of
normalized tool names with cumulative call and failed-call counts. An unloaded
session is **unknown**, not zero; retained history may still supply its latest
known breakdown. Names prefer a provider-reported tool identifier, then a safe
single-token title, and finally the coarse ACP kind. Normalization is
case-insensitive, length-bounded, and accepts only identifier punctuation.

The additive telemetry-store v1 field retains only tool name and integer counts.
It never persists tool-call ids, arguments, outputs, content, file locations,
source text, prompts, or error messages. Old v1 documents without the field load
with unknown tool coverage. Cardinality is bounded per agent and deterministic
when providers report more distinct names than the limit.

**Applies to.** `AgentMetricSnapshot`, `collect_agent_metrics`,
`TelemetryStore`, `project_agent_timeline`, and `AgentStatsView` timeline detail.

**Why.** A total failure count identifies a struggling agent but cannot reveal
whether the problem is concentrated in shell execution, file editing, search,
or another tool. Names and grouped outcomes provide actionable optimization
evidence without retaining operational payloads.

**Status.** `implemented`

**Enforcement.**
`telemetry::agent::tests::loaded_agent_tool_usage_is_normalized_bounded_and_grouped`,
`telemetry::store::tests::older_v1_documents_load_with_unknown_project_and_tool_diagnostic_coverage`,
`agent_stats_view::tests::agent_timeline_projects_named_tool_deltas`, and
`verify_harness.rs::agent_stats_timeline_restores_concrete_tools_failure_reasons_and_compact_cards`.

### UXI-AgentStats-9 — Agents are project-grouped and timeline samples are concise

**Statement.** The Agents and Inactive pages group root-session rows by their
registered Yalda project. Project ownership is derived from the session working
directory using the longest matching registered project root, so a nested
project wins over its parent. Sessions without known working-directory coverage
or a registered matching root appear under an explicit **Unassigned** group;
the UI never guesses from an agent label. Group headings include row counts and
remain inside the existing global row bound.

Agent Stats describes Yalda/ACP root sessions. Provider-internal subagents are
not independent fleet rows because the universal roster does not assign them a
stable session identity; their activity remains attributable to the parent
session's tools until such an identity exists.

The per-agent detail presents provider and model once in its identity header,
not in every observation. A settled-turn counter change is a divider between
observation cards rather than a repeated key/value row. Cards omit cumulative
turn and tool totals. Instead, the first retained card labels its cumulative
tool coverage as **Tools known at first sample**, and later cards show **Tools
in this sample**: the named, counted tool deltas observed since the prior
retained sample. Unknown legacy coverage and known empty activity have distinct
copy. These deltas belong to the sample interval and do not claim exact
tool-call timestamps.

Each sampled failed-tool delta includes the bounded provider-reported reason
when one is available. Collection never reads tool input and never retains a
successful tool's output. For a failed call it extracts at most one
single-line, 240-character diagnostic from explicit error/message/stderr output
or ACP text content, groups identical `(tool, reason)` observations, and uses an
explicit **No reason reported** fallback. At most 64 distinct failure-reason
rows are retained per agent. These local diagnostics can contain provider- or
tool-authored paths and values; the UI and schema describe them as bounded
diagnostics, not as secret-redacted data. Tool-call ids and complete payloads
remain excluded.

Timeline card labels have a fixed non-wrapping column wide enough for the
longest label. Tool and failure details use multiline rows rather than one long
wrapped key/value value.

**Applies to.** `AgentMetricSnapshot`, `ToolMetricSnapshot`,
`collect_agent_metrics`, project grouping, timeline projection,
`AgentStatsView`, and `TelemetryStore` v1 compatibility.

**Why.** Fleet work is primarily understood by codebase, while a useful agent
timeline should expose the activity that changed without repeating stable
identity and cumulative counters. Failure counts identify concentration;
bounded reasons make that concentration actionable.

**Status.** `implemented`

**Enforcement.** `telemetry::agent::tests::failed_tool_diagnostics_are_bounded_grouped_and_ignore_inputs_and_successes`,
`telemetry::store::tests::older_v1_documents_load_with_unknown_project_and_tool_diagnostic_coverage`,
`agent_stats_view::tests::agent_project_groups_use_longest_registered_root_and_keep_unknown_unassigned`,
`agent_stats_view::tests::agent_timeline_projects_named_tool_deltas`,
`verify_harness.rs::agent_stats_groups_active_and_inactive_sessions_by_project`,
and `verify_harness.rs::agent_stats_timeline_restores_concrete_tools_failure_reasons_and_compact_cards`.
