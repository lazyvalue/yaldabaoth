# Worklog: Project-grouped Agent Stats and actionable tool failures

**Date:** 2026-08-26
**Branches touched:** `agent-stats-project-tool-failures` (`c77bb8a`) →
`main` (`d7bd0fa`)

## Cog execution evidence

- Graph id: `n2r`

### Initial render

```text
graph agent-stats-project-tool-failures (frontiers)
frontier 0: spec_contract [open]
frontier 1: telemetry_context [open]
frontier 2: project_groups [open]
frontier 3: timeline_cards [open]
frontier 4: real_guards [open]
frontier 5: verify [open]
frontier 6: integrate_log [open]
frontier 7: omega [open] (omega)
```

### Node execution

- `mxma` `spec_contract`: claimed → closed; output: specified longest-root
  project ownership, root-session scope, concise sampled cards, and bounded
  failure-diagnostic retention.
- `vrlt` `telemetry_context`: claimed → closed; output: added optional cwd and
  grouped failed-tool diagnostics while preserving telemetry-store v1 loading.
- `9nsp` `project_groups`: claimed → closed; output: grouped both fleet pages
  by registered project with a bounded explicit Unassigned section.
- `rhnc` `timeline_cards`: claimed → closed; output: moved identity out of
  cards, added turn dividers and concrete tool/failure detail, and fixed labels.
- `pn72` `real_guards`: claimed → closed; output: added real painted grouping,
  restore, timeline-detail, turn-divider, and label-geometry guards with three
  observed-RED negative controls.
- `397k` `verify`: claimed → closed; output: checks and 757 GPUI tests passed;
  14 focused mutants produced 10 caught, 4 unviable, and zero survivors.
- `vnpt` `integrate_log`: claimed → closed; output: feature commit `c77bb8a`
  merged to `main` as `d7bd0fa`; merged-main tests and release builds passed.
- `3axt` `omega`: claimed → closed; output: project grouping, actionable
  timelines, persistence compatibility, guards, integration, and release were
  confirmed complete.

### Notes

- Graph seq `2`, topic `decision`: Agent Stats represents root Yalda/ACP
  sessions; provider-internal subagents remain parent tool activity until they
  have stable roster identities. Project ownership uses the longest registered
  root prefix. Provider/model appears once, settled turns divide samples, and
  failed calls retain only one bounded diagnostic without tool inputs or full
  payloads.
- The first copied-tree mutation run was invalid because sandboxed Clang could
  not write its module cache. It was discarded and rerun with valid access in
  serial in-place mode. That run found two guard gaps; explicit vanished-reason
  and same-turn-divider cases were added before the final zero-survivor pass.

### Final status

- Status: `complete`

```text
graph agent-stats-project-tool-failures (frontiers)
frontier 0: spec_contract [done]
frontier 1: telemetry_context [done]
frontier 2: project_groups [done]
frontier 3: timeline_cards [done]
frontier 4: real_guards [done]
frontier 5: verify [done]
frontier 6: integrate_log [done]
frontier 7: omega [done] (omega)
```

## Built (with status)

- Active and Inactive agent lists now use registered-project sections selected
  by longest cwd prefix; unmatched sessions appear under Unassigned.
- Per-agent timelines show provider/model once, settled-turn dividers, named
  tools for each retained sample, latest per-agent tool aggregates, and grouped
  failure reasons with counts.
- Failure diagnostics persist across reboots in the existing v1 telemetry file
  as additive optional fields. Collection ignores inputs and successful output,
  caps each single-line reason at 240 characters, and caps distinct rows at 64.
- Timeline labels no longer word-wrap; multiline tool and diagnostic content is
  rendered as compact detail blocks.
- Feature commit `c77bb8a` is merged to `main` as `d7bd0fa`. Existing user
  changes to Cargo metadata, the scheduled-task lock, and Cog sidecars remain.

## Open / unresolved

- No implementation blocker remains. Provider-internal subagents do not yet
  receive separate fleet rows because there is no universal stable roster
  identity for them.
- Failure excerpts are bounded provider-authored diagnostics, not guaranteed
  secret-redacted strings; this is explicit in the telemetry contract.

## Decisions

- No ADR added. The work extends the existing telemetry v1 schema additively
  and keeps project ownership derived from generic registered roots rather than
  adding Fulcrum-specific behavior.

## Verification status

- Merged-main GPUI suite: 757 passed, 2 intentionally ignored.
- `cargo check --bin yalda-gpui`: passed in the feature worktree.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed
  on merged `main`.
- Real-view guards cover both project-grouped pages and production collection →
  telemetry save → fresh GUI restore → painted timeline details.
- Observed-RED controls disabled project matching, disabled diagnostic
  collection, and restored the old narrow wrapping label.
- Focused mutation gate: 14 tested, 10 caught, 4 unviable, 0 missed or timed out.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-26-agent-stats-project-tool-failures.md`
  passes.

## Next

- Restart the Yalda GUI to load the rebuilt release binary. The session server
  does not need a restart for this UI/telemetry change.
- A later slice can add stable subagent identities, tool durations, filters, or
  cross-agent tool aggregates.
