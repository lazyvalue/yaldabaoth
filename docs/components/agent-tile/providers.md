# Agent Tile — Providers

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-30`.

## UXI-AgentTile-30 — Claude and Codex sessions coexist with durable provider identity

**Statement.** An unbound Agent tile offers separate **New Claude session** and
**New Codex session** rows. Existing sessions identify their provider in the
selector, and a bound tile identifies it in the status strip. Each session owns
exactly one provider for its lifetime. Create, restart, WAL recovery, clear, resume,
and working-directory respawn all route through that provider; a mixed roster may
run Claude and Codex concurrently.

Codex uses the `codex-acp` adapter and the cached interactive login created by
`codex login`. Yalda removes ambient OpenAI API-auth variables from Codex children
unless `YALDA_CODEX_ALLOW_API_KEY=1`, preventing a shell API key from silently
changing a subscription-backed session into metered API usage. A missing provider
on an older wire request or WAL header defaults to Claude.

**Applies to.** `AgentProvider`, `AgentSpawner`, the session protocol/client/server,
WAL recovery, the Agent selector and status strip, and
`AcpChannelClient::spawn_with_resume_in_for`.

**Enforcement.** `session_proto::tests::{create_session_defaults_legacy_peer_to_claude,
create_session_round_trips_codex_provider}`; `session_wal::tests::codex_provider_survives_recovery`;
the GPUI provider-label allocator/menu tests, selector navigation/activation
tests, and `codex_picker_session_retains_provider_without_server`. Live adapter
authentication remains a runtime check because it uses the user's installed
`codex-acp` and private Codex login.

**Status.** `implemented` (compile + headless tests; live Codex ACP handshake
requires the local adapter installation).
