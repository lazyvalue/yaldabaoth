# Worklog: session-identity-guard

**Date:** 2026-08-27
**Branches touched:** `session-identity-guard`

## Cog execution evidence

The user explicitly opted this task out of Cog after `cog graph list` failed
because the `cog` executable was unavailable. No graph was reconstructed after
implementation.

## Built (with status)

- Durable server lifecycle resumes now use ACP's resume-only policy, preventing
  a failed `session/load` from replacing the provider identity inside an
  existing Yalda WAL.
- Recovery, explicit restart, and unarchive retry are covered. New-session
  create-with-resume remains permissive because its Yalda WAL has no prior
  conversation to merge.
- Added bug-0069 and reconciled UXI-AgentTile-19 enforcement.

## Open / unresolved

- `cargo-mutants` is not installed, so the mutation command could not run. The
  changed recovery predicate was manually negative-controlled instead.
- Applying the preservation-oriented live WAL split and restarting the systemd
  server will interrupt every active agent, including the session performing
  this work; it must happen at an explicit safe restart point.

## Decisions

- Preserve provider identity over availability: a failed durable resume becomes
  disconnected/unavailable rather than silently creating a new conversation.
- Preserve both affected provider conversations during repair by splitting at
  the second `SessionAttached` boundary; do not discard either provider id.

## Verification status

- `cargo check --bin yalda-session-server` passed.
- `cargo test --bin yalda-session-server` passed: 68/68.
- `cargo test --lib acp_channel` passed: 27 passed, 1 intentionally ignored.
- `cargo test --features test-support --test agent_transport_fake_test` passed:
  8/8.
- Negative control: setting recovery's `resume_only` argument back to `false`
  made `durable_recovery_never_replaces_a_failed_resume_with_a_fresh_identity`
  fail for the intended reason; restoring the fix returned it to green.
- Live process audit: one systemd-owned server PID owned the sole Unix socket.

## Next

- Merge and build the release server.
- At a safe interruption point, stop the service, back up and split the affected
  WAL, install the release binary, restart the service, and verify both provider
  identities appear exactly once.
