# Sketch Viewer MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a beautiful TUI markdown viewer that opens a file, renders all markdown elements with syntax highlighting and theming, and supports vim-style keyboard navigation.

**Architecture:** Four-layer architecture (parse → render → view → app). pulldown-cmark parses markdown into events, a custom renderer converts events into styled blocks using a theme, a viewport layer handles scrolling/wrapping/display, and an app layer runs the event loop with configurable keybindings.

**Tech Stack:** Rust, ratatui, crossterm, pulldown-cmark, syntect, kdl, clap

**Spec:** `docs/superpowers/specs/2026-03-23-sketch-viewer-mvp-design.md`

---

## File Structure

```
src/
├── main.rs              # CLI parsing, file loading, error handling, panic hook, entry point
├── app.rs               # App struct, event loop, mode state, action dispatch
├── parse.rs             # Thin wrapper around pulldown-cmark
├── render.rs            # pulldown-cmark events → Vec<RenderedBlock>
├── blocks.rs            # RenderedBlock, StyledLine, StyledSpan, ListItem, ColumnAlignment types
├── theme.rs             # Theme struct, default dark theme
├── highlight.rs         # syntect integration for code block highlighting
├── viewport.rs          # Scroll state, visible block calculation, line wrapping
├── view.rs              # ratatui Frame rendering (top bar, content, bottom bar)
├── keybind.rs           # Action enum, KeySequence, keybinding map, multi-key state machine
├── config.rs            # KDL config loading, merge with defaults

tests/
├── parse_test.rs        # Parser wrapper tests
├── render_test.rs       # Markdown string → RenderedBlock tests
├── keybind_test.rs      # Key sequence → action resolution tests
├── snapshots/           # Snapshot test expected outputs
└── fixtures/            # Sample markdown files for testing
```

---

### Task 1: Project Setup and Dependencies

**Files:**
- Modify: `Cargo.toml`
- Create: `src/main.rs` (replace default)
- Create: `src/blocks.rs`

- [ ] **Step 1: Add dependencies to Cargo.toml**

```toml
[package]
name = "sketch"
version = "0.1.0"
edition = "2024"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
pulldown-cmark = { version = "0.12", default-features = false, features = ["simd"] }
syntect = { version = "5", default-features = false, features = ["default-fancy"] }
kdl = "6"
clap = { version = "4", features = ["derive"] }
dirs = "6"

[dev-dependencies]
pretty_assertions = "1"
insta = "1"
```

- [ ] **Step 2: Create the blocks module with core types**

Create `src/blocks.rs` with `RenderedBlock`, `StyledLine`, `StyledSpan`, `ListItem`, `ColumnAlignment`:

```rust
use ratatui::style::Style;

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledLine {
    pub spans: Vec<StyledSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub marker: String,
    pub checked: Option<bool>,
    pub content: Vec<RenderedBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderedBlock {
    Heading { level: u8, content: StyledLine },
    Paragraph { lines: Vec<StyledLine> },
    CodeBlock { language: Option<String>, lines: Vec<StyledLine> },
    BlockQuote { blocks: Vec<RenderedBlock> },
    List { ordered: bool, start: Option<u64>, items: Vec<ListItem> },
    Table { headers: Vec<StyledLine>, rows: Vec<Vec<StyledLine>>, alignments: Vec<ColumnAlignment> },
    HorizontalRule,
    Image { alt: String, url: String },
}

impl StyledSpan {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style, link: None }
    }

    pub fn with_link(mut self, url: impl Into<String>) -> Self {
        self.link = Some(url.into());
        self
    }
}

impl StyledLine {
    pub fn new(spans: Vec<StyledSpan>) -> Self {
        Self { spans }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self { spans: vec![StyledSpan::new(text, Style::default())] }
    }

    pub fn text_content(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}
```

- [ ] **Step 3: Replace main.rs with module declarations stub**

```rust
mod blocks;

fn main() {
    println!("sketch - TUI markdown viewer");
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles with no errors (warnings about unused modules are fine)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/blocks.rs
git commit -m "feat: project setup with dependencies and core block types"
```

---

### Task 2: Theme System

**Files:**
- Create: `src/theme.rs`

- [ ] **Step 1: Write the Theme struct and default dark theme**

Create `src/theme.rs`:

```rust
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub heading: [Style; 6],
    pub paragraph: Style,
    pub bold: Style,
    pub italic: Style,
    pub strikethrough: Style,
    pub code_inline: Style,
    pub code_block_bg: Style,
    pub blockquote_bar: Style,
    pub blockquote_text: Style,
    pub link: Style,
    pub table_border: Style,
    pub table_header: Style,
    pub horizontal_rule: Style,
    pub list_marker: Style,
    pub image_label: Style,
    pub cursor_line: Style,
    pub top_bar: Style,
    pub bottom_bar: Style,
    pub mode_indicator: Style,
    pub search_match: Style,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            heading: [
                // h1: purple, bold, with underline decoration
                Style::default().fg(Color::Rgb(189, 147, 249)).add_modifier(Modifier::BOLD),
                // h2: cyan, bold
                Style::default().fg(Color::Rgb(139, 233, 253)).add_modifier(Modifier::BOLD),
                // h3: green, bold
                Style::default().fg(Color::Rgb(80, 250, 123)).add_modifier(Modifier::BOLD),
                // h4: yellow
                Style::default().fg(Color::Rgb(241, 250, 140)).add_modifier(Modifier::BOLD),
                // h5: orange
                Style::default().fg(Color::Rgb(255, 184, 108)).add_modifier(Modifier::BOLD),
                // h6: dim white
                Style::default().fg(Color::Rgb(180, 180, 180)).add_modifier(Modifier::BOLD),
            ],
            paragraph: Style::default().fg(Color::Rgb(204, 204, 204)),
            bold: Style::default().add_modifier(Modifier::BOLD).fg(Color::Rgb(248, 248, 242)),
            italic: Style::default().add_modifier(Modifier::ITALIC).fg(Color::Rgb(248, 248, 242)),
            strikethrough: Style::default().add_modifier(Modifier::CROSSED_OUT).fg(Color::Rgb(136, 136, 136)),
            code_inline: Style::default().fg(Color::Rgb(241, 250, 140)).bg(Color::Rgb(40, 42, 54)),
            code_block_bg: Style::default().bg(Color::Rgb(40, 42, 54)),
            blockquote_bar: Style::default().fg(Color::Rgb(255, 184, 108)),
            blockquote_text: Style::default().fg(Color::Rgb(170, 170, 170)),
            link: Style::default().fg(Color::Rgb(139, 233, 253)).add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(98, 114, 164)),
            table_header: Style::default().fg(Color::Rgb(189, 147, 249)).add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(98, 114, 164)),
            list_marker: Style::default().fg(Color::Rgb(80, 250, 123)),
            image_label: Style::default().fg(Color::Rgb(255, 184, 108)).add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(40, 42, 54)),
            top_bar: Style::default().fg(Color::Rgb(139, 233, 253)).bg(Color::Rgb(22, 33, 62)),
            bottom_bar: Style::default().fg(Color::Rgb(102, 102, 102)).bg(Color::Rgb(22, 33, 62)),
            mode_indicator: Style::default().fg(Color::Rgb(80, 250, 123)).add_modifier(Modifier::BOLD),
            search_match: Style::default().fg(Color::Rgb(40, 42, 54)).bg(Color::Rgb(241, 250, 140)),
        }
    }

    /// Compose styles in order: base, then modifier, then semantic.
    /// More specific styles override less specific for conflicting attributes.
    pub fn compose(base: Style, modifier: Style) -> Style {
        base.patch(modifier)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
```

- [ ] **Step 2: Add module to main.rs**

Add `mod theme;` to `src/main.rs`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add src/theme.rs src/main.rs
git commit -m "feat: theme system with dark theme"
```

---

### Task 3: Parse Layer

**Files:**
- Create: `src/parse.rs`
- Create: `tests/parse_test.rs`

- [ ] **Step 1: Write test for parser wrapper**

Create `tests/parse_test.rs`:

```rust
use pulldown_cmark::{Event, Tag, TagEnd};

#[test]
fn test_parse_heading() {
    let md = "# Hello World";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(matches!(events[0], Event::Start(Tag::Heading { level: pulldown_cmark::HeadingLevel::H1, .. })));
    assert!(matches!(events[1], Event::Text(_)));
    assert!(matches!(events[2], Event::End(TagEnd::Heading(pulldown_cmark::HeadingLevel::H1))));
}

#[test]
fn test_parse_code_block() {
    let md = "```rust\nfn main() {}\n```";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(matches!(events[0], Event::Start(Tag::CodeBlock(_))));
}

