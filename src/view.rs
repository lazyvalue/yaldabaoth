use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::blocks::*;
use crate::theme::Theme;
use crate::viewport::Viewport;

pub struct ViewState<'a> {
    pub filename: &'a str,
    pub blocks: &'a [RenderedBlock],
    pub viewport: &'a Viewport,
    pub theme: &'a Theme,
    pub mode_label: &'a str,
    pub search_query: &'a str,
    pub search_input_mode: bool,
    pub search_input_buffer: &'a str,
    pub search_match_count: usize,
    pub menu_active: bool,
    pub menu_nodes: Vec<(char, String, bool)>, // (key, label, is_submenu)
    pub menu_label: Option<String>,            // submenu breadcrumb label
    pub file_browser_open: bool,
    pub file_browser_dir: String,
    pub file_browser_entries: Vec<(String, bool, bool)>, // (name, is_dir, is_selected)
    pub file_browser_filter_mode: bool,
    pub file_browser_filter_text: String,
    pub file_browser_panel_width: u16,
    pub file_browser_hint: String,
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
    ])
    .areas(area);

    draw_top_bar(frame, top_bar, state);
    if state.file_browser_open {
        let [browser_area, doc_area] = Layout::horizontal([
            Constraint::Length(state.file_browser_panel_width),
            Constraint::Min(1),
        ])
        .areas(content_area);

        draw_file_browser_panel(frame, browser_area, state);
        draw_content(frame, doc_area, state);
    } else {
        draw_content(frame, content_area, state);
    }
    if state.menu_active {
        draw_menu_popup(frame, content_area, state);
    }
    draw_bottom_bar(frame, bottom_bar, state);
}

fn draw_top_bar(frame: &mut Frame, area: Rect, state: &ViewState) {
    let _viewport_height = (frame.area().height as usize).saturating_sub(2);
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
    if state.search_input_mode {
        let prompt = format!("/{}", state.search_input_buffer);
        let available = area.width as usize;
        let padding = available.saturating_sub(prompt.len() + 1);
        let line = Line::from(vec![
            Span::styled(format!(" {}", prompt), state.theme.bottom_bar),
            Span::styled(" ".repeat(padding), state.theme.bottom_bar),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let hints = "j/k scroll · {/} heading · / search · q quit";
    let available = area.width as usize;
    let mode_len = state.mode_label.len();

    let match_info = if state.search_match_count > 0 && !state.search_query.is_empty() {
        format!(" [{}] ", state.search_match_count)
    } else {
        String::new()
    };

    let padding = available.saturating_sub(mode_len + match_info.len() + hints.len() + 3);

    let line = Line::from(vec![
        Span::styled(format!(" {}", state.mode_label), state.theme.mode_indicator),
        Span::styled(match_info, state.theme.mode_indicator),
        Span::styled(" ".repeat(padding), state.theme.bottom_bar),
        Span::styled(format!("{} ", hints), state.theme.bottom_bar),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_menu_popup(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Menu takes 2 rows: label row + entries row
    let popup_height = 2u16;
    let popup_area = Rect::new(area.x, area.y, area.width, popup_height.min(area.height));

    // Opaque background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 58)));
    frame.render_widget(bg, popup_area);

    // Label row
    let label_text = state.menu_label.as_deref().unwrap_or("Commands");
    let label_line = Line::from(Span::styled(
        format!("  {}", label_text.to_uppercase()),
        Style::default().fg(Color::Rgb(98, 114, 164)),
    ));
    if popup_area.height >= 1 {
        frame.render_widget(
            Paragraph::new(label_line),
            Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
        );
    }

    // Entries row
    if popup_area.height >= 2 {
        let mut spans = vec![Span::raw("  ")];
        for (i, (key, label, is_submenu)) in state.menu_nodes.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(Color::Rgb(189, 147, 249))
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            if *is_submenu {
                spans.push(Span::styled(
                    format!("{} \u{25b8}", label),
                    Style::default().fg(Color::Rgb(139, 233, 253)),
                ));
            } else {
                spans.push(Span::styled(
                    label.clone(),
                    Style::default().fg(Color::Rgb(204, 204, 204)),
                ));
            }
        }
        let entries_line = Line::from(spans);
        frame.render_widget(
            Paragraph::new(entries_line),
            Rect::new(popup_area.x, popup_area.y + 1, popup_area.width, 1),
        );
    }
}

fn draw_file_browser_panel(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Split panel into: header (1), optional filter (1), file list (fill), footer (1)
    let has_filter = state.file_browser_filter_mode;
    let constraints = if has_filter {
        vec![
            Constraint::Length(1), // header
            Constraint::Length(1), // filter input
            Constraint::Min(1),    // file list
            Constraint::Length(1), // footer
        ]
    } else {
        vec![
            Constraint::Length(1), // header
            Constraint::Min(1),    // file list
            Constraint::Length(1), // footer
        ]
    };
    let areas = Layout::vertical(constraints).split(area);

    let (header_area, filter_area, list_area, footer_area) = if has_filter {
        (areas[0], Some(areas[1]), areas[2], areas[3])
    } else {
        (areas[0], None, areas[1], areas[2])
    };

    // Border separator on right edge
    for y in area.y..area.y + area.height {
        let sep_area = Rect::new(area.x + area.width - 1, y, 1, 1);
        frame.render_widget(
            Paragraph::new("│").style(Style::default().fg(Color::Rgb(98, 114, 164))),
            sep_area,
        );
    }

    let panel_width = area.width.saturating_sub(1); // exclude border

    // Header
    let dir_display = if state.file_browser_dir.len() > panel_width as usize - 2 {
        let start = state.file_browser_dir.len() - (panel_width as usize - 2);
        format!(" …{}", &state.file_browser_dir[start..])
    } else {
        format!(" {}", state.file_browser_dir)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            dir_display,
            Style::default().fg(Color::Rgb(98, 114, 164)),
        ))),
        Rect::new(header_area.x, header_area.y, panel_width, 1),
    );

    // Filter input
    if let Some(filter_area) = filter_area {
        let filter_line = Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Rgb(255, 184, 108))),
            Span::styled(
                &state.file_browser_filter_text,
                Style::default().fg(Color::Rgb(241, 250, 140)),
            ),
            Span::styled("▎", Style::default().fg(Color::Rgb(102, 102, 102))),
        ]);
        frame.render_widget(
            Paragraph::new(filter_line),
            Rect::new(filter_area.x + 1, filter_area.y, panel_width - 1, 1),
        );
    }

    // File list
    let list_height = list_area.height as usize;
    if state.file_browser_entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (empty)",
                Style::default().fg(Color::Rgb(102, 102, 102)),
            ))),
            Rect::new(list_area.x + 1, list_area.y, panel_width - 1, 1),
        );
    }
    for (i, (name, is_dir, is_selected)) in state.file_browser_entries.iter().enumerate() {
        if i >= list_height {
            break;
        }

        let marker = if *is_selected { "▸ " } else { "  " };
        let style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };
        let name_style = if *is_dir {
            style.fg(Color::Rgb(139, 233, 253))
        } else {
            style.fg(Color::Rgb(204, 204, 204))
        };

        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(name.clone(), name_style),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(list_area.x + 1, list_area.y + i as u16, panel_width - 1, 1),
        );
    }

    // Footer
    let hint = &state.file_browser_hint;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", hint),
            Style::default().fg(Color::Rgb(102, 102, 102)),
        ))),
        Rect::new(footer_area.x, footer_area.y, panel_width, 1),
    );
}

