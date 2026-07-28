# bug-0026: archived-session-visible-in-waiting

**Status:** FIXED
**First seen:** 2026-07-28
**Component:** `docs/components/jump-panel.md` (`UXI-JumpPanel-16`)

## Symptom

An archived session appears in the Waiting tab. Archiving is a visibility flag,
so an archived row should appear only in Archived regardless of whether its
underlying live activity is Waiting or Working.

## Context / root cause

The checked-in `agent_rows_for_tab` projection already excludes archived rows
from Waiting. The report came from runtime/build skew: the live GUI process
started at `15:25:31`, while the release executable on disk finished updating
at `15:25:51`, so the process could not be the just-built executable.

The existing archive guard also had a blind spot: it archived a **Working**
session. Its Waiting assertion therefore never proved that an idle archived
session leaves the selected Waiting tab after the real context-menu action.

## Solution

Added a real-path painted regression guard that starts with a connected idle
session in Waiting, right-clicks its actual row, clicks the actual Archive menu
item, and proves the row leaves both the Waiting projection and paint while
remaining visible in Archived. Mutation-checking the production Waiting
predicate produced the expected RED failure. Rebuilding and restarting the GUI
from merged main removes the runtime skew.

## Approaches already tried (do NOT repeat)

- The original archive guard proved an archived **Working** row was absent from
  Working/All and present in Archived; it did not cover this reported state.

---

## Log

### 2026-07-28 15:50 PDT — reproduction started

Opened the first dedicated history for the report. No fix yet: the existing
test's archived session was Working, leaving the archived-Waiting action and
paint path unguarded.

### 2026-07-28 16:04 PDT — runtime skew localized; guard RED/GREEN

The real archive-action/Waiting-paint guard passed against current source before
any production edit. Live diagnostics found the GUI process started 20 seconds
before the release executable's final modification time. Removed
`!row.archived` as the negative control and observed the new guard fail at the
Waiting projection; restored it and observed GREEN. The remaining fix is to
ship the guard and restart from the merged release build.

### 2026-07-28 16:06 PDT — full verification passed

`cargo test` passed the complete suite, including 496 GPUI tests and every
non-live integration suite. Live network/auth tests remained intentionally
ignored. Ready to merge, rebuild release, restart the GUI, and re-query the
preserved session server.

### 2026-07-28 16:09 PDT — merged release running

Merged to `main`, built the release GUI, and replaced the mismatched process.
The new PID started after the release artifact's modification time and maps
that artifact's inode. It reconnected successfully to the unchanged session
server; the read-only roster query still reported all 17 live sessions.
