use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::blocks::*;
use crate::menu::MenuNodeKind;
use crate::style::{Color, Modifier, Style};
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Global view mode: either fully rendered or fully raw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    Rendered,
    Raw,
}

pub struct FullBrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_selected: bool,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

pub struct FullBrowserViewState {
    pub dir: String,
    pub entries: Vec<FullBrowserEntry>,
    pub filter_mode: bool,
    pub filter_text: String,
    pub came_from_dropdown: bool,
    pub sort_label: String,
}

pub struct FullBufferListEntry {
    pub path: String,
    pub is_modified: bool,
    pub is_active: bool,
    pub is_selected: bool,
}

pub struct FullBufferListViewState {
    pub entries: Vec<FullBufferListEntry>,
    pub filter_mode: bool,
    pub filter_text: String,
    pub total_count: usize,
}

pub struct ViewState<'a> {
    pub filename: &'a str,
    pub modified: bool,
    pub view_mode: ViewMode,
    pub rendered_blocks: &'a [RenderedBlock],
    pub raw_lines: &'a [String],
    pub raw_highlights: &'a [Vec<(String, Style)>],
    pub viewport: &'a Viewport,
    pub theme: &'a Theme,
    pub mode_label: &'a str,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub show_block_cursor: bool,
    pub search_query: &'a str,
    pub search_input_mode: bool,
    pub search_input_buffer: &'a str,
    pub search_match_count: usize,
    pub search_matches: &'a [(usize, usize)], // (line, col) pairs
    pub search_current_match: usize,
    pub rendered_cursor_row: usize,
    pub rendered_cursor_col: usize,
    pub menu_active: bool,
    pub menu_nodes: Vec<(String, String, MenuNodeKind)>, // (key_display, label, kind)
    pub menu_label: Option<String>,                      // submenu breadcrumb label
    pub file_browser_open: bool,
    pub file_browser_dir: String,
    pub file_browser_entries: Vec<(String, bool, bool)>, // (name, is_dir, is_selected)
    pub file_browser_filter_mode: bool,
    pub file_browser_filter_text: String,
    pub command_mode: bool,
    pub command_buffer: &'a str,
    pub command_error: &'a str,
    pub buffer_list_open: bool,
    pub buffer_list_entries: Vec<(String, bool, bool, bool)>, // (path, is_modified, is_active, is_selected)
    pub buffer_list_filter_mode: bool,
    pub buffer_list_filter_text: String,
    pub buffer_count: usize,
    pub active_buffer_index: usize,
    pub outline_open: bool,
    pub outline_entries: Vec<(String, u8, bool)>, // (title, level, is_selected)
    pub outline_filter_mode: bool,
    pub outline_filter_text: String,
    pub outline_breadcrumb: Option<String>,
    pub nav_mode_label: Option<String>,
    pub nav_highlight: Option<(usize, usize, usize)>, // (rendered_row, col_start, col_end)
    pub full_browser: Option<FullBrowserViewState>,
    pub full_buffer_list: Option<FullBufferListViewState>,
    /// Active selection range in raw mode as ((start_line, start_col), (end_line, end_col))
    pub selection: Option<((usize, usize), (usize, usize))>,
    /// Whether the editor is in extend (selection-growing) mode
    pub extend_mode: bool,
    /// Char ranges in the rope where Claude's prose lives (only meaningful in
    /// the *claude* buffer). Used to render Claude's text differently from
    /// the user's inline insertions.
    pub frozen_ranges: Vec<(usize, usize)>,
    /// Char index of the locked-prefix boundary. Chars below this are styled
    /// muted (older locked turns).
    pub lockable_through_char: usize,
    // --- Compose textbox state ---
    pub compose_active: bool,
    pub compose_lines: Vec<String>,
    pub compose_cursor_line: usize,
    pub compose_cursor_col: usize,
    pub compose_insert_mode: bool,
}

/// Ground-truth feedback from the renderer back to App, populated during
/// `draw` so callers (and the debug overlay) can detect when scroll math
/// disagrees with what was actually painted.
#[derive(Debug, Default, Clone, Copy)]
pub struct DrawReport {
    /// Height of the content_area chunk the renderer used (the real number
    /// of doc rows it can paint).
    pub content_area_height: u16,
    /// Y coordinate (in screen rows) where the cursor was painted, or None
    /// if the cursor wasn't on-screen this frame.
    pub cursor_screen_y: Option<u16>,
    /// First and last doc-line indices that had any visual row painted.
    pub first_visible_doc_line: Option<usize>,
    pub last_visible_doc_line: Option<usize>,
    /// Number of visual rows the renderer actually painted (excludes the
    /// rows it skipped past scroll_offset and the rows below viewport).
    pub painted_rows: usize,
    /// True if the splash screen was shown instead of buffer content. The
    /// cursor is intentionally not drawn in this case — debug logging should
    /// ignore "off-screen" status here.
    pub is_splash: bool,
}

pub fn draw(frame: &mut Frame, state: &ViewState, report: &mut DrawReport) {
    let area = frame.area();

    if area.width < 40 || area.height < 5 {
        let msg = Paragraph::new("Terminal too small (min 40x5)");
        frame.render_widget(msg, area);
        return;
    }

    if let Some(ref fb_state) = state.full_browser {
        draw_full_file_browser(frame, area, fb_state, state.theme);
        return;
    }

    if let Some(ref bl_state) = state.full_buffer_list {
        draw_full_buffer_list(frame, area, bl_state, state.theme);
        return;
    }

    // Calculate buffer list height
    let buffer_list_height = if state.buffer_list_open {
        let max_height = (area.height as usize) / 3;
        let entry_rows =
            state.buffer_list_entries.len() + if state.buffer_list_filter_mode { 1 } else { 0 };
        entry_rows.min(max_height).max(1) as u16
    } else {
        0
    };

    // Calculate file browser height
    let file_browser_height = if state.file_browser_open {
        let max_height = (area.height as usize) / 2;
        let header_rows = 1;
        let filter_rows = if state.file_browser_filter_mode { 1 } else { 0 };
        let entry_rows = state.file_browser_entries.len();
        (header_rows + filter_rows + entry_rows)
            .min(max_height)
            .max(1) as u16
    } else {
        0
    };

    // Calculate outline height
    let outline_height = if state.outline_open {
        let max_height = (area.height as usize) / 3;
        let header_rows = if state.outline_breadcrumb.is_some() {
            1
        } else {
            0
        };
        let filter_rows = if state.outline_filter_mode { 1 } else { 0 };
        let entry_rows = state.outline_entries.len().max(1);
        (header_rows + filter_rows + entry_rows)
            .min(max_height)
            .max(1) as u16
    } else {
        0
    };

    let needs_bottom_bar =
        state.command_mode || state.search_input_mode || !state.command_error.is_empty();
    let bottom_bar_height = if needs_bottom_bar { 1u16 } else { 0 };

    // Compose textbox panel
    let compose_height = if state.compose_active {
        let lines = state.compose_lines.len().max(1);
        let capped = lines.min((area.height as usize) / 3).clamp(3, 12);
        (capped + 1) as u16 // +1 for separator line
    } else {
        0
    };

    let chunks = Layout::vertical([
        Constraint::Length(1),                   // top bar
        Constraint::Length(buffer_list_height),  // buffer list
        Constraint::Length(file_browser_height), // file browser
        Constraint::Length(outline_height),      // outline
        Constraint::Min(1),                      // content
        Constraint::Length(compose_height),      // compose textbox
        Constraint::Length(bottom_bar_height),   // bottom bar (conditional)
    ])
    .split(area);

    let top_bar = chunks[0];
    let buffer_list_area = chunks[1];
    let file_browser_area = chunks[2];
    let outline_area = chunks[3];
    let content_area = chunks[4];
    let compose_area = chunks[5];
    let bottom_bar = chunks[6];

    draw_top_bar(frame, top_bar, state);

    if state.buffer_list_open && buffer_list_height > 0 {
        draw_buffer_list(frame, buffer_list_area, state);
    }

    if state.file_browser_open && file_browser_height > 0 {
        draw_file_browser_panel(frame, file_browser_area, state);
    }

    if state.outline_open && outline_height > 0 {
        draw_outline(frame, outline_area, state);
    }

    report.content_area_height = content_area.height;
    draw_content(frame, content_area, state, report);
    if state.menu_active {
        draw_menu_popup(frame, content_area, state);
    }
    if state.compose_active && compose_height > 0 {
        draw_compose_box(frame, compose_area, state);
    }
    if needs_bottom_bar {
        draw_bottom_bar(frame, bottom_bar, state);
    }
}

