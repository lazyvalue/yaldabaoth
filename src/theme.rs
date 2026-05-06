use crate::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    #[default]
    Dracula,
    Nightfox,
}

impl ThemeName {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "dracula" | "dark" => Some(Self::Dracula),
            "nightfox" => Some(Self::Nightfox),
            _ => None,
        }
    }

    pub fn syntect_theme(&self) -> &'static str {
        match self {
            Self::Dracula => "base16-ocean.dark",
            Self::Nightfox => "base16-ocean.dark",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: ThemeName,
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
    pub top_bar_mode: Style,
    pub bottom_bar: Style,
    pub mode_indicator: Style,
    pub search_match: Style,
    pub search_match_current: Style,
    pub midpoint_marker: Style,
    pub line_number: Style,
    pub line_number_current: Style,
}

impl Theme {
    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dracula => Self::dracula(),
            ThemeName::Nightfox => Self::nightfox(),
        }
    }

    pub fn dracula() -> Self {
        Self {
            name: ThemeName::Dracula,
            heading: [
                Style::default()
                    .fg(Color::Rgb(189, 147, 249))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(139, 233, 253))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(80, 250, 123))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(241, 250, 140))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(255, 184, 108))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(180, 180, 180))
                    .add_modifier(Modifier::BOLD),
            ],
            paragraph: Style::default().fg(Color::Rgb(204, 204, 204)),
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(248, 248, 242)),
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(248, 248, 242)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(136, 136, 136)),
            code_inline: Style::default()
                .fg(Color::Rgb(241, 250, 140))
                .bg(Color::Rgb(40, 42, 54)),
            code_block_bg: Style::default().bg(Color::Rgb(40, 42, 54)),
            blockquote_bar: Style::default().fg(Color::Rgb(255, 184, 108)),
            blockquote_text: Style::default().fg(Color::Rgb(170, 170, 170)),
            link: Style::default()
                .fg(Color::Rgb(139, 233, 253))
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(98, 114, 164)),
            table_header: Style::default()
                .fg(Color::Rgb(189, 147, 249))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(98, 114, 164)),
            list_marker: Style::default().fg(Color::Rgb(80, 250, 123)),
            image_label: Style::default()
                .fg(Color::Rgb(255, 184, 108))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(40, 42, 54)),
            top_bar: Style::default()
                .fg(Color::Rgb(139, 233, 253))
                .bg(Color::Rgb(22, 33, 62)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(80, 250, 123))
                .bg(Color::Rgb(22, 33, 62))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(102, 102, 102))
                .bg(Color::Rgb(22, 33, 62)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(80, 250, 123))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(40, 42, 54))
                .bg(Color::Rgb(98, 114, 164)),
            search_match_current: Style::default()
                .fg(Color::Rgb(40, 42, 54))
                .bg(Color::Rgb(80, 250, 123))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(98, 114, 164)),
            line_number: Style::default().fg(Color::Rgb(98, 114, 164)),
            line_number_current: Style::default().fg(Color::Rgb(248, 248, 242)),
        }
    }

    pub fn nightfox() -> Self {
        // Nightfox palette from https://github.com/EdenEast/nightfox.nvim
        // bg0: #131a24  bg1: #192330  bg2: #212e3f  bg3: #29394f  bg4: #39506d
        // fg0: #d6d6d7  fg1: #cdcecf  fg2: #aeafb0  fg3: #71839b
        // sel0: #2b3b51  sel1: #3c5372
        // black:   #393b44  red:     #c94f6d  green:   #81b29a
        // yellow:  #dbc074  blue:    #719cd6  magenta: #9d79d6
        // cyan:    #63cdcf  white:   #dfdfe0  orange:  #f4a261
        // pink:    #d67ad2  comment: #738091
        Self {
            name: ThemeName::Nightfox,
            heading: [
                // h1: blue
                Style::default()
                    .fg(Color::Rgb(0x71, 0x9c, 0xd6))
                    .add_modifier(Modifier::BOLD),
                // h2: magenta
                Style::default()
                    .fg(Color::Rgb(0x9d, 0x79, 0xd6))
                    .add_modifier(Modifier::BOLD),
                // h3: green
                Style::default()
                    .fg(Color::Rgb(0x81, 0xb2, 0x9a))
                    .add_modifier(Modifier::BOLD),
                // h4: yellow
                Style::default()
                    .fg(Color::Rgb(0xdb, 0xc0, 0x74))
                    .add_modifier(Modifier::BOLD),
                // h5: orange
                Style::default()
                    .fg(Color::Rgb(0xf4, 0xa2, 0x61))
                    .add_modifier(Modifier::BOLD),
                // h6: comment
                Style::default()
                    .fg(Color::Rgb(0x73, 0x80, 0x91))
                    .add_modifier(Modifier::BOLD),
            ],
            // fg1
            paragraph: Style::default().fg(Color::Rgb(0xcd, 0xce, 0xcf)),
            // fg0 bold
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0xd6, 0xd6, 0xd7)),
            // fg0 italic
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0xd6, 0xd6, 0xd7)),
            // fg3 strikethrough
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0x71, 0x83, 0x9b)),
            // yellow on bg2
            code_inline: Style::default()
                .fg(Color::Rgb(0xdb, 0xc0, 0x74))
                .bg(Color::Rgb(0x21, 0x2e, 0x3f)),
            // bg2
            code_block_bg: Style::default().bg(Color::Rgb(0x21, 0x2e, 0x3f)),
            // orange
            blockquote_bar: Style::default().fg(Color::Rgb(0xf4, 0xa2, 0x61)),
            // fg2
            blockquote_text: Style::default().fg(Color::Rgb(0xae, 0xaf, 0xb0)),
            // cyan underlined
            link: Style::default()
                .fg(Color::Rgb(0x63, 0xcd, 0xcf))
                .add_modifier(Modifier::UNDERLINED),
            // bg4
            table_border: Style::default().fg(Color::Rgb(0x39, 0x50, 0x6d)),
            // blue bold
            table_header: Style::default()
                .fg(Color::Rgb(0x71, 0x9c, 0xd6))
                .add_modifier(Modifier::BOLD),
            // bg4
            horizontal_rule: Style::default().fg(Color::Rgb(0x39, 0x50, 0x6d)),
            // green
            list_marker: Style::default().fg(Color::Rgb(0x81, 0xb2, 0x9a)),
            // pink italic
            image_label: Style::default()
                .fg(Color::Rgb(0xd6, 0x7a, 0xd2))
                .add_modifier(Modifier::ITALIC),
            // bg3
            cursor_line: Style::default().bg(Color::Rgb(0x29, 0x39, 0x4f)),
            // cyan on bg0
            top_bar: Style::default()
                .fg(Color::Rgb(0x63, 0xcd, 0xcf))
                .bg(Color::Rgb(0x13, 0x1a, 0x24)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0xa9, 0xdc, 0x76))
                .bg(Color::Rgb(0x13, 0x1a, 0x24))
                .add_modifier(Modifier::BOLD),
            // fg3 on bg0
            bottom_bar: Style::default()
                .fg(Color::Rgb(0x71, 0x83, 0x9b))
                .bg(Color::Rgb(0x13, 0x1a, 0x24)),
            // green bold
            mode_indicator: Style::default()
                .fg(Color::Rgb(0x81, 0xb2, 0x9a))
                .add_modifier(Modifier::BOLD),
            // bg1 on bg4 (muted)
            search_match: Style::default()
                .fg(Color::Rgb(0xcd, 0xce, 0xcf))
                .bg(Color::Rgb(0x39, 0x50, 0x6d)),
            // bg1 on green (bright)
            search_match_current: Style::default()
                .fg(Color::Rgb(0x19, 0x23, 0x30))
                .bg(Color::Rgb(0x81, 0xb2, 0x9a))
                .add_modifier(Modifier::BOLD),
            // bg4
            midpoint_marker: Style::default().fg(Color::Rgb(0x39, 0x50, 0x6d)),
            line_number: Style::default().fg(Color::Rgb(0x39, 0x50, 0x6d)),
            line_number_current: Style::default().fg(Color::Rgb(0xd8, 0xde, 0xe9)),
        }
    }

    /// Rename the old `dark()` to keep backward compatibility.
    pub fn dark() -> Self {
        Self::dracula()
    }

    pub fn compose(base: Style, modifier: Style) -> Style {
        base.patch(modifier)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dracula()
    }
}
