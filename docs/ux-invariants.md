# Spec: UX Invariants (LEGACY — migrated to docs/components/)

> **⚠️ FROZEN / LEGACY.** Every `INV-UX-N` below has been migrated into a
> per-component spec under `docs/components/` as a `UXI-<Component>-N` invariant,
> which is now **authoritative**. This file is kept only so the `INV-UX-N`
> references still living in code comments, tests, and prose resolve. **Do not add
> or edit invariants here** — add `UXI-<Component>-N` in the owning component spec
> (via `/new-ux`). The `INV-UX-N → UXI-<Component>-N` crosswalk is in
> `docs/components/README.md`.

**Status:** FROZEN — superseded by `docs/components/`.
**Last updated:** 2026-06-29 (frozen 2026-07-12)

## What this is (historical)

The single, canonical list of **behavioral invariants every tile and UX element
must honor.** Element-specific specs (`spec-chatbox-caret-containment.md`,
`spec-textbox-compose.md`, `spec-agent-presentation.md`, `spec-yux.md`, …) refine
*how* a given surface satisfies these; this file states *what* is true everywhere,
so a reader (or reviewer, or future change) has one place that says "the cursor is
always visible" without re-deriving it per surface.

## How to use it (historical — see `docs/components/`)

The rules below describe how this file worked while it was authoritative. It no
longer is — the same disciplines now apply to the `UXI-<Component>-N` entries in the
per-component specs under `docs/components/`:

- **Every code change is still checked against the invariants** — but against the
  owning component's `UXI` list, not this file.
- **New UX is added as `UXI-<Component>-N`** in the component spec (via `/new-ux`),
  never as a new `INV-UX-N` here.
- **Each invariant still names its enforcement** and carries a status —
  `implemented` / `partial` / `not implemented` (the old words here were
  `honored` / `partial` / `target`).

## Invariants

### INV-UX-1 — The cursor is always visible, and moving it moves the visible text

**Statement.** In any tile or element that has a cursor/caret, the caret is always
within the visible viewport. Moving the caret scrolls the content so the caret
stays visible — both vertically and horizontally. The caret is never stranded
off-screen (above, below, or past the right edge), and the viewport never shows a
region the caret has left.

**Applies to.** Every editable/navigable surface: file read/edit buffers
(`EditView` Code + WordProcessor), the rendered doc view cursor, the agent
transcript navigation caret (transcript-focus), and the agent compose buffer
(worksheet inline + chatbox pinned). Any future surface with a caret.

**Why.** A caret you can't see is a caret you can't use — you don't know where you
are or where your edit will land. This has been the single most-regressed UX
property in the app (the chatbox caret-offscreen bug, "fixed" 15+ times; the
worksheet caret-below-buffer bug; the streaming caret-drift bug).

**How (the discipline that satisfies it).**
- A surface computes its scroll window from the CURRENT caret + the MEASURED
  viewport extent, at ONE chokepoint, and scrolls *to* that window — it never
  reads the scroll offset back or lets a stale offset win
  (`spec-chatbox-caret-containment.md`: `compute_window` for the compose;
  `ListView`/`ScrollAnchoredList` splice-anchoring elsewhere).
- Programmatic edits (streaming, freeze, paste) that shift text shift the caret
  with it, so the caret never drifts out of view (the `splice_insert`/
  `splice_delete` cursor-shift discipline).
- A pending-reveal latch re-renders so the reveal is consumed on the next frame.

**Status:** `honored` — vertical caret containment on all surfaces. The agent
compose no longer needs the horizontal axis at all: it **word-wraps** (INV-UX-2),
so the caret is always on a rendered visual row and there are no off-screen-right
columns. Other monospace surfaces that still scroll horizontally keep the
`compute_window` horizontal half.

**Enforcement.** Headless, two levels:
- **Model:** the caret-containment guards
  (`chatbox_caret_cell_stays_in_window_for_every_edit_path`); the WRAPPED-compose
  vertical-containment guard `compose_wrapped_caret_never_below_the_fold`; the
  worksheet caret-on-tail / streaming-cursor tests.
- **Paint (the real proof):** `compose_caret_row_painted_inside_box_when_wrapped`
  drives a real layout/paint pass (`run_until_parked`) and asserts — via the
  layout probe (`probe_bounds` / `layout_probe_*` in render_blocks.rs) — that the
  caret's row is actually PAINTED inside the compose box. The virtualized list
  never paints an off-screen row, so a caret below the fold fails the test
  (validated by injecting the regression). This closes what a model test can't
  (does the list actually scroll + paint there); the probe is the reusable #3.2
  capability for any painted-geometry assertion.

> **Subtlety (regressed once by INV-UX-2, now pinned):** under word-wrap a logical
> line spans multiple VISUAL rows, so the compose's vertical window MUST be
> computed in visual-row space (`compose_visual_metrics` →
> `compose_first_visible_line` → `compose_item_for_visual_row`). Computing it over
> logical lines strands the caret below the fold — the recurring chatbox-cursor
> bug, reintroduced by the wrap change and re-fixed.

### INV-UX-2 — The agent compose (chatbox / worksheet) always word-wraps

**Statement.** The agent tile's compose buffer wraps long lines to the available
width. Text never runs off the right edge of the box requiring horizontal scroll
to read it; a long line flows onto the next visual row.

**Applies to.** The agent compose buffer in BOTH placements
(`InputModeKind::Chatbox` pinned box, `InputModeKind::Worksheet` inline).

**Why.** A compose box is for composing prose; horizontally-scrolled input is
unreadable and you lose sight of what you wrote. Wrapping keeps the whole draft
visible.

**Status:** `honored` (runtime-unverified for paint, per the GPUI headless gap).
The compose **word-wraps**: `wrap_line_cols` (agent.rs) partitions each logical
line into ≤width visual rows at space boundaries (over-long words hard-break),
covering every char; `build_chatbox_wrapped_line` renders one visual row per
segment via `build_chatbox_line` (each row sliced to exactly its segment ⇒ no
clip, no horizontal scroll), with the caret on the row `caret_visual_row` picks.
The small/virtualized decision keys on TOTAL VISUAL rows so a long wrapped line
can't overflow the un-scrolled small box. This **retired the compose's
horizontal-scroll window** (`spec-chatbox-caret-containment.md` horizontal axis);
the vertical caret-containment is kept.

**Enforcement.** Headless: `wrap_line_cols_word_wraps_and_covers_every_char`
(wraps, hard-breaks, covers every char, ≥1 row, makes progress) +
`caret_visual_row_places_caret_on_a_rendered_row` (caret always on a rendered
row). Runtime (GPUI paint not headless): type a line wider than the box in both
placements and confirm it wraps with the caret visible.

### INV-UX-3 — Agent text uses the normal tile/desktop background

**Statement.** Agent transcript text sits on the SAME background as the normal
yalda desktop / tile — there is no per-turn "card" background tint behind agent
or user turns. Turns are distinguished by the gutter label, the foreground
author tint, and the left bar — never by a different background color.

**Applies to.** The agent transcript (`TranscriptView`). The transient
focused-row highlight (a dim band on the cursor row, shown ONLY while the
transcript is focused for navigation) is NOT a violation — it's a focus/nav cue,
not a resting background. Code blocks keep their own background (code styling, not
a turn card). The compose box keeps its pinned-control affordance.

**Why.** A tinted card per turn makes the transcript read as a separate surface
floating on the desktop; the agent's text should blend into the tile like every
other surface, so the workspace looks like one continuous space.

**Status:** `honored` (runtime-unverified for paint). `transcript_view.rs` sets
`row_bg` to transparent for every committed turn; the per-turn `claude_turn_bg`/
`user_turn_bg` (theme `agent_turn_bg`/`user_turn_bg`) tints are no longer applied.
The cursor-row dim highlight remains, gated on transcript focus
(`cursor_line == usize::MAX` when composing, so no row matches).

**Enforcement.** Runtime (GPUI paint not headless): open an agent tile and
confirm agent/user turns show no background tint distinct from the tile; the
focused row highlights only during transcript (`f`) navigation. (A headless guard
awaits the element-tree-snapshot harness — `docs/projects/headless-e2e/` #3.2.)

### INV-UX-4 — No empty turn header

**Statement.** A `You` / `Claude` turn divider is rendered ONLY for a turn that
has visible content — a prose line, a rendered block, a tool group, or the
in-flight thinking indicator. The transcript never shows a turn header with
nothing under it, and never a stack of empty alternating `You`/`Claude`
dividers.

**Applies to.** The agent transcript (`rebuild_agent_view_model` →
`FlatItem::TurnHeader`).

**Why.** Empty turns are visual noise that make the conversation unreadable and
imply exchanges that didn't happen (the reported "blank turns" — a screenful of
empty `You`/`Claude` dividers between the real turns). They arise when a turn's
only lines are blank (stripped by the blank-collapse pass) or when blank
separator / resume-artifact lines carry their own escalating turn numbers.

**Status:** `honored`. After the flat-item build (blank-collapse, tool-group
merge, thinking indicator), `rebuild_agent_view_model` runs a right→left pass
that drops any `TurnHeader` with no non-header item before the next header.

**Enforcement.** Headless: `rebuild_drops_empty_turn_headers` builds a transcript
with empty turns (blank lines carrying escalating turn numbers) interleaved with
real turns and asserts no header is orphaned (every header is followed by content;
header count == content-bearing-turn count). Validated by disabling the pass →
the test fails.

### INV-UX-5 — Subagents are detected from the harness, shown as a one-per-line list above the compose

