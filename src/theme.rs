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
                Style::default().fg(Color::Rgb(189, 147, 249)).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Rgb(139, 233, 253)).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Rgb(80, 250, 123)).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Rgb(241, 250, 140)).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Rgb(255, 184, 108)).add_modifier(Modifier::BOLD),
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

    pub fn compose(base: Style, modifier: Style) -> Style {
        base.patch(modifier)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
