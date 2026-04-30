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
