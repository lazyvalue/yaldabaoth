# bug-0028: archive-tears-down-the-gui-connection

**Status:** FIXED (needs a session-server restart to take effect)
**First seen:** 2026-08-04
**Component:** `docs/components/jump-panel.md` (`UXI-JumpPanel-16`),
`docs/specs/spec-session-server-actor.md`

## Symptom

Reported as *"when I unarchive a session it has trouble starting up again."*

The user's own system console shows the pattern — unarchive, then immediately
re-archive, twice in a row:

```
INFO	unarchived agent session "ers queue drain"
INFO	archived agent session "ers queue drain"
INFO	unarchived agent session "interaction timeline view"
INFO	archived agent session "interaction timeline view"
```

No `could not unarchive agent session` error was logged, so the `SetArchived`
request itself succeeded every time. The failure is downstream of the ACK.

## Context / root cause

**Archiving one session tore down the GUI's entire session-server socket.**

`do_set_archived`'s archive branch released the session's forwarder by setting
`ForwarderHandle::evicted`. But `evicted` is not a general "stop this forwarder"
flag — it is the **high-water kill flag** (spec §6), and its handler in
`forward_notifications::tail_snapshot` does:

```rust
let _ = writer.lock().await.shutdown().await;
```

`writer` is the **per-connection** write half (`stream.into_split()`, wrapped in
one `Arc<Mutex<_>>` and cloned to every session forwarder on that connection).
Shutting it down closes the socket for *every* session the GUI has attached, by
design — the high-water path wants the client to see EOF and reconnect from
base.

So the sequence was:

1. Archive session A → `evicted` set on A's forwarder.
2. `publish_snapshot()` (the next line) wakes that forwarder immediately.
3. It sees `evicted`, shuts down the shared write half → **the whole GUI
   connection drops**; every attached session reconnects from base.
4. The user unarchives A. The server respawns A's agent correctly, but the GUI
   is mid-reconnect with stale attachments, so A (and everything else) looks
   like it is failing to start.

The damage is done at **archive** time; unarchive is just when it becomes
visible. This is also a strong candidate for the long-standing reconnect-storm
observation (hundreds of client reconnects) — every archive was one teardown.

Contributing factor: the unarchive → respawn path had **no test at all**. The
existing `archive_releases_runtime_state_and_wal_but_keeps_durable_session`
covers only the archive direction, and asserts nothing about the forwarder's
effect on the connection.

## Fix (as shipped)

Give "stop one session's forwarder" its own signal, distinct from "kill this
connection":

- `ForwarderHandle::released` — new flag alongside `evicted`.
- `forwarder_stop_action(&ForwarderHandle) -> Option<ForwarderStop>` — a pure
  mapping to `ThisSessionOnly` / `ShutdownConnection`. `released` is checked
  first so a handle carrying both cannot escalate to a teardown.
- `tail_snapshot` matches on it: `ShutdownConnection` keeps the existing socket
  shutdown; `ThisSessionOnly` returns quietly, leaving the shared writer alone.
- `do_set_archived` sets `released`, not `evicted`.

Extracting the predicate is what makes this guardable at all — the effect
otherwise only exists inside an `async` task holding a real Unix socket.

## Approaches already tried (do NOT repeat)

- Do NOT "fix" an apparently-stuck unarchived session by force-restarting it.
  The respawn was never the broken part; the connection underneath it was.
- The `session/load` → `session/new` fallback was investigated and cleared: the
  `ReplayComplete` marker is deliberately emitted on every fallback path
  (`acp_channel.rs`, "resume-hang bug, take two"), so the replay fence does
  clear. That is not this bug.

---

## Log

### 2026-08-04 — root-caused and fixed (attempt 1)

Traced from the report through the console log (request ACKed, so not the wire
call), then through `do_set_archived` → `forwarder.evicted` → `tail_snapshot` →
`writer.lock().await.shutdown()`, and confirmed `writer` is connection-scoped by
its construction at `stream.into_split()`.

**Guard.**
`archiving_one_session_stops_its_forwarder_without_killing_the_connection`
drives the real `do_set_archived` against a session holding a real
`ForwarderProgress`, then asserts the resolved stop action is `ThisSessionOnly`,
that `evicted` is untouched, and that the action is never
`ShutdownConnection`. It also pins both directions of the mapping that must not
regress: a genuinely evicted handle still resolves to `ShutdownConnection`, and
an unflagged handle keeps streaming.

**Negative control observed.** Restoring `forwarder.evicted.store(true, …)` in
`do_set_archived` failed with `left: Some(ShutdownConnection), right:
Some(ThisSessionOnly)` — the archive resolving to a connection teardown, which
is the bug stated exactly. Restoring `released` returned it to green.

**Suites.** session-server 49, GPUI harness 518, library 162 — all green.

**Not done until the daemon restarts.** This is server-side, and the running
daemon (started 2026-08-03 13:54) predates the fix.

**Still unverified.** The socket teardown itself is reasoned from code, not
observed live: reproducing it needs a real client connection with two attached
sessions, archiving one, and watching the other's stream drop. If the symptom
survives a restarted daemon, that live repro is the next step — the honest gap
here is the async/socket path, not the flag logic.
