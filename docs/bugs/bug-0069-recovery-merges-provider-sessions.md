# bug-0069: recovery-merges-provider-sessions

**Status:** FIXED
**First seen:** 2026-08-27
**Component:** Agent Tile / session binding

## Symptom

One Yalda session can show content from two different underlying Claude
conversations after the session server restarts. Repeated auto-labels such as
`claude-4` make the corruption look like an indexing collision, but the labels
are not the identity key.

## Context / root cause

WAL recovery correctly recovered the durable provider session id, then called
the ACP client's permissive resume path. If `session/load` failed or timed out,
that path silently called `session/new`. `ManagedSession::apply_channel_state`
then recorded the fresh provider id as another `SessionAttached` event in the
same WAL. Subsequent recovery derived identity from the last attachment, so two
provider conversations had become one Yalda history.

This violates `UXI-AgentTile-19`: an unresumable durable session must become
explicitly unavailable and offer “start fresh”; it must not silently replace its
identity. A live-system audit also proved that one systemd-owned server was the
sole owner of the Yalda socket when the corruption occurred, ruling out duplicate
server indexing as the cause.

## Planned solution

Make every durable lifecycle resume (server recovery, restart, and unarchive)
use ACP's existing resume-only policy. A load failure leaves the Yalda session
disconnected with its original provider identity intact. Keep the permissive
fallback only for create-with-resume, whose new Yalda WAL has no prior
conversation to merge.

## Approaches already tried (do NOT repeat)

- Treating duplicate display labels as identity collisions: server sessions are
  keyed by UUID; labels are presentation and sort data.
- Allowing recovery to fall back to `session/new`: this is the corruption path.

---

## Log

### 2026-08-27 11:40 — durable resumes now fail closed

Extended `AgentSpawner` with an explicit `resume_only` policy and routed server
recovery, restart, and unarchive/lifecycle retries through it. The production
spawner uses `AcpChannelClient::spawn_resume_only_in_for`, while genuinely new
Yalda sessions retain the prior permissive import behavior.

Added
`durable_recovery_never_replaces_a_failed_resume_with_a_fresh_identity` at the
real recovery-worker seam. Negative control changed the recovery policy back to
permissive and observed the guard fail with `(provider-id, false)` instead of
`(provider-id, true)`.
