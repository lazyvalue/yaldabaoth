use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};

use crate::blocks::*;
use crate::highlight::Highlighter;
use crate::parse;
use crate::style::Style;
use crate::theme::Theme;

pub fn render(markdown: &str, theme: &Theme) -> Vec<RenderedBlock> {
    render_mapped(markdown, theme).blocks
}

/// Render plus a [`SourceSpan`] per top-level block (see [`Rendered`]).
pub fn render_mapped(markdown: &str, theme: &Theme) -> Rendered {
    // Match the theme so code fences aren't always base16-ocean.dark regardless
    // of the active theme (e.g. dark tokens washing out on Folio's linen bg).
    let highlighter = Highlighter::with_syntect_theme(theme.name.syntect_theme());
    let (events, ranges): (Vec<_>, Vec<_>) = parse::parse_with_offsets(markdown).unzip();
    let mut renderer = Renderer::new(theme, &highlighter);
    renderer.top_ranges = Some(ranges);
    let blocks = renderer.render(&events);
    let starts = renderer.top_spans;
    let mut lines = LineIndex::new(markdown);
    let spans = starts
        .into_iter()
        .map(|bytes| SourceSpan {
            lines: lines.lines_of(&bytes),
            bytes,
        })
        .collect();
    Rendered { blocks, spans }
}

/// Byte offset → 0-based line, for monotonically non-decreasing queries.
struct LineIndex<'s> {
    text: &'s str,
    pos: usize,
    line: usize,
}

impl<'s> LineIndex<'s> {
    fn new(text: &'s str) -> Self {
        Self { text, pos: 0, line: 0 }
    }

    fn line_of(&mut self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if byte < self.pos {
            // Out-of-order query: recount from the start.
            self.pos = 0;
            self.line = 0;
        }
        self.line += self.text.as_bytes()[self.pos..byte]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        self.pos = byte;
        self.line
    }

    /// Half-open line range covered by `bytes` (a trailing newline does not
    /// extend it onto the next line).
    fn lines_of(&mut self, bytes: &std::ops::Range<usize>) -> std::ops::Range<usize> {
        let start = self.line_of(bytes.start);
        let last = if bytes.end > bytes.start {
            self.line_of(bytes.end - 1)
        } else {
            start
        };
        start..last + 1
    }
}

/// One heading in a document outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineHeading {
    pub level: u8,
    /// Plain heading text (markup stripped).
    pub text: String,
    /// Source byte range of the heading element.
    pub bytes: std::ops::Range<usize>,
    /// 0-based source line the heading starts on.
    pub line: usize,
}

/// Every heading in `markdown` at any nesting depth (ATX and setext; `#`
/// lines inside code blocks are code, not headings). The one outline both the
/// rendered and raw views use, so they always agree.
pub fn outline(markdown: &str) -> Vec<OutlineHeading> {
    let mut out = Vec::new();
    let mut lines = LineIndex::new(markdown);
    let mut current: Option<OutlineHeading> = None;
    for (event, range) in parse::parse_with_offsets(markdown) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some(OutlineHeading {
                    level: heading_level_to_u8(level),
                    text: String::new(),
                    line: lines.line_of(range.start),
                    bytes: range,
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut h) = current.take() {
                    h.text = h.text.trim().to_string();
                    if !h.text.is_empty() {
                        out.push(h);
                    }
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = current.as_mut() {
                    h.text.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(h) = current.as_mut() {
                    h.text.push(' ');
                }
            }
            _ => {}
        }
    }
    out
}

/// Render using an existing Highlighter (avoids re-loading syntax definitions).
pub fn render_with_highlighter(
    markdown: &str,
    theme: &Theme,
    highlighter: &Highlighter,
) -> Vec<RenderedBlock> {
    let events: Vec<_> = parse::parse(markdown).collect();
    let mut renderer = Renderer::new(theme, highlighter);
    renderer.render(&events)
}

struct Renderer<'a, 't> {
    theme: &'t Theme,
    highlighter: &'a Highlighter,
    /// Source ranges parallel to the top-level event slice, when mapping.
    top_ranges: Option<Vec<std::ops::Range<usize>>>,
    /// Byte range of each top-level block emitted (filled only when mapping).
    top_spans: Vec<std::ops::Range<usize>>,
    /// `render` nesting depth: only depth 1 is the top-level event slice.
    depth: usize,
}

