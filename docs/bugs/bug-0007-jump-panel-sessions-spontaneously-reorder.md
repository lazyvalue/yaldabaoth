# bug-0007: jump-panel-sessions-spontaneously-reorder

**Status:** RECURRED → FIXED (2026-07-21; see the latest log entry)
**First seen:** 2026-07-16
**Component:** docs/components/jump-panel.md (`JumpPanel`)

## Symptom

Agent sessions in the jump panel sometimes reorder on their own, with no user
action — "automatically reordered for some weird reason."

## Context / root cause

`jump_panel_agent_rows` (jump_panel_view.rs) builds the list in TWO phases with two
different orderings:
1. Roster-known sessions, in `AgentRoster::entries_by_label()` order (sorted by label).
2. Local-only sessions (opened here but the roster hasn't caught up), **appended in
   store-iteration order**, regardless of label.

So a session's position depends on whether the roster knows it yet. A freshly-created
local session (a just-bound placeholder whose sid isn't in the roster yet) renders
LAST. Then the async `refresh_roster` (a background `list_sessions`) upserts it into
the roster, and on the next render it moves from the appended tail into its
label-sorted slot among the roster rows — it HOPS. Because the trigger is an async
refresh / a `SessionCreated` broadcast (from this or another client), the jump feels
spontaneous and unattributable.

Reproduced headlessly: a roster `claude-2` + a local-only `claude-1` yields
`["claude-2", "claude-1"]` (local appended last) — not label order; once the roster
learns the local sid it becomes `["claude-1", "claude-2"]`.

## Planned solution

Order the COMBINED list (roster + local-only) by label with a stable tiebreak, so a
session occupies the same slot whether or not the roster has caught up — no hop. The
per-cwd grouping + the user's drag-reorder (`order_grouped_rows` / `jump_session_order`)
still apply on top, unchanged.

## Approaches already tried (do NOT repeat)

- **Sorting the combined roster+local rows by label** (2026-07-16) — correct and
  still in place, but it only stabilizes the DEFAULT order. It cannot help when the
  user has a drag order (`jump_session_order`), which ranks by sid; that was the
  2026-07-21 recurrence. Don't re-attack a reorder report at the label-sort layer
  without first checking whether `jump_session_order` is non-empty.

---

## Log

### 2026-07-16 — sort combined roster+local rows by label (FIXED)

**What changed.** `jump_panel_view.rs::jump_panel_agent_rows` now ends with a
`rows.sort_by(label, then jump_target_key)` over the combined roster + local-only
list (new pure helper `jump_target_key` for a deterministic tiebreak). A local-only
session is now placed in its label slot immediately, so the async roster catch-up
no longer moves it.

**How verified.** Localized with an exploratory probe on the real `jump_panel_agent_rows`
(roster `claude-2` + local `claude-1` → `["claude-2","claude-1"]`); probe removed.
Guard `jump_panel_orders_local_and_roster_sessions_by_label` drives the real path and
asserts `["claude-1","claude-2"]`. **Negative control (observed RED):** comment out the
final `rows.sort_by(...)` → local `claude-1` stays appended last → assert fails; restored
→ green. Existing jump-reorder + roster tests still pass. Full suite: 378.

**Outcome.** Sessions keep a stable label-order slot across roster refreshes; the
spontaneous hop is gone. Runtime-unverified (headless-green). Note: label order is
lexicographic (`claude-10` sorts before `claude-2`) — a separate cosmetic nit, not
this bug.

### 2026-07-21 — RECURRED: a session sinks to the BOTTOM after `/clear` (FIXED)

**Symptom (user).** "Agent sessions still move around in the jump panel. I noticed
one agent session moved to the bottom of the list after a clear. Never, ever do
this."

**Why the 2026-07-16 fix did not cover it.** That fix made the *default* (by-label)
ordering stable. This is a different mechanism, in the layer ON TOP of it: the
user's drag order. `~/.yalda/preferences.json` holds a 9-sid `jump_session_order`,
and `order_grouped_rows` ranked a row by **its own server sid**, with anything
unlisted ranked `usize::MAX` (sorts last). `/clear` (`clear_agent_session`) *kills*
the server session and creates a brand-new one — same tile, same label, same cwd,
**new sid**. The new sid is not in the user's order list ⇒ rank `MAX` ⇒ the row
drops to the bottom of its cwd group, and stays there. (It sinks twice over: while
the placeholder is local-only, `sess_rank` returned `None` for every
`JumpTarget::Local` row unconditionally.)

**What changed (this attempt — different from the last).** Session-order identity
is now decoupled from "the current sid", via an explicit succession:

- `AgentRow::order_sid: Option<String>` — the sid a row occupies in the user's
  order. Roster row → its own sid. Local row → its own sid if bound, else its
  predecessor's. `order_grouped_rows::sess_rank` ranks by `order_sid`, not by
  `target` (`jump_panel_view.rs`).
- `YaldaGpuiView::jump_order_succession: HashMap<SessionId, String>` (`main.rs`) —
  placeholder → predecessor sid. Recorded by `clear_agent_session` for BOTH
  branches (`record_order_succession`), consumed at bind by `inherit_order_slot`
  (`agent_ui.rs`), which substitutes the fresh sid for the predecessor's **in
  place** in `jump_session_order` and persists.

Net: the row holds its slot through the whole close → create → bind window, and the
persisted order carries forward with no append and no duplicate.

**How verified.** `verify_harness.rs::clear_keeps_the_sessions_jump_panel_slot`
drives the REAL `clear_agent_session` (forced down the server branch via
`with_server_clear_branch`) and the REAL `apply_open_agent_resolution(Created)`,
snapshotting `order_grouped_rows(group_agent_rows_by_cwd(jump_panel_agent_rows()))`
exactly as the render does. Non-vacuous: the user's order is the REVERSE of label
order, so a surviving slot can only come from the order list. **Negative controls
(both observed RED, separately):** (1) drop the succession fallback from the local
row's `order_sid` → MID-OPEN assert fails with `["a-two","z-one"]` — the exact
sink; (2) early-return from `inherit_order_slot` → POST-BIND assert fails the same
way. Suite: 395 bin tests + 157 lib, all green.

**Spec.** `UXI-JumpPanel-2` gained clause 4: a row NEVER moves except by a user
drag; any future kill-and-recreate flow must record an order succession.

**Outcome.** Fixed on the real path, headless-green. Runtime-unverified (gap: the
panel's painted row order after a live `/clear` is a human check).
