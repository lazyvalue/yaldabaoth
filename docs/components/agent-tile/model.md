# Agent Tile — Model selector

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-16`.

## Description

A per-session model switcher: each agent session can switch its model live from
the exact picklist the agent advertises — never a hardcoded list — sourced from
the model `Select` config option the agent returns on `session/new` /
`session/load`. Switching issues an ACP `session/set_config_option` rather than
recreating the session, so the conversation is preserved, and the refreshed
selector updates the badge. It is reachable two ways that share one dispatch
path: the keyboard `space → M → <n>` submenu and clicking the status-strip
`model ▾` badge.

## References

- INV-UX-22 in docs/ux-invariants.md → migrated here.
- `docs/components/agent-tile/README.md` — parent component.

## UX invariants

### UXI-AgentTile-16 — The agent model is switchable per session, from what the agent advertises

**Statement.** Each agent session can switch its model live (Opus / Fable /
Sonnet / …) from **the exact picklist the agent advertises** — never a hardcoded
list. The list is the model `Select` config option (id `"model"`, category
`Model`) the agent returns on `session/new` / `session/load`; yalda parses its
`current_value` + `options` into `AgentState.available_models` + `agent_model`.
Switching issues an ACP `session/set_config_option` for the `model` option (NOT a
new session — the conversation is preserved); the agent applies it and echoes the
refreshed selector back, which updates the badge. Three properties:

1. **Agent-sourced, never hardcoded.** The offered models are exactly
   `available_models`, populated from the advertised `Select.options`. An adapter
   that surfaces no model selector shows no switcher (plain label, no `▾`).
2. **Live, conversation-preserving.** A switch is `set_config_option`, applied
   mid-session (even mid-turn); it does not clear or re-create the session. The
   current model is marked (`✓`) in the picker.
3. **Two reachable gestures, one path.** Keyboard `space → M → <n>` (a dynamic
   "switch model" submenu) and clicking the status-strip model badge (`model ▾` →
   opens the local menu) both dispatch `set-model:<id>` → `set_agent_model`,
   which routes through `session_server.set_model` (server-backed) or the local
   channel's `set_model` (direct-spawn).

**Applies to.** `acp_channel.rs` — `ModelOption`, `ReplyEvent::ModelsAvailable`,
`model_state_from_config_options` / `model_reply_events`, the worker set-model
task issuing `SetSessionConfigOptionRequest`, `TransportHandle::set_model`;
`session_proto.rs` `Request::SetModel`; the session-server `do_set_model`;
`session_client.rs` `set_model`; `agent.rs` `AgentState.available_models`;
`agent_ui.rs` `set_agent_model` + the `ModelsAvailable` reducer arm; `main.rs`
`agent_local_menu_dynamic` + the `set-model:` dispatch; the clickable badge in
`screens.rs`. Chrome-class: the badge renders at native size (unaffected by
document zoom).

**Why.** The model is a first-class per-task choice (Opus for hard work, Sonnet
for routine, Fable for the longest runs), and it must reflect what the agent
actually offers rather than drifting from a hand-maintained list — the agent's
advertised picklist is the single source of truth.

**Status.** `implemented` (headless for the config parse, reducer capture, dynamic
menu, and the channel-dispatch; the live ACP `session/set_config_option`
round-trip is the sole `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2 — covered by the `#[ignore]` `tests/model_switch_live.rs`).

**Enforcement.** `acp_channel.rs`: `model_state_parses_select_current_and_options`
(config parse + `model_reply_events`). `verify_harness.rs`:
`agent_reply_models_available_captures_picklist` (reducer capture),
`agent_menu_lists_advertised_models_and_marks_current` (dynamic submenu + `✓` +
`set-model:<id>` commands), `set_agent_model_issues_set_config_on_channel` (the
real switch path reaches the channel). `tests/model_switch_live.rs`
(`set_model_round_trips_against_real_agent_live`, `#[ignore]`) closes the live
round-trip. Negative controls documented at each test.
