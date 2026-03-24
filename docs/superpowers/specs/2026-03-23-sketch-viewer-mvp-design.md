# Sketch: TUI Markdown Viewer MVP — Design Spec

## Overview

Sketch is a TUI markdown viewer (and eventually editor) built in Rust. The MVP is a **viewer only** — open a markdown file, render it beautifully with full markdown support, navigate with vim-style keybindings, and quit.

The long-term vision is a modal editor with Obsidian-style inline rendering, configurable keybindings, and eventual code editing support. The MVP lays the foundation by nailing rendering quality and establishing the right abstractions.

## Tech Stack

- **ratatui** — TUI framework (layout, rendering, event loop)
- **crossterm** — terminal backend for ratatui
- **pulldown-cmark** — CommonMark-compliant markdown parser
- **syntect** — syntax highlighting for code blocks (bundled syntaxes/themes)
- **kdl** — config file parsing
- **clap** — CLI argument parsing

## Architecture

Four layers, each with a single responsibility:

```
┌─────────────────────────────────┐
│  App Layer (main.rs)            │  Event loop, state, keybindings
├─────────────────────────────────┤
│  View Layer                     │  Layout, scrolling, viewport
├─────────────────────────────────┤
│  Render Layer                   │  Markdown → styled ratatui spans
├─────────────────────────────────┤
│  Parse Layer                    │  pulldown-cmark event stream
└─────────────────────────────────┘
```

- **Parse Layer** — Takes a markdown string, produces a pulldown-cmark event iterator. Thin wrapper to keep the parser swappable (for tree-sitter when editing is added).
- **Render Layer** — Walks the event stream, produces a `Vec<RenderedBlock>`. Each block knows its styled content. Takes a `&Theme` for all color/style decisions.
- **View Layer** — Handles viewport: scrolling, line wrapping, visibility. Produces the final ratatui `Frame` output. Only layer that knows terminal dimensions.
- **App Layer** — Event loop, mode state, keybind dispatch, file loading, quit. Only layer that mutates state.

Parse and render are pure functions — easy to test. View handles presentation. App handles mutation.

## Rendered Block Model

The central data structure between rendering and display:

```rust
enum RenderedBlock {
    Heading { level: u8, content: StyledLine },
    Paragraph { lines: Vec<StyledLine> },
    CodeBlock { language: Option<String>, lines: Vec<StyledLine> },
    BlockQuote { blocks: Vec<RenderedBlock> },
    List { ordered: bool, start: Option<u64>, items: Vec<ListItem> },
    Table { headers: Vec<StyledLine>, rows: Vec<Vec<StyledLine>>, alignments: Vec<Alignment> }, // Alignment = Left | Center | Right
    HorizontalRule,
    Image { alt: String, url: String },
}

struct StyledLine {
    spans: Vec<StyledSpan>,
}

struct StyledSpan {
    text: String,
    style: Style,           // ratatui Style (fg, bg, bold, italic, etc.)
    link: Option<String>,   // URL if this span is a link
}

struct ListItem {
    marker: String,              // "•", "1.", "a)"
    checked: Option<bool>,       // Some(true) = [x], Some(false) = [ ], None = not a task
    content: Vec<RenderedBlock>, // recursive — blocks can nest
}
```

- `BlockQuote` and `ListItem` contain `Vec<RenderedBlock>` for natural nesting
- `StyledSpan` carries link URL separate from display style
- `StyledLine` is pre-styled — the view layer just positions, never re-styles
- `ListItem.checked` supports task list rendering (`- [x]` / `- [ ]`)

## Theme System

A `Theme` struct provides all color/style values. The render layer takes `&Theme` as input — swapping themes changes all colors in one place.

```rust
struct Theme {
    heading: [Style; 6],       // h1 through h6
    paragraph: Style,
    bold: Style,
    italic: Style,
    strikethrough: Style,
    code_inline: Style,
    code_block_bg: Style,
    blockquote_bar: Style,
    blockquote_text: Style,
    link: Style,
    table_border: Style,
    table_header: Style,
    horizontal_rule: Style,
    list_marker: Style,
    image_label: Style,
}
```

MVP ships with one dark theme. Dark/light mode and custom themes are a future config option — the struct makes this trivial to add.

**Style composition:** Styles are merged in order: base element style (e.g., paragraph), then inline modifiers (bold, italic, strikethrough), then semantic styles (link, code_inline). More specific styles override less specific ones for conflicting attributes — e.g., `code_inline` foreground wins over `link` foreground when text is both.

## Keybinding System

Keybindings are defined as a map of `Action → Vec<KeySequence>`. The app works entirely in terms of actions, never raw keys.

- Vim defaults ship out of the box
- Multi-key sequences (`gg`, `]]`) are first-class
- Config file extends defaults (user bindings are added/overridden; unspecified defaults remain). To unbind a default, map to `action="None"`
- When custom modal bindings are added later, the map is swapped/layered per mode

Config format (KDL):

```kdl
mode "normal" {
    key "j" action="ScrollDown"
    key "k" action="ScrollUp"
    key "gg" action="JumpTop"
    key "G" action="JumpBottom"
    key "/" action="SearchForward"
    key "v" action="EnterVisual"
    key "}" action="NextHeading"
}

mode "visual" {
    key "y" action="Yank"
    key "Escape" action="ExitVisual"
    key "ip" action="SelectInnerParagraph"
    key "ic" action="SelectInnerCodeBlock"
}
```

### Default Keybindings (MVP)