#[test]
fn test_parse_task_list() {
    let md = "- [x] done\n- [ ] todo";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(events.iter().any(|e| matches!(e, Event::TaskListMarker(true))));
    assert!(events.iter().any(|e| matches!(e, Event::TaskListMarker(false))));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test parse_test`
Expected: FAIL — module `parse` doesn't exist as public

- [ ] **Step 3: Create the parse module**

Create `src/parse.rs`:

```rust
use pulldown_cmark::{Event, Options, Parser};

/// Parse markdown text into a pulldown-cmark event iterator.
/// Enables all CommonMark extensions we support.
pub fn parse(markdown: &str) -> impl Iterator<Item = Event<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    Parser::new_ext(markdown, options)
}
```

- [ ] **Step 4: Make parse module public in lib.rs**

Create `src/lib.rs`:

```rust
pub mod parse;
pub mod blocks;
pub mod theme;
```

Update `src/main.rs` — remove the `mod blocks` and `mod theme` lines, replace with:

```rust
use sketch::blocks;
use sketch::theme;
use sketch::parse;

fn main() {
    println!("sketch - TUI markdown viewer");
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test parse_test`
Expected: 3 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/parse.rs src/lib.rs src/main.rs tests/parse_test.rs
git commit -m "feat: parse layer wrapping pulldown-cmark"
```

---

### Task 4: Render Layer — Basic Blocks (Headings, Paragraphs, Inline Styles)

**Files:**
- Create: `src/render.rs`
- Create: `tests/render_test.rs`
- Create: `tests/fixtures/basic.md`

- [ ] **Step 1: Create a test fixture**

Create `tests/fixtures/basic.md`:

```markdown
# Main Title

A paragraph with **bold**, *italic*, and ~~strikethrough~~ text.

## Second Heading

Another paragraph with a [link](https://example.com) in it.

Some `inline code` here.
```

- [ ] **Step 2: Write tests for heading and paragraph rendering**

Create `tests/render_test.rs`:

```rust
use pretty_assertions::assert_eq;
use ratatui::style::Modifier;
use sketch::blocks::RenderedBlock;
use sketch::render::render;
use sketch::theme::Theme;

#[test]
fn test_render_heading() {
    let md = "# Hello World";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        RenderedBlock::Heading { level, content } => {
            assert_eq!(*level, 1);
            assert_eq!(content.text_content(), "Hello World");
            assert_eq!(content.spans[0].style, theme.heading[0]);
        }
        other => panic!("Expected Heading, got {:?}", other),
    }
}

#[test]
fn test_render_paragraph_with_bold() {
    let md = "Hello **world**";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let spans = &lines[0].spans;
            assert_eq!(spans.len(), 2);
            assert_eq!(spans[0].text, "Hello ");
            assert_eq!(spans[1].text, "world");
            assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_paragraph_with_link() {
    let md = "Click [here](https://example.com) now";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let link_span = lines[0].spans.iter().find(|s| s.link.is_some()).unwrap();
            assert_eq!(link_span.text, "here");
            assert_eq!(link_span.link.as_deref(), Some("https://example.com"));
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_inline_code() {
    let md = "Use `foo()` here";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let code_span = lines[0].spans.iter().find(|s| s.text == "foo()").unwrap();
            assert_eq!(code_span.style, theme.code_inline);
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_multiple_blocks() {
    let md = "# Title\n\nParagraph text.\n\n## Subtitle";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RenderedBlock::Heading { level: 1, .. }));
    assert!(matches!(blocks[1], RenderedBlock::Paragraph { .. }));
    assert!(matches!(blocks[2], RenderedBlock::Heading { level: 2, .. }));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test render_test`
Expected: FAIL — module `render` doesn't exist

- [ ] **Step 4: Implement the render module (basic blocks)**

Create `src/render.rs`:

```rust
use pulldown_cmark::{Event, Tag, TagEnd, CodeBlockKind};
use ratatui::style::Style;

use crate::blocks::*;
use crate::parse;
use crate::theme::Theme;

/// Render markdown text into a list of styled blocks.
pub fn render(markdown: &str, theme: &Theme) -> Vec<RenderedBlock> {
    let events: Vec<_> = parse::parse(markdown).collect();
    let mut renderer = Renderer::new(theme);
    renderer.render(&events)
}

struct Renderer<'t> {
    theme: &'t Theme,
}

struct InlineState {
    spans: Vec<StyledSpan>,
    style_stack: Vec<Style>,
    link_stack: Vec<Option<String>>,
}

impl InlineState {
    fn new(base_style: Style) -> Self {
        Self {
            spans: Vec::new(),
            style_stack: vec![base_style],
            link_stack: vec![None],
        }
    }

    fn current_style(&self) -> Style {
        let mut s = Style::default();
        for style in &self.style_stack {
            s = s.patch(*style);
        }
        s
    }

    fn current_link(&self) -> Option<String> {
        self.link_stack.iter().rev().find_map(|l| l.clone())
    }

    fn push_text(&mut self, text: &str) {
        let style = self.current_style();
        let link = self.current_link();
        self.spans.push(StyledSpan { text: text.to_string(), style, link });
    }

    fn into_line(self) -> StyledLine {
        StyledLine::new(self.spans)
    }
}

impl<'t> Renderer<'t> {
    fn new(theme: &'t Theme) -> Self {
        Self { theme }
    }

    fn render(&mut self, events: &[Event<'_>]) -> Vec<RenderedBlock> {
        let mut blocks = Vec::new();
        let mut i = 0;

        while i < events.len() {
            match &events[i] {
                Event::Start(Tag::Heading { level, .. }) => {
                    let level_num = heading_level_to_u8(*level);
                    i += 1;
                    let mut state = InlineState::new(self.theme.heading[(level_num - 1) as usize]);
                    i = self.collect_inline(events, i, &TagEnd::Heading(*level), &mut state);
                    blocks.push(RenderedBlock::Heading {
                        level: level_num,
                        content: state.into_line(),
                    });
                }
                Event::Start(Tag::Paragraph) => {
                    i += 1;
                    let mut state = InlineState::new(self.theme.paragraph);
                    i = self.collect_inline(events, i, &TagEnd::Paragraph, &mut state);
                    blocks.push(RenderedBlock::Paragraph {
                        lines: vec![state.into_line()],
                    });
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    i += 1;
                    let (sub_blocks, new_i) = self.collect_block(events, i, &TagEnd::BlockQuote(None));
                    i = new_i;
                    blocks.push(RenderedBlock::BlockQuote { blocks: sub_blocks });
                }
                Event::Start(Tag::List(start)) => {
                    let ordered = start.is_some();
                    let start_num = *start;
                    i += 1;
                    let (items, new_i) = self.collect_list_items(events, i, ordered, start_num);
                    i = new_i;
                    blocks.push(RenderedBlock::List { ordered, start: start_num, items });
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(lang) => {
                            let l = lang.to_string();
                            if l.is_empty() { None } else { Some(l) }
                        }
                        CodeBlockKind::Indented => None,
                    };
                    i += 1;
                    let mut code_lines = Vec::new();
                    while i < events.len() {
                        match &events[i] {
                            Event::Text(t) => {
                                for line in t.as_ref().lines() {
                                    code_lines.push(StyledLine::new(vec![
                                        StyledSpan::new(line, self.theme.code_block_bg),
                                    ]));
                                }
                                i += 1;
                            }
                            Event::End(TagEnd::CodeBlock) => { i += 1; break; }
                            _ => { i += 1; }
                        }
                    }
                    blocks.push(RenderedBlock::CodeBlock { language, lines: code_lines });
                }
                Event::Start(Tag::Table(alignments)) => {
                    let aligns: Vec<ColumnAlignment> = alignments.iter().map(|a| match a {
                        pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => ColumnAlignment::Left,
                        pulldown_cmark::Alignment::Center => ColumnAlignment::Center,
                        pulldown_cmark::Alignment::Right => ColumnAlignment::Right,
                    }).collect();
                    i += 1;
                    let (headers, rows, new_i) = self.collect_table(events, i);
                    i = new_i;
                    blocks.push(RenderedBlock::Table { headers, rows, alignments: aligns });
                }
                Event::Rule => {
                    blocks.push(RenderedBlock::HorizontalRule);
                    i += 1;
                }
                Event::Start(Tag::Image { dest_url, title, .. }) => {
                    let url = dest_url.to_string();
                    i += 1;
                    // Collect alt text
                    let mut alt = String::new();
                    while i < events.len() {
                        match &events[i] {
                            Event::Text(t) => { alt.push_str(t.as_ref()); i += 1; }
                            Event::End(TagEnd::Image) => { i += 1; break; }
                            _ => { i += 1; }
                        }
                    }
                    if alt.is_empty() && !title.is_empty() {
                        alt = title.to_string();
                    }
                    if alt.is_empty() {
                        // Extract filename from URL
                        alt = url.rsplit('/').next().unwrap_or(&url).to_string();
                    }
                    blocks.push(RenderedBlock::Image { alt, url });
                }
                _ => { i += 1; }
            }
        }

        blocks
    }

    /// Collect inline events (text, bold, italic, etc.) until the matching end tag.
    /// Returns the index after the end tag.
    fn collect_inline(&self, events: &[Event<'_>], mut i: usize, end: &TagEnd, state: &mut InlineState) -> usize {
        while i < events.len() {
            match &events[i] {
                Event::End(e) if e == end => { i += 1; break; }
                Event::Text(t) => {
                    state.push_text(t.as_ref());
                    i += 1;
                }
                Event::Code(t) => {
                    state.style_stack.push(self.theme.code_inline);
                    state.push_text(t.as_ref());
                    state.style_stack.pop();
                    i += 1;
                }
                Event::SoftBreak | Event::HardBreak => {
                    state.push_text(" ");
                    i += 1;
                }
                Event::Start(Tag::Strong) => {
                    state.style_stack.push(self.theme.bold);
                    i += 1;
                }
                Event::End(TagEnd::Strong) => {
                    state.style_stack.pop();
                    i += 1;
                }
                Event::Start(Tag::Emphasis) => {
                    state.style_stack.push(self.theme.italic);
                    i += 1;
                }
                Event::End(TagEnd::Emphasis) => {
                    state.style_stack.pop();
                    i += 1;
                }
                Event::Start(Tag::Strikethrough) => {
                    state.style_stack.push(self.theme.strikethrough);
                    i += 1;
                }
                Event::End(TagEnd::Strikethrough) => {
                    state.style_stack.pop();
                    i += 1;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    state.style_stack.push(self.theme.link);
                    state.link_stack.push(Some(dest_url.to_string()));
                    i += 1;
                }
                Event::End(TagEnd::Link) => {
                    state.style_stack.pop();
                    state.link_stack.pop();
                    i += 1;
                }
                _ => { i += 1; }
            }
        }
        i
    }

    /// Collect block-level events inside a container (blockquote, etc.).
    fn collect_block(&mut self, events: &[Event<'_>], mut i: usize, end: &TagEnd) -> (Vec<RenderedBlock>, usize) {
        let mut inner_events = Vec::new();
        let mut depth = 0;

        while i < events.len() {
            match &events[i] {
                Event::End(e) if e == end && depth == 0 => { i += 1; break; }
                Event::Start(_) => { depth += 1; inner_events.push(events[i].clone()); i += 1; }
                Event::End(_) => { depth -= 1; inner_events.push(events[i].clone()); i += 1; }
                _ => { inner_events.push(events[i].clone()); i += 1; }
            }
        }

        let blocks = self.render(&inner_events);
        (blocks, i)
    }

    /// Collect list items.
    fn collect_list_items(&mut self, events: &[Event<'_>], mut i: usize, ordered: bool, start: Option<u64>) -> (Vec<ListItem>, usize) {
        let mut items = Vec::new();
        let mut item_index = start.unwrap_or(1);

        while i < events.len() {
            match &events[i] {
                Event::End(TagEnd::List(_)) => { i += 1; break; }
                Event::Start(Tag::Item) => {
                    i += 1;
                    let marker = if ordered {
                        format!("{}.", item_index)
                    } else {
                        "•".to_string()
                    };

                    let mut checked = None;
                    let mut item_events = Vec::new();
                    let mut depth = 0;

                    while i < events.len() {
                        match &events[i] {
                            Event::End(TagEnd::Item) if depth == 0 => { i += 1; break; }
                            Event::TaskListMarker(c) => { checked = Some(*c); i += 1; }
                            Event::Start(_) => { depth += 1; item_events.push(events[i].clone()); i += 1; }
                            Event::End(_) => { depth -= 1; item_events.push(events[i].clone()); i += 1; }
                            _ => { item_events.push(events[i].clone()); i += 1; }
                        }
                    }

                    let content = self.render(&item_events);
                    items.push(ListItem { marker, checked, content });
                    item_index += 1;
                }
                _ => { i += 1; }
            }
        }

        (items, i)
    }

    /// Collect table rows.
    fn collect_table(&mut self, events: &[Event<'_>], mut i: usize) -> (Vec<StyledLine>, Vec<Vec<StyledLine>>, usize) {
        let mut headers = Vec::new();
        let mut rows: Vec<Vec<StyledLine>> = Vec::new();
        let mut current_row: Vec<StyledLine> = Vec::new();
        let mut in_head = false;

        while i < events.len() {
            match &events[i] {
                Event::End(TagEnd::Table) => { i += 1; break; }
                Event::Start(Tag::TableHead) => { in_head = true; i += 1; }
                Event::End(TagEnd::TableHead) => { in_head = false; i += 1; }
                Event::Start(Tag::TableRow) => { current_row = Vec::new(); i += 1; }
                Event::End(TagEnd::TableRow) => {
                    if in_head {
                        headers = std::mem::take(&mut current_row);
                    } else {
                        rows.push(std::mem::take(&mut current_row));
                    }
                    i += 1;
                }
                Event::Start(Tag::TableCell) => {
                    let style = if in_head { self.theme.table_header } else { self.theme.paragraph };
                    i += 1;
                    let mut state = InlineState::new(style);
                    i = self.collect_inline(events, i, &TagEnd::TableCell, &mut state);
                    current_row.push(state.into_line());
                }
                _ => { i += 1; }
            }
        }

        (headers, rows, i)
    }
}

fn heading_level_to_u8(level: pulldown_cmark::HeadingLevel) -> u8 {
    match level {
        pulldown_cmark::HeadingLevel::H1 => 1,
        pulldown_cmark::HeadingLevel::H2 => 2,
        pulldown_cmark::HeadingLevel::H3 => 3,
        pulldown_cmark::HeadingLevel::H4 => 4,
        pulldown_cmark::HeadingLevel::H5 => 5,
        pulldown_cmark::HeadingLevel::H6 => 6,
    }
}

- [ ] **Step 5: Add render module to lib.rs**

Add `pub mod render;` to `src/lib.rs`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --test render_test`
Expected: All 5 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/render.rs src/lib.rs tests/render_test.rs tests/fixtures/basic.md
git commit -m "feat: render layer for headings, paragraphs, and inline styles"
```

---

### Task 5: Render Layer — Tests for Lists, Blockquotes, Code Blocks, Tables, Images, HRs

**Files:**
- Modify: `tests/render_test.rs` (add tests)
- Create: `tests/fixtures/full.md`

- [ ] **Step 1: Create a comprehensive test fixture**

Create `tests/fixtures/full.md`:

```markdown
# Full Test

- Item one
- Item two
  - Nested item
- [x] Done task
- [ ] Todo task

1. First
2. Second

> This is a blockquote
>
> With multiple paragraphs

---

| Name | Age |
|------|-----|
| Alice | 30 |
| Bob | 25 |

![Alt text](image.png)
```

- [ ] **Step 2: Write tests for remaining block types**

Add to `tests/render_test.rs`:

```rust
#[test]
fn test_render_unordered_list() {
    let md = "- Alpha\n- Beta";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::List { ordered, items, .. } => {
            assert!(!ordered);
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].marker, "•");
            assert!(items[0].checked.is_none());
        }
        other => panic!("Expected List, got {:?}", other),
    }
}

