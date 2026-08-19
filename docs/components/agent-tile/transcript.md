# Agent Tile — Transcript

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-4..8`,
`UXI-AgentTile-23`, `UXI-AgentTile-25`, `UXI-AgentTile-26`, `UXI-AgentTile-28`,
`UXI-AgentTile-34`, `UXI-AgentTile-37`, `UXI-AgentTile-38`.

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
- `docs/components/common/paragraph-spacing.md` — the transcript's markdown
  blocks + list items obey `ParagraphSpacing` (`UXI-ParagraphSpacing-1`).
- `docs/components/common/diagram.md` — a `mermaid` fenced block renders inline as
  its diagram image in the transcript (`UXI-Diagram-1`).

## UX invariants

### UXI-AgentTile-40 — `J`/`K` move directly between user turns

**Statement.** In normal transcript navigation, bare uppercase `J` moves to the
next/newer user turn and bare uppercase `K` moves to the previous/older user turn.
They are direct, repeatable movement keys: there is no menu command to enter a
turn-jump mode and lowercase `j`/`k` retain their ordinary navigation behavior.
The movement clamps at the available turns; moving forward once more from the
newest turn reveals the page end.

**Applies to.** `agent_ui.rs::handle_claude_key` and
`main.rs::jump_user_turn`.

**Why.** Turn navigation is frequent, reversible motion. Requiring a leader-menu
toggle before every navigation sequence made it modal, harder to remember, and
inconsistent with the command-panel placement rule.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::uppercase_jk_move_directly_between_user_turns`
feeds multiple user turns through the real reducer, drives the real Agent key
listener with `Shift-J`/`Shift-K`, and asserts the jump ordinal moves without
enabling the legacy mode.

### UXI-AgentTile-4 — Agent text uses the normal tile/desktop background

**Statement.** **Agent** transcript text (`TurnId::Llm`) sits on the SAME
background as the normal yalda desktop / tile — there is no per-turn "card"
background tint behind agent turns. Agent turns are distinguished by the gutter
label, the foreground author tint, and the left bar — never by a different
background color. (Scoped to agent turns by ADR-0027: **user** turns DO carry a
faint background tint — see `UXI-AgentTile-23`. Tool/system turns are also
untinted.)

**Applies to.** The agent transcript (`TranscriptView`), agent-turn lines. The
transient focused-row highlight (a dim band on the cursor row, shown ONLY while
the transcript is focused for navigation) is NOT a violation — it's a focus/nav
cue, not a resting background. Code blocks keep their own background (code
styling, not a turn card). The compose box keeps its pinned-control affordance.

**Why.** A tinted card behind the *agent's* prose — the bulk of the transcript —
makes it read as a separate surface floating on the desktop; the agent's text
should blend into the tile like every other surface, so the workspace looks like
one continuous space. (The user's own turns are a small fraction and are
deliberately picked out — ADR-0027.)

**Status.** `implemented` (runtime-unverified for paint). `transcript_view.rs`
leaves `row_bg` transparent for agent/tool/system committed turns; only user turns
receive the `user_turn_bg` tint (`UXI-AgentTile-23`). The cursor-row dim highlight
remains, gated on transcript focus (`cursor_line == usize::MAX` when composing, so
no row matches).

