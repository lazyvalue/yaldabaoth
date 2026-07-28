# bug-0025: autoname request stuck without key

**Status:** FIXED  
**First seen:** 2026-07-28  
**Component:** `UXI-AgentTile-27`

## Symptom

Session summaries appeared late or never appeared. In particular, an install
without `ANTHROPIC_API_KEY` could leave a session with no summary forever.

## Root cause

`drain_autoname_requests` moved the one-shot from `Pending` to `Requested`.
`spawn_autoname_worker` then returned immediately when no key was configured,
without calling `finish_autoname`. The state never became `Done`, no sidecar
entry was written, and there was no visible in-flight feedback. Network failures
did settle, but permanently spent the one-shot without any useful summary.

## Fix

- Install a compact opening-user-turn topic immediately; show
  `summarizing topic…` only when no useful excerpt exists yet.
- Bound the direct naming request to eight seconds.
- Ask for topic/goal only, cap summaries at 140 characters, and exclude progress.
- Derive a deterministic fallback from the opening user turn.
- Missing credentials, request failures, and omitted summaries all settle and
  persist that fallback through the existing id-keyed sidecar.

## Verification

`autoname_without_api_key_settles_with_persisted_topic` drives the exact
no-credential branch and proves state becomes `Done`, a useful summary is
installed, and the sidecar contains it. Negative control observed RED: removing
the settlement left `Requested` (`left: Requested`, `right: Done`).