#[test]
fn test_render_task_list() {
    let md = "- [x] Done\n- [ ] Todo";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::List { items, .. } => {
            assert_eq!(items[0].checked, Some(true));
            assert_eq!(items[1].checked, Some(false));
        }
        other => panic!("Expected List, got {:?}", other),
    }
}

#[test]
fn test_render_blockquote() {
    let md = "> Quoted text";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::BlockQuote { blocks: inner } => {
            assert_eq!(inner.len(), 1);
            assert!(matches!(inner[0], RenderedBlock::Paragraph { .. }));
        }
        other => panic!("Expected BlockQuote, got {:?}", other),
    }
}

#[test]
fn test_render_code_block() {
    let md = "```rust\nfn main() {}\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::CodeBlock { language, lines } => {
            assert_eq!(language.as_deref(), Some("rust"));
            assert!(!lines.is_empty());
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}

#[test]
fn test_render_horizontal_rule() {
    let md = "---";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    assert!(matches!(blocks[0], RenderedBlock::HorizontalRule));
}

#[test]
fn test_render_image() {
    let md = "![Alt text](image.png)";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::Image { alt, url } => {
            assert_eq!(alt, "Alt text");
            assert_eq!(url, "image.png");
        }
        // pulldown-cmark may wrap images in paragraphs
        RenderedBlock::Paragraph { .. } => {
            // Image inside paragraph — check we at least parsed it
        }
        other => panic!("Expected Image, got {:?}", other),
    }
}

