# Agent Window — Worksheet + Chatbox + Sidebars

> **PARTIALLY SUPERSEDED (2026-06-28).** The **worksheet/chatbox behavioral model**
> here (§4–§20: two co-equal user-toggled input modes) is superseded by
> **`spec-worksheet.md`** + **`docs/ux-invariants.md` INV-UX-9**: the worksheet is
> the inline-editable buffer (You-block on Insert), and the chatbox is the
> **mid-turn-only** input surface — there is no user-selected mode toggle. The
> §9–§15 *inline-edit mechanics* (turn gutter, frozen-line invariants, submit
> freezes editable lines, cursor-anchored auto-scroll §19) remain the correct
> baseline. The data substrate is Model C (ADR-0024). The sidebars (§21–§29),
> status strip (§30), persistence (§35), and editor extensions (§E1–§E4) are
> unaffected.

**Status:** ACTIVE — phases 1–4 shipped. Section-level markers below reflect what landed; the rendering polish around sub-agent transcript swap (§27) and a few corner cases still flagged DRAFT.

**Last updated:** 2026-05-23

## Builds On

- **`spec-multi-session.md`** — Defines the multi-session ring, lifecycle, sidebar, and persistence schema. This spec **inherits the session lifecycle and ring intact** (renamed `SessionRing → AgentRing`, `SessionSlot → AgentSlot`, `ClaudeState → AgentState`), but **supersedes the rendering of an individual session** (§9–§12 of that spec): the per-session chrome and chat-body layout are replaced by the Worksheet / Chatbox / sidebar model defined here.
- **`spec-textbox-compose.md`** — Specified the existing compose-box overlay. This spec **retires it**: the compose box is replaced by the Chatbox input mode, which is one of two co-equal input modalities (the other being the Worksheet) rather than a transient overlay on top of an editable transcript. The `ComposeBox` field on `ClaudeState`, the `ComposeToggle` / `ComposeSend` actions, and the height/separator rendering all go away. The mechanical pieces (standalone `Editor`, append-on-close splice path, scroll suppression) are reused under different names.
- **`spec-workspaces-and-splits.md`** — Defines `WindowContent::Claude(SessionRing)` as one of four window-kinds inside the workspace tab/split tree. This spec renames the variant to `WindowContent::Agent(AgentRing)` and changes its internal rendering; the workspace tree itself is unaffected. Agent-window sidebars live **inside** the agent window's screen rect (not inside the workspace tile structure), so splits and tabs continue to work as before. The cross-view edit-broadcast machinery `spec-workspaces-and-splits.md` §10 yaldaes (DRAFT, unshipped) is **partially inlined** here as the Editor Extensions section — anchors and per-line metadata are needed for the Worksheet gutter and tool-call anchoring regardless of whether the broader cross-view broadcast lands. If both specs ship, they share the same `LineAnchor` infrastructure; if only this one ships, the anchors stay scoped to the agent window.
- **`src/acp_channel.rs`** — Provides `AcpChannelClient`, the ACP transport. Today it forwards `Chunk`, `ToolCallStarted`, `ToolCallUpdated` and drops everything else (`Plan`, `CurrentModeUpdate`, `UsageUpdate`, `AgentThoughtChunk`, `AvailableCommandsUpdate`, `SessionInfoUpdate`, `UserMessageChunk`, `ConfigOptionUpdate`). This spec extends `ReplyEvent` with `PlanUpdated`, `ModeChanged`, and (under the `unstable_session_usage` Cargo feature) `UsageUpdated`. The rest stay dropped — they're listed as future parking-lot items in Behaviors §31.

## Overview

Yalda's agent surface today is a single chat-shaped screen (`ClaudeView`) over an in-memory `Editor` with frozen LLM ranges, a compose-box overlay, and inline tool calls. It works for a single conversation but doesn't scale to the workflow the project is aiming at: yalda as the primary way the user interacts with one or more coding agents (Claude Code today; Codex and other ACP-compatible agents tomorrow), with structured visibility into the agent's plan, sub-agents, model identity, permission mode, and context budget.

This spec defines the **Agent Window**: a unified, agent-agnostic screen that any ACP-attached session renders into. The Agent Window has two co-equal input modes (**Worksheet** and **Chatbox**) the user freely toggles between, two toggleable right-side **sidebars** (Tasklist, Subagents) sourced from ACP signals, and a compact **Status Strip** along the top surfacing the model / permission / token / context-window state.

The spec introduces eight named artifacts:

