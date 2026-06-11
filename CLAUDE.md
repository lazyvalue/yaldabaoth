# Yaldabaoth

Agentic operating system for Scott's life. Yaldabaoth is the Demiurge, the blind
craftsman who spins up a whole hierarchy of archons to run the world beneath him
while remaining serenely unaware there's a higher pleroma he's not party to.

Built in Rust. The surface is a GPUI desktop GUI (`yalda-gpui`), backed by
supporting binaries (`yalda-channel`, `yalda-session-server`). It began life as
a markdown editor; that's now just one App among many.

## Tiles and Apps

The workspace is a tree of **Tiles** (tabs + n-ary splits; see
`docs/specs/spec-tabs-and-splits.md`). Each Tile holds exactly one **App**
(`docs/specs/spec-tiles-and-apps.md`, ADR-0019) — the Demiurge arranges the Apps;
the work happens inside them:

- **`App::Buffer`** — a view onto the shared file-buffer pool, always in exactly
  one `BufferMode`: `Picking` (file/buffer browser), `Viewing` (rendered
  markdown), or `Editing` (raw source). `Viewing ⇄ Editing` toggle over the same
  pooled `SharedCore`; `Picking` is reachable via Cmd+O (Buffer-scoped).
- **`App::Agent`** — an `AgentTile`: a **viewport** bound to (at most) one ACP
  session. `App::Agent` is just the enum tag; the real split is `AgentTile` =
  the viewport/UX (in the layout tree, holds `bound: Option<SessionId>`) vs
  `AgentSession` = the conversation (transcript, channel, tools), owned by the
  `AgentSessions` store on the view (see `spec-agent-session-ownership.md`). The
  store enforces strict **1:1** — a session is bound by at most one tile; a
  session no tile binds is **free** and re-bindable. An unbound tile
  (`bound: None`) renders the **selector** (free sessions + "create new"); close
  / unbind / rebind keep the tile `App::Agent` showing the selector — it never
  vanishes and never silently becomes a Buffer (Agent and Buffer are orthogonal;
  there is no nested `underlying` buffer, and no "leave agent" gesture — an
  agent tile stays an agent tile; you close it or open a Buffer tile normally).
  Agent commands (`.` menu): select session · stop · send message · switch
  Worksheet⇄Message Box.

## Dev system (read this for how we work)

`docs/dev-system.md` is the operating manual: the spec → decision → scaffold →
implement → verify → integrate → log lifecycle, the definition of done, parallel-
work discipline, and the verification-harness plan. Key artifacts:

- `docs/specs/` — design (what). Skill: `/spec`.
- `docs/decisions/` — ADRs (why a path was chosen). Skill: `/decision`.
- `docs/worklog/` + `docs/backlog.md` — what happened / what's open. Skill: `/worklog`.
- `docs/projects/` — **multi-session project tickets** (see below).
- `/integrate` — converge parallel branches into one buildable branch.

### Project planning (`docs/projects/`) — skill: `/plan`

Work that spans multiple sessions (a refactor done in stages, a feature with a
tail of follow-ups) gets a **durable project record** so it survives context
loss. The in-session task list (TaskCreate) is the live mirror; these files are
the record that outlives it. Scaffold and extend them with `/plan`.

```
docs/projects/<project-slug>/
  project.md             # standing context: problem/why, goals, scope, the model, tickets table
  NNN-ticket-<slug>.md   # one actionable task: goal, subtask `- [ ]` checkboxes, verification, links
```

`project.md` is **context, not a task** — the shared understanding every ticket
assumes (root cause, the model, links, a tickets status table). A ticket is one
coherent deliverable with subtasks as checkboxes. Litmus: writing "why / the
model" → `project.md`; writing "do X, then Y" → a ticket. Tick boxes as subtasks
land and keep the session task list in sync; new threads get a new ticket
(`NNN+1`), not scope creep. Live on `main`. Example:
`docs/projects/agent-model-refactor/`.

**Definition of done:** builds + tests + pasted evidence + runtime-checked-or-
flagged + artifacts updated. "Compiles" is not done. The GPUI app can't be
driven headlessly yet, so most UX/perf changes need a human runtime check — say
so explicitly (building the verification harness is the top backlog item).

## Worktree workflow (default)

**Do substantial work in a git worktree, not the main checkout.** Each task /
feature / agent gets its own worktree + branch so the main working dir stays
clean and parallel work can't collide. Place worktrees under
`./.claude/worktrees/` (NOT as siblings of the repo in `~/ws/` — that clutters
the workspace dir). The harness already uses `./.claude/worktrees/` for agent
isolation; task worktrees live there too. `./.claude/worktrees/` is gitignored.

```
git worktree add .claude/worktrees/<task-slug> -b <task-slug>
```

Trivial one-file edits and conversational answers don't need a worktree; new
features, multi-file changes, and anything you'd run agents on do.

## The GUI

`yalda-gpui` is the user-facing surface; all new UX work targets it. The
shared document/editor/render crates live under `src/` (see "Shared crates"
below) and the GUI binary lives under `src/bin/yalda-gpui/`.

`cargo run --bin yalda-gpui [path]` launches it.

### GUI layout

`src/bin/yalda-gpui/` is a module-per-concern split (modules glob-import the
root via `use super::*;` and the root re-exports them with `pub(crate) use`,
so items stay crate-visible regardless of file):