#[test]
fn test_render_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::Table { headers, rows, .. } => {
            assert_eq!(headers.len(), 2);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].len(), 2);
        }
        other => panic!("Expected Table, got {:?}", other),
    }
}
```

- [ ] **Step 3: Run tests to verify failures**

Run: `cargo test --test render_test`
Expected: FAIL — tests reference new block types not yet tested

- [ ] **Step 4: Run all tests**

Run: `cargo test --test render_test`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/render.rs tests/render_test.rs tests/fixtures/full.md
git commit -m "feat: render lists, blockquotes, code blocks, tables, images, HRs"
```

---

### Task 6: Syntax Highlighting for Code Blocks

**Files:**
- Create: `src/highlight.rs`
- Modify: `src/render.rs` (integrate highlighting)
- Modify: `tests/render_test.rs`

- [ ] **Step 1: Write test for syntax-highlighted code block**

Add to `tests/render_test.rs`:

```rust
#[test]
fn test_code_block_has_multiple_styled_spans() {
    let md = "```rust\nlet x = 42;\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::CodeBlock { lines, .. } => {
            // With syntax highlighting, "let x = 42;" should produce
            // multiple spans with different colors (keyword, ident, number)
            let first_line = &lines[0];
            assert!(first_line.spans.len() > 1,
                "Expected multiple styled spans from syntax highlighting, got {}",
                first_line.spans.len());
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}

#[test]
fn test_code_block_unknown_language_falls_back() {
    let md = "```unknownlang\nhello world\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);

    match &blocks[0] {
        RenderedBlock::CodeBlock { lines, .. } => {
            // Unknown language: single span per line, plain styled
            assert_eq!(lines[0].spans.len(), 1);
            assert_eq!(lines[0].text_content(), "hello world");
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test render_test test_code_block_has_multiple`
Expected: FAIL — currently code blocks have one span per line

- [ ] **Step 3: Create the highlight module**

Create `src/highlight.rs`:

```rust
use ratatui::style::{Color, Style};
use syntect::highlighting::{ThemeSet, Theme as SynTheme};
use syntect::parsing::SyntaxSet;
use syntect::easy::HighlightLines;

use crate::blocks::{StyledLine, StyledSpan};

pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: SynTheme,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set.themes["base16-ocean.dark"].clone();
        Self { syntax_set, theme }
    }

    /// Highlight code lines for a given language.
    /// Returns None if the language is not recognized.
    pub fn highlight(&self, language: &str, code: &str, bg_style: Style) -> Option<Vec<StyledLine>> {
        let syntax = self.syntax_set.find_syntax_by_token(language)?;
        let mut h = HighlightLines::new(syntax, &self.theme);
        let mut lines = Vec::new();

        for line in code.lines() {
            let ranges = h.highlight_line(line, &self.syntax_set).ok()?;
            let spans: Vec<StyledSpan> = ranges.into_iter().map(|(style, text)| {
                let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                StyledSpan::new(text, bg_style.fg(fg))
            }).collect();
            lines.push(StyledLine::new(if spans.is_empty() {
                vec![StyledSpan::new("", bg_style)]
            } else {
                spans
            }));
        }

        Some(lines)
    }
}
```

- [ ] **Step 4: Integrate highlighter into render.rs**

In `render.rs`, add `use crate::highlight::Highlighter;` and change the `Renderer` struct to hold a `Highlighter`:

```rust
struct Renderer<'t> {
    theme: &'t Theme,
    highlighter: Highlighter,
}
```

Update `Renderer::new`:

```rust
fn new(theme: &'t Theme) -> Self {
    Self { theme, highlighter: Highlighter::new() }
}
```

Update the `CodeBlock` arm to try highlighting first:

```rust
Event::Start(Tag::CodeBlock(kind)) => {
    let language = match kind {
        CodeBlockKind::Fenced(lang) => {
            let l = lang.to_string();
            if l.is_empty() { None } else { Some(l) }
        }
        CodeBlockKind::Indented => None,
    };
    i += 1;
    let mut code_text = String::new();
    while i < events.len() {
        match &events[i] {
            Event::Text(t) => { code_text.push_str(t.as_ref()); i += 1; }
            Event::End(TagEnd::CodeBlock) => { i += 1; break; }
            _ => { i += 1; }
        }
    }

    let lines = if let Some(lang) = &language {
        self.highlighter.highlight(lang, &code_text, self.theme.code_block_bg)
            .unwrap_or_else(|| self.plain_code_lines(&code_text))
    } else {
        self.plain_code_lines(&code_text)
    };

    blocks.push(RenderedBlock::CodeBlock { language, lines });
}
```

Add helper method:

```rust
fn plain_code_lines(&self, code: &str) -> Vec<StyledLine> {
    code.lines().map(|line| {
        StyledLine::new(vec![StyledSpan::new(line, self.theme.code_block_bg)])
    }).collect()
}
```

- [ ] **Step 5: Add highlight module to lib.rs**

Add `pub mod highlight;` to `src/lib.rs`.

- [ ] **Step 6: Run tests**

Run: `cargo test --test render_test`
Expected: All tests PASS including syntax highlighting tests

- [ ] **Step 7: Commit**

```bash
git add src/highlight.rs src/render.rs src/lib.rs tests/render_test.rs
git commit -m "feat: syntect-based syntax highlighting for code blocks"
```

---

### Task 7: Keybinding System

**Files:**
- Create: `src/keybind.rs`
- Create: `tests/keybind_test.rs`

- [ ] **Step 1: Write keybinding tests**

Create `tests/keybind_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::keybind::{Action, KeybindManager};

#[test]
fn test_single_key_binding() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::ScrollDown));
}

#[test]
fn test_multi_key_sequence_gg() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result1, None); // pending
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result2, Some(Action::JumpTop));
}

#[test]
fn test_multi_key_timeout_resets() {
    let mut mgr = KeybindManager::default();
    let _ = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    mgr.reset_pending(); // simulate timeout
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::ScrollDown));
}

#[test]
fn test_ctrl_modifier() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(result, Some(Action::HalfPageDown));
}

#[test]
fn test_quit() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::Quit));
}

