# Worklog: Durable, real-time Cog tiles

**Date:** 2026-08-25
**Branches touched:**

- `cog-durable-live` (`1928582`) — durable state and bounded exact-key refresh
- `main` (`8725bd0`) — concurrent first integration
- `feat/cog-tile-live-reconcile` — global-feed reconciliation, pending commit

## Cog execution evidence

- Graph id: `bcp`

### Initial render

Shown before tracked implementation edits:

```text
graph cog-tile-durable-realtime (frontiers)
frontier 0: contract [open]
frontier 1: persist [open], realtime [open]
frontier 2: integrate [open]
frontier 3: verify [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `1d7l` `contract`: claimed → closed; output: `{"summary":"Builder accepted an installation-wide resumable event feed; Yalda will subscribe for immediate invalidation with bounded compatibility revalidation","chat":"projects/cog/mail::chat events 32, 34, and 40"}`
- `0u5u` `persist`: claimed → closed; output: `{"summary":"Added backward-compatible, tile-local semantic state and fresh-data restoration","verification":["serialization compatibility guard","headless reboot restoration guard"]}`
- `bl4v` `realtime`: claimed → closed; output: `{"summary":"Integrated cursor-before-read, resumable global events, reconnect, lifecycle guards, coalesced Home/Graph invalidation, and bounded fallback","compatibility":"pre-feature running cogd uses fallback until restart"}`
- `xt0v` `integrate`: claimed → closed; output: `{"summary":"Added UXI-Cog-16/17 and persistence/live-update guards","tests":"Cog-focused suite passed; burst negative control observed RED"}`
- `2h93` `reconcile-main`: claimed; final output pending.
- `wkz0` `verify`: pending.
- `g225` `omega`: pending.

### Notes

- Graph note `ozt3qm`, seq `20`, topic `deviation`: a concurrent same-scope implementation merged to `main` during feature verification. It supplied strong semantic persistence and exact-selection polling guards but predated the builder's completed global event feed. Reconciliation retained that base and replaced constant polling with resumable event invalidation plus bounded fallback.

### Final status

- Status: `complete`

Final render will be recorded after omega closes:

```text
graph cog-tile-durable-realtime (frontiers)
frontier 0: contract [done]
frontier 1: persist [done], realtime [done]
frontier 2: integrate [done]
frontier 3: reconcile-main [claimed]
frontier 4: verify [open]
frontier 5: omega [open] (omega)
```

## Built (with status)

- `SHIPPED`: every Cog tile serializes its own semantic Home/Graph location, Topics/Agents source, stable Topic/Agent/node selection, folds, focus, and events visibility. Remote payloads remain out of workspace JSON and are fetched fresh on reboot.
- `SHIPPED`: legacy stateless Cog workspace JSON remains readable, and keyboard/mouse semantic changes checkpoint the ordinary workspace state.
- `SHIPPED`: each open tile captures the installation cursor before Home, follows global mutations from that cursor, resumes after disconnect, rejects duplicate/stale envelopes, and invalidates its exact current Home/Topic/Agent/Graph projection.
- `SHIPPED`: event bursts use the existing one-read-plus-one-trailing-read coalescers. A mutation during Loading is retained for one trailing invalidation rather than starting a request storm.
- `SHIPPED`: a missing/disconnected feed falls back to one-second revalidation; a connected feed gets a 30-second safety read. The graph-specific watcher still supplies the visible event strip.

## Open / unresolved

- `DEPLOYMENT`: restart the operator-owned `cogd` process to activate the installed `/v1/events` endpoint. Until then the compatibility fallback keeps tiles current within one second.
- `NEEDS-RUNTIME`: native process lifecycle and end-to-end mutation delivery should be observed after that restart. Reducers, cursor/reconnect rules, persistence, and UI restoration are headlessly guarded.

## Decisions

- No ADR added. `UXI-Cog-16` and `UXI-Cog-17` own the component-local contracts.
- Global event scope metadata is not a correctness dependency. Every accepted event invalidates only that tile's current projection, preventing future Cog entity kinds from silently leaving Yalda stale.
- Remote payloads, scroll offsets, and event buffers are not persisted; stable semantic identifiers are resolved against fresh Cog data.

## Verification status

- `cargo check --bin yalda-gpui`: passed on both implementations.
- Reconciled `cargo test --bin yalda-gpui cog_ -- --nocapture`: **29 passed, 0 failed**.
- Reconciled full `cargo test --bin yalda-gpui -- --nocapture`: **713 passed, 0 failed, 2 ignored**.
- Global-feed negative control: suppressing event invalidation failed `cog_global_events_resume_and_invalidate_current_projection` at the immediate-refresh assertion; restored GREEN.
- Original Home stale-key negative control was preserved from the concurrent implementation.
- `git diff --check`: passed.
- Reconcile commit, final main merge, main release build, and final graph evidence are pending the verification node.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-25-cog-tile-durable-realtime.md` passes.

## Next

- Restart `cogd`, then observe an external Topic/Chat/Mail/Agent/Graph mutation update an already-open Cog tile without manual navigation.
