# ADR-0013: Supervise the session server with a launchd LaunchAgent

**Status:** Accepted
**Date:** 2026-06-07
**Related:** spec-session-server-actor.md § Single-instance / Rollout phase 7, ADR-0009 (durable WAL — makes the install handoff lossless), ADR-0012 (hand-rolled actor)

## Context

For "agent sessions run with no GUI attached" the server must be **always
present** and **survive its own crashes**. Today the server exists only because
a GUI auto-launched it (`SessionServerClient::connect_or_launch`); nothing
restarts it if it dies while no GUI is around, and it never starts at login. The
durable WAL (ADR-0009) makes a *restart* lossless, but something must actually
perform the restart and the at-login start.

## Options considered

- **launchd socket activation** (kernel owns the socket; launchd starts the
  server lazily on first connect, gives single-instance by construction).
  REJECTED as the primary mechanism: lazy start is the *opposite* of the goal
  (always-present for headless agents), and it requires `launch_activate_socket`
  FFI plus a dual-mode bind path (inherited fd vs self-bind for the
  manual/GUI-fallback case). Its main benefit (don't run when idle) is a
  non-goal here.
- **LaunchDaemon (system-wide, root, at boot).** REJECTED: sessions are per-user
  (the user's files, the user's Claude subscription); a per-user **LaunchAgent**
  is the correct scope — starts at login, runs as the user, stops at logout.
- **No supervision (status quo) / a hand-rolled watchdog process.** REJECTED:
  reinvents what launchd already does well; a watchdog is itself unsupervised.

## Decision

Ship a per-user **LaunchAgent** (`com.yalda.session-server`) with
`RunAtLoad=true` and `KeepAlive={SuccessfulExit=false}`, managed by
`yalda-session-server install | uninstall | status`.

- `RunAtLoad` → starts at login. `KeepAlive{SuccessfulExit=false}` → restarts on
  a **crash** (non-zero/signal) but NOT on a clean exit. The clean-exit
  exclusion is load-bearing: the single-instance guard exits `0` when another
  server already owns the socket, and a bare `KeepAlive=true` would restart that
  clean exit in a tight loop.
- Single-instance is provided by launchd (one job) plus the existing socket-probe
  guard (catches a GUI/manual duplicate). We did **not** add flock — it's
  redundant given those two and would need a new dependency.
- `install` writes the plist, then **hands off any running server** by SIGTERM
  (graceful) before `launchctl load`, so launchd's instance becomes the socket
  owner. The handoff is **lossless because of the WAL**: the killed server's
  sessions are recovered from their WALs by the launchd-started replacement.

## Consequences

- The server becomes a real, supervised, start-at-login daemon — agent sessions
  persist across crashes and logouts (until logout for a LaunchAgent) with no GUI.
- The GUI's `connect_or_launch` remains the **fallback** when the LaunchAgent
  isn't installed; when it is, the GUI just connects to launchd's server. A brief
  race where the GUI auto-launches during a launchd restart is bounded by the
  guard (`SuccessfulExit=false` prevents a restart loop) and harmless thanks to
  the WAL.
- `install`/`uninstall` shell out to `launchctl load -w / unload -w` — simple and
  broadly compatible (NEEDS-RUNTIME: verified by the user running `install` once;
  the plist generation + arg dispatch are unit-tested). If a future macOS drops
  `load/unload`, switch to `bootstrap/bootout gui/$UID`.
- Auto-installing on first GUI run was deliberately NOT done — installing a
  LaunchAgent is a system change that should be the user's explicit choice (a GUI
  "enable background sessions" affordance can call `install` later).
