# Agent Interaction & Rendering Pipeline

**Status:** DRAFT

**Last updated:** 2026-06-04

## Builds On

- **`spec-agent-window.md`** — Defines the structural layout (status strip, transcript,
  chatbox, sidepanes) and the worksheet/chatbox behavioral contracts, plus the Editor
  Extensions (`§E1` LineAnchor, `§E2` LineMetadata, `§E3` `append_llm_chunk`). This spec
  does **not** redefine layout or worksheet semantics. It defines the **data-flow
  lifecycle**: how an ACP signal becomes a rendered pixel, and how a transcript is
  reconstructed when a session is refreshed.
- **`spec-agent-presentation.md`** — Defines the *visual* treatment of each element
  (spacing, color, typography). This spec defines what elements *exist to render* and in
  what order; presentation styles them.
- **`spec-multi-session.md`** — Defines the ring, persistence schema, and the resume path
  (`session/load`). This spec specifies what that resume path must *reconstruct in the
  transcript* — the part `spec-multi-session.md` left to "the agent replays it."
- **`src/acp_channel.rs`** — The ACP transport. Owns the subprocess, the notification
  handler, and the `ReplyEvent` queue this pipeline consumes.

## Why this spec exists

Two observed defects motivated it, and neither was covered by an existing spec because the
existing specs describe *structure* and *appearance* but not the *data lifecycle*:

1. **Agent text is sometimes cut off in the view.** Streamed agent output occasionally
   appears truncated at the tail.
2. **On session refresh, the user's own text is not visible.** After reboot / re-open /
   resume, agent replies reappear but the user's prompts are gone.

This spec states the invariants that make both impossible, so the codebase can be measured
against them.

## Overview

The agent transcript is produced by a one-directional pipeline. Each stage has a single
responsibility and a defined hand-off:

```
ACP subprocess
   │  SessionUpdate notifications (live stream AND session/load replay)
   ▼
[1] Channel decode        src/acp_channel.rs   on_receive_notification
   │  ReplyEvent queue (Chunk, ToolCall*, Plan, Mode, Usage, …)
   ▼
[2] Pump drain            main.rs pump_session / apply_reply_events   (16ms loop)
   │  mutates AgentState: editor splice, tool_calls, plan, usage
   ▼
[3] Editor model          src/editor.rs append_llm_chunk + frozen ranges + TurnId meta
   │  rope text + frozen_ranges + LineMetadata<TurnId> + LineAnchors
   ▼
[4] View-model build       main.rs render_agent  (memoized on view_model_fingerprint)
   │  Vec<FlatItem> (TurnHeader | Line | Block | ToolGroup | ThinkingIndicator)
   ▼
[5] Block parse            detect_block_ranges / parse_block_range  (cached on frozen count)
   │  RenderedBlock for table/code/heading ranges
   ▼
[6] List reconcile         list_state.splice / reset ; list_item_count
   │  GPUI ListState item count == flat_items.len()
   ▼
[7] Paint + scroll         build_wrapped_line per visible item ; scroll_to_reveal_item
        pixels
```

The two defects are failures of stage **[1]** (refresh) and stages **[6]/[7]** (cut-off),
respectively. The spec is organized stage-by-stage; each stage lists its **contract** and
its **invariants**.

## Named artifacts

- **ReplyEvent** — the channel's decoded output enum (`acp_channel.rs`).
- **AgentState** — per-session state holder (transcript editor, tool calls, plan, list
  state, follow flag). Renamed from `ClaudeState`.
- **TurnId** — `enum { Llm(usize), User(usize), Tool(usize) }`, stored per-line in
  `LineMetadata<TurnId>`, keyed by `LineAnchor`.
- **FlatItem** — one renderable transcript row: `TurnHeader | Line(idx) | Block(b) |
  ToolGroup{anchor,ids} | ThinkingIndicator`.
- **view_model_fingerprint** — the memo key that decides whether `flat_items` is rebuilt.
- **follow_output** — sticky-bottom flag; true when the viewport is pinned to the tail.

