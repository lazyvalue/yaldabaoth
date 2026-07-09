# 001 — Model switcher end-to-end

**Goal.** Let a user switch an agent tile's model (Opus / Fable / Sonnet / …)
live, per session, from the picklist the agent advertises. See `project.md` for
the model + data flow. Behavior contract: INV-UX-21.

## Subtasks

- [x] **Worker (`acp_channel.rs`).** `ModelOption` + `ReplyEvent::ModelsAvailable`;
  `model_state_from_config_options` / `model_reply_events` (parse current +
  options, flatten groups); emit at `session/new`, `session/load`, and on
  `ConfigOptionUpdate`. Out-of-band set-model channel (`TransportHandle::set_model`
  / `AcpChannelClient::set_model` → std→async bridge → dedicated worker task
  issuing `SetSessionConfigOptionRequest`, re-emitting the selector from the
  response). Aborted at both driver exits + teardown.
- [x] **Proto + server.** `Request::SetModel`; `Command::SetModel` + `do_set_model`
  (errors if no live agent) + `send_set_model` + request dispatch. `ModelsAvailable`
  returns `None` from `agent_kind_from_reply` → recorded as a plain reply event
  (flows to GUI + WAL replay, not the transcript).
- [x] **Client handle.** `SessionServerClient::set_model` + `SessionServerHandle::set_model`.
- [x] **GUI.** `AgentState.available_models`; `ModelsAvailable` reducer arm;
  `set_agent_model` (dual server/direct path, mirrors `cycle_claude_permission_mode`);
  dynamic `agent_local_menu_dynamic` "switch model" submenu (current marked `✓`,
  children dispatch `set-model:<id>`); `set-model:` dispatch handler; clickable
  `model ▾` status-strip badge → opens local menu.
- [x] **Tests.** `model_state_parses_select_current_and_options` (lib);
  `agent_reply_models_available_captures_picklist`,
  `agent_menu_lists_advertised_models_and_marks_current`,
  `set_agent_model_issues_set_config_on_channel` (verify_harness, each
  negative-controlled RED); `tests/model_switch_live.rs` `#[ignore]` (live
  `set_config_option` round-trip).
- [x] **Docs.** INV-UX-21; this project record.

## Verification

- `cargo test --lib --bin yalda-gpui --bin yalda-session-server --features test-support` — green.
- Negative controls observed RED (reducer arm, submenu push, `ch.set_model`).
- Live round-trip: `cargo test --test model_switch_live -- --ignored`.

## Notes / follow-ups

- The `effort` config option (`category: thought_level`, values low..max) is
  advertised alongside `model` and could get the same treatment (a future ticket).
- Server-side: a model set on a session with no live channel errors (no persisted
  field to re-apply on spawn, unlike permission mode). Acceptable — the switcher
  only shows once a session is live.
</content>
