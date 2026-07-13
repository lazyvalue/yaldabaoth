# Agent Tile — Recap

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-15`.

## Description

A session recap panel: invoking "recap this session" generates an LLM prose
summary of the focused session and pins it inside that session's agent tile,
above the subagents/tasks panels, until the user dismisses it. Recaps are keyed
by `SessionId`, so each tile holds its own and one tile's recap never appears in
another; a recap is re-runnable at any time, which supersedes that session's
prior run. Recap generation runs on a throwaway side-channel so it never routes
through the visible transcript reducer and cannot reorder the conversation it
summarizes.

## References

- INV-UX-20 in docs/ux-invariants.md → migrated here.
- `docs/components/agent-tile/README.md` — parent component.

## UX invariants

### UXI-AgentTile-15 — A summoned session recap is pinned and isolated

**Statement.** Invoking "recap this session" (agent menu `R` → `recap-session`)
generates an LLM prose summary of the focused session and pins it **inside that
session's agent tile, above the subagents/tasks panels** (the compose sits below
those), until the user dismisses it (`✕` / `recap-dismiss`). A recap is **specific
to its tile**: recaps are keyed by `SessionId` (`self.recaps`), so two tiles can
each hold their own and one tile's recap never appears in another. It is
**re-runnable** at any time (`⟳` / `recap-session`), which supersedes that
session's prior run. Three hard properties:

1. **Isolation.** Recap generation runs on a THROWAWAY `AcpChannelClient`
   side-channel fed the transcript text inline. Its reply stream NEVER routes
   through the visible transcript reducer (`apply_reply_events`) — summoning a
   recap adds nothing to any session's transcript and cannot reorder it.
2. **Visible progress, last-writer-wins.** While `Generating` the panel shows
   "Summarizing…" and streams chunks in as they arrive; on turn resolution it
   flips to the finished prose (`Ready`), or a reason (`Failed`) on spawn/send
   error or an empty reply. A run token guards every state transition, so a
   superseded (re-run / dismissed) run can never scribble on the current one.
3. **Tile-scoped placement.** The recap renders in its own tile, above the
   subagents/tasks panels and the compose — never in the global jump panel, and
   never in a tile bound to a different session.

**Applies to.** `agent_ui.rs` — `summon_recap` / `rerun_recap` / `start_recap_for`
/ `spawn_recap_worker` / `drain_recap` / `apply_recap_event` / `finalize_recap` /
`fail_recap` / `dismiss_recap` / `dismiss_recap_for`; the `RecapState` /
`RecapStatus` model + the `recaps: HashMap<SessionId, RecapState>` field
(`agent.rs` / `main.rs`); the inline `render_agent_recap` in the agent tile
(`screens.rs`). Chrome-class: native size, unaffected by document zoom.

**Why.** A recap is a manual, re-orienting glance — it must be summonable without
mutating the conversation it summarizes (property 1 is exactly the transcript-
ordering-corruption class this codebase has fought repeatedly), and it must show
its work and never leak a stale worker's output onto a newer request (property 2).

**Status.** `implemented` (headless for the reducer + panel; the live throwaway
subprocess is the sole `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2).

**Enforcement.** `verify_harness.rs`: `recap_summon_sets_generating`,
`recap_chunks_accumulate_and_finalize_ready`, `recap_empty_reply_fails`,
`recap_dismiss_clears`, `recap_rerun_supersedes_stale_run`, and
`recap_panel_paints_in_agent_tile` (layout probe: paints above the compose). Each
drives the REAL menu-dispatched entry point / reducer methods; negative controls
documented at the tests.
