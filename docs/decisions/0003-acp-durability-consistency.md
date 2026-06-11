# ADR-0003: ACP session durability and cross-panel consistency

**Status:** Accepted
**Date:** 2026-06-02
**Related:** session_client.rs, session_proto.rs, yalda-session-server/main.rs

## Context

Stopping / double-stopping a session sometimes disconnected the GUI, and the
session list went stale after closes/renames or with the same session in
multiple panels. Root causes: (1) the client never drained its `pending`
request map on socket death, so a blocking call (e.g. `close_session`) parked on
its 30s timeout and froze the paint thread; (2) no reconnect path at all; (3)
the session list was a local optimistic cache synced once, never reconciled;
(4) the same session attached as Owner from two panels duplicated forwarders and
stranded one panel's stream.

## Decision

- **Fail-fast on disconnect:** drain `pending` (drop senders) the instant either
  client thread sees a dead socket; blocking `request` returns `BrokenPipe`, not
  a 30s hang. Add `Drop` to do the same.
- **Reconnect with backoff + resubscribe:** `SessionServerClient::reconnect()`
  rebuilds threads/channels; the pump drives it on a 1s backoff when the wake
  channel closes, then re-attaches every slot (server replays the log, so each
  slot is reset and rebuilt from replay).
- **Manager-level broadcasts:** server emits `SessionClosed` / `SessionRenamed`
  / `SessionCreated` to *all* connections; the GUI reconciles every panel.
- **Confirm-then-mutate close:** call the server first; only drop the local slot
  on success (or detach for an observer); a connection error keeps the slot.
- **Prevent duplicate Owner-attach** of one session across panels.

## Rationale

The server is the source of truth; the local list must be a reconciled
projection, not a write-once cache. Idempotent, confirmed mutations + broadcasts
keep every panel/GUI consistent without polling.

## Consequences

- Adds protocol variants (`session_proto.rs`) → daemon restart needed to pick up.
- The duplicate-attach *prevention* is the right fix for *accidental* dupes, but
  intentional multi-panel viewing of one session (workspaces also-show for
  agents) wants the opposite — fan-out to all views. That flip needs the session
  Core/View split (ADR-0005) and is deferred. (`ff-ui-threading` later removed
  the now-unused dedup helper as part of moving open/attach off-thread.)
- Reconnect transcript-rebuild is not runtime-verified (needs kill-server test).
