# Agent Tile — Model selector

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-16`.

## Description

A per-session model switcher: each agent session can switch its model live from
the picklist the agent advertises, sourced from
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

### UXI-AgentTile-16 — The agent model is switchable per session, from what Yalda asks the agent to advertise

**Statement.** Each agent session can switch its model live (Opus / Fable /
Sonnet / …) from the model picklist the agent advertises. The list is the model
`Select` config option (id `"model"`, category
`Model`) the agent returns on `session/new` / `session/load`; yalda parses its
`current_value` + `options` into `AgentState.available_models` + `agent_model`.
For Claude sessions, Yalda supplies its supported-model allowlist in the
per-session `_meta.claudeCode.options.settings.availableModels` tier. This keeps
the menu Yalda-specific while making the newest Claude models — e.g.
`claude-opus-5-5` (Opus 5.5, released 2026-09-22) and `claude-fable-5-1[1m]`
(Fable 5.1) — selectable even before they enter the adapter's default picker.
Codex and other providers receive no Claude metadata. The adapter remains
authoritative for validation, labels, ordering, and deduplication; an
`availableModels` id the adapter's SDK does not recognize is **not** dropped — it
is surfaced verbatim as a picker option and passed to the API on switch (adapter
`applyAvailableModelsAllowlist` passthrough), so a brand-new model id works before
the adapter ships a label for it.

**The advertised list is config-driven.** It is the compiled default set
(`YALDA_CLAUDE_AVAILABLE_MODELS`) merged, in order and de-duplicated, with an
optional per-install config file — `$YALDA_CLAUDE_MODELS`, else
`~/.config/yalda/claude-models.conf` — of one model id per line (`#` comments and
blank lines ignored). Absent file ⇒ exactly the compiled defaults. The file is
read fresh in the lib crate at each `session/new` / `session/load`, so **adding a
model is a one-line config edit picked up by the next session — no code change,
recompile, or process restart.** The helper `scripts/yalda-add-claude-model.sh
<model-id>` performs that edit (dedup) and reports adapter recognition. Under
`cfg(test)` the path resolver honours a thread-local override (default `None`) so
tests never read the real file.
Switching issues an ACP `session/set_config_option` for the `model` option (NOT a
new session — the conversation is preserved); the agent applies it and echoes the
refreshed selector back, which updates the badge. Three properties:

1. **Agent-validated.** The offered models are exactly `available_models`,
   populated from the advertised `Select.options`. Claude's advertised options
   may be constrained by Yalda's per-session supported-model allowlist. An adapter
   that surfaces no model selector shows no switcher (plain label, no `▾`).
2. **Live, conversation-preserving.** A switch is `set_config_option`, applied
   mid-session (even mid-turn); it does not clear or re-create the session. The
   current model is marked (`✓`) in the picker.
3. **Two reachable gestures, one path.** Keyboard `space → M → <n>` (a dynamic
   "switch model" submenu) and clicking the status-strip model badge (`model ▾` →
   opens the local menu) both dispatch `set-model:<id>` → `set_agent_model`,
   which routes through `session_server.set_model` (server-backed) or the local
   channel's `set_model` (direct-spawn).

**Applies to.** `acp_channel.rs` — `YALDA_CLAUDE_AVAILABLE_MODELS` (compiled
defaults), `claude_models_config_path` (path resolver + `cfg(test)`
`CLAUDE_MODELS_PATH_OVERRIDE` / `with_claude_models_path` seam),
`claude_available_models` (defaults⋈file merge), `agent_session_meta` (advertises
the merged list); `scripts/yalda-add-claude-model.sh` (the add helper);
`ModelOption`, `ReplyEvent::ModelsAvailable`,
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
actually accepts. The agent's advertised picklist remains the UI source of
truth; Yalda's Claude-only allowlist is an input to that advertisement.

**Status.** `implemented` (headless for the config parse, reducer capture, dynamic
menu, and the channel-dispatch; the live ACP `session/set_config_option`
round-trip is the sole `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2 — covered by the `#[ignore]` `tests/model_switch_live.rs`).

**Enforcement.** `acp_channel.rs`: `model_state_parses_select_current_and_options`
(config parse + `model_reply_events`);
`session_meta_advertises_latest_claude_models_only_to_claude` (Opus 5.5 + Fable
5.1 reach the `session/new` wire payload with no file present, no duplicates, none
leak to Codex); `claude_available_models_merges_config_file_over_defaults` (config
file ids are appended to the defaults, deduped, `#`/blank lines skipped, via the
`cfg(test)` path override) and `session_meta_reflects_config_file_addition` (a
config-added id reaches the real `session/new` wire payload through
`agent_session_meta`). `verify_harness.rs`:
`agent_reply_models_available_captures_picklist` (reducer capture),
`agent_menu_lists_advertised_models_and_marks_current` (dynamic submenu + `✓` +
`set-model:<id>` commands), `set_agent_model_issues_set_config_on_channel` (the
real switch path reaches the channel). `tests/model_switch_live.rs`
(`set_model_round_trips_against_real_agent_live`, `#[ignore]`) closes the live
round-trip. Negative controls documented at each test.