fn draw_top_bar(frame: &mut Frame, area: Rect, state: &ViewState) {
    let current_line = state.viewport.scroll_offset + 1;
    let total = state.viewport.total_lines.max(1);
    let percent = (state.viewport.scroll_offset * 100) / total.max(1);

    let buffer_info = if state.buffer_count > 1 {
        format!(
            " [{}/{}]",
            state.active_buffer_index + 1,
            state.buffer_count
        )
    } else {
        String::new()
    };

    let nav_info = state.nav_mode_label.as_deref().unwrap_or("");
    let nav_display = if !nav_info.is_empty() {
        format!(" [{}]", nav_info)
    } else {
        String::new()
    };
    let extend_display = if state.extend_mode { " [SEL]" } else { "" };
    let nav_display = format!("{}{}", nav_display, extend_display);

    let mode_display = format!(" {}", state.mode_label);
    let position = format!(
        "line {}/{} {}%{}{}",
        current_line, total, percent, buffer_info, nav_display
    );
    let available = area.width as usize;

    let modified_indicator = if state.modified { " [+]" } else { "" };
    let name_with_mod = format!("{}{}", state.filename, modified_indicator);
    let name_width = available.saturating_sub(position.len() + mode_display.len() + 2);
    let name_display = if name_with_mod.len() > name_width {
        &name_with_mod[name_with_mod.len() - name_width..]
    } else {
        &name_with_mod
    };

    let padding =
        available.saturating_sub(name_display.len() + mode_display.len() + position.len() + 1);
    let line = Line::from(vec![
        Span::styled(format!(" {}", name_display), state.theme.top_bar),
        Span::styled(mode_display, state.theme.top_bar_mode),
        Span::styled(" ".repeat(padding), state.theme.top_bar),
        Span::styled(format!("{} ", position), state.theme.top_bar),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_bottom_bar(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Command mode: show :buffer
    if state.command_mode {
        let prompt = format!(":{}", state.command_buffer);
        let available = area.width as usize;
        let padding = available.saturating_sub(prompt.len() + 1);
        let line = Line::from(vec![
            Span::styled(format!(" {}", prompt), state.theme.bottom_bar),
            Span::styled(" ".repeat(padding), state.theme.bottom_bar),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // Search input mode
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

    // Command error display
    if !state.command_error.is_empty() {
        let available = area.width as usize;
        let padding = available.saturating_sub(state.command_error.len() + 1);
        let line = Line::from(vec![
            Span::styled(
                format!(" {}", state.command_error),
                Style::default().fg(Color::Rgb(255, 85, 85)),
            ),
            Span::styled(" ".repeat(padding), state.theme.bottom_bar),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}

fn draw_compose_box(frame: &mut Frame, area: Rect, state: &ViewState) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let separator_style = Style::default()
        .fg(Color::Rgb(100, 100, 120))
        .add_modifier(Modifier::DIM);
    let compose_bg = Style::default().bg(Color::Rgb(25, 25, 40));

    // First row: separator
    let label = if state.compose_insert_mode {
        " compose (insert) "
    } else {
        " compose "
    };
    let width = area.width as usize;
    let dash_total = width.saturating_sub(label.len());
    let left_dashes = dash_total / 2;
    let right_dashes = dash_total - left_dashes;
    let sep_text = format!(
        "{}{}{}",
        "─".repeat(left_dashes),
        label,
        "─".repeat(right_dashes),
    );
    let sep_line = Line::from(Span::styled(sep_text, separator_style));
    let sep_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(sep_line), sep_area);

    // Remaining rows: compose text
    let text_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let text_height = text_area.height as usize;
    let text_width = text_area.width as usize;

    // Render compose lines, scrolling if needed to keep cursor visible
    let scroll_offset = if state.compose_cursor_line >= text_height {
        state.compose_cursor_line - text_height + 1
    } else {
        0
    };

    for row in 0..text_height {
        let line_idx = scroll_offset + row;
        let y = text_area.y + row as u16;
        let line_text = state
            .compose_lines
            .get(line_idx)
            .map(|s| s.as_str())
            .unwrap_or("");
        // Truncate to fit width
        let display: String = line_text.chars().take(text_width).collect();
        let padding = text_width.saturating_sub(display.len());
        let spans = vec![
            Span::styled(display, compose_bg.fg(Color::Rgb(220, 220, 220))),
            Span::styled(" ".repeat(padding), compose_bg),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: text_area.x,
                y,
                width: text_area.width,
                height: 1,
            },
        );
    }

    // Position cursor in the compose box
    let cursor_row_in_view = state.compose_cursor_line.saturating_sub(scroll_offset);
    if cursor_row_in_view < text_height {
        let cursor_y = text_area.y + cursor_row_in_view as u16;
        let cursor_x =
            text_area.x + (state.compose_cursor_col as u16).min(text_area.width.saturating_sub(1));
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_menu_popup(frame: &mut Frame, area: Rect, state: &ViewState) {
    let menu_bg = Style::default().bg(Color::Rgb(30, 30, 58));

    // Build displayable entries (skip separators for column layout)
    let entries: Vec<(&str, &str, &MenuNodeKind)> = state
        .menu_nodes
        .iter()
        .filter(|(_, _, kind)| !matches!(kind, MenuNodeKind::Separator))
        .map(|(k, l, kind)| (k.as_str(), l.as_str(), kind))
        .collect();

    // Determine column layout: use 2 columns if width >= 50 and we have > 1 entry
    let entry_count = entries.len();
    let use_two_cols = area.width >= 50 && entry_count > 1;
    let rows_needed = if use_two_cols {
        entry_count.div_ceil(2)
    } else {
        entry_count
    };

    // Total popup height: 1 label row + entry rows
    let popup_height = (1 + rows_needed as u16).min(area.height);
    let popup_area = Rect::new(area.x, area.y, area.width, popup_height);

    // Opaque background for the whole popup — fill every row with spaces
    for row in 0..popup_area.height {
        let fill = " ".repeat(popup_area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(fill, menu_bg))),
            Rect::new(popup_area.x, popup_area.y + row, popup_area.width, 1),
        );
    }

    // Label row — fill full width
    let label_text = state.menu_label.as_deref().unwrap_or("Commands");
    let label_str = format!("  {}", label_text.to_uppercase());
    let label_padding = (popup_area.width as usize).saturating_sub(label_str.len());
    let label_line = Line::from(vec![
        Span::styled(label_str, menu_bg.fg(Color::Rgb(98, 114, 164))),
        Span::styled(" ".repeat(label_padding), menu_bg),
    ]);
    if popup_area.height >= 1 {
        frame.render_widget(
            Paragraph::new(label_line),
            Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
        );
    }

    // Entry rows
    let col_width = if use_two_cols {
        popup_area.width / 2
    } else {
        popup_area.width
    };

    for (idx, (key_display, label, kind)) in entries.iter().enumerate() {
        let (row, col) = if use_two_cols {
            (idx / 2, idx % 2)
        } else {
            (idx, 0)
        };
        let y = popup_area.y + 1 + row as u16;
        if y >= popup_area.y + popup_area.height {
            break;
        }
        let x = popup_area.x + col as u16 * col_width;

        let entry_text = if **kind == MenuNodeKind::Label {
            vec![
                Span::styled("  ", menu_bg),
                Span::styled((*label).to_string(), menu_bg.fg(Color::Rgb(98, 114, 164))),
            ]
        } else {
            let mut spans = vec![
                Span::styled("  ", menu_bg),
                Span::styled(
                    (*key_display).to_string(),
                    menu_bg
                        .fg(Color::Rgb(189, 147, 249))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", menu_bg),
            ];
            if **kind == MenuNodeKind::Submenu {
                spans.push(Span::styled(
                    format!("{} \u{25b8}", label),
                    menu_bg.fg(Color::Rgb(139, 233, 253)),
                ));
            } else {
                spans.push(Span::styled(
                    (*label).to_string(),
                    menu_bg.fg(Color::Rgb(204, 204, 204)),
                ));
            }
            spans
        };

        // Pad the entry to fill its column width
        let text_len: usize = entry_text.iter().map(|s| s.content.len()).sum();
        let pad = (col_width as usize).saturating_sub(text_len);
        let mut spans = entry_text;
        spans.push(Span::styled(" ".repeat(pad), menu_bg));

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(x, y, col_width, 1),
        );
    }
}

fn draw_file_browser_panel(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    let mut y = 0u16;

    // Directory header
    if y < area.height {
        let dir_display = if state.file_browser_dir.len() > area.width as usize - 3 {
            let start = state.file_browser_dir.len() - (area.width as usize - 3);
            format!(" \u{2026}{}", &state.file_browser_dir[start..])
        } else {
            format!(" {}", state.file_browser_dir)
        };
        let header_line = Line::from(Span::styled(
            dir_display,
            Style::default().fg(Color::Rgb(98, 114, 164)),
        ));
        frame.render_widget(
            Paragraph::new(header_line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }

    // Filter input
    if state.file_browser_filter_mode && y < area.height {
        let filter_line = Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Rgb(255, 184, 108))),
            Span::styled(
                &state.file_browser_filter_text,
                Style::default().fg(Color::Rgb(241, 250, 140)),
            ),
            Span::styled("\u{258e}", Style::default().fg(Color::Rgb(102, 102, 102))),
        ]);
        frame.render_widget(
            Paragraph::new(filter_line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }

    // File entries
    if state.file_browser_entries.is_empty() && y < area.height {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (empty)",
                Style::default().fg(Color::Rgb(102, 102, 102)),
            ))),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        return;
    }

    // Compute scroll offset to keep selected entry visible
    let visible_rows = (area.height - y) as usize;
    let selected_idx = state
        .file_browser_entries
        .iter()
        .position(|(_, _, sel)| *sel)
        .unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    for (i, (name, is_dir, is_selected)) in state.file_browser_entries.iter().enumerate() {
        if i < scroll_offset {
            continue;
        }
        if y >= area.height {
            break;
        }

        let marker = if *is_selected { "\u{25b8} " } else { "  " };
        let bg_style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };
        let name_style = if *is_dir {
            bg_style.fg(Color::Rgb(139, 233, 253))
        } else {
            bg_style.fg(Color::Rgb(204, 204, 204))
        };
        let suffix = if *is_dir { "/" } else { "" };

        let line = Line::from(vec![
            Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
            Span::styled(format!("{}{}", name, suffix), name_style),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }
}

fn draw_buffer_list(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    let mut y = 0u16;

    // Filter input row
    if state.buffer_list_filter_mode && y < area.height {
        let filter_line = Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Rgb(255, 184, 108))),
            Span::styled(
                &state.buffer_list_filter_text,
                Style::default().fg(Color::Rgb(241, 250, 140)),
            ),
            Span::styled("\u{258e}", Style::default().fg(Color::Rgb(102, 102, 102))),
        ]);
        frame.render_widget(
            Paragraph::new(filter_line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }

    // Buffer entries — scroll to keep selection visible
    let visible_rows = (area.height - y) as usize;
    let selected_idx = state
        .buffer_list_entries
        .iter()
        .position(|(_, _, _, sel)| *sel)
        .unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    for (i, (path, is_modified, is_active, is_selected)) in
        state.buffer_list_entries.iter().enumerate()
    {
        if i < scroll_offset {
            continue;
        }
        if y >= area.height {
            break;
        }

        let marker = if *is_selected { "\u{25b8} " } else { "  " };
        let modified_indicator = if *is_modified { " [+]" } else { "" };

        let bg_style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };

        let path_style = if *is_active {
            bg_style
                .fg(Color::Rgb(139, 233, 253))
                .add_modifier(Modifier::BOLD)
        } else {
            bg_style.fg(Color::Rgb(204, 204, 204))
        };

        let line = Line::from(vec![
            Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
            Span::styled(path.clone(), path_style),
            Span::styled(
                modified_indicator.to_string(),
                bg_style.fg(Color::Rgb(255, 184, 108)),
            ),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }
}

fn draw_outline(frame: &mut Frame, area: Rect, state: &ViewState) {
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    let mut y = 0u16;

    // Breadcrumb header (when descended into a level)
    if let Some(ref breadcrumb) = state.outline_breadcrumb
        && y < area.height
    {
        let header_line = Line::from(vec![
            Span::styled(" \u{25c2} ", Style::default().fg(Color::Rgb(98, 114, 164))),
            Span::styled(
                breadcrumb.clone(),
                Style::default()
                    .fg(Color::Rgb(98, 114, 164))
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(header_line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }

    // Filter input
    if state.outline_filter_mode && y < area.height {
        let filter_line = Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Rgb(255, 184, 108))),
            Span::styled(
                &state.outline_filter_text,
                Style::default().fg(Color::Rgb(241, 250, 140)),
            ),
            Span::styled("\u{258e}", Style::default().fg(Color::Rgb(102, 102, 102))),
        ]);
        frame.render_widget(
            Paragraph::new(filter_line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }

    // Outline entries
    if state.outline_entries.is_empty() && y < area.height {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (no headings)",
                Style::default().fg(Color::Rgb(102, 102, 102)),
            ))),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        return;
    }

    let visible_rows = (area.height - y) as usize;
    let selected_idx = state
        .outline_entries
        .iter()
        .position(|(_, _, sel)| *sel)
        .unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    for (i, (title, level, is_selected)) in state.outline_entries.iter().enumerate() {
        if i < scroll_offset {
            continue;
        }
        if y >= area.height {
            break;
        }

        let marker = if *is_selected { "\u{25b8} " } else { "  " };
        let bg_style = if *is_selected {
            Style::default().bg(Color::Rgb(40, 42, 54))
        } else {
            Style::default()
        };

        // Color by heading level
        let level_style = match level {
            1 => bg_style
                .fg(Color::Rgb(189, 147, 249))
                .add_modifier(Modifier::BOLD),
            2 => bg_style
                .fg(Color::Rgb(139, 233, 253))
                .add_modifier(Modifier::BOLD),
            3 => bg_style.fg(Color::Rgb(80, 250, 123)),
            4 => bg_style.fg(Color::Rgb(241, 250, 140)),
            5 => bg_style.fg(Color::Rgb(255, 184, 108)),
            _ => bg_style.fg(Color::Rgb(204, 204, 204)),
        };

        let line = Line::from(vec![
            Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
            Span::styled(title.clone(), level_style),
        ]);

        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
        y += 1;
    }
}

fn draw_content(frame: &mut Frame, area: Rect, state: &ViewState, report: &mut DrawReport) {
    if is_buffer_empty(state) {
        draw_splash(frame, area, state.theme);
        report.is_splash = true;
        return;
    }
    match state.view_mode {
        ViewMode::Rendered => draw_content_rendered(frame, area, state),
        ViewMode::Raw => draw_content_raw(frame, area, state, report),
    }
}

/// Empty buffer = no rendered blocks AND no non-empty raw lines. Holds
/// regardless of which view mode the buffer is currently in (raw_lines is
/// only populated in Raw mode, rendered_blocks only in Rendered mode).
fn is_buffer_empty(state: &ViewState) -> bool {
    let rendered_empty = state.rendered_blocks.is_empty();
    let raw_empty =
        state.raw_lines.is_empty() || (state.raw_lines.len() == 1 && state.raw_lines[0].is_empty());
    match state.view_mode {
        ViewMode::Rendered => rendered_empty,
        ViewMode::Raw => raw_empty,
    }
}

fn draw_content_rendered(frame: &mut Frame, area: Rect, state: &ViewState) {
    let terminal_width = area.width as usize;
    let viewport_height = area.height as usize;
    let content_width = state.viewport.content_width(terminal_width);
    let x_offset = state.viewport.content_offset(terminal_width);

    // Draw midpoint marker in the left margin
    let mid_y = viewport_height / 2;
    if mid_y < viewport_height {
        let marker_x = if x_offset >= 2 {
            area.x + x_offset as u16 - 2
        } else {
            area.x
        };
        let marker_area = Rect::new(marker_x, area.y + mid_y as u16, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled("\u{2192}", state.theme.midpoint_marker)),
            marker_area,
        );
    }

    let mut y = 0usize;
    let view_start = state.viewport.scroll_offset;
    let view_end = state.viewport.scroll_offset + viewport_height;
    // Global match counter — the Nth match found scanning rendered text
    // top-to-bottom corresponds to search_matches[N].
    let mut rendered_match_counter = 0usize;

    for block in state.rendered_blocks.iter() {
        let h = state.viewport.block_height(block, content_width);
        let block_end = y + h;

        let is_visible = block_end > view_start && y < view_end;
        let has_search = !state.search_query.is_empty();

        // We must process all blocks (not just visible) to keep the match counter correct
        if is_visible || has_search {
            let is_code_block = matches!(block, RenderedBlock::CodeBlock { .. });
            let lines = render_block_to_lines(block, content_width, state.theme);

            let query_lower: Vec<char> = if has_search {
                state.search_query.to_lowercase().chars().collect()
            } else {
                Vec::new()
            };
            let qlen = query_lower.len();

            for (line_idx, line) in lines.iter().enumerate() {
                let render_y = y + line_idx;
                let screen_y = render_y as i32 - state.viewport.scroll_offset as i32;
                let on_screen = screen_y >= 0 && screen_y < viewport_height as i32;

                if on_screen {
                    let line_area = Rect::new(
                        area.x + x_offset as u16,
                        area.y + screen_y as u16,
                        content_width.min(area.width as usize - x_offset) as u16,
                        1,
                    );

                    if is_code_block && line_idx < lines.len() - 1 {
                        let bg = Paragraph::new("").style(state.theme.code_block_bg);
                        frame.render_widget(bg, line_area);
                    }

                    let ratatui_line = styled_line_to_ratatui(line);
                    frame.render_widget(Paragraph::new(ratatui_line), line_area);

                    // Draw rendered-mode cursor / nav object highlight
                    if state.view_mode == ViewMode::Rendered {
                        if let Some((obj_row, obj_col_start, obj_col_end)) = state.nav_highlight {
                            // Object mode: highlight full span
                            if render_y == obj_row {
                                let line_text = line.text_content();
                                let line_chars: Vec<char> = line_text.chars().collect();
                                let start = obj_col_start.min(line_chars.len());
                                let end = obj_col_end.min(line_chars.len());
                                if start < end {
                                    let highlight_text: String =
                                        line_chars[start..end].iter().collect();
                                    let highlight_x = area.x + x_offset as u16 + start as u16;
                                    let w = (end - start) as u16;
                                    if highlight_x < area.x + area.width {
                                        let clamped_w = w.min(area.x + area.width - highlight_x);
                                        let highlight_style = Style::default()
                                            .fg(Color::Rgb(40, 42, 54))
                                            .bg(Color::Rgb(248, 248, 242));
                                        let highlight_area = Rect::new(
                                            highlight_x,
                                            area.y + screen_y as u16,
                                            clamped_w,
                                            1,
                                        );
                                        frame.render_widget(
                                            Paragraph::new(Span::styled(
                                                highlight_text,
                                                highlight_style,
                                            )),
                                            highlight_area,
                                        );
                                    }
                                }
                            }
                        } else if render_y == state.rendered_cursor_row {
                            // Character mode: single char cursor
                            let line_text = line.text_content();
                            let line_chars: Vec<char> = line_text.chars().collect();
                            let col = state
                                .rendered_cursor_col
                                .min(line_chars.len().saturating_sub(1));
                            let cursor_char = line_chars.get(col).copied().unwrap_or(' ');
                            let cursor_x = area.x + x_offset as u16 + col as u16;
                            if cursor_x < area.x + area.width {
                                let mut span_col = 0;
                                let mut on_link = false;
                                for span in &line.spans {
                                    let span_len = span.text.chars().count();
                                    if col >= span_col && col < span_col + span_len {
                                        on_link = span.link.is_some();
                                        break;
                                    }
                                    span_col += span_len;
                                }
                                let cursor_style = if !state.show_block_cursor {
                                    Style::default()
                                        .fg(Color::Rgb(248, 248, 242))
                                        .bg(Color::Rgb(80, 80, 120))
                                } else if on_link {
                                    Style::default()
                                        .fg(Color::Rgb(40, 42, 54))
                                        .bg(Color::Rgb(139, 233, 253))
                                        .add_modifier(Modifier::UNDERLINED)
                                } else {
                                    Style::default()
                                        .fg(Color::Rgb(40, 42, 54))
                                        .bg(Color::Rgb(248, 248, 242))
                                };
                                let cursor_area =
                                    Rect::new(cursor_x, area.y + screen_y as u16, 1, 1);
                                frame.render_widget(
                                    Paragraph::new(Span::styled(
                                        cursor_char.to_string(),
                                        cursor_style,
                                    )),
                                    cursor_area,
                                );
                            }
                        }
                    }
                }

                // Count and highlight search matches (count even off-screen)
                if has_search && qlen > 0 {
                    let line_text = line.text_content();
                    let line_chars: Vec<char> = line_text.chars().collect();
                    let lower_chars: Vec<char> = line_text.to_lowercase().chars().collect();

                    let mut ci = 0;
                    while ci + qlen <= lower_chars.len() {
                        if lower_chars[ci..ci + qlen] == query_lower[..] {
                            if on_screen {
                                let style = if rendered_match_counter == state.search_current_match
                                {
                                    state.theme.search_match_current
                                } else {
                                    state.theme.search_match
                                };
                                let highlight_x = area.x + x_offset as u16 + ci as u16;
                                if highlight_x < area.x + area.width {
                                    let w = qlen.min((area.x + area.width - highlight_x) as usize)
                                        as u16;
                                    let match_text: String =
                                        line_chars[ci..ci + w as usize].iter().collect();
                                    let highlight_area =
                                        Rect::new(highlight_x, area.y + screen_y as u16, w, 1);
                                    frame.render_widget(
                                        Paragraph::new(Span::styled(match_text, style)),
                                        highlight_area,
                                    );
                                }
                            }
                            rendered_match_counter += 1;
                            ci += qlen;
                        } else {
                            ci += 1;
                        }
                    }
                }
            }
        }

        if y >= view_end {
            break;
        }
        y += h;
    }
}

fn draw_splash(frame: &mut Frame, area: Rect, theme: &Theme) {
    const LOGO: &[&str] = &[
        "  ___ _        _      _    ",
        " / __| |_____ | |_ __| |_  ",
        " \\__ \\ / / -_)|  _/ _| ' \\ ",
        " |___/_\\_\\___| \\__\\__|_||_|",
    ];

    let logo_h = LOGO.len();
    let block_h = (logo_h + 4) as u16; // logo + blank + version + build
    if area.height < block_h || area.width < 32 {
        return;
    }

    let logo_w = LOGO.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let top = area.y + (area.height.saturating_sub(block_h)) / 2;

    let logo_style = Style::default()
        .fg(Color::Rgb(189, 147, 249))
        .add_modifier(Modifier::BOLD);
    for (i, row) in LOGO.iter().enumerate() {
        let y = top + i as u16;
        let row_w = row.chars().count() as u16;
        let x = area.x + (area.width.saturating_sub(row_w)) / 2;
        let rect = Rect::new(x, y, row_w.min(area.width), 1);
        frame.render_widget(Paragraph::new(Span::styled(*row, logo_style)), rect);
    }

    let version_line = format!("v{}", env!("CARGO_PKG_VERSION"));
    let build_line = crate::BUILD_INFO;

    let center_x = area.x + area.width / 2;

    let v_w = version_line.chars().count() as u16;
    let v_y = top + logo_h as u16 + 1;
    let v_rect = Rect::new(
        center_x.saturating_sub(v_w / 2),
        v_y,
        v_w.min(area.width),
        1,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            version_line,
            Style::default()
                .fg(Color::Rgb(248, 248, 242))
                .add_modifier(Modifier::BOLD),
        )),
        v_rect,
    );

    let b_w = build_line.chars().count() as u16;
    let b_y = v_y + 1;
    let b_rect = Rect::new(
        center_x.saturating_sub(b_w / 2),
        b_y,
        b_w.min(area.width),
        1,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(build_line, theme.line_number)),
        b_rect,
    );

    let _ = logo_w; // reserved for future centering refinements
}

