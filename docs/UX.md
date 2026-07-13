# Yalda UX Reference — component & tile catalog

**What this is.** A catalog of every **tile type** and **UX component** in the
GUI, and the **features each one has**. It answers "what can this surface do?".

**How it differs from `ux-invariants.md`.** `ux-invariants.md` is the cross-
cutting *behavior contract* — the universal maxims every surface must honor
(`INV-UX-N`: the cursor is always visible, the compose word-wraps, …). This file
is the opposite axis: it enumerates the surfaces themselves and their
*properties*. Invariants are laws that apply everywhere; this is the parts list.
When a component gains a behavior worth pinning everywhere, it becomes an
`INV-UX-N` there; when it gains a feature, it's recorded here.

LIVING — keep entries in sync as surfaces gain/lose features. Each entry names
its code home and any governing spec.

## Window & layout taxonomy

Shared vocabulary for the GUI's containment hierarchy:

| Term | Code type | Meaning |
|---|---|---|
| **Frame** | GPUI `Window`/`WindowHandle` | The OS-level desktop window. |
| **Workspace** | `Workspace<App>` | Single tab-strip + buffer-pool container. One per frame. |
| **Tab** | `Tab<App>` | One tab-bar entry. Owns a layout tree and a focused-tile pointer. |
| **Split** | `Layout::Split` | Interior node in the layout tree — direction (`H`=stacked, `V`=side-by-side) + weighted children. |
| **Tile** | `Window<App>` (code name is `Window`) | A leaf in the split tree. Stable `WindowId` + one `App`. |
| **App** | `App` enum | What's inside a tile: `Buffer`, `Agent`, `Linear`. |

The code-level struct is still called `Window`, but in discussion we say **tile**
to avoid confusion with the OS-level frame. See
`docs/specs/spec-tabs-and-splits.md` (tabs/splits) and
`docs/specs/spec-tiles-and-apps.md` + ADR-0019 (one App per tile).

---

## Tiles / Apps — what lives inside a tile

Each tile holds exactly one `App` (`main.rs` `enum App`). Apps are orthogonal —
there is no nesting (no Agent "behind" a Buffer); you close a tile or open
another. `App = Buffer(BufferApp) | Agent(AgentTile) | Linear(LinearTile)`.

### Buffer — `App::Buffer(BufferApp)`

A view onto the shared file-buffer pool (ADR-0007), always in exactly one mode.
`Viewing ⇄ Editing` toggle over the same pooled `SharedCore`.

- **`Picking` (Browser view, `BrowserView`)** — file/buffer browser. Features:
  directory navigation, parent (`go_parent`), hidden-file toggle, sort cycle,
  worktree mode, filter input. Reached via `Cmd+O` (Buffer-scoped). Code:
  `browser_ui.rs`, `screens.rs::render_browser`.
- **`Viewing` (Doc view, `YaldaView`)** — rendered markdown, block-by-block.
  Features: left orange cursor-bar on the focused block; `j/k`/arrows move block
  focus; `g`/`G` top/bottom; `Ctrl-D`/`Ctrl-U` page; wiki-links; document text
  zoom (`Cmd-=`/`-`/`0`); marks (`m`/`'`). Built from `RenderedBlock`s. Code:
  `screens.rs::render_doc`, `render_blocks.rs`.
- **`Editing` (Edit view, `EditView`)** — raw markdown source, two submodes
  toggled with `Ctrl-W`:
  - **Code (RAW)** — monospace, line-number gutter, `md_highlight` source colors.
  - **WordProcessor (WP)** — proportional font, per-line typographic
    classification (`classify_wp_line`) for headings/lists/blockquote/code.
  - Vim-style Normal/Insert submodes (`AppMode`); the caret is always visible and
    tracks text (INV-UX-1). Code: `edit_ui.rs`, `screens.rs::render_edit`.

### Agent — `App::Agent(AgentTile)`

