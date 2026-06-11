# 001 — Refactor the agent session/tile ownership model

**Project:** agent-model-refactor
**Status:** ✅ core complete — #1/#3/#4/#5 done & merged to `main`; #2 + #6 are tracked follow-ups
**Opened:** 2026-06-10
**Branch:** `agent-session-owner` (worktree `.claude/worktrees/agent-session-owner`)
**Spec:** `docs/specs/spec-agent-session-ownership.md` (lives on the branch; lands on `main` at merge)
**Live task mirror:** in-session task list #1–#5 (this ticket is the durable copy)

---

## Problem

Agent session management has been broken for months and survived two large
"redesign" refactors. Symptoms: two agent tiles mirroring each other's input
and output, "attached ×4" (one session attached multiple times), duplicate
forwarders, stuck-reconnecting ghost tiles, the picker letting you re-attach an
already-open session.

**Root cause (one defect, many hats):** there was no enforced invariant binding
a session to a tile, and the binding state lived in raw public fields
(`AgentSlot.server_session_id`, `AgentRing.slots`) that ~11 code paths mutated
directly with no coordination. `for_each_server_session_slot` then *fanned a
session's events out to every tile holding it* — turning any duplicate binding
into visible mirroring instead of a loud failure.

**Why the big refactors didn't fix it:** they were *layer* refactors (transcript
reconciler, render pipeline, session-server actor, lease). Each made its own
layer internally correct. This bug lives in the **seam** between the server's
notion of a "session" and the GUI's notion of a "tile" — no layer owned the
seam, so the symptom relocated to whichever bind path wasn't hardened that round.

The whole multi-subscriber apparatus (forwarder fan-out, leases, owner/observer,
candidate/promote) existed for ONE feature: the self-hosting blue-green
`:promote` loop (old + new build watching one live session). Scott confirmed
that feature is **not needed** — which removes the entire reason for many-to-many
binding.

## Decision

1. **Strict 1:1.** A server session is shown in at most one tile. No mirroring.
2. **One owner.** A single struct (`SessionStore`, alias `AgentSessions`) owns
   all agent-session state behind a private API. The `sid → SessionId` index is
   private. Two sessions for one sid is *unrepresentable* — the only way to get a
   session for a sid is `open_or_focus`, which returns the existing one.
3. **Ownership inversion.** Session state moves OUT of the layout tree into the
   store; tiles hold lightweight `SessionId` keys. Routing is an O(1) map lookup.
4. **`:promote` machinery → dormant now, deleted later.** The client stops using
   the server's lease/forwarder-fan/owner surface; the server is untouched this
   pass (no protocol change) and cleaned up in a separate ticket.

## The model (final, after design discussion)

```
App::Agent(AgentTile)          App::Buffer(BufferApp)
        │                              │
        │ bound: Option<SessionId>     │ (view onto the buffer pool)
        ▼                              ▼
  AgentSessions store            file-buffer pool
```

- **`App::Agent` is just the enum tag** — not a third entity, owns nothing
  beyond its `AgentTile` payload. The real split is two homes:
  - **`AgentTile` = the viewport (UX state), in the layout tree.** `bound`,
    input mode (Worksheet ⇄ Message Box) + chatbox draft, cursor mode, scroll,
    render caches, transient status, focused sub-agent, sidebar toggles, picker,
    `pending_open_token`.
  - **`AgentSession` = the conversation, in the store.** Transcript editor, ACP
    channel + attach state, tool calls, turn phase, plan, agent mode, permission
    mode, usage, generation, identity (label/cwd/resume_id).
  - Litmus test for which side a field is on: *"does it still mean something when
    no tile is showing this session?"* Yes → session. No → tile.
- **Free sessions + rebind.** A session is **free** when no tile binds it
  (`free = store ids − tiles' bound ids`). A tile can be **rebound** to any free
  session; the old one frees and keeps running (rebind never kills). Closing a
  tile frees (not kills) its session. Killing is an explicit, separate act.
- **Unbound tile renders the selector** — the `SessionPicker` listing free
  sessions + "create new". Session-close / unbind / rebind all leave the tile as
  `App::Agent` with `bound: None` showing the selector; a tile never vanishes or
  silently becomes a Buffer.
- **No `underlying` buffer, and no "leave agent" gesture.** Agent and Buffer are
  fully orthogonal App variants; an Agent never nests/stashes a Buffer. Flipping a
  tile loses nothing (both are views onto pools). (Ctrl-V was removed — an agent
  tile stays an agent tile; close it or open a Buffer tile normally.)

### Canonical agent-tile commands (in the `.` local menu)

1. **Select session** — opens the selector (free sessions + create-new); on a
   bound tile this is the rebind flow.
2. **Stop** — `stop_agent` (Cmd-.).
3. **Send message** — `submit_agent` (Ctrl-Enter).
4. **Switch Worksheet ⇄ Message Box** — `toggle_agent_input_mode`
   (Ctrl-Alt-Enter). Message Box = the chatbox (compose box, transcript
   read-only); Worksheet = the transcript itself is editable.

(Existing extras — tasklist/subagents sidebars, change-cwd, rename — are kept.)

## Subtasks