fn draw_content(frame: &mut Frame, area: Rect, state: &ViewState) {
    let terminal_width = area.width as usize;
    let viewport_height = area.height as usize;
    let content_width = state.viewport.content_width(terminal_width);
    let x_offset = state.viewport.content_offset(terminal_width);

    let visible = state
        .viewport
        .visible_blocks(state.blocks, content_width, viewport_height);

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
                lines.push(StyledLine::new(vec![StyledSpan::new(
                    rule,
                    theme.horizontal_rule,
                )]));
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
                        if remaining == 0 {
                            break;
                        }
                        if span.text.len() <= remaining {
                            truncated_spans.push(span.clone());
                            remaining -= span.text.len();
                        } else {
                            truncated_spans
                                .push(StyledSpan::new(&span.text[..remaining], span.style));
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
                let inner_lines =
                    render_block_to_lines(inner_block, width.saturating_sub(4), theme);
                for line in inner_lines {
                    let mut spans = vec![StyledSpan::new("▎ ", theme.blockquote_bar)];
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
                    let inner_lines = render_block_to_lines(
                        content_block,
                        width.saturating_sub(marker_display.len()),
                        theme,
                    );
                    for line in inner_lines {
                        let mut spans = if first {
                            first = false;
                            vec![StyledSpan::new(&marker_display, theme.list_marker)]
                        } else {
                            vec![StyledSpan::new(
                                " ".repeat(marker_display.len()),
                                Style::default(),
                            )]
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
            // Calculate column widths from both headers and row data
            let mut col_widths: Vec<usize> = headers
                .iter()
                .map(|h| h.text_content().len().max(3))
                .collect();
            for row in rows {
                for (i, cell) in row.iter().enumerate() {
                    if i < col_widths.len() {
                        col_widths[i] = col_widths[i].max(cell.text_content().len());
                    }
                }
            }

            // Header
            let header_spans: Vec<StyledSpan> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let padded = format!(
                        "{:<width$}",
                        h.text_content(),
                        width = col_widths.get(i).copied().unwrap_or(5)
                    );
                    StyledSpan::new(padded, theme.table_header)
                })
                .collect();
            let mut hline_spans = Vec::new();
            for (i, span) in header_spans.into_iter().enumerate() {
                if i > 0 {
                    hline_spans.push(StyledSpan::new(" │ ", theme.table_border));
                }
                hline_spans.push(span);
            }
            out.push(StyledLine::new(hline_spans));

            // Separator
            let sep: String = col_widths
                .iter()
                .map(|w| "─".repeat(*w))
                .collect::<Vec<_>>()
                .join("─┼─");
            out.push(StyledLine::new(vec![StyledSpan::new(
                sep,
                theme.table_border,
            )]));

            // Rows
            for row in rows {
                let mut row_spans = Vec::new();
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 {
                        row_spans.push(StyledSpan::new(" │ ", theme.table_border));
                    }
                    let padded = format!(
                        "{:<width$}",
                        cell.text_content(),
                        width = col_widths.get(i).copied().unwrap_or(5)
                    );
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
        line.spans
            .iter()
            .map(|s| Span::styled(s.text.clone(), s.style))
            .collect::<Vec<_>>(),
    )
}
