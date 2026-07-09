# Worklog: per-session agent model switcher

**Date:** 2026-07-09
**Branches touched:** `agent-model-switch` (worktree)

## Built (with status)

Live, per-session model switching (Opus / Fable / Sonnet / …) in agent tiles,
sourced from the picklist the agent advertises. Full end-to-end; INV-UX-21.

- **Mechanism (de-risked first).** The `claude-agent-acp` adapter already exposes
  the model as a config-option `Select` (`id:"model"`, `category:"model"`) on
  `session/new`, with `currentValue` + an `options` picklist. A node probe against
  the live agent confirmed `session/set_config_option` (`configId:"model"`,
  `value:<id>`) round-trips and echoes the updated `currentValue` — a **stable**
  request, mid-session, conversation-preserving. The adapter advertises **no**
  `models`/`SessionModelState`, so the unstable `session/set_model` is NOT used.
- **Worker (`acp_channel.rs`).** `ModelOption` + `ReplyEvent::ModelsAvailable`;
  `model_state_from_config_options` / `model_reply_events` parse current + full
  picklist (flattening groups). Emitted at `session/new`, `session/load`, and on
  the previously-dropped `ConfigOptionUpdate`. Outbound switch is a dedicated
  worker task (`TransportHandle::set_model` → std→async bridge → task issuing
  `SetSessionConfigOptionRequest`, re-emitting the selector from the response),
  independent of both prompt drivers, aborted at each driver exit + teardown.
- **Proto + server + client.** `Request::SetModel` → `Command::SetModel` →
  `do_set_model` (errors if no live agent) → `channel.set_model`. `ModelsAvailable`
  records as a plain reply event (flows to GUI + WAL replay, not the transcript).
  `SessionServerClient/Handle::set_model`.
- **GUI.** `AgentState.available_models`; `ModelsAvailable` reducer arm;
  `set_agent_model` (dual server/direct path, mirrors
  `cycle_claude_permission_mode`). Two gestures, one path: `space → M → <n>`
  (dynamic "switch model" submenu, current marked `✓`, children dispatch
  `set-model:<id>`) and a clickable `model ▾` status-strip badge that opens the
  local menu.
- **Tests.** `model_state_parses_select_current_and_options` (lib);
  `agent_reply_models_available_captures_picklist`,
  `agent_menu_lists_advertised_models_and_marks_current`,
  `set_agent_model_issues_set_config_on_channel` (verify_harness) — each observed
  RED with its fix reverted, restored green. `tests/model_switch_live.rs`
  `#[ignore]` closes the live ACP round-trip (gap #2). Full suite green
  (350 pre-existing + 3 new headless), no regressions.

## Decisions

- **Config-option `Select`, not `session/set_model`.** The agent surfaces the
  model as a stable config option and advertises no `SessionModelState`, so the
  stable `session/set_config_option` is the right (and only working) mechanism —
  no new cargo feature. See ADR candidate in `project.md`.
- **Agent-sourced list, never hardcoded.** The offered models are exactly the
  advertised `Select.options`; an adapter with no model selector shows a plain
  label (no `▾`, no submenu).
- **Switch is out-of-band, conversation-preserving.** A dedicated worker task (not
  the prompt queue) issues `set_config_option`; it does not clear/re-create the
  session.

## Open / follow-ups

- **`effort` config option** (`category: thought_level`, low..max) is advertised
  alongside `model` and could get identical treatment — future ticket.
- **Not yet on `main`.** Lives on the `agent-model-switch` worktree; merge +
  rebuild + restart needed to reach the running binary (anti-circling rule 5).
- **Live round-trip** verified via the `#[ignore]` test against the real agent;
  the GUI↔server↔agent full loop through the daemon is the standing `NEEDS-RUNTIME`
  gap (feeds the reducer directly in tests).
</content>
