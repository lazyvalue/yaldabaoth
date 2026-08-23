# bug-0055: timed-out-session-request-leaks-waiter

**Status:** FIXED
**First seen:** 2026-08-22
**Component:** session client request/response transport

## Symptom

Long-lived clients accumulate pending RPC waiters after server stalls, send
failures, or disconnects. A late response can still find stale state after the
caller has already received a timeout, and repeated failures retain memory for
the life of the client.

## Context / root cause

The synchronous client and cloneable handle duplicated request bookkeeping.
Timeout and write-error branches returned without removing the request id from
the shared pending map; disconnect cleanup was not the single owner of all
outstanding waiters.

## Planned solution

Centralize request/response registration and cleanup. Register before the
write, remove on write failure or timeout, drain on disconnect, and treat a
response for an expired request as a harmless late response.

## Approaches already tried (do NOT repeat)

- Relying on the reader thread to eventually clean the map does not cover a
  connected server that simply never answers one request.

---

## Log

### 2026-08-22 — centralized request cleanup shipped

- Both client surfaces now use one `request_response` path with synchronous
  registration and deterministic cleanup for timeout, send failure, and EOF.
- Guards prove timed-out client and handle requests leave no waiter and that a
  late response is ignored without poisoning the next request.
- All three guards were observed RED against the leaking behavior, then passed.
- Implemented in `b5192b7`; merged to main in `c354664`.
