# ADR-0037: The session server's lifecycle belongs to systemd; the GUI never launches one; boot is Highlander-guarded

**Date:** 2026-09-01
**Status:** accepted
**Related:** bug-0064 (RECURRED 2026-09-01), ADR-0009/-0018 (WAL durability),
Cog graph `f5x`, UXI-Workspace-29

## Context

On 2026-09-01 the bug-0064 split-brain recurred *after* its fix had merged:
the screen locked, the GUI froze, and the GUI's `connect_or_launch` reconnect
path auto-launched a second `yalda-session-server` from `~/.local/bin` — a
stale Aug 27 binary predating the OS-lease fix (`8eef3b1`). The stale binary
ran the old connect-then-unlink socket check, stole the socket pathname from
the (equally stale, equally lease-less) resident server, recovered the same
roster, and resumed twin `claude --resume` workers for a dozen ACP identities.
One session recovered with 122,296 dropped events.

Three structural facts made this possible:

1. **The GUI had launch authority.** Any transient connect failure — a frozen
   process, a screen-lock hiccup — let the client fork a server from PATH,
   with whatever binary happened to be installed.
2. **Nothing owned the binary path or restarts.** The lease fix existed only
   in git: verification built it in a worktree `target/`, nothing installed it
   to `~/.local/bin`, and the no-restart rule (correctly) kept agents from
   bouncing the resident server — so the fix never executed. "Merged" and
   "running" silently diverged for two days.
3. **The lease only excludes servers that take it.** A pre-lease (or foreign)
   binary never contends for the flock, so the lease cannot see it. Process
   coexistence itself was never checked.

## Decision

Scott's directive, adopted verbatim as the lifecycle contract:

1. **The GUI can never automatically start a server.**
   `SessionServerClient::connect`/`reconnect` are connect-only;
   `connect_or_launch`, `find_server_binary`, and the client-side server log
   plumbing are deleted. When no server is reachable the GUI restores tiles as
   reconnectable placeholders, shows the splash instruction
   (UXI-Workspace-29), and retries the connect on the pump's backoff.
2. **systemd (user service) manages the server process.**
   `dist/systemd/yalda-session-server.service` + `install-service.sh` are the
   one supervision path on Linux (launchd on macOS). Every dev flow deploys
   through it: `dev-server.sh` now delegates to `deploy-server.sh`
   (build → atomic install to `~/.local/bin` → `systemctl --user restart`),
   and the in-app "Rebuild & Restart all" does the same install + restart
   instead of pkill + respawn-by-GUI.
3. **Highlander rule at server boot.** Before touching lease, socket, WALs, or
   agents, the server scans `/proc` for any other process whose executable is
   named `yalda-session-server` (a ` (deleted)` suffix — an unlinked,
   since-reinstalled binary — counts). If any exist it refuses to boot with a
   clean `exit(0)` (so `Restart=on-failure` cannot loop it) and a loud log.
   `--force` overrides for deliberate side-by-side runs on private sockets
   (tests).
4. **The GUI surfaces "no server" instead of papering over it.** The splash
   names the exact start command and stays up while the condition holds.

## Alternatives rejected

- **Keep auto-launch but version-check the binary first.** Still leaves the
  GUI racing supervisors and other GUIs for launch authority; version checks
  cannot see a hung-but-alive old server. Ownership, not smarter launching,
  was the fix.
- **Rely on the OS lease alone.** Proven insufficient by this recurrence: a
  binary that predates (or simply doesn't implement) the lease is invisible to
  it. The Highlander scan checks the actual failure unit — process
  coexistence.
- **Have the new server kill the old one on takeover.** Automated kills of an
  unknown server process violate the no-restart safety rule and can destroy
  in-flight work; refusing to boot and telling the operator is strictly safer.

## Consequences

- A fresh checkout must run `./install-service.sh` once; after that,
  `./dev-server.sh` (or the in-app rebuild) always deploys the *current*
  build — "merged but never installed" can no longer happen silently.
- With no server started, the GUI is intentionally degraded: agent tiles wait
  as placeholders and the splash instructs; nothing direct-spawns unless
  `YALDA_SESSION_SERVER=0` is set explicitly.
- The Highlander refusal means activating a new build while an unmanaged
  (pre-systemd) server still runs requires stopping that server first — by
  design, a human decision.
- macOS keeps launchd supervision; the `/proc` scan is a no-op there (the
  guard is Linux-evidence-based).

## Enforcement

`highlander_refuses_boot_while_another_server_process_exists`,
`highlander_force_boots_side_by_side`,
`client_connect_never_launches_a_server` (tests/session_resilience_test.rs);
`splash_paints_start_server_instruction_when_server_missing`
(verify_harness.rs). All four observed RED with their fix reverted
(2026-09-01).