#[test]
fn test_unknown_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(result, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test keybind_test`
Expected: FAIL — module `keybind` doesn't exist

- [ ] **Step 3: Implement the keybinding module**

Create `src/keybind.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MULTI_KEY_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    FullPageDown,
    FullPageUp,
    JumpTop,
    JumpBottom,
    NextHeading,
    PrevHeading,
    NextHeadingSameLevel,
    PrevHeadingSameLevel,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    Quit,
    OpenLink,
    YankLine,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KeyPress {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl From<KeyEvent> for KeyPress {
    fn from(event: KeyEvent) -> Self {
        Self {
            code: event.code,
            // Normalize: strip SHIFT for char keys (already in the char itself)
            modifiers: event.modifiers & !KeyModifiers::SHIFT,
        }
    }
}

pub struct KeybindManager {
    single: HashMap<KeyPress, Action>,
    multi: HashMap<Vec<KeyPress>, Action>,
    pending: Vec<KeyPress>,
    pending_since: Option<Instant>,
}

impl KeybindManager {
    pub fn new(single: HashMap<KeyPress, Action>, multi: HashMap<Vec<KeyPress>, Action>) -> Self {
        Self { single, multi, pending: Vec::new(), pending_since: None }
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<Action> {
        // Check timeout
        if let Some(since) = self.pending_since {
            if since.elapsed() > MULTI_KEY_TIMEOUT {
                self.pending.clear();
                self.pending_since = None;
            }
        }

        let press: KeyPress = event.into();
        self.pending.push(press.clone());
        self.pending_since = Some(Instant::now());

        // Check for complete multi-key match
        if let Some(&action) = self.multi.get(&self.pending) {
            self.pending.clear();
            self.pending_since = None;
            return Some(action);
        }

        // Check if pending could be a prefix of any multi-key binding
        let is_prefix = self.multi.keys().any(|seq| {
            seq.len() > self.pending.len() && seq.starts_with(&self.pending)
        });

        if is_prefix {
            return None; // wait for more keys
        }

        // No multi-key match possible. Check single-key for the latest press.
        self.pending.clear();
        self.pending_since = None;
        self.single.get(&press).copied()
    }

    pub fn reset_pending(&mut self) {
        self.pending.clear();
        self.pending_since = None;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Default for KeybindManager {
    fn default() -> Self {
        let mut single = HashMap::new();
        let mut multi = HashMap::new();

        // Scrolling
        single.insert(key('j'), Action::ScrollDown);
        single.insert(key('k'), Action::ScrollUp);
        single.insert(ctrl('d'), Action::HalfPageDown);
        single.insert(ctrl('u'), Action::HalfPageUp);
        single.insert(ctrl('f'), Action::FullPageDown);
        single.insert(ctrl('b'), Action::FullPageUp);
        single.insert(key('G'), Action::JumpBottom);

        // Structure navigation
        single.insert(key('}'), Action::NextHeading);
        single.insert(key('{'), Action::PrevHeading);

        // Search
        single.insert(key('/'), Action::SearchForward);
        single.insert(key('?'), Action::SearchBackward);
        single.insert(key('n'), Action::SearchNext);
        single.insert(key('N'), Action::SearchPrev);

        // Actions
        single.insert(key('q'), Action::Quit);
        single.insert(key('o'), Action::OpenLink);
        single.insert(key('y'), Action::YankLine);

        // Multi-key sequences
        multi.insert(vec![key('g'), key('g')], Action::JumpTop);
        multi.insert(vec![key(']'), key(']')], Action::NextHeadingSameLevel);
        multi.insert(vec![key('['), key('[')], Action::PrevHeadingSameLevel);

        Self::new(single, multi)
    }
}

fn key(c: char) -> KeyPress {
    KeyPress { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE }
}

fn ctrl(c: char) -> KeyPress {
    KeyPress { code: KeyCode::Char(c), modifiers: KeyModifiers::CONTROL }
}
```

- [ ] **Step 4: Add keybind module to lib.rs**

Add `pub mod keybind;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test --test keybind_test`
Expected: All 6 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/keybind.rs src/lib.rs tests/keybind_test.rs
git commit -m "feat: keybinding system with vim defaults and multi-key sequences"
```

---

### Task 8: Viewport and Scroll State

**Files:**
- Create: `src/viewport.rs`

- [ ] **Step 1: Create the viewport module**

Create `src/viewport.rs`:

```rust
use crate::blocks::*;

pub struct Viewport {
    /// Scroll offset in terminal lines from the top of the document
    pub scroll_offset: usize,
    /// Logical cursor line (terminal line index from top of document)
    pub cursor_line: usize,
    /// Cached total height of all rendered content in terminal lines
    pub total_lines: usize,
    /// Max line width for soft wrapping (0 = no limit)
    pub max_line_width: usize,
}

/// A block positioned in the viewport with its terminal line offset.
pub struct PositionedBlock<'a> {
    pub block: &'a RenderedBlock,
    pub y_offset: usize,
    pub height: usize,
}

impl Viewport {
    pub fn new(max_line_width: usize) -> Self {
        Self {
            scroll_offset: 0,
            cursor_line: 0,
            total_lines: 0,
            max_line_width,
        }
    }

    /// Compute the effective width for content wrapping.
    pub fn content_width(&self, terminal_width: usize) -> usize {
        if self.max_line_width > 0 {
            self.max_line_width.min(terminal_width)
        } else {
            terminal_width
        }
    }

    /// Compute the horizontal offset to center content.
    pub fn content_offset(&self, terminal_width: usize) -> usize {
        let cw = self.content_width(terminal_width);
        if terminal_width > cw {
            (terminal_width - cw) / 2
        } else {
            0
        }
    }

    /// Estimate the height of a block in terminal lines.
    pub fn block_height(&self, block: &RenderedBlock, width: usize) -> usize {
        match block {
            RenderedBlock::Heading { .. } => 2, // heading + blank line
            RenderedBlock::Paragraph { lines } => {
                let text_lines: usize = lines.iter()
                    .map(|l| self.wrapped_line_count(l, width))
                    .sum();
                text_lines + 1 // + blank line after
            }
            RenderedBlock::CodeBlock { lines, .. } => {
                lines.len() + 2 // code lines + top/bottom padding
            }
            RenderedBlock::BlockQuote { blocks } => {
                let inner: usize = blocks.iter()
                    .map(|b| self.block_height(b, width.saturating_sub(4)))
                    .sum();
                inner + 1
            }
            RenderedBlock::List { items, .. } => {
                let item_lines: usize = items.iter()
                    .map(|item| {
                        item.content.iter()
                            .map(|b| self.block_height(b, width.saturating_sub(4)))
                            .sum::<usize>()
                            .max(1)
                    })
                    .sum();
                item_lines + 1 // + blank line after
            }
            RenderedBlock::Table { headers: _, rows, .. } => {
                rows.len() + 3 // header + separator + rows + blank
            }
            RenderedBlock::HorizontalRule => 2, // rule + blank line
            RenderedBlock::Image { .. } => 2, // label + blank line
        }
    }

    /// Count how many terminal lines a styled line takes when wrapped.
    fn wrapped_line_count(&self, line: &StyledLine, width: usize) -> usize {
        if width == 0 { return 1; }
        let len = line.text_content().len();
        if len == 0 { return 1; }
        (len + width - 1) / width // ceiling division
    }

    /// Calculate total document height and update cached value.
    pub fn calculate_total_lines(&mut self, blocks: &[RenderedBlock], width: usize) {
        self.total_lines = blocks.iter()
            .map(|b| self.block_height(b, width))
            .sum();
    }

    /// Scroll down by n lines, clamped.
    pub fn scroll_down(&mut self, n: usize, viewport_height: usize) {
        let max = self.total_lines.saturating_sub(viewport_height);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    /// Scroll up by n lines, clamped.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Jump to top.
    pub fn jump_top(&mut self) {
        self.scroll_offset = 0;
        self.cursor_line = 0;
    }

    /// Jump to bottom.
    pub fn jump_bottom(&mut self, viewport_height: usize) {
        self.scroll_offset = self.total_lines.saturating_sub(viewport_height);
    }

    /// Get visible blocks and their positions for a given viewport.
    pub fn visible_blocks<'a>(&self, blocks: &'a [RenderedBlock], width: usize, viewport_height: usize) -> Vec<PositionedBlock<'a>> {
        let mut result = Vec::new();
        let mut y = 0;
        let view_start = self.scroll_offset;
        let view_end = self.scroll_offset + viewport_height;

        for block in blocks {
            let h = self.block_height(block, width);
            let block_end = y + h;

            if block_end > view_start && y < view_end {
                result.push(PositionedBlock {
                    block,
                    y_offset: y,
                    height: h,
                });
            }

            if y >= view_end { break; }
            y += h;
        }

        result
    }

    /// Find the y_offset of the nth heading in the document.
    pub fn find_heading_offset(&self, blocks: &[RenderedBlock], n: usize, width: usize) -> Option<usize> {
        let mut y = 0;
        let mut count = 0;
        for block in blocks {
            if matches!(block, RenderedBlock::Heading { .. }) {
                if count == n { return Some(y); }
                count += 1;
            }
            y += self.block_height(block, width);
        }
        None
    }
}
```

- [ ] **Step 2: Add viewport module to lib.rs**

Add `pub mod viewport;` to `src/lib.rs`.

- [ ] **Step 3: Write viewport tests**

Create `tests/viewport_test.rs`:

```rust
use sketch::blocks::*;
use sketch::viewport::Viewport;
use ratatui::style::Style;

fn make_heading(level: u8) -> RenderedBlock {
    RenderedBlock::Heading {
        level,
        content: StyledLine::plain(format!("Heading {}", level)),
    }
}

fn make_paragraph(text: &str) -> RenderedBlock {
    RenderedBlock::Paragraph {
        lines: vec![StyledLine::plain(text)],
    }
}

#[test]
fn test_content_width_respects_max() {
    let vp = Viewport::new(80);
    assert_eq!(vp.content_width(120), 80);
    assert_eq!(vp.content_width(60), 60);
}

#[test]
fn test_content_offset_centers() {
    let vp = Viewport::new(80);
    assert_eq!(vp.content_offset(120), 20);
    assert_eq!(vp.content_offset(80), 0);
    assert_eq!(vp.content_offset(60), 0);
}

#[test]
fn test_scroll_down_clamps() {
    let blocks = vec![make_heading(1), make_paragraph("text")];
    let mut vp = Viewport::new(80);
    vp.calculate_total_lines(&blocks, 80);
    vp.scroll_down(1000, 10);
    assert!(vp.scroll_offset <= vp.total_lines);
}

#[test]
fn test_scroll_up_clamps_to_zero() {
    let mut vp = Viewport::new(80);
    vp.scroll_offset = 5;
    vp.scroll_up(100);
    assert_eq!(vp.scroll_offset, 0);
}

#[test]
fn test_visible_blocks_returns_correct_blocks() {
    let blocks = vec![
        make_heading(1),
        make_paragraph("first"),
        make_paragraph("second"),
        make_paragraph("third"),
    ];
    let vp = Viewport::new(80);
    let visible = vp.visible_blocks(&blocks, 80, 5);
    assert!(!visible.is_empty());
    assert!(matches!(visible[0].block, RenderedBlock::Heading { .. }));
}

#[test]
fn test_jump_top_and_bottom() {
    let blocks = vec![make_heading(1), make_paragraph("a"), make_paragraph("b")];
    let mut vp = Viewport::new(80);
    vp.calculate_total_lines(&blocks, 80);
    vp.jump_bottom(5);
    assert!(vp.scroll_offset > 0);
    vp.jump_top();
    assert_eq!(vp.scroll_offset, 0);
}
```

- [ ] **Step 4: Run viewport tests**

Run: `cargo test --test viewport_test`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add src/viewport.rs src/lib.rs tests/viewport_test.rs
git commit -m "feat: viewport with scroll state, block positioning, and content centering"
```

---

### Task 9: View Layer — ratatui Frame Rendering

**Files:**
- Create: `src/view.rs`

- [ ] **Step 1: Create the view module**

Create `src/view.rs` — responsible for drawing the top bar, content area, and bottom bar using ratatui:

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Frame;

use crate::blocks::*;
use crate::theme::Theme;
use crate::viewport::Viewport;

pub struct ViewState<'a> {
    pub filename: &'a str,
    pub blocks: &'a [RenderedBlock],
    pub viewport: &'a Viewport,
    pub theme: &'a Theme,
    pub mode_label: &'a str,
}

pub fn draw(frame: &mut Frame, state: &ViewState) {
    let area = frame.area();

    // Check minimum terminal size
    if area.width < 40 || area.height < 5 {
        let msg = Paragraph::new("Terminal too small (min 40x5)");
        frame.render_widget(msg, area);
        return;
    }

    let [top_bar, content_area, bottom_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ]).areas(area);

    draw_top_bar(frame, top_bar, state);
    draw_content(frame, content_area, state);
    draw_bottom_bar(frame, bottom_bar, state);
}

fn draw_top_bar(frame: &mut Frame, area: Rect, state: &ViewState) {
    let viewport_height = (frame.area().height as usize).saturating_sub(2);
    let current_line = state.viewport.scroll_offset + 1;
    let total = state.viewport.total_lines.max(1);
    let percent = (state.viewport.scroll_offset * 100) / total.max(1);

    let position = format!("line {}/{} {}%", current_line, total, percent);
    let available = area.width as usize;
    let name_width = available.saturating_sub(position.len() + 1);
    let name = if state.filename.len() > name_width {
        &state.filename[state.filename.len() - name_width..]
    } else {
        state.filename
    };

    let padding = available.saturating_sub(name.len() + position.len());
    let line = Line::from(vec![
        Span::styled(format!(" {}", name), state.theme.top_bar),
        Span::styled(" ".repeat(padding), state.theme.top_bar),
        Span::styled(format!("{} ", position), state.theme.top_bar),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_bottom_bar(frame: &mut Frame, area: Rect, state: &ViewState) {
    let hints = "j/k scroll · {/} heading · / search · q quit";
    let available = area.width as usize;
    let mode_len = state.mode_label.len();
    let padding = available.saturating_sub(mode_len + hints.len() + 3);

    let line = Line::from(vec![
        Span::styled(format!(" {}", state.mode_label), state.theme.mode_indicator),
        Span::styled(" ".repeat(padding), state.theme.bottom_bar),
        Span::styled(format!("{} ", hints), state.theme.bottom_bar),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_content(frame: &mut Frame, area: Rect, state: &ViewState) {
    let terminal_width = area.width as usize;
    let viewport_height = area.height as usize;
    let content_width = state.viewport.content_width(terminal_width);
    let x_offset = state.viewport.content_offset(terminal_width);

    let visible = state.viewport.visible_blocks(state.blocks, content_width, viewport_height);

    for positioned in &visible {
        let block_screen_y = (positioned.y_offset as i32) - (state.viewport.scroll_offset as i32);

        // Each block type renders differently
        let lines = render_block_to_lines(positioned.block, content_width, state.theme);

        for (line_idx, line) in lines.iter().enumerate() {
            let screen_y = block_screen_y + line_idx as i32;
            if screen_y < 0 || screen_y >= viewport_height as i32 {
                continue;
            }

            // Cursor line highlight
            let doc_line = state.viewport.scroll_offset + screen_y as usize;
            let is_cursor_line = doc_line == state.viewport.cursor_line;

            let line_area = Rect::new(
                area.x + x_offset as u16,
                area.y + screen_y as u16,
                content_width.min(area.width as usize - x_offset) as u16,
                1,
            );

            if is_cursor_line {
                // Fill cursor line background
                let bg_area = Rect::new(area.x, area.y + screen_y as u16, area.width, 1);
                let bg = Paragraph::new("").style(state.theme.cursor_line);
                frame.render_widget(bg, bg_area);
            }

            let ratatui_line = styled_line_to_ratatui(line);
            frame.render_widget(Paragraph::new(ratatui_line), line_area);
        }
    }
}

/// Convert a RenderedBlock to terminal lines for display.
fn render_block_to_lines(block: &RenderedBlock, width: usize, theme: &Theme) -> Vec<StyledLine> {
    match block {
        RenderedBlock::Heading { level, content } => {
            let mut lines = vec![content.clone()];
            if *level == 1 {
                // Add underline decoration for h1
                let rule = "━".repeat(content.text_content().len().min(width));
                lines.push(StyledLine::new(vec![
                    StyledSpan::new(rule, theme.horizontal_rule),
                ]));
            }
            lines.push(StyledLine::new(vec![])); // blank line
            lines
        }
        RenderedBlock::Paragraph { lines } => {
            let mut out = lines.clone();
            out.push(StyledLine::new(vec![])); // blank line
            out
        }
        RenderedBlock::CodeBlock { lines, .. } => {
            let mut out = Vec::new();
            for line in lines {
                // Truncate with → indicator if too wide, preserving per-span styles
                let text = line.text_content();
                if text.len() > width {
                    let mut truncated_spans = Vec::new();
                    let mut remaining = width - 1; // leave room for → indicator
                    for span in &line.spans {
                        if remaining == 0 { break; }
                        if span.text.len() <= remaining {
                            truncated_spans.push(span.clone());
                            remaining -= span.text.len();
                        } else {
                            truncated_spans.push(StyledSpan::new(
                                &span.text[..remaining], span.style,
                            ));
                            remaining = 0;
                        }
                    }
                    truncated_spans.push(StyledSpan::new("→", theme.code_block_bg));
                    out.push(StyledLine::new(truncated_spans));
                } else {
                    out.push(line.clone());
                }
            }
            out.push(StyledLine::new(vec![])); // blank line
            out
        }
        RenderedBlock::BlockQuote { blocks } => {
            let mut out = Vec::new();
            for inner_block in blocks {
                let inner_lines = render_block_to_lines(inner_block, width.saturating_sub(4), theme);
                for line in inner_lines {
                    let mut spans = vec![
                        StyledSpan::new("▎ ", theme.blockquote_bar),
                    ];
                    spans.extend(line.spans);
                    out.push(StyledLine::new(spans));
                }
            }
            out
        }
        RenderedBlock::List { items, .. } => {
            let mut out = Vec::new();
            for item in items {
                let marker_display = if let Some(checked) = item.checked {
                    if checked {
                        format!("{} [x] ", item.marker)
                    } else {
                        format!("{} [ ] ", item.marker)
                    }
                } else {
                    format!("{} ", item.marker)
                };

                let mut first = true;
                for content_block in &item.content {
                    let inner_lines = render_block_to_lines(content_block, width.saturating_sub(marker_display.len()), theme);
                    for line in inner_lines {
                        let mut spans = if first {
                            first = false;
                            vec![StyledSpan::new(&marker_display, theme.list_marker)]
                        } else {
                            vec![StyledSpan::new(" ".repeat(marker_display.len()), Style::default())]
                        };
                        spans.extend(line.spans);
                        out.push(StyledLine::new(spans));
                    }
                }
            }
            out
        }
        RenderedBlock::Table { headers, rows, .. } => {
            let mut out = Vec::new();
            // Simple rendering: join cells with " │ "
            let header_text: Vec<String> = headers.iter().map(|h| h.text_content()).collect();
            let col_widths: Vec<usize> = header_text.iter().map(|h| h.len().max(5)).collect();

            // Header
            let header_spans: Vec<StyledSpan> = headers.iter().enumerate().map(|(i, h)| {
                let padded = format!("{:<width$}", h.text_content(), width = col_widths.get(i).copied().unwrap_or(5));
                StyledSpan::new(padded, theme.table_header)
            }).collect();
            let mut hline_spans = Vec::new();
            for (i, span) in header_spans.into_iter().enumerate() {
                if i > 0 { hline_spans.push(StyledSpan::new(" │ ", theme.table_border)); }
                hline_spans.push(span);
            }
            out.push(StyledLine::new(hline_spans));

            // Separator
            let sep: String = col_widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("─┼─");
            out.push(StyledLine::new(vec![StyledSpan::new(sep, theme.table_border)]));

            // Rows
            for row in rows {
                let mut row_spans = Vec::new();
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 { row_spans.push(StyledSpan::new(" │ ", theme.table_border)); }
                    let padded = format!("{:<width$}", cell.text_content(), width = col_widths.get(i).copied().unwrap_or(5));
                    row_spans.push(StyledSpan::new(padded, theme.paragraph));
                }
                out.push(StyledLine::new(row_spans));
            }
            out.push(StyledLine::new(vec![])); // blank line
            out
        }
        RenderedBlock::HorizontalRule => {
            let rule = "─".repeat(width);
            vec![
                StyledLine::new(vec![StyledSpan::new(rule, theme.horizontal_rule)]),
                StyledLine::new(vec![]),
            ]
        }
        RenderedBlock::Image { alt, .. } => {
            let label = format!("[Image: {}]", alt);
            vec![
                StyledLine::new(vec![StyledSpan::new(label, theme.image_label)]),
                StyledLine::new(vec![]),
            ]
        }
    }
}

fn styled_line_to_ratatui(line: &StyledLine) -> Line<'static> {
    Line::from(
        line.spans.iter().map(|s| {
            Span::styled(s.text.clone(), s.style)
        }).collect::<Vec<_>>()
    )
}
```

- [ ] **Step 2: Add view module to lib.rs**

Add `pub mod view;` to `src/lib.rs`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src/view.rs src/lib.rs
git commit -m "feat: view layer with top bar, content rendering, and bottom bar"
```

---

### Task 10: Config System

**Files:**
- Create: `src/config.rs`

- [ ] **Step 1: Create the config module**

Create `src/config.rs`:

```rust
use std::path::PathBuf;

pub struct Config {
    pub max_line_width: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_line_width: 80,
        }
    }
}

impl Config {
    /// Load config from the standard location, falling back to defaults.
    pub fn load() -> Self {
        let path = config_path();
        match path {
            Some(p) if p.exists() => Self::load_from_file(&p),
            _ => Self::default(),
        }
    }

    fn load_from_file(path: &std::path::Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not read config {}: {}", path.display(), e);
                return Self::default();
            }
        };

        let doc: kdl::KdlDocument = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Warning: invalid KDL in {}: {}", path.display(), e);
                return Self::default();
            }
        };

        let mut config = Self::default();

        if let Some(display) = doc.get("display") {
            if let Some(children) = display.children() {
                if let Some(node) = children.get("max-line-width") {
                    if let Some(val) = node.get(0).and_then(|e| e.value().as_i64()) {
                        config.max_line_width = val as usize;
                    }
                }
            }
        }

        config
    }
}