fn draw_content_raw(frame: &mut Frame, area: Rect, state: &ViewState, report: &mut DrawReport) {
    let terminal_width = area.width as usize;
    let viewport_height = area.height as usize;
    let view_start = state.viewport.scroll_offset;

    let total_lines = state.raw_lines.len();
    let line_num_digits = if total_lines == 0 {
        1
    } else {
        total_lines.ilog10() as usize + 1
    };
    let gutter_width = line_num_digits + 2; // digits + space + separator

    let text_area_width = terminal_width.saturating_sub(gutter_width + 1); // +1 for gap after gutter
    let content_width = state.viewport.content_width(text_area_width);
    let text_x = area.x + gutter_width as u16 + 1;

    let wrap_width = content_width.max(1);

    let text_style = Style::default().fg(Color::Rgb(204, 204, 204));
    let selection_bg = Color::Rgb(68, 71, 90);
    // Range styling (Claude buffer): frozen ranges = Claude's prose, render
    // with the default text colour. Everything else (the user's typed text,
    // whether in the active turn or locked into history) renders bright green.
    let editable_fg = Color::Rgb(80, 250, 123); // Dracula green — user replies
    let prefix_fg = Color::Rgb(255, 121, 198); // Dracula pink — `> ` quote marker
    const REPLY_PREFIX: &str = "> ";
    const REPLY_PREFIX_LEN: usize = 2;

    // Precompute char offset of each doc line's start, for projecting frozen
    // ranges (which are rope-char indices) onto visual rows.
    let mut line_char_starts: Vec<usize> = Vec::with_capacity(state.raw_lines.len() + 1);
    let mut acc = 0usize;
    line_char_starts.push(0);
    for line in state.raw_lines.iter() {
        acc += line.chars().count() + 1; // +1 for the implicit newline
        line_char_starts.push(acc);
    }
    let has_range_styling = !state.frozen_ranges.is_empty() || state.lockable_through_char > 0;

    let mut screen_y: usize = 0;
    let mut visual_row_counter: usize = 0;
    for (doc_line, raw_line) in state.raw_lines.iter().enumerate() {
        if screen_y >= viewport_height {
            break;
        }

        let is_cursor_line = doc_line == state.cursor_line;
        // Compute the selection's [start_col, end_col) range projected onto this line, if any.
        let line_char_count = raw_line.chars().count();
        let sel_on_line: Option<(usize, usize)> =
            state.selection.and_then(|((sl, sc), (el, ec))| {
                if doc_line < sl || doc_line > el {
                    None
                } else {
                    let s = if doc_line == sl { sc } else { 0 };
                    let e = if doc_line == el {
                        ec.min(line_char_count)
                    } else {
                        line_char_count
                    };
                    if s <= e { Some((s, e)) } else { None }
                }
            });

        let empty_segments: Vec<(String, Style)>;
        let segments: &[(String, Style)] = if !state.search_query.is_empty() {
            let line = build_highlighted_line(raw_line, doc_line, state, text_style);
            empty_segments = line
                .spans
                .iter()
                .map(|s| (s.content.to_string(), s.style.into()))
                .collect();
            &empty_segments
        } else if let Some(segs) = state.raw_highlights.get(doc_line) {
            segs.as_slice()
        } else {
            empty_segments = vec![(raw_line.clone(), text_style)];
            &empty_segments
        };

        // Treat a doc line as a "user line" iff it contains at least one
        // non-whitespace char and none of those non-whitespace chars are inside
        // a frozen (Claude) range. User lines get a `> ` prefix.
        let user_line = is_user_line(
            line_char_starts[doc_line],
            raw_line,
            &state.frozen_ranges,
            state.lockable_through_char,
        );
        let line_wrap_width = if user_line {
            wrap_width.saturating_sub(REPLY_PREFIX_LEN).max(1)
        } else {
            wrap_width
        };
        let wrapped = wrap_styled_segments(segments, line_wrap_width);
        let line_visual_rows = wrapped.rows.len();
        // Doc line entirely above the viewport — skip without rendering.
        if visual_row_counter + line_visual_rows <= view_start {
            visual_row_counter += line_visual_rows;
            continue;
        }
        let skip_rows_in_line = view_start.saturating_sub(visual_row_counter);

        let (cursor_visual_row, cursor_visual_col) = if is_cursor_line {
            locate_cursor_in_wrapped(&wrapped.row_char_starts, state.cursor_col, line_wrap_width)
        } else {
            (0, 0)
        };

        for (visual_idx, row_segs) in wrapped.rows.iter().enumerate() {
            if visual_idx < skip_rows_in_line {
                continue;
            }
            if screen_y >= viewport_height {
                break;
            }
            let y = area.y + screen_y as u16;

            // No row-wide background fill on the cursor line — the bright
            // gutter marker below carries the indication. Painting the row bg
            // was too easy to confuse with selection highlight.

            // Render the line number on the first VISIBLE visual row of this
            // doc line. When scroll splits a wrapped line, that's the first
            // row past `skip_rows_in_line`, not necessarily visual_idx 0.
            let marker_style = Style::default()
                .fg(Color::Rgb(255, 215, 0))
                .add_modifier(Modifier::BOLD);
            if visual_idx == skip_rows_in_line {
                let num_style = if is_cursor_line {
                    state.theme.line_number_current.add_modifier(Modifier::BOLD)
                } else {
                    state.theme.line_number
                };
                let num_str = format!("{:>width$} ", doc_line + 1, width = line_num_digits);
                let marker_span = if is_cursor_line {
                    Span::styled("▎", marker_style)
                } else {
                    Span::raw(" ")
                };
                let spans = vec![marker_span, Span::styled(num_str, num_style)];
                let gutter_area = Rect::new(area.x, y, gutter_width as u16, 1);
                frame.render_widget(Paragraph::new(Line::from(spans)), gutter_area);
            } else if is_cursor_line {
                // Continuation row of a wrapped cursor line: still paint the
                // leftmost cell so the indicator runs the full visual height.
                let gutter_area = Rect::new(area.x, y, 1, 1);
                frame.render_widget(Paragraph::new(Span::styled("▎", marker_style)), gutter_area);
            }

            let line_area = Rect::new(
                text_x,
                y,
                wrap_width.min(area.width as usize - gutter_width - 1) as u16,
                1,
            );

            // Apply selection background if this row overlaps the selection.
            let row_start_col = wrapped.row_char_starts[visual_idx];
            let row_end_col = wrapped
                .row_char_starts
                .get(visual_idx + 1)
                .copied()
                .unwrap_or(row_start_col);

            // Step 1: range styling. Chars inside a frozen range render with
            // their default style (Claude's prose); everything else gets the
            // user-edit foreground (green) — applied uniformly across the
            // active turn AND prior-turn user replies in the locked prefix.
            let mut styled_row_segs = if has_range_styling {
                apply_range_styles(
                    row_segs,
                    line_char_starts[doc_line] + row_start_col,
                    &state.frozen_ranges,
                    editable_fg,
                )
            } else {
                row_segs.clone()
            };

            // Step 2: selection bg overlays everything.
            if let Some((sel_s, sel_e)) = sel_on_line {
                let local_s = sel_s.saturating_sub(row_start_col);
                let local_e = sel_e
                    .saturating_sub(row_start_col)
                    .min(row_end_col - row_start_col);
                if local_s < local_e {
                    styled_row_segs =
                        apply_selection_bg(&styled_row_segs, local_s, local_e, selection_bg);
                }
            }

            // Build the final span list, prepending `> ` on user lines.
            let mut spans: Vec<Span> = Vec::with_capacity(styled_row_segs.len() + 1);
            if user_line {
                spans.push(Span::styled(
                    REPLY_PREFIX,
                    Style::default().fg(prefix_fg).add_modifier(Modifier::BOLD),
                ));
            }
            for (t, s) in styled_row_segs.iter() {
                spans.push(Span::styled(t.clone(), *s));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), line_area);

            let row_indent = if user_line {
                REPLY_PREFIX_LEN as u16
            } else {
                0
            };
            if is_cursor_line && visual_idx == cursor_visual_row {
                let cursor_x = text_x + row_indent + cursor_visual_col as u16;
                if cursor_x < area.x + area.width {
                    let cursor_char = row_text_char_at(row_segs, cursor_visual_col).unwrap_or(' ');
                    let cursor_style = if state.show_block_cursor {
                        Style::default()
                            .fg(Color::Rgb(40, 42, 54))
                            .bg(Color::Rgb(248, 248, 242))
                    } else {
                        Style::default()
                            .fg(Color::Rgb(248, 248, 242))
                            .bg(Color::Rgb(80, 80, 120))
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled(cursor_char.to_string(), cursor_style)),
                        Rect::new(cursor_x, y, 1, 1),
                    );
                    // Ground truth: this is where the cursor actually was painted.
                    report.cursor_screen_y = Some(y);
                }
            }

            // Track first/last visible doc lines for the debug overlay.
            if report.first_visible_doc_line.is_none() {
                report.first_visible_doc_line = Some(doc_line);
            }
            report.last_visible_doc_line = Some(doc_line);
            report.painted_rows += 1;

            screen_y += 1;
        }
        visual_row_counter += line_visual_rows;
    }
}