## Stage contracts

### [1] Channel decode — `acp_channel.rs::on_receive_notification`

**Contract.** Translate every `SessionUpdate` that carries transcript-visible content into a
`ReplyEvent`. The channel is the *only* place that decides what the rest of the app can ever
see; an event dropped here is unrecoverable downstream.

**Critical distinction — live vs replay.** The same notification handler serves two regimes:

- **Live turn:** the user submits a prompt; the agent streams `AgentMessageChunk`s. The
  user's own text was already placed in the transcript locally by Submit (stage [3]), so the
  agent's *echo* of the user message (`UserMessageChunk`) is redundant and may be dropped.
- **Resume replay:** on `session/load`, the agent re-emits the **entire prior
  conversation** as `SessionUpdate` notifications — including the user's past prompts as
  `UserMessageChunk` — without any local Submit having run. The editor starts empty; the
  replay is the *only* source of transcript content.

**INV-1 (replay completeness).** Every role that appears in the transcript during a live
session MUST be reconstructible from the replay stream. Concretely: `UserMessageChunk` MUST
be forwarded as a `ReplyEvent` so user turns can be rebuilt on resume. Dropping it is only
acceptable if user turns are reconstructed from some *other* persisted source (they are not
— see stage [3]).

**INV-2 (no silent drop of visible content).** Any `SessionUpdate` variant that carries
content the user would see in a live turn (agent text, user text, tool calls, plan) must
either be forwarded or have a written rationale for why its absence is invisible to the
user. "Parked / out of scope" is acceptable for *additive* signals (thoughts, available
commands, session-info) but NOT for content that breaks transcript reconstruction.

### [2] Pump drain — `main.rs::pump_session` / `apply_reply_events`

**Contract.** Drain the `ReplyEvent` queue on the 16ms loop, applying each event to
`AgentState`. A `Chunk` is spliced into the editor; `ToolCall*` updates the tool store;
`Plan`/`Mode`/`Usage` update their fields. After draining, apply the auto-scroll policy
(stage [7]).

**INV-3 (turn attribution).** Each spliced chunk is tagged with the correct `TurnId`. Agent
chunks → `TurnId::Llm(k)`; replayed user chunks → `TurnId::User(k)`. The turn counter is the
single source of `k`; it advances identically on live turns and on replay so gutter tags and
TurnHeaders are correct in both regimes.

**INV-4 (drain completeness).** The drain budget must not strand events across a regime
boundary. On `session/load`, the full replay (potentially hundreds of notifications) must be
fully drained before the transcript is considered settled; a per-tick budget that silently
leaves events queued is acceptable only if the loop guarantees eventual full drain.

### [3] Editor model — `editor.rs::append_llm_chunk` + frozen ranges + TurnId metadata

**Contract.** `append_llm_chunk(turn_tag, chunk)` finds the insertion point (end of the last
line carrying `turn_tag`, or EOF), inserts the chunk via `programmatic_insert` (which bumps
`edit_seq` and shifts frozen ranges, anchors, and metadata), extends the frozen range to
cover exactly the inserted lines, and tags each new line's anchor with `turn_tag`. Editable
user lines elsewhere are untouched (`spec-agent-window.md §E3`).

Live user Submit is the inverse: append the user's text at EOF, freeze it, tag each line
`TurnId::User(k)`.

**INV-5 (frozen-range exactness).** The frozen range after a splice covers every line the
chunk wrote and no line it did not. (Verified correct today: ropey's trailing-empty-line
counting makes the newline-terminated case land on `add_frozen_lines(start, last+1)`.)

**INV-6 (reconstruction parity).** A user turn rebuilt from a replayed `UserMessageChunk`
MUST land in the editor with the same shape a live Submit produces: frozen lines tagged
`TurnId::User(k)`. This is the symmetry that makes stage [4] render replayed and live user
turns identically. There must be a single code path (or two paths proven equivalent) that
turns "user text + turn number" into frozen, tagged transcript lines — used by both live
Submit and replay.