- `main.rs` (~6.5k) — `YaldaGpuiView` struct, the `Render` impl, app/tab/
  split/doc methods, marks/layout-modes/tags, menus + overlays + pickers,
  key bindings + `main()`. A Tile (`Window<App>`) holds one `App`
  (`spec-tiles-and-apps.md`, ADR-0019): `App::Buffer(BufferApp)` —
  `BufferApp::{Picking(file browser), Viewing(rendered doc), Editing(raw)}`
  — or `App::Agent(AgentTile)` (a viewport bound to one session in the
  `AgentSessions` store; see `spec-agent-session-ownership.md`). The render path
  branches on that, each screen with its own `key_context` (`YaldaView`,
  `EditView`, `BrowserView`, `AgentView`) and its own `on_action` wiring.
- `screens.rs` — the screen render bodies: `render_doc`, `render_edit`
  (Code + WP), `render_agent`, `render_browser`.
- `agent.rs` — agent-tile data layer: tool-call model, `FlatItem` view model
  + S1 cache + `rebuild_agent_view_model`, `TurnPhase`, `AgentState`,
  `AgentSession`, `AgentTile`.
- `agent_sessions.rs` — the `SessionStore`/`AgentSessions` owner: the private
  `SessionId → AgentSession` registry that enforces the 1:1 binding invariant
  (`open_or_focus`, `bind_sid`, `locate`, `close`).
- `agent_ui.rs` — agent/session methods on the view: open/attach/create/
  close flows, server pump + reducers (`apply_server_batch`
  / `apply_reply_events` / `apply_agent_event`), submit paths, Claude key
  handler.
- `chrome.rs` — focused-window/layout render, tab strip, tag bar, rails.
- `edit_ui.rs` / `browser_ui.rs` — per-screen methods (edit entry/exit + key
  dispatch; browser nav + rail).
- `render_blocks.rs` — free render helpers: colors/fonts, styled-line/block/
  table elements, wiki links, WP line classifier.
- `persist.rs` — paths, preferences, workspace + ACP-session persistence,
  server launch helpers.
- `workspace.rs` — tab strip + n-ary split tree (`Workspace<C>`,
  `FocusedWindow`, etc.). See `docs/specs/spec-tabs-and-splits.md`.
- `tests.rs` / `verify_harness.rs` — unit tests + headless render harness.

Keep the split honest: new agent-tile logic goes in `agent.rs`/`agent_ui.rs`,
new render helpers in `render_blocks.rs` — don't let `main.rs` re-accrete.

### GUI screens

- **Doc view (`YaldaView`)** — rendered markdown, block-by-block. Cursor
  is a left orange bar on the focused block; j/k or arrows move block focus.
  Built from `RenderedBlock`s via `block_element` / `block_inner` /
  `styled_line_element`.
- **Edit view (`EditView`)** — raw markdown editing, two sub-views toggleable
  with Ctrl-W:
  - `EditView::Code` (RAW): monospace, line-number gutter, `md_highlight`
    source colors.
  - `EditView::WordProcessor` (WP): proportional font, per-line typographic
    classification (`classify_wp_line`) for headings/lists/blockquote/code.
- **Browser view (`BrowserView`)** — file picker for `Cmd+O`.
- **Claude session (`ClaudeView`)** — ACP chat panel for the active session.

### GUI key conventions

Per-screen vim-style bindings live with `Some("YaldaView")` etc. contexts.
Global Cmd shortcuts (Quit, OpenBrowser, OpenClaude, tab/split management,
zoom) are registered with `None` context and **must** have a matching
`on_action(Self::handler)` on every screen's root so the dispatch lands.

### Document text zoom

`Cmd-=` / `Cmd-+` zoom in, `Cmd--` zooms out, `Cmd-0` resets. Implementation
is a `text_scale: f32` on `YaldaGpuiView` (clamped `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`,
step `TEXT_SCALE_STEP = 1.1`) that multiplies the body `text_size(px(14.0))`
and every heading size. Threaded into `RenderCtx::text_scale` for block
rendering. **Chrome stays fixed** — status bars, tab strip, browser rows,
Claude session blocks all render at their native sizes. To extend the zoom
to a new surface, multiply that surface's base `text_size` by `self.text_scale`
and add `on_action(Self::zoom_in/out/reset)` to its root.

## Naming Conventions

### Modes

Two top-level modes:

- **View Mode** — rendered markdown display (read-only navigation)
- **Edit Mode** — raw markdown source editing, with two submodes:
  - **Normal** — vim-style navigation and commands
  - **Insert** — text input

In code, `ViewMode::Rendered` corresponds to View Mode, and `ViewMode::Raw` corresponds to Edit Mode. `AppMode::Normal` and `AppMode::Insert` are the Edit Mode submodes.

## Shared crates

The document/editor/render layer under `src/` (consumed by `yalda-gpui` and
the supporting binaries):

- `document.rs` — text buffer backed by ropey rope
- `render.rs` — markdown-to-rendered-blocks conversion (pulldown-cmark)
- `editor.rs` — editing operations over the document
- `keybind.rs` — key binding definitions and sequence matching
- `keys.rs` / `style.rs` — frontend-neutral key + styling primitives
- `command.rs` — command registry (`:` commands)
- `md_highlight.rs` — syntax highlighting for edit mode
- `theme.rs` — color themes
- `blocks.rs` — rendered block types (Heading, Paragraph, Table, etc.)