**Enforcement.** Headless mapping: `verify_harness.rs`
`user_turn_gets_tint_agent_turn_does_not` asserts the row-background selector
returns transparent for an `Llm` turn line (and the tint for `User`). The actual
painted hue is a runtime check (GPUI paint not headless, gap #1): open an agent
tile and confirm agent turns show no background tint distinct from the tile.

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
output. A Claude child renders its rich parent Task call (`append_tool_body_rich`,
the same markdown/code/diff/chips sections the expanded tool card shows,
UXI-AgentTile-25). A Codex child is a separate durable thread, so focusing it lazily
loads that exact thread with ACP `session/load` and renders its user/agent/tool
timeline read-only. The loader is resume-only: failure or a stale child id is shown
as unavailable and can never fall back to creating a replacement session. The cached
main `TranscriptView` is **not rendered** while swapped. Returning to the main agent
is easy and always available: click **`← Back`**, or press **`Esc`** (`Esc` with a
focused subagent calls `unfocus_subagent`, ahead of its per-mode meaning). Switching
the panel highlight to a Plan row, or any `focused_subagent = None`, restores the
main transcript. The swap is a pure render-time branch on `focused_subagent`; no
transcript state is touched, so Back is lossless.

**Applies to.** `screens.rs::render_agent`: the `focused_subagent` match that builds
the `subagent-view` (Back header + `append_tool_body_rich`) OR the transcript body.
`agent_ui.rs`: `focus_subagent` / `unfocus_subagent` (set/clear), the `Esc`-returns
branch in `handle_claude_key`, and `reveal_panel_selection` (panel highlight → swap).
`agent.rs`: `focused_subagent: Option<SubAgentKey>`, `classify_subagent`, and the
Codex replay cache/reducer. `acp_channel.rs`: the resume-only load path.

**Why.** A subagent's work is a self-contained sub-conversation; reading it should
feel like *entering* it — a full view you can scroll — not squinting at an inline
expanded card. A single obvious Back (button + `Esc`) keeps it non-trapping.

**Bounds.** Claude context is whatever its Task tool call carries. Codex context is
the replayable child thread exposed by the adapter; this surface is an inspector,
not a second driver for that thread.

**Status.** `implemented` (headless — the swap is proven by the layout probe; exact
pixels/colors are gap-1).

**Enforcement.** `verify_harness.rs`: `subagent_focus_swaps_the_painted_view` — with
a subagent focused, the `subagent-view` PAINTS and `transcript-viewport` does NOT;
after Back the `subagent-view` is gone (negative control: render the transcript
unconditionally ⇒ `subagent-view` never paints ⇒ RED). Plus
`panel_highlight_swaps_to_subagent` for the panel-driven entry.
The provider-specific data paths are pinned by
`codex_child_replay_reducer_preserves_roles_and_tools` and the Codex classifier/fold
tests named in the sidepanel facet.

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

**Exception — a mouse gesture freezes the id (bug-0023).** Keying the element id on
the fingerprint also re-keys every DESCENDANT's element state whenever the
fingerprint moves. A press inside the transcript moves it by itself
(`transcript_mouse_down` sets the caret + focus, both fingerprint fields), so gpui's
`pending_mouse_down` was thrown away between down and up and NO `on_click` inside the
transcript ever fired — the tool fold header stopped expanding. So
`TranscriptView::element_fp_freeze` holds the fingerprint at its pre-press value from
mouse-down until mouse-up (`element_fp(live)`), the same gesture-scoped stability
bug-0015's `drag_protect_line` gives the flat-item count. The self-notify path keeps
invalidating normally during the gesture; only the dropped-notify backstop is
deferred, by one press.

**Status.** `implemented` (headless — the reuse-decision path is deterministic in the
harness).

**Enforcement.** `verify_harness.rs`:
`transcript_dropped_notify_id_forces_render` — mutates the transcript editor
WITHOUT notifying the session (deterministically reproducing a dropped notify),
forces a root frame, and asserts the transcript render count still advances +1.
Negative control (observed RED): revert the embed to the fingerprint-independent
`cached_child(transcript_view)` and the count stays flat — the stale-tail bug
reproduced. The freeze exception is pinned by
`tool_group_header_click_expands_the_fold` (below).

### UXI-AgentTile-29 — A folded tool block expands on click, and `j`/`k` hop over it

**Statement.** In the transcript:

1. **Click expands.** Clicking a folded tool-use group's header (`▶ ● bash …`)
   toggles it open/closed. This holds even though the press itself moves the
   transcript's caret + focus.
2. **Navigation hops over it.** A tool call splices a dedicated BLANK anchor line
   into the document, which renders as the tool card (its own blank `Line` item is
   stripped by blank-collapse). Transcript navigation therefore never RESTS on a
   tool-anchor line: a motion that would land there continues in the direction of
   travel to the next real content line, so one `j` (or `k`) crosses the whole tool
   block — including a run of back-to-back anchors that render as one merged group.
   A motion that doesn't change the line (`h`/`l`, an edge no-op) never teleports.

**Why.** (1) was a shipped regression: the fold was unopenable (bug-0023). (2) is the
matching keyboard behavior — the anchor line has no text, and stopping on it puts the
cursor bar on an invisible row (worse: it forces the blank line to be KEPT by the
caret-protection rule, so the block visibly grows a blank row as you pass it).

**Applies to.** `transcript_view.rs` — the `FlatItem::ToolGroup` header `on_click`
plus `element_fp_freeze` / `element_fp` (see the UXI-AgentTile-7 exception above);
`screens.rs` `render_agent` (`transcript_view.read(cx).element_fp(live_fp)`);
`agent.rs` — `AgentState::tool_anchor_lines` / `hop_cursor_over_tool_anchors` and the
pure `hop_over_tool_anchors`; `agent_ui.rs` — the transcript-nav dispatch.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs`:
`tool_group_header_click_expands_the_fold` — probes the header's REAL painted rect
and drives the window's REAL mouse dispatch (`simulate_click`) at it, asserting
`tools.expanded` flips (and, non-vacuously, that the press really does move the
render fingerprint). **NC observed RED**: use the live fingerprint for the wrapper id
(drop the freeze) ⇒ "clicking the folded tool-use header did NOTHING".
`transcript_jk_hops_over_tool_blocks` — real `handle_claude_key("j"/"k")` from the
content line above a TWO-anchor run; asserts the caret clears both in one press and
comes back. **NC observed RED**: drop the `hop_cursor_over_tool_anchors` call ⇒ the
caret lands on anchor line 1.

### UXI-AgentTile-8 — A tool call never splits an agent sentence

**Statement.** A tool-call row interleaved into an agent turn's streamed prose may
break it only at a **sentence boundary** — never inside a word, and never mid-clause.
The test is asked of the PRE-tool text alone: when a tool call arrives while the
turn's tail line is still OPEN (the last streamed chunk did not end the run) and that
line does **not** end a sentence — its content, trailing whitespace trimmed and
closing markup (`` *_`~)]}"'»”’ ``) stripped, does not end on `.!?:` — the
continuation **rejoins** the open run's end-of-content and the tool group renders
AFTER the completed text. A break after a finished sentence, or a continuation that
itself starts with a newline, is a legitimate `text → tool → text` interleave and is
left in place (tool between the two statements). So the reconstructed transcript
reads as the model wrote it: `` `mode=max` `` is never `` `m `` | ToolSearch |
`ode=max`, and `…the fix for` is never cut from ` it on my side…`.

**History.** The original rule (`dbe67be`) required the open line's last char AND the
chunk's first char to both be **alphanumeric** — mid-*word* only, conservative to
avoid mis-fusing ambiguous punctuation. That left every continuation starting with a
space or punctuation split (bug-0013, including a stranded lone `.` line). The rule
above supersedes it and is a strict superset: an alphanumeric last char is never one
of `.!?:`.

**Applies to.** `editor.rs` — `Editor::append_llm_chunk_floored` +
`continuation_rejoin_point` (the unfinished-sentence detector, and
`SENTENCE_CLOSERS`) vs `find_llm_insertion_point`
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
sentence-boundary case (chunks ending `". "` stay interleaved with their tools — the
trailing-whitespace trim is what keeps this true). AND
`verify_harness.rs::tool_call_midsentence_does_not_split_agent_sentence` (bug-0013)
— the same real reducer seam with the two NON-alphanumeric breaks: a continuation
starting with a SPACE, and one that is a bare `.`. Asserts the sentence is contiguous
and that no line is left holding only the terminator. Negative control: restore the
two `is_alphanumeric()` gates → it fails RED while the mid-word test stays green.

### UXI-AgentTile-23 — The user's own turns carry a faint background tint

**Statement.** In the agent transcript, a **user** turn (`TurnId::User`, the
frozen `U<n>` blocks — the messages the user sent) renders on a **faint blue
background band** (`AgentTheme::user_turn_bg`, per-theme), so the user's own
contributions stand out from the agent's at a glance. Agent turns (`TurnId::Llm`),
tool turns (`TurnId::Tool`), and system turns stay on the plain tile background
(`UXI-AgentTile-4`). The tint is *faint* — a low-contrast band, not a floating
card. This is a deliberate reversal, for user turns only, of the earlier
"no per-turn background" rule (ADR-0027).

**Precedence.** The transient nav-focus cursor-row highlight (a dim band on the
cursor row, only while the transcript is focused for navigation) OVERRIDES the
user tint on that one row, so no row shows two competing fills. Code blocks inside
a user turn keep their own code background. The live compose box / draft `You`
block is unaffected (it keeps its accent affordance; the tint is for *committed*
turns).

**Applies to.** `transcript_view.rs`: the pure `committed_row_bg(tag,
user_turn_bg)` selector (returns `user_turn_bg` for `TurnId::User`, transparent
otherwise) and the frozen-line `row_bg` in `TranscriptView::render` that consumes
it under the cursor-row check. `src/theme.rs`: `AgentTheme::user_turn_bg`
(retuned to a faint blue in every theme constructor).

**Why.** In a long transcript the user asked to pick out what *they* said versus
what the agent said. The left bar + `U<n>` gutter label + foreground tint were too
thin to scan by; a faint background band per user turn is scannable without
turning the transcript into a wall of cards (the agent turns — the bulk — stay
card-less, so `UXI-AgentTile-4`'s "one continuous surface" concern holds).

**Status.** `implemented`.

**Deviation from plan.** None material. The tint reuses the pre-existing
`AgentTheme::user_turn_bg` field (present on all 8 theme constructors, previously
an unused warm-green from the `UXI-AgentTile-4` removal), retuned in place to a
faint blue — no new theme field was added. The faint-blue hex per theme is a first
cut chosen for subtlety on each theme's background; the exact shade is runtime-
tunable by eye (gap #1).

**Enforcement.** Headless mapping (the paint hue is gap #1, human eye):
`verify_harness.rs` `user_turn_gets_tint_agent_turn_does_not` — the pure
`committed_row_bg` returns the passed `user_turn_bg` for a `TurnId::User` tag and
transparent for `TurnId::Llm` / `TurnId::Tool` / `TurnId::System` / `None`. Negative
control observed RED: force the `User` arm transparent → "user turn is tinted"
fails.

### UXI-AgentTile-25 — Tool-call inputs/outputs render as beautiful sections, not raw JSON

**Statement.** An expanded tool call's body — everywhere it appears (the inline
transcript tool groups AND the focused-subagent context view, UXI-AgentTile-6) —
renders as **typed, semantically-labeled sections**, never as
`serde_json::to_string_pretty` monospace blobs:

- **Markdown text renders as markdown.** A subagent's **prompt** (input) and its
  **report** (output), and any prose tool output, are parsed with the doc markdown
  renderer (`render_with_wiki` + `block_inner`) — headings, lists, code fences,
  bold, tables — in the proportional body font, code in the code font. The report
  gets an emphasized (warm-accent) tile; it's the star.
- **Per-tool structured input.** Bash → a `command` **code** section; Read/Search/
  Move → **chips** (`path`, `pattern`, `glob`, …) in the code font; Edit → the
  **diff** (from `content`, or synthesized from `old_string`/`new_string`), with
  `old_string`/`new_string` never dumped as JSON; Write → a `content` code section;
  Task → `agent` chip + `task` prose + `prompt` markdown.
- **Terminal output stays monospace.** Execute/Bash output renders as **code**, not
  markdown (a leading `#` is a shell comment, not an H1).
