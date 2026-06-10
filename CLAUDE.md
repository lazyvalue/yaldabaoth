# Sketch

A markdown editor built with Rust. Ships as two binaries: a GPUI desktop GUI
(`sketch-gpui`) and a terminal TUI (`sketch`) built on ratatui + crossterm.

## Dev system (read this for how we work)

`docs/dev-system.md` is the operating manual: the spec → decision → scaffold →
implement → verify → integrate → log lifecycle, the definition of done, parallel-
work discipline, and the verification-harness plan. Key artifacts:

- `docs/specs/` — design (what). Skill: `/spec`.
- `docs/decisions/` — ADRs (why a path was chosen). Skill: `/decision`.
- `docs/worklog/` + `docs/backlog.md` — what happened / what's open. Skill: `/worklog`.
- `/integrate` — converge parallel branches into one buildable branch.

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

## Default surface: the GUI

**Work on the GUI by default.** Unless the user says "TUI" or names the TUI by
feature (debug overlay, viewport wrap math, etc.), assume new work targets
`sketch-gpui`. Both binaries share the document/editor/render crates under
`src/`, but the user-facing surface is the GPUI app.

`cargo run --bin sketch-gpui [path]` launches it.

### GUI layout

`src/bin/sketch-gpui/` is a module-per-concern split (modules glob-import the
root via `use super::*;` and the root re-exports them with `pub(crate) use`,
so items stay crate-visible regardless of file):

- `main.rs` (~6.5k) — `SketchGpuiView` struct, the `Render` impl, app/tab/
  split/doc methods, marks/layout-modes/tags, menus + overlays + pickers,
  key bindings + `main()`. The render path branches on `WindowContent`
  (Doc / Edit / Browser / ClaudeSession), each with its own `key_context`
  (`SketchView`, `EditView`, `BrowserView`, `ClaudeView`) and its own
  `on_action` wiring.
- `screens.rs` — the screen render bodies: `render_doc`, `render_edit`
  (Code + WP), `render_agent`, `render_browser`.
- `agent.rs` — agent-tile data layer: tool-call model, `FlatItem` view model
  + S1 cache + `rebuild_agent_view_model`, `TurnPhase`, `AgentState`,
  `AgentRing`.
- `agent_ui.rs` — agent/session methods on the view: open/attach/create/
  close flows, lease heartbeat, server pump + reducers (`apply_server_batch`
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

- **Doc view (`SketchView`)** — rendered markdown, block-by-block. Cursor
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

Per-screen vim-style bindings live with `Some("SketchView")` etc. contexts.
Global Cmd shortcuts (Quit, OpenBrowser, OpenClaude, tab/split management,
zoom) are registered with `None` context and **must** have a matching
`on_action(Self::handler)` on every screen's root so the dispatch lands.

### Document text zoom

`Cmd-=` / `Cmd-+` zoom in, `Cmd--` zooms out, `Cmd-0` resets. Implementation
is a `text_scale: f32` on `SketchGpuiView` (clamped `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`,
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

(The TUI uses the View/Edit terminology above. The GPUI app uses the screen
names from "GUI screens" — they don't perfectly correspond, since the GUI's
EditView itself has Code/WordProcessor sub-modes that the TUI doesn't have.)

## TUI Architecture

- `app.rs` — main application loop, input handling, state management
- `view.rs` — rendering (both rendered and raw modes)
- `document.rs` — text buffer backed by ropey rope
- `render.rs` — markdown-to-rendered-blocks conversion (pulldown-cmark)
- `viewport.rs` — scroll position, content width/offset
- `keybind.rs` — key binding definitions and sequence matching
- `command.rs` — command registry (`:` commands)
- `md_highlight.rs` — syntax highlighting for raw/edit mode
- `theme.rs` — color themes
- `blocks.rs` — rendered block types (Heading, Paragraph, Table, etc.)

## Debug Overlay (TUI)

Run with `SKETCH_DEBUG=1` to capture per-frame ground-truth state from the
renderer to a JSON-lines log:

```
SKETCH_DEBUG=1 sketch <file>
tail -f ~/.sketch/debug.log | jq .   # all platforms (durable home, ADR-0018)
```

Each line records terminal size, computed vs. actual content-area height,
scroll offset, total visual rows, the cursor's expected visual row (from
scroll math), the cursor's actual screen y (from the renderer — `null` if
the cursor wasn't painted), the first/last visible doc-line indices, frozen
state, mode, and view mode.

The log dedupes identical frames (so it stays quiet on idle ticks) and ALWAYS
records frames where the cursor is off-screen or where off-screen status
flipped. Splash frames are skipped.

Use this whenever you suspect a viewport/scroll bug. Compare `expected_visual_row`
(scroll math's view) against `cursor_screen_y` and `last_visible_doc_line`
(renderer's view); when they disagree, the bug is in the predictor, not the
renderer.

### Source-of-truth invariant for visual row math

The renderer's wrap algorithm is the only authority on how lines lay out.
`view::wrap_row_count` and `view::wrap_row_count_with_cursor` expose that
algorithm; `buffer::raw_visual_row_count` and `buffer::raw_cursor_visual_row`
MUST call them (against the same tab-expanded line text the renderer uses)
rather than re-implementing wrap with `div_ceil`. Any divergence between
predictor and renderer compounds over the buffer and pushes the cursor
off-screen near the bottom of the viewport.
