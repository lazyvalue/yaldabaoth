# Worklog: Cog topic browser and agent mail

**Date:** 2026-08-23
**Branches touched:** `codex/cog-topic-browser` (`cc3c8e9`) — topic-first Cog UX implementation and contract reconciliation

## Cog execution evidence

- Graph id: `q3u`

### Initial render

Shown before tracked implementation edits:

```text
graph cog-topic-browser (frontiers)
frontier 0: spec-contract [open]
frontier 1: domain-client [open]
frontier 2: topic-shell [open]
frontier 3: agents-mail [open]
frontier 4: verify-reconcile [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `pc6l` `spec-contract`: claimed → closed; output: `{"summary":"Captured the request and specified UXI-Cog-13..15","files":["docs/backlog.md","docs/components/cog.md"]}`
- `i75s` `domain-client`: claimed → closed; output: `{"summary":"Added typed Topic/address/delivery/Mail/List models, deterministic hierarchy construction, CLI readers, and parsing tests","tests":"23 cog tests passed"}`
- `4751` `topic-shell`: claimed → closed; output: `{"summary":"Built the default Topics explorer, typed detail surfaces, and graph drill-in/back","tests":"hierarchy/detail and cached-body guards passed"}`
- `kfsz` `agents-mail`: claimed → closed; output: `{"summary":"Built the Agents directory, presence/delivery detail, readable mail, and empty/error states","tests":"agent delivery/mail production-path guard passed"}`
- `prox` `verify-reconcile`: claimed → closed; output: `{"summary":"Observed both new guards RED then GREEN, ran 704 GUI tests and the GUI build, reconciled docs, backlog, and worklog"}`
- `in2q` `omega`: claimed → closed; output: `{"summary":"Confirmed implementation, verification, artifacts, and integration are complete"}`

### Notes

- Node `i75s`, seq `2`, topic `finding`: the README documents recursive root discovery with `cog topic list ""`, but the current sibling debug CLI/server rejects the empty prefix as an invalid `TopicPath`; the installed PATH CLI is older and lacks topic/address/mail commands. The UI targets the documented contract and retains a retryable error surface until Cog deployment aligns.
- Node `4751` output deviation: shared Agents-tab scaffolding landed at the end of the topic-shell node because both slices use the same cached Home/tab state; the behavior was completed and verified under `kfsz`.

### Final status

- Status: `complete`

```text
graph cog-topic-browser (frontiers)
frontier 0: spec-contract [done]
frontier 1: domain-client [done]
frontier 2: topic-shell [done]
frontier 3: agents-mail [done]
frontier 4: verify-reconcile [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- `SHIPPED` on `codex/cog-topic-browser`: the Cog tile now opens on an expandable hierarchical Topics explorer and keeps the selector visible while Graph, Note/Bulletin, or Mailing List detail renders on the right.
- `SHIPPED`: graph leaves enter the existing full graph Overview/node/live-event viewer; Back restores the prior Topics tree selection and expansion.
- `SHIPPED`: an Agents tab lists active then retired registered routes with presence and renders binding metadata, delivery cursor/retry/block state, and globally readable direct Mail. All new actions are read-only and asynchronous behind the existing request guard.
- `/new-ux` materially shaped the result by requiring explicit `UXI-Cog-13..15` contracts, production-path headless guards, observed-RED controls, and backlog/runtime reconciliation.

## Open / unresolved

- `NEEDS-RUNTIME`: native GPUI pixel/layout review of the first draft; tracked in [the backlog](../backlog.md).
- `NEEDS-RUNTIME`: live Topic/address/mail data cannot load until the installed `cog`/`cogd` ship the README commands and the root Topic-list contract accepts an empty prefix. This is external to the Yaldabaoth UI and is also recorded in the backlog.

## Decisions

- No ADR added. The local design choice is recorded in `UXI-Cog-13..15`: one persistent read-only two-pane browser, with graph drill-in preserving the existing graph viewer.

## Verification status

- `cargo test --bin yalda-gpui`: **704 passed, 0 failed, 2 ignored**.
- `cargo test --bin yalda-gpui cog_`: **25 passed, 0 failed** after restoration.
- `cargo build --bin yalda-gpui`: passed.
- Observed-RED hierarchy control: disabling descendant flattening failed with 2 visible rows instead of 6; restored GREEN.
- Observed-RED mail control: suppressing populated entry cards failed because `cog-agent-mail-entry` did not paint; restored GREEN.
- `git diff --check`: passed before the implementation commit.
- Native pixels and the live new Cog data path were not runtime-verified for the two concrete reasons above.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-23-cog-topic-browser.md` passes.

## Next

- Restart the release GUI and review the Topics/Agents composition visually.
- Upgrade/restart the operator-owned Cog deployment once the new CLI/server root-list mismatch is resolved, then exercise live Topic, Agent, and Mail payloads end to end.

