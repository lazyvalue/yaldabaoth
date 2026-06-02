# Agent Pane Presentation System

**Status:** DRAFT

**Last updated:** 2026-06-01

## Builds On

- **`spec-agent-window.md`** — Defines the structural layout of the agent pane (status strip, transcript, chatbox, sidepanes, footer) and the behavioral contracts (worksheet vs chatbox modes, submit semantics, sidepane toggling). This spec **does not redefine structure**; it defines the visual presentation of every element within that structure: spacing, typography, color, borders, and inter-element adjacency rules. WHERE things go is the agent-window spec; HOW they look is this spec.

## Overview

The agent pane renders a conversation between the user and an ACP-attached agent. The visual presentation must solve three problems simultaneously: (1) distinguish user-authored content from agent-authored content at a glance, (2) maintain consistent spacing rhythm across heterogeneous element types, and (3) provide a clear visual hierarchy from chrome (status strip, footer) through structural markers (turn headers) down to content (text lines, tool cards, blocks).

The system is organized in three layers:

1. **Spacing grid** — A small set of named spacing constants derived from a base unit. Every margin, padding, and gap in the agent pane is one of these constants.
2. **Element definitions** — Seven element types, each with intrinsic properties (typography, background, border, internal padding) expressed in terms of the grid.
3. **Adjacency rules** — A matrix defining the vertical gap between every pair of adjacent element types in the transcript. The rule for a pair always resolves to one grid constant.

Additionally, five **chrome regions** (status strip, info bar, footer, chatbox, sidepanes) have fixed presentation rules that don't participate in the adjacency matrix — they're separated from the transcript by hard boundaries (borders, background changes), not by spacing.

### Named artifacts

- **SpacingGrid** — the constant table (§Grid).
- **TranscriptLine** — a single line of user or agent text in the transcript.
- **ToolCard** — a collapsible card representing one or more tool calls.
- **TurnHeader** — the "Claude" / "You" divider between turns.
- **ContentBlock** — a rendered markdown block (heading, table, code fence, etc.) parsed from frozen agent output.
- **ThinkingIndicator** — the pulsing "Thinking..." element shown while awaiting a reply.
- **Gutter** — the left-margin column showing turn labels and the author bar.
- **StatusStrip** — the top chrome bar (model, mode, tokens, cost, timer).
- **InfoBar** — secondary chrome bar showing context window, cwd, and active subagents.
- **Footer** — the bottom chrome bar (mode label, cursor position, keybinding hints).
- **Chatbox** — the compose input area pinned below the transcript.
- **Sidepane** — the fixed-width right-side panels (Tasklist, Subagents).

## Behaviors

### Spacing grid

The grid defines six named constants. Every spacing value in the agent pane is one of these.

| Constant | Value | Usage |
|---|---|---|
| `none` | 0px | No gap (e.g., consecutive lines within the same turn) |
| `xs` | 1px | Line-internal vertical padding (above and below each transcript line) |
| `sm` | 4px | Tight interior padding (gutter label padding, pane entry rows, footer vertical padding) |
| `md` | 8px | Standard gap (tool card margin, chatbox vertical padding, bar-to-content gap, element-to-element within a turn) |
| `lg` | 16px | Section-level inset (tool card horizontal margin, chrome horizontal padding, sidepane horizontal padding) |
| `xl` | 24px | Major structural break (turn-to-turn gap, transcript body horizontal padding) |

One derived constant:

| Constant | Value | Usage |
|---|---|---|
| `turn_gap` | 32px | Top padding on TurnHeader — the dominant rhythm-setting gap between turns. Not a clean grid multiple; tuned by eye for the visual weight of the label + rule. |

All `px_N()` shorthands in GPUI map to `N * 4px` (e.g., `px_2()` = 8px = `md`, `px_4()` = 16px = `lg`, `px_6()` = 24px = `xl`). The grid aligns with this scale.

### Element definitions

#### TranscriptLine

A single line of text in the transcript, rendered in monospace. User-authored lines and agent-authored lines are the same element type with different color treatments.