- **Content/output dedup.** When `content` and `raw_output` carry the same text
  (Claude Code mirrors output into content), it is shown **once**, not doubled.
- **JSON only as a last resort.** Genuinely unknown shapes fall back to
  pretty-printed JSON; a long/multiline string field of an unknown tool becomes its
  own readable **code** section (real newlines), not a `\n`-riddled one-liner.
- All colors/fonts are **theme-driven** (`AgentTheme` tiles: input = neutral
  `tool_card_border`/`tool_body_bg`, output = `agent_tint`/`tool_output_bg`, report
  = `warm_accent`), and every size multiplies by `text_scale` (zoom applies, which
  the old JSON tiles ignored). Payloads are byte-capped before parse; the inline
  transcript caps markdown blocks (a "+N more blocks" footer) while the focused
  subagent view shows the whole report.

**Applies to.** New module `tool_body.rs`: the **pure** planner
`plan_tool_sections(tc, policy) -> Vec<ToolSection>` (`SectionBody::{Chips, Prose,
Code, Markdown, Diff, Json}`) + `extract_output_text` (`agent.rs`), and the render
layer `render_tool_section` / `append_tool_body_rich` over a `ToolBodyCtx` (theme,
fonts, `text_scale`, markdown cap). `render_blocks.rs::render_markdown_column`
(read-only `block_inner` column). Call sites: `screens.rs` focused-subagent view
(drops its old `font_family(mono)` container) and `transcript_view.rs`
single-/multi-tool paths (`build_tool_block_with_weak` now takes a `ToolBodyCtx`).
Replaces `agent.rs::append_tool_body`/`tool_body_free` (removed).

