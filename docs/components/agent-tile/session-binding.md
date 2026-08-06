# Agent Tile — Session binding & restore

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-18..19`, `-22`.

## Description

How a tile remembers, persists, and re-acquires the session it holds across a
restart. The binding (`AgentTile.bound: Option<SessionId>`) is the live 1:1 link
(`spec-agent-session-ownership.md`); this facet covers its **durable identity**: the
bound session's server id is cached on the tile as `resume_sid` and written into the
persisted layout leaf, so restore rebinds each tile to its OWN session.

## References

- `docs/components/README.md` § Terminology — a session with no durable
  workspace-tile reference is **free**
  (the standing term); a *tile* with no session is **unbound**.
- ADR-0025 — identity-based binding + auto-resume (the decision).
- `spec-agent-session-ownership.md` — the live 1:1 store invariant.
- `spec-workspaces-and-splits.md` Behavior 23–24 — workspace persistence.
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

### UXI-AgentTile-22 — Closing a session requires a typed `yes` confirmation

**Statement.** `x` ("close session") in the agent space-menu no longer closes
anything. It **arms a confirmation** and appends one line to the session's own
transcript — never sent to the agent:

```
> <Yaldabaoth System>: Confirm close session (yes or any key for no)?
```

While armed, the **next submit is consumed**: nothing is sent to the agent, on any
input surface (worksheet You-blocks or chatbox). If the submitted text trims to
exactly `yes`, the session closes (today's `close_active_agent_session` — server
close, store drop, tile → live selector). Anything else **cancels**: the confirm
disarms, no message is sent, and the draft text is left in the compose exactly as
it was. Rules pinned by the user:

1. **Arming changes nothing else.** No focus move, no You-block opened, no compose
   clear. A pre-existing draft stays put; if the user submits it while armed it is
   *not* sent (it cancels) and it stays in the buffer. Reaching a place where `yes`
   can be typed (e.g. `o` in worksheet nav) is the user's job.
   **→ AMENDED by `UXI-AgentTile-23`:** the focus half now holds only when a draft is
   present. With an **empty** compose, arming also drops the user into insert. The
   no-clear half stands unconditionally.
2. **Only `yes` clears** — a trimmed exact match. `y`, `Yes`, `yes please` cancel.
3. **The prompt line stays.** Answered either way, the appended line remains in the
   transcript as a permanent record; a second `x` appends a second line and re-arms.
4. **Arms regardless of turn state** — mid-stream too, matching today's
   unconditional close. Unbound tile (no session) is still a no-op.

**Applies to.** `agent.rs::AgentState.close_confirm_armed`;
`agent_ui.rs::{arm_close_confirm, append_system_line, submit_compose}` (the
interception sits ABOVE the worksheet/chatbox branch so both surfaces are covered);
`main.rs` `"claude-close"` menu dispatch.

**Why.** Closing kills a live agent session irrecoverably (server close + WAL
boundary) and sat one keystroke deep in a menu with no undo. The confirm is
in-transcript rather than a modal so it obeys the tile's existing surfaces and
leaves a durable record of the ask.

**Status.** `implemented` (headless, end-to-end on the real paths).

**Enforcement.** `verify_harness.rs::close_session_requires_typed_yes_confirmation`
— drives the REAL menu dispatch (`dispatch_menu_command("claude-close")`, the exact
command string the `x` entry carries) and the REAL `submit_agent` →
`submit_compose` against the in-process test channel, asserting: (1) arming leaves
the session BOUND and puts the prompt line in the transcript, (2) a `nope` submit
puts **nothing on the wire** (`controls.prompt_rx` empty — proven on the channel,
not inferred from state), keeps the session bound, and leaves `nope` in the compose,
(3) the gate is one-shot — resubmitting that same draft really sends, (4) re-arming
mid-turn and answering `yes` unbinds the tile and still sends nothing. **Two
negative controls observed RED:** pointing `"claude-close"` back at
`close_active_agent_session` → "arming must NOT close the session" fires; disabling
the `consume_close_confirm` call → "a submit consumed by the confirm must never
reach the agent" fires.

**Deviation from plan.** None behaviorally. Two implementation notes: (a)
`append_system_notice` was split so `append_system_line` can splice a RAW line —
the prompt is a `>` blockquote and must not carry the `―` notice prefix; (b)
`dispatch_menu_command` went `fn` → `pub(crate) fn` so the harness can drive the
real menu path rather than calling `arm_close_confirm` directly. Test-side note:
step 4 types into the compose directly instead of reusing `worksheet_real_submit`,
because that helper presses `i` first and mid-turn (a turn is in flight from step 3)
the `i` is literal text — it produced the answer `iyes`. That is a harness artifact,
not a product behavior; the mid-turn chatbox path is what a real user hits there.

### UXI-AgentTile-23 — Arming the close confirm drops you into insert, unless a draft is at risk

**Statement.** `x` ("close session") arms the confirm (`UXI-AgentTile-22`) and, when
the compose is **empty**, also puts the user where `yes` can be typed — so the whole
gesture is `<space>` `x` `yes` `⏎`. The move is exactly the one the app already
makes for text entry, per surface:

1. **Chatbox** — focus moves to the compose and it enters **Insert**.
2. **Worksheet, idle** — a You-block opens at the caret
   (`open_you_block_at_cursor`, which focuses the compose in Insert and reveals it).
3. **Worksheet, mid-turn** — the compose enters **Insert** but focus stays on the
   **transcript**: mid-turn the worksheet routes input to the bottom chatbox
   (`UXI-AgentTile-11` rule 7), and `focus = Compose` there is the state that strands
   focus over a vanished box when the turn ends (the fuzzer-found edge B1).
4. **A non-empty draft suppresses all of the above** — with text already in the
   compose, arming behaves exactly as it did before (no focus move, no You-block, no
   clear). This is deliberate: typing `yes` after a draft would not trim to exactly
   `yes`, so the close would silently **cancel**, and clearing the draft to make room
   would destroy the user's work. A draft means the user finishes the job by hand.

This **amends `UXI-AgentTile-22` rule 1** ("arming changes nothing else… reaching a
place where `yes` can be typed is the user's job"), which is now true only of the
draft case (clause 4). The rest of rule 1 stands: no compose clear, ever, on either
path. It applies on every surface — a workspace tile and the bare agent view alike.

**Applies to.** `agent_ui.rs::arm_close_confirm`; `agent.rs`
(`AgentState::open_you_block_at_cursor`, `InputSurface::is_chatbox`,
`TurnPhase::is_awaiting`).

**Why.** Closing a session was five steps (`<space>` `x`, then reach a typeable
place, `yes`, submit) for what the user reads as one decision. The compose-empty
condition is what makes the shortcut safe: it fires only when there is nothing to
lose and nothing to corrupt the `yes` with.

**Status.** `implemented` (headless, end-to-end on the real close path).

**Enforcement.** `verify_harness.rs::arming_close_drops_into_insert_unless_a_draft_is_at_risk`
— drives the REAL `dispatch_menu_command("claude-close")` twice. **Part A** (empty
compose, idle worksheet resting in nav): after arming, `focus == Compose`, the compose
is in `Insert`, and `you_block_open` is true; then `yes` is typed and submitted with
**no focus/insert call of the test's own**, and the session really closes — so the
assert chain proves the gesture, not a simulated state. **Part B** (draft "half a
thought", stepped back to transcript nav): focus, mode, and draft text are all
unchanged by the arm.
*Negative controls (both observed RED):* delete the auto-insert block → part A's
"arming with an empty compose focuses it" fires (`Transcript` vs `Compose`); make it
unconditional → part B's "must NOT move focus" fires (`Compose` vs `Transcript`).

**Deviation from plan.** None behaviorally. One test-side consequence worth
recording: the pre-existing `close_session_requires_typed_yes_confirmation` used the
`worksheet_real_submit` helper, which presses `i` before typing — now that arming
already enters Insert, that `i` became literal text (`inope`). The test was updated to
type directly, which is also what a real user now does. That failure is itself
evidence the auto-insert lands on the real path.

### UXI-AgentTile-33 — Tagging a session opens a two-column add/remove dialog

**Statement.** The agent space-menu's **`tag session…`** command (`t`) opens a
modal **tag editor** (`session_tag_editor.rs`) — a two-column dialog for editing
the focused session's tags, driveable entirely by keyboard OR mouse. It replaces
the earlier in-tile add/remove prompt.

Layout:

- A type-to-filter **input** at the top.
- **ADD** (left column) — every tag in use across all sessions that this session
  doesn't already carry, filtered by the input; plus a synthetic **＋ create
  "<typed>"** row when the typed text is a novel tag. Activating a row adds that
  tag.
- **ON THIS SESSION** (right column) — the session's current tags, each with a
  trailing `✕`. Activating a row removes that tag.

Interaction is **modal**, consistent with the rest of the app's editors:

- **Normal mode** (on open) — vim navigation: `j`/`k` (or `↑↓`) move within the
  focused column; `h`/`l` (or `←`/`→`/`tab`) switch columns; `enter` toggles the
  highlighted row (add from ADD, remove from ON THIS SESSION); `x`/`d`/`delete`
  remove in the Current column; `i` (or `a`/`/`) enters Insert; `esc`/`q` (or a
  click on the backdrop) closes.
- **Insert mode** — typing edits the filter / new-tag text (and focuses ADD);
  `enter` adds the highlighted ADD row; `esc` returns to Normal.
- **Mouse.** Clicking any row toggles it; hovering moves the highlight and focus
  to that column — independent of mode.
- Adding an already-present tag is a no-op (it's a set); after any add the filter
  clears so the just-added tag hops to the right column.

The palette is **neutral**: selection is the overlay's gray wash and the cool
`agent_tint`/`jump_subheader` accents — never `warm_accent` (the forbidden
gold/brown).

- **A tag is per session, keyed by the server sid**, persisted in the id-keyed
  `session_tags.json` sidecar (`UXI-JumpPanel-20`). A session with **no sid yet**
  (mid-create) can't be tagged: the command sets a `session not ready to tag`
  transient note and opens no dialog.

**Applies to.** `session_tag_editor.rs`: `TagEditorOverlay` / `TagEditorColumn` /
`TagLeftRow`, `tag_editor_model` + `all_known_tags` (the pure column derivation),
`open_tag_editor`, `handle_tag_editor_key`, `activate_tag_editor` /
`tag_editor_add` / `tag_editor_remove`, `render_tag_editor`. `main.rs`:
`ActiveOverlay::TagEditor`, its accessors, the `overlay_is_tag_editor` render +
`capture_key_down` branch, and the `"claude-tag"` → `open_tag_editor` dispatch. The
sidecar mutators `add_session_tag` / `remove_session_tag` (`agent_ui.rs`) are
reused unchanged.

**Why.** The user rejected the linear add-then-remove prompt: they want to see the
session's tags and the pool of existing tags at once, and add/remove any of them in
one place, by keyboard or mouse. A two-column dialog is that surface.

**Status.** `implemented` (headless). The `untag session` command and the whole
`AgentState.tag_prompt` / `arm_*` / `consume_tag_prompt` / `Compose::reset`
in-tile-prompt path are **removed** (superseded by this dialog).

**Enforcement.** `verify_harness.rs`:
`tag_editor_keyboard_adds_and_removes` (opens via the REAL
`dispatch_menu_command("claude-tag")` in Normal mode, then `simulate_keystrokes`
through the REAL capture handler: `i` enters Insert, a typed novel tag + `enter`
creates+adds it, `enter` again adds an existing known tag, `esc` returns to Normal
without closing, vim `l` focuses the Current column, `x` removes the highlight, and
`esc` in Normal closes; NC observed RED by breaking `i`→Insert),
`tag_editor_mouse_click_toggles` (clicks the painted `tag-editor-left-0` /
`tag-editor-current-0` rows to add then remove, through the occluding card), and
`tag_editor_requires_a_sid` (a sid-less session opens no dialog and sets the note).
**Negative controls observed RED:** no-op'ing `tag_editor_add` fails both add
asserts; no-op'ing `tag_editor_remove` fails both remove asserts.

**Deviation from plan.** The requested "select from a list of in-use / previously
used tags" is sourced from `all_known_tags` (the union of every session's tags in
the sidecar) — there is no separate "history" store, so a tag becomes reusable
once it's been applied anywhere. The ADD column hides tags already on the session
(they live in the right column instead of showing as disabled).