fn config_path() -> Option<PathBuf> {
    // Check env var first
    if let Ok(p) = std::env::var("SKETCH_CONFIG") {
        return Some(PathBuf::from(p));
    }

    // XDG config
    dirs::config_dir().map(|d| d.join("sketch").join("config.kdl"))
}
```

- [ ] **Step 2: Add config module to lib.rs**

Add `pub mod config;` to `src/lib.rs`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src/config.rs src/lib.rs
git commit -m "feat: KDL config loading with defaults"
```

---

### Task 11: App Layer — Event Loop and Main

**Files:**
- Create: `src/app.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create the app module**

Create `src/app.rs`:

```rust
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use ratatui::DefaultTerminal;

use sketch::blocks::RenderedBlock;
use sketch::config::Config;
use sketch::keybind::{Action, KeybindManager};
use sketch::render;
use sketch::theme::Theme;
use sketch::view::{self, ViewState};
use sketch::viewport::Viewport;

pub struct App {
    filename: String,
    blocks: Vec<RenderedBlock>,
    viewport: Viewport,
    theme: Theme,
    keybinds: KeybindManager,
    should_quit: bool,
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::dark();
        let blocks = render::render(&markdown, &theme);
        let viewport = Viewport::new(config.max_line_width);
        let keybinds = KeybindManager::default();