**INV-7 (no transcript persistence dependence).** The transcript editor is *not* persisted
to disk (`acp_sessions.json` holds only id/label/active/mode/pane flags). Therefore the
transcript MUST be fully reconstructible from the replay stream alone. Any content that is
neither replayed (stage [1]) nor locally regenerated is permanently lost on refresh. This is
the invariant defect #2 violates.

### [4] View-model build — `render_agent`, memoized on `view_model_fingerprint`

**Contract.** Build `Vec<FlatItem>` from the editor's lines + frozen ranges + TurnId
metadata + tool anchors + block ranges. Insert a `TurnHeader` at every role change; emit one
`FlatItem::Line` per non-block line; replace block ranges with one `FlatItem::Block`; anchor
`ToolGroup`s at their lines; append `ThinkingIndicator` while awaiting. Then run the
blank-collapse pass.

**INV-8 (memo soundness).** `view_model_fingerprint` MUST change whenever any input to the
flat-item build changes. Today it hashes `edit_seq`, `frozen_line_count`,
`tool_call_order.{len,last}`, the expanded set, and `awaiting_reply`. Because every splice
bumps `edit_seq`, streaming correctly invalidates the memo. Any new input to the build (e.g.
a new role source) MUST be added to the fingerprint, or rows will go stale ("cut off").

**INV-9 (no row loss in collapse).** The blank-collapse pass may only remove lines that are
*whitespace-only*. It must never drop a line with non-whitespace content. The pass strips
blank frozen lines, blank lines adjacent to structural items, and runs of consecutive blank
user lines — all whitespace-only by construction. A collapse that removes a content line is
a defect (candidate cause of defect #1).

**INV-10 (block/line partition).** Every editor line is rendered exactly once: either as the
`Block` that owns its range, or as a standalone `Line`. If `parse_block_range` returns
`None` for a detected range, the range's lines MUST fall back to standalone `Line`s (not be
marked `in_block` and then have no `Block` emitted). A line that is both suppressed as
in-block and never emitted as a Block vanishes — a candidate cause of defect #1, especially
for half-streamed code fences / partial tables.

### [5] Block parse — `detect_block_ranges` / `parse_block_range` (cached on frozen count)

**Contract.** Detect table / fenced-code / heading ranges within frozen content and parse
each into a `RenderedBlock`, cached and only recomputed when `frozen_line_count` changes.

**INV-11 (mid-stream stability).** Block detection must be stable across the streaming of a
block. A code fence that is opened but not yet closed, or a table mid-emission, must render
its already-arrived lines (as Lines or a partial Block) — never suppress them while waiting
for the closing delimiter. Re-detection is keyed on `frozen_line_count`, which advances as
lines freeze, so each in-progress state is re-evaluated; the partition invariant (INV-10)
must hold at every intermediate state.

### [6] List reconcile — `list_state.splice` / `reset`, `list_item_count`

**Contract.** Keep GPUI's `ListState` item count equal to `flat_items.len()` every frame.
Splice incrementally when the list only grew and no blocks are active; `reset` when blocks
are active or the list shrank.

**INV-12 (count parity).** `list_item_count == flat_items.len()` after every render. A
ListState whose count is less than the flat-item count will never paint the trailing items —
a direct cause of defect #1 (tail "cut off"). This reconcile is a per-frame side effect and
MUST run outside the view-model memo (it does today).

### [7] Paint + scroll — `build_wrapped_line`, `scroll_to_reveal_item`

**Contract.** Render only visible items; wrap each `Line` via monospace flex-wrap; apply the
auto-scroll policy: in Chatbox mode follow the tail when `follow_output` is set; in Worksheet
mode follow only when the cursor is at EOF.

