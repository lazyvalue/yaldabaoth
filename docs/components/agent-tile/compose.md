# Agent Tile — Compose

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-9..14`, `-21`.

## Description

The agent input surface, in two placements (`InputModeKind`): **Worksheet**
(default) — an inline-editable conversation buffer where `i` opens a You-block at
the caret in the transcript column — and **Chatbox** (message box) — a diminutive
pinned box at the bottom, shown only mid-turn, whose input steers/queues. Both
placements are editable surfaces and obey the common `TextEditing` model
(word-wrap, caret always visible). Primary code home: `screens.rs::render_agent`
(the `compose_panel`), `agent.rs`, and `agent_ui.rs`.

## References

- `docs/components/common/text-editing.md` — the compose buffer is an editable
  surface and obeys `TextEditing`.
- `docs/specs/spec-worksheet.md` — the worksheet inline-edit design (AUTHORITATIVE
  for INV-UX-9).
- `docs/specs/spec-textbox-compose.md` — the compose-surface design.
- `docs/specs/spec-turn-steering.md` — the v2-ready mid-turn steering shape.
- Migrated from `docs/ux-invariants.md`: INV-UX-2, INV-UX-8, INV-UX-9, INV-UX-16,
  INV-UX-7, INV-UX-21.

## UX invariants

### UXI-AgentTile-9 — The agent compose (chatbox / worksheet) always word-wraps

**Statement.** The agent tile's compose buffer wraps long lines to the available
width. Text never runs off the right edge of the box requiring horizontal scroll
to read it; a long line flows onto the next visual row.

**Applies to.** The agent compose buffer in BOTH placements
(`InputModeKind::Chatbox` pinned box, `InputModeKind::Worksheet` inline).

**Why.** A compose box is for composing prose; horizontally-scrolled input is
unreadable and you lose sight of what you wrote. Wrapping keeps the whole draft
visible.

**Status.** `implemented` (runtime-unverified for paint, per the GPUI headless gap).
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

### UXI-AgentTile-10 — Worksheet renders inline-flush; chatbox renders as a pinned box

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

**Status.** `implemented` — placement chrome implemented; the visible distinction is
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

### UXI-AgentTile-11 — The worksheet is an inline-editable conversation buffer (chatbox is mid-turn only)

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
6. **Multiple insertion points — but never two ADJACENT** (adjacency clarified
   2026-07-16, bug-0004). Insert at a **genuinely separated** legal point (surviving
   non-blank content between it and the existing blocks) opens another block (the
   previous parks in place, text kept); one is active, the rest render inline
   read-only; Submit sends them all combined + freezes each in place. A fresh (empty)
   session opens one tail block so there's always a visible input.
   - **Two You-blocks must NEVER render next to each other.** Insertion points are
     resolved by RENDER SLOT, not raw anchor: if the caret's snapped anchor would
     render adjacent to an existing block (no surviving non-blank line between — e.g.
     the blank tail line between them collapses), the insert **resumes that block**
     instead of spawning a neighbour. Enforced in
     `AgentState::open_you_block_at_cursor` via `you_blocks_would_be_adjacent`
     (every doc line strictly between the two anchors is blank ⇒ they collapse into
     one slot ⇒ resume). Pinned by `worksheet_you_blocks_never_render_adjacent`
     (NC-verified: matching on raw anchor equality renders two adjacent YouBlocks →
     RED); genuine separated multi-insertion still covered by
     `worksheet_multiple_insertion_points`.
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

**Status.** `partial` — **stages 1 + 2** (tickets 001/002) landed:
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

### UXI-AgentTile-12 — Keystrokes that route to the compose are ALWAYS painted (routing ⇒ painting)

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

**Status.** `implemented` (headless — the real key handler + real render, with the
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

### UXI-AgentTile-13 — A submit is delivered immediately (even mid-turn); failed sends queue, never drop (stop is ⌘., not Esc)

> **Numbering note:** `INV-UX-6` is reserved for the parallel `toolgroup-expand-key`
> branch (tool-group collapse). This invariant is `INV-UX-7` to avoid a collision
> at integrate time.

**Statement.** Submitting a message **sends it to the agent immediately, even
while a turn is in flight**, and commits it as a user turn — it does **not** start
a duplicate competing local turn (a mid-turn steer rides the in-flight turn; the
running clocks are not reset). When the agent advertises the `promptQueueing`
capability the worker forwards the prompt concurrently, so the agent receives the
steer mid-turn and processes it the instant the current turn finishes. If the send
**fails** (offline / reconnecting) the draft is **left in the compose** with a
status so the user can retry — never silently moved or dropped. The stop gesture is
**`⌘.`** (`stop_agent` → `session/cancel`; a second press force-restarts). **`Esc`
is NOT a stop** — it is the worksheet mode key (Insert→Normal, leave-block), and
binding it to stop conflicted with mode switching, so it was unbound (2026-06-29).

**Applies to.** The agent tile: `submit_compose` / `send_prompt_to_session`
(`agent_ui.rs`) and the worker's concurrent driver (`acp_channel.rs`, gated on
`promptQueueing`). There is no client-side steering queue — delivery is immediate.

**Why.** Over ACP **v1 a prompt is a turn** and there is no mid-turn input
message — but the live agent (`claude-agent-acp`) advertises a vendor capability
`_meta.claudeCode.promptQueueing` and (verified by live probe) **accepts a
`session/prompt` while a turn is in flight**, queueing it without interrupting.
yalda's worker previously *serialized* (awaited each turn before sending the next),
so a steer couldn't reach the agent until the boundary; the concurrent driver
fixes that. ACP v2 (the `unstable_protocol_v2` draft) is NOT yet honored by the
agent — it negotiates down to v1 — so promptQueueing is the real mechanism, and
this design is the v2-ready shape (`spec-turn-steering.md`).

**Status.** `implemented` — state, ordering, and transport verified.

**Enforcement.** Headless in `verify_harness.rs`:
`steering_submit_while_awaiting_sends_immediately` (mid-turn submit sends + commits
+ doesn't reset the turn), `steering_midturn_ordering_and_dedup` (steer lands after
prior agent content, committed once, agent echo deduped — via the real reducer),
and `esc_interrupts_in_flight_turn` / `stop_interrupts_only_when_in_flight`.
Transport (live, not headless): `tests/steering_midturn_live.rs` drives the REAL
worker + REAL `claude-agent-acp` and proves a mid-turn steer is delivered and
processed; the v2-refused / promptQueueing facts were confirmed by a live probe.
The only thing left for a human is the subjective visual feel.

### UXI-AgentTile-14 — A pasted image is staged, shown, and sent as a content block

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

**Status.** `implemented` (headless for the paste-staging, the mixed content-block
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

### UXI-AgentTile-21 — `[N]r` over agent text opens a reply You-block seeded with a quotation

**Statement.** In an **idle worksheet** with the **transcript focused** (Normal
mode) and the caret resting on an **agent line** at a legal insertion point,
pressing `r` opens a You-block — exactly like `o`/`i` (same open/park/legality
mechanics) — but **seeded** with a quotation of the agent text the caret is on:

```
re
> <first N sentences of the caret's line, joined on one line>
▏
```

Concretely:

1. **The quoted block is the caret's line.** The "agent block" is the rope line
   under the caret (the nav-highlighted element; a wrapped paragraph is one
   logical line). The quote is drawn from that line's text only — it **never
   spills** into the next line/paragraph.
2. **Count = sentences.** A vim-style numeric prefix chooses how many sentences
   to quote: `3r` quotes the first three sentences; a bare `r` quotes the first
   one. `N` is read from the same `pending_count` used by every other counted
   action (`keybind.rs`). The count **clamps** to the sentences available (`9r`
   on a 3-sentence line quotes all 3, no error).
3. **Sentence definition.** A sentence ends at `.`/`!`/`?` — optionally followed
   by a run of **closing markup** (`*`, `_`, `` ` ``, `~`, `)`, `]`, `}`, `"`,
   `'`, `»`, `”`, `’`) — **then whitespace or end-of-text**. A decimal point
   (`3.5` — the dot is followed by a digit) and a common abbreviation (`e.g.`,
   `i.e.`, `vs.`, `etc.`, `Mr.`, `Dr.`, …) do **not** split. Sentences are joined
   by a single space onto **one** quote line (Option A — not one `>` line per
   sentence). This is a heuristic; exotic punctuation is out of scope.
   - **Closing markup must not swallow the boundary** (fixed 2026-07-21, reported
     as "bold breaks the sentence parser"). `*this sentence is bold.*` put a `*`
     between the `.` and the space, so the whitespace rule failed and the sentence
     ran on into the next one. The closers are consumed AND kept **inside** the
     returned sentence, so the quoted markup stays balanced. A `*` that is not
     followed by whitespace still doesn't fabricate a boundary (`a.*b` is one
     sentence).
