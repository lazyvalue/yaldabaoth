# Agent Tile — Session binding & restore

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-18..19`.

## Description

How a tile remembers, persists, and re-acquires the session it holds across a
restart. The binding (`AgentTile.bound: Option<SessionId>`) is the live 1:1 link
(`spec-agent-session-ownership.md`); this facet covers its **durable identity**: the
bound session's server id is cached on the tile as `resume_sid` and written into the
persisted layout leaf, so restore rebinds each tile to its OWN session.

## References

- ADR-0025 — identity-based binding + auto-resume (the decision).
- `spec-agent-session-ownership.md` — the live 1:1 store invariant.
- `spec-tabs-and-splits.md` Behavior 23–24 — workspace persistence.
- Code: `agent.rs::AgentTile.resume_sid`, `agent_ui.rs::save_agent_ring`,
  `persist.rs::{snapshot_content, restore_layout, PersistedKind::Agent}`,
  `main.rs::restore_agent_leaves`.

## UX invariants

### UXI-AgentTile-18 — A tile auto-resumes ITS OWN session on restart (identity, not index)

**Statement.** The workspace remembers which session occupies which agent tile and,
on restart, **automatically rebinds each tile to that same session** — no picker.
The binding is by **identity**, not position: each agent leaf persists its bound
session's durable server id in the layout leaf, and restore binds each tile to its
own id (session details — mode / draft / cwd — resolved from the id-keyed
side-channel). Order of the session list, cwd drift, and layout changes do not
misbind or fall back to the picker. (The picker remains only for a genuinely
*unbound* tile the user opens manually.) An old pre-identity `workspace.json`
(no per-leaf ids) falls back to positional binding once, then re-saves with ids.

**Applies to.** `agent.rs::AgentTile.resume_sid`; `agent_ui.rs::save_agent_ring`
(stamps `resume_sid`); `persist.rs::snapshot_content` (writes
`PersistedKind::Agent { session_id }`) + `restore_layout` (returns
`(WindowId, Option<String>)` per agent leaf); `main.rs::restore_agent_leaves`
(identity bind, no positional zip).

**Why.** The prior positional zip lost the tile↔session mapping whenever the zip
broke (empty list / cwd mismatch / count change / duplicate sid), dropping the user
into a picker on restart. Users want their sessions back in the same tiles,
automatically.

**Status.** `implemented` (persistence layer, headless — identity round-trips per
leaf; the live re-attach of the resumed session is the runtime tail, harness gap #2).

**Enforcement.** `tests.rs::agent_tile_persists_session_identity_not_index` — the
identity round-trips per leaf through `snapshot_layout`/`restore_layout` (independent
of list order; negative-controlled). AND
`verify_harness.rs::created_server_session_persists_its_id_for_restore` — drives the
REAL `save_agent_ring` for a freshly-CREATED server-managed session (`resume_id`
None, `channel` None) and asserts its id IS persisted (via the store's `sid_of`), not
dropped. **The second test is load-bearing:** the first passed while the app was
still broken because it set `resume_sid` by hand and never exercised the save path
that resolves a created session's id (bug-0001). Live re-attach: human runtime check.

### UXI-AgentTile-19 — An unresumable session shows an inline "start fresh" notice, never a picker

**Statement.** If a tile's remembered session cannot be resumed on restart (the
daemon GC'd it; `session/load` fails/times out), the tile shows a small **inline
"session unavailable — start fresh" affordance** — one click to bind a fresh session
in that same tile. It never drops to the free-session **picker**.

**Applies to.** A new `AgentTile` render state beside transcript + picker; the
restore/attach path in `main.rs` / `agent_ui.rs` + the worker→reducer resume-failure
signal.

**Why.** The user ruled out the picker entirely; a dead session must degrade to an
explicit, one-click recovery in place, not a re-selection chore.

**Status.** `implemented` (headless — the flip + notice paint are proven at the
reconciler/render seam; the live "session gone" attach result driving it is the
runtime tail, harness gap #2). The signal is already the GUI's:
`spawn_attach_sessions(resuming = true)` detects the permanent
`is_session_gone_error` and routes it to `reconcile_session_unavailable` (not the
close→picker path). `resume_sid` is kept so a later restart re-attempts. "Start
fresh" (`start_fresh_after_unavailable`) clears the notice and opens a new session
in the tile.

**Enforcement.** `verify_harness.rs::unresumable_session_shows_inline_notice_not_picker`
— drives `reconcile_session_unavailable` (the method the resuming attach-failure
calls) on a bound restored tile and asserts it flips to `unavailable` (bound None,
picker None, resume_sid kept) AND the `agent-unavailable` notice PAINTS with area.
Negative-controlled (routing to `reconcile_session_closed` / setting the picker →
"must NOT drop to the picker" fires RED). Live "session gone" attach result: human
runtime check.
