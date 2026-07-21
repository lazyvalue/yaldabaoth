# bug-0007: jump-panel-sessions-spontaneously-reorder

**Status:** FIXED
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

- <none — first attempt held>

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
