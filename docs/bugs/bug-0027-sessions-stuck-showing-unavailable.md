# bug-0027: sessions-stuck-showing-unavailable

**Status:** FIXED (needs a session-server restart to take effect)
**First seen:** 2026-07-31
**Component:** `docs/components/jump-panel.md` (`UXI-JumpPanel-12`)

## Symptom

Sessions are frequently listed as `Unavailable` in the jump panel even though
the agent is demonstrably alive — the reporter's own live, actively-replying
session showed `Unavailable` while it was mid-turn. The state eventually
corrects itself, which makes it read as "temporarily unavailable".

## Context / root cause

This is the exact residue of **bug-0022**. That fix made `busy` roster-wide by
adding `SessionInfo.busy` plus a global `SessionBusy` broadcast. It did not do
the same for `connected` — and `connected` is the only input to `Unavailable`
(`jump_panel_view.rs::AgentRow::activity`, `connected: info.connected` at the
roster row builder).

`connected` means "an agent subprocess is live right now"
(`yalda-session-server/main.rs::ManagedSession::info`,
`channel.as_ref().is_some_and(|c| c.is_connected())`). Three routine moments
make it genuinely false on the server:

- **create** — `do_create` inserts the session and broadcasts `SessionCreated`
  with `channel: None` (so `connected: false`), *then* spawns the agent on a
  background thread that performs the blocking initialize + `session/new`
  handshake.
- **WAL recovery** — every recovered session is built with `channel: None` and
  re-resumed by an async worker.
- **agent exit** — `Command::AgentDisconnected` sets `s.channel = None`.

The defect is that the transition **back** to connected is never published.
`Command::PublishChannel` — the moment the subprocess becomes live — calls
`broadcast_busy` (bug-0022's addition) and broadcasts nothing about
connectivity. No notification in `session_proto::Notification` carries
`connected` at all.

So `AgentRoster.connected` is a snapshot frozen at whatever the last **full**
`list_sessions` said. `refresh_roster` is explicitly documented as "a seed, not
a poll" — it runs at boot/connect and when a selector opens. Between seeds the
`SessionCreated` / `SessionClosed` / `SessionRenamed` / `SessionBusy`
broadcasts keep the roster live, and none of them touches `connected`.

Result: a session created at time T enters the roster with `connected: false`
and **stays** Unavailable — through its whole life, mid-turn included — until
some unrelated action happens to trigger a roster reseed. That is the
"temporary" the reporter sees: it is not the spawn window, it is the gap until
the next seed.

Note the row builder already applies a local-authoritative override for `busy`
(bug-0022: `local_activity … .or(Some(info.busy))`) but not for `connected`, so
even a session this GUI holds open and is actively streaming cannot correct its
own row.

## Fix (as shipped)

Mirrored bug-0022 exactly, one axis over:

1. `session_proto.rs` — add `Notification::SessionConnected { session_id,
   connected }`.
2. `yalda-session-server` — broadcast it at every real transition:
   `PublishChannel` (true), `AgentDisconnected` (false), `SpawnFailed` (false).
3. `yalda-gpui` — `AgentRoster::set_connected` plus the reducer arm in
   `apply_server_batch`, notifying on a real change.

Server-side is the right layer (as in bug-0022): it fixes every GUI and every
roster-only session at once, rather than papering over it with a local override
that only helps sessions this GUI happens to hold.

Like bug-0022, this **needs a session-server restart** to take effect — the
daemon outlives the GUI.

## Approaches already tried (do NOT repeat)

- Nothing yet. Do NOT "fix" this by polling `list_sessions` on a timer: the
  roster is deliberately broadcast-driven, and a poll would reintroduce the
  same staleness window at a shorter period while adding per-tick cost.

---

## Log

### 2026-07-31 — root-caused and fixed (attempt 1)

Reported as "frequently sessions are listed (temporarily) as unavailable", then
sharpened by the reporter: *"this session is listed as 'unavailable' even though
you are very clearly available"* — which ruled out the spawn-window explanation
and pointed at a stale flag rather than an honest one.

Traced `Unavailable` back through `AgentRow::activity` → `info.connected` →
`AgentRoster` → the broadcast set, and found `connected` has no live source at
all: `refresh_roster` is a seed, and none of the four manager-level broadcasts
carries it. `PublishChannel` — the exact moment connectivity becomes true —
publishes `busy` and nothing else.

Shipped:

- `session_proto.rs` — `Notification::SessionConnected { session_id, connected }`.
- `yalda-session-server/main.rs` — `broadcast_connected`, called from
  `PublishChannel` (true, gated on the session still existing),
  `AgentDisconnected` (false), and `SpawnFailed` (false).
- `agent_roster.rs` — `AgentRoster::set_connected`, returning `true` only on a
  real flip. It deliberately does NOT touch `state_since`: coming back online is
  not a Waiting/Working transition and must not reorder the state tabs.
- `agent_ui.rs` — the `SessionConnected` arm in `apply_server_batch`.

**Guard.** `verify_harness.rs::agent_coming_online_clears_the_unavailable_row`
drives the real reducer through the real broadcast sequence: `SessionCreated`
with `connected: false` (as the server actually sends it) → `Unavailable`;
`SessionConnected { true }` → `Waiting`; `SessionBusy { true }` → `Working` (the
reported symptom, asserted directly); `SessionConnected { false }` →
`Unavailable` again.

**Negative control observed.** Removing the `SessionConnected` arm from
`apply_server_batch` failed the guard with `left: Unavailable, right: Waiting` —
the row stayed unavailable after the agent came up. Restoring it returned green.

**Suites.** GPUI harness 514, library 161, session-server 47 — all green.

**Not done until the daemon restarts.** The session server outlives the GUI, so
the running daemon is still the old binary that never broadcasts. Exactly like
bug-0022, the user must restart `yalda-session-server` for this to take effect.

**Left open deliberately.** The row builder still has no local-authoritative
override for `connected` the way it does for `busy`. With the broadcast in place
it should not need one; if the symptom ever recurs against a restarted daemon,
that override is the next place to look — not a `list_sessions` poll.