A **viewport** bound to at most one ACP session (`spec-agent-session-ownership.md`,
strict 1:1). `AgentTile` = the viewport/UX; `AgentSession` = the conversation
(owned by the `AgentSessions` store). An unbound tile renders the **selector**
(free sessions + "create new"); it never vanishes or becomes a Buffer. Composed
of the Transcript, the Compose surface, and optional sidebars (below).

- **Selector** — lists free sessions + "create new"; binds on pick.
- **Session lifecycle** — select / stop (`Cmd-.`) / send / switch
  Worksheet⇄Message-box (`Ctrl-Alt-Enter`); `/clear`; permission-mode cycle.
- **Turn phase** (`TurnPhase`: Idle / Awaiting / StopRequested) drives the
  thinking indicator, the Stop button, and the jump-panel status dot (INV-UX-10).
- Governing specs: `spec-agent-window.md`, `spec-agent-presentation.md`,
  `spec-worksheet.md`, `spec-turn-steering.md`. Code: `agent.rs`, `agent_ui.rs`,
  `agent_sessions.rs`, `screens.rs::render_agent`.

### Linear — `App::Linear(LinearTile)`

Views Linear issues/projects by tag via GraphQL (`Cmd-L`). Features: issue/project
list and cached detail body (built on **yux**); `LINEAR_API_KEY` env. Code:
`linear.rs`, `linear_ui.rs`, `linear_view.rs`. See memory `project_linear_app`.

---

## Components — chrome & reusable surfaces

### Jump panel (`jump_panel_view.rs`, `spec-jump-panel.md`, ADR-0021)

An always-visible root-level **navigator sidebar** (fixed `JUMP_PANEL_WIDTH`),
laid out outside the workspace content so it stays put across workspace switches
(INV-JP1). Toggle: `toggle_jump_panel`. Rendered inline (cheap, O(workspaces +
sessions)), not cached. Sections:

- **Pinned** — *placeholder* (pinning mechanics land later).
- **Workspaces** — one row per non-ephemeral tab, active marked (accent label).
  - Each row's badge shows the **1-based workspace number** (`idx + 1`) — the
    digit `ctrl-<n>` jumps to (INV-UX-11).
  - Click → `select_tab`.
- **Agent sessions** — the universal roster (every server session) ∪ local-only
  mid-create sessions (`jump_panel_agent_rows`).
  - **Dot shape** = binding: `●` in-use / `○` free.
  - **Dot color** = per-session status light (INV-UX-10): **working** (reply in
    flight) = warm accent, **waiting for you** (turn finished) = green,
    **neutral/disconnected** = dim. Disconnected also dims the whole row.
  - Click → bound session focuses its tile; **free** session opens in an
    ephemeral virtual workspace (torn down on switch-away).

### Rail (`spec-rail.md`, `chrome.rs`)

A persistent **per-tab** side column (distinct from the root-level jump panel).
Kinds: file-browser rail (`Cmd-B` / `ToggleFileBrowserRail`), outline rail
(`ToggleOutlineRail`). Features: side flip (`FlipRailSide`); rail-focused nav
(`RailDown/Up/Select/Close/Parent`), hidden toggle, sort cycle, worktrees,
filter (`RailView` context).

### Tab strip (`chrome.rs`, `workspace.rs`)

The workspace's tab bar. Features: per-tab label (`display_label`), active marker,
click-to-select, rename overlay (`RenameTab`); `Ctrl-Tab`/`Ctrl-Shift-Tab` next/
prev, `Cmd-T` new tab, `Cmd-Shift-W` close, `ctrl-<n>` jump by number (INV-UX-11).

### Tag bar / layout modes (`spec-layout-patterns.md`, `main.rs`)

Tile tags + automatic layout patterns. Features: tag view/toggle chords, clear
tag view; layout-mode cycle, desktop tile size, promote-to-master, master-count
+/-.

### Transcript view (`transcript_view.rs`, `TranscriptView`; yux cached child)

The agent conversation, a **cached child entity** (the reference yux component;
load-bearing for typing latency). Features: per-turn gutter label + author tint +
left bar (no per-turn card background, INV-UX-3); tool-call cards with status
glyphs; collapsible tool groups (fold header is one line); diff highlighting;
wiki-links; selection + copy; navigation caret with focus-row highlight; thinking
indicator while awaiting. Append-only / ordered (INV-ORDER, Model C / ADR-0024).

