# Worklog: server-lifecycle-highlander — bug-0064 RECURRED, lifecycle redesign

**Date:** 2026-09-01 (evening session)
**Branches touched:** `server-lifecycle-highlander` (45a8ea7, merged to `main` as cfd990a, branch deleted)

## Cog execution evidence

- Graph id: `f5x`

### Initial render

```text
graph server-lifecycle-highlander (frontiers)
frontier 0: evidence-recurrence [open], omega [open] (omega)
frontier 1: systemd-lifecycle [open], no-gui-autolaunch [open], highlander-guard [open]
frontier 2: no-server-splash [open]
frontier 3: docs-adr-bug [open]
frontier 4: verify-integrate [open]
```

### Node execution

- `9bx9` `evidence-recurrence`: claimed → closed; output: split-brain forensics
  (two pre-fix servers, socket steal by frozen-GUI auto-launch at screen lock,
  twin `claude --resume` workers, `recovered_dropped=122296`, binaries dated
  Aug 27 vs fix commits Aug 30/Sep 1 — the lease fix never deployed).
- `tknn` `no-gui-autolaunch`: claimed → closed; output: `connect_or_launch`/
  `find_server_binary` deleted, connect-only client, guard
  `client_connect_never_launches_a_server`, NC RED (spawn reintroduced).
- `yqw6` `highlander-guard`: claimed → closed; output: `/proc` scan before
  lease/socket/WAL/agents, clean exit(0) refusal, `--force`, guards
  `highlander_refuses_boot_while_another_server_process_exists` +
  `highlander_force_boots_side_by_side`, NC RED (scan disabled → server booted
  beside a live one).
- `q9sw` `no-server-splash`: claimed → closed; output: UXI-Workspace-29 splash
  instruction + no-expire hold, thread-local `with_server_path_disabled` seam,
  guard `splash_paints_start_server_instruction_when_server_missing`, NC RED
  (`server_missing()` hard-wired false → probe miss).
- `fwd9` `systemd-lifecycle`: claimed → closed; output: dev-server.sh delegates
  to deploy-server.sh; in-app Rebuild&Restart installs to `~/.local/bin` +
  `systemctl --user restart` (pkill/GUI-respawn removed).
- `751y` `docs-adr-bug`: claimed → closed; output: ADR-0037, bug-0064 RECURRED
  entry + manifest (times-addressed 3), UXI-Workspace-29.
- `52x2` `verify-integrate`: claimed → closed; output: merge cfd990a, suites on
  main (see Built), 3 NCs RED, release built + server binary installed, no
  process restarted, 5 leaked old-binary servers flagged for operator kill.
- `yott` omega: claimed → closed.

### Notes

- `yqw6`, topic `deviation`: claimed after implementation began (single-session
  work, no contention). (Note-add with `--topic/--note` flags failed CLI
  parsing; recorded here instead.)

### Final status

- Status: `complete`

```text
graph server-lifecycle-highlander (frontiers)
frontier 0: evidence-recurrence [done], omega [done] (omega)
frontier 1: systemd-lifecycle [done], no-gui-autolaunch [done], highlander-guard [done]
frontier 2: no-server-splash [done]
frontier 3: docs-adr-bug [done]
frontier 4: verify-integrate [done]
```

## Built (with status)

- **Incident response** (no branch): stale pre-fix server 897160 + ~40
  descendants SIGTERMed with explicit operator approval; graceful
  "WALs are durable" shutdown observed; GUI-connected server 521644 untouched.
- **Server-lifecycle redesign** (`server-lifecycle-highlander` 45a8ea7 →
  `main` cfd990a): the four-point contract from Scott's directive — see
  ADR-0037 for rationale, bug-0064 log for forensics. Verified: `--lib` 223,
  server bin 68, resilience 14 (3 new), transcript 14, GUI 765/766 across four
  consecutive full runs; the one failure
  (`archived_waiting_session_is_removed_from_the_painted_waiting_tab`) fails
  identically at pre-merge 3004569 — pre-existing, already in backlog.
  Negative controls: all three fix-classes observed RED reverted. Mutation
  gate: left to CI `--in-diff`.
- **Release build + install** (23:16): both binaries built on merged main;
  `yalda-session-server` atomically installed to `~/.local/bin`. **No process
  restarted** (activation boundary honored).

## Unverified / runtime-pending

- **NEEDS-RUNTIME (gap 2, live loop):** the serverless-boot → splash →
  start-server → auto-reattach cycle end-to-end with a real systemd unit, and
  the in-app Rebuild&Restart's systemctl path. Headless guards cover the
  reducer/paint/process layers; the live systemd interaction needs Scott's
  activation pass.
- Steering-pair test flakes seen only while background NC servers were
  hammering the box; four clean consecutive suite runs afterward — treated as
  environmental, watch on next CI run.

## Incident tail (open at session end)

Five MORE old-binary servers auto-launched by the still-running old GUI during
the session (21:56–23:14), each stealing the socket path; all six bound to
`/tmp/yalda-session-scott.sock` on different inodes, ~57 claude workers total.
The GUI's live connection remains on 521644. Multiplication is stopped (since
23:16 any auto-launch runs the NEW binary → Highlander-refuses), but the five
(968767, 1166646, 1281509, 1286826, 1459868) await Scott's kill approval, then
`./install-service.sh` + GUI restart onto the new build.