/// Restyle a row's segments per the *claude* buffer's range model:
///   - chars IN a frozen range  → unchanged (Claude prose, default colour)
///   - chars OUTSIDE all frozen → bright fg (user reply, active or locked)
fn apply_range_styles(
    segs: &[(String, Style)],
    row_first_char: usize,
    frozen: &[(usize, usize)],
    editable_fg: Color,
) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::with_capacity(segs.len());
    let mut idx = row_first_char;
    for (text, style) in segs {
        let mut current = String::new();
        let mut current_style = *style;
        let mut started = false;
        for ch in text.chars() {
            let new_style = if frozen.iter().any(|&(s, e)| idx >= s && idx < e) {
                *style
            } else {
                style.fg(editable_fg)
            };
            if started && new_style != current_style {
                out.push((std::mem::take(&mut current), current_style));
                current_style = new_style;
            } else if !started {
                current_style = new_style;
                started = true;
            }
            current.push(ch);
            idx += 1;
        }
        if !current.is_empty() {
            out.push((current, current_style));
        }
    }
    out
}

/// A doc line counts as a "user reply line" iff it has at least one
/// non-whitespace char, none of those non-whitespace chars are inside a frozen
/// range, AND it's not the turn-delimiter HR (`---`).
fn is_user_line(
    line_first_char: usize,
    line_text: &str,
    frozen: &[(usize, usize)],
    lockable_through_char: usize,
) -> bool {
    if line_text.trim_end_matches('\n').trim() == "---" {
        return false;
    }
    let mut idx = line_first_char;
    let mut had_non_ws = false;
    for ch in line_text.chars() {
        if !ch.is_whitespace() {
            had_non_ws = true;
            if frozen.iter().any(|&(s, e)| idx >= s && idx < e) {
                return false;
            }
        }
        idx += 1;
    }
    if had_non_ws {
        return true;
    }
    // Empty/whitespace-only line: treat as a user line if it sits in the
    // editable draft region (past the locked prefix and not inside any frozen
    // Claude range). This makes `> ` appear the instant you press Enter on a
    // fresh draft line, before you've typed anything.
    let line_end = idx;
    if line_first_char < lockable_through_char {
        return false;
    }
    !frozen
        .iter()
        .any(|&(s, e)| line_first_char < e && s < line_end.max(line_first_char + 1))
}

