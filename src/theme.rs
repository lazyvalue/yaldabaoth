use crate::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    #[default]
    Dracula,
    Nightfox,
    SolarizedLight,
    SolarizedDark,
    GruvboxDark,
    FinancialTimes,
    FinancialTimesDark,
}

impl ThemeName {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "dracula" | "dark" => Some(Self::Dracula),
            "nightfox" => Some(Self::Nightfox),
            "solarized" | "solarized-light" | "light" => Some(Self::SolarizedLight),
            "solarized-dark" => Some(Self::SolarizedDark),
            "gruvbox" | "gruvbox-dark" => Some(Self::GruvboxDark),
            "ft" | "financial-times" | "financialtimes" => Some(Self::FinancialTimes),
            "ft-dark" | "financial-times-dark" | "financialtimes-dark" => {
                Some(Self::FinancialTimesDark)
            }
            _ => None,
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            Self::Dracula => "Dracula",
            Self::Nightfox => "Nightfox",
            Self::SolarizedLight => "Solarized Light",
            Self::SolarizedDark => "Solarized Dark",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::FinancialTimes => "Financial Times",
            Self::FinancialTimesDark => "Financial Times Dark",
        }
    }

    /// Kebab-case identifier suitable for serialization and config files.
    /// `parse()` accepts these strings, so `parse(name.as_kebab()) == Some(name)`.
    pub fn as_kebab(&self) -> &'static str {
        match self {
            Self::Dracula => "dracula",
            Self::Nightfox => "nightfox",
            Self::SolarizedLight => "solarized-light",
            Self::SolarizedDark => "solarized-dark",
            Self::GruvboxDark => "gruvbox-dark",
            Self::FinancialTimes => "financial-times",
            Self::FinancialTimesDark => "financial-times-dark",
        }
    }

    pub fn syntect_theme(&self) -> &'static str {
        match self {
            Self::Dracula
            | Self::Nightfox
            | Self::GruvboxDark
            | Self::SolarizedDark
            | Self::FinancialTimesDark => "base16-ocean.dark",
            Self::SolarizedLight | Self::FinancialTimes => "base16-ocean.light",
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
    /// Editor area background — the surface behind document text, edit
    /// buffers, browser rows, etc. The GPUI binary reads this for every
    /// screen-level `.bg(...)` call so that light themes (Solarized,
    /// Financial Times) actually look light.
    pub editor_bg: Color,
    /// Default foreground used when a span has no explicit color (or
    /// `Color::Reset`). The GPUI binary's `DEFAULT_FG` fallback.
    pub editor_fg: Color,
}

