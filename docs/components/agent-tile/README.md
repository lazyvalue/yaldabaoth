# Component: Agent Tile

**Status:** living (decomposed — this component is large)
**Component token:** `AgentTile` (⇒ invariants are `UXI-AgentTile-N`)

## Description

An `App::Agent` tile: a **viewport** bound to (at most) one ACP session. The enum
tag `App::Agent` splits into `AgentTile` (the viewport/UX, holds
`bound: Option<SessionId>` in the layout tree) vs `AgentSession` (the conversation —
transcript, channel, tools — owned by the `AgentSessions` store). The store enforces
strict **1:1**: a session is bound by at most one tile; an unbound tile
(`bound: None`) renders the **selector**. The tile's surfaces: a top **status
strip**, the **transcript** (a cached child, `TranscriptView`), the **compose** input
(worksheet inline or chatbox pinned), an optional **recap panel**, and the right
**sidepanel** (Plan + Subagents). Primary code home: `agent.rs`, `agent_ui.rs`,
`agent_sessions.rs`, `screens.rs::render_agent`, `transcript_view.rs`.

## References

- `docs/components/common/text-editing.md` — the compose buffer obeys `TextEditing`.
- `docs/specs/spec-agent-session-ownership.md` — the 1:1 binding + placement choke.
- `docs/specs/spec-agent-presentation.md` — transcript/tool rendering.
- `docs/projects/gpui-responsiveness/` — the cached-child performance model.
- ADR-0019 (Tiles & Apps), ADR-0024 (worksheet = read-only transcript + compose).

## Facets (decomposed files)

- [sidepanel.md](sidepanel.md) — `UXI-AgentTile-1..3`: the segmented right sidepanel
  (Plan + Subagents) and its keyboard focus model.
- [transcript.md](transcript.md) — `UXI-AgentTile-4..8`: the transcript reading
  surface (background, turn headers, subagent swap, render freshness, token splits).
- [compose.md](compose.md) — `UXI-AgentTile-9..14`: the compose input (word-wrap,
  worksheet vs chatbox, paint-on-route, immediate submit, image paste).
- [recap.md](recap.md) — `UXI-AgentTile-15`: the pinned, isolated session recap.
- [model.md](model.md) — `UXI-AgentTile-16`: the per-session model selector.

<!-- Add facets as more of the agent tile's behavior is migrated in:
     session-binding.md, status-strip.md, … -->

## UX invariant index (authoritative list)

| Id | Facet | Title | Status |
|----|-------|-------|--------|
| UXI-AgentTile-1 | sidepanel | Plan + Subagents live in a segmented right sidepanel | implemented |
| UXI-AgentTile-2 | sidepanel | Subagents are one-per-line; a subagent detected structurally | implemented |
| UXI-AgentTile-3 | sidepanel | `Cmd-0` focuses the sidepanel; vim selects (2-D), Esc restores | implemented |
| UXI-AgentTile-4 | transcript | Agent text uses the normal tile/desktop background | implemented |
| UXI-AgentTile-5 | transcript | No empty turn header | implemented |
| UXI-AgentTile-6 | transcript | Focusing a subagent swaps the main agent view to its context | implemented |
| UXI-AgentTile-7 | transcript | A moved transcript fingerprint is ALWAYS rendered (no stale tail) | implemented |
| UXI-AgentTile-8 | transcript | A tool call never splits an agent text token | implemented |
| UXI-AgentTile-9 | compose | The agent compose always word-wraps | implemented |
| UXI-AgentTile-10 | compose | Worksheet renders inline-flush; chatbox renders as a pinned box | implemented |
| UXI-AgentTile-11 | compose | The worksheet is an inline-editable conversation buffer | partial |
| UXI-AgentTile-12 | compose | Keystrokes that route to the compose are ALWAYS painted | implemented |
| UXI-AgentTile-13 | compose | A submit is delivered immediately (even mid-turn); failed sends queue | implemented |
| UXI-AgentTile-14 | compose | A pasted image is staged, shown, and sent as a content block | implemented |
| UXI-AgentTile-15 | recap | A summoned session recap is pinned and isolated | implemented |
| UXI-AgentTile-16 | model | The agent model is switchable per session, from what the agent advertises | implemented |
