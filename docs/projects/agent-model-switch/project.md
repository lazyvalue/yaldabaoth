# Project: Per-session agent model switcher

## Problem / why

Agent tiles ran on whatever model the agent defaulted to (or `.claude/settings.json`),
with no in-app way to switch between Opus / Fable / Sonnet per session. The model
was read-only chrome: yalda parsed the current model from `session/new`'s
`config_options` and showed it in the status strip, but discarded the available
list and never issued a change.

## The model (root understanding every ticket assumes)

The `claude-agent-acp` adapter already exposes the model as a **config-option
`Select`** on `session/new` / `session/load`:

- `id: "model"`, `category: "model"`, `type: "select"`
- `currentValue`: the active model id
- `options`: `[{value, name, description}, …]` — the advertised picklist

Observed values (2026-07-09): `default` (Opus 4.8), `claude-fable-5[1m]` (Fable),
`sonnet`, `sonnet[1m]`, `haiku`. **The list is agent-sourced, never hardcoded.**

Switching is a stable ACP request — `session/set_config_option` with
`configId: "model"`, `value: <id>` — proven live to round-trip and echo the
updated selector back. It applies **mid-session** and **preserves the
conversation** (not a new session). The adapter advertises **no** `models`
(`SessionModelState`) field, so the dedicated unstable `session/set_model` API is
NOT used; the stable config-option path is the mechanism.

### Data flow

```
session/new  ─┐
session/load ─┼─ config_options → model Select → ModelChanged + ModelsAvailable
set_config   ─┘                                    (ReplyEvent, crosses server boundary)
ConfigOptionUpdate notif ─────────────────────────┘

GUI reducer (apply_reply_events): ModelsAvailable → AgentState.available_models
                                  ModelChanged     → AgentState.agent_model

switch:  space M <n> / click "model ▾"  → set-model:<id>  → set_agent_model
         → session_server.set_model (server-backed)  |  channel.set_model (direct)
         → worker set-model task → SetSessionConfigOptionRequest
```

The worker runs the set as a **dedicated task** (not folded into either prompt
driver), so a switch applies out-of-band regardless of the prompt-queueing driver
variant, and both driver exits abort it.

## Links

- INV-UX-22 (`docs/ux-invariants.md`) — the behavior contract.
- Mechanism de-risk: node probe against the live agent confirmed the `Select`
  shape + that `session/set_config_option` echoes the new `currentValue`.
- Reference for the dual server/direct path: `cycle_claude_permission_mode`.

## Tickets

| # | Title | Status |
|---|-------|--------|
| 001 | Model switcher end-to-end (worker → proto → server → GUI → tests) | done |
</content>