/// Apply a background color to chars in `[start_col, end_col)` of a row's segments.
/// Returns a new segment list where the styled range has the bg overridden.
fn apply_selection_bg(
    segs: &[(String, Style)],
    start_col: usize,
    end_col: usize,
    bg: Color,
) -> Vec<(String, Style)> {
    let mut result: Vec<(String, Style)> = Vec::new();
    let mut col = 0usize;
    for (text, style) in segs {
        let mut current_text = String::new();
        let mut current_style = *style;
        let mut started = false;
        for ch in text.chars() {
            let is_selected = col >= start_col && col < end_col;
            let new_style = if is_selected { style.bg(bg) } else { *style };
            if started && new_style != current_style {
                result.push((std::mem::take(&mut current_text), current_style));
                current_style = new_style;
            } else if !started {
                current_style = new_style;
                started = true;
            }
            current_text.push(ch);
            col += 1;
        }
        if !current_text.is_empty() {
            result.push((current_text, current_style));
        }
    }
    result
}

struct WrappedLine {
    rows: Vec<Vec<(String, Style)>>,
    /// For each row index, the character offset into the original line where it starts.
    /// Has `rows.len() + 1` entries; last entry is total char length of the line.
    row_char_starts: Vec<usize>,
}

/// Count how many visual rows `text` will occupy when wrapped at `width`
/// using the SAME word-boundary rule as `wrap_styled_segments`. This MUST
/// stay byte-for-byte equivalent to the renderer's wrap, otherwise scroll
/// math (which calls this) and the renderer disagree on cumulative row
/// counts and the cursor ends up off-screen near the bottom of the viewport.
pub fn wrap_row_count(text: &str, width: usize) -> usize {
    if text.is_empty() {
        return 1;
    }
    if width == 0 {
        return 1;
    }
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let mut rows = 0usize;
    let mut pos = 0usize;
    while pos < total {
        let hard_end = (pos + width).min(total);
        let break_at = if hard_end < total {
            (pos..hard_end)
                .rev()
                .find(|&i| chars[i] == ' ')
                .map(|i| i + 1)
                .unwrap_or(hard_end)
        } else {
            hard_end
        };
        let end = break_at.max(pos + 1);
        rows += 1;
        pos = end;
    }
    rows.max(1)
}