**Scrolling:**
- `j`/`k` — line up/down
- `Ctrl+d`/`Ctrl+u` — half page
- `Ctrl+f`/`Ctrl+b` — full page
- `gg`/`G` — top/bottom

**Structure navigation:**
- `{`/`}` — prev/next heading (centers heading on screen)
- `[[`/`]]` — prev/next heading at same level

**Search:**
- `/` — search forward
- `?` — search backward
- `n`/`N` — next/prev match

**Actions:**
- `q` — quit
- `o` — open link under cursor in browser
- `y` — yank current line

## Syntax Highlighting

`syntect` handles code block highlighting:

- Language tag from fenced code blocks selects the syntax
- Highlighted spans map to `StyledSpan` foreground colors
- Code blocks get distinct background + left border accent
- Unrecognized/missing language falls back to plain monospace with code block styling
- All syntaxes and one theme bundled in the binary (~5MB increase, worth it for single-binary distribution)

## Image Rendering

MVP renders images as styled text placeholders: `[Image: alt text]` (or `[Image: filename]` if no alt text). The placeholder uses the `image_label` theme style.

Kitty graphics protocol rendering (actual inline images) is deferred to post-MVP.

## Scrolling & Viewport

The viewport works in terminal lines, not blocks — blocks have variable height.

- `scroll_offset` tracks position in terminal lines
- On render, walks block list, accumulates heights, determines visible blocks
- Partially visible blocks at top/bottom are clipped

**Line wrapping:**
- Paragraphs soft-wrap at `min(max_line_width, terminal_width)`
- Content is centered horizontally when terminal is wider than `max_line_width`
- Code blocks do NOT wrap — truncated at terminal width with a `→` indicator at the right edge
- Terminal resize triggers re-wrap and scroll adjustment to keep roughly the same content visible

**Configurable wrap width:**

```kdl
display {
    max-line-width 80
}
```

**Cursor:**
- Logical cursor position exists even in viewer mode (needed for visual mode, link activation, future editing)
- Cursor line gets a subtle background highlight

## CLI Interface

```
sketch README.md        # open file
sketch                  # show usage/help
```

- File read fully into memory on startup
- Absolute path displayed in top bar
- Non-existent file prints error and exits (no TUI)

## Config

Location: `~/.config/sketch/config.kdl` (XDG), with `SKETCH_CONFIG` env var override.

## UI Layout

```
┌──────────────────────────────────────────┐
│  README.md                line 24/186 13%│  ← top bar
├──────────────────────────────────────────┤
│                                          │
│  # Sketch                                │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━              │
│                                          │
│  A beautiful TUI markdown viewer...      │
│                                          │
│  ## Installation                         │
│                                          │
│  ┃ cargo install sketch                  │  ← content area
│                                          │
│  > Note: Requires Rust 1.75             │
│                                          │
│  • Fast rendering with ratatui           │
│  • Syntax highlighting via syntect       │
│                                          │
├──────────────────────────────────────────┤
│  NORMAL     j/k scroll · / search · q quit│  ← bottom bar
└──────────────────────────────────────────┘
```

- **Top bar:** filename + scroll position (line/total, percentage)
- **Content area:** rendered markdown, full width up to `max_line_width`
- **Bottom bar:** mode indicator (left) + context-sensitive key hints (right)

## Error Handling

- **Non-existent file:** Print error to stderr, exit with code 1. No TUI entered.
- **Non-UTF-8 file:** Print error to stderr, exit with code 1.
- **Malformed markdown:** pulldown-cmark handles all valid and invalid markdown gracefully (it never errors — garbage in, best-effort rendering out). No special handling needed.
- **Terminal too small:** If terminal is below 40 columns or 5 rows, display a "terminal too small" message in the TUI instead of rendered content. Re-check on resize.
- **Config parse errors:** Malformed KDL in config file logs a warning to stderr on startup, falls back to all defaults.
- **Panic recovery:** Install a panic hook that restores terminal state (disable raw mode, show cursor, leave alternate screen) before printing the panic message. Prevents a crash from leaving the terminal in a broken state.

## Testing

- **Unit tests** for the parse and render layers: known markdown inputs → expected `Vec<RenderedBlock>` output. These are pure functions, easy to test exhaustively.
- **Unit tests** for the keybinding mapper: key sequences → expected actions.
- **Snapshot tests** for rendered output: render sample markdown files and compare styled text output against saved snapshots (useful for catching visual regressions).
- **No TUI integration tests in MVP** — manual testing for viewport/scrolling behavior.

## Future Considerations (Not in MVP)

These are explicitly out of scope for the MVP but the architecture accommodates them:

- **Editing:** Obsidian-style inline rendered editing (cursor reveals raw markdown per block)
- **Raw/rendered toggle:** Switch between edit and preview modes
- **Tree-sitter parser:** Incremental parsing for editing performance
- **File browser / tree sidebar:** Multi-file navigation
- **Custom themes:** User-defined themes in config
- **Dark/light mode toggle**
- **Lua scripting:** Programmable config for complex keybinding logic
- **Visual mode:** Character and line selection, text objects (`ip`, `ic`, `ih`, `ib`), yank selection
- **Stdin piping:** `sketch -` to read from stdin
- **Kitty image rendering:** Inline images via Kitty graphics protocol (Ghostty compatible)
- **Footnotes:** CommonMark footnote rendering
- **HTTP image fetching**
- **Image caching**
- **Code block horizontal scrolling** (upgrade from truncation)
- **Code editing support**
