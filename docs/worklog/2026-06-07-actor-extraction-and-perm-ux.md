# Worklog: actor extraction + permission UX + phase-7 hardening complete

**Date:** 2026-06-07
**Branches touched (all merged to `master`, then deleted):**
- `session-hardening` → `bd796d4`,`1e2c881` (earlier in the day)
- `perm-ux-devtools` → `03e8d10`
- `actor-extraction` → `23747a0`
- `slow-sub-hardening` → `a70ef74`

Continues the session-server arc from `handover2.md`. Driven largely by
scout→implement→adversarial-review workflows; server-side work verified
headlessly against the resilience + transcript harness (the dev-system oracle).

## Built (with status)

- **Safe→Yolo permission default, now config-driven (ADR-0014 + addendum).**
  First shipped a safe default (`AskEachTime`); reverted to `Yolo` per user
  (AskEach too annoying without an inline-approval UI). `DEFAULT_PERMISSION_MODE`
  = Yolo (no-config fallback); `default-permission-mode` in `config.kdl`; the
  session server loads config at startup and applies it in `create_session` (no
  wire change). `0600` socket (TOCTOU-closed via `umask`-around-`bind`) +
  owner-gated escalation retained. ✅ headless tests (hermetic to config).
- **Permission mode is session state, not channel state (GUI bug fix).** The
  badge/cycle read a local `AcpChannelClient`, empty in the session-server model
  → badge invisible + "cycle says no agent". Now `AgentState.permission_mode`
  sourced from `SessionInfo.permission_mode`; status-strip badge renders from it
  (always visible); `<space> c m` cycles via the `SetPermissionMode` wire verb.
  Plus: "new session" always fresh (was resuming per-cwd first); in-app
  "rebuild & restart gui" (`<space> c g`). ✅ runtime-confirmed by user (badge
  visible at top, cycle works, new-session fresh, 0600 socket).
- **Structured `tracing` + `admin_status` verb (phase-7).** Server `eprintln`→
  `tracing` (stderr, env-filter); additive `admin_status` → `AdminSnapshot`.
  ✅ runtime-confirmed (timestamped/structured log output) + headless test.
- **Actor extraction (phase 3, ADR-0012) — the big one.** Replaced
  `Mutex<HashMap<…, ManagedSession>>` + the poison-tolerant `lock_sessions()`
  with a single hand-rolled `run_manager` task (tokio mpsc `Command` inlet +
  oneshot replies) that exclusively owns the map. Forwarders read lock-free from
  a per-session `watch::Sender<Arc<Vec<Notification>>>` (event_log is already COW
  `Arc`); OwnerChanged moved to a per-session `watch<bool>`; the pump thread owns
  the `AcpChannelClient` by move and forwards generation-stamped Commands (the
  actor never holds the `!Sync` receiver). Behavior-preserving: conn_id ownership
  unchanged, no wire change. Both reviewed blockers resolved (A: old channel
  Dropped off-actor to avoid the worker-join deadlock; B: generation fence drops
  late messages after restart). Landed in 7 harness-green checkpoints. ✅
  harness green 5× incl. crash-recovery/mid-turn-reconnect/large-replay/
  restart-storm; **test files unchanged** (green against the original oracle);
  two adversarial reviews SOLID.
- **Slow-subscriber disconnect (phase-7).** Every server→client write
  (per-session forwarder, manager-level session-list forwarder, response writer)
  bounded by a timeout (default 60s; `YALDA_SLOW_SUB_TIMEOUT_MS`). A stuck peer
  is dropped → reconnects + replays from the watch snapshot; owner never gapped
  (log is source of truth). ✅ test reaps a non-draining observer while the owner
  completes its turn; green full-harness ×2 + isolated ×3.

Final master: resilience 8 · transcript 6 · server-unit 2 · config 18 ·
acp_channel 11 — all green.

## Decisions

- **ADR-0014 addendum:** default reverted to Yolo, config-driven.
- **ADR-0015:** "run with no GUI" includes *starting* work headlessly (yes) —
  implies an admin/CLI enqueue-prompt verb (READY, needs a short spec).

## Open / unresolved

- **Phases 4–5 (lease ownership + cursor reconnect)** — still open; now unblocked
  by the actor (conn_id ownership is isolated behind the `Command` inlet). Needs
  the event-stream `seq` work. The actor's generation fencing is a stepping stone.
- **GUI stale-session robustness** — when the GUI's persisted session list
  outlives the server's, attach failures read as "new sessions failing" (hit this
  session; worked around via clean-room reset). The GUI should drop/ignore unknown
  session ids on startup rather than churn. `READY`, GUI-side, NEEDS-RUNTIME.
- **In-app rebuild + permission-mode-on-reconnect** — `dev_rebuild_restart_gui`
  and the permission badge after a sid-only reconnect (shows default until
  re-synced) are NEEDS-RUNTIME (GPUI not headless-drivable).
- **WAL compaction/snapshot + mid-turn-crash fence** — still deferred (ADR-0009).

## Verification status

- All server-side work headless-verified via the harness (the actor extraction's
  race-prone paths green 5×). The permission-UX GUI bits runtime-confirmed by the
  user this session; the in-app rebuild + reconnect-badge remain NEEDS-RUNTIME.

## Next

- Lease ownership + cursor reconnect (phases 4–5) on top of the actor.
- Spec + build the headless enqueue-prompt verb (ADR-0015).
- GUI stale-session-id robustness; tracing/admin_status could now also surface
  per-subscriber lag for proactive slow-subscriber visibility.