/// Like `wrap_row_count` but ALSO returns the visual row offset within the
/// wrapped line at character column `target_col`. Used by scroll math to
/// place the cursor on the right visual row when the line wraps.
pub fn wrap_row_count_with_cursor(text: &str, width: usize, target_col: usize) -> (usize, usize) {
    if text.is_empty() || width == 0 {
        return (1, 0);
    }
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let mut rows = 0usize;
    let mut cursor_row = 0usize;
    let mut found = false;
    let mut pos = 0usize;
    while pos < total {
        let hard_end = (pos + width).min(total);
        let break_at = if hard_end < total {
            (pos..hard_end)
                .rev()
                .find(|&i| chars[i] == ' ')
                .map(|i| i + 1)
                .unwrap_or(hard_end)
        } else {
            hard_end
        };
        let end = break_at.max(pos + 1);
        if !found && target_col >= pos && target_col < end {
            cursor_row = rows;
            found = true;
        }
        rows += 1;
        pos = end;
    }
    if !found {
        // target_col is at or past end-of-text — stick to the last row.
        cursor_row = rows.saturating_sub(1);
    }
    (rows.max(1), cursor_row)
}

fn wrap_styled_segments(segments: &[(String, Style)], width: usize) -> WrappedLine {
    let mut flat: Vec<(char, Style)> = Vec::new();
    for (text, style) in segments {
        for ch in text.chars() {
            flat.push((ch, *style));
        }
    }

    if flat.is_empty() {
        return WrappedLine {
            rows: vec![Vec::new()],
            row_char_starts: vec![0, 0],
        };
    }

    let total = flat.len();
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let mut row_starts: Vec<usize> = Vec::new();
    let mut pos = 0;

    while pos < total {
        row_starts.push(pos);
        let hard_end = (pos + width).min(total);
        let break_at = if hard_end < total {
            // Prefer a word boundary: split AFTER the last space in [pos, hard_end].
            (pos..hard_end)
                .rev()
                .find(|&i| flat[i].0 == ' ')
                .map(|i| i + 1)
                .unwrap_or(hard_end)
        } else {
            hard_end
        };
        let end = break_at.max(pos + 1);

        let mut row: Vec<(String, Style)> = Vec::new();
        let mut cur_text = String::new();
        let mut cur_style = flat[pos].1;
        for (ch, st) in flat[pos..end].iter().copied() {
            if st != cur_style {
                if !cur_text.is_empty() {
                    row.push((std::mem::take(&mut cur_text), cur_style));
                }
                cur_style = st;
            }
            cur_text.push(ch);
        }
        if !cur_text.is_empty() {
            row.push((cur_text, cur_style));
        }
        rows.push(row);
        pos = end;
    }

    row_starts.push(total);
    WrappedLine {
        rows,
        row_char_starts: row_starts,
    }
}

fn locate_cursor_in_wrapped(
    row_starts: &[usize],
    cursor_col: usize,
    wrap_width: usize,
) -> (usize, usize) {
    if row_starts.len() < 2 {
        return (0, cursor_col);
    }
    // Find the row such that row_starts[i] <= cursor_col < row_starts[i+1].
    let mut row = 0;
    for i in 0..row_starts.len() - 1 {
        if cursor_col >= row_starts[i] && cursor_col < row_starts[i + 1] {
            row = i;
            break;
        }
        if i + 1 == row_starts.len() - 1 && cursor_col >= row_starts[i + 1] {
            row = i; // cursor past end → stick to last row
        }
    }
    let col_in_row = cursor_col.saturating_sub(row_starts[row]);
    (row, col_in_row.min(wrap_width))
}

fn row_text_char_at(row: &[(String, Style)], col: usize) -> Option<char> {
    let mut remaining = col;
    for (t, _) in row {
        let n = t.chars().count();
        if remaining < n {
            return t.chars().nth(remaining);
        }
        remaining -= n;
    }
    None
}

/// Build a line with search match highlighting.
fn build_highlighted_line<'a>(
    raw_line: &str,
    doc_line: usize,
    state: &ViewState<'a>,
    base_style: Style,
) -> Line<'static> {
    let query_len = state.search_query.len();
    if query_len == 0 {
        return Line::from(Span::styled(raw_line.to_string(), base_style));
    }

    // Collect match columns on this line
    let mut match_cols: Vec<(usize, bool)> = Vec::new(); // (col, is_current)
    for (i, &(line, col)) in state.search_matches.iter().enumerate() {
        if line == doc_line {
            match_cols.push((col, i == state.search_current_match));
        }
    }

    if match_cols.is_empty() {
        return Line::from(Span::styled(raw_line.to_string(), base_style));
    }

    // `col` and `query_len` are CHARACTER indices; `raw_line[..]` slicing is
    // byte-indexed. Convert via char_indices to a safe byte boundary and
    // clamp to the line's char count — search matches can be stale relative
    // to the line's current content (e.g. after edits) so we never index out
    // of bounds.
    let line_char_count = raw_line.chars().count();
    let char_to_byte = |c: usize| -> usize {
        if c >= line_char_count {
            return raw_line.len();
        }
        raw_line
            .char_indices()
            .nth(c)
            .map(|(b, _)| b)
            .unwrap_or(raw_line.len())
    };

    let mut spans = Vec::new();
    let mut pos_b = 0usize;
    for (col, is_current) in &match_cols {
        let col_clamped = (*col).min(line_char_count);
        let end_clamped = (col_clamped + query_len).min(line_char_count);
        let col_b = char_to_byte(col_clamped);
        let end_b = char_to_byte(end_clamped);
        if col_b > pos_b {
            spans.push(Span::styled(raw_line[pos_b..col_b].to_string(), base_style));
        }
        if end_b > col_b {
            let match_style = if *is_current {
                state.theme.search_match_current
            } else {
                state.theme.search_match
            };
            spans.push(Span::styled(
                raw_line[col_b..end_b].to_string(),
                match_style,
            ));
        }
        pos_b = end_b;
    }
    if pos_b < raw_line.len() {
        spans.push(Span::styled(raw_line[pos_b..].to_string(), base_style));
    }

    Line::from(spans)
}