struct InlineState {
    spans: Vec<StyledSpan>,
    style_stack: Vec<Style>,
    link_stack: Vec<Option<String>>,
    /// Lines already ended by a hard break (only when `hard_breaks`).
    lines: Vec<StyledLine>,
    /// Whether `HardBreak` starts a new line (paragraphs) or collapses to a
    /// space (headings, table cells — single-line containers).
    hard_breaks: bool,
    /// `spans.len()` at each open inline image, to detect an empty alt text.
    image_starts: Vec<usize>,
}

impl InlineState {
    fn new(base_style: Style) -> Self {
        Self {
            spans: Vec::new(),
            style_stack: vec![base_style],
            link_stack: vec![None],
            lines: Vec::new(),
            hard_breaks: false,
            image_starts: Vec::new(),
        }
    }

    /// A multi-line container: `HardBreak` ends the current line.
    fn multiline(base_style: Style) -> Self {
        Self {
            hard_breaks: true,
            ..Self::new(base_style)
        }
    }

    fn hard_break(&mut self) {
        if self.hard_breaks {
            let spans = std::mem::take(&mut self.spans);
            self.lines.push(StyledLine::new(spans));
        } else {
            self.push_text(" ");
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
        self.spans.push(StyledSpan {
            text: text.to_string(),
            style,
            link,
        });
    }

    fn into_line(self) -> StyledLine {
        StyledLine::new(self.spans)
    }

    /// Every line, including the one still open.
    fn into_lines(mut self) -> Vec<StyledLine> {
        self.lines.push(StyledLine::new(self.spans));
        self.lines
    }

    fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.lines.iter().all(|l| l.spans.is_empty())
    }
}

/// Events that live inside a paragraph (inline content). Anything else starts
/// or ends a block.
fn is_inline_event(event: &Event<'_>) -> bool {
    match event {
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
        ),
        Event::End(tag) => matches!(
            tag,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image
        ),
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::InlineHtml(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::Html(_) | Event::Rule => false,
    }
}

/// Label shown for an image with no alt text: the last path segment of `url`.
fn image_fallback_label(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

/// A footnote reference marker: numeric labels become superscript digits
/// (`[^12]` → `¹²`), anything else keeps a bracketed label (`[^note]` → `[note]`).
pub fn footnote_marker(label: &str) -> String {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    if !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()) {
        label
            .chars()
            .map(|c| SUP[c.to_digit(10).unwrap_or(0) as usize])
            .collect()
    } else {
        format!("[{label}]")
    }
}

impl<'a, 't> Renderer<'a, 't> {
    fn new(theme: &'t Theme, highlighter: &'a Highlighter) -> Self {
        Self {
            theme,
            highlighter,
            top_ranges: None,
            top_spans: Vec::new(),
            depth: 0,
        }
    }

