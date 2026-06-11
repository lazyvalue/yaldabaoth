# Worklog — 2026-06-07 — launchd supervision (server always-present)

Phase 7 (supervision slice) of spec-session-server-actor. The last
infrastructure gap for "agents run with no GUI": the server only existed because
a GUI auto-launched it — nothing restarted it if it crashed while no GUI was
around, and it never started at login. Now it's a supervised, start-at-login
LaunchAgent. Branch: `launchd-supervision`. Decision: ADR-0013.

## What shipped

- **`src/bin/yalda-session-server/launchd.rs`** (new bin module): per-user
  LaunchAgent integration.
  - `launch_agent_plist(exe, log)` — pure plist generator (unit-tested). Label
    `com.yalda.session-server`; `RunAtLoad=true`; `KeepAlive={SuccessfulExit=false}`;
    Background ProcessType; std{out,err} → `~/Library/Caches/yalda/session-server.log`;
    a sane `PATH` (the agent-command resolution also falls back through a login
    shell). XML-escapes paths.
  - `install()` — write plist → `launchctl unload -w` (clear any prior) →
    **hand off any running server** (read pid file, SIGTERM, wait for the socket
    to free) → `launchctl load -w`. The handoff is **lossless because of the WAL**
    (ADR-0009): the killed server's sessions are recovered by launchd's
    replacement.
  - `uninstall()` — `launchctl unload -w` + remove plist.
  - `status()` — reports plist present / loaded (`launchctl list`) / socket
    listening.
- **`main.rs`** — `clap` subcommand dispatch: no subcommand → run the server (the
  default the GUI auto-launches); `install` / `uninstall` / `status` manage
  supervision and exit.

## Why this shape (ADR-0013)

`RunAtLoad` + `KeepAlive{SuccessfulExit=false}`, NOT socket activation: lazy
start contradicts "always present for headless agents." `SuccessfulExit=false`
is load-bearing — the single-instance guard exits 0 when another server owns the
socket, and a bare `KeepAlive=true` would restart that clean exit in a tight
loop. Per-user LaunchAgent (not a root LaunchDaemon) because sessions are the
user's. No flock — launchd (one job) + the existing socket guard already give
single-instance.

## Verification

- Unit tests (`cargo test --bin yalda-session-server`): plist contains the
  required keys (Label, ProgramArguments, RunAtLoad, KeepAlive/SuccessfulExit,
  log path) and XML-escapes special chars in paths. 2 passed.
- Smoke: `--help` lists the subcommands; `status` correctly reports
  not-installed / not-loaded / socket-listening (a GUI-launched server was up).
- Regression: server still runs under the no-subcommand path — resilience (4) +
  transcript (5) suites green. All bins build.
- **NEEDS-RUNTIME (deliberately not run on the dev machine):** `install` /
  `uninstall` shell out to `launchctl` and modify the user's launchd domain +
  start a real daemon, so they were not executed here. The user runs `install`
  once and verifies: `launchctl list com.yalda.session-server` is loaded, the
  socket stays up across `kill`-ing the server (KeepAlive restarts it), and a
  session created before the kill recovers (WAL). Plist/dispatch are unit-tested.

## Where "agents run with no GUI" stands now

Foundation complete: detached server survives GUI exit; turns complete
unattended; prompt-then-leave is durable; the transcript survives a server
**crash** (WAL); and the server is now **always-present + crash-restarted +
start-at-login** (launchd). Remaining (separate work):
- **Actor extraction** (phase 3, ADR-0012) — internal correctness, not a no-GUI
  blocker.
- Rest of phase-7 hardening: safe-default permission mode (currently Yolo),
  `0600` socket assertion, bounded queues / slow-subscriber disconnect,
  `tracing` + `admin_status`.
- **Scope question still open:** should "run with no GUI" include *starting* work
  headlessly (cron/automation enqueuing a prompt to an unowned session)?
