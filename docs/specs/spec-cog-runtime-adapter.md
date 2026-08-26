# Spec: Cog runtime-delivery adapter

- **Status:** ACTIVE DESIGN — implementation authorized; live activation gated
- **Date:** 2026-08-25
- **Normative peer contract:** `cog.runtime-delivery` protocol `1`, standalone
  v9, Cog Chat event `141` at `projects/cog/mail::chat`
- **Related:** ADR-0036; ADR-0015; ADR-0009;
  `spec-agent-session-ownership.md`; `spec-session-server-actor.md`;
  `spec-event-stream.md`

## 1. Goal

Make the already-running `yalda-session-server` a durable external Cog runtime
host. Cog may wake it for pending Mail and Chat addressed to explicitly selected
Agent Addresses. Yalda submits the immutable attempt to the address's existing
provider session and advances no Cog cursor until that provider turn reaches a
successful terminal boundary.

The adapter is a transport and lifecycle owner, not a peer-message interpreter.
Every delivered entry remains untrusted user-role data and cannot expand the
agent's credentials, tools, filesystem/network scope, or authority.

## 2. Placement and ownership

The adapter is a supervised child of `yalda-session-server`, not a GUI task and
not a second ACP process. The session server already owns the one durable
Codex/Claude transport, the session WAL, permission mode, active-turn state, and
canonical `AgentEvent::TurnEnded` outcome. The adapter calls a new internal
delivery command on that owner and waits for its correlated terminal result.

This preserves the existing invariants:

- one ACP transport per Yalda session;
- at most one GUI tile bound to a session, with headless work allowed by
  ADR-0015;
- no second attach/forwarder and no second provider resume;
- the provider session's stored permission mode remains authoritative.

No GPUI surface or tile behavior changes in V1.

## 3. Configuration and activation

Configuration is optional and explicit at `~/.yalda/cog-runtime.json`, with the
`YALDA_COG_RUNTIME_CONFIG` override used by tests and alternate instances:

```json
{
  "schema_version": "1",
  "cog_url": "http://127.0.0.1:7666",
  "host_id": "yalda-session-server",
  "allow_takeover": false,
  "addresses": [
    {
      "address_id": "opaque-cog-address",
      "yalda_session_id": "stable-server-session-id",
      "provider": "codex"
    }
  ]
}
```

An address entry is the operator's explicit selection for external ownership.
Duplicate address or Yalda-session mappings, empty ids, unknown providers,
non-loopback Cog URLs, archived/missing sessions, or provider mismatches fail
closed before any Cog mutation.

On start and bounded revalidation, the adapter requests
`GET /v1/runtime-delivery/capabilities`. It remains inert—no host lease, owner
transfer, claim, wake connection, or provider input—unless the response is
HTTP 200, protocol `1`, both source kinds, all configured provider kinds, and
every required v9 feature. HTTP 404 is the expected compatibility state during
Cog rollout. Unknown required values or limits also fail closed.

`allow_takeover=false` never replaces a different live host instance.
`allow_takeover=true` is explicit local-supervisor authorization for the one
initial CAS takeover observed at startup; a later CAS race is surfaced and is
not automatically retried against a newly observed live generation.

## 4. Exact wire model

The typed client implements the v9 media type
`application/vnd.cog.runtime-delivery.v1+json` and SSE wake stream. Every `U64`
is a validated decimal JSON string and is stored as `u64`; it is never decoded
through a JSON number or floating point value. Opaque ids remain byte-for-byte
UTF-8 strings. Timestamps, SHA-256 strings, UUIDs, cursor vectors, status unions,
and error unions are strictly validated.

Unknown optional object fields are ignored. Unknown required enum/source/status
values fail before provider dispatch. If a claimed attempt can be represented,
that failure is reported as `unsupported_contract_value`; otherwise activation
stops without mutating the attempt.

HTTP lives behind a `CogRuntimeTransport` trait. Production uses loopback
`ureq`; deterministic tests use a scripted transport that captures requests and
provides chunked/replayed SSE frames.

## 5. Runtime lifecycle

### 5.1 Host and ownership

Each OS process start creates a fresh, non-persisted `instance_id`. The adapter
acquires the configured host lease under the exact v9 create/renew/takeover CAS
rules and renews it before half of the negotiated lease has elapsed.

Only after the lease is live does it reconcile every selected address:

1. read the durable delivery owner;
2. leave an identical external owner unchanged;
3. CAS-transfer any cogd/other owner to this host using the observed generation;
4. reject retired/missing/mismatched addresses without affecting other routes.

External ownership is durable and never falls back to cogd when Yalda is
offline. Removing an address from config does not silently return ownership;
ownership changes require an explicit operator action in Cog.

### 5.2 Recovery before new claims

After lease and ownership reconciliation, inspect all open attempts for the
host, following pagination. Reclaim stable attempts before freezing new work.
For each attempt, require the same address, `delivery_key`, `payload_digest`,
immutable entries, and cursor vectors recorded in the journal. A mismatch fails
closed and never dispatches.