/// Convert a RenderedBlock to terminal lines for display.
pub fn render_block_to_lines(
    block: &RenderedBlock,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    match block {
        RenderedBlock::Heading { level, content } => {
            let prefix = "#".repeat(*level as usize);
            let style = theme.heading[(*level as usize).saturating_sub(1).min(5)];
            let mut spans = vec![StyledSpan::new(format!("{} ", prefix), style)];
            spans.extend(content.spans.iter().cloned());
            let prefixed = StyledLine::new(spans);
            let mut lines = vec![prefixed.clone()];
            if *level == 1 {
                let rule = "\u{2501}".repeat(prefixed.text_content().len().min(width));
                lines.push(StyledLine::new(vec![StyledSpan::new(
                    rule,
                    theme.horizontal_rule,
                )]));
            }
            lines.push(StyledLine::new(vec![])); // blank line
            lines
        }
        RenderedBlock::Paragraph { lines } => {
            let mut out = Vec::new();
            for line in lines {
                wrap_styled_line(line, width, &mut out);
            }
            out.push(StyledLine::new(vec![])); // blank line
            out
        }
        RenderedBlock::CodeBlock { lines, .. } => {
            let mut out = Vec::new();
            for line in lines {
                // Truncate with arrow indicator if too wide, preserving per-span styles
                let text = line.text_content();
                if text.chars().count() > width {
                    let mut truncated_spans = Vec::new();
                    let mut remaining = width - 1;
                    for span in &line.spans {
                        if remaining == 0 {
                            break;
                        }
                        let span_chars = span.text.chars().count();
                        if span_chars <= remaining {
                            truncated_spans.push(span.clone());
                            remaining -= span_chars;
                        } else {
                            let byte_end = span
                                .text
                                .char_indices()
                                .nth(remaining)
                                .map(|(i, _)| i)
                                .unwrap_or(span.text.len());
                            truncated_spans
                                .push(StyledSpan::new(&span.text[..byte_end], span.style));
                            remaining = 0;
                        }
                    }
                    truncated_spans.push(StyledSpan::new("\u{2192}", theme.code_block_bg));
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
                    let mut spans = vec![StyledSpan::new("\u{258e} ", theme.blockquote_bar)];
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
            let col_widths = table_column_widths(headers, rows, width);
            let mut out = Vec::new();

            // Header (wrapped)
            let header_texts: Vec<String> = headers.iter().map(|h| h.text_content()).collect();
            let header_wrapped: Vec<Vec<String>> = header_texts
                .iter()
                .enumerate()
                .map(|(i, t)| wrap_text(t, col_widths[i]))
                .collect();
            let header_height = header_wrapped.iter().map(|w| w.len()).max().unwrap_or(1);
            for line_idx in 0..header_height {
                let mut spans = Vec::new();
                for (i, wrapped) in header_wrapped.iter().enumerate() {
                    if i > 0 {
                        spans.push(StyledSpan::new(" \u{2502} ", theme.table_border));
                    }
                    let text = wrapped.get(line_idx).cloned().unwrap_or_default();
                    let padded = format!("{:<width$}", text, width = col_widths[i]);
                    spans.push(StyledSpan::new(padded, theme.table_header));
                }
                out.push(StyledLine::new(spans));
            }

            // Separator
            let sep: String = col_widths
                .iter()
                .map(|w| "\u{2500}".repeat(*w))
                .collect::<Vec<_>>()
                .join("\u{2500}\u{253c}\u{2500}");
            out.push(StyledLine::new(vec![StyledSpan::new(
                sep,
                theme.table_border,
            )]));

            // Rows (wrapped)
            for row in rows {
                let cell_texts: Vec<String> = row.iter().map(|c| c.text_content()).collect();
                let cell_wrapped: Vec<Vec<String>> = cell_texts
                    .iter()
                    .enumerate()
                    .map(|(i, t)| wrap_text(t, *col_widths.get(i).unwrap_or(&5)))
                    .collect();
                let row_height = cell_wrapped.iter().map(|w| w.len()).max().unwrap_or(1);
                for line_idx in 0..row_height {
                    let mut spans = Vec::new();
                    for (i, wrapped) in cell_wrapped.iter().enumerate() {
                        if i > 0 {
                            spans.push(StyledSpan::new(" \u{2502} ", theme.table_border));
                        }
                        let text = wrapped.get(line_idx).cloned().unwrap_or_default();
                        let cw = *col_widths.get(i).unwrap_or(&5);
                        let padded = format!("{:<width$}", text, width = cw);
                        spans.push(StyledSpan::new(padded, theme.paragraph));
                    }
                    out.push(StyledLine::new(spans));
                }
            }
            out.push(StyledLine::new(vec![])); // blank line
            out
        }
        RenderedBlock::HorizontalRule => {
            let rule = "\u{2500}".repeat(width);
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

/// Word-wrap a StyledLine into multiple lines that fit within `width` chars.
/// Tries to break at word boundaries (spaces); falls back to hard break if a
/// single word exceeds the width.
fn wrap_styled_line(line: &StyledLine, width: usize, out: &mut Vec<StyledLine>) {
    if width == 0 {
        out.push(line.clone());
        return;
    }

    let total_len: usize = line.spans.iter().map(|s| s.text.len()).sum();
    if total_len <= width {
        out.push(line.clone());
        return;
    }

    // Flatten all spans into a list of (char, style, link) for wrapping
    let mut chars: Vec<(char, Style, Option<String>)> = Vec::new();
    for span in &line.spans {
        for ch in span.text.chars() {
            chars.push((ch, span.style, span.link.clone()));
        }
    }

    let mut pos = 0;
    while pos < chars.len() {
        let remaining = chars.len() - pos;
        let line_len = remaining.min(width);
        let end = pos + line_len;

        // Try to find a word boundary (space) to break at
        let break_at = if end < chars.len() {
            // Look backwards from end for a space
            let mut best = end;
            for i in (pos..end).rev() {
                if chars[i].0 == ' ' {
                    best = i + 1; // break after the space
                    break;
                }
            }
            if best == end && end < chars.len() {
                // No space found — hard break at width
                end
            } else {
                best
            }
        } else {
            end
        };

        // Build a StyledLine from chars[pos..break_at]
        let mut spans: Vec<StyledSpan> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = if pos < chars.len() {
            chars[pos].1
        } else {
            Style::default()
        };
        let mut current_link = if pos < chars.len() {
            chars[pos].2.clone()
        } else {
            None
        };

        for &(ch, style, ref link) in &chars[pos..break_at] {
            if style != current_style || *link != current_link {
                if !current_text.is_empty() {
                    spans.push(StyledSpan {
                        text: current_text.clone(),
                        style: current_style,
                        link: current_link.clone(),
                    });
                    current_text.clear();
                }
                current_style = style;
                current_link = link.clone();
            }
            current_text.push(ch);
        }
        if !current_text.is_empty() {
            spans.push(StyledSpan {
                text: current_text,
                style: current_style,
                link: current_link,
            });
        }

        out.push(StyledLine::new(spans));
        pos = break_at;
    }
}

/// Calculate column widths for a table, fitting within the available width.
/// Each column gets at least 3 chars. If the natural widths exceed the available
/// space, columns are shrunk proportionally (widest columns shrink first).
pub fn table_column_widths(
    headers: &[StyledLine],
    rows: &[Vec<StyledLine>],
    available_width: usize,
) -> Vec<usize> {
    let ncols = headers.len();
    if ncols == 0 {
        return Vec::new();
    }

    // Natural widths (unconstrained)
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|h| h.text_content().len().max(3))
        .collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.text_content().len());
            }
        }
    }

    // Borders take 3 chars each (" │ ")
    let border_space = if ncols > 1 { (ncols - 1) * 3 } else { 0 };
    let usable = available_width.saturating_sub(border_space);

    let total: usize = widths.iter().sum();
    if total <= usable {
        return widths;
    }

    let min_per_col = 3usize;
    let min_total = ncols * min_per_col;
    if usable <= min_total {
        return vec![min_per_col; ncols];
    }

    // Cap-and-redistribute: columns that fit within an equal share keep their
    // natural width; remaining space goes proportionally to wider columns.
    let mut result = vec![0usize; ncols];
    let mut settled = vec![false; ncols];
    let mut remaining_space = usable;
    let mut unsettled_count = ncols;

    loop {
        let fair_share = remaining_space / unsettled_count.max(1);
        let mut changed = false;
        for i in 0..ncols {
            if settled[i] {
                continue;
            }
            if widths[i] <= fair_share {
                result[i] = widths[i];
                settled[i] = true;
                remaining_space -= widths[i];
                unsettled_count -= 1;
                changed = true;
            }
        }
        if !changed || unsettled_count == 0 {
            break;
        }
    }

    if unsettled_count > 0 {
        // Distribute remaining space proportionally among unsettled columns
        let unsettled_natural: usize = (0..ncols).filter(|&i| !settled[i]).map(|i| widths[i]).sum();
        for i in 0..ncols {
            if settled[i] {
                continue;
            }
            let allocated = if unsettled_natural > 0 {
                (widths[i] as f64 / unsettled_natural as f64 * remaining_space as f64).floor()
                    as usize
            } else {
                remaining_space / unsettled_count
            };
            result[i] = allocated.max(min_per_col);
        }

        // Fix rounding: distribute leftover or trim excess
        let mut result_total: usize = result.iter().sum();
        while result_total < usable {
            if let Some(idx) = result
                .iter()
                .enumerate()
                .filter(|&(i, _)| !settled[i])
                .min_by_key(|&(i, &w)| (w as isize - widths[i] as isize).abs())
                .map(|(i, _)| i)
            {
                result[idx] += 1;
                result_total += 1;
            } else {
                break;
            }
        }
        while result_total > usable {
            if let Some(idx) = result
                .iter()
                .enumerate()
                .filter(|&(_, &w)| w > min_per_col)
                .max_by_key(|&(_, &w)| w)
                .map(|(i, _)| i)
            {
                result[idx] -= 1;
                result_total -= 1;
            } else {
                break;
            }
        }
    }

    result
}