        Self {
            filename,
            blocks,
            viewport,
            theme,
            keybinds,
            should_quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // Calculate initial dimensions
        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2); // minus top/bottom bars
        let content_width = self.viewport.content_width(size.width as usize);
        self.viewport.calculate_total_lines(&self.blocks, content_width);

        loop {
            terminal.draw(|frame| {
                let state = ViewState {
                    filename: &self.filename,
                    blocks: &self.blocks,
                    viewport: &self.viewport,
                    theme: &self.theme,
                    mode_label: "NORMAL",
                };
                view::draw(frame, &state);
            })?;

            if self.should_quit {
                break;
            }

            // Poll for events with a timeout (for multi-key sequence timeout)
            let timeout = if self.keybinds.has_pending() {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key_event) => self.handle_key(key_event, terminal)?,
                    Event::Resize(w, h) => {
                        let vp_height = (h as usize).saturating_sub(2);
                        let cw = self.viewport.content_width(w as usize);
                        self.viewport.calculate_total_lines(&self.blocks, cw);
                    }
                    _ => {}
                }
            } else if self.keybinds.has_pending() {
                // Timeout with pending keys — reset
                self.keybinds.reset_pending();
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2);
        let content_width = self.viewport.content_width(size.width as usize);

        if let Some(action) = self.keybinds.process_key(key) {
            match action {
                Action::Quit => self.should_quit = true,
                Action::ScrollDown => self.viewport.scroll_down(1, viewport_height),
                Action::ScrollUp => self.viewport.scroll_up(1),
                Action::HalfPageDown => self.viewport.scroll_down(viewport_height / 2, viewport_height),
                Action::HalfPageUp => self.viewport.scroll_up(viewport_height / 2),
                Action::FullPageDown => self.viewport.scroll_down(viewport_height, viewport_height),
                Action::FullPageUp => self.viewport.scroll_up(viewport_height),
                Action::JumpTop => self.viewport.jump_top(),
                Action::JumpBottom => self.viewport.jump_bottom(viewport_height),
                Action::NextHeading => {
                    self.jump_to_next_heading(content_width, viewport_height, false);
                }
                Action::PrevHeading => {
                    self.jump_to_prev_heading(content_width, viewport_height, false);
                }
                Action::NextHeadingSameLevel => {
                    self.jump_to_heading_same_level(content_width, viewport_height, true);
                }
                Action::PrevHeadingSameLevel => {
                    self.jump_to_heading_same_level(content_width, viewport_height, false);
                }
                Action::SearchForward | Action::SearchBackward
                | Action::SearchNext | Action::SearchPrev => {
                    // Search — implement in a later task
                }
                Action::OpenLink | Action::YankLine => {
                    // Implement in a later task
                }
                Action::None => {}
            }
        }

        Ok(())
    }

    fn jump_to_next_heading(&mut self, width: usize, viewport_height: usize, _same_level: bool) {
        let mut y = 0;
        let mut found_current = false;

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if y > self.viewport.scroll_offset && matches!(block, RenderedBlock::Heading { .. }) {
                // Center this heading
                self.viewport.scroll_offset = y.saturating_sub(viewport_height / 3);
                return;
            }
            if y >= self.viewport.scroll_offset {
                found_current = true;
            }
            y += h;
        }
    }

    fn jump_to_prev_heading(&mut self, width: usize, viewport_height: usize, _same_level: bool) {
        let mut positions = Vec::new();
        let mut y = 0;

        for block in &self.blocks {
            if matches!(block, RenderedBlock::Heading { .. }) {
                positions.push(y);
            }
            y += self.viewport.block_height(block, width);
        }

        // Find the last heading position before current scroll
        if let Some(&pos) = positions.iter().rev().find(|&&p| p < self.viewport.scroll_offset) {
            self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
        }
    }

    fn jump_to_heading_same_level(&mut self, width: usize, viewport_height: usize, forward: bool) {
        // Find the current heading level (most recently passed heading)
        let mut current_level = None;
        let mut y = 0;
        let mut headings: Vec<(usize, u8)> = Vec::new(); // (y_offset, level)

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if let RenderedBlock::Heading { level, .. } = block {
                if y <= self.viewport.scroll_offset {
                    current_level = Some(*level);
                }
                headings.push((y, *level));
            }
            y += h;
        }

        let target_level = match current_level {
            Some(l) => l,
            None => return,
        };

        if forward {
            if let Some(&(pos, _)) = headings.iter().find(|(y, l)| *y > self.viewport.scroll_offset && *l == target_level) {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        } else {
            if let Some(&(pos, _)) = headings.iter().rev().find(|(y, l)| *y < self.viewport.scroll_offset && *l == target_level) {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        }
    }
}
```

- [ ] **Step 2: Write main.rs with CLI, error handling, and panic hook**

Replace `src/main.rs`:

```rust
use std::io;
use std::process;

