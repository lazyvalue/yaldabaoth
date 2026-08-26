# Component: Agent Tile

**Status:** living (decomposed — this component is large)
**Component token:** `AgentTile` (⇒ invariants are `UXI-AgentTile-N`)

## Description

An `App::Agent` tile: a **viewport** bound to (at most) one ACP session. The enum
tag `App::Agent` splits into `AgentTile` (the viewport/UX, holds
`bound: Option<SessionId>` in the layout tree) vs `AgentSession` (the conversation —
transcript, channel, tools — owned by the `AgentSessions` store). The store enforces
strict **1:1**: a session is selected by at most one Agent tile; an empty tile
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

- [sidepanel.md](sidepanel.md) — `UXI-AgentTile-1..3`, `-17`, `-20`: the segmented
  right sidepanel (Plan + Subagents), its keyboard focus model, and `Cmd-B` hide.
- [transcript.md](transcript.md) — `UXI-AgentTile-4..8`, `-40`: the transcript reading
  surface (background, turn headers, subagent swap, render freshness, token splits).
- [compose.md](compose.md) — `UXI-AgentTile-9..14`, `-21`, `-41..43`: the compose
  input (word-wrap, worksheet vs Message Box, paint-on-route, immediate submit,
  image paste, history recall, and command/Topic completion).
- [naming.md](naming.md) — `UXI-AgentTile-27`: one-shot autonaming + summary of a
  session from its first exchange; an explicit rename latches and wins.
- [recap.md](recap.md) — `UXI-AgentTile-15`: the pinned, isolated session recap.
- [model.md](model.md) — `UXI-AgentTile-16`: the per-session model selector.
- [session-binding.md](session-binding.md) — `UXI-AgentTile-18..19`, `-22..23`, `-34`: durable
  tile↔session identity + auto-resume on restart (the unresumable-session notice,
  and the typed-`yes` close confirmation + its auto-insert).
- [providers.md](providers.md) — `UXI-AgentTile-30`, `-44`: explicit
  Claude/Codex choice with durable provider ownership, subscription-safe Codex
  authentication, and Luna-default Codex subagents.
- [picker.md](picker.md) — `UXI-AgentTile-32`: existing archived sessions are
  absent from both selectable and in-use picker rows.

<!-- Add facets as more of the agent tile's behavior is migrated in:
     status-strip.md, … -->

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
| UXI-AgentTile-8 | transcript | A tool call never splits an agent sentence | implemented |
| UXI-AgentTile-9 | compose | The agent compose always word-wraps | implemented |
| UXI-AgentTile-10 | compose | Worksheet renders inline-flush; chatbox renders as a pinned box | implemented |
| UXI-AgentTile-11 | compose | The worksheet is an inline-editable conversation buffer | partial |
| UXI-AgentTile-12 | compose | Keystrokes that route to the compose are ALWAYS painted | implemented |
| UXI-AgentTile-13 | compose | A submit is delivered immediately (even mid-turn); failed sends queue | implemented |
| UXI-AgentTile-14 | compose | A pasted image is staged, shown, and sent as a content block | implemented |
| UXI-AgentTile-15 | recap | A summoned session recap is pinned and isolated | implemented |
| UXI-AgentTile-16 | model | The agent model is switchable per session, from what the agent advertises | implemented |
| UXI-AgentTile-17 | sidepanel | A subagent row stacks label over prompt (two lines, not side-by-side columns) | implemented |
| UXI-AgentTile-18 | session-binding | A tile auto-resumes ITS OWN session on restart (identity, not index) | implemented |
| UXI-AgentTile-19 | session-binding | An unresumable session shows an inline "start fresh" notice, never a picker | implemented |
| UXI-AgentTile-20 | sidepanel | `Cmd-B` force-hides the whole sidepanel; `Cmd-0` un-hides; persists per session | implemented |
| UXI-AgentTile-21 | compose | `[N]r` over agent text opens a reply You-block seeded with a quotation | implemented |
| UXI-AgentTile-22 | session-binding | Closing a session requires a typed `yes` confirmation | implemented (rule 1 amended by `-23`) |
| UXI-AgentTile-23 | session-binding | Arming the close confirm drops you into insert, unless a draft is at risk | implemented |
| UXI-AgentTile-27 | naming | A session names + summarizes itself once; an explicit rename wins forever | implemented |
| UXI-AgentTile-28 | transcript | The tile says whether the agent is working or waiting on you | implemented |
| UXI-AgentTile-30 | providers | Claude and Codex sessions coexist with durable provider identity | implemented |
| UXI-AgentTile-31 | transcript | Narrow tiles wrap header chrome; usage owns a line | implemented |
| UXI-AgentTile-32 | picker | Archived sessions never appear in the session picker | implemented |
| UXI-AgentTile-33 | session-binding | Tagging a session opens a two-column add/remove dialog | implemented |
| UXI-AgentTile-34 | session-binding | Session selection and workspace ownership are independent | implemented |
| UXI-AgentTile-40 | transcript | `J`/`K` move directly between user turns | implemented |
| UXI-AgentTile-41 | compose | Up/Down recall previously-sent compose messages | implemented |
| UXI-AgentTile-42 | compose | Slash-command autocomplete popup in the compose | implemented |
| UXI-AgentTile-43 | compose | Cog Topic path autocomplete in both compose placements | implemented |
| UXI-AgentTile-44 | providers | Codex subagents default to Luna without changing the parent model | implemented |