4. **Caret lands after the quote.** The seed ends in a trailing newline, so the
   compose caret (via `Compose::seeded`, cursor-at-end) rests on the **blank line
   below the quote**, in Insert, ready to type the reply. The literal first line
   is the two characters `re`.
5. **Same legality gate as `o`/`i` — no new surfaces.** `r` fires only where a
   You-block may open: idle (not mid-turn), transcript-focused, and the caret at
   a `you_block_anchor_is_legal` point (the latest agent turn or the tail). Over
   an older/frozen turn, over your own text, or mid-turn, `r` is a **no-op** (no
   block opens; a status hint is shown), matching `o`. A blank line or a line
   with **no sentence text** is also a no-op (nothing to quote).

**Applies to.** `agent_ui.rs` — the worksheet Normal-mode transcript dispatch
(`handle_claude_key`), a new `r` branch beside the `i`/`a`/`o` branch that calls
`AgentState::reply_quote_at_cursor`. `agent.rs` — `reply_quote_at_cursor`
(reads+clears the count via `keybinds.take_count`, checks legality, extracts the
quote, opens the block, seeds the compose) and the pure `first_n_sentences`
splitter. Reuses `open_you_block_at_cursor` (open/park/anchor), `Compose::seeded`
(caret-at-end), and `you_block_anchor_is_legal` (gate). Builds on
[UXI-AgentTile-11](#uxi-agenttile-11--the-worksheet-is-an-inline-editable-conversation-buffer-chatbox-is-mid-turn-only).

**Why.** Replying to a specific thing the agent said meant hand-retyping or
manually quoting it. `r` makes "reply, quoting what you just said" a single
keystroke (with a count for how much), reusing the whole You-block machinery so
the reply is an ordinary pending user turn — sent and frozen like any other.

**Status.** `implemented` — `reply_quote_at_cursor` + `first_n_sentences`
(`agent.rs`) and the `r` branch in `handle_claude_key` (`agent_ui.rs`). The inline
caret tint over the reply is a paint/human-eye detail (harness gap #1); the
behaviour (open, seed text, caret line, count, no-op) is headless.

**Deviation from plan.** (1) The count-overflow **clamp** and the no-terminator /
blank cases are guarded at the pure-function level (`first_n_sentences`) rather
than through a keystroke, because the harness end-to-end tests already cover the
1- and 3-sentence keystroke paths; the clamp is a property of the splitter, not
the dispatch. (2) The planned "`r` refused over an OLDER turn" keystroke test was
replaced by `worksheet_r_noop_on_blank_line` (a legal-but-empty tail anchor):
the two-turn synthetic batch didn't tag distinct turns in the harness, and the
older-turn refusal is the SAME `you_block_anchor_is_legal` gate already guarded
for `o` (`worksheet_stale_anchor_is_rejected`). (3) Edge: `r` on a slot that
already holds a parked/active draft **reseeds** it (the open reuses that slot,
then the seed replaces the draft) — a reply is a fresh quote by intent; every
OTHER parked block is preserved by `open_you_block_at_cursor`.

**Enforcement.** Headless: `verify_harness.rs::worksheet_r_seeds_reply_quote_from_agent_line`
(real `handle_claude_key(r)` over a multi-sentence agent line → the open You-block
draft is exactly `re\n> First sentence.\n` and the compose caret is at line 2 col 0,
the blank tail), `worksheet_count_r_quotes_n_sentences` (`3` then `r` through the
real dispatch → `re\n> One. Two. Three.\n`, exercising the shared `pending_count`),
and `worksheet_r_noop_on_blank_line` (legal-but-empty tail → no block opens). Pure
unit: `tests.rs::first_n_sentences_splits_and_respects_abbrevs` (count/join, clamp,
`e.g.`/decimal no-split, `?`/`!` terminators, blank ⇒ `""`) and
`tests.rs::first_n_sentences_terminates_through_closing_markup` (the bold/emphasis
regression: `*…bold.*`, `**Bold.**`, `_Emph._`, `` `code.` ``, `(Aside.)`,
`"go."`, counting across emphasised sentences, and `a.*b` NOT splitting). Negative
controls (both observed): reverting the seed to `""` turns the draft-equality
asserts RED (`left: ""`); disabling the closing-markup run turns the bold case RED
with `left: "*this sentence is bold.* Next one."` — the exact reported symptom.
The seeded `> ` quote renders italic per
[UXI-Blockquote-1](../common/blockquote.md).
