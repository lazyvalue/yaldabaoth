# Sketch: TUI Markdown Viewer MVP — Design Spec

## Overview

Sketch is a TUI markdown viewer (and eventually editor) built in Rust. The MVP is a **viewer only** — open a markdown file, render it beautifully with full markdown support, navigate with vim-style keybindings, and quit.

The long-term vision is a modal editor with Obsidian-style inline rendering, configurable keybindings, and eventual code editing support. The MVP lays the foundation by nailing rendering quality and establishing the right abstractions.

## Tech Stack

- **ratatui** — TUI framework (layout, rendering, event loop)
- **crossterm** — terminal backend for ratatui
- **pulldown-cmark** — CommonMark-compliant markdown parser
- **syntect** — syntax highlighting for code blocks (bundled syntaxes/themes)
- **image** — image decoding/resizing for inline image support
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
    Table { headers: Vec<StyledLine>, rows: Vec<Vec<StyledLine>>, alignments: Vec<Alignment> },
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
    content: Vec<RenderedBlock>, // recursive — blocks can nest
}
```

- `BlockQuote` and `ListItem` contain `Vec<RenderedBlock>` for natural nesting
- `StyledSpan` carries link URL separate from display style
- `StyledLine` is pre-styled — the view layer just positions, never re-styles

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

## Keybinding System

Keybindings are defined as a map of `Action → Vec<KeySequence>`. The app works entirely in terms of actions, never raw keys.

- Vim defaults ship out of the box
- Multi-key sequences (`gg`, `]]`) are first-class
- Config file can override/extend defaults
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
- `y` — yank current line (or selection in visual mode)

**Visual mode:**
- `v` — enter character visual mode
- `V` — enter line visual mode
- Motion keys extend selection
- Text objects: `ip` (inner paragraph), `ic` (inner code block)
- `y` — yank selection to system clipboard

Text objects are extensible — adding a new one is a function `(cursor, blocks) → (start, end)` range.

## Syntax Highlighting

`syntect` handles code block highlighting:

- Language tag from fenced code blocks selects the syntax
- Highlighted spans map to `StyledSpan` foreground colors
- Code blocks get distinct background + left border accent
- Unrecognized/missing language falls back to plain monospace with code block styling
- All syntaxes and one theme bundled in the binary (~5MB increase, worth it for single-binary distribution)

## Image Rendering

Inline images via the Kitty graphics protocol:

- Image files loaded from local paths (resolved relative to the markdown file's directory)
- Decoded and resized to fit terminal width (maintaining aspect ratio) using the `image` crate
- Encoded as base64, sent via Kitty APC escape sequences
- Terminal support detected via `TERM`/`TERM_PROGRAM` env vars
- Fallback: `[Image: alt text]` styled placeholder when protocol unsupported or image can't load
- No HTTP fetching, no caching in MVP

Primary target terminal: Ghostty (Kitty protocol compatible).

## Scrolling & Viewport

The viewport works in terminal lines, not blocks — blocks have variable height.

- `scroll_offset` tracks position in terminal lines
- On render, walks block list, accumulates heights, determines visible blocks
- Partially visible blocks at top/bottom are clipped

**Line wrapping:**
- Paragraphs soft-wrap at `min(max_line_width, terminal_width)`
- Content is centered horizontally when terminal is wider than `max_line_width`
- Code blocks do NOT wrap — horizontal scroll or truncation with visual indicator
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
sketch -                # read from stdin
```

- File read fully into memory on startup
- Absolute path displayed in top bar
- Non-existent file prints error and exits (no TUI)
- stdin mode reads all input then enters viewer — supports `cat foo.md | sketch`, `gh issue view 123 | sketch`

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

## Future Considerations (Not in MVP)

These are explicitly out of scope for the MVP but the architecture accommodates them:

- **Editing:** Obsidian-style inline rendered editing (cursor reveals raw markdown per block)
- **Raw/rendered toggle:** Switch between edit and preview modes
- **Tree-sitter parser:** Incremental parsing for editing performance
- **File browser / tree sidebar:** Multi-file navigation
- **Custom themes:** User-defined themes in config
- **Dark/light mode toggle**
- **Lua scripting:** Programmable config for complex keybinding logic
- **HTTP image fetching**
- **Image caching**
- **Code editing support**
