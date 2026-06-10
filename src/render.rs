use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};

use crate::blocks::*;
use crate::highlight::Highlighter;
use crate::parse;
use crate::style::Style;
use crate::theme::Theme;

pub fn render(markdown: &str, theme: &Theme) -> Vec<RenderedBlock> {
    let highlighter = Highlighter::new();
    render_with_highlighter(markdown, theme, &highlighter)
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
        self.spans.push(StyledSpan {
            text: text.to_string(),
            style,
            link,
        });
    }

    fn into_line(self) -> StyledLine {
        StyledLine::new(self.spans)
    }
}

impl<'a, 't> Renderer<'a, 't> {
    fn new(theme: &'t Theme, highlighter: &'a Highlighter) -> Self {
        Self { theme, highlighter }
    }

    fn render(&mut self, events: &[Event<'_>]) -> Vec<RenderedBlock> {
        let mut blocks = Vec::new();
        let mut i = 0;

        while i < events.len() {
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
                    let mut state = InlineState::new(self.theme.paragraph);
                    i = self.collect_inline(events, i, &TagEnd::Paragraph, &mut state);
                    blocks.push(RenderedBlock::Paragraph {
                        lines: vec![state.into_line()],
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

                    blocks.push(RenderedBlock::CodeBlock {
                        language,
                        lines,
                        source_file: false,
                        start_line: 0,
                    });
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
                Event::Start(Tag::Image {
                    dest_url, title, ..
                }) => {
                    let url = dest_url.to_string();
                    let title_str = title.to_string();
                    i += 1;
                    let mut alt = String::new();
                    while i < events.len() {
                        match &events[i] {
                            Event::Text(t) => {
                                alt.push_str(t.as_ref());
                                i += 1;
                            }
                            Event::End(TagEnd::Image) => {
                                i += 1;
                                break;
                            }
                            _ => {
                                i += 1;
                            }
                        }
                    }
                    if alt.is_empty() && !title_str.is_empty() {
                        alt = title_str;
                    }
                    if alt.is_empty() {
                        alt = url.rsplit('/').next().unwrap_or(&url).to_string();
                    }
                    blocks.push(RenderedBlock::Image { alt, url });
                }
                _ => {
                    i += 1;
                }
            }
        }

        blocks
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
                            Event::TaskListMarker(c) => {
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
                    let mut content = self.render(&item_events);
                    // Handle tight lists: pulldown-cmark omits Paragraph wrappers
                    // in tight lists, leaving bare Text events that self.render() ignores.
                    // Collect them into a paragraph manually.
                    if content.is_empty() && !item_events.is_empty() {
                        let mut state = InlineState::new(self.theme.paragraph);
                        for event in &item_events {
                            match event {
                                Event::Text(t) => state.push_text(t.as_ref()),
                                Event::Code(t) => {
                                    state.style_stack.push(self.theme.code_inline);
                                    state.push_text(t.as_ref());
                                    state.style_stack.pop();
                                }
                                Event::SoftBreak | Event::HardBreak => state.push_text(" "),
                                Event::Start(Tag::Strong) => {
                                    state.style_stack.push(self.theme.bold)
                                }
                                Event::End(TagEnd::Strong) => {
                                    state.style_stack.pop();
                                }
                                Event::Start(Tag::Emphasis) => {
                                    state.style_stack.push(self.theme.italic)
                                }
                                Event::End(TagEnd::Emphasis) => {
                                    state.style_stack.pop();
                                }
                                Event::Start(Tag::Strikethrough) => {
                                    state.style_stack.push(self.theme.strikethrough)
                                }
                                Event::End(TagEnd::Strikethrough) => {
                                    state.style_stack.pop();
                                }
                                Event::Start(Tag::Link { dest_url, .. }) => {
                                    state.style_stack.push(self.theme.link);
                                    state.link_stack.push(Some(dest_url.to_string()));
                                }
                                Event::End(TagEnd::Link) => {
                                    state.style_stack.pop();
                                    state.link_stack.pop();
                                }
                                _ => {}
                            }
                        }
                        let line = state.into_line();
                        if !line.spans.is_empty() {
                            content.push(RenderedBlock::Paragraph { lines: vec![line] });
                        }
                    }
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