1. **AgentWindow** — the renamed `ClaudeView` screen. Per-tab, per-split.
2. **AgentRing / AgentSlot / AgentState** — renamed `SessionRing` / `SessionSlot` / `ClaudeState`. Lifecycle from `spec-multi-session.md` is unchanged.
3. **Worksheet** — input mode where the user's lines interleave with LLM lines in the transcript, gated by frozen-line invariants and a left-margin **turn gutter**.
4. **Chatbox** — input mode with a separate `Editor` pinned to the bottom of the window; the transcript above is read-only while in this mode.
5. **TasklistSidebar** — right-side sidebar that mirrors the agent's current `Plan` (from ACP).
6. **SubagentsSidebar** — right-side sidebar that lists sub-agents extracted from tool calls; selecting one swaps the main transcript view in place.
7. **StatusStrip** — single-row header above the main transcript area surfacing agent identity, model, permission mode, context-window utilization, cumulative cost, and turn / elapsed.
8. **SubAgent** — yalda-side classification of a `ToolCall` that represents a sub-agent transcript. Stored on `AgentState`; produced by a heuristic classifier over `ToolCall.kind` and `name`.

## Behaviors

### Window layout

1. **Single layout. [DRAFT]** Every `AgentWindow` renders the same vertical stack:

    ```
    +-----------------------------------------------------------+
    | Status strip                                              |  1 row
    +---------------------------------+----------+--------------+
    | Main transcript area            | Tasklist | Subagents    |
    |                                 |          |              |
    +---------------------------------+----------+--------------+
    | Chatbox (only when mode = Chatbox)                        |  dyn rows
    +-----------------------------------------------------------+
    | Footer                                                    |  1 row
    +-----------------------------------------------------------+
    ```

    Status strip and footer span the full window width. Sidebars sit beside the transcript area but never extend into the status strip or footer. The Chatbox row only exists when the active input mode is Chatbox; in Worksheet mode the transcript IS the editing surface and the row is absent.

2. **Sidebar stacking. [DRAFT]** When multiple sidebars are open they stack horizontally in a fixed order: Tasklist (innermost / closest to the transcript), then Subagents, then any future sidebar. Each occupies a fixed column width (28 chars). The transcript area's width shrinks to accommodate.

3. **Backwards compatibility with `spec-multi-session.md`'s session sidebar. [DRAFT]** The session sidebar described in `spec-multi-session.md` §9 — the LEFT column listing every session — is retained for the case `ring.slots.len() > 1`. With a single session, the left sidebar is hidden (current behavior). The session sidebar is the *workspace*'s navigation between agent sessions; the right sidebars are *one session's* internal columns. The two coexist.

### Mode contract

