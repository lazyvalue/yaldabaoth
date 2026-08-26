# Worklog: Readable Agent Stats timeline and tool breakdowns

**Date:** 2026-08-26
**Branches touched:** `agent-stats-readable-timeline` (`7526e1c`, `7d92438`,
`fbd3df4`) → `main` (`b96e940`, `0846bb2`)

## Cog execution evidence

- Graph id: `k2k`

### Initial render

```text
graph agent-stats-readable-timeline (frontiers)
frontier 0: spec_timeline [open]
frontier 1: compact_layout [open], timeline_projection [open]
frontier 2: timeline_ui [open]
frontier 3: headless_guards [open]
frontier 4: verify [open]
frontier 5: worklog_integrate [open]
frontier 6: omega [open] (omega)
```

### Node execution

- `4z5m` `spec_timeline`: claimed → closed; output specified the 1280px
  readable measure, stable row identity, sampled-timeline truth, navigation,
  privacy, and durable v1 history.
- `jmxw` `compact_layout`: claimed → closed; output recorded the centered,
  fluid content bound across agent and repository views.
- `xj2y` `timeline_projection`: claimed → closed; output recorded bounded
  chronological projection, unrelated-churn collapse, and known non-negative
  metric deltas.
- `qg16` `timeline_ui`: claimed → closed; output recorded active/inactive row
  selection, Back/Escape/tab behavior, summaries, event cards, and cache
  ownership.
- `ry1p` `headless_guards`: claimed → closed; output recorded real painted
  geometry and click tests plus width-cap and click-handler negative controls.
- `6spq` `verify`: claimed → closed; output recorded focused/full tests,
  binary builds, diff hygiene, and 14 timeline mutations with no survivors.
- `862g` `tool_contract`: added when the user extended the shipped timeline;
  claimed → closed; output specified named tool deltas, per-agent failure
  breakdowns, optional coverage, cardinality bounds, privacy, and old-v1
  compatibility.
- `zd9l` `tool_collection`: claimed → closed; output recorded provider-name
  normalization, coarse fallback, 64-row deterministic aggregation, additive
  persistence, and payload-exclusion guards.
- `j6sl` `tool_breakdown_ui`: claimed → closed; output recorded the
  latest-known calls/failures/rate table and sampled per-observation tool deltas.
- `k0ny` `tool_guards`: claimed → closed; output recorded a production
  collection → save → fresh GUI restore → painted click test and its
  observed RED negative control.
- `8q56` `tool_verify`: claimed → closed; output recorded 17 focused tests,
  753 GPUI tests, 213 library tests, builds, checks, negative control, and two
  mutation campaigns with no viable survivors.
- `5iwu` `worklog_integrate`: claimed → closed; output recorded feature/main
  commits, merged-main tests, release binaries, preserved unrelated edits, and
  this validated worklog.
- `dsd8` `omega`: claimed → closed; output confirmed the compact layout,
  durable timeline, named tool aggregates/failures, persistence compatibility,
  verification, documentation, and integration were complete.

### Notes

- Graph seq `2`, topic `decision`: keep tabs tile-wide while centering a fluid
  1280px body; use stable row ids and durable sampled observations; Back/Escape
  return to the originating list; do not retain sensitive payloads.
- Graph seq `40`, topic `decision`: loaded sessions have named-tool coverage,
  unloaded sessions are unknown, retained history may preserve latest-known
  coverage, and tool deltas belong to samples rather than exact timestamps.
- The user extended scope after the initial readable-timeline integration, so
  the same graph gained the tool contract, collection, UI, guard, and
  verification chain before final closure.
- A purportedly scoped `cargo fmt` invocation reformatted unrelated files in
  the isolated worktree. The safety reviewer rejected a broad restore; only the
  five intended feature files were staged, so none of that unrelated formatting
  entered either feature commit or `main`.

### Final status

- Status: `complete`

```text
graph agent-stats-readable-timeline (frontiers)
frontier 0: spec_timeline [done]
frontier 1: compact_layout [done], timeline_projection [done]
frontier 2: timeline_ui [done]
frontier 3: headless_guards [done], tool_contract [done]
frontier 4: verify [done], tool_collection [done]
frontier 5: tool_breakdown_ui [done]
frontier 6: tool_guards [done]
frontier 7: tool_verify [done]
frontier 8: worklog_integrate [done]
frontier 9: omega [done] (omega)
```

## Built (with status)

- Agent Stats content now stays within a centered 1280px readable measure on
  wide tiles while remaining fluid on narrower tiles.
- Clicking any active, archived, or unavailable agent opens a durable observed
  timeline with sampled lifecycle/metric changes and honest timing caveats.
- Each agent timeline now identifies named tools in observation cards and shows
  a latest-known aggregate table with calls, failures, and failure rate by tool.
- Loaded sessions collect a bounded normalized tool-name breakdown. Unloaded
  coverage is unknown rather than zero; prior known coverage remains useful.
- Telemetry store v1 gained an additive optional field and remains compatible
  with old v1 documents. Only names and integer counts persist—never call ids,
  inputs, outputs, source, paths, prompts, or error text.
- Feature commits through `fbd3df4` merged to `main` through `0846bb2`; existing
  user changes to Cargo metadata, the scheduled-task lock, and Cog sidecars were
  preserved.

## Open / unresolved

- No implementation blocker remains. Exact human visual review of the new tool
  table against a large live fleet is still useful after restarting Yalda; real
  GPUI paint geometry, selection, persistence, and named-row probes are green.

## Decisions

- No ADR added. This extends the existing Agent Stats telemetry and privacy
  contract additively; it does not introduce a new cross-repository
  architectural decision.

## Verification status

- Merged-main Agent Stats suite: 17 passed.
- Merged-main GPUI suite: 753 passed, 2 intentionally ignored.
- Merged-main library suite: 213 passed, 2 intentionally ignored.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed.
- Real-view guard collects three production tool calls with two failures, saves
  telemetry, boots a fresh browser, clicks the painted agent row, and observes
  named `read`/`bash` aggregate rows plus both event-detail probes.
- Negative control: replacing production summarization with an empty vector
  failed at zero collected calls versus the expected three, then passed after
  restoration.
- Tool normalization/counting mutation campaign: 24 tested, 23 caught, 1
  unviable, 0 survived.
- Tool timeline projection campaign: effectively 12 caught, 2 unviable, 0
  survived after adding the failure-only status-transition guard.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-26-agent-stats-readable-timeline.md`
  passes.

## Next

- Restart Yalda so the rebuilt release binary loads, then inspect a tool-using
  agent to evaluate the live table density and decide whether the next slice is
  filters, tool-duration telemetry, or cross-agent tool aggregates.