/// Wrap text to fit within a given width (in chars), breaking at word boundaries when possible.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    if text.chars().count() <= width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.chars().count() <= width {
            lines.push(remaining.to_string());
            break;
        }

        // Byte index of the (width)th char — safe split point.
        let hard_byte = remaining
            .char_indices()
            .nth(width)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());

        // Look for the last space at or before the hard split point.
        let break_at = remaining[..hard_byte].rfind(' ').unwrap_or(hard_byte);

        let (chunk, rest) = remaining.split_at(break_at);
        lines.push(chunk.to_string());
        remaining = rest.trim_start();
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Compute scroll offset so that `selected` is visible within `visible_rows`.
fn scroll_to_keep_visible(selected: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    if selected >= visible_rows {
        selected - visible_rows + 1
    } else {
        0
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

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_mtime(time: std::time::SystemTime) -> String {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let (_year, month, day) = days_to_ymd(days);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = months.get(month as usize).unwrap_or(&"???");
    format!("{} {:2}", mon, day)
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m - 1, d) // month 0-indexed for array lookup
}

fn draw_full_file_browser(
    frame: &mut Frame,
    area: Rect,
    fb: &FullBrowserViewState,
    _theme: &Theme,
) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    // Layout: header(1) + entries(fill) + filter(0 or 1) + hints(1)
    let filter_height = if fb.filter_mode { 1u16 } else { 0 };
    let chunks = Layout::vertical([
        Constraint::Length(1),             // header
        Constraint::Min(1),                // entry list
        Constraint::Length(filter_height), // filter input
        Constraint::Length(1),             // hint bar
    ])
    .split(area);

    let header_area = chunks[0];
    let list_area = chunks[1];
    let filter_area = chunks[2];
    let hint_area = chunks[3];

    // --- Header: breadcrumb path ---
    let max_dir_width = header_area.width as usize - 3;
    let dir_display = if fb.dir.len() > max_dir_width {
        let start = fb.dir.len() - max_dir_width;
        format!(" \u{25b8} \u{2026}{}", &fb.dir[start..])
    } else {
        format!(" \u{25b8} {}", fb.dir)
    };
    let header_line = Line::from(Span::styled(
        dir_display,
        Style::default()
            .fg(Color::Rgb(139, 233, 253))
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(Color::Rgb(40, 42, 54))),
        header_area,
    );

    // --- Entry list ---
    let visible_rows = list_area.height as usize;
    let selected_idx = fb.entries.iter().position(|e| e.is_selected).unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    // Column widths: 2 marker + name(fill) + 2 pad + 6 size + 2 pad + 7 mtime
    let size_col_width: u16 = 6;
    let mtime_col_width: u16 = 7;
    let padding: u16 = 2;
    let metadata_width = padding + size_col_width + padding + mtime_col_width;

    if fb.entries.is_empty() {
        let empty_line = Line::from(Span::styled(
            "  (empty)",
            Style::default().fg(Color::Rgb(102, 102, 102)),
        ));
        frame.render_widget(
            Paragraph::new(empty_line),
            Rect::new(list_area.x, list_area.y, list_area.width, 1),
        );
    } else {
        let mut y = 0u16;
        for (i, entry) in fb.entries.iter().enumerate() {
            if i < scroll_offset {
                continue;
            }
            if y >= list_area.height {
                break;
            }

            let row_area = Rect::new(list_area.x, list_area.y + y, list_area.width, 1);

            let marker = if entry.is_selected { "\u{25b8} " } else { "  " };
            let bg_style = if entry.is_selected {
                Style::default().bg(Color::Rgb(50, 52, 68))
            } else {
                Style::default().bg(Color::Rgb(30, 30, 48))
            };

            // Fill row background
            let bg_fill = Paragraph::new("").style(bg_style);
            frame.render_widget(bg_fill, row_area);

            let name_style = if entry.is_dir {
                bg_style.fg(Color::Rgb(139, 233, 253))
            } else {
                bg_style.fg(Color::Rgb(204, 204, 204))
            };
            let suffix = if entry.is_dir { "/" } else { "" };

            let name_max = (list_area.width - metadata_width - 2) as usize; // 2 for marker
            let name_text = format!("{}{}", entry.name, suffix);
            let name_display = if name_text.len() > name_max {
                format!("\u{2026}{}", &name_text[name_text.len() - name_max + 1..])
            } else {
                name_text.clone()
            };

            let size_str = match entry.size {
                Some(s) => format_file_size(s),
                None => "\u{2014}".to_string(),
            };
            let mtime_str = match entry.modified {
                Some(t) => format_mtime(t),
                None => "\u{2014}".to_string(),
            };

            let name_padding = name_max.saturating_sub(name_display.len());

            let spans = vec![
                Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
                Span::styled(name_display, name_style),
                Span::styled(" ".repeat(name_padding), bg_style),
                Span::styled("  ", bg_style),
                Span::styled(
                    format!("{:>width$}", size_str, width = size_col_width as usize),
                    bg_style.fg(Color::Rgb(98, 114, 164)),
                ),
                Span::styled("  ", bg_style),
                Span::styled(
                    format!("{:>width$}", mtime_str, width = mtime_col_width as usize),
                    bg_style.fg(Color::Rgb(98, 114, 164)),
                ),
            ];

            frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
            y += 1;
        }
    }

    // --- Filter input ---
    if fb.filter_mode {
        let filter_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default()
                    .fg(Color::Rgb(255, 184, 108))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
            Span::styled(
                &fb.filter_text,
                Style::default()
                    .fg(Color::Rgb(241, 250, 140))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
            Span::styled(
                "\u{258e}",
                Style::default()
                    .fg(Color::Rgb(102, 102, 102))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
        ]);
        frame.render_widget(Paragraph::new(filter_line), filter_area);
    }

    // --- Hint bar ---
    let mut hints = format!(
        " enter:open  o:open+stay  -:parent  .:hidden  s:sort({})  /:filter",
        fb.sort_label
    );
    if fb.came_from_dropdown {
        hints.push_str("  tab:collapse");
    }
    hints.push_str("  q:close");
    let hint_line = Line::from(Span::styled(
        hints,
        Style::default()
            .fg(Color::Rgb(98, 114, 164))
            .bg(Color::Rgb(25, 25, 40)),
    ));
    frame.render_widget(
        Paragraph::new(hint_line).style(Style::default().bg(Color::Rgb(25, 25, 40))),
        hint_area,
    );
}

fn draw_full_buffer_list(
    frame: &mut Frame,
    area: Rect,
    bl: &FullBufferListViewState,
    _theme: &Theme,
) {
    // Background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 48)));
    frame.render_widget(bg, area);

    let filter_height = if bl.filter_mode { 1u16 } else { 0 };
    let chunks = Layout::vertical([
        Constraint::Length(1),             // header
        Constraint::Min(1),                // entry list
        Constraint::Length(filter_height), // filter input
        Constraint::Length(1),             // hint bar
    ])
    .split(area);

    let header_area = chunks[0];
    let list_area = chunks[1];
    let filter_area = chunks[2];
    let hint_area = chunks[3];

    // --- Header ---
    let visible_count = bl.entries.len();
    let header_text = if bl.filter_text.is_empty() {
        format!(" \u{25b8} buffers ({})", bl.total_count)
    } else {
        format!(
            " \u{25b8} buffers ({}/{}) — \"{}\"",
            visible_count, bl.total_count, bl.filter_text
        )
    };
    let header_line = Line::from(Span::styled(
        header_text,
        Style::default()
            .fg(Color::Rgb(139, 233, 253))
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(header_line).style(Style::default().bg(Color::Rgb(40, 42, 54))),
        header_area,
    );

    // --- Entry list ---
    let visible_rows = list_area.height as usize;
    let selected_idx = bl.entries.iter().position(|e| e.is_selected).unwrap_or(0);
    let scroll_offset = scroll_to_keep_visible(selected_idx, visible_rows);

    if bl.entries.is_empty() {
        let empty_line = Line::from(Span::styled(
            "  (no buffers match)",
            Style::default().fg(Color::Rgb(102, 102, 102)),
        ));
        frame.render_widget(
            Paragraph::new(empty_line),
            Rect::new(list_area.x, list_area.y, list_area.width, 1),
        );
    } else {
        let mut y = 0u16;
        for (i, entry) in bl.entries.iter().enumerate() {
            if i < scroll_offset {
                continue;
            }
            if y >= list_area.height {
                break;
            }

            let row_area = Rect::new(list_area.x, list_area.y + y, list_area.width, 1);

            let marker = if entry.is_selected { "\u{25b8} " } else { "  " };
            let bg_style = if entry.is_selected {
                Style::default().bg(Color::Rgb(50, 52, 68))
            } else {
                Style::default().bg(Color::Rgb(30, 30, 48))
            };

            // Fill row background
            let bg_fill = Paragraph::new("").style(bg_style);
            frame.render_widget(bg_fill, row_area);

            let active_indicator = if entry.is_active { "\u{25cf} " } else { "  " };
            let active_style = if entry.is_active {
                bg_style.fg(Color::Rgb(80, 250, 123))
            } else {
                bg_style
            };

            let path_style = if entry.is_active {
                bg_style
                    .fg(Color::Rgb(139, 233, 253))
                    .add_modifier(Modifier::BOLD)
            } else {
                bg_style.fg(Color::Rgb(204, 204, 204))
            };

            let modified_indicator = if entry.is_modified { " [+]" } else { "" };
            let modified_style = bg_style.fg(Color::Rgb(255, 184, 108));

            let line = Line::from(vec![
                Span::styled(marker, bg_style.fg(Color::Rgb(189, 147, 249))),
                Span::styled(active_indicator, active_style),
                Span::styled(entry.path.clone(), path_style),
                Span::styled(modified_indicator.to_string(), modified_style),
            ]);

            frame.render_widget(Paragraph::new(line), row_area);
            y += 1;
        }
    }

    // --- Filter input ---
    if bl.filter_mode {
        let filter_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default()
                    .fg(Color::Rgb(255, 184, 108))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
            Span::styled(
                &bl.filter_text,
                Style::default()
                    .fg(Color::Rgb(241, 250, 140))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
            Span::styled(
                "\u{258e}",
                Style::default()
                    .fg(Color::Rgb(102, 102, 102))
                    .bg(Color::Rgb(30, 30, 48)),
            ),
        ]);
        frame.render_widget(Paragraph::new(filter_line), filter_area);
    }

    // --- Hint bar ---
    let hints = " enter/l:switch  d:close  /:filter  g/G:top/bottom  q:close ";
    let hint_line = Line::from(Span::styled(
        hints,
        Style::default()
            .fg(Color::Rgb(98, 114, 164))
            .bg(Color::Rgb(25, 25, 40)),
    ));
    frame.render_widget(
        Paragraph::new(hint_line).style(Style::default().bg(Color::Rgb(25, 25, 40))),
        hint_area,
    );
}