**INV-13 (tail visibility under intra-line growth).** Auto-scroll MUST keep the streaming
tail visible even when the chunk extends an *existing* line rather than adding a new one.
Today the pump scrolls every tick with events, but the render-time re-scroll fires only when
`list_item_count` changes (`new_count != old_count`). A chunk that grows the last logical
line without adding a row (common: agent prose before a `\n`) changes the last item's
*height* but not the count, so the render-time re-scroll is skipped and the freshly grown
tail can fall below the fold. This is the leading candidate for the *intermittent* form of
defect #1. The auto-scroll trigger must key on "content grew" (e.g. `edit_seq` advanced with
`follow` active), not solely on "row count grew."

**INV-14 (measurement honesty).** A wrapped line's measured row count (for ListState height)
must match what the renderer actually paints, consistent with the project's source-of-truth
wrap invariant (CLAUDE.md). A height under-measurement of the last item also manifests as a
cut-off tail.

## Data Model

This spec introduces no new persisted structures. It requires one **enum extension** to make
INV-1 satisfiable:

```rust
pub enum ReplyEvent {
    Chunk(String),                  // agent text  → TurnId::Llm
    UserMessage(String),            // NEW: replayed user text → TurnId::User  (INV-1)
    ToolCallStarted(ToolCall),
    ToolCallUpdated(ToolCallUpdate),
    PlanUpdated(Plan),
    ModeChanged(SessionModeId),
    UsageUpdated(UsageSnapshot),    // feature-gated emitter
    Notice(String),                 // existing: sketch-local lifecycle notice (attach/detach/…)
    ReplayComplete,                 // NEW: end-of-replay marker, emitted once after session/load
                                    // returns; gates finalize so an empty queue mid-replay can't
                                    // infer turn-end (INV-4). Ordered after the last replayed chunk.
}
```

The `UserMessage` variant is unconditional in the enum (match-exhaustiveness discipline from
`spec-agent-window.md §31`); the emitter forwards `SessionUpdate::UserMessageChunk`. The pump
(stage [2]) routes it through the shared "freeze as user turn" path (INV-6).

`TurnId`, `LineAnchor`, `LineMetadata<T>`, `FlatItem`, and `AgentState` are all as defined in
`spec-agent-window.md` — unchanged.

## Interfaces

- `AcpChannelClient::try_recv() -> Option<ReplyEvent>` — extended enum; unchanged signature.
- `Editor::append_llm_chunk(turn_tag, chunk)` — used for agent chunks (stage [3]).
- A shared `freeze_as_user_turn(editor, text, turn_k)` (new or factored from Submit) — used
  by both live Submit and replay (INV-6). Single source of truth for user-turn shape.
- `render_agent` — stages [4]–[7]; the only consumer of the view model.

## Constraints

1. **One-directional pipeline.** Stages run in order; no stage reaches back. The channel
   decides visibility (stage [1]); everything downstream can only narrow, never recover.

2. **No transcript persistence.** The transcript is always rebuilt from the replay stream
   (INV-7). This is a deliberate choice inherited from `spec-multi-session.md` — do not add a
   transcript-on-disk cache to paper over a replay-completeness bug; fix stage [1].

3. **Memo correctness over memo coverage.** The view-model memo (stage [4]) is a perf
   optimization; correctness (INV-8) dominates. When in doubt, add the input to the
   fingerprint.

4. **Scroll keys on content growth, not row count.** (INV-13.) The auto-scroll trigger must
   observe intra-line growth.

5. **Live and replay produce identical transcripts.** A conversation viewed live and the
   same conversation after `session/load` MUST render identically (modulo in-flight state
   like the thinking indicator). This is the master invariant; INV-1, INV-3, INV-6 are its
   components.

6. **TUI out of scope.** GPUI agent window only.

## Revision History

- 2026-06-04 — Initial DRAFT. Defines the seven-stage agent rendering pipeline and its
  invariants (INV-1…INV-14), motivated by two observed defects: agent text cut off
  (stages [6]/[7]) and user text missing on refresh (stage [1]). Written to be diffed
  against the codebase; the companion findings note records where reality diverges.