impl Theme {
    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dracula => Self::dracula(),
            ThemeName::Nightfox => Self::nightfox(),
            ThemeName::SolarizedLight => Self::solarized_light(),
            ThemeName::SolarizedDark => Self::solarized_dark(),
            ThemeName::GruvboxDark => Self::gruvbox_dark(),
            ThemeName::FinancialTimes => Self::financial_times(),
            ThemeName::FinancialTimesDark => Self::financial_times_dark(),
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
            editor_bg: Color::Rgb(40, 42, 54),
            editor_fg: Color::Rgb(248, 248, 242),
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
            editor_bg: Color::Rgb(0x13, 0x1a, 0x24),
            editor_fg: Color::Rgb(0xcd, 0xce, 0xcf),
        }
    }

    /// Solarized Light — Ethan Schoonover's classic paper palette.
    /// Background `base3` (#fdf6e3), default text `base00` (#657b83).
    /// Accents come from the standard solarized 8-color set; heading order
    /// follows the "accent-warm-to-cool" convention so h1 stands out most.
    pub fn solarized_light() -> Self {
        // base3:   #fdf6e3   base2:  #eee8d5
        // base1:   #93a1a1   base0:  #839496   base00: #657b83
        // base01:  #586e75   base02: #073642   base03: #002b36
        // yellow:  #b58900   orange: #cb4b16   red:    #dc322f
        // magenta: #d33682   violet: #6c71c4   blue:   #268bd2
        // cyan:    #2aa198   green:  #859900
        Self {
            name: ThemeName::SolarizedLight,
            heading: [
                Style::default()
                    .fg(Color::Rgb(0xcb, 0x4b, 0x16))
                    .add_modifier(Modifier::BOLD), // orange
                Style::default()
                    .fg(Color::Rgb(0xd3, 0x36, 0x82))
                    .add_modifier(Modifier::BOLD), // magenta
                Style::default()
                    .fg(Color::Rgb(0x6c, 0x71, 0xc4))
                    .add_modifier(Modifier::BOLD), // violet
                Style::default()
                    .fg(Color::Rgb(0x26, 0x8b, 0xd2))
                    .add_modifier(Modifier::BOLD), // blue
                Style::default()
                    .fg(Color::Rgb(0x2a, 0xa1, 0x98))
                    .add_modifier(Modifier::BOLD), // cyan
                Style::default()
                    .fg(Color::Rgb(0x85, 0x99, 0x00))
                    .add_modifier(Modifier::BOLD), // green
            ],
            paragraph: Style::default().fg(Color::Rgb(0x58, 0x6e, 0x75)), // base01
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0x00, 0x2b, 0x36)), // base03
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0x58, 0x6e, 0x75)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0x93, 0xa1, 0xa1)), // base1
            code_inline: Style::default()
                .fg(Color::Rgb(0xb5, 0x89, 0x00)) // yellow
                .bg(Color::Rgb(0xee, 0xe8, 0xd5)), // base2
            code_block_bg: Style::default().bg(Color::Rgb(0xee, 0xe8, 0xd5)),
            blockquote_bar: Style::default().fg(Color::Rgb(0xcb, 0x4b, 0x16)), // orange
            blockquote_text: Style::default().fg(Color::Rgb(0x65, 0x7b, 0x83)), // base00
            link: Style::default()
                .fg(Color::Rgb(0x26, 0x8b, 0xd2)) // blue
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0x93, 0xa1, 0xa1)),
            table_header: Style::default()
                .fg(Color::Rgb(0x6c, 0x71, 0xc4))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0x93, 0xa1, 0xa1)),
            list_marker: Style::default().fg(Color::Rgb(0x85, 0x99, 0x00)), // green
            image_label: Style::default()
                .fg(Color::Rgb(0xcb, 0x4b, 0x16))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0xee, 0xe8, 0xd5)),
            top_bar: Style::default()
                .fg(Color::Rgb(0x07, 0x36, 0x42))
                .bg(Color::Rgb(0xee, 0xe8, 0xd5)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0x85, 0x99, 0x00))
                .bg(Color::Rgb(0xee, 0xe8, 0xd5))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0x83, 0x94, 0x96))
                .bg(Color::Rgb(0xee, 0xe8, 0xd5)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0x85, 0x99, 0x00))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0xfd, 0xf6, 0xe3))
                .bg(Color::Rgb(0x93, 0xa1, 0xa1)),
            search_match_current: Style::default()
                .fg(Color::Rgb(0xfd, 0xf6, 0xe3))
                .bg(Color::Rgb(0xb5, 0x89, 0x00))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0x93, 0xa1, 0xa1)),
            line_number: Style::default().fg(Color::Rgb(0x93, 0xa1, 0xa1)),
            line_number_current: Style::default().fg(Color::Rgb(0x00, 0x2b, 0x36)),
            editor_bg: Color::Rgb(0xfd, 0xf6, 0xe3), // base3
            editor_fg: Color::Rgb(0x58, 0x6e, 0x75), // base01
        }
    }

    /// Solarized Dark — Schoonover's original dark mode. Same accent palette
    /// as the light variant; only the bg/fg base tones flip. `base03` (#002b36)
    /// for the editor surface, `base0` (#839496) for body text, `base01` for
    /// strikethrough / muted spans.
    pub fn solarized_dark() -> Self {
        // base03:  #002b36   base02: #073642   base01:  #586e75
        // base00:  #657b83   base0:  #839496   base1:   #93a1a1
        // base2:   #eee8d5   base3:  #fdf6e3
        // yellow:  #b58900   orange: #cb4b16   red:     #dc322f
        // magenta: #d33682   violet: #6c71c4   blue:    #268bd2
        // cyan:    #2aa198   green:  #859900
        Self {
            name: ThemeName::SolarizedDark,
            heading: [
                Style::default()
                    .fg(Color::Rgb(0xcb, 0x4b, 0x16))
                    .add_modifier(Modifier::BOLD), // orange
                Style::default()
                    .fg(Color::Rgb(0xd3, 0x36, 0x82))
                    .add_modifier(Modifier::BOLD), // magenta
                Style::default()
                    .fg(Color::Rgb(0x6c, 0x71, 0xc4))
                    .add_modifier(Modifier::BOLD), // violet
                Style::default()
                    .fg(Color::Rgb(0x26, 0x8b, 0xd2))
                    .add_modifier(Modifier::BOLD), // blue
                Style::default()
                    .fg(Color::Rgb(0x2a, 0xa1, 0x98))
                    .add_modifier(Modifier::BOLD), // cyan
                Style::default()
                    .fg(Color::Rgb(0x85, 0x99, 0x00))
                    .add_modifier(Modifier::BOLD), // green
            ],
            paragraph: Style::default().fg(Color::Rgb(0x83, 0x94, 0x96)), // base0
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0xfd, 0xf6, 0xe3)), // base3
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0x93, 0xa1, 0xa1)), // base1
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0x58, 0x6e, 0x75)), // base01
            code_inline: Style::default()
                .fg(Color::Rgb(0xb5, 0x89, 0x00)) // yellow
                .bg(Color::Rgb(0x07, 0x36, 0x42)), // base02
            code_block_bg: Style::default().bg(Color::Rgb(0x07, 0x36, 0x42)),
            blockquote_bar: Style::default().fg(Color::Rgb(0xcb, 0x4b, 0x16)),
            blockquote_text: Style::default().fg(Color::Rgb(0x93, 0xa1, 0xa1)),
            link: Style::default()
                .fg(Color::Rgb(0x26, 0x8b, 0xd2))
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0x58, 0x6e, 0x75)),
            table_header: Style::default()
                .fg(Color::Rgb(0x6c, 0x71, 0xc4))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0x58, 0x6e, 0x75)),
            list_marker: Style::default().fg(Color::Rgb(0x85, 0x99, 0x00)),
            image_label: Style::default()
                .fg(Color::Rgb(0xcb, 0x4b, 0x16))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0x07, 0x36, 0x42)),
            top_bar: Style::default()
                .fg(Color::Rgb(0xfd, 0xf6, 0xe3))
                .bg(Color::Rgb(0x07, 0x36, 0x42)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0x85, 0x99, 0x00))
                .bg(Color::Rgb(0x07, 0x36, 0x42))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0x65, 0x7b, 0x83))
                .bg(Color::Rgb(0x07, 0x36, 0x42)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0x85, 0x99, 0x00))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0xfd, 0xf6, 0xe3))
                .bg(Color::Rgb(0x58, 0x6e, 0x75)),
            search_match_current: Style::default()
                .fg(Color::Rgb(0x00, 0x2b, 0x36))
                .bg(Color::Rgb(0xb5, 0x89, 0x00))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0x58, 0x6e, 0x75)),
            line_number: Style::default().fg(Color::Rgb(0x58, 0x6e, 0x75)),
            line_number_current: Style::default().fg(Color::Rgb(0xfd, 0xf6, 0xe3)),
            editor_bg: Color::Rgb(0x00, 0x2b, 0x36), // base03
            editor_fg: Color::Rgb(0x83, 0x94, 0x96), // base0
        }
    }

    /// Gruvbox Dark — Pavel Pertsev's warm retro palette (hard contrast).
    /// Background `bg0_h` (#1d2021), default text `fg1` (#ebdbb2). Heading
    /// hues sweep through gruvbox's signature warm aqua / yellow / orange.
    pub fn gruvbox_dark() -> Self {
        // bg0_h: #1d2021  bg0: #282828  bg1: #3c3836  bg2: #504945
        // bg3:   #665c54  bg4: #7c6f64
        // fg1:   #ebdbb2  fg2: #d5c4a1  fg3: #bdae93  fg4: #a89984
        // red:    #fb4934  green: #b8bb26  yellow: #fabd2f
        // blue:   #83a598  purple: #d3869b  aqua:   #8ec07c
        // orange: #fe8019  gray:   #928374
        Self {
            name: ThemeName::GruvboxDark,
            heading: [
                Style::default()
                    .fg(Color::Rgb(0xfa, 0xbd, 0x2f))
                    .add_modifier(Modifier::BOLD), // yellow
                Style::default()
                    .fg(Color::Rgb(0xfe, 0x80, 0x19))
                    .add_modifier(Modifier::BOLD), // orange
                Style::default()
                    .fg(Color::Rgb(0xb8, 0xbb, 0x26))
                    .add_modifier(Modifier::BOLD), // green
                Style::default()
                    .fg(Color::Rgb(0x8e, 0xc0, 0x7c))
                    .add_modifier(Modifier::BOLD), // aqua
                Style::default()
                    .fg(Color::Rgb(0x83, 0xa5, 0x98))
                    .add_modifier(Modifier::BOLD), // blue
                Style::default()
                    .fg(Color::Rgb(0xd3, 0x86, 0x9b))
                    .add_modifier(Modifier::BOLD), // purple
            ],
            paragraph: Style::default().fg(Color::Rgb(0xeb, 0xdb, 0xb2)),
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0xfb, 0xf1, 0xc7)),
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0xd5, 0xc4, 0xa1)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0x92, 0x83, 0x74)),
            code_inline: Style::default()
                .fg(Color::Rgb(0xfa, 0xbd, 0x2f))
                .bg(Color::Rgb(0x3c, 0x38, 0x36)),
            code_block_bg: Style::default().bg(Color::Rgb(0x3c, 0x38, 0x36)),
            blockquote_bar: Style::default().fg(Color::Rgb(0xfe, 0x80, 0x19)),
            blockquote_text: Style::default().fg(Color::Rgb(0xa8, 0x99, 0x84)),
            link: Style::default()
                .fg(Color::Rgb(0x83, 0xa5, 0x98))
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0x50, 0x49, 0x45)),
            table_header: Style::default()
                .fg(Color::Rgb(0xfa, 0xbd, 0x2f))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0x50, 0x49, 0x45)),
            list_marker: Style::default().fg(Color::Rgb(0xb8, 0xbb, 0x26)),
            image_label: Style::default()
                .fg(Color::Rgb(0xfe, 0x80, 0x19))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0x3c, 0x38, 0x36)),
            top_bar: Style::default()
                .fg(Color::Rgb(0xfa, 0xbd, 0x2f))
                .bg(Color::Rgb(0x28, 0x28, 0x28)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0xb8, 0xbb, 0x26))
                .bg(Color::Rgb(0x28, 0x28, 0x28))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0xa8, 0x99, 0x84))
                .bg(Color::Rgb(0x28, 0x28, 0x28)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0xb8, 0xbb, 0x26))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0x1d, 0x20, 0x21))
                .bg(Color::Rgb(0x7c, 0x6f, 0x64)),
            search_match_current: Style::default()
                .fg(Color::Rgb(0x1d, 0x20, 0x21))
                .bg(Color::Rgb(0xfa, 0xbd, 0x2f))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0x50, 0x49, 0x45)),
            line_number: Style::default().fg(Color::Rgb(0x50, 0x49, 0x45)),
            line_number_current: Style::default().fg(Color::Rgb(0xeb, 0xdb, 0xb2)),
            editor_bg: Color::Rgb(0x1d, 0x20, 0x21),
            editor_fg: Color::Rgb(0xeb, 0xdb, 0xb2),
        }
    }

    /// Financial Times — the salmon-pink paper, dark charcoal text, claret
    /// for emphasis, oxford-blue for links. Mirrors the FT.com brand
    /// palette: paper #FFF1E5, claret #990F3D, oxford #0F5499, teal
    /// #0D7680, slate #33302E. Headings sweep claret → oxford → teal in
    /// the order an FT page tends to use them (claret for the lead).
    pub fn financial_times() -> Self {
        // Paper:          #fff1e5   Wheat:    #f2dfce
        // Slate:          #33302e   Black:    #1a1a1a
        // Claret:         #990f3d   Oxford:   #0f5499
        // Teal:           #0d7680   Mandarin: #ff8833
        // Wheat-tint-15:  #ccc1b7   Sage:     #4e6e58
        Self {
            name: ThemeName::FinancialTimes,
            heading: [
                Style::default()
                    .fg(Color::Rgb(0x99, 0x0f, 0x3d))
                    .add_modifier(Modifier::BOLD), // claret
                Style::default()
                    .fg(Color::Rgb(0x0f, 0x54, 0x99))
                    .add_modifier(Modifier::BOLD), // oxford
                Style::default()
                    .fg(Color::Rgb(0x0d, 0x76, 0x80))
                    .add_modifier(Modifier::BOLD), // teal
                Style::default()
                    .fg(Color::Rgb(0x33, 0x30, 0x2e))
                    .add_modifier(Modifier::BOLD), // slate
                Style::default()
                    .fg(Color::Rgb(0xff, 0x88, 0x33))
                    .add_modifier(Modifier::BOLD), // mandarin
                Style::default()
                    .fg(Color::Rgb(0x4e, 0x6e, 0x58))
                    .add_modifier(Modifier::BOLD), // sage
            ],
            paragraph: Style::default().fg(Color::Rgb(0x33, 0x30, 0x2e)), // slate
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0x1a, 0x1a, 0x1a)),
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0x33, 0x30, 0x2e)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0xcc, 0xc1, 0xb7)),
            // Code on the muted wheat tint so it still reads as a pull-out
            // but doesn't stab the eye against the paper.
            code_inline: Style::default()
                .fg(Color::Rgb(0x99, 0x0f, 0x3d))
                .bg(Color::Rgb(0xf2, 0xdf, 0xce)),
            code_block_bg: Style::default().bg(Color::Rgb(0xf2, 0xdf, 0xce)),
            blockquote_bar: Style::default().fg(Color::Rgb(0x99, 0x0f, 0x3d)), // claret
            blockquote_text: Style::default().fg(Color::Rgb(0x33, 0x30, 0x2e)),
            link: Style::default()
                .fg(Color::Rgb(0x0f, 0x54, 0x99)) // oxford
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0xcc, 0xc1, 0xb7)),
            table_header: Style::default()
                .fg(Color::Rgb(0x99, 0x0f, 0x3d))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0xcc, 0xc1, 0xb7)),
            list_marker: Style::default().fg(Color::Rgb(0x99, 0x0f, 0x3d)),
            image_label: Style::default()
                .fg(Color::Rgb(0x0d, 0x76, 0x80))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0xf2, 0xdf, 0xce)),
            // Chrome strips share the wheat tint — like a sidebar rule on
            // the FT page rather than a fully-inverted toolbar.
            top_bar: Style::default()
                .fg(Color::Rgb(0x99, 0x0f, 0x3d))
                .bg(Color::Rgb(0xf2, 0xdf, 0xce)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0x0f, 0x54, 0x99))
                .bg(Color::Rgb(0xf2, 0xdf, 0xce))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0x33, 0x30, 0x2e))
                .bg(Color::Rgb(0xf2, 0xdf, 0xce)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0x99, 0x0f, 0x3d))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0x33, 0x30, 0x2e))
                .bg(Color::Rgb(0xff, 0xc6, 0x9b)),
            search_match_current: Style::default()
                .fg(Color::Rgb(0xff, 0xf1, 0xe5))
                .bg(Color::Rgb(0x99, 0x0f, 0x3d))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0xcc, 0xc1, 0xb7)),
            line_number: Style::default().fg(Color::Rgb(0xcc, 0xc1, 0xb7)),
            line_number_current: Style::default().fg(Color::Rgb(0x33, 0x30, 0x2e)),
            editor_bg: Color::Rgb(0xff, 0xf1, 0xe5), // paper
            editor_fg: Color::Rgb(0x33, 0x30, 0x2e), // slate
        }
    }

    /// Financial Times Dark — FT.com's night-mode counterpart to the paper
    /// theme. Charcoal background (#1a1a1a) with warm wheat-tint text so the
    /// page still reads like newsprint rather than a generic IDE dark theme.
    /// Headings and accents brighten the claret / oxford / teal palette so
    /// they stay legible against the dark surface.
    pub fn financial_times_dark() -> Self {
        // Paper:        #fff1e5   Wheat:           #f2dfce
        // Slate:        #33302e   Charcoal:        #1a1a1a
        // Claret:       #990f3d   Claret-bright:   #d63b6a
        // Oxford:       #0f5499   Oxford-bright:   #5ea7d9
        // Teal:         #0d7680   Teal-bright:     #34b0b8
        // Mandarin:     #ff8833
        Self {
            name: ThemeName::FinancialTimesDark,
            heading: [
                Style::default()
                    .fg(Color::Rgb(0xd6, 0x3b, 0x6a))
                    .add_modifier(Modifier::BOLD), // claret-bright
                Style::default()
                    .fg(Color::Rgb(0x5e, 0xa7, 0xd9))
                    .add_modifier(Modifier::BOLD), // oxford-bright
                Style::default()
                    .fg(Color::Rgb(0x34, 0xb0, 0xb8))
                    .add_modifier(Modifier::BOLD), // teal-bright
                Style::default()
                    .fg(Color::Rgb(0xf2, 0xdf, 0xce))
                    .add_modifier(Modifier::BOLD), // wheat
                Style::default()
                    .fg(Color::Rgb(0xff, 0x88, 0x33))
                    .add_modifier(Modifier::BOLD), // mandarin
                Style::default()
                    .fg(Color::Rgb(0x7d, 0xa6, 0x8a))
                    .add_modifier(Modifier::BOLD), // sage-bright
            ],
            paragraph: Style::default().fg(Color::Rgb(0xf2, 0xdf, 0xce)), // wheat
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0xff, 0xf1, 0xe5)), // paper
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0xf2, 0xdf, 0xce)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0x66, 0x60, 0x5c)),
            // Code on a slightly warmer charcoal so it stands off from the
            // body but doesn't go pure black.
            code_inline: Style::default()
                .fg(Color::Rgb(0xff, 0xc6, 0x9b))
                .bg(Color::Rgb(0x2a, 0x26, 0x24)),
            code_block_bg: Style::default().bg(Color::Rgb(0x2a, 0x26, 0x24)),
            blockquote_bar: Style::default().fg(Color::Rgb(0xd6, 0x3b, 0x6a)),
            blockquote_text: Style::default().fg(Color::Rgb(0xf2, 0xdf, 0xce)),
            link: Style::default()
                .fg(Color::Rgb(0x5e, 0xa7, 0xd9)) // oxford-bright
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0x4a, 0x44, 0x40)),
            table_header: Style::default()
                .fg(Color::Rgb(0xd6, 0x3b, 0x6a))
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0x4a, 0x44, 0x40)),
            list_marker: Style::default().fg(Color::Rgb(0xd6, 0x3b, 0x6a)),
            image_label: Style::default()
                .fg(Color::Rgb(0x34, 0xb0, 0xb8))
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0x2a, 0x26, 0x24)),
            top_bar: Style::default()
                .fg(Color::Rgb(0xd6, 0x3b, 0x6a))
                .bg(Color::Rgb(0x2a, 0x26, 0x24)),
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0x5e, 0xa7, 0xd9))
                .bg(Color::Rgb(0x2a, 0x26, 0x24))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0xa8, 0x9d, 0x95))
                .bg(Color::Rgb(0x2a, 0x26, 0x24)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0xd6, 0x3b, 0x6a))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0x1a, 0x1a, 0x1a))
                .bg(Color::Rgb(0xff, 0xc6, 0x9b)),
            search_match_current: Style::default()
                .fg(Color::Rgb(0xff, 0xf1, 0xe5))
                .bg(Color::Rgb(0xd6, 0x3b, 0x6a))
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0x4a, 0x44, 0x40)),
            line_number: Style::default().fg(Color::Rgb(0x4a, 0x44, 0x40)),
            line_number_current: Style::default().fg(Color::Rgb(0xff, 0xf1, 0xe5)),
            editor_bg: Color::Rgb(0x1a, 0x1a, 0x1a), // charcoal
            editor_fg: Color::Rgb(0xf2, 0xdf, 0xce), // wheat
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
