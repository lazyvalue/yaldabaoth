# Agent Tile — Transcript

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-4..8`.

## Description

The agent conversation surface: a **cached child entity** (`transcript_view.rs`,
`TranscriptView`; the reference yux component, load-bearing for typing latency)
that renders the transcript **block-by-block**. It is the read-only reading
surface — per-turn gutter label + author tint + left bar (no per-turn card
background), tool-call cards with status glyphs, collapsible tool groups, diff
highlighting, wiki-links, selection + copy, a navigation caret with focus-row
highlight, and a thinking indicator while awaiting. Append-only / ordered
(INV-ORDER, Model C / ADR-0024).

## References

- INV-UX-3, INV-UX-4, INV-UX-15, INV-UX-19, INV-UX-23 in
  `docs/ux-invariants.md` → migrated here.
- `docs/components/agent-tile/README.md` — parent component.

## UX invariants

### UXI-AgentTile-4 — Agent text uses the normal tile/desktop background

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

**Status.** `implemented` (runtime-unverified for paint). `transcript_view.rs` sets
`row_bg` to transparent for every committed turn; the per-turn `claude_turn_bg`/
`user_turn_bg` (theme `agent_turn_bg`/`user_turn_bg`) tints are no longer applied.
The cursor-row dim highlight remains, gated on transcript focus
(`cursor_line == usize::MAX` when composing, so no row matches).

**Enforcement.** Runtime (GPUI paint not headless): open an agent tile and
confirm agent/user turns show no background tint distinct from the tile; the
focused row highlights only during transcript (`f`) navigation. (A headless guard
awaits the element-tree-snapshot harness — `docs/projects/headless-e2e/` #3.2.)

### UXI-AgentTile-5 — No empty turn header

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

**Status.** `implemented`. After the flat-item build (blank-collapse, tool-group
merge, thinking indicator), `rebuild_agent_view_model` runs a right→left pass
that drops any `TurnHeader` with no non-header item before the next header.

**Enforcement.** Headless: `rebuild_drops_empty_turn_headers` builds a transcript
with empty turns (blank lines carrying escalating turn numbers) interleaved with
real turns and asserts no header is orphaned (every header is followed by content;
header count == content-bearing-turn count). Validated by disabling the pass →
the test fails.

### UXI-AgentTile-6 — Focusing a subagent swaps the main agent view to its context

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

**Status.** `implemented` (headless — the swap is proven by the layout probe; exact
pixels/colors are gap-1).

**Enforcement.** `verify_harness.rs`: `subagent_focus_swaps_the_painted_view` — with
a subagent focused, the `subagent-view` PAINTS and `transcript-viewport` does NOT;
after Back the `subagent-view` is gone (negative control: render the transcript
unconditionally ⇒ `subagent-view` never paints ⇒ RED). Plus
`panel_highlight_swaps_to_subagent` for the panel-driven entry.

### UXI-AgentTile-7 — A moved transcript fingerprint is ALWAYS rendered (no stale tail)

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

**Status.** `implemented` (headless — the reuse-decision path is deterministic in the
harness).

**Enforcement.** `verify_harness.rs`:
`transcript_dropped_notify_id_forces_render` — mutates the transcript editor
WITHOUT notifying the session (deterministically reproducing a dropped notify),
forces a root frame, and asserts the transcript render count still advances +1.
Negative control (observed RED): revert the embed to the fingerprint-independent
`cached_child(transcript_view)` and the count stays flat — the stale-tail bug
reproduced.

### UXI-AgentTile-8 — A tool call never splits an agent text token

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

**Status.** `implemented` (headless — the split is a buffer-content property the
reducer produces, fully observable without paint).

**Enforcement.** `verify_harness.rs`:
`tool_call_midtoken_does_not_split_agent_text_run` (drives the REAL
`apply_server_batch` → `append_llm_chunk_floored` with a `Chunk` / `ToolCallStarted`
/ `Chunk` mid-token stream; asserts the token stays whole AND the tool group
renders after the reassembled line; negative control: the buffer becomes
`` `m\n\node=max ``). `tests.rs`:
`floored_tools_and_text_stay_in_order_above_draft` pins the complementary
whitespace-boundary case (chunks ending `". "` stay interleaved with their tools).
