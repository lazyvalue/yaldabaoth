# bug-0002: restore-drops-replayed-history

**Status:** FIXED
**First seen:** 2026-07-15
**Component:** `docs/components/agent-tile/` (transcript replay) · spec §4 (generation
rebaseline) / §9 (additive `AgentEvent` gate)

## Symptom

Sometimes, after restarting the GUI and restoring a session, **several messages
have not been pumped to the screen** — the resumed transcript is missing history
(often everything up to the last live exchange). Reported as "still session-server
pump errors."

## Context / root cause

Server-side replay is lossless (the forwarder tails the full event log from
`log_base` to tip; `forward_notifications` / `publish_snapshot`). Both content
streams are complete mirrors: `Command::Record` records the canonical `Agent`
event AND the legacy `ReplyEvent` for every content event. So the loss is
GUI-side, in the reducer dual-stream handoff.

The §9 additive gate (`apply_server_batch`, agent arm): until a session is
`agent_stream_authoritative` (it has seen a real forwarded `TurnEnded`), the
**legacy `ReplyEvent` stream drives the transcript** and the `Agent` stream is
only *observed* — the caller passed an `Agent` event to `apply_agent_event` only
when `authoritative_before || is_boundary`. Pre-gate, non-boundary `Agent` events
(chunks, `ChannelOpened`) were **skipped**.

That is safe only if the legacy-rendered content is never wiped. But a
**generation rebaseline wipes it.** `apply_agent_event` runs `reset_for_replay`
(clears the whole transcript) when it first sees `event.generation >
claude.generation`. Because pre-gate the caller only forwarded *boundary* events,
the rebaseline was **deferred** to the new generation's first boundary
(`TurnEnded` / `ReplayEnd`) — and by then the legacy stream had already rendered
the entire replayed history for the intervening generation(s). `reset_for_replay`
wiped it, and since the gated reducer had skipped the `Agent`-stream copies, that
history could never be rebuilt.

**Trigger (why "sometimes"):** the first channel is `channel_generation == 0`, so
a single-generation session never rebaselines and replays fine. The bug needs a
**respawn (generation bump) while the gate is still closed** — i.e. the
bumped-from generation completed *no* turn (so no `TurnEnded` ever flipped the
gate). In practice: the agent crashes / times out during the very first turn, the
session-server respawns it (gen 0 → 1), the respawn replays the recovered history
ending in `ReplayComplete` → `Agent{TurnEnded{ReplayEnd} gen1}`. On the next GUI
restart's full-log replay, the deferred rebaseline fires at that `ReplayEnd`,
wiping the whole gen-1 replay that legacy had just rendered. After any *completed*
turn the gate is true and stays true, so subsequent bumps rebaseline correctly —
which is why it doesn't happen every time.

## Planned solution

Apply the rebaseline the instant the newer generation is observed, instead of
deferring it: in the caller's agent arm, forward an event to `apply_agent_event`
also when `event.generation > claude.generation` (a rebaseline signal). The
session-server emits `ChannelOpened` as the FIRST event of every (re)spawned
channel, so the reset then lands at `ChannelOpened` — it wipes only the
strictly-older, superseded generation; the new generation's replay renders via
the legacy stream afterwards and survives (no later same-generation reset).
`ChannelOpened` is content-free, so applying it pre-gate can't double-apply
against the legacy stream.

## Approaches already tried (do NOT repeat)

- <none — first attempt>

---

## Log

### 2026-07-15 11:06 — apply the generation rebaseline eagerly (not deferred)

- Fix: `agent_ui.rs::apply_server_batch` (the `ServerNotification::Agent` arm) now
  also forwards an event to `apply_agent_event` when `event.generation >
  claude.generation` (`is_rebaseline`), so the `reset_for_replay` runs at the
  respawned channel's `ChannelOpened` rather than being deferred to its first
  boundary. This stops the reset from wiping legacy-rendered replay history the
  gated reducer had skipped.
- Guard: `verify_harness.rs::restore_keeps_replayed_history_across_a_gate_closed_generation_bump`
  drives the REAL `apply_server_batch` with the MIXED stream the pump sees on a
  full-log replay (per-event `Agent` twin THEN legacy `ReplyEvent`, matching
  `Command::Record`): gen 0 crashes mid-first-turn (no boundary → gate stays
  closed), gen 1 respawn replays the history and ends on `ReplayEnd`. Asserts the
  replayed answer survives and the superseded gen-0 attempt is wiped.
- Negative control (mandatory): reverted the `|| is_rebaseline` clause →
  test FAILED with an EMPTY transcript ("transcript was:" blank) — the exact
  data-loss symptom, for the right reason. Restored the fix → green.
- Full suite: 370 `cargo test --bin yalda-gpui` green (incl. the reducer
  rebaseline tests `agent_reducer_rebaselines_on_newer_generation` /
  `agent_reducer_drives_transcript_after_gate_flips`, unchanged, and the
  state-machine fuzzer).
- Runtime status: the fix is on the real reducer path and headlessly reproduced.
  The live GUI↔server↔agent loop remains harness gap #2 (no mock session-server),
  so a human runtime confirm — crash/respawn a session on its first turn, restart
  the GUI, verify the history is fully restored — is still worth doing before
  closing the loop, but the drop is reproduced + fixed on the code the pump runs.