- durable `provider_succeeded` → complete without redispatch;
- durable `provider_failed` → fail without redispatch;
- queryable active provider turn → wait and renew;
- unknown outcome → redispatch the same stable `delivery_key` (documented
  at-least-once provider execution; Cog completion remains idempotent).

### 5.3 Wake, claim, capacity

One resumable SSE connection covers the host. Claim immediately after connect
and after each `delivery-ready` frame; wakes are only hints. Resume with the last
persisted `WakeId`, while every claim is authoritative.

Claims use configured, successfully reconciled addresses and negotiated limits.
One address has at most one active attempt. Retired/unowned/unavailable ids are
ignored per-address; an all-ineligible claim returns an empty 200 and performs no
provider action. `remaining_due` schedules another capacity claim without
waiting for a wake. `remaining_incompatible` is surfaced and never spun.

### 5.4 Dispatch and renewal

Before provider submission, append and fsync `dispatch_started`. Provider input
is exactly one user-role message with exactly two text content blocks:

1. the v9 fixed untrusted-data warning, byte-for-byte;
2. `COG_DELIVERY_V1_JSON\n` plus one compact JSON serialization of the v9
   Envelope using the claimed `DeliveryEntry` values byte/value-for-value.

No entry-derived prose exists outside block 2. The adapter does not parse entry
content for commands and never calls tools on its behalf.

Idle sessions start a turn. A compatible busy Codex session receives native
steering into its active turn. Busy Claude sessions and Codex sessions without
native steering are serialized behind the active turn; they are never cancelled
or concurrently resumed. While waiting or running, the adapter renews the
attempt before half its lease elapses and renews the host independently.

Only canonical live `AgentEvent::TurnEnded { outcome: Completed }`, correlated
to the submitted generation/turn, is provider success. `ReplayEnd`, cancellation,
refusal, max tokens, disconnect, prompt rejection, or failed outcome are not
success and map to a typed failure receipt. Acceptance, queueing, steering
acknowledgement, timeout, or socket disconnect alone never completes Cog.

### 5.5 Completion and failure

Provider success/failure and provider identifiers are appended and fsynced
before the corresponding Cog mutation. Completion/failure uses one persisted
idempotency key and exact request body. A lost response repeats that same request;
an exact replay accepts the stored response, while any changed semantic body is
an idempotency conflict.

The adapter treats Cog cursor vectors and advances as opaque receipt data. It
never computes or submits an advance. Only Cog's successful completion
transaction moves source cursors.

## 6. Durable journal

`~/.yalda/cog-runtime-journal.ndjson` (or the test/config-adjacent override) is an
append-only, fsync-on-transition journal. Each record contains schema version,
monotonic local sequence, attempt/address/key/digest, all current fences,
dispatch state, idempotency key, provider session/turn ids when known, terminal
provider result when known, and time. Valid states are:

`claimed → dispatch_started → provider_succeeded|provider_failed → cog_completed`

Replay folds by attempt id. A torn final line is ignored only when it is the
final unterminated record; malformed or contradictory earlier records fail
activation. Journal acknowledgement always precedes Cog acknowledgement.
Compaction, when added, must be atomic replace + directory fsync; V1 may retain
the bounded historical log.

## 7. Membership, ownership, and retirement races

Chat leave, address retirement, host-fence change, and owner transfer are
terminal/fencing facts from Cog. Any renew/complete/fail that returns
`attempt_already_terminal` or a stale fence stops local mutation and records the
terminal observation. It never advances a cursor or fabricates success.

A provider side effect may already exist when Chat leave or address retirement
commits. Yalda accepts that v9 at-least-once race but never attempts completion
after Cog reports the attempt terminal. Retired routes never wake, claim,
dispatch, retry, skip, transfer owner, or revive from journal recovery.

## 8. Shutdown and supervision

The adapter task has its own cancellation token and bounded reconnect backoff.
On graceful server shutdown it stops new claims, records any observed provider
terminal result, releases still-claimed attempts without cursor movement, then
releases the exact host lease. A crash relies on lease expiry and journal replay.

Adapter failure does not stop the session server. The supervisor logs a stable
inactive/error status and retries capability/transport failures with a cap.
Contract violations fail closed until configuration or Cog changes.

## 9. Verification

- codec/unit tests: decimal U64 beyond JavaScript range, exact unions, unknown
  required values, compact envelope escaping, exactly two text blocks;
- scripted transport: capabilities, error mapping, lease CAS, owner transfer,
  pagination, claim capacity, SSE replay/resume, lost responses;
- coordinator tests: zero/one/many capacity, fairness input, oversize,
  unsupported source, renewals, crash journal matrix, stable-batch recovery,
  retirement and Chat-leave terminalization;
- real session-manager tests: idle delivery, busy Codex steering, Claude
  serialization, rejection/disconnect/cancel/failure, and true terminal success;
- local Cog fixture once capabilities is live: mixed Mail/Chat ordering, cursor
  equivalence, ownership, retirement, lost completion response, and hostile
  content through the real server boundary.

Every new guard is negative-controlled at the real changed seam. The full build,
focused tests, workspace tests, strict lint, and changed-predicate mutation gate
must pass before merge. Until Cog returns a compatible capability 200, runtime
evidence is explicitly `activation disabled`, not an unverified claim.