- [x] **1. Ownership inversion (strict 1:1).** Done & merged. Code landed,
  5-lens adversarial review done** (21 findings → 3 must-fix +
  8 should-fix); **fixes in progress** (impl agent). ⏳ then re-check + runtime
  verify (#4). Review verdict: the store layer is correct (1:1 genuinely
  enforced, fan-out gone) — but the live view code *discarded the store's
  recovery info*, so conflicts produced stuck/orphaned tiles instead of focusing
  the existing one. Must-fixes:
  - M1 (root) — `bind_session_sid` collapsed `AlreadyBound(owner)` → `false`;
    callers left an orphan placeholder + never focused the owner. Fix: return the
    owner, `close(orphan)`, rebind tile to owner (the AlreadyOpen semantics the
    dead `show_session` choke already had).
  - M2 — multi-tile restore strands tiles (every leaf re-lists, all get the same
    `Attached`, only `first` binds). Fix: drive restore by the persisted snapshot
    list, one sid per leaf, bind up front.
  - M3 — close/reconcile lands the tile in a *dead* selector (`bound=None` AND
    `picker=None` → permanent "loading…", Enter a no-op). Fix: install a loading
    picker + `spawn_list_sessions_for_picker` after unbinding.
  - Should-fix tail: cwd close-before-create race; `desktop_tile_title` `&self`
    aliasing (UB under SB/Miri); `/clear` leaks the session (frees vs kills);
    `attach_active_agent_session` dual-pump; the harness depends on the ambient
    server (one test weakened — force `session_server=None`); placeholder-rebind
    orphan; save/restore tab-scope mismatch; **missing INV-2 no-mirror +
    multi-tile-restore tests**.

## Progress log

- 2026-06-11 — Stage 2 ownership inversion landed (impl agent, build + 153
  tests green). 5-lens adversarial review (`SessionStore` correct; live placement
  layer leaks orphans on bind-conflict). Full fix set dispatched back to the impl
  agent. Next: re-check the root fix + new tests, then #2 (AgentState split),
  #3 (server cleanup), #5 (docs), and the human #4 runtime check.
- 2026-06-11 — All 11 review fixes applied + re-verified (158 tests, M1 root fix
  confirmed correct, new no-mirror/dedup tests real). **#4 runtime-verified by
  Scott** ("looks good"). **Ctrl-V (leave-agent-to-buffer) removed** — Agent and
  Buffer fully orthogonal. **Merged to `main` (`9776148`)**; full suite green.
  Remaining: #3 (server promote/lease deletion, in flight on
  `agent-server-cleanup`) and #2 (AgentState field split, deferred).
  Store owns `AgentSession`; `App::Agent(AgentRing)` → `App::Agent(AgentTile{
  bound: Option<SessionId>, pending_open_token, picker })`; all 11 bind paths
  routed through `open_or_focus`; delete `for_each_server_session_slot` fan-out,
  `AgentSlot`, ring-cycling, `is_driver`, lease heartbeat; selector on unbound
  tile; rebind-to-free; no `underlying`; the four `.` menu commands.
  **AgentState stays monolithic on `AgentSession` this pass.**
  Gate: `cargo build` + `cargo test` green. (Stage 1 — the `SessionStore` owner +
  5 invariant tests — already committed: `368d369`.)
- [ ] **2. Split `AgentState`** into conversation (→ `AgentSession`) vs viewport
  (→ `AgentTile`) fields. The purity follow-up; deferred from #1. Subtlety: in
  Worksheet mode the editable lines live in the transcript editor (session) — only
  the mode flag + chatbox draft are viewport. *Blocked by #1.*
- [x] **3. Delete dormant server-side `:promote`/lease/owner/forwarder-fan
  machinery** (~232 sites in the session server + protocol surface). Protocol
  change; do only once the client no longer references lease/owner APIs.
  Update `spec-session-server-actor.md`. *Blocked by #1.*
- [x] **4. Runtime-verify the 1:1 model** (GPUI can't be driven headlessly).
  Two tiles → distinct sessions, no mirrored I/O, no "attached ×N"; rebind frees
  the old; close → selector (not buffer/vanish); Ctrl-V → buffer picker;
  Worksheet⇄MessageBox toggle; the four `.` commands; logs show exactly one
  forwarder per session. Use `./dev-all.sh`. *Blocked by #1.*
- [x] **5. Reconcile `spec-agent-session-ownership.md` + `CLAUDE.md`** with the
  final model (App::Agent = tag; AgentTile = UX; AgentSession = conversation; no
  underlying; selector states; command set). Worklog the change. *Blocked by #1.*

### Open follow-ups (surfaced during the work; not blocking)

- [ ] **6. Session-server protocol version handshake (upgrade-skew guard).**
  Surfaced by the #3 review. After the lease/owner deletion, `Request::Attach`
  dropped its non-`#[serde(default)]` `mode`/`client_id` fields with **no
  protocol version negotiation**. Hazard: a NEW GUI connecting to an OLD
  still-running server (e.g. a launchd LaunchAgent kept alive across a GUI
  rebuild, ADR-0013) fails to deserialize the new Attach frame → "bad frame" →
  every attach silently fails. Mitigation today: kill the running
  `yalda-session-server` before launching a rebuilt GUI (`dev-all.sh` does this);
  documented as a ⚠ UPGRADE HAZARD in `spec-session-server-actor.md`. Proper
  fix: implement the deferred `initialize` handshake — client sends a protocol
  version on connect; on mismatch with a reused running server, SIGTERM-handoff +
  relaunch the matching binary (as `install` does). *Accepted + documented during
  #3; not blocking.*

  (#2 above — the AgentState field split — is the other open follow-up: pure
  internal cleanup, deferred from #1, no behavior change.)

## Key invariants (enforced by `SessionStore`)

- INV-1 — one session per sid (`open_or_focus`/`bind_sid` are the only writers).
- INV-2 — at most one tile per session; unbound = free = re-bindable.
- INV-3 — one channel/forwarder per session.
- INV-4 — routing is total and unique (`locate`); fan-out deleted.

## Methodology note

This ticket is the cross-session-durable record. It mirrors the in-session task
list but outlives it. See `CLAUDE.md` → "Project tickets (docs/projects/)" for
the convention.
