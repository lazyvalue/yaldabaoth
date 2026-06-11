# Worklog — 2026-06-07 — Session-server reconnect storm: root-caused + fixed

Goal the user stated: *agent sessions keep running on the server even with no GUI,
and the GUI can restart with zero interruption.* That architecture already exists
(detached `yalda-session-server` daemon, event-log transcript, re-attach on
reconnect). What broke the "zero interruption" promise was the open
**reconnect-storm** bug (`NEEDS-RUNTIME-REPRO`). This session root-caused and
fixed it. Branch: `session-resilience`.

## Root cause — missing socket shutdown on client drop

`SessionServerClient` had **no `shutdown` on its socket when dropped.** The reader
thread is detached and blocks forever on `lines()`; dropping the client (notably
`reconnect()`'s `*self = fresh`) dropped the `JoinHandle` (detaching, not joining)
and left the socket fd open. So:

- The reader thread **leaked** (one per reconnect), and
- The **server never observed the disconnect** → never ran its connection cleanup
  → **never released session ownership.**

Every in-place `reconnect()` therefore orphaned a zombie connection that still
"owned" its sessions. The new connection's re-attach as `Owner` was rejected with
*"another GUI already owns this session"*, and the old connection only really
closed when the whole GUI process exited. That is precisely the field signature:
**489 "reconnected" vs ~5 "connected", almost no closes**, and `close/create`
round-trips failing with `session server disconnected` when they landed in a
flapping window.

The previously-suspected cause (a disconnect during the large attach-replay due
to broadcast lag) was a **red herring** — `forward_notifications` is already
self-healing (source of truth is `event_log`, `Lagged` just re-tails). The real
cause was the socket never closing.

## Fix (4 source files + 1 test)

1. **`src/session_client.rs` — `Drop` now `shutdown(Both)`s the socket.** Kept a
   `shutdown_handle: UnixStream` clone purely for this. The detached reader's
   `lines()` returns EOF and it exits; the server sees the close and releases
   ownership at once. This is the core fix. (Folded into the existing `Drop` that
   already set `connected=false` + failed pending requests.)
2. **`src/session_client.rs` — `attach_owner_with_retry`** added. After an
   in-place reconnect the new socket can re-attach before the server has finished
   tearing down the old one, so a bare attach still loses the race momentarily.
   Retries on ownership contention only (~1s, 20×50ms), then Observer fallback —
   mirrors the open-path helper but in the lib so it's testable.
3. **`src/bin/yalda-gpui/main.rs` — `reconnect_session_server` re-attaches off
   the paint thread** via the existing `spawn_attach_sessions` (which already does
   the Owner-reclaim retry + per-slot status reconcile), replacing raw inline
   blocking `attach()` round-trips that had **no retry** and also **froze
   rendering**. Signature gained `cx`; the one call site updated.
4. **`src/bin/yalda-session-server/main.rs` — single-instance guard.** If a
   server is already listening on the socket, exit cleanly instead of
   `remove_file` + rebind (which silently steals the socket and orphans the live
   server's sessions). The client auto-launches a server on any failed connect,
   so spurious concurrent launches happen; this makes them harmless.
5. **`src/session_proto.rs` — `pid_file_path` + `session_server_persist_path` now
   follow `YALDA_SESSION_SOCKET`.** Default paths unchanged
   (`/tmp/yalda-session-$USER.{pid}`, cache-dir state). The override enables
   isolated instances (tests) and makes the PID file a real per-socket guard.

## Verification — new headless harness

`tests/session_resilience_test.rs` drives the **real** `yalda-session-server`
binary (`CARGO_BIN_EXE_…`) on a private socket, with `YALDA_ACP_AGENT=/usr/bin/true`
so **no real ACP agent is needed** — `create_session` returns before the handshake
and the session persists in the manager even if the agent fails to spawn, so the
whole connect/attach/reconnect/ownership layer is exercised without Claude.

This is the seam the top-priority **verification-harness** backlog item has been
asking for: the session server is now headlessly drivable.

Tests (all pass; each **reproduced the bug before the fix**):
- `session_survives_client_restart` — drop client, fresh client re-attaches.
- `repeated_restarts_no_storm` — 30 sequential restarts, owner reclaimed each
  time, accepts bounded, **every connection closes (no zombies)**.
- `in_place_reconnect_reattaches` — the GUI pump's `reconnect()` path; re-attach
  reclaims ownership via retry.
- `second_server_does_not_steal_socket` — duplicate server exits, original keeps
  its session.

Invariant the harness pins: **accepts == closes** (a leaked/zombie connection
shows up as accepts > closes). Full suite green except pre-existing
`tree_test.rs` compile break (fails identically on base `1c16056` — unrelated
tree-sitter `TreeState::edit` API drift).

## Still owed — runtime (GPUI not headless-drivable)

The GPUI app's reconnect path (#3) compiles and reuses proven off-thread attach
code, but the end-to-end "GUI reconnects seamlessly after the server reader sees
EOF" needs a human runtime check. Repro: launch GUI with a live session, `kill`
the server (or trigger a server restart), confirm the session re-attaches with no
flapping and no "another GUI already owns" in `~/Library/Caches/yalda/session-server.log`.

## Follow-ups

- Extend the harness with a **stub ACP agent** that streams a real transcript, so
  large-replay reconnect and prompt/turn flows get headless coverage too.
- Reconnect backoff jitter is now unnecessary (the storm came from the leak, not
  the cadence) — left as-is at fixed 1s.