| Property | Value |
|---|---|
| **Vertical padding** | 2px above and below |
| **Background** | `agent_turn_bg` for `TurnId::Llm` lines, `user_turn_bg` for `TurnId::User` lines, **transparent** for `TurnId::Tool` lines and `None` (unsubmitted). Tool-anchor lines float on `editor_bg` because they're structural (gutter shows `Tn`), not authored content — Constraint 6 applies. |
| **Text font** | `code_font` (monospace) |
| **Text size** | 13px (inherited from body container) |
| **Text color** | Agent lines: `frozen_fg`. User lines: `editor_fg`. |
| **Text tint** | Base-style tokens tinted with `agent_tint` (agent) or `user_tint` (user) |
| **Cursor line** | Background overridden to `dim` at 20% opacity |
| **Selection** | Background highlight at `selection_bg` on selected char ranges. **Known violation of Constraint 2:** currently a hardcoded constant (`SELECTION_BG`, Dracula-specific). Needs migration to an `AgentTheme` slot with per-theme values. |

Lines use `build_wrapped_line` for monospace flex-wrap layout. Empty lines render a single space token to maintain line height.

#### Gutter

The left-margin column rendered as part of each TranscriptLine. Three sub-elements in a fixed horizontal layout:

| Sub-element | Width | Properties |
|---|---|---|
| **Turn label** | 28px fixed | 10px text, `code_font`, right-padded `sm` (4px). Shows `N` (agent turn), `Un` (user turn), `Tn` (tool anchor), or blank. Color: `frozen_bar` for agent, `user_bar` for user, `tool_label` for tool, `dim` for blank. Label only shown on the first line of each contiguous turn block. |
| **Author bar** | 3px fixed | Solid color block. `frozen_bar` for agent lines, `user_bar` for user lines with content, transparent for empty/unsubmitted lines. Right margin `md` (8px). |
| **Content** | flex-1 | The TranscriptLine content, filling remaining width. |

The gutter is the primary mechanism for distinguishing user from agent content. The author bar provides a continuous vertical color band; the turn label provides the turn number.

#### ToolCard

A collapsible card containing one or more tool calls. Rendered as a distinct visual container, inset from the transcript edges.

| Property | Value |
|---|---|
| **Top margin** | `lg` (16px) above |
| **Bottom margin** | `md` (8px) below — provides breathing room before the next text line |
| **Horizontal margin** | `lg` (16px) left and right |
| **Background** | transparent (floats on `editor_bg` per Constraint 6) |
| **Border** | none |
| **Header padding** | `sm`+2px (6px) vertical, `md` (8px) horizontal |
| **Header gap** | `md` (8px) between arrow, status glyph, and title |
| **Header font** | 12px `code_font` for title, `editor_fg` text color |
| **Status glyph** | `tool_completed` (green), `tool_in_progress` (yellow), `tool_failed` (red), `tool_pending` (muted) |
| **Expand/collapse** | `▼` expanded, `▶` collapsed, ` ` non-expandable. Color: `dim`. Click toggles. |

**Expanded body** contains up to three optional panes (input, content, output), each:

| Property | Value |
|---|---|
| **Top margin** | `sm` (4px) |
| **Horizontal margin** | `md` (8px) |
| **Internal padding** | `md` (8px) horizontal, `sm` (4px) vertical |
| **Background** | `tool_body_bg` for input/content, `tool_output_bg` for output |
| **Border radius** | `rounded_sm` |
| **Font** | 11px `code_font`, `tool_body_fg` text color |
| **Label** | 10px, 2px bottom padding |
| **Diff coloring** | `diff_add`, `diff_remove`, `diff_header` applied per-line |
| **Truncation** | Per `ToolRenderPolicy`: HeaderOnly (no body), Truncated (capped lines), Full |

**Multi-tool groups** nest child tool blocks inside the card with `border_l_2` left accent (color: `tool_card_border`), `sm` (4px) vertical margin, `md` (8px) left margin and padding.

#### TurnHeader

A visual divider inserted at role boundaries (user-to-agent, agent-to-user). Contains a bold role label and a horizontal rule.

| Property | Value |
|---|---|
| **Top padding** | `turn_gap` (32px) |
| **Bottom padding** | `md` (8px) |
| **Horizontal padding** | `lg` (16px) |
| **Gap (label to rule)** | 12px |
| **Label font** | 11px `body_font` (proportional), bold |
| **Label color** | `turn_header_agent` for "Claude", `turn_header_user` for "You" |
| **Rule** | 1px tall, `turn_rule` color, flex-1 to fill remaining width |
| **Background** | transparent |

The TurnHeader is the primary structural break in the transcript. Its `turn_gap` top padding is the largest vertical spacing in the system, creating a clear visual separation between turns.

#### ContentBlock