**Statement.** Sub-agents (the agent's `Task` spawns) are detected from the
STRUCTURE the harness emits over ACP — not a name heuristic — and surfaced as a
compact **one-per-line list ABOVE the compose box** (status glyph + label + spawn
**prompt** snippet on each line; not cards). Clicking a line focuses the subagent
(the transcript shows its output).

**Applies to.** The agent tile (`classify_subagent` / `AgentState::subagents`;
the subagent-panes strip in `render_agent`).

**Why.** The user must see what each subagent was asked (prompt) and how it's
doing (status) + its output. The previous detector keyed on `kind == ToolKind::Other`,
which a real `Task` never has (claude-code-acp maps `Task` → `ToolKind::Think`),
so subagents were invisible; and they were tucked in a right sidebar.

**Status:** `honored` (detection AND pane layout headless-tested). `classify_subagent`
keys on `Think` + a `prompt`/`subagent_type` raw-input (excluding `TodoWrite`'s
`todos`), with a name fallback, and captures the prompt. `render_agent` renders the
list one-per-line above the compose (auto when subagents exist; `Cmd-2`/`ToggleSubagents`
collapses).

**Enforcement.** Headless: `classify_subagent_detects_the_harness_task_shape`
(Think+prompt detected, prompt captured; TodoWrite/Read excluded; name fallback) +
`subagents_surfaces_registered_task_with_prompt` (end-to-end through the real
`ToolCall`) + **PAINT proof** `subagent_panes_paint_right_of_compose` (the layout
probe asserts the list strip is painted in the right sidepanel, with its left edge
at/right-of the compose box's right edge). Runtime: only click-to-show-output is
left for a human eyeball.

### INV-UX-7 — A mid-turn submit steers Claude and interrupts Codex; failed sends stay editable (stop is ⌘., not Esc)

> **Numbering note:** `INV-UX-6` is reserved for the parallel `toolgroup-expand-key`
> branch (tool-group collapse). This invariant is `INV-UX-7` to avoid a collision
> at integrate time.

**Statement.** Submitting a message sends it to the agent and commits it as a
user turn even while work is in flight. Claude keeps immediate `promptQueueing`
steering: the steer rides the in-flight turn without resetting its clocks. Codex
instead sends one graceful ACP `session/cancel`, then sends the typed message as
the replacement prompt; normal-message interruption does not enter the Stop
button's `StopRequested` / second-press force-restart state. If the prompt send
**fails** (offline / reconnecting) the draft is **left in the compose** with a
status so the user can retry — never silently moved or dropped. The stop gesture is
**`⌘.`** (`stop_agent` → `session/cancel`; a second press force-restarts). **`Esc`
is NOT a stop** — it is the worksheet mode key (Insert→Normal, leave-block), and
binding it to stop conflicted with mode switching, so it was unbound (2026-06-29).

**Applies to.** The agent tile: `submit_compose` / `send_prompt_to_session`
(`agent_ui.rs`), the shared graceful-cancel transport used by Stop, and the
worker's concurrent driver (`acp_channel.rs`, gated on `promptQueueing`). There
is no client-side steering queue.

**Why.** Over ACP **v1 a prompt is a turn** and there is no mid-turn input
message — but the live agent (`claude-agent-acp`) advertises a vendor capability
`_meta.claudeCode.promptQueueing` and (verified by live probe) **accepts a
`session/prompt` while a turn is in flight**, queueing it without interrupting.
yalda's worker previously *serialized* (awaited each turn before sending the next),
so a steer couldn't reach the agent until the boundary; the concurrent driver
fixes that. ACP v2 (the `unstable_protocol_v2` draft) is NOT yet honored by the
agent — it negotiates down to v1 — so promptQueueing is the real mechanism, and
this design is the v2-ready shape (`spec-turn-steering.md`).

**Status:** `honored` — provider-specific state, ordering, and transport are
guarded headlessly; the live Codex subprocess remains the documented runtime gap.

**Enforcement.** Headless in `verify_harness.rs`:
`steering_submit_while_awaiting_sends_immediately` (mid-turn submit sends + commits
+ doesn't reset the turn), `steering_midturn_ordering_and_dedup` (steer lands after
prior agent content, committed once, agent echo deduped — via the real reducer),
`codex_normal_message_interrupts_in_flight_turn` (real submit and in-process
transport: idle Codex no cancel; mid-turn Codex cancel + prompt; mid-turn Claude
prompt without cancel), and `esc_does_not_stop_in_flight_turn` /
`stop_interrupts_only_when_in_flight`.
Transport (live, not headless): `tests/steering_midturn_live.rs` drives the REAL
worker + REAL `claude-agent-acp` and proves a mid-turn steer is delivered and
processed; the v2-refused / promptQueueing facts were confirmed by a live probe.
The only thing left for a human is the subjective visual feel.

### INV-UX-8 — Worksheet renders inline-flush; chatbox renders as a pinned box

**Statement.** The two compose **placements** are visually distinct, not
cosmetically identical. In **Worksheet** placement the compose renders **inline
flush in the transcript column** — no box chrome (no panel background, border, or
horizontal margins), with an accent **left bar** as the `›` draft gutter and the
**You** label — so the draft reads as a continuation of the conversation. In
**Chatbox** placement the compose renders as a **pinned, bordered box** inset from
the column edge by a horizontal margin. Toggling placement (`Ctrl-Alt-Enter` /
space → w) therefore produces a **visible** change, not a near-no-op.

**Applies to.** The agent tile compose panel (`compose_panel` in `render_agent`,
`screens.rs`), both the small-draft and virtualized render paths.

**Why.** Model C (ADR-0024) made worksheet and chatbox **one model at two
placements**, but v1 shipped the placement axis with near-identical rendering
(both a boxed panel; worksheet differed only by an accent border + label), so
switching "felt like nothing happened" and worksheet read as broken. The flush
inline rendering is the deferred §7/v1 styling from `design-c.md` that makes the
worksheet placement actually distinct and usable. The inner compose body (wrap,
caret-containment window, virtualization) is **unchanged** — only the outer chrome
differs — so INV-UX-1/INV-UX-2 are untouched.

**Status:** `honored` — placement chrome implemented; the visible distinction is
headless-tested via the captured compose bounds. Exact glyphs/colors are the one
human-eye item (harness gap #1).

**Enforcement.** Headless in `verify_harness.rs`:
`worksheet_renders_flush_chatbox_renders_boxed` — boots in chatbox, captures the
compose box's painted bounds via the `compose-box` layout probe, toggles to
worksheet, re-captures, and asserts the worksheet box paints **~8px further left**
(the chatbox `mx_2` margin is gone) — i.e. flush in the column. The border/bg/
accent-bar differences are color-level (harness gap #1, a human eye). INV-UX-1's
`compose_caret_row_painted_inside_box_when_wrapped` still passes in both
placements (caret math unchanged).

> **SUPERSEDED by INV-UX-9 (2026-06-28).** INV-UX-8 described the two compose
> *placements* of the Model C UX (always-present worksheet compose vs pinned
> chatbox box). INV-UX-9 replaces that UX: the worksheet has **no always-present
> compose** — you edit **inline in the transcript buffer** (a You-block), and the
> chatbox is **mid-turn-only**. The flush-vs-boxed distinction is retained only for
> the mid-turn chatbox. The data substrate (Model C, ADR-0024) is unchanged.

### INV-UX-9 — The worksheet is an inline-editable conversation buffer (chatbox is mid-turn only)

**Statement.** New agent sessions **default to Worksheet** (the canonical mode);
the chatbox is the mid-turn surface plus an optional persistent placement
(`Ctrl-Alt-Enter`). Worksheet mode behaves as an **editable conversation buffer**,
per `spec-worksheet.md` (AUTHORITATIVE). Concretely:

1. **Free navigation.** In Normal mode the caret moves freely over the whole
   transcript (read navigation; nothing editable until Insert). While navigating,
   the element under the cursor is focus-highlighted — including **You-blocks** (a
   block highlights when the cursor is on its anchor line), so your insertion points
   read like every other transcript element. (The tint color is a paint/human-eye
   check — harness gap #1.)
2. **Insert opens a You-block at the caret** — a `You` delimiter + editable region
   where typed text lives.
3. **Empty insert is a no-op** — leaving Insert with only whitespace removes the
   You-block; the transcript is byte-identical to before (no phantom turn, cf.
   INV-UX-4).
4. **Non-empty You-block persists and is sent** — real text stays in place as the
   pending reply; the next Submit sends it and freezes it as a committed user turn.
5. **Insert is bounded** — a You-block can be opened **only within the most-recent
   agent turn, only after an agent newline**; frozen content is not editable.
6. **Multiple insertion points** — Insert at a new legal point opens another block
   (the previous parks in place, text kept); one is active, the rest render inline
   read-only; Submit sends them all combined + freezes each in place. A fresh
   (empty) session opens one tail block so there's always a visible input.
   - **The You-block is a DOCUMENT, not a text box (intent, 2026-07-01).** It renders
     EVERY line and GROWS with its content — it must never become a fixed-height
     region that scrolls its own text out of view (you are co-authoring a doc, not
     typing in a little box). Keeping the caret visible is the TRANSCRIPT scroll's
     job: the reveal scrolls to the caret's visual ROW *within* the block (parked ~2
     rows above the viewport bottom, so earlier lines flow up the page), never by
     truncating the block. This upholds INV-UX-1 for a block of any height. (Was
     windowed to 10 logical lines — the "You div scrolls after a while" bug.) Pinned
     by the PAINTED guard `worksheet_tall_you_block_grows_caret_painted_in_viewport`
     (a block taller than the viewport, caret proven inside it) in
     `transcript_view.rs` (render: all lines) + the reveal in `TranscriptView`.
7. **Mid-turn → chatbox.** While the agent is mid-turn the transcript is read-only
   and a chatbox appears **pinned at the bottom**; input goes there (steers/queues,
   INV-UX-7). The chatbox is **not visible when the agent is idle**.
   - **The leaders stay universal mid-turn (revised 2026-07-01).** Suppressing all
     mid-turn keystrokes into the chatbox also killed the `<space>`/`.`/`?` leader
     menus — the reported "leaders don't work mid-turn" bug. The rule now keys off
     the **steering draft**: with an EMPTY draft the worksheet is resting in nav, so
     the leaders FIRE (open the tile/workspace/global menu); once the draft is
     non-empty the keystrokes belong to the chatbox (so spaces in a steer stay
     spaces) and the leaders are suppressed. The bare `m`/`'` **mark chord** never
     fires mid-turn — `m` always types (it routes to the chatbox), independent of
     the draft. Governed by `focused_in_insert_mode` (the `midturn_steer` term) and
     the mark-chord `transcript_nav` guard (`agent_ui.rs`). Pinned real-state (via
     the in-process channel seam, `AcpChannelClient::test_connected`) by
     `real_midturn_worksheet_empty_draft_space_opens_menu`,
     `real_midturn_worksheet_typed_draft_space_is_suppressed`, and
     `real_midturn_worksheet_m_types_not_marks` (run with `--features test-support`).

**Substrate.** This is layered on Model C (ADR-0024), which stays the durable
implementation: transcript is the single ordered source of truth, agent content
appends/streams at a clean EOF, only user *text* enters the buffer. Rules 5–7 make
that safe — editing happens only while idle, so streaming never lands in a buffer
being edited (the ordering-corruption class Model C killed stays unrepresentable).

**Applies to.** The agent tile: the worksheet key dispatch (`handle_claude_key`,
`agent_ui.rs`), the You-block lifecycle (insert-enter opens / empty-exit discards /
submit freezes), the insertable-point guard (`agent.rs`), and the mid-turn chatbox
visibility (`screens.rs` `render_agent`).

**Why.** The Model C *UX* (read-only transcript + always-present separate compose)
made the worksheet "functionally useless — can't place the cursor anywhere to
respond." The inline-edit behavior is what the user actually wants and what
`spec-agent-window.md` §9–§15 originally specified; INV-UX-9 restores it on the
durable Model C substrate.

**Status:** `partially implemented` — **stages 1 + 2** (tickets 001/002) landed:
rules 1–7. The You-block opens at the caret (Insert from worksheet navigation),
renders INLINE in the transcript at its anchor (`FlatItem::YouBlock`), is discarded
on empty Esc (transcript byte-identical), persists on non-empty Esc (one block),
freezes IN PLACE at the anchor on Submit (`freeze_as_user_turn_at`), and is gated to
the latest agent turn / tail (the legal-point guard). The bottom chatbox shows only
mid-turn. **Deferred (ticket 003):** retiring the user-selected Worksheet⇄Chatbox
toggle + defaulting new sessions to Worksheet. Do NOT mark `honored` until 003 +
a human runtime check (the inline caret/colours are harness gap #1). See
`docs/projects/worksheet-inline-edit/`.

**Enforcement.** Headless in `verify_harness.rs`:
`worksheet_insert_opens_and_empty_esc_discards_you_block` (open + byte-identical
discard, INV-1), `worksheet_nonempty_you_block_persists_after_esc` (rules 4/6),
`worksheet_compose_visibility_tracks_block_and_turn` + `worksheet_renders_flush_chatbox_renders_boxed`
(painted: inline block vs bottom chatbox vs hidden — rules 2/6/7). In `tests.rs`:
`you_block_anchor_guard_restricts_to_latest_turn` (rule 5). In `editor.rs`:
`freeze_as_user_turn_at_inserts_between_agent_lines` +
`freeze_as_user_turn_at_tail_degrades_to_eof_append` (the in-place freeze + its
metadata auto-shift). The render-count proxy `transcript_021_*` still passes —
chatbox typing leaves the transcript flat (the compose fingerprint is live only for
the inline block).

### INV-UX-10 — The jump-panel agent dot is a per-session status light

**Statement.** Each agent-session row in the jump panel carries a leading dot
whose **shape** encodes binding (`●` in-use / `○` free) and whose **color** is a
status light for the session's turn phase:

- **working** (a reply is in flight) → warm accent (`theme.agent.warm_accent`),
- **waiting for you** (the turn finished; it's the user's move) → green
  (`theme.agent.tool_completed`),
- **neutral** (dim) when the phase is unknown (a roster-only session running on
  the server but never opened in this GUI, so no local `TurnPhase`) **or** the
  agent is disconnected (which also dims the whole row).

The mapping is a pure function of `(connected, awaiting)` — `AgentRow::dot_status`
→ `AgentDotStatus::{Working, WaitingForYou, Neutral}` — so the render just picks
the hue. Disconnected wins over any prior phase.

**Applies to.** `jump_panel_view.rs`: `jump_panel_agent_rows` (reads each opened
session's `state.turn_phase.is_awaiting()` into `AgentRow::awaiting`; roster-only
rows stay `None`) and `render_jump_panel` (shape from `bound`, color from
`dot_status`).

**Why.** The user wants to glance at the panel and see which agents need them
versus which are still working — without opening each tile.

**Status:** `partially honored` — the **mapping** is headless-guarded; the actual
**hue** is a paint/human-eye detail (harness gap #1). Roster-only sessions can't
show working/waiting until the server reports turn state in `SessionInfo` (today
`Neutral`).

**Enforcement.** Headless in `verify_harness.rs`:
`agent_status_dot_reflects_turn_phase` (idle→WaitingForYou, mid-turn→Working
through the real `jump_panel_agent_rows`) and the pure `agent_dot_status_mapping`
unit test (totality + disconnected-wins). The hue itself is a runtime check.

### INV-UX-11 — `ctrl-<n>` jumps to the n-th workspace (the number the panel shows)

**Statement.** The jump panel numbers **non-ephemeral** workspaces `1..N` (the
`idx + 1` badge), and `ctrl-1`…`ctrl-9` / `ctrl-0` (the 10th) jump straight to
that workspace. The displayed digit and the keystroke target always agree because
both skip ephemeral virtual workspaces (ADR-0021) — `goto_workspace_number(n)`
selects the n-th non-ephemeral tab. A digit past the last workspace is a no-op.

**Applies to.** `main.rs`: the `GotoWorkspace1..10` actions + `ctrl-<n>`
bindings (app-global, `None` context), `goto_workspace_number`, and the
`WorkspaceNavExt::workspace_nav` helper wired onto every screen root (the action
needs a handler in the focused element's ancestry — same discipline as
`toggle_jump_panel`). `jump_panel_view.rs`: the workspace-row number badge.

**Edge.** An **empty-layout** workspace renders a bare div with no action
handlers (chrome.rs), so global keys (incl. `ctrl-<n>`, `ctrl-tab`, `cmd-t`)
don't dispatch while sitting on one — a pre-existing, transient edge state, not
specific to this binding.

**Why.** Direct numeric workspace switching, matching the visible numbering.

**Status:** `honored` (headless).

**Enforcement.** `verify_harness.rs`: `ctrl_digit_switches_workspace` (full
keymap→action→handler dispatch: `ctrl-3` then `ctrl-1`, plus past-the-end no-op)
and `workspace_number_skips_ephemeral` (numbering skips the ephemeral tab).

### INV-UX-12 — `Cmd-0` focuses the agent sidepanel; vim selects (2-D), Esc restores

> **→ migrated to `UXI-AgentTile-1..3`** (`docs/components/agent-tile/sidepanel.md`).
> That component spec is now authoritative for the sidepanel; this entry is kept for
> the crosswalk + existing `INV-UX-12` code/test references.

**Statement.** The agent Plan + Subagents panels render as a **segmented,
fixed-width sidepanel on the RIGHT** of the agent tile — **Plan / Tasklist on TOP,
Subagents BELOW** (each segment shown only when open; Subagents only when non-empty;
one open fills the sidepanel height; both visible at once when both open). The
sidepanel sits beside the main column (transcript + recap + compose), which takes
the remaining width. In an agent tile `Cmd-0` enters **panel focus**: the sidepanel
widens and one row in one segment is selected. Selection is **2-D** — `panel_col`
(which segment) + `panel_sel` (the row WITHIN that segment). `h`/`←` and `l`/`→`
switch the active segment to the adjacent **open** segment (clamping the row into
it); `j`/`k`/`↑`/`↓` move the row within the active segment (clamped); `g`/`G` jump
to its ends. **Highlighting a
Subagent SWAPS the main view to its context** (see INV-UX-15) — entering the panel
previews the first row too; highlighting a Plan row clears the swap so the main
transcript returns (the plan is read in the panel itself). `Enter` **activates** the
selected row: it commits the preview (a Subagent stays swapped in) and leaves panel
focus so it's readable. `Esc` leaves panel focus, **restoring the focus captured on
entry** (`panel_return_focus`). The mode is **modal** — while focused, other keys
are inert (no leaders, no compose typing). You can never be panel-focused with no
focusable column: entering requires a column with ≥1 row (lands on the leftmost
such), and closing the active column **re-seats** to another open column or exits if
none remain. In an agent tile `Cmd-0` is panel-focus, **not** zoom-reset (the
`AgentView`-scoped binding is registered after the global `cmd-0 ZoomReset` so
GPUI's most-recent-first match prefers it); elsewhere `Cmd-0` still resets zoom.

**Applies to.** `agent.rs`: `AgentFocus::Panel`, `PanelColumn`, `PanelItem`,
`panel_column_rows` / `panel_open_columns` / `reseat_panel_focus`, the `panel_col` /
`panel_sel` / `panel_return_focus` state. `agent_ui.rs`: `focus_agent_panel` /
`exit_agent_panel` / `panel_move_selection` / `panel_switch_column` /
`panel_select_end` / `panel_activate_selection` / `reveal_panel_selection` (sets/
clears `focused_subagent`), the modal interception at the top of `handle_claude_key`,
and the re-seat in `toggle_tasklist` / `toggle_subagents`.
`main.rs`: the `FocusAgentPanel` action + `cmd-0` `AgentView` binding.
`screens.rs::render_agent`: the segmented right-sidepanel layout (`agent-sidepanel`
container, `tasklist-panel` + `subagent-panes` segments stacked) + widen-on-focus +
per-segment selection highlight (Plan entries theme-colored + wrapped for full text)
+ `FocusAgentPanel` `on_action`.

**Why.** Keyboard-drive the Plan/Subagents panels (view a subagent's context)
without a mouse, with a clear enter/navigate/exit gesture that always returns you
where you were; the right sidepanel keeps both lists visible at once beside the
conversation instead of stealing height above the compose. Highlight-to-swap makes
selection *do* something visible — you scan the panel and the main view follows —
rather than a bare selection with no feedback.

**Status:** `honored` (headless; exact widen px / highlight color are gap-1).

**Enforcement.** `verify_harness.rs`: `agent_panel_cmd0_enters_and_esc_restores`,
`agent_panel_vim_moves_selection`, `agent_panel_hl_switches_columns`,
`agent_panel_enter_focuses_subagent`, `agent_panel_cmd0_binding_enters_panel` (real
keymap dispatch proves the AgentView binding shadows zoom-reset),
`agent_panel_closing_last_panel_exits_focus`,
`subagent_panes_paint_right_of_compose` (PAINT proof the panels sit in the right
sidepanel, beside the compose), `plan_and_subagents_share_the_sidepanel` (both
segments painted stacked inside one `agent-sidepanel`),
`panel_highlight_swaps_to_subagent` (highlight sets `focused_subagent`; unfocus
clears it), `panel_enter_reveals_and_exits` (Enter leaves panel focus), plus the
state-machine fuzzer ops + oracle (`focus ∈ {Compose,Transcript,Panel}`,
panel-focused ⇒ a panel is open). The swap render itself: see INV-UX-15.

### INV-UX-13 — Document text zoom scales the agent transcript, like a buffer

**Statement.** `Cmd-=` / `Cmd-+` (in) and `Cmd--` (out) — the document text-zoom
`text_scale` — scale the **agent transcript** the same way they scale the buffer
doc view: the conversation **prose** and the transcript's **markdown blocks**
(headings / code / tables) multiply by `text_scale`. Zoom is GLOBAL (not session
state): its action handler pushes `notify_transcript_views(TextStyle)` so every live
`TranscriptView` re-renders and re-reads `text_scale` off the root (via
`RootSnapshot`) — it is NOT a per-session `TranscriptSeqs` seq. As with buffers,
**chrome stays at native size**: the turn/tool gutter labels, tool-card status
glyphs, the right sidepanel (Plan/Subagents), the status footer, and the **compose input** (its caret
and line-box are pixel-pinned for caret-containment — INV-UX-1 — so its font is held
fixed; scaling it would require scaling the caret + `CHATBOX_CHAR_W` in lockstep, a
separate change). `Cmd-0` resets zoom everywhere EXCEPT agent tiles, where it is
panel-focus (INV-UX-12) — zoom-out then back is the reset there.

**Applies to.** `transcript_view.rs`: `RootSnapshot.text_scale` (read from
`root.text_scale`), the per-line `text_size(px(13.0 * text_scale))` on the
`FlatItem::Line` **row wrapper** (NOT just `claude-body` — `gpui::list` items do
not inherit the list's ambient text size, so the size must live on the item, the
same way the doc/WP views set it on each line wrapper), and the `FlatItem::Block`
`RenderCtx { text_scale }`. `main.rs`: `set_text_scale` → `notify_transcript_views`.

**Why.** Reading the agent conversation at a comfortable size should work exactly
like reading a document — the transcript is the agent tile's primary reading
surface.

**Status:** `honored` (headless — the painted line height is probed).

**Enforcement.** `verify_harness.rs`: `transcript_prose_scales_with_zoom` (probes
a prose line's PAINTED height at 1× vs 2× — it must grow, so the font actually
scales, not just the cache busting) + `transcript_021_theme_and_zoom_bust_cache`
(zoom re-renders the transcript once — the invalidation path). Exact glyph shape
remains the harness's pixel gap (#1), but the size change is now guarded.

### INV-UX-14 — Selecting text auto-copies it to the clipboard (X11-style)

**Statement.** Finishing a **mouse drag-selection** over a read-only reading
surface writes the selected text to the **system clipboard automatically** — no
`Cmd-C` — so the very next `Cmd-V` pastes it. This is the X11 "select = copy"
convention (macOS has no separate PRIMARY buffer, so the ordinary clipboard is
used). Applies to both selectable reading surfaces:

- **Buffer doc (Viewing / `YaldaView`)** — the existing click-drag selection
  (`doc_selection`, `doc_mouse_*`, hit-tested via per-line `TextLayout`s in
  `line_layouts`) copies on `doc_mouse_up` when the finalized selection is
  non-empty. `Cmd-C` still works; auto-copy is additive.
- **Agent transcript (`TranscriptView`)** — a drag selects transcript text and
  copies on release. Because each transcript line is rendered as a `flex_wrap`
  row of many **monospace** tokenized `styled_line_element`s (not one hittable
  `StyledText`), hit-testing uses a **paint-time token sink**: each painted token
  registers its window-space bounds + covered `(line, start_char, count)` via
  `register_token_on_paint`, and `hit_test_tokens` maps a point → `(line, char)`
  by the token's own width (`width / char_count`, exact for monospace). The drag
  drives the transcript editor's anchor/head selection (the SAME model the
  keyboard selection band renders from, gated on `AgentFocus::Transcript`), so a
  mouse-down also focuses the transcript. The caret is **suppressed while
  dragging** (`TranscriptView.dragging`) so every visible line takes the uniform,
  registerable non-cursor render path.

**Applies to.** `main.rs`: `doc_mouse_up` (buffer). `transcript_view.rs`:
`transcript_mouse_down` / `_move` / `_up` + the `token_hits` sink cleared in
`build_body` and refilled by `RegisterTokenOnPaint` (`render_blocks.rs`).
`agent.rs`: `build_wrapped_line` (`token_sink` / `line_idx` params).

**Why.** The transcript and doc view are reading surfaces; the muscle-memory of
"drag to grab a line of agent output, paste it elsewhere" should not require a
second chord. Copy-on-select is the lowest-friction path.

**Non-goals / bounds.** The buffer raw **Edit** view (Code/WP) has keyboard
selection only — no mouse drag, so nothing to auto-copy there yet. Character
precision on the transcript relies on the surface being monospace; a
proportional transcript font would need per-token `index_for_position` instead.

**Status:** `honored` (headless — the drag is driven through the real
`simulate_mouse_*` path and the clipboard is read back).

**Enforcement.** `verify_harness.rs`: `doc_drag_autocopies_selection_to_clipboard`
(buffer) and `transcript_drag_autocopies_selection_to_clipboard` (agent) — each
seeds a sentinel clipboard value, drags across a known line via real mouse
events, and asserts the clipboard now holds the dragged text (negative control:
disabling the `write_to_clipboard` leaves the sentinel ⇒ RED).

### INV-UX-15 — Focusing a subagent swaps the main agent view to its context

**Statement.** When a subagent is **focused** (`focused_subagent = Some(key)` — set
by clicking its row, or highlighting it in the Subagents panel per INV-UX-12), the
agent tile's **main area is replaced** by that subagent's **context**: a `← Back`
header (label of the subagent) over a scrollable view of its prompt + content +
output (`append_tool_body`, the same body the expanded tool card shows). The cached
main `TranscriptView` is **not rendered** while swapped. Returning to the main agent
is easy and always available: click **`← Back`**, or press **`Esc`** (`Esc` with a
focused subagent calls `unfocus_subagent`, ahead of its per-mode meaning). Switching
the panel highlight to a Plan row, or any `focused_subagent = None`, restores the
main transcript. The swap is a pure render-time branch on `focused_subagent`; no
transcript state is touched, so Back is lossless.

**Applies to.** `screens.rs::render_agent`: the `focused_subagent` match that builds
the `subagent-view` (Back header + `append_tool_body`) OR the transcript body.
`agent_ui.rs`: `focus_subagent` / `unfocus_subagent` (set/clear), the `Esc`-returns
branch in `handle_claude_key`, and `reveal_panel_selection` (panel highlight → swap).
`agent.rs`: `focused_subagent: Option<ToolCallKey>`, `classify_subagent` (label).

**Why.** A subagent's work is a self-contained sub-conversation; reading it should
feel like *entering* it — a full view you can scroll — not squinting at an inline
expanded card. A single obvious Back (button + `Esc`) keeps it non-trapping.

**Bounds.** A subagent's "context" is whatever the Task tool call carries (prompt +
accumulated content/output blocks), not a separate live nested transcript with its
own tool cards — that's all the agent surfaces over ACP today.

**Status:** `honored` (headless — the swap is proven by the layout probe; exact
pixels/colors are gap-1).

**Enforcement.** `verify_harness.rs`: `subagent_focus_swaps_the_painted_view` — with
a subagent focused, the `subagent-view` PAINTS and `transcript-viewport` does NOT;
after Back the `subagent-view` is gone (negative control: render the transcript
unconditionally ⇒ `subagent-view` never paints ⇒ RED). Plus
`panel_highlight_swaps_to_subagent` for the panel-driven entry.

### INV-UX-16 — Keystrokes that route to the compose are ALWAYS painted (routing ⇒ painting)

**Statement.** In an agent worksheet, a keystroke ROUTES to the compose buffer
whenever `focus == AgentFocus::Compose` (agent_ui.rs:4231; the transcript-nav
branch is taken only when focus is on the transcript). Therefore, whenever focus
is on the compose in an idle worksheet, the compose **must be painted** and its
edits must **bust the cached transcript** — otherwise the user types into a
surface that renders nowhere (the "invisible text" class). This is guaranteed
structurally by deriving the render/notify gate from the SAME fact the router
uses:

```
inline_you_block_active() =
    (you_block_open || focus == AgentFocus::Compose) && !awaiting && !chatbox
```

The `|| focus == Compose` clause is load-bearing: it closes the recurring
"`/clear` worksheet-invisible" bug ("the hole" =
`focus==Compose ∧ you_block_open==false ∧ idle ∧ worksheet`, where routing sent
keys to a compose that painted nowhere — the bottom box only shows chatbox /
mid-turn, screens.rs:1188). Mid-turn (`awaiting`) and chatbox stay excluded —
their draft is the bottom box, which IS painted there.

**Applies to.** `agent.rs`: `inline_you_block_active()` (:3977, the ONE shared
gate), the flat-list injection (`rebuild_agent_view_model` :2892, on the derived
gate), and the view-model memo key (`view_model_fingerprint` :3350, hashes the
DERIVED gate so a focus-only flip busts the flat-list memo — else a stale list
without the YouBlock row is reused). Everything else
(`TranscriptSeqs::of`/`YouBlockSnap`, the keystroke session-notify
agent_ui.rs:4260, reveal, submit anchor) already routes through the predicate and
self-aligns.

**Second mechanism — the You-block LIST ITEM must be invalidated on every compose
edit (added 2026-07-06).** The gate being correct is necessary but NOT sufficient.
The active You-block is ONE `FlatItem::YouBlock` list item, and GPUI's `ListState`
**caches rendered items** — an item is only re-measured/repainted when it is
`splice`d. `reconcile_list` (agent.rs:1638) splices the tail on a *transcript*
`edit_seq` move and diffs `FlatKey::YouBlock` on `parked` only. But the You-block's
content is driven by the **compose** buffer, whose `compose_edit_seq` bumps
neither the transcript's `edit_seq` NOR the key — so a keystroke left the item
un-spliced and GPUI repainted its STALE cached element. The observe fired,
`build_body` ran, the caret/gate were all correct, and the char was STILL invisible
until an unrelated event (jump bar, chatbox toggle, a transcript change) forced a
splice. This was the true, seven-times-recurring root cause the six gate/fingerprint
fixes never touched (found by autonomous runtime repro, `YALDA_CLEAR_DEBUG`: the
observe logged `compose_edit_seq` advancing while `build_body` reported
`memo_hit=true` with no repaint). Fix: `build_body` (transcript_view.rs) hashes the
active block's render inputs into `you_block_seq` and splices exactly that item when
it moves (vs `TranscriptScroll::last_you_block_seq`). Invariant: **any change to the
active You-block's compose text / caret / mode / selection MUST splice its list item
the same frame.**

**Why.** Six prior "fixes" added another `settle_input_focus()` at another guessed
producer of the hole; the hole has many producers (e.g. `force_restart_agent`
Idles without settle, agent_ui.rs:3602) and was never made unrepresentable.
Deriving painting from routing makes the disagreement set empty for EVERY
producer — no writer can strand focus on an unpainted compose without visibly
breaking routing. See `docs/projects/clear-worksheet-invisible/` (spec + critique
+ failure-log).

**Status:** `honored` (headless — the real key handler + real render, with the
paint probe; the live `/clear` producer is confirmed via `YALDA_CLEAR_DEBUG`).

**Enforcement.** `verify_harness.rs::clear_worksheet_hole_types_and_paints` —
enter the hole (pre-asserted 4-part state), type via the REAL `handle_claude_key`,
assert the cached transcript re-renders (render count advances) AND an inline
You-block PAINTS inside the transcript viewport. **Negative control: each of the
three edits (predicate :3977, injection :2892, memo :3350) reverted independently
produces RED for its OWN reason** (flat count / no paint / stale-list no paint) —
verified. Plus `tests.rs::inline_you_block_active_truth_table` (the
`focus==Compose` rows). The buffer-only assertion of the six prior fixes
(`compose().text()=="hello"`) is explicitly NOT the guard — it is green while the
screen is blank.

**Enforcement (second mechanism).**
`verify_harness.rs::clear_worksheet_you_block_keystroke_splices_item` — rest in the
post-`/clear` typeable worksheet (inline block active, pre-asserted), settle so
`last_you_block_seq` catches up, then type via the REAL `handle_claude_key` and
assert the `YOU_BLOCK_SPLICE_LABEL` perf counter advances (the You-block item was
invalidated). **Negative control (observed): delete the `you_block_seq != …` splice
block in `build_body` → RED, splice count `0 -> 0`** — the item is never
invalidated, the exact cached-item staleness the user reports. Paint is NOT the
guard here: the headless harness re-renders every list item each frame, masking the
`ListState` item cache — which is precisely why the paint-based repros were falsely
green through six rounds.

**Third mechanism — a session-SWAP must re-seat the cached transcript view in the
committed frame (added 2026-07-08).** Even with the gate and the item-splice both
correct, the FULL real `/clear` path stayed broken: `clear_agent_session` drops the
old session's `TranscriptView` (`transcript_views.remove`) and the async rebind
(`apply_open_agent_resolution`) creates a NEW one — which GPUI routinely hands the
**same entity slot** the dropped view just freed (observed: boot `EntityId(3v1)` →
post-clear `EntityId(3v3)`). Embedded via `cached_child` at the SAME tree position,
the fresh view (a) inherits the dropped view's stale cached prepaint (gpui keys
`AnyViewState` by `GlobalElementId` = tree position, `view.rs:208-214`), and worse
(b) having never been painted into the COMMITTED `rendered_frame.dispatch_tree`, its
self-notifies are silently dropped by `mark_view_dirty` (`window.rs` — empty
`view_path` ⇒ nothing enters `dirty_views`). The transcript FREEZES: the observe
fires and even a DIRECT `cx.notify()` on the view does nothing; typed text never
repaints until an unrelated event (a mouse click) forces a full refresh. This was
the residual "invisible until I click" the gate + splice fixes never reached — it is
NOT a routing/gate fault at all but a cached-view lifecycle fault. Fix:
`transcript_view_for` (main.rs), on CREATING a new view, `cx.defer`s a full
`app.refresh_windows()` — one forced (`refreshing`-bypassing) paint that seats the
new view in the dispatch tree, after which its observe-notifies land normally.
Invariant: **creating a `TranscriptView` (any session open / `/clear` / rebind /
restore) MUST force one full window refresh so the new cached view is re-seated in
the committed frame — a bare notify cannot dirty a view GPUI has never painted.**

**Enforcement (third mechanism).**
`verify_harness.rs::real_clear_server_branch_then_type_paints` — drives the ENTIRE
real path no prior test composed: REAL `clear_agent_session` down the client/server
branch (via the `FORCE_SERVER_CLEAR_BRANCH` seam) → REAL async
`apply_open_agent_resolution(Created)` bind → REAL `handle_claude_key` → assert the
transcript re-renders AND the You-block PAINTS inside the viewport. A pre-clear
CONTROL (boot session types + paints) makes it non-vacuous. **Negative control
(observed): comment out the `cx.defer(|app| app.refresh_windows())` in
`transcript_view_for` → RED, `after_r/after_s == 0` and `you-block` never paints —
the exact user symptom.**

### INV-UX-17 — The keybindings tile shows the LIVE keymap and rebinds it in place

**Statement.** The `App::Keymap` reference tile is not a static cheat-sheet: it
renders the SAME `KeymapRegistry` that `register_keymap` applies to the app
(`keymap_registry.rs`, `DEFAULT_BINDINGS` → `KeymapRegistry::apply`). Two
consequences that must hold:

1. **Truthfulness (dynamic).** Every keystroke the tile displays is a live
   binding, grouped by context (the GPUI `key_context`) then theme (category).
   There is no second copy of the keymap to drift from — `register_keymap` is
   `KeymapRegistry::load().apply(app)`, and the tile reads the registry off the
   root. A rebind updates that one registry, so the row updates the moment it
   commits.
2. **Rebind = apply + persist, and capture grabs the keyboard.** Committing a
   rebind (`keymap_ui.rs::keymap_commit_rebind`) mutates the registry,
   re-`apply`s the whole keymap atomically (`clear_key_bindings` + `bind_keys`,
   so GPUI precedence is unchanged from the ported defaults), and persists the
   diff to `~/.yalda/keymap-overrides.json` (reloaded next launch). While
   capturing the new chord the app keymap is CLEARED, so the pressed chord is
   **recorded, not fired** (pressing `cmd-t` during capture must not open a tab);
   commit/cancel re-apply the registry, restoring bindings with the new one live.

**Applies to.** `keymap_registry.rs` (the table + `apply`/`rebind`/`reset`/
`persist`/`conflicts`), `keymap_view.rs` (`KeymapView` — the cached body; the
browse cursor is always on a marked row via the `›` gutter, INV-UX-1's spirit for
this surface), `keymap_ui.rs` (the key handler + capture grab), and `main.rs`
`register_keymap` (now data-driven from the table).

**Why.** Bindings were ~120 inline `KeyBinding::new` calls with no introspection
and no rebind path. Lifting them into one declarative table that BOTH drives the
live keymap AND backs the reference makes "what the sheet says" and "what the app
does" the same object, so they cannot disagree.

**Status:** `honored` (headless).

**Enforcement.** `verify_harness.rs`: `keymap_registry_table_is_valid` (every
action builds / context + keystrokes parse — nothing silently unbound),
`keymap_rebind_via_real_keystrokes` (drive the REAL `handle_keymap_key` capture →
commit; the live registry entry changes; negative control verified),
`keymap_rebind_persists_and_reloads` (override survives a reload; garbage keys
rejected), `keymap_conflict_detection`, and
`keymap_body_is_cached_and_self_invalidates` (the render-count perf guard for the
new cached surface).

### INV-UX-18 — Jump-panel items reorder by drag, at two levels, cwd-bounded

**Statement.** The jump panel's "Agent sessions" section is user-reorderable by
drag-and-drop at two levels, and the ordering survives restart:

1. **cwd groups reorder.** Dragging a cwd subheader onto another moves that whole
   group; the header order is the user's, persisted in
   `Preferences::jump_cwd_order` (a list of cwd display keys).
2. **sessions reorder within their group.** Dragging a session row onto another
   in the SAME group reorders it there, persisted in
   `Preferences::jump_session_order` (a list of server sids).
3. **A session never crosses cwd groups.** A session drag is accepted only by a
   drop target in its own cwd group (`can_drop` gates on the drag payload's
   `cwd_key`), and `reorder_session` defensively re-checks and refuses a
   cross-cwd move. So a session can never be dragged into a cwd it doesn't belong
   in — it is a hard gate at the gesture AND a guard in the state change.

Both orders are applied as a **stable** sort by "rank in the order list" over the
cwd-grouped rows (`order_grouped_rows`), so an empty/absent order list is a total
no-op: the panel stays alphabetical (groups) / by-label (sessions) until the user
drags. Items not yet in an order list sort after the listed ones, keeping their
default position. Only roster-backed sessions (stable sid) drag; local mid-create
placeholders don't.

**Applies to.** `jump_panel_view.rs` (`SessionDrag`/`CwdDrag` payloads,
`JumpDragPreview`, `group_agent_rows_by_cwd` + `order_grouped_rows` +
`reorder_move`, `reorder_cwd_group`/`reorder_session`, and the `on_drag` /
`drag_over` / `can_drop` / `on_drop` wiring in `render_jump_panel`), and the
`Preferences::{jump_cwd_order, jump_session_order}` persistence (`persist.rs`,
loaded/saved in `main.rs`).

**Why.** Alphabetical-by-label is not how a user thinks about their projects and
agents; manual curation (drag the active project to the top, order its agents by
task) is. Grouping stays cwd-anchored because the cwd is the project axis — so a
session moving to a foreign project's group would be a lie about where it runs,
hence the hard cwd gate.

**Status:** `honored` (headless for the ordering + reorder state change; the GPUI
mouse-drag GESTURE that dispatches the drop is `NEEDS-RUNTIME` — gap #2, there is
no headless drag-dispatch seam, so the on_drag/on_drop wiring itself is
human-verified).

**Enforcement.** `verify_harness.rs`:
`jump_reorder_ordering_applies_and_defaults_to_alpha` (the pure order +
negative-control default), `jump_reorder_move_semantics` (list surgery), and
`jump_reorder_methods_reorder_and_gate_by_cwd` (drives the REAL
`reorder_cwd_group` / `reorder_session` the drop handlers call: headers reorder,
sessions reorder within group, cross-cwd refused).

### INV-UX-19 — A tool call never splits an agent text token

**Statement.** A tool-call row interleaved into an agent turn's streamed prose
must break the prose only at a **word / sentence boundary**, never inside a word.
Concretely: when a tool call arrives while the turn's tail line is still OPEN (the
last streamed chunk did not end the run) and the interruption falls mid-word — the
open line's last content char AND the next chunk's first char are both
**alphanumeric** — the continuation **rejoins** the open run's end-of-content, and
the tool group renders AFTER the completed text. Any other boundary (whitespace,
or sentence/word-terminating punctuation like the '.' ending "here.") is a
legitimate `text → tool → text` interleave and is left in place (tool between the
two statements). This makes the reconstructed transcript read as the model wrote
it: `` `mode=max` `` is never rendered as `` `m `` | ToolSearch | `ode=max`. The
alphanumeric-only rule is conservative on purpose — it fixes the word-cut-in-half
case (what reads worst) without guessing at ambiguous punctuation splits.

**Applies to.** `editor.rs` — `Editor::append_llm_chunk_floored` +
`midtoken_rejoin_point` (the mid-token detector) vs `find_llm_insertion_point`
(the whitespace-boundary interleave, whose `ends_with('\n')` → different-turn →
EOF branch is the splitter this guards). Driven from the agent reducer
(`agent_ui.rs` `apply_reply_events`, the `Chunk` / `ToolCallStarted` arms).

**Why.** Streamed `ReplyEvent`s can deliver a tool-call notification between two
text deltas of one content block. Anchoring the tool on its own line then forcing
the continuation below it (INV-ORDER keeps the transcript append-only) bisected
whatever token straddled the delta boundary — the reported "interleaved toolcalls
with agent text" screenshot, where a code span was cut in half. The token-straddle
test distinguishes that artifact from a genuine `text → tool → text` agent-loop
interleave (which breaks at a sentence boundary and must be preserved).

**Status:** `honored` (headless — the split is a buffer-content property the
reducer produces, fully observable without paint).

**Enforcement.** `verify_harness.rs`:
`tool_call_midtoken_does_not_split_agent_text_run` (drives the REAL
`apply_server_batch` → `append_llm_chunk_floored` with a `Chunk` / `ToolCallStarted`
/ `Chunk` mid-token stream; asserts the token stays whole AND the tool group
renders after the reassembled line; negative control: the buffer becomes
`` `m\n\node=max ``). `tests.rs`:
`floored_tools_and_text_stay_in_order_above_draft` pins the complementary
whitespace-boundary case (chunks ending `". "` stay interleaved with their tools).

### INV-UX-20 — A summoned session recap is pinned and isolated

**Statement.** Invoking "recap this session" (agent menu `R` → `recap-session`)
generates an LLM prose summary of the focused session and pins it **inside that
session's agent tile, above the subagents/tasks panels** (the compose sits below
those), until the user dismisses it (`✕` / `recap-dismiss`). A recap is **specific
to its tile**: recaps are keyed by `SessionId` (`self.recaps`), so two tiles can
each hold their own and one tile's recap never appears in another. It is
**re-runnable** at any time (`⟳` / `recap-session`), which supersedes that
session's prior run. Three hard properties:

1. **Isolation.** Recap generation runs on a THROWAWAY `AcpChannelClient`
   side-channel fed the transcript text inline. Its reply stream NEVER routes
   through the visible transcript reducer (`apply_reply_events`) — summoning a
   recap adds nothing to any session's transcript and cannot reorder it.
2. **Visible progress, last-writer-wins.** While `Generating` the panel shows
   "Summarizing…" and streams chunks in as they arrive; on turn resolution it
   flips to the finished prose (`Ready`), or a reason (`Failed`) on spawn/send
   error or an empty reply. A run token guards every state transition, so a
   superseded (re-run / dismissed) run can never scribble on the current one.
3. **Tile-scoped placement.** The recap renders in its own tile, above the
   subagents/tasks panels and the compose — never in the global jump panel, and
   never in a tile bound to a different session.

**Applies to.** `agent_ui.rs` — `summon_recap` / `rerun_recap` / `start_recap_for`
/ `spawn_recap_worker` / `drain_recap` / `apply_recap_event` / `finalize_recap` /
`fail_recap` / `dismiss_recap` / `dismiss_recap_for`; the `RecapState` /
`RecapStatus` model + the `recaps: HashMap<SessionId, RecapState>` field
(`agent.rs` / `main.rs`); the inline `render_agent_recap` in the agent tile
(`screens.rs`). Chrome-class: native size, unaffected by document zoom.

**Why.** A recap is a manual, re-orienting glance — it must be summonable without
mutating the conversation it summarizes (property 1 is exactly the transcript-
ordering-corruption class this codebase has fought repeatedly), and it must show
its work and never leak a stale worker's output onto a newer request (property 2).

**Status:** `honored` (headless for the reducer + panel; the live throwaway
subprocess is the sole `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2).

**Enforcement.** `verify_harness.rs`: `recap_summon_sets_generating`,
`recap_chunks_accumulate_and_finalize_ready`, `recap_empty_reply_fails`,
`recap_dismiss_clears`, `recap_rerun_supersedes_stale_run`, and
`recap_panel_paints_in_agent_tile` (layout probe: paints above the compose). Each
drives the REAL menu-dispatched entry point / reducer methods; negative controls
documented at the tests.

### INV-UX-21 — A pasted image is staged, shown, and sent as a content block

**Statement.** Cmd+V into an agent tile whose clipboard holds an image stages
that image as a **pending attachment** on the compose rather than typing text.
Three hard properties:

1. **Stage, don't type.** The image is base64-encoded (with its mime type, taken
   from GPUI's clipboard `ImageFormat`) and pushed to `Compose::pending_images`;
   the compose editor text is untouched. A clipboard with no image falls back to
   an ordinary text paste. Image bytes never enter the compose/transcript text.
2. **Visible before send.** Each staged image renders as a `🖼 <label>` chip
   above the compose box so the user sees what will go with the next submit.
3. **Sent as a content block; cleared after.** On submit (both the chatbox/tail
   path `send_prompt_to_session` and the worksheet path `submit_worksheet_blocks`)
   the attachments become ACP `ContentBlock::Image`s appended after the text block
   in the `session/prompt` request — travelling the GUI→session-server wire as
   `Request::Prompt.images` (additive, `#[serde(default)]`). The transcript
   records a `🖼 image N (EXT)` marker line for the sent images, and
   `pending_images` clears on the post-submit compose reset. An image-only prompt
   (no typed text) is sendable. Attachments are **ephemeral** — not persisted in
   the WAL, so a resumed transcript shows the marker's text but not the image.

**Applies to.** `agent_ui.rs` — `paste_into_compose` / `pending_image_from_clipboard`
/ `image_ext` / `image_turn_marker`, `send_prompt_to_session`,
`submit_worksheet_blocks`; the `PendingImage` model + `Compose::pending_images`
(`agent.rs`); the chip row in `render_agent` (`screens.rs`); `ImageAttachment` /
`PromptPayload` + `content_blocks()` (`acp_channel.rs`); `Request::Prompt.images`
(`session_proto.rs`) threaded through `session_client::prompt_with_images` and the
session-server prompt path. Chrome-class: the chips render at native size.

**Why.** Pasting a screenshot is the natural way to show the agent something; the
old paste path was text-only (`pbpaste`), so Cmd+V of an image did nothing useful.
Staging + a visible chip + an explicit content block keeps the image out of the
conversation text (which the transcript-ordering invariants protect) while still
reaching the model.

**Status:** `honored` (headless for the paste-staging, the mixed content-block
build, the wire round-trip, and the end-to-end worksheet submit; the live
subprocess loop is the `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2 — and exact chip glyphs/colors are gap 1).

**Enforcement.** `verify_harness.rs`: `image_paste_stages_pending_attachment`
(real Cmd+V → real test-platform clipboard → staged base64 round-trips; compose
text stays empty) and `image_submit_sends_block_marks_transcript_and_clears`
(real submit → `PromptPayload.images` on the channel + transcript marker +
cleared). `acp_channel.rs`: `prompt_payload_builds_text_then_image_blocks` /
`_image_only_omits_empty_text_block` / `_empty_yields_one_text_block`.
`session_proto.rs`: `prompt_deserializes_without_images` / `prompt_round_trips_images`.
Negative controls documented at each test.
### INV-UX-22 — The agent model is switchable per session, from what the agent advertises

**Statement.** Each agent session can switch its model live (Opus / Fable /
Sonnet / …) from **the exact picklist the agent advertises** — never a hardcoded
list. The list is the model `Select` config option (id `"model"`, category
`Model`) the agent returns on `session/new` / `session/load`; yalda parses its
`current_value` + `options` into `AgentState.available_models` + `agent_model`.
Switching issues an ACP `session/set_config_option` for the `model` option (NOT a
new session — the conversation is preserved); the agent applies it and echoes the
refreshed selector back, which updates the badge. Three properties:

1. **Agent-sourced, never hardcoded.** The offered models are exactly
   `available_models`, populated from the advertised `Select.options`. An adapter
   that surfaces no model selector shows no switcher (plain label, no `▾`).
2. **Live, conversation-preserving.** A switch is `set_config_option`, applied
   mid-session (even mid-turn); it does not clear or re-create the session. The
   current model is marked (`✓`) in the picker.
3. **Two reachable gestures, one path.** Keyboard `space → M → <n>` (a dynamic
   "switch model" submenu) and clicking the status-strip model badge (`model ▾` →
   opens the local menu) both dispatch `set-model:<id>` → `set_agent_model`,
   which routes through `session_server.set_model` (server-backed) or the local
   channel's `set_model` (direct-spawn).

**Applies to.** `acp_channel.rs` — `ModelOption`, `ReplyEvent::ModelsAvailable`,
`model_state_from_config_options` / `model_reply_events`, the worker set-model
task issuing `SetSessionConfigOptionRequest`, `TransportHandle::set_model`;
`session_proto.rs` `Request::SetModel`; the session-server `do_set_model`;
`session_client.rs` `set_model`; `agent.rs` `AgentState.available_models`;
`agent_ui.rs` `set_agent_model` + the `ModelsAvailable` reducer arm; `main.rs`
`agent_local_menu_dynamic` + the `set-model:` dispatch; the clickable badge in
`screens.rs`. Chrome-class: the badge renders at native size (unaffected by
document zoom).

**Why.** The model is a first-class per-task choice (Opus for hard work, Sonnet
for routine, Fable for the longest runs), and it must reflect what the agent
actually offers rather than drifting from a hand-maintained list — the agent's
advertised picklist is the single source of truth.

**Status:** `honored` (headless for the config parse, reducer capture, dynamic
menu, and the channel-dispatch; the live ACP `session/set_config_option`
round-trip is the sole `NEEDS-RUNTIME` gap — dev-system § Verification harness
gap 2 — covered by the `#[ignore]` `tests/model_switch_live.rs`).

**Enforcement.** `acp_channel.rs`: `model_state_parses_select_current_and_options`
(config parse + `model_reply_events`). `verify_harness.rs`:
`agent_reply_models_available_captures_picklist` (reducer capture),
`agent_menu_lists_advertised_models_and_marks_current` (dynamic submenu + `✓` +
`set-model:<id>` commands), `set_agent_model_issues_set_config_on_channel` (the
real switch path reaches the channel). `tests/model_switch_live.rs`
(`set_model_round_trips_against_real_agent_live`, `#[ignore]`) closes the live
round-trip. Negative controls documented at each test.

### INV-UX-23 — A moved transcript fingerprint is ALWAYS rendered (no stale tail)

**Statement.** When any input the agent transcript reads changes — most visibly
the FINAL streamed chunk of a turn — the transcript re-renders that same frame.
It is never left showing stale content until an unrelated event heals it. The
symptom this bans: "the last agent message doesn't render in the tile" (it
appears only after a keystroke / theme toggle / scroll).

**Root cause it closes.** `TranscriptView` is a cached child that invalidates by
`cx.observe`→`cx.notify()` on itself. GPUI's `mark_view_dirty` walks the
committed frame's dispatch tree via `view_path`; if the view had no node in that
frame (a view swap/rebind at the same slot, a tab hiding the tile, a
`/clear`-then-stream race), the notify inserts nothing into `dirty_views` and is
SILENTLY DROPPED — and since `TurnEnded` is the last event of the turn, nothing
re-arms it, so the cached prepaint is reused stale. The self-notify hop is
inherently droppable.

**Mechanism (the backstop, Option A).** `render_agent` keys the cached
transcript's element id on its render fingerprint:
`div().id(("transcript-fp", TranscriptSeqs::of(state).fingerprint_hash()))`. A
moved fingerprint yields a fresh `GlobalElementId`, so gpui's
`with_element_state` misses and the transcript's `render()` is FORCED —
independent of `mark_view_dirty`/`view_path`. The self-notify path stays the
fast O(changed) invalidation; the id only closes the hole when a notify is
dropped. A stable fingerprint keeps the id stable ⇒ cache hit ⇒ render-skip is
preserved (typing in the chatbox never moves the transcript fingerprint), so the
perf guarantee (INV under `transcript_021_*`) is untouched. The root is uncached
and recomputes the id each frame, so the backstop can't itself be parked.

**Applies to.** `screens.rs` `render_agent` (the id'd transcript wrapper);
`transcript_view.rs` `TranscriptSeqs` (`Hash` derive + `fingerprint_hash`).

**Why.** Every render input must be in the fingerprint (the cached-surface rule),
but the fingerprint only busts the cache if its notify LANDS. Keying the element
id on the fingerprint makes "fingerprint moved ⇒ render ran" true by construction
of the element tree, not by a notify that a framework hole can eat.

**Status:** `honored` (headless — the reuse-decision path is deterministic in the
harness).

**Enforcement.** `verify_harness.rs`:
`transcript_dropped_notify_id_forces_render` — mutates the transcript editor
WITHOUT notifying the session (deterministically reproducing a dropped notify),
forces a root frame, and asserts the transcript render count still advances +1.
Negative control (observed RED): revert the embed to the fingerprint-independent
`cached_child(transcript_view)` and the count stays flat — the stale-tail bug
reproduced.

## Cross-references

- `spec-turn-steering.md` — the full steering design (queue, delivery modes, the
  ACP v1/v2 constraint) this invariant summarizes.
- `spec-chatbox-caret-containment.md` — the compose caret window. Its VERTICAL
  axis still governs the compose; its HORIZONTAL axis is RETIRED for the compose
  (superseded by INV-UX-2 word-wrap).
- `spec-agent-presentation.md` / `spec-agent-render-pipeline.md` — the agent
  render path + `TranscriptSeqs` fingerprint discipline (every render input
  covered, never notify in render) that keeps caret state from going stale.
- `spec-yux.md` — `ScrollAnchoredList` / `ListView` splice-anchoring (the scroll
  primitive INV-UX-1 leans on).
- ADR-0024 — Model C (read-only transcript + separate compose); the compose is
  the surface INV-UX-2 governs.

## Revision history

- 2026-07-12 — Added INV-UX-23: a moved transcript fingerprint is ALWAYS
  rendered (no stale tail). Fixes the intermittent "last agent message doesn't
  render" bug: the transcript's self-notify is silently dropped by gpui
  `mark_view_dirty` when the view has no `view_path` in the committed frame, so
  the cached prepaint is reused stale. Backstop keys the cached transcript's
  element id on `TranscriptSeqs::fingerprint_hash()`, forcing a render via a
  fresh `GlobalElementId` independent of the notify hop. Guard
  `transcript_dropped_notify_id_forces_render` (verify_harness) with a
  deterministic dropped-notify repro + negative control.
- 2026-07-09 (2) — Added INV-UX-21: Cmd+V of a clipboard image stages it as a
  pending attachment (chip above the compose), sent on submit as an ACP
  `ContentBlock::Image` (both submit paths) with a `🖼` transcript marker;
  attachments clear after send and are ephemeral (not persisted). Wire carries
  `Request::Prompt.images` additively. Guards `image_paste_stages_pending_attachment`
  + `image_submit_sends_block_marks_transcript_and_clears` (verify_harness),
  `prompt_payload_*` (acp_channel), `prompt_*_images` (session_proto).
- 2026-07-09 — Added INV-UX-22: the agent model is switchable per session, live,
  from the picklist the agent advertises (`space M` submenu or the clickable
  `model ▾` badge) via ACP `session/set_config_option` — never a hardcoded list,
  and conversation-preserving. Guards in `acp_channel.rs` + `verify_harness.rs` +
  `tests/model_switch_live.rs`.
- 2026-07-09 — Added INV-UX-20: a summoned session recap (agent menu `R`) is an
  LLM prose summary pinned INSIDE its agent tile, above the subagents/tasks
  panels, keyed per-session (`recaps: HashMap<SessionId, RecapState>`) so it's
  specific to that tile; re-runnable, dismissed, and generated on a throwaway
  side-channel so it never touches the visible transcript. Guards `recap_*` in
  `verify_harness.rs`. (Revised same day from an initial jump-panel placement.)
- 2026-07-06 — Added INV-UX-17: the `App::Keymap` reference tile shows the LIVE
  keymap (one `KeymapRegistry` drives both `register_keymap` and the tile) and
  rebinds it in place (apply + persist; capture grabs the keyboard). New feature:
  a dynamic, rebindable keybindings sheet (Cmd-/). Guards `keymap_*` in
  `verify_harness.rs`.

- 2026-07-02 (8) — Added INV-UX-16: keystrokes that route to the compose are always
  painted. `inline_you_block_active()` now derives from `focus==Compose` (not just
  `you_block_open`), closing the recurring "`/clear` worksheet-invisible" bug
  (routing keyed on `focus`, painting keyed on `you_block_open` — the disagreement
  set was "the hole"). Aligned the flat-list injection + view-model memo key onto
  the shared gate. Guarded by `clear_worksheet_hole_types_and_paints` (3 edits
  independently negative-controlled). Root cause + fix + adversarial critique:
  `docs/projects/clear-worksheet-invisible/`.
- 2026-07-02 (7) — Added INV-UX-15: focusing a subagent swaps the main agent view
  to its context (Back header + prompt/content/output), Back/Esc returns. Reworked
  INV-UX-12's highlight behavior from a transcript-scroll into this swap (the
  scroll machinery — `pending_reveal_line` / `plan_anchor` — was removed). Plan
  panel entries are now theme-colored + wrapped (readable full text on any scheme).
- 2026-07-02 (6) — Added INV-UX-14: X11-style select-to-clipboard. A finalized
  mouse drag-selection auto-copies to the system clipboard on both the buffer doc
  view (`doc_mouse_up`) and the agent transcript (new `TranscriptView`
  mouse handlers + a paint-time monospace token hit-test sink,
  `register_token_on_paint` / `hit_test_tokens`). Guarded by
  `doc_drag_autocopies_selection_to_clipboard` +
  `transcript_drag_autocopies_selection_to_clipboard`.
- 2026-06-29 (5) — Added INV-UX-13: document text zoom (`Cmd-±`) now scales the
  agent **transcript** (conversation prose + markdown blocks) by `text_scale`, like
  the buffer doc view; chrome + the pixel-pinned compose input stay native. The
  size must live on each `FlatItem::Line` wrapper — `gpui::list` items don't inherit
  the list's ambient text size, which is why an initial `claude-body`-only attempt
  left the prose unscaled. Guard: `transcript_prose_scales_with_zoom` (probes the
  painted line height grows 1×→2×).
- 2026-06-29 (4) — Reworked the agent bottom panels into **two side-by-side
  columns** above the compose — Plan/Tasklist (left) + Subagents (right) — and added
  INV-UX-12 (`Cmd-0` focuses + enlarges them; **2-D** vim selection: `h`/`l` switch
  column, `j`/`k` move the row within it, `g`/`G` ends; `Enter` activates, `Esc`
  restores prior focus; modal; re-seats/auto-exits when the active column closes;
  `Cmd-0` is panel-focus, not zoom-reset, in agent tiles). Guards:
  `agent_panel_cmd0_enters_and_esc_restores`, `agent_panel_vim_moves_selection`,
  `agent_panel_hl_switches_columns`, `agent_panel_enter_focuses_subagent`,
  `agent_panel_cmd0_binding_enters_panel`, `agent_panel_closing_last_panel_exits_focus`,
  + fuzzer ops/oracle.
- 2026-06-29 (3) — Added INV-UX-10 (jump-panel agent dot is a per-session status
  light: working=warm accent, waiting-for-you=green, neutral=dim/disconnected;
  mapping `AgentRow::dot_status`) and INV-UX-11 (`ctrl-<n>` jumps to the n-th
  workspace, the number the panel shows; `goto_workspace_number` skips ephemeral
  workspaces). Guards: `agent_status_dot_reflects_turn_phase`,
  `agent_dot_status_mapping`, `ctrl_digit_switches_workspace`,
  `workspace_number_skips_ephemeral`.
- 2026-07-01 — **Worksheet rests TYPEABLE, not in nav (supersedes 2026-06-29 (2)).**
  Resting in nav made `/clear` (and fresh open) require pressing `i` before anything
  you typed appeared — the "can't see anything I'm typing after clear" bug. A fresh /
  cleared / restored worksheet now rests focused + Insert (`settle_input_focus` →
  focus=Compose), so typing lands and is visible immediately. The tile-menu leaders
  are NOT lost: the `<space>`/`.`/`?` still fire on an EMPTY block via the empty-draft
  heuristic in `focused_in_insert_mode` (bare space on a blank block opens the menu;
  once you've typed, space types — the same rule as mid-turn steer). Both prior
  complaints now hold. Guards: `worksheet_typing_after_clear_is_visible_without_pressing_i`,
  `clear_resets_worksheet_to_a_typeable_block`, `fresh_worksheet_space_opens_the_tile_menu`,
  `focused_in_insert_mode_agent_tile_gate`.
- 2026-06-29 (2) — Worksheet rests in transcript NAV (not auto-Insert) after fresh
  open / `/clear` / restore: the input block is VISIBLE but the `space`/`.`/`?`
  leaders open the tile/app menus (they fire only outside text entry) — fixing "can't
  use the tile menu when an agent tile is focused"; press `i` to type. Tool-group
  fold headers clamped to one line (`fold_header_line`) so a multi-line command no
  longer renders its body in the header ("tool use not folded"). Guards:
  `fresh_worksheet_space_opens_the_tile_menu`, `fold_header_line_is_single_short_line`.
- 2026-06-29 — Worksheet runtime fixes: `/clear` settles to a typeable block;
  multiple insertion points editable deterministically (caret on an existing block
  resumes it); you-div scoped-Normal indicator (`You · NORMAL` + accent tint) and
  nav-cursor highlight on you-divs; **`Esc` unbound from stopping a turn** (INV-UX-7
  updated — stop is `⌘.`; Esc is the worksheet mode key); bare-`m`/`'` mark chord
  gated to transcript-nav only so `m`/`'` stay typeable in the compose.
- 2026-06-28 — Added INV-UX-8 (worksheet renders inline-flush in the transcript
  column; chatbox stays a pinned box — the two placements are now visibly
  distinct, closing the "toggling worksheet does nothing" gap). The deferred
  `design-c.md` §7/v1 inline-flush styling. Guards
  `worksheet_renders_flush_chatbox_renders_boxed` in `verify_harness.rs`.
- 2026-06-26 — Added INV-UX-7 (mid-turn submits steer/queue instead of starting a
  competing turn; no optimistic transcript echo; Esc interrupts an in-flight
  turn). See `spec-turn-steering.md`. Guards `steering_*` +
  `esc_interrupts_in_flight_turn` in `verify_harness.rs`. (INV-UX-6 reserved for
  the parallel tool-group branch.)

- 2026-06-25 (5) — Added INV-UX-5 (subagents detected structurally from the
  harness + shown as panes below the compose, with the prompt). Fixes the
  `kind==Other` detector that never matched a real `Task`. Guards
  `classify_subagent_detects_the_harness_task_shape` +
  `subagents_surfaces_registered_task_with_prompt`.

- 2026-06-25 (4) — Added INV-UX-4 (no empty turn header) → `honored`: a right→left
  pass in `rebuild_agent_view_model` drops `TurnHeader`s with no content before the
  next header (the "blank turns" bug). Guard `rebuild_drops_empty_turn_headers`.
- 2026-06-25 (3) — Added INV-UX-3 (agent text uses the tile/desktop background;
  no per-turn card tint) → `honored`: `transcript_view.rs` `row_bg` transparent
  for all turns; focus-row highlight retained, gated on transcript focus.
- 2026-06-25 (2) — INV-UX-2 implemented → `honored`: the compose word-wraps
  (`wrap_line_cols` / `build_chatbox_wrapped_line`), retiring its horizontal-scroll
  window; INV-UX-1's compose horizontal half is now moot. Tests
  `wrap_line_cols_word_wraps_and_covers_every_char` +
  `caret_visual_row_places_caret_on_a_rendered_row`.
- 2026-06-25 — Created. INV-UX-1 (cursor always visible + tracks text;
  `partial`/`honored`), INV-UX-2 (agent compose word-wraps; `target` — chatbox
  currently horizontal-scrolls, a known gap).
