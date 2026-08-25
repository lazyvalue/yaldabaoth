# Worklog: agent and repository telemetry v1

**Date:** 2026-08-25
**Branches touched:**
- `agent-stats-v1` (`4a55e61`) — verified implementation
- `main` (`d8c2040`) — implementation merge

## Cog execution evidence

- Graph id: `p62`
- Roadmap graph: `b68`
- Cog namespace: `%projects/cog/telemetry`

### Initial render

Shown before tracked-file implementation began:

```text
graph telemetry-v1 (frontiers)
frontier 0: spec_v1 [open]
frontier 1: repository_metrics [open], agent_metrics [open]
frontier 2: stats_view [open], fulcrum_baseline [open]
frontier 3: shell_integration [open]
frontier 4: headless_guards [open]
frontier 5: verify_v1 [open]
frontier 6: worklog_v1 [open]
frontier 7: omega [open] (omega)
```

### Node execution

- `1asx` `spec_v1`: claimed → closed; output: `{"contract":"UXI-AgentStats-1..4","scope":"repository-neutral two-tab singleton"}`.
- `4obx` `repository_metrics`: claimed → closed; output: `{"scanner":"generic bounded read-only Git census","fulcrum":{"tracked":5686,"source":3468}}`.
- `k70g` `agent_metrics`: claimed → closed; output: `{"metrics":"fleet and per-agent state, turns, tools, context, cost","unknowns":"unavailable"}`.
- `dwn5` `stats_view`: claimed → closed; output: `{"tabs":["Agents","Repository"],"cache_guard":"passed"}`.
- `w3xp` `fulcrum_baseline`: claimed → closed; output: `{"commit_p50":"4 files / 1 root","commit_p90":"23 files / 5 roots","conclusion":"no broad layout failure proven"}`.
- `8o8a` `shell_integration`: claimed → closed; output: `{"entry_points":["System jump row","Cmd-P"],"identity":"one persistent singleton"}`.
- `44lg` `durable_telemetry`: added for the reboot requirement, claimed → closed; output: `{"path":"~/.yalda/telemetry/v1.json","retention":"bounded","restore_before_refresh":true}`.
- `pu0u` `headless_guards`: claimed → closed; output: `{"focused":"8 passed","negative_controls":4,"UXI-AgentStats":"1..5"}`.
- `jv28` `verify_v1`: claimed → closed; output: `{"build":"passed","gpui":"741/0/2","lib":"181/0/2","mutants":"7 caught, 0 missed"}`.
- `stt8` `worklog_v1`: claimed → closed; output: `{"worklog":"validated","feature_commit":"4a55e61","main_merge":"d8c2040","main_verification":"passed"}`.
- `vgfz` `omega`: claimed → closed; output: `{"status":"complete"}`.

### Notes

- Graph `p62`, seq `3`, topic `decision`: v1 reports only values supported by current events; missing durations, token classes, and outcomes remain explicitly unavailable.
- Graph `p62`, seq `6`, topic `decision`: repository analysis is generic; Fulcrum is the primary qualification corpus, never a product-code special case.
- Graph `p62`, seq `27`, topic `deviation`: the user explicitly authorized implementation to continue during Cog maintenance; outputs were reconciled into the original nodes after restoration.
- Graph `p62`, seq `44`, topic `decision`: the reboot-persistence requirement superseded the earlier live/static-only boundary and added durable telemetry to v1.

### Final status

- Status: `complete`

```text
graph telemetry-v1 (frontiers)
frontier 0: spec_v1 [done]
frontier 1: repository_metrics [done], agent_metrics [done]
frontier 2: stats_view [done], fulcrum_baseline [done]
frontier 3: shell_integration [done]
frontier 4: durable_telemetry [done]
frontier 5: headless_guards [done]
frontier 6: verify_v1 [done]
frontier 7: worklog_v1 [done]
frontier 8: omega [done] (omega)
```

## Built (with status)

- **Shipped:** one persistent Agent Stats system tile, immediately below System Console and reachable through Cmd-P, with exactly `Agents` and `Repository` tabs.
- **Shipped:** honest fleet aggregates and per-agent state, turn, tool-call/failure, context-occupancy, optional-cost, and current-turn values from data Yalda actually has.
- **Shipped:** generic bounded repository census, distributions, instruction/manifests, large-source, and recent-churn projections. Scans retain metadata, not source contents.
- **Shipped:** versioned atomic persistence at `~/.yalda/telemetry/v1.json`, bounded to 512 agent observations and 64 repository analyses. Missing, corrupt, or unknown-version data fails safe; restored observations are timestamped and labeled `Restored observation` until live collection replaces them.
- **Qualified against Fulcrum:** 5,686 tracked files, 3,468 recognized source files, 9 instruction files, 37 manifests, and 2,690 paths touched across 500 recent commits. Static evidence found two likely stale paths and two dependency/hotspot candidates, but did not establish broad repository-layout failure.

## Open / unresolved

- [Deeper agent and repository telemetry](../backlog.md) is `DEFERRED` on roadmap `b68`: exact lifecycle/tool durations, provider token classes, normalized outcomes, empirical navigation traces, and evidence-based frustration/failure analysis.
- Exact pixels/colors and a live external agent subprocess remain optional runtime observations. State, persistence, loading, singleton, and cache behavior are covered headlessly.
- Main already contained unrelated user changes to `Cargo.toml`, `Cargo.lock`, `.claude/scheduled_tasks.lock`, and Cog WAL files; they were preserved and are not part of this work.

## Decisions

- No ADR added. The v1 truthfulness, generic-repository, and reboot-persistence decisions are recorded in Cog graph `p62`; future telemetry architecture remains on `b68`.

## Verification status

- Feature branch: required binaries built; `cargo test --bin yalda-gpui --no-fail-fast` reported 741 passed, 0 failed, 2 ignored; `cargo test --lib --no-fail-fast` reported 181 passed, 0 failed, 2 ignored.
- Focused: 8 Agent Stats tests and 7 telemetry-store tests passed.
- Mutation gate: 7 representative mutants caught; 0 missed, timed out, or unviable. Four manual negative controls also observed the new production-path guards fail red.
- Merged main (`d8c2040`, dirty only from the preserved unrelated files): both required binaries rebuilt; the full GPUI suite again reported 741 passed, 0 failed, 2 ignored; the library suite again reported 181 passed, 0 failed, 2 ignored.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-25-agent-repository-telemetry-v1.md` passes.

## Next

- Accumulate the durable v1 baseline, then use real task traces to decide which roadmap metrics best explain agent navigation cost, delays, and failures.