A rendered markdown block parsed from frozen agent output. Blocks are produced by `block_inner()` from the shared render crate and include headings, paragraphs, tables, code fences, blockquotes, lists, horizontal rules, and images.

| Property | Value |
|---|---|
| **Font** | `body_font` (proportional) for prose, `code_font` for code blocks |
| **Text size** | Headings: 28/24/20/18/16/15px (H1–H6). Body: 13px inherited. |
| **Code block** | `md` (8px) padding, `rounded_md`, `code_block_bg` background |
| **Table** | 1px border, `rounded_md`, `md` (8px) horizontal cell padding, `sm` (4px) vertical cell padding, bold header row |
| **Blockquote** | 3px left bar (`blockquote_bar` color), `lg`-3px (12px) left padding, italic |
| **Horizontal rule** | 1px tall, `md` (8px) vertical margin |
| **List** | 24px marker column, `md` (8px) gap to content |

ContentBlocks render at `text_scale = 1.0` in the agent pane (no zoom). They use the document view's `block_inner` function directly — the same rendering path as the Doc view, minus the cursor and selection overlays.

#### ThinkingIndicator

A pulsing status element shown at the end of the transcript while the agent is composing a reply.

| Property | Value |
|---|---|
| **Top padding** | `md` (8px) |
| **Bottom padding** | `sm` (4px) |
| **Left padding** | `sm` (4px) |
| **Gap (dot to text)** | `md` (8px) |
| **Dot** | 14px text, cyan hue (h=0.53, s=0.9, l=0.76), pulsing opacity 0.3–1.0 on 750ms sine wave |
| **Text** | 12px `body_font`, neutral gray (l=0.6), same pulsing opacity |

### Adjacency rules

The transcript is a flat list of elements rendered top-to-bottom. The vertical gap between any two adjacent elements is the sum of the first element's bottom spacing and the second element's top spacing.

**Critical GPUI constraint:** GPUI flex columns stack margins additively — there is no CSS-style margin collapse. If element A has 8px bottom margin and element B has 8px top margin, the gap is 16px, not 8px. All spacing in this system uses **top-margin-only convention**: each element declares spacing above itself; no element declares bottom margin. This ensures each inter-element gap is produced by exactly one element's margin.

| ↓ then → | TranscriptLine | ToolCard | TurnHeader | ContentBlock | ThinkingIndicator |
|---|---|---|---|---|---|
| **TranscriptLine** | `none` (0px) | `md` (8px) | `turn_gap` (20px) | `none` (0px) | `md` (8px) |
| **ToolCard** | ~1px (line py) | `md` (8px) | `turn_gap` (20px) | `none` (0px) | `md` (8px) |
| **TurnHeader** | 6px (pb) | 6px + 8px (14px) | — | 6px (pb) | 6px + 8px (14px) |
| **ContentBlock** | `none` (0px) | `md` (8px) | `turn_gap` (20px) | `none` (0px) | `md` (8px) |
| **ThinkingIndicator** | — | — | — | — | — |

**Implementation notes:**

