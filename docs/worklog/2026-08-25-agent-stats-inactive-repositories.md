# Worklog: Agent Stats inactive agents and repository selection

**Date:** 2026-08-25
**Branches touched:**
- `agent-stats-inactive-repositories` (`f7839e0`) — verified implementation
- `main` (`d623531`) — implementation merge

## Cog execution evidence

- Graph id: `9ku`

### Initial render

Shown before tracked-file implementation began:

```text
graph agent-stats-inactive-repositories (frontiers)
frontier 0: spec_followup [open], classify_states [open], repo_catalog [open]
frontier 1: inactive_ui [open], repo_selector_ui [open]
frontier 2: headless_guards [open]
frontier 3: verify_followup [open]
frontier 4: worklog_integrate [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `fgs9` `spec_followup`: claimed → closed; output: `{"tabs":["Agents","Inactive","Repository"],"repositories":"generic registered and retained catalog"}`.
- `u1yj` `classify_states`: claimed → closed; output: `{"states":{"active":["Working","Ready"],"inactive":["Archived","Unavailable"]},"store_version":1}`.
- `w1lh` `repo_catalog`: claimed → closed; output: `{"catalog":"all registered projects plus retained-only analyses","hardcoded_paths":false}`.
- `ouoc` `inactive_ui`: claimed → closed; output: `{"navigation":"click, 1/2/3, arrows, h/l, Tab","cached_child":"self-invalidating"}`.
- `uwq2` `repo_selector_ui`: claimed → closed; output: `{"picker":"bounded keyboard and mouse selector","badge":"Analyzed"}`.
- `m653` `headless_guards`: claimed → closed; output: `{"focused":12,"negative_controls":2,"repositories":["hermetic Yalda","hermetic Fulcrum"]}`.
- `sx0q` `verify_followup`: claimed → closed; output: `{"gpui":"746 passed, 2 ignored","lib":"181 passed, 2 ignored","mutants":"6 caught, 1 unviable"}`.
- `w74z` `worklog_integrate`: claimed → closed; output: `{"worklog":"validated","feature_commit":"f7839e0","main_merge":"d623531","main_verification":"passed"}`.
- `1lai` `omega`: claimed → closed; output: `{"status":"complete"}`.

### Notes

- Graph `9ku`, seq `7`, topic `decision`: Archived is server-authoritative; Unavailable is unarchived and disconnected; Agents averages use Working and Ready only; the repository catalog is generic and persistence remains backward-compatible v1.
- Node `u1yj`, seq `12`, topic `deviation`: the new Archived state required one downstream exhaustive UI color arm, added with coordinator authorization.
- Node `sx0q`, seq `4`, topic `deviation`: Luna review found implicit repository selection was too sticky and a Saved badge overstated write timing; explicit-selection provenance, active-project refocus behavior, an Analyzed badge, and regression coverage resolved both findings. Final Luna review found no remaining repository-selection correctness issue.

### Final status

- Status: `complete`

```text
graph agent-stats-inactive-repositories (frontiers)
frontier 0: spec_followup [done], classify_states [done], repo_catalog [done]
frontier 1: inactive_ui [done], repo_selector_ui [done]
frontier 2: headless_guards [done]
frontier 3: verify_followup [done]
frontier 4: worklog_integrate [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- **Shipped:** Agent Stats now has exactly `Agents | Inactive | Repository`. Agents contains Working and Ready only; Inactive contains distinct Archived and Unavailable rows and counts; fleet averages use the active population and preserve known-value denominators.
- **Shipped:** the Repository page selects from every registered project plus retained analyses. Yalda, Fulcrum, and future repositories use one production path with no name or path special cases.
- **Shipped:** implicit selection follows the active project on singleton refocus until the user picks a repository. An explicit picker choice remains stable across refocus and `r` refresh.
- **Shipped:** accepted scans are gated by generation and normalized selected root, so a late Yalda result cannot overwrite Fulcrum after selection.
- **Preserved:** telemetry remains atomically persisted in the bounded versioned store at `~/.yalda/telemetry/v1.json`; the new archived count is serde-defaulted, so existing v1 documents remain readable across Yalda reboots.

## Open / unresolved

- [Deeper agent and repository telemetry](../backlog.md) remains `DEFERRED` on roadmap graph `b68`: exact lifecycle/tool durations, provider token classes, normalized outcomes, empirical navigation traces, and evidence-based frustration/failure analysis.
- State, keyboard, cache, persistence, selection, and painted row presence are headlessly verified. Exact native pixels were not reviewed by a human and are not required to establish these semantics.
- Main already contained unrelated user changes to `Cargo.toml`, `Cargo.lock`, `.claude/scheduled_tasks.lock`, and Cog WAL files; they were preserved and are not part of this work.

## Decisions

- No ADR added. The state precedence, active-average population, generic catalog, selection provenance, and persistence-compatibility decisions are recorded in Cog graph `9ku`.

## Verification status

- Feature branch: required binaries built; `cargo test --bin yalda-gpui --no-fail-fast` reported 746 passed, 0 failed, 2 ignored; `cargo test --lib --no-fail-fast` reported 181 passed, 0 failed, 2 ignored.
- Focused: `cargo test --bin yalda-gpui agent_stats -- --nocapture` reported 12 passed, including active/inactive partitioning, reboot restoration, generic two-repository selection, explicit-choice stickiness, stale-result rejection, and cached render ownership.
- Mutation gate: 7 representative mutants exercised classification and selected-root predicates; 6 were caught and 1 was unviable. Two manual negative controls observed the active filter and selected-root guard fail RED before restoration.
- Merged main (`d623531`, dirty only from preserved unrelated files): both required binaries rebuilt; the full GPUI suite again reported 746 passed, 0 failed, 2 ignored; the library suite again reported 181 passed, 0 failed, 2 ignored.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-25-agent-stats-inactive-repositories.md` passes.

## Next

- Let the durable baseline accumulate across real Yalda and Fulcrum work, then prioritize deeper lifecycle, token, tool-duration, repository-navigation, and failure-mode instrumentation from observed gaps.
