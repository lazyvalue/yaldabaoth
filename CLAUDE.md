# Sketch

A terminal-based markdown editor built with Rust, ratatui, and crossterm.

## Naming Conventions

### Modes

Two top-level modes:

- **View Mode** — rendered markdown display (read-only navigation)
- **Edit Mode** — raw markdown source editing, with two submodes:
  - **Normal** — vim-style navigation and commands
  - **Insert** — text input

In code, `ViewMode::Rendered` corresponds to View Mode, and `ViewMode::Raw` corresponds to Edit Mode. `AppMode::Normal` and `AppMode::Insert` are the Edit Mode submodes.

## Architecture

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

## Debug Overlay

Run with `SKETCH_DEBUG=1` to capture per-frame ground-truth state from the
renderer to a JSON-lines log:

```
SKETCH_DEBUG=1 sketch <file>
tail -f ~/Library/Caches/sketch/debug.log | jq .   # macOS
tail -f ~/.cache/sketch/debug.log | jq .           # Linux
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