    fn render(&mut self, events: &[Event<'_>]) -> Vec<RenderedBlock> {
        self.depth += 1;
        let mut blocks = Vec::new();
        let mut i = 0;

        while i < events.len() {
            let (event_start, blocks_before) = (i, blocks.len());
            i = self.render_one(events, i, &mut blocks);
            if self.depth == 1
                && let Some(ranges) = &self.top_ranges
            {
                for _ in blocks_before..blocks.len() {
                    self.top_spans.push(ranges[event_start].clone());
                }
            }
        }

        self.depth -= 1;
        blocks
    }

    /// Consume the element starting at `events[*i]`, pushing its block (if
    /// any) onto `blocks`.
    fn render_one(
        &mut self,
        events: &[Event<'_>],
        mut i: usize,
        blocks: &mut Vec<RenderedBlock>,
    ) -> usize {
        {
            match &events[i] {
                Event::Start(Tag::Heading { level, .. }) => {
                    let level_num = heading_level_to_u8(*level);
                    i += 1;
                    let heading_level = *level;
                    let mut state = InlineState::new(self.theme.heading[(level_num - 1) as usize]);
                    i = self.collect_inline(events, i, &TagEnd::Heading(heading_level), &mut state);
                    blocks.push(RenderedBlock::Heading {
                        level: level_num,
                        content: state.into_line(),
                    });
                }
                Event::Start(Tag::Paragraph) => {
                    i += 1;
                    // An image alone in its paragraph (`![alt](src)` on its own
                    // line — the normal way to embed one) is a block image.
                    if matches!(events.get(i), Some(Event::Start(Tag::Image { .. }))) {
                        let (image, after) = Self::collect_image(events, i);
                        if matches!(events.get(after), Some(Event::End(TagEnd::Paragraph))) {
                            blocks.push(image);
                            return after + 1;
                        }
                    }
                    let mut state = InlineState::multiline(self.theme.paragraph);
                    i = self.collect_inline(events, i, &TagEnd::Paragraph, &mut state);
                    blocks.push(RenderedBlock::Paragraph {
                        lines: state.into_lines(),
                    });
                }
                Event::Start(Tag::BlockQuote(kind)) => {
                    let kind = *kind;
                    i += 1;
                    let (sub_blocks, new_i) =
                        self.collect_block(events, i, &TagEnd::BlockQuote(kind));
                    i = new_i;
                    blocks.push(RenderedBlock::BlockQuote { blocks: sub_blocks });
                }
                Event::Start(Tag::List(start)) => {
                    let ordered = start.is_some();
                    let start_num = *start;
                    i += 1;
                    let (items, new_i) = self.collect_list_items(events, i, ordered, start_num);
                    i = new_i;
                    blocks.push(RenderedBlock::List {
                        ordered,
                        start: start_num,
                        items,
                    });
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
                    let mut code_text = String::new();
                    while i < events.len() {
                        match &events[i] {
                            Event::Text(t) => {
                                code_text.push_str(t.as_ref());
                                i += 1;
                            }
                            Event::End(TagEnd::CodeBlock) => {
                                i += 1;
                                break;
                            }
                            _ => {
                                i += 1;
                            }
                        }
                    }

                    let lines = if let Some(lang) = &language {
                        self.highlighter
                            .highlight(lang, &code_text, self.theme.code_block_bg)
                            .unwrap_or_else(|| self.plain_code_lines(&code_text))
                    } else {
                        self.plain_code_lines(&code_text)
                    };

                    // A ` ```mermaid ` fence becomes a Diagram block (rendered
                    // to an image off-thread by the frontend), carrying the
                    // highlighted `lines` as placeholder/fallback. Any other
                    // language stays a CodeBlock. See UXI-Diagram-1.
                    let is_mermaid = language
                        .as_deref()
                        .and_then(|l| l.split_whitespace().next())
                        .is_some_and(|tok| tok.eq_ignore_ascii_case("mermaid"));
                    if is_mermaid {
                        let source = code_text.strip_suffix('\n').unwrap_or(&code_text).to_string();
                        blocks.push(RenderedBlock::Diagram { source, lines });
                    } else {
                        blocks.push(RenderedBlock::CodeBlock {
                            language,
                            lines,
                            source_file: false,
                            start_line: 0,
                        });
                    }
                }
                Event::Start(Tag::Table(alignments)) => {
                    let aligns: Vec<ColumnAlignment> = alignments
                        .iter()
                        .map(|a| match a {
                            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => {
                                ColumnAlignment::Left
                            }
                            pulldown_cmark::Alignment::Center => ColumnAlignment::Center,
                            pulldown_cmark::Alignment::Right => ColumnAlignment::Right,
                        })
                        .collect();
                    i += 1;
                    let (headers, rows, new_i) = self.collect_table(events, i);
                    i = new_i;
                    blocks.push(RenderedBlock::Table {
                        headers,
                        rows,
                        alignments: aligns,
                    });
                }
                Event::Rule => {
                    blocks.push(RenderedBlock::HorizontalRule);
                    i += 1;
                }
                // Frontmatter (bug-0014). The parser hands us the raw block text;
                // keep it line-by-line — the setext-heading misparse this replaces
                // collapsed the newlines into one run-on line.
                Event::Start(Tag::MetadataBlock(_)) => {
                    i += 1;
                    let mut raw = String::new();
                    while i < events.len() {
                        match &events[i] {
                            Event::Text(t) => {
                                raw.push_str(t.as_ref());
                                i += 1;
                            }
                            Event::End(TagEnd::MetadataBlock(_)) => {
                                i += 1;
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                    let lines: Vec<StyledLine> = raw
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(StyledLine::plain)
                        .collect();
                    if !lines.is_empty() {
                        blocks.push(RenderedBlock::Metadata { lines });
                    }
                }
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    let label = label.to_string();
                    let (sub_blocks, new_i) =
                        self.collect_block(events, i + 1, &TagEnd::FootnoteDefinition);
                    i = new_i;
                    blocks.push(RenderedBlock::Footnote {
                        label,
                        blocks: sub_blocks,
                    });
                }
                Event::Start(Tag::Image { .. }) => {
                    let (image, after) = Self::collect_image(events, i);
                    blocks.push(image);
                    i = after;
                }
                _ => {
                    i += 1;
                }
            }
        }
        i
    }

    /// Consume the image element starting at `events[i]` (a `Start(Image)`)
    /// into a [`RenderedBlock::Image`]; returns it and the index after its
    /// `End(Image)`. Alt text falls back to the title, then the file name.
    fn collect_image(events: &[Event<'_>], mut i: usize) -> (RenderedBlock, usize) {
        let (url, title) = match &events[i] {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => (dest_url.to_string(), title.to_string()),
            _ => (String::new(), String::new()),
        };
        i += 1;
        let mut alt = String::new();
        let mut depth = 0usize;
        while i < events.len() {
            match &events[i] {
                Event::Text(t) | Event::Code(t) => alt.push_str(t.as_ref()),
                Event::SoftBreak | Event::HardBreak => alt.push(' '),
                Event::Start(Tag::Image { .. }) => depth += 1,
                Event::End(TagEnd::Image) if depth == 0 => {
                    i += 1;
                    break;
                }
                Event::End(TagEnd::Image) => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        if alt.is_empty() {
            alt = title;
        }
        if alt.is_empty() {
            alt = image_fallback_label(&url);
        }
        (RenderedBlock::Image { alt, url }, i)
    }

    fn plain_code_lines(&self, code: &str) -> Vec<StyledLine> {
        code.lines()
            .map(|line| StyledLine::new(vec![StyledSpan::new(line, self.theme.code_block_bg)]))
            .collect()
    }

    fn collect_inline(
        &self,
        events: &[Event<'_>],
        mut i: usize,
        end: &TagEnd,
        state: &mut InlineState,
    ) -> usize {
        while i < events.len() {
            match &events[i] {
                Event::End(e) if e == end => {
                    i += 1;
                    break;
                }
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
                Event::SoftBreak => {
                    state.push_text(" ");
                    i += 1;
                }
                Event::HardBreak => {
                    state.hard_break();
                    i += 1;
                }
                Event::FootnoteReference(label) => {
                    state.style_stack.push(self.theme.link);
                    state.push_text(&footnote_marker(label));
                    state.style_stack.pop();
                    i += 1;
                }
                // An image mixed with text: its alt text as a link to the image.
                Event::Start(Tag::Image { dest_url, .. }) => {
                    state.style_stack.push(self.theme.link);
                    state.link_stack.push(Some(dest_url.to_string()));
                    state.image_starts.push(state.spans.len());
                    i += 1;
                }
                Event::End(TagEnd::Image) => {
                    if state.image_starts.pop() == Some(state.spans.len()) {
                        let url = state.current_link().unwrap_or_default();
                        state.push_text(&image_fallback_label(&url));
                    }
                    state.style_stack.pop();
                    state.link_stack.pop();
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
                _ => {
                    i += 1;
                }
            }
        }
        i
    }

    fn collect_block(
        &mut self,
        events: &[Event<'_>],
        mut i: usize,
        end: &TagEnd,
    ) -> (Vec<RenderedBlock>, usize) {
        let mut inner_events = Vec::new();
        let mut depth = 0;
        while i < events.len() {
            match &events[i] {
                Event::End(e) if e == end && depth == 0 => {
                    i += 1;
                    break;
                }
                Event::Start(_) => {
                    depth += 1;
                    inner_events.push(events[i].clone());
                    i += 1;
                }
                Event::End(_) => {
                    depth -= 1;
                    inner_events.push(events[i].clone());
                    i += 1;
                }
                _ => {
                    inner_events.push(events[i].clone());
                    i += 1;
                }
            }
        }
        let blocks = self.render(&inner_events);
        (blocks, i)
    }

    fn collect_list_items(
        &mut self,
        events: &[Event<'_>],
        mut i: usize,
        ordered: bool,
        start: Option<u64>,
    ) -> (Vec<ListItem>, usize) {
        let mut items = Vec::new();
        let mut item_index = start.unwrap_or(1);
        while i < events.len() {
            match &events[i] {
                Event::End(TagEnd::List(_)) => {
                    i += 1;
                    break;
                }
                Event::Start(Tag::Item) => {
                    i += 1;
                    let marker = if ordered {
                        format!("{}.", item_index)
                    } else {
                        "\u{2022}".to_string()
                    };
                    let mut checked = None;
                    let mut item_events = Vec::new();
                    let mut depth = 0;
                    while i < events.len() {
                        match &events[i] {
                            Event::End(TagEnd::Item) if depth == 0 => {
                                i += 1;
                                break;
                            }
                            // Only this item's own marker: a nested item's
                            // stays in its events for the nested list.
                            Event::TaskListMarker(c) if depth == 0 => {
                                checked = Some(*c);
                                i += 1;
                            }
                            Event::Start(_) => {
                                depth += 1;
                                item_events.push(events[i].clone());
                                i += 1;
                            }
                            Event::End(_) => {
                                depth -= 1;
                                item_events.push(events[i].clone());
                                i += 1;
                            }
                            _ => {
                                item_events.push(events[i].clone());
                                i += 1;
                            }
                        }
                    }
                    let content = self.render_item_content(&item_events);
                    items.push(ListItem {
                        marker,
                        checked,
                        content,
                    });
                    item_index += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
        (items, i)
    }

    /// A list item's content. Loose items wrap their text in `Paragraph`s;
    /// tight items (pulldown-cmark omits the wrapper) carry bare inline
    /// events, possibly followed by nested blocks. Each run of bare inline
    /// events becomes a paragraph through the same `collect_inline` path.
    fn render_item_content(&mut self, events: &[Event<'_>]) -> Vec<RenderedBlock> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < events.len() {
            if is_inline_event(&events[i]) {
                let start = i;
                while i < events.len() && is_inline_event(&events[i]) {
                    i += 1;
                }
                let mut state = InlineState::multiline(self.theme.paragraph);
                // A run holds no block tags, so `End(Paragraph)` never matches:
                // it is consumed to its end.
                self.collect_inline(&events[start..i], 0, &TagEnd::Paragraph, &mut state);
                if !state.is_empty() {
                    out.push(RenderedBlock::Paragraph {
                        lines: state.into_lines(),
                    });
                }
            } else {
                i = self.render_one(events, i, &mut out);
            }
        }
        out
    }

    fn collect_table(
        &mut self,
        events: &[Event<'_>],
        mut i: usize,
    ) -> (Vec<StyledLine>, Vec<Vec<StyledLine>>, usize) {
        let mut headers = Vec::new();
        let mut rows: Vec<Vec<StyledLine>> = Vec::new();
        let mut current_row: Vec<StyledLine> = Vec::new();
        let mut in_head = false;
        while i < events.len() {
            match &events[i] {
                Event::End(TagEnd::Table) => {
                    i += 1;
                    break;
                }
                Event::Start(Tag::TableHead) => {
                    in_head = true;
                    i += 1;
                }
                Event::End(TagEnd::TableHead) => {
                    if !current_row.is_empty() {
                        headers = std::mem::take(&mut current_row);
                    }
                    in_head = false;
                    i += 1;
                }
                Event::Start(Tag::TableRow) => {
                    current_row = Vec::new();
                    i += 1;
                }
                Event::End(TagEnd::TableRow) => {
                    if in_head {
                        headers = std::mem::take(&mut current_row);
                    } else {
                        rows.push(std::mem::take(&mut current_row));
                    }
                    i += 1;
                }
                Event::Start(Tag::TableCell) => {
                    let style = if in_head {
                        self.theme.table_header
                    } else {
                        self.theme.paragraph
                    };
                    i += 1;
                    let mut state = InlineState::new(style);
                    i = self.collect_inline(events, i, &TagEnd::TableCell, &mut state);
                    current_row.push(state.into_line());
                }
                _ => {
                    i += 1;
                }
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

#[cfg(test)]
mod frontmatter_tests {
    use super::*;

    /// bug-0014: a document opening with YAML frontmatter must NOT render that
    /// frontmatter as a giant setext heading. Without
    /// `ENABLE_YAML_STYLE_METADATA_BLOCKS`, CommonMark reads the closing `---` as a
    /// setext underline and promotes the whole metadata paragraph to an `<h2>` —
    /// the screenshot symptom (`name: docs description: … tools: Read, Edit, …`
    /// rendered enormous and run together).
    ///
    /// Negative control: drop the option from `parse::parse` → the first block is
    /// `HorizontalRule` and the second is `Heading { level: 2 }`, failing both
    /// asserts.
    #[test]
    fn yaml_frontmatter_is_metadata_not_a_heading() {
        let md = "---\nname: docs\ndescription: Editorial documentation agent\ntools: Read, Edit\n---\n\n# Real Title\n\nBody text.\n";
        let blocks = render(md, &Theme::default());

        assert!(
            matches!(blocks.first(), Some(RenderedBlock::Metadata { .. })),
            "frontmatter renders as its own metadata block; got {:?}",
            blocks.first()
        );
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, RenderedBlock::Heading { level: 2, content }
                    if content.text_content().contains("name: docs"))),
            "frontmatter must never be promoted to a heading; got {blocks:?}"
        );
        // The document's REAL heading survives and is the dominant one.
        assert!(
            blocks.iter().any(|b| matches!(b, RenderedBlock::Heading { level: 1, content }
                if content.text_content() == "Real Title")),
            "the document's own H1 still renders; got {blocks:?}"
        );
        // The line breaks the setext-paragraph collapse ate are preserved.
        let Some(RenderedBlock::Metadata { lines }) = blocks.first() else {
            unreachable!()
        };
        assert_eq!(
            lines.len(),
            3,
            "one styled line per frontmatter source line; got {lines:?}"
        );
    }
}

#[cfg(test)]
mod highlight_theme_tests {
    use super::*;
    use crate::style::Color;
    use crate::theme::ThemeName;

    fn code_fg(theme: &Theme, needle: &str) -> Color {
        let md = "```rust\nlet s = \"hi\";\n```\n";
        let blocks = render(md, theme);
        let lines = blocks
            .iter()
            .find_map(|b| match b {
                RenderedBlock::CodeBlock { lines, .. } => Some(lines),
                _ => None,
            })
            .expect("a code block");
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text.contains(needle))
            .unwrap_or_else(|| panic!("no token containing {needle:?}"))
            .style
            .fg
            .expect("token fg")
    }

    /// The markdown *block* path (buffer Viewing + sub-agent timeline) must honor
    /// the active theme, not always paint base16-ocean.dark. Under Folio the
    /// string token is Folio sage; under a dark theme it differs.
    ///
    /// Negative control: revert `render()` to `Highlighter::new()` and both the
    /// `== sage` assert and the `!=` assert fail (dark tokens under both themes).
    #[test]
    fn code_fences_follow_the_active_theme() {
        let folio_string = code_fg(&Theme::folio(), "hi");
        assert_eq!(
            folio_string,
            Color::Rgb(0x4f, 0x6d, 0x1f),
            "Folio code fence string should be olive, not base16-ocean.dark"
        );
        let dark_string = code_fg(&Theme::from_name(ThemeName::Dracula), "\"");
        assert_ne!(
            folio_string, dark_string,
            "block path must vary syntax colors by theme"
        );
    }
}

#[cfg(test)]
mod render_fixes_tests {
    //! graph 4f1 `render-fixes`: GFM constructs the block model used to drop.
    use super::*;

    fn blocks(md: &str) -> Vec<RenderedBlock> {
        render(md, &Theme::default())
    }

    fn texts(lines: &[StyledLine]) -> Vec<String> {
        lines.iter().map(|l| l.text_content()).collect()
    }

    /// Negative control: collapse `HardBreak` to a space in `collect_inline`
    /// → one line `"one two three soft"`.
    #[test]
    fn hard_break_starts_a_new_line_soft_break_does_not() {
        let b = blocks("one  \ntwo\\\nthree\nsoft\n");
        let [RenderedBlock::Paragraph { lines }] = b.as_slice() else {
            panic!("one paragraph: {b:?}")
        };
        assert_eq!(texts(lines), vec!["one", "two", "three soft"]);
    }

    /// The tight-list path (no `Paragraph` wrapper) breaks lines too, and keeps
    /// an item's text when a nested list follows it.
    #[test]
    fn tight_list_items_keep_hard_breaks_and_text_before_nested_lists() {
        let b = blocks("- a  \n  b\n- c\n  - nested\n");
        let [RenderedBlock::List { items, .. }] = b.as_slice() else {
            panic!("one list: {b:?}")
        };
        let [RenderedBlock::Paragraph { lines }] = items[0].content.as_slice() else {
            panic!("item 0 is one paragraph: {:?}", items[0].content)
        };
        assert_eq!(texts(lines), vec!["a", "b"]);
        assert!(
            matches!(items[1].content.as_slice(),
                [RenderedBlock::Paragraph { lines }, RenderedBlock::List { .. }]
                    if lines[0].text_content() == "c"),
            "item text survives a nested list: {:?}",
            items[1].content
        );
    }

    #[test]
    fn task_items_carry_their_checked_state() {
        let b = blocks("- [ ] todo\n- [x] done\n- plain\n  - [x] nested\n");
        let [RenderedBlock::List { items, .. }] = b.as_slice() else {
            panic!("one list: {b:?}")
        };
        let checked: Vec<_> = items.iter().map(|i| i.checked).collect();
        assert_eq!(checked, vec![Some(false), Some(true), None]);
        // A nested item's marker belongs to the nested item, not its parent.
        let Some(RenderedBlock::List { items: nested, .. }) = items[2].content.last() else {
            panic!("nested list: {:?}", items[2].content)
        };
        assert_eq!(nested[0].checked, Some(true));
    }

    /// Negative control: drop the lone-image lift in the `Paragraph` arm →
    /// a `Paragraph` holding the alt text.
    #[test]
    fn an_image_alone_in_its_paragraph_is_an_image_block() {
        let b = blocks("![A cat](img/cat.png)\n\n![](pics/dog.jpg)\n");
        assert_eq!(
            b,
            vec![
                RenderedBlock::Image {
                    alt: "A cat".into(),
                    url: "img/cat.png".into()
                },
                RenderedBlock::Image {
                    alt: "dog.jpg".into(),
                    url: "pics/dog.jpg".into()
                },
            ]
        );
    }

    /// Negative control: ignore `Start(Image)` in `collect_inline` → the alt
    /// text is plain, unlinked prose.
    #[test]
    fn an_inline_image_is_a_link_styled_alt_span() {
        let theme = Theme::default();
        let b = render("See ![the chart](c.png) here.", &theme);
        let [RenderedBlock::Paragraph { lines }] = b.as_slice() else {
            panic!("one paragraph: {b:?}")
        };
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.text == "the chart")
            .expect("alt text span");
        assert_eq!(span.link.as_deref(), Some("c.png"));
        assert_eq!(span.style, Style::default().patch(theme.paragraph).patch(theme.link));
        assert_eq!(lines[0].text_content(), "See the chart here.");
    }

    /// Negative control: remove `ENABLE_FOOTNOTES` from `parse::options` →
    /// the reference stays literal `[^1]` text and no `Footnote` block exists.
    #[test]
    fn footnotes_render_markers_and_definition_blocks() {
        let theme = Theme::default();
        let b = render("Claim[^1] and[^note].\n\n[^1]: The source.\n\n[^note]: Aside.\n", &theme);
        let RenderedBlock::Paragraph { lines } = &b[0] else {
            panic!("paragraph first: {b:?}")
        };
        assert_eq!(lines[0].text_content(), "Claim¹ and[note].");
        let marker = lines[0].spans.iter().find(|s| s.text == "¹").expect("marker");
        assert_eq!(marker.style, Style::default().patch(theme.paragraph).patch(theme.link));
        let notes: Vec<_> = b
            .iter()
            .filter_map(|blk| match blk {
                RenderedBlock::Footnote { label, blocks } => Some((label.as_str(), blocks.len())),
                _ => None,
            })
            .collect();
        assert_eq!(notes, vec![("1", 1), ("note", 1)]);
        assert_eq!(footnote_marker("10"), "¹⁰");
    }
}

#[cfg(test)]
mod source_map_tests {
    use super::*;

    const CORPUS: &[&str] = &[
        "",
        "plain",
        "# Title\n\nPara one\nstill one.\n\n## Sub\n\n- a\n- b\n  - nested\n\n> quote\n> more\n",
        "---\ntitle: x\n---\n# After frontmatter\n\ntext\n",
        "```rust\n# not a heading\nfn a() {}\n```\n\n# Real\n",
        "| a | b |\n|:--|--:|\n| 1 | 2 |\n\n***\n\n1. one\n2. two\n",
        "Setext\n======\n\nbody\n\nSub\n---\n",
        "- [ ] todo\n- [x] done\n\n![alt](img.png)\n\n    indented code\n",
        "<div>html</div>\n\npara after html\n",
        "no trailing newline at end",
        "\n\n\n# heading after blanks\n\n\n",
        "```mermaid\ngraph TD; A-->B;\n```\n",
        // render-fixes (graph 4f1): lone/inline images, hard breaks, footnotes,
        // tight task lists with nested blocks.
        "![only](a.png)\n\nText ![inline](b.png) more.\n\n![](c/d.png \"t\")\n",
        "one  \ntwo\\\nthree\nsoft\n\n- tight  \n  broken\n- [x] done\n  - [ ] sub\n",
        "Note[^1] and[^x].\n\n[^1]: First.\n\n[^x]: Second\n    para.\n\nAfter.\n",
    ];

    fn theme() -> Theme {
        Theme::default()
    }

    /// Every top-level block gets exactly one span; spans are ordered,
    /// non-overlapping, inside the text, and their line ranges agree with the
    /// bytes they cover.
    #[test]
    fn spans_tile_the_document_in_order() {
        for md in CORPUS {
            let r = render_mapped(md, &theme());
            assert_eq!(r.blocks, render(md, &theme()), "mapping must not change blocks: {md:?}");
            assert_eq!(r.blocks.len(), r.spans.len(), "one span per block: {md:?}");
            let mut prev_end = 0;
            for s in &r.spans {
                assert!(s.bytes.start >= prev_end, "ordered/non-overlapping: {md:?} {s:?}");
                assert!(s.bytes.start < s.bytes.end && s.bytes.end <= md.len(), "{md:?} {s:?}");
                let line = md[..s.bytes.start].matches('\n').count();
                assert_eq!(s.lines.start, line, "start line: {md:?} {s:?}");
                assert!(s.lines.end > s.lines.start, "{md:?} {s:?}");
                prev_end = s.bytes.end;
            }
        }
    }

    #[test]
    fn spans_point_at_their_source() {
        let md = "# Title\n\nPara\n\n## Sub\n\n```rust\nfn a() {}\n```\n";
        let r = render_mapped(md, &theme());
        let src: Vec<&str> = r.spans.iter().map(|s| &md[s.bytes.clone()]).collect();
        assert!(src[0].starts_with("# Title"));
        assert!(src[1].starts_with("Para"));
        assert!(src[2].starts_with("## Sub"));
        assert!(src[3].starts_with("```rust"));
        assert_eq!(r.spans[3].lines, 6..9);
        assert_eq!(r.block_at_line(0), Some(0));
        assert_eq!(r.block_at_line(1), Some(0), "blank line maps to preceding block");
        assert_eq!(r.block_at_line(7), Some(3));
    }

    #[test]
    fn outline_excludes_code_and_includes_setext_and_nested() {
        let md = "    # indented code\n\n# One\n\n```sh\n# comment, not a heading\n```\n\nTwo\n===\n\n- item\n\n  ## Nested in list\n\n### `code` *three*\n";
        let o = outline(md);
        let got: Vec<(u8, &str, usize)> =
            o.iter().map(|h| (h.level, h.text.as_str(), h.line)).collect();
        assert_eq!(
            got,
            vec![(1, "One", 2), (1, "Two", 8), (2, "Nested in list", 13), (3, "code three", 15)]
        );
    }
}
