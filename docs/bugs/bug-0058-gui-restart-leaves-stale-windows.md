# bug-0058: gui-restart-leaves-stale-windows

**Status:** FIXED
**First seen:** 2026-08-22
**Component:** development activation (`dev-gui.sh`)

## Symptom

After rebuilding/restarting the GUI, the old empty Yalda windows remained while a
second fixed GUI also launched. Two `yalda-gpui` processes attached the same 30
live sessions, making a successful repair appear not to have taken effect.

## Context / root cause

`dev-gui.sh` sent TERM to debug/release GUI processes, slept for a fixed 300ms,
and launched the replacement without checking that the prior processes exited.
The running GPUI process did not honor TERM inside that window (and remained
alive for more than two hours), so activation accumulated a second window set
and server connection.

The server and WAL were healthy; the defect was exclusively in the development
restart boundary.

## Planned solution

Match only repo-built `target/debug/yalda-gpui` and
`target/release/yalda-gpui` processes. Send TERM, poll for a bounded grace
period, KILL only those exact matches if needed, verify none remain, and refuse
to launch a duplicate if teardown still failed. Never match the session server.

## Approaches already tried (do NOT repeat)

- A fixed post-TERM sleep is not an exit guarantee.

---

## Log

### 2026-08-22 20:40 — require old GUI exit before replacement launch

- `dev-gui.sh` now uses one target-specific process regex, a three-second TERM
  grace period, a bounded KILL fallback, and a final no-stale-process assertion.
- `bash -n dev-gui.sh` passed. The regex matched the intended release GUI and
  not `yalda-session-server`.
- End-to-end release activation rebuilt the GUI, terminated the prior process,
  restored all 30 leaves, and left exactly one
  `./target/release/yalda-gpui` process. Server/WAL state was untouched.
- Commit `2685397`; main merge `e20446e`; Cog graph `od4`.
