# bug-0067: leaked-test-servers-block-highlander-boot

**Status:** FIXED
**First seen:** 2026-09-09
**Component:** session server lifecycle (ADR-0037 Highlander guard) / `tests/session_resilience_test.rs`

## Symptom

After `./deploy-server.sh` at 01:39:36 PDT the systemd unit went
`inactive (dead)` after 36 ms (clean exit 0, so `Restart=on-failure` did not
retry). `/tmp/yalda-session-scott.sock` did not exist; the GUI had no server;
Scott: "I'm not seeing any session replaying whenever I go to them."

## Context / root cause

`~/.yalda/session-server.log`:

```
WARN found another yalda-session-server process pid=111336 exe=…/.claude/worktrees/wal-history-streaming/target/debug/yalda-session-server (deleted)
… (×5)
WARN refusing to boot: 5 other yalda-session-server process(es) running (Highlander rule).
```

The five processes were **test servers** from the graph-`aop` resilience
runs (`YALDA_SESSION_SOCKET=/tmp/yalda-restest-*-{disk,cold}.sock`, parent
pid 1 after the test process exited). `spawn_server_on`,
`interior_corrupt_wal_is_skipped_and_the_server_still_boots` and
`v1_wal_session_survives_server_upgrade` held a bare `std::process::Child` and
relied on an explicit `child.kill()` at the end of the test; the
negative-control runs panicked on their assertions first, so the kill never
ran. `TestServer` already reaps in `Drop`; the hand-spawned paths did not.

The Highlander guard did exactly what ADR-0037 says — it counts *any*
`yalda-session-server` process — so the leak turned into a production
outage on the next deploy.

## Planned solution

Reap on drop for every hand-spawned server (`ReapOnDrop(Child)`: kill + wait
in `Drop`), returned by `spawn_server_on` and wrapping the two manual
spawns, plus a guard that panics mid-test inside `catch_unwind` and asserts
the child is gone.

## Approaches already tried (do NOT repeat)

- <none>

---

## Log

### 2026-09-09 01:47 — production unblocked (approved), guard shipped

- Read-only check identified the five PIDs as test servers by their
  `YALDA_SESSION_SOCKET` env (private `/tmp/yalda-restest-*` sockets, zero
  sessions). With Scott's explicit one-shot approval they were SIGTERMed
  (each verified against its socket path before the signal), the stale
  restest sockets removed, and `systemctl --user start yalda-session-server`
  run: `active (running)` 01:47:52, 55 sessions recovered, 26 GUI attaches
  streamed from disk, 0 errors.
- Fix: `ReapOnDrop` in `tests/session_resilience_test.rs`; `spawn_server_on`
  returns it; the interior-corrupt and v1-upgrade tests wrap their children.
- Guard: `hand_spawned_server_is_reaped_when_the_test_panics` — spawns via
  `spawn_server_on` inside `catch_unwind`, panics, then asserts
  `/proc/<pid>` is gone and the socket is dead.
- Negative control: `Drop` body emptied → guard RED
  ("leaked test server pid … survived the panic"). Restored, green.
- Cog graph `cl8`.