### Compose surface (`spec-worksheet.md`, `spec-textbox-compose.md`)

The agent input, in two placements (`InputModeKind`):

- **Worksheet (default)** — an inline-editable conversation buffer (INV-UX-9):
  free Normal-mode navigation over the transcript; `i` opens a You-block at the
  caret; empty-exit discards (byte-identical); non-empty persists and is sent;
  multiple insertion points; insert gated to the latest agent turn.
- **Chatbox (message box)** — a diminutive **pinned box** at the bottom, shown
  **only mid-turn**; input steers/queues (INV-UX-7).
- Both **word-wrap** (INV-UX-2) and keep the caret visible (INV-UX-1).

### Agent sidepanel (`screens.rs::render_agent`)

A segmented, fixed-width **sidepanel on the RIGHT** of the tile: **Plan / Tasklist**
on TOP (`Cmd-1`) and **Subagents** BELOW (`Cmd-2`), divided by a segment border, both
visible at once. Each segment is a scrollable strip, one row per entry; with one open
it fills the sidepanel height. The main column (transcript + compose) takes the
remaining width. Subagents are detected structurally from the harness and shown
one-per-line (INV-UX-5); clicking a subagent row focuses its output.

- **Panel focus (`Cmd-0`, INV-UX-12)** — focuses + widens the sidepanel for
  keyboard use. Selection is **2-D**: **`h`/`l`** (or `←`/`→`) switch the active
  **segment** (Plan ↔ Subagents), **`j`/`k`** (or `↑`/`↓`) move the **row** within
  it, **`g`/`G`** jump to top/bottom of the segment. **`Enter`** activates (a
  Subagent row focuses its output and exits; a Plan row has no target yet), **`Esc`**
  leaves and restores the prior focus. The mode is **modal** (other keys inert) and
  re-seats / auto-exits when the active segment closes. In an agent tile `Cmd-0` is
  panel-focus, not zoom-reset.

### Menus & overlays (`main.rs`)

Single-keypress command surfaces and transient inputs:

- **Leader menus** — `space` → tile/app menu, `.` → workspace menu, `?` → global
  menu (`spec-menu-scopes.md`). The global menu lists goto-workspace entries
  (digit `i+1`).
- **Buffer switcher** (`BufferSwitcherView`) — pick among open buffers; filter.
- **Workspace picker** (`WorkspacePickerView`) — move / also-show the focused
  tile into another workspace (incl. "+ new workspace").
- **Rename overlay** (`RenameOverlayView`) — single-line tab rename.
- **Tag input / mark & tag chords** — capture-phase next-key chords (`m`/`'`,
  tag).
- **Splash** (`SplashView`) — startup overlay, dismissed on any key/click (auto
  after ~1.5s).
- **Toasts** — transient bottom-right status notifications.

Overlays dispatch keys in **capture phase** with `stop_propagation` so they
consume input before global action dispatch.

### Status bars / footer (`screens.rs`, `chrome.rs`)

Per-screen header/footer chrome (block position, mode indicator, hint line). Render
at native size — **chrome stays fixed** under document zoom.

### Document text zoom (`main.rs`)

`Cmd-=`/`Cmd-+` in, `Cmd--` out, `Cmd-0` reset — a `text_scale` multiplying body
+ heading sizes. It scales the buffer doc + edit views **and the agent transcript**
(conversation prose + markdown blocks, INV-UX-13). Chrome (status bars, tab strip,
browser rows, jump panel, agent gutter/labels, the right sidepanel, and the pixel-pinned
compose input) stays at native size. `Cmd-0` resets everywhere except agent tiles,
where it is panel-focus (INV-UX-12).

---

## See also

- `docs/ux-invariants.md` — the universal behavior contract (`INV-UX-N`).
- `src/bin/yalda-gpui/yux/CLAUDE.md` — the component/perf rules every cached
  surface follows.
- `docs/specs/` — per-surface design specs (linked per entry above).
