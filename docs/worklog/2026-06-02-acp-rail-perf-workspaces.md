# Worklog: ACP durability, rail, perf, workspaces, dev-system

**Date:** 2026-06-02
**Branches touched:**
- `master` — fast-forwarded `f282130` (base) → `8036ccf` (= `integration`)
- `rail-fixes` (`bf0b791`), `perf` (`9a41bfc`), `perf-tachyon` (`cc31808`), `workspaces` (`b7146e4`)
- `integration` (`8036ccf`) — combined buildable branch
- follow-ups off `integration`: `ff-server-perf` (`7a352ea`), `ff-editor-perf` (`42b4507`), `ff-ui-threading` (`c7b138f`), `ff-buffer-pool` (in flight)

## Built (with status)
- **ACP durability + consistency** (base `f282130`): pending-drain fail-fast, `reconnect()` + pump backoff + resubscribe, manager-level `SessionClosed/Renamed/Created` broadcasts, confirm-then-mutate close, duplicate-attach guard. Builds + 42 lib/64 bin tests. ADR-0003. **Reconnect path NOT runtime-verified.**
- **Rail fixes** (`rail-fixes`): rail renders beside the focused pane (was window-edge), higher-contrast entries, command-menu items. Builds + tests. **Runtime-confirmed by user: placement fixed, menu items present.**
- **Perf** (`perf` synthesis of 3 branches, then `perf-tachyon` +S1/S2/S3): O(1) streaming, advance_fence, in-place anchor shift, coalesced reply apply, ~1Hz anim tick, drain-then-sleep server pump, coalesced socket writes, memoized render_agent view-model (+guard tests). Builds + 66 bin/42 lib tests. ADR-0004. **Perf gains NOT runtime-verified.**
- **Workspaces** (`workspaces`): Tabs→Workspaces (strings), move pane (Ctrl-W m), also-show docs (Ctrl-W M), multi-home dot. Builds + 69 tests. ADR-0002. **Chords/dot need runtime check; also-show reads from disk (pool unwired).**
- **Follow-ups (off `integration`):** `ff-server-perf` Arc event_log (#6, done); `ff-editor-perf` delta undo + insertion cache (#4/#9, done, +10 tests); `ff-ui-threading` off-thread open/attach (S4, done — open is now instant); `ff-buffer-pool` (wire the pool — in flight). All build + green; **not folded to `integration` yet, none runtime-verified.**
- **Docs:** `spec-rail.md`, `spec-workspaces-tagging.md` (research), `docs/research/refactor-review-perf-hot-path.md`, and the **dev system** (`docs/dev-system.md`, `decisions/`, `worklog/`, `backlog.md`, skills).

## Open / unresolved
See `docs/backlog.md`. Headlines: **verification harness** (top), fold `ff-*` perf/cleanup branches, hold behavior-changing `ff-buffer-pool`/`ff-ui-threading` for runtime review, retarget `/refactor`'s Fulcrum preamble, agent multi-membership (`NEEDS-DECISION`), server lock-sharding #7 (deferred).

## Decisions
- ADR-0001 worktree workflow · 0002 workspaces model · 0003 ACP durability/consistency · 0004 perf O(changed) + synthesis-over-fan-out · 0005 shared-content Core/View pool (buffer pool deferred).

## Verification status
Everything builds + unit-tests green. **Only the rail fixes are runtime-confirmed** (by the user). All perf, workspaces, ACP-reconnect, and follow-up behavior is unverified at runtime — the GPUI app can't be driven headlessly. This is the binding constraint; the harness is backlog item #1.

## Next
- Fold `ff-server-perf` + `ff-editor-perf` (behavior-preserving) into `master`/`integration` after a build check.
- Runtime-test `ff-ui-threading` + `ff-buffer-pool` (behavior-changing) before folding.
- Start the verification harness so future work isn't human-gated.
- Decide agent multi-membership; retarget `/refactor`.