- **Top-margin-only convention.** Each element owns the gap above it. TranscriptLine and ContentBlock have no top margin (they flow continuously). ToolCard has `mt(md)` = 8px. TurnHeader has `pt(turn_gap)` = 20px. ThinkingIndicator has `pt(md)` = 8px. No element uses bottom margin.
- **ToolCard → TranscriptLine** gap is intentionally tight (~1px from the line's internal padding). The card's bottom border provides visual separation; adding bottom margin would double the gap when followed by another ToolCard.
- **TurnHeader → ToolCard** gap is 14px (6px TurnHeader bottom padding + 8px ToolCard top margin). This is intentional — the ToolCard's own top margin provides its standard gap, and the TurnHeader's bottom padding provides its label-to-content spacing.
- TranscriptLine and ContentBlock use `none` between same-type consecutive items because they're visually continuous prose within a turn. The `xs` (1px) vertical padding on each line provides minimal breathing room without creating a visible gap.
- ThinkingIndicator is always the last item; no element follows it.

### Transcript body container

The scrollable container holding all transcript elements.

| Property | Value |
|---|---|
| **Horizontal padding** | `xl` (24px) |
| **Vertical padding** | 12px |
| **Font** | 13px `code_font` |
| **Text color** | `editor_fg` |
| **Layout** | Flex column, flex-1, min-height 0 (scroll) |
| **List** | GPUI virtualised list with `ListSizingBehavior::Auto` |

### Chrome regions

Chrome regions are separated from the transcript by hard visual boundaries (background color changes, borders). They do not participate in the adjacency matrix. Five chrome regions total.

#### StatusStrip

| Property | Value |
|---|---|
| **Height** | 28px fixed |
| **Padding** | `lg` (16px) horizontal, `sm` (4px) vertical |
| **Background** | `top_bar.bg` (theme), fallback `STATUS_BG` (0x16213e) |
| **Font** | 12px, bold |
| **Text color** | `top_bar.fg` (theme), fallback `STATUS_FG` (0x8be9fd) |
| **Field spacing** | `md` (8px) right padding between fields |
| **Field colors** | Label: foreground. Most fields: `dim`. Active timer: `warm_accent`. |
| **Layout** | Flex row, items centered. Turn/elapsed field right-aligned via flex spacer. |

Fields are conditionally rendered — absent data produces no element, no placeholder.

#### InfoBar

A secondary chrome bar positioned between the status strip and the transcript (or between transcript and footer, depending on `AgentStatusPosition` configuration). Shows context-window usage, working directory, and active subagents.

| Property | Value |
|---|---|
| **Height** | 22px fixed |
| **Padding** | `lg` (16px) horizontal, `sm` (4px) vertical |
| **Background** | `bottom_bar.bg` (theme), fallback `STATUS_BG` |
| **Font** | 11px `code_font` |
| **Text color** | `bottom_bar.fg` (theme), fallback 0x666666 |
| **Field gap** | `md` (8px) between fields |
| **Field labels** | `dim` color ("ctx", "cwd", "agents") |
| **Separator glyphs** | `·` between fields. **Known violation of Constraint 2:** separator color is hardcoded `0x44475a`. Should migrate to `turn_rule` or a new slot. |
| **Layout** | Flex row, items centered |

Fields: context window tokens (used/total/%), cwd (shortened for display), active/pending subagent labels with status glyphs. Absent usage data shows em-dash. Empty subagent list shows em-dash.

#### Footer

| Property | Value |
|---|---|
| **Height** | 22px fixed |
| **Padding** | `lg` (16px) horizontal, `sm` (4px) vertical |
| **Background** | `bottom_bar.bg` (theme), fallback `STATUS_BG` |
| **Font** | 11px |
| **Text color** | `bottom_bar.fg` (theme), fallback 0x666666 |
| **Layout** | Flex row, space-between. Left: mode + cursor position + status. Right: keybinding hints. |

#### Chatbox

| Property | Value |
|---|---|
| **Max height** | 144px (8 lines at 18px line height). **Note:** `spec-agent-window.md` §20 specifies `min(viewport_height / 3, 12)` rows; the implementation uses a fixed 8-row cap. This spec documents the implementation. The viewport-relative policy from the parent spec is unimplemented. |
| **Internal padding** | `lg` (16px) horizontal, `md` (8px) vertical |
| **External margin** | `md` (8px) horizontal, `sm` (4px) bottom |
| **Background** | Tinted variant of `editor_bg` (hue 0.55, saturation shift 0.1, lightness shift 0.03) |
| **Border** | 1px solid `dim`, `rounded_md` |
| **Font** | 13px `code_font` |
| **Text color** | `editor_fg` (DEFAULT_FG) |
| **Cursor** | `cursor` color from AgentTheme |
| **Separator** | 1px line above chatbox, `dim` color at reduced opacity. The `compose_separator` slot is used for the chatbox border edge, not the separator line itself. |
| **Overflow** | Vertical scroll when content exceeds max height. Horizontal overflow hidden. |

Only present when input mode is Chatbox.

#### Sidepane (Tasklist, Subagents)

Both sidepanes share identical container treatment. They stack horizontally to the right of the transcript, in fixed order: Tasklist (inner), Subagents (outer).

| Property | Value |
|---|---|
| **Width** | 196px fixed (28 columns * 7px) |
| **Background** | `pane_bg` |
| **Border** | 1px left border, `pane_border` color |
| **Vertical padding** | `sm` (4px) |
| **Font** | 12px `code_font` |
| **Header** | Bold, `pane_header` color, `md` (8px) horizontal padding, `sm` (4px) vertical padding |
| **Entry rows** | `md` (8px) horizontal padding, `xs` (1px) vertical padding, `editor_fg` text color |
| **Focused entry** | `warm_accent` text color, `dim` at 20% opacity background |
| **Empty state** | "(no plan)" / "(no subagents)" in `dim` color |
| **Entry truncation** | Tasklist: 22 chars max (21 + "..."). Subagents: 20 chars max (19 + "..."). |

### Color slot inventory

All color decisions are expressed as abstract slots in `AgentTheme`. Each slot has a defined semantic role; per-theme concrete RGB values live in `theme.rs`.

#### Author identity

| Slot | Role |
|---|---|
| `frozen_bar` | Gutter author bar for agent lines |
| `user_bar` | Gutter author bar for user lines |
| `agent_tint` | Text tint applied to agent prose (base-style tokens only) |
| `user_tint` | Text tint applied to user prose |
| `frozen_fg` | Text color for agent (frozen) lines |
| `agent_turn_bg` | Subtle background tint behind agent turn lines |
| `user_turn_bg` | Subtle background tint behind user turn lines |
| `selection_bg` | Selection highlight background. **Not yet an AgentTheme slot** — currently hardcoded as `SELECTION_BG` constant (Dracula-specific RGB). Must be added to `AgentTheme` with per-theme values. |

#### Turn structure

| Slot | Role |
|---|---|
| `turn_header_agent` | "Claude" label color in TurnHeader |
| `turn_header_user` | "You" label color in TurnHeader |
| `turn_rule` | Horizontal rule in TurnHeader |

#### Tool cards

| Slot | Role |
|---|---|
| `tool_card_bg` | Card container background |
| `tool_card_border` | Card border and nested tool left-accent |
| `tool_body_bg` | Input/content pane background |
| `tool_output_bg` | Output pane background |
| `tool_body_fg` | Text color inside tool body panes |
| `tool_completed` | Status glyph: completed (green) |
| `tool_in_progress` | Status glyph: in-progress (yellow) |
| `tool_failed` | Status glyph: failed (red) |
| `tool_pending` | Status glyph: pending (muted) |
| `tool_label` | Gutter label color for tool-call anchor lines |

#### Diff highlighting

| Slot | Role |
|---|---|
| `diff_add` | Added lines in tool output |
| `diff_remove` | Removed lines in tool output |
| `diff_header` | Header lines (`---`, `+++`, `@@`) in tool output |

#### Accents and chrome

| Slot | Role |
|---|---|
| `dim` | Muted accent — gutters, arrows, disabled fields, sidepane empty state |
| `warm_accent` | Active accent — focused subagent, active timer, live indicators |
| `cursor` | Cursor color in chatbox |
| `compose_bg` | **Dead slot — remove.** Defined in all 7 themes but never read. The chatbox background uses `tint_bg(editor_bg, ...)` instead. Should be deleted from `AgentTheme` and all theme palettes. |
| `compose_separator` | Chatbox border edge color (not the 1px separator line, which uses `dim`) |
| `pane_bg` | Sidepane background |
| `pane_border` | Sidepane left border |
| `pane_header` | Sidepane header text |

#### Inherited from document theme

| Slot | Role |
|---|---|
| `top_bar.bg` / `top_bar.fg` | StatusStrip background/foreground |
| `bottom_bar.bg` / `bottom_bar.fg` | Footer background/foreground |
| `editor_bg` / `editor_fg` | Transcript background/foreground |
| `code_block_bg` | Code fence background in ContentBlocks |
| `blockquote_bar` | Blockquote left bar in ContentBlocks |
| `table_border` | Table border in ContentBlocks |

### Typography scale

| Context | Font | Size | Weight |
|---|---|---|---|
| Transcript line | `code_font` | 13px | normal |
| Gutter label | `code_font` | 10px | normal |
| TurnHeader label | `body_font` | 11px | bold |
| ToolCard header | `code_font` | 12px | normal |
| Tool body pane text | `code_font` | 11px | normal |
| Tool body pane label | `code_font` | 10px | normal |
| ContentBlock prose | `body_font` | 13px | normal |
| ContentBlock headings | `body_font` | 28/24/20/18/16/15px | bold |
| ThinkingIndicator dot | — | 14px | normal |
| ThinkingIndicator text | `body_font` | 12px | normal |
| StatusStrip | inherited | 12px | bold |
| Footer | inherited | 11px | normal |
| Chatbox | `code_font` | 13px | normal |
| Sidepane entries | `code_font` | 12px | normal |
| Sidepane header | `code_font` | 12px | bold |

## Data Model

This spec does not introduce new data structures. All presentation state (expanded tool calls, focused subagent, input mode, pane visibility) lives on `AgentState` as defined in `spec-agent-window.md`. All color slots live on `AgentTheme` in `theme.rs`. The spacing grid is a set of literal `px()` values in the rendering code — no runtime configuration.

## Interfaces

This spec does not define new APIs. The rendering code in `render_agent()` is the sole consumer. It reads `AgentTheme` for colors, `AgentState` for content and interaction state, and applies the grid constants and adjacency rules defined here as literal layout properties in the GPUI element tree.

The spec's value is as a **reference contract**: when modifying any spacing, color, font, or border in the agent pane rendering, the change should be expressible as a modification to this spec's tables. Ad-hoc values that don't map to a grid constant or a named color slot are a spec violation.

## Constraints

1. **Grid discipline.** Every margin, padding, and gap in the agent pane MUST be one of the six grid constants (`none`, `xs`, `sm`, `md`, `lg`, `xl`) or the derived `turn_gap`. Two exceptions use 6px (between `sm` and `md`): TurnHeader bottom padding (tuned for the visual weight of the label+rule) and ToolCard header vertical padding (tuned for the compact card header). No other non-grid values are permitted.

2. **No hardcoded colors.** All color decisions route through either `AgentTheme` slots or the inherited document theme (`top_bar`, `bottom_bar`, `editor_bg`, etc.). Raw hex values in rendering code are a spec violation. The two legacy constants (`STATUS_BG`, `STATUS_FG`) are fallbacks for when the theme doesn't provide `top_bar`/`bottom_bar` — they should not be used as primary color sources.

3. **Chrome is fixed-size.** StatusStrip (28px), footer (22px), and sidepane width (196px) are fixed. They do not scale with text zoom. The chatbox has a max height (144px) but grows with content below that cap.

4. **Transcript elements are virtualised.** The transcript body uses GPUI's `list()` with `ListSizingBehavior::Auto`. Element rendering must be stateless — each `FlatItem` renders independently from its index. Adjacency-aware spacing (e.g., "add extra gap before a ToolCard that follows a TranscriptLine") is implemented via margins on the element itself, not by inspecting neighbors at render time.

5. **Two fonts only.** `code_font` (monospace) and `body_font` (proportional). No third font family. `code_font` is the default in the transcript body container; `body_font` is used for TurnHeader labels, ThinkingIndicator text, and ContentBlock prose.

6. **Background layering.** Turn backgrounds (`agent_turn_bg`, `user_turn_bg`) are applied **only** to TranscriptLine rows tagged `TurnId::Llm` or `TurnId::User` respectively. Three categories of elements float on the base `editor_bg` with no turn tint:
   - **Structural elements:** TurnHeaders, ToolCards, ThinkingIndicator — these are visual scaffolding, not authored content.
   - **Tool-anchor lines:** TranscriptLines tagged `TurnId::Tool` — they exist as positional anchors for tool cards, not as user or agent prose.
   - **Unsubmitted lines:** TranscriptLines with no `TurnId` (`None`) — the user's in-flight draft.

   This creates a visual rhythm: tinted bands of authored text separated by neutral-background structural elements. Violating this (e.g., giving `TurnId::Tool` the `user_turn_bg`) causes the green user tint to bleed onto agent-turn tool cards.

## Revision History

- 2026-06-01 (2) — Adversarial review pass. Eight findings addressed: (1) `selection_bg` acknowledged as hardcoded Constraint 2 violation, added to slot inventory with migration note. (2) `compose_bg` marked as dead slot for removal. (3) Chatbox separator color corrected — uses `dim`, not `compose_separator`; slot description updated. (4) Chatbox max height divergence from `spec-agent-window.md` §20 documented (code uses fixed 8 rows, parent spec says viewport-relative). (5) `TurnId::Unsubmitted` clarified as unimplemented — code uses `None`. (6) InfoBar chrome region added (was undocumented). (7) Tool card header 6px acknowledged as second grid exception alongside TurnHeader. (8) Info bar's hardcoded separator color (`0x44475a`) flagged as Constraint 2 violation.
- 2026-06-01 — Initial DRAFT. Defines the three-layer presentation system (grid, elements, adjacency) for the agent pane. Covers all seven transcript element types, five chrome regions, full color slot inventory, and typography scale. Motivated by non-uniform spacing between tool cards and text, and insufficient visual differentiation between user and agent content.
