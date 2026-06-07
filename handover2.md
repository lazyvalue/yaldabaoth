# Handover 2 — Session server: resilience → durability → supervision

Scope: the session-server work done in this arc, and what remains. Goal driving
it all: **agent sessions keep running on the server with no GUI attached, and the
GUI can restart with zero interruption.**

## Done (on `master`)

| Commit | What |
|---|---|
| `81ae216` | **Reconnect-storm root cause + fix.** `SessionServerClient` never `shutdown` its socket on drop → reader thread leaked, server never saw disconnect, never released ownership → re-attach rejected ("another GUI already owns") → 489-reconnect storm. Fixed + single-instance guard + socket-scoped pid/state paths + headless harness `tests/session_resilience_test.rs`. |
| `bf7cb0e` | **Fire-and-forget flush fix.** `prompt()`/`cancel`/… return before the writer flushes; the drop-shutdown raced it → a prompt sent right before the GUI left was lost. Writer now drains+flushes, *then* shuts down. |
| stub merge | **Headless ACP stub agent** (`src/bin/sketch-acp-stub.rs`) + `tests/session_transcript_test.rs` (prompt round-trip, 800-chunk large-replay reconnect, mid-turn reconnect, turn-completes-with-no-subscriber). The session server is now fully drivable headlessly. |
| `a4acfd0` | **`docs/specs/spec-session-server-actor.md`** — north-star: actor/single-writer model, lease ownership, durable substrate, hardening, 8-phase rollout. Adversarial-reviewed (REVISE findings folded in). |
| `2bbc2a1` | **ADR-0012** — surveyed actor frameworks (actix/ractor/kameo/xtra/coerce/riker); decision: **hand-roll** the actor (bare `tokio::mpsc` + `oneshot`). Reach for kameo only if it grows. |
| `ec3a504` | **Durable WAL (ADR-0009)** — `src/session_wal.rs`. Per-session append-only NDJSON; write-immediately + fsync at turn boundaries; recovery by replay (torn-line tolerant); agent re-adoption via `session/load`. Replaces JSON-snapshot-on-clean-shutdown. Verified by `session_recovered_after_server_crash` (SIGKILL → restart → transcript recovers). |
| `1051bc4` | **launchd supervision (ADR-0013)** — `sketch-session-server install\|uninstall\|status`. LaunchAgent `RunAtLoad` + `KeepAlive{SuccessfulExit=false}`; start-at-login + restart-on-crash. `install` hands off a running server losslessly via the WAL. |

**"Runs with no GUI" is now functionally complete:** survives GUI exit · completes
turns unattended · prompt-then-leave is durable · survives a server **crash**
(WAL) · **always-present + crash-restarted + start-at-login** (launchd).

## NEEDS-RUNTIME (not runnable headlessly / not run on the dev box)

1. **launchd install** (`1051bc4`): `install`/`uninstall` shell out to `launchctl`
   and modify the user's launchd domain + start a real daemon — NOT executed in
   dev. Verify once:
   ```
   sketch-session-server install
   launchctl list com.sketch.session-server      # loaded
   sketch-session-server status                  # installed/loaded/listening
   # kill the server pid → confirm KeepAlive restarts it
   # create a session before the kill → confirm it recovers (WAL)
   ```
2. **GUI reconnect seam** (`81ae216`): the GPUI app reconnecting seamlessly after
   the server reader sees EOF — reuses the proven off-thread attach, but GPUI
   isn't headless-drivable. Repro: live session → `kill` the server → confirm the
   session re-attaches with no flapping and no "another GUI already owns" in
   `~/Library/Caches/sketch/session-server.log`.

## Remaining work (prioritized)

### 1. Security hardening — safe-default permission mode (real foot-gun TODAY)
`create_session` defaults `permission_mode = PermissionMode::Yolo` (auto-approve
tool calls — file writes, shell; `acp_channel.rs:~542`). Any same-uid process can
connect to `/tmp/sketch-session-$USER.sock` and drive an auto-approving agent.
- **Do:** default to a SAFE permission mode; escalate to Yolo only on explicit
  user action. Assert socket mode `0600`.
