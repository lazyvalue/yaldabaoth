# bug-0057: session-restart-close-attach-races

**Status:** FIXED
**First seen:** 2026-08-22
**Component:** session server restart, attach, unarchive, and close transitions

## Symptom

Restart/unarchive can publish a stale channel or fail once and never retry.
Duplicate Attach requests on one connection can leave duplicate forwarding
tasks. A thread-builder failure can leave a session permanently spawning. If
WAL deletion fails during Close, the UI can lose the live session only for it
to reappear after the next daemon restart.

## Context / root cause

Restart fencing occurred too late and depended on a live channel for the ACP
resume identity. The already-unarchived fast path suppressed retry. Spawn worker
thread creation errors were discarded. Per-connection subscription replacement
did not abort its old task. Close removed the in-memory session before checking
whether durable deletion succeeded.

## Planned solution

Fence restart generation immediately, use the durable ACP id as fallback, allow
unarchive to retry a disconnected session, surface thread creation failure as a
terminal spawn failure, abort duplicate subscriptions in the connection-local
task map, and delete the WAL before dropping live state.

## Approaches already tried (do NOT repeat)

- Releasing the actor's prior forwarder on every Attach breaks legitimate
  observers on separate connections. Cleanup must stay connection-local.
- Removing live state before WAL deletion turns an I/O error into guaranteed
  closed-session resurrection.

---

## Log

### 2026-08-22 — fenced transitions and resurrection-safe close shipped

- Restart now bumps generation and drops the old channel before starting its
  worker; durable ACP identity survives a dead transport.
- Repeated unarchive retries disconnected handshakes and all thread-builder
  failures enter the normal spawn-failed terminal path.
- Duplicate Attach replaces and aborts only the task owned by that connection.
  A real-wire slow-observer test caught and prevented an over-broad actor-level
  release during integration.
- Close now removes durable state first; deletion failure returns an error and
  keeps the live session present. Negative control proved the ordering guard.
- Implemented in `39406ab` and `b62f421`; merged to main in `c354664`.