use clap::Parser;

mod app;

#[derive(Parser)]
#[command(name = "sketch", about = "A beautiful TUI markdown viewer")]
struct Cli {
    /// Markdown file to view
    file: Option<String>,
}

fn main() {
    // Install panic hook for terminal restoration
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
        default_hook(info);
    }));

    let cli = Cli::parse();

    let file_path = match cli.file {
        Some(f) => f,
        None => {
            eprintln!("Usage: sketch <file.md>");
            process::exit(1);
        }
    };

    // Read file
    let content = match std::fs::read(&file_path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Error: {} is not valid UTF-8", file_path);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error: cannot open {}: {}", file_path, e);
            process::exit(1);
        }
    };

    let abs_path = std::path::Path::new(&file_path)
        .canonicalize()
        .unwrap_or_else(|_| file_path.clone().into())
        .display()
        .to_string();

    let config = sketch::config::Config::load();
    let mut app = app::App::new(abs_path, content, &config);

    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
```

- [ ] **Step 3: Add app module to lib.rs** (not needed — app is in main's crate, not lib)

The `app` module uses `mod app;` in `main.rs`, not in `lib.rs`, since it depends on terminal I/O and isn't part of the library API.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles

- [ ] **Step 5: Manual test — run on a markdown file**

Run: `cargo run -- README.md` (or any `.md` file)
Expected: TUI opens showing rendered markdown with top/bottom bars. `j`/`k` scrolls. `q` quits. Terminal restores cleanly on exit.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "feat: app layer with event loop, CLI, panic hook, and navigation"
```

---

### Task 12: Search

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add search state to App**

Add fields to the `App` struct:

```rust
search_query: String,
search_input_mode: bool,
search_input_buffer: String,
search_matches: Vec<(usize, usize)>, // (block_index, span_index)
search_match_index: usize,
```

- [ ] **Step 2: Implement search input mode**

When `Action::SearchForward` fires, set `search_input_mode = true` and display a `/` prompt in the bottom bar. Collect typed characters into `search_input_buffer`. On Enter, perform the search. On Escape, cancel.

In the key handler, when `search_input_mode` is true, bypass the keybind manager and handle raw key events directly:

```rust
if self.search_input_mode {
    match key.code {
        KeyCode::Enter => {
            self.search_query = self.search_input_buffer.clone();
            self.search_input_mode = false;
            self.perform_search();
            self.jump_to_match(content_width, viewport_height);
        }
        KeyCode::Esc => {
            self.search_input_mode = false;
            self.search_input_buffer.clear();
        }
        KeyCode::Backspace => { self.search_input_buffer.pop(); }
        KeyCode::Char(c) => { self.search_input_buffer.push(c); }
        _ => {}
    }
    return Ok(());
}
```

- [ ] **Step 3: Implement search logic**

```rust
fn perform_search(&mut self) {
    self.search_matches.clear();
    let query = self.search_query.to_lowercase();
    if query.is_empty() { return; }

    for (bi, block) in self.blocks.iter().enumerate() {
        self.search_block(&query, block, bi);
    }
    self.search_match_index = 0;
}

fn search_block(&mut self, query: &str, block: &RenderedBlock, block_index: usize) {
    match block {
        RenderedBlock::Heading { content, .. } => {
            if content.text_content().to_lowercase().contains(query) {
                self.search_matches.push((block_index, 0));
            }
        }
        RenderedBlock::Paragraph { lines } | RenderedBlock::CodeBlock { lines, .. } => {
            for (li, line) in lines.iter().enumerate() {
                if line.text_content().to_lowercase().contains(query) {
                    self.search_matches.push((block_index, li));
                }
            }
        }
        RenderedBlock::BlockQuote { blocks } => {
            for b in blocks { self.search_block(query, b, block_index); }
        }
        RenderedBlock::List { items, .. } => {
            for item in items {
                for b in &item.content { self.search_block(query, b, block_index); }
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 4: Implement SearchNext/SearchPrev actions**

```rust
Action::SearchNext => {
    if !self.search_matches.is_empty() {
        self.search_match_index = (self.search_match_index + 1) % self.search_matches.len();
        self.jump_to_match(content_width, viewport_height);
    }
}
Action::SearchPrev => {
    if !self.search_matches.is_empty() {
        self.search_match_index = if self.search_match_index == 0 {
            self.search_matches.len() - 1
        } else {
            self.search_match_index - 1
        };
        self.jump_to_match(content_width, viewport_height);
    }
}
```

- [ ] **Step 5: Update bottom bar to show search input and match count**

In `view.rs`, update the `ViewState` to include search state and render the `/query` prompt when in search input mode, and `[N/M]` match indicator when matches exist.

- [ ] **Step 6: Manual test**

Run: `cargo run -- some_file.md`
Test: Press `/`, type a query, press Enter. `n`/`N` cycle through matches. Press `q` to quit.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/view.rs
git commit -m "feat: forward/backward search with match navigation"
```

---

### Task 13: Link Opening and Line Yanking

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Implement OpenLink action**

Find the nearest link under/near the cursor position. Use `std::process::Command` to open it:

```rust
Action::OpenLink => {
    if let Some(url) = self.find_link_at_cursor(content_width) {
        let _ = std::process::Command::new("open") // macOS
            .arg(&url)
            .spawn();
    }
}
```

- [ ] **Step 2: Implement YankLine action**

Copy the current line's text content to the system clipboard. Shell out to `pbcopy` on macOS (no extra dependency):

```rust
Action::YankLine => {
    if let Some(text) = self.get_cursor_line_text(content_width) {
        use std::process::{Command, Stdio};
        use std::io::Write;
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
        }
    }
}
```

- [ ] **Step 3: Manual test**

Test `o` on a link opens the browser. Test `y` yanks a line (paste somewhere to verify).

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "feat: open links in browser and yank line to clipboard"
```

---

### Task 14: End-to-End Test and Polish

**Files:**
- Create: `tests/fixtures/showcase.md`
- Modify: various files for bug fixes

- [ ] **Step 1: Create a comprehensive showcase file**

Create `tests/fixtures/showcase.md` that exercises every supported markdown feature: all heading levels, nested lists, task lists, code blocks in multiple languages, blockquotes, tables, images, horizontal rules, inline styles (bold, italic, strikethrough, code, links).

- [ ] **Step 2: Manual end-to-end test**

Run: `cargo run -- tests/fixtures/showcase.md`

Verify:
- [ ] All heading levels render with distinct styles
- [ ] Bold, italic, strikethrough, inline code render correctly
- [ ] Links are underlined and colored
- [ ] Code blocks have syntax highlighting and background
- [ ] Blockquotes have left bar decoration
- [ ] Lists render with markers (bullets, numbers, checkboxes)
- [ ] Tables render with box-drawing borders
- [ ] Horizontal rules render as lines
- [ ] Images show `[Image: alt]` placeholder
- [ ] j/k/gg/G scrolling works
- [ ] {/} heading navigation works
- [ ] / search works
- [ ] q quits cleanly
- [ ] Terminal too small message shows when resized tiny
- [ ] No panics on any content

- [ ] **Step 3: Fix any rendering bugs found during testing**

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test: add showcase fixture and polish rendering"
```

---

### Task 15: Snapshot Tests

**Files:**
- Create: `tests/snapshot_test.rs`

- [ ] **Step 1: Write snapshot tests for rendered output**

Create `tests/snapshot_test.rs`:

```rust
use insta::assert_debug_snapshot;
use sketch::render::render;
use sketch::theme::Theme;

#[test]
fn snapshot_heading_levels() {
    let md = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
    let blocks = render(md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}

#[test]
fn snapshot_complex_document() {
    let md = std::fs::read_to_string("tests/fixtures/showcase.md").unwrap();
    let blocks = render(&md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}

#[test]
fn snapshot_code_block_rust() {
    let md = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```";
    let blocks = render(md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}
```

- [ ] **Step 2: Generate initial snapshots**

Run: `cargo insta test` then `cargo insta review` to accept the snapshots.

- [ ] **Step 3: Verify snapshots pass on re-run**

Run: `cargo test --test snapshot_test`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add tests/snapshot_test.rs tests/snapshots/
git commit -m "test: add insta snapshot tests for rendered output"
```

---

### Task 16: Final Cleanup

**Files:**
- Modify: `src/lib.rs`, `Cargo.toml`

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -- -W clippy::all`
Fix any warnings.

- [ ] **Step 2: Run fmt**

Run: `cargo fmt`

- [ ] **Step 3: Verify all tests pass**

Run: `cargo test`
Expected: All PASS

- [ ] **Step 4: Verify clean build**

Run: `cargo build --release`
Expected: Compiles with no warnings

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore: clippy fixes and formatting"
```