- **Skip:** the capability token — it's theater for the single-user threat model
  (ADR analysis in spec § Authorization). Permission mode is the real control.
- Headlessly testable (stub agent + a permission-request path).

### 2. Actor extraction (phase 3 — ADR-0012)
Replace `Mutex<HashMap<ServerSessionId, ManagedSession>>` in
`src/bin/sketch-session-server/main.rs` with a single Manager task + `Command`
enum inlet (hand-rolled `tokio::mpsc` + `oneshot`). Mechanical, behavior-
preserving (keep `conn_id` ownership for now). Kills the shared-mutex race class
and the poison-tolerant-lock hack. **Watch the sync→async bridge:** the per-
session pump owns a non-`Sync` `std::sync::mpsc::Receiver` (why pumps are OS
threads today) — it must forward events as `Command::Record` into the inlet via a
`Send` `Sender` clone; the actor never holds the receiver (spec § Data Model
"transport bridge"). NOT a no-GUI blocker — internal correctness.

### 3. Rest of phase-7 hardening
Bounded queues + slow-subscriber disconnect (per-backlog, not subscriber-count);
structured `tracing` + an `admin_status` query verb (today diagnosis is grepping
eprintln). See spec § Behaviors.

### 4. Lease ownership + cursor reconnect (phases 4–5)
Replace `owner: Option<conn_id>` with a `Lease{client_id, expires_at}` +
heartbeat (deterministic reconnect, retires `attach_with_owner_retry`); carry
`(generation, seq)` on attach for incremental resume instead of full from-0
replay. Cross-link the `OwnerChanged → LeaseChanged` rename to spec-event-stream
§12's one migration. Needs the event-stream `seq` work.

### 5. WAL compaction/snapshot (deferred, ADR-0009)
Plain append-only log replayed in full today. Add snapshot + tail when a long
session measurably hurts memory or recovery latency. Also untested: **mid-turn
crash recovery** (crash with `turns==0` → `replay_fence==0` → the agent's
`session/load` replay could double-log without a fence). Add a test when seq work
lands.

## Open decision (needs the user)

**Should "run with no GUI" include *starting* work headlessly?** Today a prompt
requires an owner GUI to send it, so "no GUI" means *finish what was started*,
not *start autonomously*. Cron/automation enqueuing a prompt to an unowned
session would need an admin/CLI verb. Product call — unanswered.

## Map

- `src/session_wal.rs` — durable WAL (lib, unit-tested).
- `src/session_client.rs` — GUI-side client; `Drop`/writer teardown, reconnect, `attach_owner_with_retry`.
- `src/session_proto.rs` — wire types + path helpers (`socket_path`, `pid_file_path`, `session_server_persist_path`, `session_wal_dir` — all follow `SKETCH_SESSION_SOCKET`).
- `src/bin/sketch-session-server/main.rs` — the daemon (`ManagedSession`, `record`/`log_only`/`wal_append`, recovery, single-instance guard, clap dispatch).
- `src/bin/sketch-session-server/launchd.rs` — LaunchAgent install/uninstall/status.
- `src/bin/sketch-acp-stub.rs` — test-support ACP agent (`STUB_CHUNKS`/`STUB_DELAY_MS`/`STUB_CHUNK_TEXT`/`STUB_REPLAY_USER`).
- `tests/session_resilience_test.rs` (no-op agent: socket/ownership/reconnect) · `tests/session_transcript_test.rs` (stub agent: transcript/replay/crash recovery).
- Specs/decisions: `docs/specs/spec-session-server-actor.md`, `docs/specs/spec-event-stream.md`; ADR-0009 (WAL), ADR-0012 (hand-roll actor), ADR-0013 (launchd).
- Worklogs: `docs/worklog/2026-06-07-{durable-wal,launchd-supervision}.md`.

## How to verify everything (headless)

```
cargo test --test session_resilience_test --test session_transcript_test -- --test-threads=1
cargo test --lib session_wal
cargo test --bin sketch-session-server
```
Pre-existing unrelated breakage: `tests/tree_test.rs` fails to compile on `master`
(tree-sitter `TreeState::edit` API drift) — not part of this work.
