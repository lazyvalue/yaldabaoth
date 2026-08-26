# Worklog: Cog runtime-delivery adapter

**Date:** 2026-08-25
**Branch:** `yalda-cog-runtime-adapter`
**Feature tip:** `4ac6568`
**Main merge:** `a79bdef`

## Cog execution evidence

- Graph id: `vkf`
- Cog Chat: `%projects/cog/mail::chat`
- Accepted contract: standalone runtime-delivery v9, Chat event `141`

### Initial render

Shown before tracked-file implementation began:

```text
graph yalda-cog-runtime-adapter-v1 (frontiers)
frontier 0: accept-contract [open]
frontier 1: spec-adapter [open]
frontier 2: build-provider-bridge [open], build-wire-client [open]
frontier 3: build-coordinator [open]
frontier 4: wire-supervision [open]
frontier 5: verify-end-to-end [open]
frontier 6: document-integrate [open]
frontier 7: omega [open] (omega)
```

### Node execution

- `j7dq` `accept-contract`: claimed → closed with output; reviewed event `132`, sent the complete lifecycle semantics in event `137`, accepted the standalone canonical successor in event `143`, and recorded the live capability 404.
- `xdix` `spec-adapter`: claimed → closed with output; added the normative spec, ADR-0036, project record, ticket, placement, activation, ownership, recovery, shutdown, and verification contracts (`a45f5ba`).
- `whyv` `build-wire-client`: claimed → closed with output; implemented strict protocol-v1 codecs, typed HTTP operations, error unions, pagination, and resumable wake SSE (`e047064`).
- `v3f7` `build-provider-bridge`: claimed → closed with output; implemented the exact two-block untrusted provider envelope and private existing-session delivery path (`588f9d7`).
- `c23b` `build-coordinator`: claimed → closed with output; implemented fencing, ownership CAS, capacity claims, fsync journal, stable recovery, retirement, and idempotent terminal retries (`bb80d64`).
- `dlq4` `wire-supervision`: claimed → closed with output; embedded strict optional configuration, capability revalidation, authoritative claims, resumable wakes, renewals, bounded backoff, and graceful release in `yalda-session-server` (`d2bd96b`).
- `bte7` `verify-end-to-end`: claimed → closed with output; passed workspace and strict lint verification, negative controls, real loopback/session-manager seams, and a focused 2/2 mutation gate after adding same-instance renewal coverage (`1708e33`).
- `cspj` `document-integrate`: claimed → closed with output; documented operations, merged `a79bdef` to main, preserved unrelated local edits, rebuilt the release server, validated this worklog, and confirmed activation remains disabled (`4ac6568`, final integration commit).
- `tszb` `omega`: claimed → closed with output; complete implementation, verification, documentation, integration, and safe activation state.

### Notes

- Chat event `137` supplied the prepared lifecycle semantics; event `141` was the builder's standalone canonical v9 contract, and event `143` accepted it after the technical review of event `132` and its successors.
- The live capability endpoint returned HTTP 404 with an empty body throughout implementation and again after the main rebuild on 2026-08-25 at 20:36 PDT. Per contract, no host lease, ownership transfer, claim, wake subscription, or provider dispatch was activated.
- The real live-Cog mutation leg of the verification matrix was therefore intentionally skipped. Its loopback production transport, scripted Cog lifecycle, and real `SessionManager` provider boundary ran instead; activation remains ready but fail-closed until Cog advertises the accepted capability set.
- The first adapter-wide mutation sample exposed missing coverage for the same-live-instance lease branch. A focused renewal test was added, and replacements of that predicate with both `true` and `false` were subsequently caught.
- Main already contained unrelated changes to `Cargo.toml`, `Cargo.lock`, `.claude/scheduled_tasks.lock`, and Cog WAL files. Git autostashed and restored them around the merge; they remain outside this work.

### Final status

- Status: `complete`

```text
graph yalda-cog-runtime-adapter-v1 (frontiers)
frontier 0: accept-contract [done]
frontier 1: spec-adapter [done]
frontier 2: build-provider-bridge [done], build-wire-client [done]
frontier 3: build-coordinator [done]
frontier 4: wire-supervision [done]
frontier 5: verify-end-to-end [done]
frontier 6: document-integrate [done]
frontier 7: omega [done] (omega)
```

## Built

- A strict Cog runtime-delivery v1 client with lossless decimal-string integers, fail-closed unions, typed lifecycle operations, and resumable SSE wakes.
- A durable fenced coordinator for explicit external ownership, multi-address capacity, fsynced dispatch/terminal state, stable replay, idempotent completion, retirement, and graceful release.
- A private provider bridge that reuses the existing Codex/Claude session owner and submits exactly two user-role text blocks, preserving peer content as untrusted data.
- Optional `yalda-session-server` supervision with strict route validation, five-minute capability revalidation, five-second authoritative claims, bounded backoff, and no embedded-runtime fallback.

## Verification status

- `cargo test --workspace --all-targets --features test-support`: passed. The run included 212 library tests, 77 session-server tests, all hermetic integration suites, and benchmark smoke tests; explicitly live/auth-dependent tests remained ignored.
- Strict Clippy passed for the library and `yalda-session-server` with only the documented pre-existing lint-category exclusions.
- Negative controls failed red for lossless integer validation, exact two-block framing, pre-dispatch journal durability, strict config fields, mixed Mail/Chat ordering, and the live-host identity predicate.
- Focused mutation gate: both `true` and `false` replacements of the live same-instance lease predicate were caught (2/2).
- `cargo build --release --bin yalda-session-server` passed on merged main at `a79bdef`.
- Runtime check: the server socket is listening, no operator Cog runtime config exists, and `GET /v1/runtime-delivery/capabilities` returns empty HTTP 404. Activation is correctly disabled.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-25-cog-runtime-adapter.md` passes.

## Activation

Create the strict route file described in the README and restart
`yalda-session-server` only after the live Cog endpoint advertises the complete
accepted v9 capability contract. Until both conditions are true, the adapter is
inert by design.
