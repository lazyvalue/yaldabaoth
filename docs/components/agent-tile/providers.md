# Agent Tile — Providers

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-30`,
`UXI-AgentTile-44`.

## UXI-AgentTile-30 — Claude and Codex sessions coexist with durable provider identity

**Statement.** An unbound Agent tile offers separate **New Claude session** and
**New Codex session** rows. Existing sessions identify their provider in the
selector and agent-turn headers; the compact bound-tile header does not repeat a
provider badge. Each session owns exactly one provider for its lifetime. Create, restart, WAL recovery, clear, resume,
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
with the same sid adopts the roster provider and repaints the cached transcript.

**Applies to.** `AgentProvider`, `AgentSpawner`, the session protocol/client/server,
WAL recovery, the Agent selector and transcript turn headers, and
`AcpChannelClient::spawn_with_resume_in_for`.

**Enforcement.** `session_proto::tests::{create_session_defaults_legacy_peer_to_claude,
create_session_round_trips_codex_provider}`; `session_wal::tests::codex_provider_survives_recovery`;
the GPUI provider-label allocator/menu tests, selector navigation/activation
tests, `codex_picker_session_retains_provider_without_server`,
`multi_session_persistence_round_trips_distinct_sids`, and
`codex_roster_identity_repairs_session_and_turn_header` (real notification
reducer plus painted transcript probe). Live adapter authentication
remains a runtime check because it uses the user's installed `codex-acp` and
private Codex login.

**Status.** `implemented` (compile + headless tests; live Codex ACP handshake
requires the local adapter installation).

## UXI-AgentTile-44 — Codex subagents default to Luna without changing the parent model

**Statement.** Every Codex adapter process that Yalda starts receives
`agents.default_subagent_model = "gpt-5.6-luna"` through the adapter's
`CODEX_CONFIG` session overlay. A Codex `spawn_agent` call that omits an explicit
model therefore uses Luna even when the parent session uses Sol or Terra. The
overlay changes only the subagent default: it does not set the parent `model`,
change the parent model picker (`UXI-AgentTile-16`), or add values to Codex's
own explicit `spawn_agent(model=...)` schema.

Yalda merges the setting into an inherited `CODEX_CONFIG` JSON object. It
preserves all unrelated root keys and all other `agents` keys. Yalda replaces
an inherited `agents.default_subagent_model` value with Luna because this is the
Yalda-host contract; a caller can still select any explicit model that Codex's
spawn tool offers. Claude adapter processes inherit `CODEX_CONFIG` unchanged.
Malformed or non-object `CODEX_CONFIG` is an adapter-launch error instead of a
silent configuration reset.

Yalda also sets `CODEX_PATH` for Codex adapters. An explicit host value remains
authoritative; otherwise Yalda resolves the standalone `codex` CLI from its
process PATH or the user's login shell. This deliberately bypasses
`codex-acp`'s bundled runtime, because adapter 1.1.7 bundles Codex 0.145.0,
which rejects Luna subagents. Luna requires a stable standalone Codex CLI
0.147.0 or newer; update it with `codex update` when necessary.

**Applies to.** `acp_channel.rs` — the provider-scoped subprocess environment
assembled by `worker_async`, including the `CODEX_CONFIG` merge helper.

**Enforcement.** `acp_channel::tests::codex_config_sets_luna_and_preserves_other_settings`
pins the structural merge. The subprocess-boundary test
`codex_spawn_injects_luna_subagent_default_without_touching_claude` starts fake
Codex and Claude ACP peers and records their real child environments. The Codex
child must receive Luna plus the inherited configuration; the Claude child must
receive the inherited environment unchanged. The ignored live guard
`tests/codex_luna_subagent_live.rs` asks an authenticated Codex parent to spawn
a default-model child and requires the child's durable
`thread_settings_applied` event to record `gpt-5.6-luna`. The durable event is
used because codex-acp currently reports the parent model when reopening a
spawned child even though the child rollout ran with Luna.

**Status.** `implemented` (pure structural merge, headless real-subprocess
environment check, and an explicit authenticated live guard).
