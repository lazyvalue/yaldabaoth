# Worklog: Provider-aware agent turn label

**Date:** 2026-07-26
**Branches touched:** `provider-turn-label`

## Built

- Replaced the transcript's hard-coded `Claude` agent-turn label with the active
  session provider's display name.
- Added the provider to the transcript cache fingerprint so a provider change
  cannot reuse a stale turn header.
- Kept the internal `TurnRole::Claude` representation unchanged for wire and
  view-model compatibility; only the user-facing label is provider-aware.

## Verification

- `agent_turn_header_uses_active_provider_name` covers Claude, Codex, the `You`
  label, and provider-driven transcript cache invalidation.
- `cargo test --bin yalda-gpui`: 479 passed, 1 ignored.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed.
- `git diff --check`: passed.

## Open / unresolved

- None.
