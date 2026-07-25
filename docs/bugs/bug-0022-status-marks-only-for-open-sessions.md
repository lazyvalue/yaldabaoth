# bug-0022: status-marks-only-for-open-sessions

**Status:** FIXED
**First seen:** 2026-07-25
**Component:** `docs/components/jump-panel.md` (`UXI-JumpPanel-1/-6/-10`)

## Symptom

"Working vs waiting doesn't appear consistently." Some sessions in the jump panel
show `◆ working` / `✦ your turn`, others show nothing while they are demonstrably
doing the same thing.

## Context / root cause

`AgentRow.awaiting` and `AgentRow.unread` were read **only** off a live in-store
session entity:

```rust
let awaiting = opened.map(|e| e.read(cx).state.turn_phase.is_awaiting()); // None if not open here
let unread   = opened.map(|e| e.read(cx).state.unread).unwrap_or(false);
```

and `dot_status()` maps `awaiting: None` to `Neutral`. But the jump panel lists the
**universal roster** — every session on the server — while `self.sessions` holds
only the ones this GUI has opened. So the mark appeared for a session bound to a
tile and was permanently absent for a free session, a session created from the jump
panel, or one another GUI instance was driving. The status wasn't flickering; it was
structurally unknowable for most rows.

The GUI cannot infer it either: per-session `ReplyEvent`s only reach **subscribers**
of that session, and the roster's `SessionInfo` carried no turn state (`turns`,
`connected`, `permission_mode` only).

## Planned solution

Make the server the source of truth for "a turn is in flight", since it owns every
session's channel:

- `SessionInfo.busy: bool` (`#[serde(default)]` — the daemon outlives GUI restarts,
  so an OLDER server's JSON must still deserialize) set when a prompt is
  accepted/queued, cleared at `TurnCount` (turn settled) and on channel (re)spawn
  (which kills whatever was running).
- New global broadcast `Notification::SessionBusy { session_id, busy }` — like
  `SessionRenamed`, to every connection, so a GUI learns about sessions it is not
  attached to.
- GUI: `AgentRoster::set_busy`; `jump_panel_agent_rows` uses local state when the
  session is open here and falls back to `info.busy`; a busy→idle broadcast for a
  session we're not looking at sets a roster-side unread mark
  (`YaldaGpuiView.roster_unread`), cleared on jump — the twin of
  `AgentState::unread` so "your turn" works for roster-only rows too.

## Approaches already tried (do NOT repeat)

- **Deriving status GUI-side from the event stream.** Can't: the GUI only receives
  a session's events while attached, and attaching every listed session would spawn
  subscriptions (and replay) for sessions the user isn't using.

---

## Log

### 2026-07-25 — server-owned busy flag + global broadcast

**Changed** — `session_proto.rs`: `SessionInfo.busy` (serde-default) +
`Notification::SessionBusy`. `yalda-session-server/main.rs`: `ManagedSession.busy`,
set in `enqueue_prompt`, cleared at `TurnCount` and in `apply_channel_state`
(re-raised only if a queued prompt flushed), `set_busy`/`broadcast_busy`, recovery
starts `false`. `agent_roster.rs`: `set_busy`. `agent_ui.rs`: the `SessionBusy` arm
+ `note_roster_turn_finished` / `mark_roster_session_read`; `mark_session_read` also
clears the roster mark. `jump_panel_view.rs`: local-then-roster status derivation.
`main.rs`: `roster_unread`.

**Verified** — `roster_only_session_shows_live_status`: a roster session this GUI
never opened goes Neutral → Working → WaitingForYou through the REAL
`apply_server_batch` reducer and the REAL row builder, and clears on read.
**NC observed RED**: drop the `.or(Some(info.busy))` fallback → `Some(Neutral)` while
working. Full workspace `cargo test` green.

**Outcome** — fixed in code. **Requires a server restart** (`./dev-server.sh`): a
still-running OLD daemon never sends `SessionBusy`, so until it is bounced the
roster-only rows fall back to whatever the last `list_sessions` said (old servers
report no `busy` key ⇒ `false`). Runtime-unverified end to end (gap 2: the live
GUI↔server↔agent loop).