4. **Two input modes. [DRAFT]** Every `AgentState` carries `mode: InputMode` where `InputMode ∈ { Worksheet, Chatbox }`. Mode is per-session, persisted, and starts at `Chatbox` for a freshly-created session (matches today's compose-box-first feel).

5. **Mode toggle. [DRAFT]** `Ctrl-Alt-Enter` (or menu: `Space → c → i`) flips the mode of the focused agent window. The toggle action is the same in both directions; data movement is asymmetric per §6–§7. (`i` for "input mode" — `m` is already taken in the claude submenu by `claude-mode-cycle` for the *permission* mode per `spec-multi-session.md` §14.)

6. **Chatbox → Worksheet. [DRAFT]** Take whatever text is in the chatbox (may be empty, may be multi-line), append it at EOF of the transcript as new **editable** user lines (one transcript line per chatbox line). Cursor lands at end of those lines. Drop the chatbox `Editor`. Set `mode = Worksheet`. If the chatbox was empty, no lines are added.

7. **Worksheet → Chatbox. [DRAFT]** Do not touch the transcript at all. Construct a fresh empty chatbox `Editor`. Cursor moves into it. Set `mode = Chatbox`. The transcript's editable user lines stay in place — they're still pending and will be swept on the next Submit.

    *Undo across mode switches:* the chatbox's undo history is per-`Editor`, and the chatbox is dropped on Chatbox → Worksheet. A subsequent Worksheet → Chatbox yields a fresh empty chatbox with empty undo history. The previous draft is recoverable as transcript content; transcript-side undo unwinds the "append at EOF" insert.

8. **Submit is never bare Enter. [DRAFT]** Submit is `Ctrl-Enter` (or menu: `Space → c → s`). Bare Enter in either mode inserts a literal newline. Bare Enter inside a frozen line in the worksheet is a no-op (see §13).

### Worksheet

9. **Frozen LLM lines. [DRAFT]** Every line of LLM output is a frozen line in the transcript `Editor`. The invariants:

    - **No edits inside a frozen line.** Inserts, deletes, and backspace targeted into a frozen line are rejected.
    - **No splits.** `Enter` while the cursor is on a frozen line is a no-op.
    - **No merges.** Backspace at the start of a frozen line is a no-op (the line above does not absorb).
    - **Cursor motion** through frozen lines, **selection** spanning frozen lines, and **yank / copy** from frozen lines all work normally.

10. **Editable insertion points. [DRAFT]** The cursor can be placed at the start of any line. If the line is frozen, typing is rejected; if the line is the user's own (or a blank line not yet claimed by an LLM turn), typing is allowed. Hitting `Enter` on a user line creates a new user line; doing it at a frozen-line boundary inserts a fresh user line in between the surrounding frozen blocks.

11. **Turn gutter. [DRAFT]** The left margin of the worksheet shows a per-line tag:

    | Tag | Meaning |
    |---|---|
    | `N` | LLM output, turn N (dim accent color) |
    | `Un` | User input frozen as part of turn n's prompt |
    | (blank) | Currently-editable user input, not yet submitted |
    | `Tn` | Tool-call block originating from turn N |

    The gutter occupies a fixed 4-char column to the left of the line content. It is read-only. All four tags resolve from the same source: `editor.metadata::<TurnId>().get(anchor_at(line))` (§E2). `Tn` lines specifically are the anchor lines of `FlatItem::ToolGroup` entries — the tool-call's originating turn is read off the metadata of its anchor line, so the `T` prefix's number stays correct even as the user inserts annotations above or below.

12. **Submit semantics. [DRAFT]** Submit in Worksheet mode:

    1. Walk the transcript in document order, collecting every editable (non-frozen, non-tool-call) line.
    2. Build the **prompt body** by concatenating every collected line that has at least one non-whitespace character, with `\n` separators. Whitespace-only lines are excluded from the prompt body but **not** from the freeze pass.
    3. If the prompt body is empty (no collected lines had any non-whitespace content), no-op with footer hint `nothing to send`. No freezing happens.
    4. Send the prompt body via `AcpChannelClient::send`.
    5. **Freeze every collected line** — including whitespace-only spacers — as part of the just-sent turn's user contribution. Each line's metadata becomes `TurnId::User(k)` where `k` is the new turn number, surfacing as gutter tag `Uk`. The lines stay in their existing positions in the document.
    6. After submit, no editable lines remain. The next LLM reply streams in as turn `k`'s LLM range, gutter `k`.

    This split (prompt skips blanks, freeze includes blanks) means a deliberate blank spacer between two LLM turns survives in the transcript as a visible structural break without polluting the prompt the agent sees.

13. **No re-submit drift. [DRAFT]** Because every editable line at submit time gets frozen, there is no accumulating-state problem where a stale annotation gets resent on every subsequent prompt. The editable set is always exactly "what the user has typed since the last submit." This is the key constraint that makes the Worksheet a stateful prompt board rather than a chat log with floating annotations.

14. **Inter-turn annotation. [DRAFT]** The user may place the cursor between two frozen blocks anywhere in the document and add new editable lines there. They will be swept and frozen by the next Submit, gutter-tagged with the turn they were sent in (not their visual position). Reading the transcript later, the gutter shows the chronological "when was this said" even when the visual order is non-chronological.

15. **Deletion. [DRAFT]** Once frozen, lines cannot be deleted. The user edits or removes them only *before* submitting. A future `:retract` action could rewrite a frozen line into a ghost (struck-through, gutter-tagged retracted) but is out of scope here.

### Chatbox

16. **Separate Editor. [DRAFT]** When `mode = Chatbox`, `AgentState.chatbox: Option<Editor>` holds a standalone editor (its own document, cursor, undo stack, modal state). The transcript editor is unaffected.

17. **Transcript read-only in Chatbox mode. [DRAFT]** No cursor renders in the transcript area. Mouse / `Ctrl-Up` / `Ctrl-Down` / `PageUp` / `PageDown` scrolls the transcript viewport. Typing routes to the chatbox.

18. **Submit semantics. [DRAFT]** Submit in Chatbox mode:

    1. Take the full chatbox text.
    2. If empty after trimming, no-op with footer hint `nothing to send`.
    3. Append the chatbox text at EOF of the transcript as new editable lines, **then immediately freeze them** as part of the just-sent turn's user contribution (gutter `Uk`).
    4. Send the prompt via `AcpChannelClient::send`.
    5. Clear the chatbox to empty. Cursor remains in the chatbox. Mode stays `Chatbox`.

19. **Auto-scroll. [DRAFT]** In Chatbox mode, new LLM chunks auto-scroll the transcript to the bottom edge (because the user's cursor isn't there to be disoriented). Standard "sticky bottom" rule: if the user has scrolled up off the bottom, suppress auto-scroll until they scroll back to the bottom. In Worksheet mode, the viewport stays anchored to the **user's cursor**, not to streaming output: if the cursor is on a user-line in the middle of the document, the viewport never jumps to follow chunks landing elsewhere. The one exception is the EOF special case — if the cursor is on the last line of the document at the moment a chunk lands, the viewport follows (sticky-bottom for cursor-at-EOF). Mental model: the worksheet is a paper sheet; the agent is writing on the far end; the user is editing where they are.

20. **Chatbox height. [DRAFT]** Starts at 3 rows. Grows with content up to `min(viewport_height / 3, 12)` rows. Above the cap, the chatbox scrolls internally. (Inherited from `spec-textbox-compose.md`'s height policy.)

### Tasklist sidebar

21. **Source: `Plan` event. [DRAFT]** ACP's `SessionUpdate::Plan` carries `Plan { entries: Vec<PlanEntry> }`. The protocol contract is that each `Plan` is a **full snapshot** that replaces the previous one. `acp_channel.rs` is extended to forward this as `ReplyEvent::PlanUpdated(Plan)`. `AgentState.current_plan: Option<Plan>` stores the latest snapshot.

22. **Rendering. [DRAFT]** When the Tasklist tile is open and `current_plan` is `Some` with `!entries.is_empty()`:

    ```
    +-----------------+
    | Plan            |
    +-----------------+
    | ●  rewrite ed…  |
    | ○  split rope   |
    | ✓  add tests    |
    | ✓  benchmarks   |
    +-----------------+
    ```

    Indicators by `PlanEntryStatus`: `●` in_progress, `○` pending, `✓` completed. If a future status `failed` is added by the protocol, `✗` is used. `PlanEntryPriority`: `High` gets a leading red bar in the indicator column; `Low` dims the line by one shade; `Medium` is the default. Long entry text truncates with `…`; full content surfaces in a tooltip on hover.

23. **Empty state. [DRAFT]** When the tile is open but `current_plan` is `None` or `entries.is_empty()`, the tile renders the placeholder `(no plan)` in dim text.

24. **Read-only in v1. [DRAFT]** Clicking an entry is a no-op. The user manipulates the plan only by instructing the agent. Marking entries done from the UI is out of scope.

### Subagents sidebar

25. **Detection. [DRAFT]** A `SubAgent` is a yalda-side classification of a `ToolCall`. A centralized function `classify_subagent(tc: &ToolCall) -> Option<SubAgent>` runs whenever a `ToolCallStarted` or `ToolCallUpdated` event lands. v1 heuristic: `tc.kind == ToolKind::Other` AND `tc.name` matches any prefix in the const slice `SUBAGENT_TOOL_NAMES = &["Task", "Subagent", "Spawn"]`. The slice is the single point of swap when ACP adds a structured sub-agent type or vendor tools rename. **Classification is flat, not tree-shaped:** when a sub-agent's own tool-call content emits further `ToolCall` events, each one is fed through `classify_subagent` and produces its own top-level entry in `AgentState.subagents`. The sub-agent list is a chronologically-ordered flat list; "sub-sub-agents" are just additional rows.

26. **Storage. [DRAFT]** `AgentState.subagents: Vec<SubAgent>` collects all classified sub-agents, ordered by first-seen. Each `SubAgent` carries:

    ```rust
    struct SubAgent {
        tool_call_id: String,           // the originating tool call's id
        label: String,                  // best-effort: tc.title or tc.name, fallback "subagent-N"
        status: ToolCallStatus,         // mirrors the underlying tool call
        transcript: Vec<ToolCallContent>,   // accumulated content blocks
    }
    ```

    `transcript` reuses the same per-payload cap as main-transcript tool calls (`cap_tool_call_payloads`, `main.rs:6257`). Each content block is truncated to that cap before storage; the sub-agent's total memory budget tracks the main store's. No additional aggregate cap in v1 — if profiling shows a long-running session with chatty sub-agents balloons, the cap can be tightened in one place.

27. **Focus swap. [DRAFT]** Clicking a sub-agent entry (or pressing `Enter` after keyboard-navigating to it) sets `AgentState.focused_subagent: Option<usize>` to its index. While focused:

    - The main transcript area renders the sub-agent's transcript instead of the root agent's.
    - The Worksheet / Chatbox input mode for the parent is preserved but inputs are blocked: typing into the Chatbox or attempting Submit shows the footer hint `can't talk to a sub-agent directly`. (Two-way sub-agent interaction is a future extension.)
    - The Status Strip gains a breadcrumb `agent / refactor-pass ◂`. The model / permission / tokens fields stay sourced from the root agent (sub-agents share the parent's session).
    - `Esc`, clicking the `◂` chip in the breadcrumb, or `Cmd-[` returns to the parent transcript by setting `focused_subagent = None`. Note: `Esc` only fires this when a sub-agent is currently focused; in all other states it remains a no-op per the project-wide "Esc never quits / closes" rule.

28. **Indicators. [DRAFT]** Each row shows `▸ <status-glyph> <label>`. The currently-focused sub-agent gets a highlighted row background.

29. **Empty state. [DRAFT]** `(no subagents)` placeholder in dim text.

### Status Strip

30. **Layout and sourcing. [DRAFT]** A single row above the main transcript area:

    ```
     claude-1  ⏵ refactor-pass ◂   sonnet-4-7   auto-edit   12.3k / 200k (6%)   $0.18   turn 4 · 0:14
    ```

    | Field | Source | Notes |
    |---|---|---|
    | agent label | `AgentSlot.label` | clickable → rename overlay |
    | sub-agent breadcrumb | `AgentState.focused_subagent` | only shown when focused; `◂` chip returns |
    | model id | `CurrentModeUpdate` / `ConfigOptionUpdate`, fallback `AcpChannelClient::description()` | new ACP forwarding |
    | permission mode | `AcpChannelClient::permission_mode()` | clickable → cycle |
    | context-window usage | `UsageUpdate` (Cargo feature `unstable_session_usage`) | hidden if absent |
    | cumulative cost | `UsageUpdate.cost_usd` | hidden if absent |
    | turn / elapsed | `AgentState.last_seen_turns` + `turn_started` | existing |

    Any field whose underlying signal is absent renders **nothing** — no placeholder, no `?`. The strip is at most as wide as the data it has.

### Open ACP signals (parking lot)

31. **Surfaced in v1**: `Plan`, `CurrentModeUpdate`, `UsageUpdate` (feature-gated emitter; see below). **Still dropped at the channel layer** (out of v1 scope): `AgentThoughtChunk` (reasoning stream — future "thinking" indicator), `AvailableCommandsUpdate` (slash command registry — future chatbox typeahead), `SessionInfoUpdate` (title metadata — could auto-populate label), `ConfigOptionUpdate`, `UserMessageChunk` (echo). The notification handler in `acp_channel.rs` keeps explicit arms for these (matched but dropped) so adding any one is a one-arm change.

    **Feature-gating discipline:** the `ReplyEvent::UsageUpdated` variant is **unconditional** in the enum — no `#[cfg]` on the variant itself. Only the *emitter* in `acp_channel.rs`'s notification handler is `#[cfg(feature = "unstable_session_usage")]`-gated. This keeps every `match ev { … }` site exhaustive at all times: consumers always handle `UsageUpdated`, the variant simply never fires when the feature is off. Same shape as today's `Chunk` / `ToolCallStarted` / `ToolCallUpdated` handling. Applies to all future ACP-feature-gated variants too.

### Key dispatch

32. **Agent-window-scoped keys. [DRAFT]** Active when `WindowContent::Agent(_)` is the focused leaf:

    | Action | Binding |
    |---|---|
    | Submit | `Ctrl-Enter` |
    | Toggle Worksheet ↔ Chatbox | `Ctrl-Alt-Enter` |
    | Toggle Tasklist sidebar | `Cmd-1` |
    | Toggle Subagents sidebar | `Cmd-2` |
    | Return from sub-agent focus to parent | `Esc` (only fires when focused) |
    | Scroll transcript (in Chatbox mode) | `Ctrl-Up` / `Ctrl-Down` / `PageUp` / `PageDown` |

33. **Inherited session keys. [SHIPPED]** Session lifecycle bindings from `spec-multi-session.md` §13 (`Ctrl-]` / `Ctrl-[` next/prev session, plus the `Space → c → *` menu) are unchanged.

34. **Retired keys. [DRAFT]** `Ctrl-T` (today's `ComposeToggle`) is removed. The compose-box-specific menu entries (`o` open chat, `t` compose) are retained semantically but rewired to the new mode-toggle action.

### Persistence

35. **AgentState extensions to `acp_sessions.json`. [DRAFT]** Each persisted slot gains three optional fields on top of today's `{id, label, active}`:

    ```json
    {
      "id": "ses_abc123",
      "label": "claude-1",
      "active": true,
      "mode": "worksheet",
      "tasklist_open": true,
      "subagents_open": false
    }
    ```

    Defaults for missing fields: `mode: "chatbox"`, `tasklist_open: false`, `subagents_open: false`. Loader treats absence as default. Saver always writes the new shape.

    **Downgrade compatibility.** An older yalda binary loading a newly-written file deserializes the per-slot record with serde's standard "ignore unknown fields" behavior — `id`, `label`, `active` are read; `mode`, `tasklist_open`, `subagents_open` are silently dropped. The downgraded session re-opens at the old defaults (no worksheet mode, no sidebars). No persisted session is lost. This is the intended downgrade contract.

36. **Not persisted. [DRAFT]**

    - **Chatbox unsent text.** Consistent with `spec-multi-session.md` Constraint §7.
    - **Sub-agent focus.** Restart returns to the root agent transcript regardless of where the user was when they quit.
    - **`current_plan`, `usage`, `agent_mode`, `subagents`.** All re-derived from agent events after `session/load`.
    - **Worksheet transcript content.** Same as today — the in-memory `Editor` is reconstructed by the agent on `session/load` (resume replays the conversation). Frozen-line ranges and gutter tags are rederived from the replayed stream.

## Editor Extensions

The Worksheet contract (§9–§15) and the tool-call anchor (§Data Model — `tool_call_anchor_line`) demand line-level invariants that today's `Editor` does not satisfy: identity that survives edits elsewhere in the document, and per-line metadata that auto-shifts with inserts and deletes. Three additions to `src/editor.rs`:

E1. **`LineAnchor`. [DRAFT]** Opaque monotonic id that names a specific line and survives inserts and deletes happening on other lines. The editor maintains a side `BTreeMap<LineAnchor, LineIdx>` updated by the same paths that already shift `frozen_lines` (`shift_frozen_lines_for_insert` / `_for_delete`). Anchors for lines that get removed by a delete are dropped from the map. Public API:

   - `editor.anchor_for_line(idx: usize) -> LineAnchor` — allocate or return the anchor for `idx`.
   - `editor.line_for_anchor(a: LineAnchor) -> Option<usize>` — `None` once the line is gone.

   All tool-call anchoring (today: `tool_call_anchor_line: HashMap<String, usize>` — `main.rs:2970`) switches to `HashMap<String, LineAnchor>`. The renderer reads `line_for_anchor` once per tool call per paint; a `None` means the anchor's line was deleted and the tool block renders at EOF as a fallback.

E2. **`LineMetadata<T>`. [DRAFT]** A typed sparse map from `LineAnchor` to a payload `T`. The editor owns one slot per metadata type registered with it. Stored as `HashMap<TypeId, HashMap<LineAnchor, Box<dyn Any>>>` (or one strongly-typed field per known consumer; either implementation is fine — the spec just requires the read/write API). Public API:

   - `editor.metadata::<T>().get(a: LineAnchor) -> Option<&T>`
   - `editor.metadata_mut::<T>().insert(a: LineAnchor, v: T)`

   Replaces this spec's previous `line_turn_ids: Vec<TurnId>` parallel-array (B3 in the adversarial review). The Worksheet gutter reads `editor.metadata::<TurnId>().get(anchor_at(line)).unwrap_or(TurnId::Unsubmitted)` for each visible line. There is no separate "gutter Vec" to keep in sync — the editor's anchor-shift machinery already keeps the metadata correct across streaming, splice, undo, and inter-block insertion.

E3. **New LLM-chunk splice contract. [DRAFT]** Today's `splice_claude_chunk` finds `splice_at = max(lockable_through_char, end_of_last_frozen)`, strips the tail `splice_at..total_len`, appends the chunk, and reattaches the stripped tail. That algorithm only works when there is exactly one editable region (at EOF). It is **replaced** by `append_llm_chunk(turn_id: TurnId, chunk: &str)`:

   1. Locate the **insertion point**: the line immediately after the last frozen line whose metadata `TurnId::Llm(n)` matches `turn_id`. If no such line exists (a new turn), the insertion point is end-of-document.
   2. Insert the chunk's text at the insertion point. The editor's `programmatic_insert` already shifts frozen ranges, anchors, and metadata correctly across the insertion.
   3. Freeze the newly inserted lines (extend the relevant frozen range).
   4. Tag each newly inserted line's anchor with metadata `TurnId::Llm(turn_id)` via `metadata_mut::<TurnId>().insert(...)`.

   Editable user lines — both trailing draft and inter-block annotations — are **not touched**. The strip-and-reattach dance is gone. Submit's freeze pass (§12) is the inverse: walk every editable line in document order, allocate `TurnId::User(k)` metadata for each, and extend the frozen range to cover them.

E4. **Out of scope here. [DRAFT]** Cross-view broadcast (multiple `EditorView`s sharing one `EditorCore` and getting position-shift events; `spec-workspaces-and-splits.md` §10) is **not specified** here. The agent window is a single view of its own transcript editor. If tabs-and-splits ships cross-view broadcast later, the `LineAnchor` / `LineMetadata` infrastructure described above is the substrate it builds on.

## Data Model

### Renames

| Old (today) | New (this spec) |
|---|---|
| `WindowContent::Claude(SessionRing)` | `WindowContent::Agent(AgentRing)` |
| `ClaudeState` | `AgentState` |
| `SessionRing` | `AgentRing` |
| `SessionSlot` | `AgentSlot` |
| `ClaudeView` (key context) | `AgentView` |
| File `bin/yalda-gpui/main.rs` Claude-named items | Agent-named items |

Persistence file name, ACP channel module, `AcpChannelClient`, and menu command strings (`claude-new`, `claude-close`, …) stay unchanged so saved `keymap.kdl` and `acp_sessions.json` continue to load without migration.

### AgentState

```rust
struct AgentState {
    // Existing (carried over from ClaudeState, unchanged unless noted):
    editor: Editor,                          // transcript editor with frozen ranges
    channel: Option<AcpChannelClient>,
    attach_pending: Option<...>,
    list_state: gpui::ListState,
    list_item_count: usize,
    status: Option<SharedString>,
    awaiting_reply: bool,
    turn_started: Option<std::time::Instant>,
    last_seen_turns: usize,
    tool_calls: HashMap<String, ToolCall>,
    tool_call_order: Vec<String>,
    tool_call_anchor_line: HashMap<String, usize>,
    expanded_tool_calls: HashSet<String>,
    block_ranges: Vec<(usize, usize)>,
    block_cache: HashMap<(usize, usize), RenderedBlock>,
    block_cache_frozen_count: usize,
    _pump: Option<Task<()>>,

    // Removed (the old compose-box overlay):
    // compose_box: Option<ComposeBox>,        ← retired; superseded by chatbox

    // New:
    mode: InputMode,                         // Worksheet | Chatbox
    chatbox: Option<Editor>,                 // Some iff mode == Chatbox
    // Per-line TurnId is NOT a parallel Vec — it lives in the editor's
    // LineMetadata<TurnId> slot (§E2), keyed by LineAnchor so it auto-
    // shifts across streaming/splice/undo/inter-block edits. No field on
    // AgentState; read via editor.metadata::<TurnId>().
    current_plan: Option<acp::Plan>,         // last-seen plan snapshot
    agent_mode: Option<acp::SessionModeId>,  // last-seen mode update
    usage: Option<UsageSnapshot>,            // last-seen usage update (feature-gated)
    subagents: Vec<SubAgent>,                // classified tool calls
    focused_subagent: Option<usize>,         // index into subagents
    tasklist_open: bool,
    subagents_open: bool,
    keybinds: KeybindManager,                // already existed; unchanged
}
```

`TurnId` is a simple `enum { Llm(usize), User(usize), Tool(usize), Unsubmitted }` where the `usize` is the turn number. `UsageSnapshot` is yalda-side flattening of whatever `UsageUpdate` carries (tokens used, tokens total, cost USD). `tool_call_anchor_line` (carried over from `ClaudeState`) changes shape: `HashMap<String, LineAnchor>` instead of `HashMap<String, usize>` — see §E1.

### ReplyEvent extension

```rust
pub enum ReplyEvent {
    Chunk(String),                                     // existing
    ToolCallStarted(ToolCall),                         // existing
    ToolCallUpdated(ToolCallUpdate),                   // existing
    PlanUpdated(Plan),                                 // new
    ModeChanged(SessionModeId),                        // new
    UsageUpdated(UsageSnapshot),                       // new, feature-gated
}
```

### Persisted shape

```json
{
  "/Users/scott/ws/yalda": [
    {
      "id": "ses_abc123",
      "label": "claude-1",
      "active": true,
      "mode": "worksheet",
      "tasklist_open": true,
      "subagents_open": false
    }
  ]
}
```

Top-level shape is unchanged from `spec-multi-session.md` §15. Three new optional fields per slot.

## Interfaces

### Channel (extended)

- `AcpChannelClient::try_recv` returns the extended `ReplyEvent` enum.
- New variants are dropped silently by callers that don't handle them (the agent window is the only consumer today).

### AgentState (new methods on top of carried-over `ClaudeState`)

- `AgentState::set_mode(mode: InputMode)` — performs the data movement per §6–§7.
- `AgentState::submit(channel: &mut AcpChannelClient)` — runs §12 (Worksheet) or §18 (Chatbox) depending on `mode`.
- `AgentState::on_reply_event(ev: ReplyEvent)` — drives `current_plan`, `agent_mode`, `usage`, `subagents`, plus existing chunk/tool-call handling.
- `AgentState::toggle_tasklist()` / `toggle_subagents()` — flip tile visibility, save.
- `AgentState::focus_subagent(idx: usize)` / `unfocus_subagent()` — §27.
- `AgentState::line_turn_id(line_idx: usize) -> TurnId` — gutter source.

### Sub-agent classifier

- `classify_subagent(tc: &ToolCall) -> Option<SubAgent>` — single function, centralizes the v1 heuristic. Called from `on_reply_event` for both `ToolCallStarted` and `ToolCallUpdated`.

### Sidebar pointer events

Pointer handling for both the Tasklist tile (entry hover-tooltip) and the Subagents tile (entry click → focus) uses the existing GPUI click handler shape: `on_mouse_down(MouseButton::Left, cx.listener(move |view, ev, w, cx| …))`, with a `WeakEntity<YaldaGpuiView>` captured for any handler that needs to mutate view state outside the immediate listener scope. Same shape as the existing session-sidebar click handler at `src/bin/yalda-gpui/main.rs:7670`. No new GPUI primitives required.

### Persistence functions (extended, not renamed)

- `save_persisted_acp_sessions(cwd, ring)` — writes the new fields for every slot.
- `load_persisted_acp_sessions(cwd) -> Vec<PersistedSlot>` — populates the new fields with defaults when absent.

## Constraints

1. **No nested agent windows.** A sub-agent is rendered inline by swapping the main transcript; it never becomes its own `WindowContent::Agent` leaf. This keeps the workspace tree free of agent-internal hierarchy and matches the "swap in place" UX the user chose.

2. **Worksheet edits are local-only.** The agent never sees the user's worksheet annotations except via Submit. There is no live "the user is editing" notification to the agent.

3. **No retroactive freeze.** Once a line is editable, it stays editable until a Submit sweeps it. The agent cannot demand that a user line be frozen.

4. **No edit of frozen content from the UI.** Frozen lines (LLM output, prior-turn user prompts, tool blocks) are immutable. The `:retract` and `:rewrite` ideas in §15 are future work.

5. **Sub-agent transcripts are read-only.** v1 does not support replying into a sub-agent. Inputs typed while focused on a sub-agent show a footer hint.

6. **Sidebar width is fixed.** 28 chars per sidebar. No resize handle in v1. The transcript area shrinks to accommodate. The auto-close threshold computes available transcript width as `window_width - workspace_tab_strip_width (160px when present) - sum_of_open_sidebar_widths`; if the result drops below the equivalent of 40 chars at the current font size, the rightmost open sidebar closes with a footer hint. Subsequent re-widening does not auto-reopen — the user re-toggles manually.

7. **TUI scope.** This spec covers the GPUI frontend only. The TUI's agent integration (`src/app/claude.rs`) is unaffected and continues to use today's compose-box / single-screen model. Reconciling the TUI is a future spec.

8. **`unstable_session_usage` feature gate.** Token / cost surfacing depends on a Cargo feature in the ACP crate that's off by default. The Status Strip's tokens and cost fields render nothing until the feature is enabled — no placeholder, no `?`. Enabling is a one-line change in `Cargo.toml` if the user wants the data; this spec doesn't enable it because the upstream feature is still marked unstable.

   *Mid-session model staleness.* The Status Strip's `model id` is best-effort. Agents that change models mid-session without emitting a `CurrentModeUpdate` (e.g., a `/model` slash command in Claude Code that the agent runs locally without notifying the client) will show a stale model id until the next attach. Surfacing `AvailableCommandsUpdate` and slash-command outputs is parking-lot per §31.

9. **No protocol-level fan-out across agents.** Multi-agent (Claude + Codex side by side) is a workspace-level concern: the user opens two agent windows, each with its own `AgentRing`. There is no shared state between rings. Agents do not communicate with each other.

10. **One transcript per session.** Despite the dual input modes, there's only ever one transcript per `AgentState`. Worksheet and Chatbox both write into the same `editor`. The Chatbox is an input staging area, not a separate conversation.

## Revision History

- 2026-05-23 — Phases 1–4 landed. Editor extensions (§E1–§E3), Claude→Agent rename, ACP signal forwarding (§31), worksheet/chatbox modes (§4–§20), turn gutter (§11), Tasklist + Subagents sidebars (§21–§29), Status Strip (§30), persistence schema extension (§35), and cursor-anchored auto-scroll (§19) all shipped. Per-window cwd is parked as a sibling spec for follow-up. Sub-agent transcript swap (§27) is wired up to the focus-state field but the main-area swap rendering hasn't been built yet — clicking a sub-agent today highlights the row and primes the field; the transcript renderer continues to show the parent agent. Spec §27's footer-hint and Cmd-[ return path are also unbuilt.
- 2026-05-22 (3) — Status bumped DRAFT → ACTIVE after the user approved the rev-2 revisions. Section-level markers stay DRAFT until each piece ships.
- 2026-05-22 (2) — Adversarial review pass. Three blocking concerns from the reviewer addressed by introducing a new **Editor Extensions** section (`§E1`–`§E4`): opaque `LineAnchor` ids replace raw line-index anchors for tool calls (fixes the "anchor doesn't shift when user inserts above" bug); typed `LineMetadata<T>` sparse map replaces the proposed `line_turn_ids: Vec<TurnId>` parallel array (anchor-keyed, auto-shifts with the same machinery as `frozen_lines`); the LLM-chunk splice is replaced with `append_llm_chunk(turn_id, chunk)`, an insert-and-tag that leaves trailing draft AND inter-block annotations untouched. Worksheet submit (§12) now distinguishes prompt-body (skips whitespace-only lines) from freeze-pass (includes them) so blank spacers survive in the transcript without polluting the wire prompt. Worksheet auto-scroll (§19) now anchors to the user's cursor, not to streaming output (sticky-bottom only when cursor is at EOF). Sub-agent classifier (§25) hardens the heuristic to a const slice of name prefixes and specifies flat (not tree) classification for nested tool calls. Sub-agent transcripts (§26) reuse the existing `cap_tool_call_payloads` per-payload cap. `ReplyEvent::UsageUpdated` (§31) is now unconditional in the enum with a feature-gated *emitter* so match-exhaustiveness stays clean across feature settings. Persistence (§35) adds an explicit downgrade-compat note. Constraint §6 expands the auto-close threshold to account for the workspace tab strip's width. Constraint §8 adds a mid-session model-staleness note for agents that change models without emitting `CurrentModeUpdate`. Menu chord for the input-mode toggle (§5) moved from `m` (collides with `claude-mode-cycle` for permission mode) to `i` ("input mode"). Mode-switch undo behavior (§7) and `Tn` gutter source (§11) clarified inline. New Interfaces entry for sidebar pointer events citing the existing session-sidebar handler pattern.
- 2026-05-22 — Initial DRAFT. Replaces the ad-hoc Claude-only screen with the unified Agent Window model. Worksheet contract (§9–§15), Chatbox contract (§16–§20), Tasklist sidebar (§21–§24), Subagents sidebar (§25–§29), Status Strip (§30), and ACP signal extensions (§31, §35) all DRAFT. Rename pass (Claude → Agent) DRAFT. `spec-textbox-compose.md` to be marked SUPERSEDED once this lands.