**Why.** The subagent view and tool cards showed inputs/outputs as raw
pretty-printed JSON — hard to read, no formatting, markdown reports mangled into
escaped one-liners. The user asked to "make this beautiful": markdown reads as
markdown, code as code, edits as diffs, params as chips.

**Status.** `implemented` (headless for the section *planning* — the exact painted
glyphs/colors/markdown layout are gap #1, human eye).

**Enforcement.** `tests.rs` (pure, no gpui):
`plan_tool_sections_subagent_prompt_and_report_are_markdown` (subagent prompt +
report are `Markdown`, report emphasized; NC observed RED by forcing the output
branch to `Json`), `plan_tool_sections_bash_is_code_not_markdown`,
`plan_tool_sections_edit_synthesizes_diff`, `plan_tool_sections_dedups_content_and_output`,
`plan_tool_sections_unknown_multiline_is_code`, and
`extract_output_text_pulls_text_from_common_shapes`. **Main-transcript
integration** (`verify_harness.rs`):
`transcript_tool_body_renders_markdown_not_json` registers a real `Task` tool
call, paints the transcript (`run_until_parked`, no panic), and asserts through
`plan_tool_sections` over the STORED tool call that the prompt + report are
`Markdown` (not JSON) — the exact call the transcript render makes; NC observed
RED (report → `Json`). The rendered pixels (fonts/colors/markdown block layout)
are the runtime check (gap #1).

**Deviation from plan.** Fable's proposed per-view markdown **parse cache**
(`tool_md_cache`) is **not** implemented in this pass: the inline transcript is
already seq-gated + list-virtualized (parse runs only when the row's cache busts),
payloads are byte-capped (96 KB) and block-capped (40 inline), and the
focused-subagent view is a transient surface — so per-frame reparse is bounded.
The cache is a clean follow-up if a wall-clock `sample` shows jank on a huge report
(gap #3). `Todos`/`Terminal` rich variants were also left out of scope (TodoWrite is
`HeaderOnly`; terminals render as a placeholder as before).

### UXI-AgentTile-26 — Tool-body markdown wraps at the pane width; Task output is readable text, never escaped JSON

**Statement.** Two guarantees for the tool-body sections of UXI-AgentTile-25,
each a live-screenshot regression:

- **Markdown wraps at the pane width — never one glyph per line.** Every block a
  tool-body markdown section renders (`render_markdown_column`) gets a **definite
  full-pane width**, so a bullet-list item holding a long, unbroken string (a file
  path, a URL) wraps horizontally at the pane edge like any paragraph. It must NOT
  collapse into a vertical column of single characters. (Root cause: a list item's
  inner content column carries `flex_1().min_w_0()`, whose min-content floor is 0;
  without a definite width above it — which the doc view's `block_element` supplies
  via `w_full()` + a `flex_1().min_w_0()` content column but `render_markdown_column`
  did not — `flex_1` distributes 0, so the text wraps char-by-char.)
- **A subagent's output renders as readable text, not escaped JSON.** The Task /
  subagent tool returns its result as a **bare** top-level content-block array
  (`[ {type:"text", text:"…"} ]`), not wrapped in `{content:[…]}`. That text — with
  its real newlines — is extracted (`extract_output_text`) and rendered as the
  markdown **report**, not dumped as a `\n`-riddled escaped-JSON blob. Genuinely
  unknown shapes still fall through to pretty-printed JSON (UXI-AgentTile-25).

**Applies to.** `render_blocks.rs::render_markdown_column` (each block wrapped in a
`w_full()` flex row + `flex_1().min_w_0()` content column, mirroring `block_element`;
`list_item_element`'s row also carries `w_full()` + a `flex_none()` marker) and
`agent.rs::extract_output_text` (a `Value::Array(items)` arm joining bare
content-blocks, shared with the `{content:[…]}` arm via `join_content_blocks`).

**Why.** A subagent pane in the live app rendered the `Files:` bullet list as a
vertical stack of single characters and dumped the Task result as a raw escaped
JSON array — both unreadable (see the 2026-07-23 screenshot).

**Status.** `implemented` (headless for the layout geometry + the extraction; exact
painted glyphs/theme colors are gap #1, human eye).

**Enforcement.** `verify_harness.rs::subagent_markdown_list_wraps_at_pane_width`
focuses a real subagent whose prompt is a long-path bullet list, paints it, and via
the layout probe asserts the painted list block (`md-block-0`) spans > 50% of the
pane width (non-vacuous: pane > 400px, path far too long to fit) — NC observed RED
by reverting the `w_full()`/`flex_1()` width fix (block collapses to the ~24px
marker column). `tests.rs::extract_output_text_handles_bare_content_block_array` and
`plan_tool_sections_bare_array_output_is_markdown_not_json` cover the bare-array
extraction + its Markdown (not Json) planning — NC observed RED by deleting the
`Value::Array(items) =>` arm (bare array → `None` → raw JSON dump).

### UXI-AgentTile-28 — The tile always says whether the agent is working or ready

**Statement.** The agent tile's activity row always carries a fixed-width status
pill. It uses compact header-specific vocabulary:

| Condition | Pill |
|---|---|
| A reply is in flight (`turn_phase.is_awaiting()`) | **`* working`** in `agent.jump_working` orange |
| Idle, including a brand-new session | **`+ ready`** in `agent.tool_completed` green |

The pill is 88px wide in both states, followed by `turn N · M:SS` and the
conditional `■ Stop ⌘.` button. The editing readout does not duplicate activity
with an `awaiting reply` suffix.

**Applies to.** `screens.rs`: `agent_header_activity` and `render_agent`'s
`agent-status-pill`.

**Why.** With a wall of transcript above it, the only "the agent is running" signals
were a dim status-strip suffix and an elapsed clock — easy to miss, and there was no
positive "it's finished, it's on you" signal at all. One loud, colored, worded pill
in a fixed place answers both questions without reading the transcript.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::agent_tile_paints_a_status_pill_while_working`
— layout probe `"agent-status-pill"` on the real `render_agent`: present on a
virgin session and exactly the same width after entering the working state. The
word/glyph mapping is pinned by
`agent_header_uses_compact_activity_and_transient_editor_vocabulary`.

### UXI-AgentTile-31 — Header information has stable semantic rows

**Statement.** Header information renders in this order: identity/model/
permission/transient compose state; activity/turn/stop with optional context
usage; linked worktree name or working directory. Each group owns a row. Rows
may wrap their own contents on narrow tiles and remain unaffected by document
zoom.

**Applies to.** `screens.rs::render_agent` (`identity_row`, `activity_row`,
usage meter, `location_row`, and `header`).

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::agent_usage_paints_on_the_activity_header_line`
paints real usage state and proves identity → activity+usage → location order.

### UXI-AgentTile-34 — `V`/`v` select agent text in the worksheet transcript

**Statement.** In an **idle worksheet** with the **transcript focused** (Normal
nav), the caret moving over agent text can select it, vim-style:

- **`V` selects the whole current line** immediately and enters a distinct
  **linewise** visual state. A repeated `V` or any following motion keeps both
  endpoints on logical-line boundaries; in particular, `j`/`k` include the
  complete destination line regardless of its length.
- **`v` starts char-wise visual** — it drops an anchor at the caret; a following
  motion (`h`/`l`/`w`/…) extends the selection character by character.
- The existing Kakoune-style keys stay bound and additive: `x` = extend the
  current selection by one whole line (one-shot, not linewise visual state),
  `;` = collapse, `,` = flip, `%` = select-all.

The selection is the transcript editor's own anchor/head selection — the SAME
model the drag-select band and copy-on-select (`UXI-Selection-1`) render from —
so it paints as a highlight band while the transcript is focused, and a live
selection is what `r` quotes (`UXI-AgentTile-35`).

**Applies to.** `keybind.rs` — the default normal-map `key('V') → "extend-line"`
binding (beside `x`). The dispatch is the shared `dispatch_normal_core` the
worksheet transcript-nav already routes through (`agent_ui.rs::handle_claude_key`
fall-through). Selection state + band: `editor.rs` (`linewise_extend_mode`,
`select_linewise`, `normalize_linewise_selection`, `toggle_extend_mode`,
`selection_range`, `pre_move`) and
`transcript_view.rs` (`sel_snap`, gated on `transcript_focused`).

**Why.** `V` was **unbound**, so the vim instinct for whole-line select did
nothing, and `v` alone paints nothing until you also move — so selecting agent
text in the worksheet read as broken. The first implementation made the initial
range whole-line but reused characterwise extend mode afterward: `V j` inherited
the first line's sticky column and stopped in the middle of a longer destination
line. A distinct linewise state is required so every motion preserves the
gesture's contract rather than only its first frame.

**Status.** `implemented` — the binding + selection are headless (real keymap +
real dispatch). The universal token palette inside the selection is specified by
`UXI-AgentTile-38`; exact perceived contrast remains a paint/human-eye judgment
(harness gap #1).

**Deviation from plan.** `V` maps to a NEW action `"select-line"`
(`select_linewise`), not the pre-existing `"extend-line"` (`x`) as first scoped.
The intermediate implementation only turned generic extend-mode on; bug-0037
showed that this grew to the next line at a character column rather than the full
line. `EditorView` now records linewise state separately and normalizes motion
endpoints to the start/end boundaries. `x` keeps the plain extend-line action.
The selection color normalization deferred here is implemented by
`UXI-AgentTile-38`.

**Enforcement.** `verify_harness.rs`: `worksheet_v_line_select_feeds_r` (real
`handle_claude_key("V")` creates a non-empty selection whose text is the whole
agent line), `worksheet_v_then_j_extends_selection` and
`worksheet_v_then_k_selects_whole_previous_line` (real `V` plus vertical motion
across deliberately unequal line lengths selects both complete lines), and
`worksheet_v_char_select_feeds_r` (`v` + 5×`l` remains exactly `First`). The
vertical guard was observed RED before bug-0037's fix: `V j` selected only
`"one\ntwo"` from the longer second line. The painted band hue is gap #1.

### UXI-AgentTile-38 — Selected transcript text has one universal color treatment

**Statement.** Inside an active transcript selection, every prose/Markdown
token uses the selection background and the line's ordinary prose foreground.
A selected bullet or ordered-list marker therefore uses the same cool blue as
selected agent prose, rather than keeping its unselected green syntax color.
The rule applies to partial and whole-line selections and to other decorative
Markdown foregrounds as well; font semantics such as bold, italic, and
monospace inline code remain intact.

**Applies to.** `transcript_view.rs`, the `FlatItem::Line` selection projection;
`render_blocks.rs::apply_selection_style` and `style_uses_code_font`; and the
frontend-neutral `Modifier::MONOSPACE` marker used only when selection replaces
the colors that previously identified inline code.

**Why.** Selection is one interaction state, not another syntax-highlighting
layer. Leaving a bullet green inside a blue prose selection made a whole-line
`V` highlight look fragmented and suggested that the marker was outside the
selection. The selection projection must also align Markdown's rendered `•` with
its raw `-`, `*`, or `+`; treating rendering as deletion-only collapsed the bullet
mapping to end-of-line and could leave the entire displayed list item unpainted.
Author tint still distinguishes agent from user lines when selected.

**Status.** `implemented`. Exact perceived contrast remains harness gap #1, but
the selected foreground/background values and retained code-font decision are
deterministic and headlessly verified.

**Enforcement.** `tests.rs`
`transcript_whole_line_selection_unifies_bullet_and_prose_color` builds the real
stripped Markdown segments for a bullet and proves the selected marker and prose
have identical foreground/background colors. Negative control observed RED:
the background-only selection path left the marker `Rgb(80,250,123)` and prose
`Rgb(169,208,224)`; exercising the exact production projection also exposed the
rendered-marker/raw-marker alignment collapse. `stripped_bullet_marker_maps_back_to_raw_marker`
locks that substitution mapping, and
`transcript_selection_color_keeps_inline_code_monospace` proves color
normalization does not change inline-code typography.

### UXI-AgentTile-37 — The replied-to source text shows a `>` marker when not editing

**Statement.** When a pending worksheet reply quotes agent text
(`UXI-AgentTile-21`/`-35`), the **source** line(s) it quotes render with a
beautiful blockquote marker (a `>` / left bar + italic tint) in the transcript, so
it is obvious what a pending reply refers to. The marker shows while you are **not
typing in the reply block** (transcript nav, or the reply's compose dropped to
Normal) and is hidden while you actively type the reply. It is **pending-scoped**:
it appears while the reply quotes that source and clears when the reply is
submitted or abandoned.

**Applies to.** `agent.rs` — `reply_source_range: Option<(usize, usize)>` captured
in `reply_quote_at_cursor` (the selection's line span or the caret line), cleared
in `close_you_block` (submit / turn-begin / replay), the empty-Esc discard, and the
`u`-pop (`agent_ui.rs`); the `reply_marker_range()` gate (`Some` only while a reply
is pending AND not compose-Insert). `transcript_view.rs` — `TranscriptSeqs::reply_marker`
(the render-input seq), the `reply_marker_snap` threaded through `TranscriptPrep`,
and the frozen-line render branch that, per source line, sets a blockquote-colored
left bar + a `>` gutter glyph. `main.rs` — `DocRenderTap::reply_marker` + the
`push_reply_marker_line` test tap.

**Why.** With a reply that lands at the tail (`UXI-AgentTile-36`) but quotes text
far up in history, there is no visual back-link from the reply to its source; the
marker restores that.

**Status.** `implemented` — the state gate + render branch are headless (real
keystrokes + the paint tap). The exact glyph/bar hue is gap #1 (human eye).

**Deviation from plan.** The marker renders as a `>` in the line's **gutter**
(right-aligned `  >`) plus a blockquote-colored 3px left bar — NOT a literal `> `
prepended to the agent text. The gutter placement keeps the source text's columns
untouched, so transcript hit-testing / caret / copy on that line stay aligned
(prepending a `> ` would offset them). Text **italic** on the source line was left
out of this pass (bar + glyph already read as a quote); it's a cheap follow-up if a
live eye wants it. The planned `transcript_021_*` render-count test was not added
separately — the marker input lives in `TranscriptSeqs` (so the existing perf
suite's "chatbox typing leaves the transcript flat" still passes with the marker
inert), and the paint test below proves the marker input DOES re-render when it
moves.

**Enforcement.** `verify_harness.rs::worksheet_replied_to_source_shows_marker_when_not_typing`
— real `r` → `escape` → `u`: asserts `reply_marker_range()` is `None` while typing
(Insert), `Some((src, src+1))` once dropped to Normal, and that the `>` marker
PAINTED on the source line via `DocRenderTap.reply_marker`; then `u` pops the reply
and both the state and the paint clear. **Two NCs observed RED:** (a) force
`is_marker_line = false` ⇒ the paint tap is empty though the state says active; (b)
drop `reply_source_range = None` in the `u`-pop ⇒ the marker survives the abandon.
