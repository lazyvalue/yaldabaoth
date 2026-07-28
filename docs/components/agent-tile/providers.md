# Agent Tile — Providers

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-30`.

## UXI-AgentTile-30 — Claude and Codex sessions coexist with durable provider identity

**Statement.** An unbound Agent tile offers separate **New Claude session** and
**New Codex session** rows. Existing sessions identify their provider in the
selector, and a bound tile identifies it in the status strip. Each session owns
exactly one provider for its lifetime. Create, restart, WAL recovery, clear, resume,
and working-directory respawn all route through that provider; a mixed roster may
run Claude and Codex concurrently. Transcript agent-turn headers use that active
provider's display name (`Claude` or `Codex`); the user header remains `You`.

Codex uses the `codex-acp` adapter and the cached interactive login created by
`codex login`. Yalda removes ambient OpenAI API-auth variables from Codex children
unless `YALDA_CODEX_ALLOW_API_KEY=1`, preventing a shell API key from silently
changing a subscription-backed session into metered API usage. A missing provider
on an older wire request or WAL header defaults to Claude. The local tile-session
snapshot stores provider identity additively; older local snapshots remain
unknown until the WAL-backed roster arrives, at which point every open session
with the same sid adopts the roster provider and repaints both the status strip
and cached transcript.

**Applies to.** `AgentProvider`, `AgentSpawner`, the session protocol/client/server,
WAL recovery, the Agent selector and status strip, and
`AcpChannelClient::spawn_with_resume_in_for`.

**Enforcement.** `session_proto::tests::{create_session_defaults_legacy_peer_to_claude,
create_session_round_trips_codex_provider}`; `session_wal::tests::codex_provider_survives_recovery`;
the GPUI provider-label allocator/menu tests, selector navigation/activation
tests, `codex_picker_session_retains_provider_without_server`,
`multi_session_persistence_round_trips_distinct_sids`, and
`codex_roster_identity_repairs_restored_tile_and_turn_header` (real notification
reducer plus painted status-strip/transcript probes). Live adapter authentication
remains a runtime check because it uses the user's installed `codex-acp` and
private Codex login.

**Status.** `implemented` (compile + headless tests; live Codex ACP handshake
requires the local adapter installation).
