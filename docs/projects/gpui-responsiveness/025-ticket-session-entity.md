# 025 — `Entity<AgentSession>` hoist (ownership only, no behavior change)

The one structural move rev 2 turns on: per-session state becomes a GPUI
entity so the framework's invalidation (notify-at-mutation-site + observation)
can work. This ticket is **ownership mechanics only** — every existing
root-level `cx.notify()` stays, rendering stays in `render_agent`, nothing is
cached yet. Behavior-identical by design so the diff reviews as pure plumbing.

## Goal

`AgentSessions = SessionStore<Entity<AgentSession>>`. All session mutation
flows through `session.update(cx, |s, cx| { …; cx.notify() })`; all reads
through `.read(cx)`. The 1:1 sid-binding invariant and the store API
(`open_or_focus` / `bind_sid` / `locate` / `close`) are untouched —
`SessionStore<P>` is payload-generic (`agent_sessions.rs:55`), so the swap is
a type-parameter change plus call-site mechanics.

## Why this is the keystone

- Notify granularity in GPUI is entity granularity. With sessions as entities,
  "this session changed" is expressible; today only "the whole app changed" is.
- Mutation sites are outside the draw (event handlers, pump reducers, timers),
  so `cx.notify()` there is timing-correct (`project.md` fact 4) — the thing
  rev 1's render-time poll could never be.
- 021 (TranscriptView observes its session) and 022 (compose widget) both
  assume this; doing it as its own behavior-neutral ticket keeps their diffs
  about *their* logic.

## Call-site mechanics (the actual work)

- ~35 direct `sessions.get`/`get_mut` sites across `agent_ui.rs` (19),
  `main.rs` (15), `screens.rs` (1), plus helper accessors. `get_mut(id)`
  followed by field pokes becomes an `update` closure; add a
  `with_session(id, cx, f)` shim to keep sites terse.
- `AgentSession` derefs to `AgentState` — keep the Deref; only the outer
  ownership changes.
- Pump reducers (`apply_server_batch` / `apply_reply_events` /
  `apply_agent_event`) run inside the root's update today; they become
  `session.update` nested within it (GPUI entity updates nest at App level).
  Each reducer that mutates session state calls `cx.notify()` on the session —
  redundant today (root also notifies), load-bearing after 021.
- `render_agent` reads via `.read(cx)`; where it currently takes `&mut` for
  lazy fix-ups, either hoist the fix-up to an update before the render path or
  use `update` from the render's `&mut App` — NO new unsafe; this ticket
  should delete the existing raw-pointer borrow idiom if it gets in the way,
  not add to it.

## Subtasks

- [x] Store payload swap + `with_session` accessor shim; root keeps
      `HashMap`-free single source (the store) — no parallel registries.
      (`AgentSessions = SessionStore<Entity<AgentSession>>`; shims
      `with_session` / `with_session_silent` / `read_session` / `agent_read` /
      `session_entity` on `YaldaGpuiView`.)
- [x] Convert mutation sites (agent_ui, main, screens) to `update` +
      mutation-site `cx.notify()` on the session. (Reducers
      `apply_server_batch`/`pump_session`/etc. notify the session entity inside
      their `update`, redundant with the existing root notify today.)
- [x] Convert read sites to `.read(cx)`; remove/avoid the raw-pointer borrow
      idiom in `render_agent`. (The `unsafe` `*mut AgentState` in `render_agent`
      is gone — the body now builds inside `session_ent.update(cx, …)` with a
      safe `&mut AgentState`; `self`/font/theme locals + the weak handle are
      precomputed before the update, the root listeners assembled after it.)
- [x] Build + full test suite; transcript reconciler seam tests stay green
      untouched (pure `agent_transcript.rs` is unaffected by ownership).
      (`cargo test` = 514 passed / 0 failed; `agent_sessions.rs` and
      `agent_transcript.rs` were NOT touched — store invariant + reconciler
      seam tests pass unchanged.)
- [ ] **Human runtime:** behavior-parity smoke — create/bind/close sessions,
      stream a turn, worksheet + chatbox typing, persistence restore. Nothing
      should look different; this ticket only moves ownership.

## Risks

Borrow discipline around simultaneous root+session access (read-then-clone or
restructure; never unsafe). Persistence paths that serialize sessions must
read through the entity. Watch `Deref` ergonomics hiding a needed `update`
(compile errors will surface these — that's the point of the type change).

## Links

`project.md` (component model), `spec-agent-session-ownership.md`,
`agent_sessions.rs` (store), tickets 021/022 (consumers).
