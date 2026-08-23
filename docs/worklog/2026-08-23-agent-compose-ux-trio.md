# Agent compose UX trio — sidepanes / history recall / slash autocomplete

**Date:** 2026-08-23
**Cog graph:** `9vs` (agent-compose-ux-trio) — status `complete`
**Branch:** `compose-ux-trio` → merged to `main`

Three requested compose/agent UX changes, each built in the worktree, guarded
headlessly with an observed-RED negative control, and merged to `main`.

## What shipped

1. **Sidepanes hidden until summoned (UXI-AgentTile-20)** — merge `337f77a`.
   `subagents_open` now defaults `false` in every `AgentState` constructor
   (`tasklist_open` was already false), so a new session shows no sidepanel and a
   newly-detected subagent/plan entry no longer pops it open. The panel appears
   only on an explicit summon (`Cmd-1`/`Cmd-2`/menu). `Cmd-0` (`focus_agent_panel`)
   is itself a summon: it opens content-bearing segments when none are open, so it
   never dead-ends.

2. **Up/Down message-history recall (UXI-AgentTile-41, ADR-0035)** — merge
   `ec2605b`. Per-session `sent_history` ring fed on every successful submit
   (`submit_compose` + `submit_worksheet_blocks`), browsed shell-style: Up on the
   top logical line stashes the current unsent draft and walks back; Down on the
   bottom line walks forward and restores the stash past the newest. Arrows still
   move the caret in a multi-line draft (recall only at the top/bottom line).

3. **ACP AvailableCommandsUpdate → model (n3a)** — merge `5b1648b`. The parked
   notification is promoted to `ReplyEvent::AvailableCommands` and rides the
   canonical `AgentEventKind::AvailableCommands` stream (survives
   `agent_stream_authoritative`), stored on `AgentState::available_commands`;
   `slash_commands()` merges the local `/clear`.

4. **Slash-command autocomplete popup (UXI-AgentTile-42)** — merge `cea723b`. A
   bare `/token` compose draft opens a filtered popup above the input in both
   placements; Up/Down/Tab/Enter/Esc drive it, out-prioritizing the recall nav;
   accept fills `/name` and closes.

## Cog execution evidence

- Graph id: `9vs`

### Initial render

```text
graph agent-compose-ux-trio (frontiers)
frontier 0: n3a-acp-commands [open], n2-history-recall [open], n1-sidepanes-hidden [open]
frontier 1: n3b-slash-popup [open]
frontier 2: omega [open] (omega)
```

### Node execution

Each node was claimed and closed with JSON output (actor `claude-code`):

- `en36` `n1-sidepanes-hidden`: claimed → closed; output: `subagents_open` defaults
  false in all constructors, summon-only, UXI-AgentTile-20 amended, new guard
  `sidepanel_hidden_by_default_until_summoned` NC-RED; merged `337f77a`.
- `9064` `n2-history-recall`: claimed → closed; output: `sent_history` ring +
  `history_up/down/reset` + `Compose::set_recalled`; real-path guards +
  `worksheet_real_submit_populates_history_ring`; NCs RED; UXI-AgentTile-41 +
  ADR-0035; merged `ec2605b`.
- `8y9e` `n3a-acp-commands`: claimed → closed; output: `AvailableCommandsUpdate`
  un-parked to ReplyEvent + AgentEventKind (KnownKind mirror); reducer stores
  `available_commands`; `slash_commands()` merges `/clear`; reducer + wire tests,
  NC RED; merged `5b1648b`.
- `lng5` `n3b-slash-popup`: claimed → closed; output: `slash_query`/
  `slash_popup_rows` + popup key interception + `render_agent` popup; real-path nav/
  accept + layout-probe paint tests, NCs RED; UXI-AgentTile-42; merged `cea723b`.
- `9a4m` `omega`: claimed → closed; output: all four work nodes done, full suite
  green.

### Notes

- **Cmd-0 is a summon.** Making the sidepanes summon-only left `focus_agent_panel`
  with no open column to focus; rather than editing 6 panel-focus tests, Cmd-0 now
  opens content-bearing segments before focusing (it is already one of the listed
  summon gestures, so this is consistent with the requirement and avoids a dead
  end). Only the two-column switch test needed an explicit second summon.
- **Recall gate is the logical line, not visual row** (ADR-0035): keeps vertical
  caret motion in multi-line drafts; recall triggers only at the top (Up) / bottom
  (Down) logical line.
- **AvailableCommands rides the AgentEvent stream too**, not only the legacy
  ReplyEvent — the GUI's ReplyEvent arm goes inert once a session is
  `agent_stream_authoritative`, so a command list re-advertised after a turn would
  otherwise be dropped (same reasoning as `ModelsAvailable`).
- **Popup placement caveat:** the popup pins above the bottom compose region; when
  an inline worksheet You-block sits mid-transcript the popup does not float at the
  caret (acceptable — a fresh `/` message is at the tail). Rendering the popup
  inside the cached `TranscriptView` at the inline caret is a possible follow-up.
- **Genuine gaps flagged:** exact glyphs/theme colors of the popup + recalled-draft
  are harness gap #1 (human eye); the live GUI↔server↔agent loop feeding a real
  `AvailableCommandsUpdate` is gap #2.

### Final status

- Status: `complete`
- omega `9a4m` claimed and closed; `cog graph status 9vs` → `complete`, islands
  none.

```text
graph agent-compose-ux-trio (frontiers)
frontier 0: n3a-acp-commands [done], n2-history-recall [done], n1-sidepanes-hidden [done]
frontier 1: n3b-slash-popup [done]
frontier 2: omega [done] (omega)
```

## Verification

- `cargo test --bin yalda-gpui`: **700 passed**, 2 ignored (default features).
- `cargo test --bin yalda-gpui --features test-support`: **710 passed**, 2 ignored.
- `cargo test --lib`: **181 passed**, 2 ignored.
- Every bugfix/behavior guard was observed RED with its fix reverted (documented at
  each test): sidepanel default→true; recall interception disabled; worksheet
  `history_push` removed; reducer arm removed; popup interception disabled; popup
  render gate disabled.

## Open / follow-ups

- Float the slash popup at the inline-worksheet caret (currently pinned above the
  bottom compose region).
- Persist `sent_history` across restart (currently in-memory; ADR-0035 deferred).
- Add a slash-popup op to the agent-tile state-machine fuzzer op list.
- Runtime confirmation of the live `AvailableCommandsUpdate` feed (gap #2) and the
  popup/recalled-draft colours (gap #1).
