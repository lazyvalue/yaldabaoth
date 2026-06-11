# Project: agent-model-refactor

**Status:** 🔄 active — #1 merged to `main` (runtime-verified) + Ctrl-V removed;
#3 in flight; #2/#5(server-doc) pending.
**Branch:** `agent-session-owner` → merged to `main` (`9776148`). #3 on
`agent-server-cleanup`.
**Spec:** `docs/specs/spec-agent-session-ownership.md` (on the branch; lands on `main` at merge).
**Tickets:** `001-ticket-refactor-model.md`.

## Problem / Why

Agent session management was broken for months and survived two large
"redesigns." Symptoms: two tiles mirroring each other's I/O, "attached ×4",
duplicate forwarders, stuck-reconnecting ghosts, the picker re-attaching an
already-open session.

Root cause: **no enforced invariant binding a session to a tile.** Binding state
lived in raw public fields mutated by ~11 uncoordinated code paths;
`for_each_server_session_slot` fanned a session's events to *every* tile holding
it, turning any duplicate binding into visible mirroring. The bug lived in the
**seam** between the server's "session" and the GUI's "tile" — so layer
refactors never reached it. The multi-subscriber apparatus existed only for the
`:promote` self-hosting loop, which Scott has dropped.

## Goals

- A single struct owns all agent-session state behind a private API; "two tiles,
  one session" is *unrepresentable*.
- Strict 1:1 session↔tile; sessions can be **free** (no tile) and **rebound**.
- Delete the multi-subscriber/lease/promote machinery.
- Clean, total agent-tile lifecycle: bound → shows session; unbound → selector.

## Scope

**In:** the GPUI client session/tile model; the session-server cleanup (dormant
machinery) as a gated follow-up; spec + CLAUDE.md reconciliation.
**Out:** the `:promote` feature (deleted), the transcript reconciler / render
pipeline (already correct), the worksheet-insertion behavior (separate issue).

## Model (final)

```
App::Agent(AgentTile)          App::Buffer(BufferApp)
        │ bound: Option<SessionId>     │ (view onto buffer pool)
        ▼                              ▼
   AgentSessions store            file-buffer pool
```

- **`App::Agent` is just the enum tag.** Two real homes:
  **`AgentTile`** = viewport/UX (in the tree); **`AgentSession`** = the
  conversation (in the store). Litmus: *"does it still mean something when no
  tile shows this session?"* → session; else → tile.
- **Free + rebind:** free = store ids − tiles' bound ids. Rebind points a tile at
  a free session; the old one frees and keeps running. Close frees (≠ kills).
- **Unbound tile renders the selector** (free sessions + create-new). Close /
  unbind / rebind keep the tile `App::Agent` with `bound: None`; never vanish,
  never silently become a Buffer.
- **No `underlying` buffer** — Agent/Buffer fully orthogonal. Ctrl-V → fresh
  Buffer picker at cwd.
- **Agent commands** (`.` menu): select session · stop · send message · switch
  Worksheet⇄Message Box.

## Invariants (enforced by `SessionStore`)

INV-1 one session per sid · INV-2 ≤1 tile per session (else free) · INV-3 one
channel per session · INV-4 routing total & unique (no fan-out).

## Tickets

| Ticket | Subtasks | Status |
|---|---|---|
| 001-ticket-refactor-model | #1 ownership inversion · #2 AgentState split · #3 server cleanup · #4 runtime verify · #5 docs reconcile | #1 code-complete (review pending); #2–#5 open |
