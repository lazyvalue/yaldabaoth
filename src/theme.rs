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
    Folio,
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
            "folio" | "foliohi" => Some(Self::Folio),
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
            Self::Folio => "Folio",
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
            Self::Folio => "folio",
        }
    }

    pub fn syntect_theme(&self) -> &'static str {
        match self {
            Self::Dracula
            | Self::Nightfox
            | Self::GruvboxDark
            | Self::SolarizedDark
            | Self::FinancialTimesDark => "base16-ocean.dark",
            Self::SolarizedLight | Self::FinancialTimes | Self::Folio => "base16-ocean.light",
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
    /// Agent session window theming — colors for the Claude chat surface.
    pub agent: AgentTheme,
    /// Overlay/menu popup theming — command menu, buffer list, session
    /// switcher, rename dialog.
    pub overlay: OverlayTheme,
}

/// Colors and accents for the agent (Claude) session window. Every
/// hardcoded hex in the old `render_agent` path has been lifted here
/// so light/dark themes can customise the entire chat surface.
#[derive(Debug, Clone)]
pub struct AgentTheme {
    // -- left margin indicators --
    /// Left bar color for frozen (Claude) lines.
    pub frozen_bar: Color,
    /// Left bar color for user-authored lines.
    pub user_bar: Color,
    /// Gutter label color for tool-call anchors.
    pub tool_label: Color,
    /// Dim/muted accent for gutters, arrows, etc.
    pub dim: Color,

    // -- author tints applied to base-style text --
    /// Tint applied to Claude prose (lines with no syntax highlight).
    pub agent_tint: Color,
    /// Tint applied to user prose.
    pub user_tint: Color,
    /// Text color for frozen (Claude) lines.
    pub frozen_fg: Color,

    // -- turn card backgrounds --
    /// Background tint for Claude turn cards (subtle).
    pub agent_turn_bg: Color,
    /// Background tint for user turn cards (subtle).
    pub user_turn_bg: Color,

    // -- turn separator --
    /// Color of the turn header label ("Claude" / "You").
    pub turn_header_agent: Color,
    pub turn_header_user: Color,
    /// Color of the horizontal rule in turn separators.
    pub turn_rule: Color,

    // -- tool call cards --
    /// Background for tool call card containers.
    pub tool_card_bg: Color,
    /// Border color for tool call cards.
    pub tool_card_border: Color,
    /// Tool status glyphs.
    pub tool_completed: Color,
    pub tool_in_progress: Color,
    pub tool_failed: Color,
    pub tool_pending: Color,

    // -- tool body tiles --
    /// Background for tool input/content tiles.
    pub tool_body_bg: Color,
    /// Background for tool output tiles.
    pub tool_output_bg: Color,
    /// Text color inside tool body tiles.
    pub tool_body_fg: Color,

    // -- diff highlighting in tool output --
    /// Color for `+` (added) lines in diffs.
    pub diff_add: Color,
    /// Color for `-` (removed) lines in diffs.
    pub diff_remove: Color,
    /// Color for diff header (`---` / `+++` / `@@`) lines.
    pub diff_header: Color,

    // -- selection --
    /// Background highlight for selected text ranges in the transcript.
    pub selection_bg: Color,

    // -- compose panel --
    /// Separator line above the compose panel.
    pub compose_separator: Color,
    /// Cursor color in the agent window.
    pub cursor: Color,

    // -- sidebar chrome --
    /// Background for sidebars (tasklist, subagents).
    pub sidebar_bg: Color,
    /// Border between sidebars and the main transcript.
    pub sidebar_border: Color,
    /// Header text color in sidebars.
    pub sidebar_header: Color,
    /// Active/warm accent (in-flight timer, focused subagent).
    pub warm_accent: Color,

    // -- jump panel (sidebar navigator; UXI-JumpPanel-7/-8/-11) --
    // NOTE: the panel BACKGROUND is no longer theme-owned — `UXI-JumpPanel-11`
    // paints the panel on the same elevated chrome surface as the command menu
    // (`menu_panel_bg`), so the old per-theme `jump_panel_bg` art-direction hook
    // is gone. The accent colors below are still theme-owned.
    /// Project-name + section header color in the jump panel (PINNED / a project
    /// name / UNFILED). Was a fixed `#ff6b6b`; now theme-owned.
    pub jump_header: Color,
    /// Per-cwd "Unfiled" subheader color in the jump panel. Was a fixed
    /// electric-blue `#3b9eff`.
    pub jump_subheader: Color,
    /// The "working" agent-session status star (a reply in flight). Was a fixed
    /// orange `#ff9e64`. (Waiting-on-you = `tool_completed`; neutral = `dim`;
    /// selection/active = `frozen_bar` — those already come from the theme.)
    pub jump_working: Color,
}

/// Colors for popup overlays: command menu, buffer list, session
/// switcher, rename dialog. Previously hardcoded to Dracula values.
#[derive(Debug, Clone)]
pub struct OverlayTheme {
    /// Popup background.
    pub bg: Color,
    /// Border around/beneath the popup.
    pub border: Color,
    /// Dim text: header labels, hint text.
    pub label: Color,
    /// Normal entry text.
    pub fg: Color,
    /// Keybinding / accent text.
    pub key: Color,
    /// Active/submenu/cyan accent.
    pub accent: Color,
    /// Selected-row background.
    pub selected_bg: Color,
    /// Modified/busy indicator.
    pub modified: Color,
    /// Input/filter text color.
    pub input: Color,
}

impl OverlayTheme {
    pub fn dracula() -> Self {
        Self {
            bg: Color::Rgb(0x1e, 0x1e, 0x3a),
            border: Color::Rgb(0x38, 0x3a, 0x4f),
            label: Color::Rgb(0x62, 0x72, 0xa4),
            fg: Color::Rgb(0xcc, 0xcc, 0xcc),
            key: Color::Rgb(0xbd, 0x93, 0xf9),
            accent: Color::Rgb(0x8b, 0xe9, 0xfd),
            selected_bg: Color::Rgb(0x38, 0x3a, 0x4f),
            modified: Color::Rgb(0xff, 0xb8, 0x6c),
            input: Color::Rgb(0xf1, 0xfa, 0x8c),
        }
    }

    pub fn nightfox() -> Self {
        Self {
            bg: Color::Rgb(0x14, 0x1b, 0x25),
            border: Color::Rgb(0x2b, 0x3b, 0x51),
            label: Color::Rgb(0x71, 0x83, 0x9b),       // fg3
            fg: Color::Rgb(0xcd, 0xce, 0xcf),          // fg1
            key: Color::Rgb(0x9d, 0x79, 0xd6),         // purple
            accent: Color::Rgb(0x63, 0xcd, 0xcf),      // cyan
            selected_bg: Color::Rgb(0x2b, 0x3b, 0x51), // sel0
            modified: Color::Rgb(0xdb, 0xc0, 0x74),    // yellow
            input: Color::Rgb(0xdb, 0xc0, 0x74),
        }
    }

    pub fn solarized_light() -> Self {
        Self {
            bg: Color::Rgb(0xee, 0xe8, 0xd5), // base2
            border: Color::Rgb(0xd3, 0xcc, 0xbc),
            label: Color::Rgb(0x93, 0xa1, 0xa1),  // base1
            fg: Color::Rgb(0x58, 0x6e, 0x75),     // base01
            key: Color::Rgb(0x6c, 0x71, 0xc4),    // violet
            accent: Color::Rgb(0x26, 0x8b, 0xd2), // blue
            selected_bg: Color::Rgb(0xd3, 0xcc, 0xbc),
            modified: Color::Rgb(0xb5, 0x89, 0x00), // yellow
            input: Color::Rgb(0xcb, 0x4b, 0x16),    // orange
        }
    }

    pub fn solarized_dark() -> Self {
        Self {
            bg: Color::Rgb(0x01, 0x27, 0x32),
            border: Color::Rgb(0x07, 0x42, 0x4e),
            label: Color::Rgb(0x58, 0x6e, 0x75),  // base01
            fg: Color::Rgb(0x83, 0x94, 0x96),     // base0
            key: Color::Rgb(0x6c, 0x71, 0xc4),    // violet
            accent: Color::Rgb(0x26, 0x8b, 0xd2), // blue
            selected_bg: Color::Rgb(0x07, 0x42, 0x4e),
            modified: Color::Rgb(0xb5, 0x89, 0x00),
            input: Color::Rgb(0xcb, 0x4b, 0x16),
        }
    }

    pub fn gruvbox_dark() -> Self {
        Self {
            bg: Color::Rgb(0x1a, 0x1a, 0x1a),
            border: Color::Rgb(0x3c, 0x38, 0x36), // bg1
            label: Color::Rgb(0x50, 0x49, 0x45),  // bg2
            fg: Color::Rgb(0xeb, 0xdb, 0xb2),     // fg
            key: Color::Rgb(0xd3, 0x86, 0x9b),    // purple
            accent: Color::Rgb(0x83, 0xa5, 0x98), // blue
            selected_bg: Color::Rgb(0x3c, 0x38, 0x36),
            modified: Color::Rgb(0xfa, 0xbd, 0x2f), // yellow
            input: Color::Rgb(0xfa, 0xbd, 0x2f),
        }
    }

    pub fn financial_times() -> Self {
        Self {
            bg: Color::Rgb(0xf2, 0xdf, 0xce), // wheat
            border: Color::Rgb(0xd8, 0xd0, 0xc4),
            label: Color::Rgb(0xcc, 0xc1, 0xb7),
            fg: Color::Rgb(0x33, 0x30, 0x2e),
            key: Color::Rgb(0x99, 0x0f, 0x3d),    // claret
            accent: Color::Rgb(0x0f, 0x54, 0x99), // oxford
            selected_bg: Color::Rgb(0xe4, 0xd4, 0xc2),
            modified: Color::Rgb(0xff, 0x88, 0x33), // mandarin
            input: Color::Rgb(0x99, 0x0f, 0x3d),
        }
    }

    pub fn financial_times_dark() -> Self {
        Self {
            bg: Color::Rgb(0x1e, 0x1b, 0x19),
            border: Color::Rgb(0x36, 0x32, 0x2e),
            label: Color::Rgb(0x4a, 0x44, 0x40),
            fg: Color::Rgb(0xd6, 0xcc, 0xc2),
            key: Color::Rgb(0xd6, 0x3b, 0x6a),    // claret-bright
            accent: Color::Rgb(0x5e, 0xa7, 0xd9), // oxford-bright
            selected_bg: Color::Rgb(0x36, 0x32, 0x2e),
            modified: Color::Rgb(0xff, 0x88, 0x33),
            input: Color::Rgb(0xd6, 0x3b, 0x6a),
        }
    }

    pub fn folio() -> Self {
        Self {
            bg: Color::Rgb(0xed, 0xeb, 0xe6), // linen
            border: Color::Rgb(0xd6, 0xd2, 0xca),
            label: Color::Rgb(0xb5, 0xa4, 0x83),       // tan
            fg: Color::Rgb(0x34, 0x2d, 0x1f),          // ink
            key: Color::Rgb(0x40, 0x5d, 0x72),         // steel
            accent: Color::Rgb(0x40, 0x67, 0x64),      // teal
            selected_bg: Color::Rgb(0xd6, 0xdc, 0xe4), // visual bg
            modified: Color::Rgb(0x8b, 0x35, 0x35),    // error red
            input: Color::Rgb(0x2d, 0x30, 0x50),       // navy
        }
    }
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
            ThemeName::Folio => Self::folio(),
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
            agent: AgentTheme::dracula(),
            overlay: OverlayTheme::dracula(),
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
            agent: AgentTheme::nightfox(),
            overlay: OverlayTheme::nightfox(),
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
            agent: AgentTheme::solarized_light(),
            overlay: OverlayTheme::solarized_light(),
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
            agent: AgentTheme::solarized_dark(),
            overlay: OverlayTheme::solarized_dark(),
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
            agent: AgentTheme::gruvbox_dark(),
            overlay: OverlayTheme::gruvbox_dark(),
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
            agent: AgentTheme::financial_times(),
            overlay: OverlayTheme::financial_times(),
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
            agent: AgentTheme::financial_times_dark(),
            overlay: OverlayTheme::financial_times_dark(),
        }
    }

    /// Folio — a warm, muted light theme inspired by aged paper and ink.
    /// Background `#F6F4F0` (warm off-white), text `#342d1f` (dark sepia).
    /// Steel-blue keywords, sage-green strings, teal types, navy headings.
    pub fn folio() -> Self {
        // Paper:     #F6F4F0   Linen:     #EDEBE6   Parchment: #E4E1DB
        // Ink:       #342d1f   Comment:   #756f61   Tan:       #B5A483
        // Navy:      #2d3050   Steel:     #405d72   Teal:      #406764
        // Sage:      #495f4e   Warm-gray: #524b46   Error:     #8B3535
        // Selection: #D6DCE4
        Self {
            name: ThemeName::Folio,
            heading: [
                // h1–h3: navy (matches markdownH1–H3)
                Style::default()
                    .fg(Color::Rgb(0x2d, 0x30, 0x50))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(0x2d, 0x30, 0x50))
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Rgb(0x2d, 0x30, 0x50))
                    .add_modifier(Modifier::BOLD),
                // h4: steel blue
                Style::default()
                    .fg(Color::Rgb(0x40, 0x5d, 0x72))
                    .add_modifier(Modifier::BOLD),
                // h5: teal
                Style::default()
                    .fg(Color::Rgb(0x40, 0x67, 0x64))
                    .add_modifier(Modifier::BOLD),
                // h6: comment
                Style::default()
                    .fg(Color::Rgb(0x75, 0x6f, 0x61))
                    .add_modifier(Modifier::BOLD),
            ],
            paragraph: Style::default().fg(Color::Rgb(0x34, 0x2d, 0x1f)), // ink
            bold: Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0x34, 0x2d, 0x1f)),
            italic: Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Rgb(0x34, 0x2d, 0x1f)),
            strikethrough: Style::default()
                .add_modifier(Modifier::CROSSED_OUT)
                .fg(Color::Rgb(0xb5, 0xa4, 0x83)), // tan
            code_inline: Style::default()
                .fg(Color::Rgb(0x49, 0x5f, 0x4e)) // sage (matches String)
                .bg(Color::Rgb(0xed, 0xeb, 0xe6)), // linen
            code_block_bg: Style::default().bg(Color::Rgb(0xed, 0xeb, 0xe6)),
            blockquote_bar: Style::default().fg(Color::Rgb(0x40, 0x5d, 0x72)), // steel
            blockquote_text: Style::default()
                .fg(Color::Rgb(0x75, 0x6f, 0x61)) // comment
                .add_modifier(Modifier::ITALIC),
            link: Style::default()
                .fg(Color::Rgb(0x40, 0x5d, 0x72)) // steel (matches markdownLinkText)
                .add_modifier(Modifier::UNDERLINED),
            table_border: Style::default().fg(Color::Rgb(0xb5, 0xa4, 0x83)), // tan
            table_header: Style::default()
                .fg(Color::Rgb(0x2d, 0x30, 0x50)) // navy
                .add_modifier(Modifier::BOLD),
            horizontal_rule: Style::default().fg(Color::Rgb(0xb5, 0xa4, 0x83)),
            list_marker: Style::default().fg(Color::Rgb(0x40, 0x5d, 0x72)), // steel
            image_label: Style::default()
                .fg(Color::Rgb(0x40, 0x67, 0x64)) // teal
                .add_modifier(Modifier::ITALIC),
            cursor_line: Style::default().bg(Color::Rgb(0xed, 0xeb, 0xe6)), // linen
            top_bar: Style::default()
                .fg(Color::Rgb(0x40, 0x5d, 0x72)) // steel
                .bg(Color::Rgb(0xe4, 0xe1, 0xdb)), // parchment
            top_bar_mode: Style::default()
                .fg(Color::Rgb(0x2d, 0x30, 0x50)) // navy
                .bg(Color::Rgb(0xe4, 0xe1, 0xdb))
                .add_modifier(Modifier::BOLD),
            bottom_bar: Style::default()
                .fg(Color::Rgb(0x75, 0x6f, 0x61)) // comment
                .bg(Color::Rgb(0xe4, 0xe1, 0xdb)),
            mode_indicator: Style::default()
                .fg(Color::Rgb(0x40, 0x5d, 0x72))
                .add_modifier(Modifier::BOLD),
            search_match: Style::default()
                .fg(Color::Rgb(0x34, 0x2d, 0x1f))
                .bg(Color::Rgb(0xd6, 0xdc, 0xe4)), // Visual bg
            search_match_current: Style::default()
                .fg(Color::Rgb(0xf6, 0xf4, 0xf0))
                .bg(Color::Rgb(0x40, 0x5d, 0x72)) // steel on paper
                .add_modifier(Modifier::BOLD),
            midpoint_marker: Style::default().fg(Color::Rgb(0xb5, 0xa4, 0x83)),
            line_number: Style::default().fg(Color::Rgb(0xb5, 0xa4, 0x83)), // tan
            line_number_current: Style::default().fg(Color::Rgb(0x34, 0x2d, 0x1f)),
            editor_bg: Color::Rgb(0xf6, 0xf4, 0xf0), // paper
            editor_fg: Color::Rgb(0x34, 0x2d, 0x1f), // ink
            agent: AgentTheme::folio(),
            overlay: OverlayTheme::folio(),
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

impl AgentTheme {
    /// Dracula agent palette — cyan/green accents on the classic dark surface.
    pub fn dracula() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x8b, 0xe9, 0xfd), // cyan
            user_bar: Color::Rgb(0x50, 0xfa, 0x7b),   // green
            tool_label: Color::Rgb(0xf1, 0xfa, 0x8c), // yellow
            dim: Color::Rgb(0x62, 0x72, 0xa4),        // comment

            agent_tint: Color::Rgb(0xa9, 0xd0, 0xe0), // cool gray-blue
            user_tint: Color::Rgb(0xb8, 0xe0, 0x9a),  // warm green
            frozen_fg: Color::Rgb(0xb6, 0xc4, 0xd6),  // muted blue-gray

            agent_turn_bg: Color::Rgb(0x23, 0x27, 0x36), // subtle cool tint
            user_turn_bg: Color::Rgb(0x28, 0x2e, 0x3a),  // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x8b, 0xe9, 0xfd),
            turn_header_user: Color::Rgb(0x50, 0xfa, 0x7b),
            turn_rule: Color::Rgb(0x44, 0x47, 0x5a),

            tool_card_bg: Color::Rgb(0x21, 0x22, 0x2c),
            tool_card_border: Color::Rgb(0x44, 0x47, 0x5a),
            tool_completed: Color::Rgb(0x50, 0xfa, 0x7b),
            tool_in_progress: Color::Rgb(0xf1, 0xfa, 0x8c),
            tool_failed: Color::Rgb(0xff, 0x55, 0x55),
            tool_pending: Color::Rgb(0x62, 0x72, 0xa4),

            tool_body_bg: Color::Rgb(0x1e, 0x1f, 0x29),
            tool_output_bg: Color::Rgb(0x28, 0x2a, 0x36),
            tool_body_fg: Color::Rgb(0xbf, 0xbf, 0xbf),

            diff_add: Color::Rgb(0x50, 0xfa, 0x7b),
            diff_remove: Color::Rgb(0xff, 0x55, 0x55),
            diff_header: Color::Rgb(0xbd, 0x93, 0xf9), // purple

            selection_bg: Color::Rgb(0x44, 0x47, 0x5a), // comment/selection
            compose_separator: Color::Rgb(0x62, 0x72, 0xa4),
            cursor: Color::Rgb(0xff, 0x55, 0x55),

            sidebar_bg: Color::Rgb(0x21, 0x22, 0x2c),
            sidebar_border: Color::Rgb(0x44, 0x47, 0x5a),
            sidebar_header: Color::Rgb(0x8b, 0xe9, 0xfd),
            warm_accent: Color::Rgb(0xf1, 0xfa, 0x8c),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Nightfox agent palette — cool blues and greens on deep navy.
    pub fn nightfox() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x63, 0xcd, 0xcf), // cyan
            user_bar: Color::Rgb(0x81, 0xb2, 0x9a),   // green
            tool_label: Color::Rgb(0xdb, 0xc0, 0x74), // yellow
            dim: Color::Rgb(0x39, 0x50, 0x6d),        // bg4

            agent_tint: Color::Rgb(0x9a, 0xbe, 0xd0),
            user_tint: Color::Rgb(0xa3, 0xc9, 0xb3),
            frozen_fg: Color::Rgb(0xa0, 0xb4, 0xc8),

            agent_turn_bg: Color::Rgb(0x16, 0x1f, 0x2d),
            user_turn_bg: Color::Rgb(0x1c, 0x22, 0x30), // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x63, 0xcd, 0xcf),
            turn_header_user: Color::Rgb(0x81, 0xb2, 0x9a),
            turn_rule: Color::Rgb(0x2b, 0x3b, 0x51), // sel0

            tool_card_bg: Color::Rgb(0x17, 0x1e, 0x28),
            tool_card_border: Color::Rgb(0x2b, 0x3b, 0x51),
            tool_completed: Color::Rgb(0x81, 0xb2, 0x9a),
            tool_in_progress: Color::Rgb(0xdb, 0xc0, 0x74),
            tool_failed: Color::Rgb(0xc9, 0x4f, 0x6d),
            tool_pending: Color::Rgb(0x39, 0x50, 0x6d),

            tool_body_bg: Color::Rgb(0x14, 0x1b, 0x25),
            tool_output_bg: Color::Rgb(0x19, 0x23, 0x30),
            tool_body_fg: Color::Rgb(0xae, 0xaf, 0xb0),

            diff_add: Color::Rgb(0x81, 0xb2, 0x9a),
            diff_remove: Color::Rgb(0xc9, 0x4f, 0x6d),
            diff_header: Color::Rgb(0x9d, 0x79, 0xd6),

            selection_bg: Color::Rgb(0x2b, 0x3b, 0x51), // sel0
            compose_separator: Color::Rgb(0x39, 0x50, 0x6d),
            cursor: Color::Rgb(0xc9, 0x4f, 0x6d),

            sidebar_bg: Color::Rgb(0x17, 0x1e, 0x28),
            sidebar_border: Color::Rgb(0x2b, 0x3b, 0x51),
            sidebar_header: Color::Rgb(0x63, 0xcd, 0xcf),
            warm_accent: Color::Rgb(0xdb, 0xc0, 0x74),

            // Jump panel, art-directed to the Nightfox palette (not the fixed
            // theme-neutral constants): a recessed navy panel, muted-rose headers,
            // soft blue subheaders, and a warm-orange "working" star.
            jump_header: Color::Rgb(0xc9, 0x4f, 0x6d),         // nightfox red
            jump_subheader: Color::Rgb(0x71, 0x9c, 0xd6),      // nightfox blue
            jump_working: Color::Rgb(0xf4, 0xa2, 0x61),        // nightfox orange
        }
    }

    /// Solarized Light agent palette — warm beige with muted accents.
    pub fn solarized_light() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x26, 0x8b, 0xd2), // blue
            user_bar: Color::Rgb(0x85, 0x99, 0x00),   // green
            tool_label: Color::Rgb(0xb5, 0x89, 0x00), // yellow
            dim: Color::Rgb(0x93, 0xa1, 0xa1),        // base1

            agent_tint: Color::Rgb(0x3b, 0x6e, 0x8c),
            user_tint: Color::Rgb(0x5a, 0x6e, 0x20),
            frozen_fg: Color::Rgb(0x47, 0x60, 0x6e),

            agent_turn_bg: Color::Rgb(0xf0, 0xeb, 0xdd), // barely darker than paper
            user_turn_bg: Color::Rgb(0xdd, 0xe4, 0xee),  // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x26, 0x8b, 0xd2),
            turn_header_user: Color::Rgb(0x85, 0x99, 0x00),
            turn_rule: Color::Rgb(0xd3, 0xcc, 0xbc),

            tool_card_bg: Color::Rgb(0xee, 0xe8, 0xd5), // base2
            tool_card_border: Color::Rgb(0xd3, 0xcc, 0xbc),
            tool_completed: Color::Rgb(0x85, 0x99, 0x00),
            tool_in_progress: Color::Rgb(0xb5, 0x89, 0x00),
            tool_failed: Color::Rgb(0xdc, 0x32, 0x2f),
            tool_pending: Color::Rgb(0x93, 0xa1, 0xa1),

            tool_body_bg: Color::Rgb(0xee, 0xe8, 0xd5),
            tool_output_bg: Color::Rgb(0xfd, 0xf6, 0xe3),
            tool_body_fg: Color::Rgb(0x65, 0x7b, 0x83),

            diff_add: Color::Rgb(0x85, 0x99, 0x00),
            diff_remove: Color::Rgb(0xdc, 0x32, 0x2f),
            diff_header: Color::Rgb(0x6c, 0x71, 0xc4), // violet

            selection_bg: Color::Rgb(0xd3, 0xcc, 0xbc), // base2 highlight
            compose_separator: Color::Rgb(0x93, 0xa1, 0xa1),
            cursor: Color::Rgb(0xdc, 0x32, 0x2f),

            sidebar_bg: Color::Rgb(0xee, 0xe8, 0xd5),
            sidebar_border: Color::Rgb(0xd3, 0xcc, 0xbc),
            sidebar_header: Color::Rgb(0x26, 0x8b, 0xd2),
            warm_accent: Color::Rgb(0xb5, 0x89, 0x00),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Solarized Dark agent palette — deep teal surface with solar accents.
    pub fn solarized_dark() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x26, 0x8b, 0xd2),
            user_bar: Color::Rgb(0x85, 0x99, 0x00),
            tool_label: Color::Rgb(0xb5, 0x89, 0x00),
            dim: Color::Rgb(0x58, 0x6e, 0x75),

            agent_tint: Color::Rgb(0x6e, 0x9e, 0xb5),
            user_tint: Color::Rgb(0x8a, 0x9e, 0x50),
            frozen_fg: Color::Rgb(0x78, 0x8e, 0x96),

            agent_turn_bg: Color::Rgb(0x02, 0x30, 0x3c),
            user_turn_bg: Color::Rgb(0x0a, 0x30, 0x40), // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x26, 0x8b, 0xd2),
            turn_header_user: Color::Rgb(0x85, 0x99, 0x00),
            turn_rule: Color::Rgb(0x07, 0x42, 0x4e),

            tool_card_bg: Color::Rgb(0x04, 0x2d, 0x38),
            tool_card_border: Color::Rgb(0x07, 0x42, 0x4e),
            tool_completed: Color::Rgb(0x85, 0x99, 0x00),
            tool_in_progress: Color::Rgb(0xb5, 0x89, 0x00),
            tool_failed: Color::Rgb(0xdc, 0x32, 0x2f),
            tool_pending: Color::Rgb(0x58, 0x6e, 0x75),

            tool_body_bg: Color::Rgb(0x01, 0x27, 0x32),
            tool_output_bg: Color::Rgb(0x00, 0x2b, 0x36),
            tool_body_fg: Color::Rgb(0x83, 0x94, 0x96),

            diff_add: Color::Rgb(0x85, 0x99, 0x00),
            diff_remove: Color::Rgb(0xdc, 0x32, 0x2f),
            diff_header: Color::Rgb(0x6c, 0x71, 0xc4),

            selection_bg: Color::Rgb(0x07, 0x42, 0x4e), // base02 highlight
            compose_separator: Color::Rgb(0x58, 0x6e, 0x75),
            cursor: Color::Rgb(0xdc, 0x32, 0x2f),

            sidebar_bg: Color::Rgb(0x04, 0x2d, 0x38),
            sidebar_border: Color::Rgb(0x07, 0x42, 0x4e),
            sidebar_header: Color::Rgb(0x26, 0x8b, 0xd2),
            warm_accent: Color::Rgb(0xb5, 0x89, 0x00),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Gruvbox Dark agent palette — warm earthy tones on a dark surface.
    pub fn gruvbox_dark() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x83, 0xa5, 0x98), // blue
            user_bar: Color::Rgb(0xb8, 0xbb, 0x26),   // green
            tool_label: Color::Rgb(0xfa, 0xbd, 0x2f), // yellow
            dim: Color::Rgb(0x50, 0x49, 0x45),        // bg2

            agent_tint: Color::Rgb(0x9b, 0xb5, 0xa8),
            user_tint: Color::Rgb(0xc0, 0xc4, 0x6e),
            frozen_fg: Color::Rgb(0xb0, 0xaa, 0x8e),

            agent_turn_bg: Color::Rgb(0x20, 0x24, 0x26),
            user_turn_bg: Color::Rgb(0x22, 0x26, 0x2e), // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x83, 0xa5, 0x98),
            turn_header_user: Color::Rgb(0xb8, 0xbb, 0x26),
            turn_rule: Color::Rgb(0x3c, 0x38, 0x36), // bg1

            tool_card_bg: Color::Rgb(0x1f, 0x1e, 0x1e),
            tool_card_border: Color::Rgb(0x3c, 0x38, 0x36),
            tool_completed: Color::Rgb(0xb8, 0xbb, 0x26),
            tool_in_progress: Color::Rgb(0xfa, 0xbd, 0x2f),
            tool_failed: Color::Rgb(0xfb, 0x49, 0x34),
            tool_pending: Color::Rgb(0x50, 0x49, 0x45),

            tool_body_bg: Color::Rgb(0x1a, 0x1a, 0x1a),
            tool_output_bg: Color::Rgb(0x1d, 0x20, 0x21),
            tool_body_fg: Color::Rgb(0xa8, 0x99, 0x84),

            diff_add: Color::Rgb(0xb8, 0xbb, 0x26),
            diff_remove: Color::Rgb(0xfb, 0x49, 0x34),
            diff_header: Color::Rgb(0xd3, 0x86, 0x9b), // purple

            selection_bg: Color::Rgb(0x3c, 0x38, 0x36), // bg1 highlight
            compose_separator: Color::Rgb(0x50, 0x49, 0x45),
            cursor: Color::Rgb(0xfb, 0x49, 0x34),

            sidebar_bg: Color::Rgb(0x1f, 0x1e, 0x1e),
            sidebar_border: Color::Rgb(0x3c, 0x38, 0x36),
            sidebar_header: Color::Rgb(0xfa, 0xbd, 0x2f),
            warm_accent: Color::Rgb(0xfa, 0xbd, 0x2f),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Financial Times agent palette — salmon paper with claret/oxford accents.
    pub fn financial_times() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x0f, 0x54, 0x99), // oxford
            user_bar: Color::Rgb(0x0d, 0x76, 0x80),   // teal
            tool_label: Color::Rgb(0xff, 0x88, 0x33), // mandarin
            dim: Color::Rgb(0xcc, 0xc1, 0xb7),

            agent_tint: Color::Rgb(0x2a, 0x5a, 0x80),
            user_tint: Color::Rgb(0x1a, 0x5e, 0x62),
            frozen_fg: Color::Rgb(0x44, 0x42, 0x40),

            agent_turn_bg: Color::Rgb(0xf8, 0xec, 0xdd),
            user_turn_bg: Color::Rgb(0xdf, 0xe6, 0xef), // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x0f, 0x54, 0x99),
            turn_header_user: Color::Rgb(0x0d, 0x76, 0x80),
            turn_rule: Color::Rgb(0xd8, 0xd0, 0xc4),

            tool_card_bg: Color::Rgb(0xf2, 0xdf, 0xce), // wheat
            tool_card_border: Color::Rgb(0xd8, 0xd0, 0xc4),
            tool_completed: Color::Rgb(0x0d, 0x76, 0x80),
            tool_in_progress: Color::Rgb(0xff, 0x88, 0x33),
            tool_failed: Color::Rgb(0x99, 0x0f, 0x3d),
            tool_pending: Color::Rgb(0xcc, 0xc1, 0xb7),

            tool_body_bg: Color::Rgb(0xf2, 0xdf, 0xce),
            tool_output_bg: Color::Rgb(0xff, 0xf1, 0xe5),
            tool_body_fg: Color::Rgb(0x33, 0x30, 0x2e),

            diff_add: Color::Rgb(0x0d, 0x76, 0x80),
            diff_remove: Color::Rgb(0x99, 0x0f, 0x3d),
            diff_header: Color::Rgb(0x0f, 0x54, 0x99),

            selection_bg: Color::Rgb(0xd8, 0xd0, 0xc4), // linen highlight
            compose_separator: Color::Rgb(0xcc, 0xc1, 0xb7),
            cursor: Color::Rgb(0x99, 0x0f, 0x3d),

            sidebar_bg: Color::Rgb(0xf2, 0xdf, 0xce),
            sidebar_border: Color::Rgb(0xd8, 0xd0, 0xc4),
            sidebar_header: Color::Rgb(0x99, 0x0f, 0x3d),
            warm_accent: Color::Rgb(0xff, 0x88, 0x33),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Financial Times Dark agent palette — warm charcoal with bright accents.
    pub fn financial_times_dark() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x5e, 0xa7, 0xd9), // oxford-bright
            user_bar: Color::Rgb(0x34, 0xb0, 0xb8),   // teal-bright
            tool_label: Color::Rgb(0xff, 0x88, 0x33),
            dim: Color::Rgb(0x4a, 0x44, 0x40),

            agent_tint: Color::Rgb(0x80, 0xb0, 0xcc),
            user_tint: Color::Rgb(0x5a, 0xb8, 0xb0),
            frozen_fg: Color::Rgb(0xc0, 0xb4, 0xa8),

            agent_turn_bg: Color::Rgb(0x1e, 0x1d, 0x1c),
            user_turn_bg: Color::Rgb(0x22, 0x26, 0x30), // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x5e, 0xa7, 0xd9),
            turn_header_user: Color::Rgb(0x34, 0xb0, 0xb8),
            turn_rule: Color::Rgb(0x36, 0x32, 0x2e),

            tool_card_bg: Color::Rgb(0x22, 0x1f, 0x1d),
            tool_card_border: Color::Rgb(0x36, 0x32, 0x2e),
            tool_completed: Color::Rgb(0x34, 0xb0, 0xb8),
            tool_in_progress: Color::Rgb(0xff, 0x88, 0x33),
            tool_failed: Color::Rgb(0xd6, 0x3b, 0x6a),
            tool_pending: Color::Rgb(0x4a, 0x44, 0x40),

            tool_body_bg: Color::Rgb(0x1e, 0x1b, 0x19),
            tool_output_bg: Color::Rgb(0x1a, 0x1a, 0x1a),
            tool_body_fg: Color::Rgb(0xa8, 0x9d, 0x95),

            diff_add: Color::Rgb(0x34, 0xb0, 0xb8),
            diff_remove: Color::Rgb(0xd6, 0x3b, 0x6a),
            diff_header: Color::Rgb(0x5e, 0xa7, 0xd9),

            selection_bg: Color::Rgb(0x36, 0x32, 0x2e), // charcoal highlight
            compose_separator: Color::Rgb(0x4a, 0x44, 0x40),
            cursor: Color::Rgb(0xd6, 0x3b, 0x6a),

            sidebar_bg: Color::Rgb(0x22, 0x1f, 0x1d),
            sidebar_border: Color::Rgb(0x36, 0x32, 0x2e),
            sidebar_header: Color::Rgb(0xd6, 0x3b, 0x6a),
            warm_accent: Color::Rgb(0xff, 0x88, 0x33),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }

    /// Folio agent palette — warm paper with steel/teal/sage accents.
    pub fn folio() -> Self {
        Self {
            frozen_bar: Color::Rgb(0x40, 0x5d, 0x72), // steel
            user_bar: Color::Rgb(0x49, 0x5f, 0x4e),   // sage
            tool_label: Color::Rgb(0x52, 0x4b, 0x46), // warm-gray
            dim: Color::Rgb(0xb5, 0xa4, 0x83),        // tan

            agent_tint: Color::Rgb(0x2d, 0x3d, 0x4e), // deep steel — darker for readability on paper
            user_tint: Color::Rgb(0x34, 0x2d, 0x1f),  // ink
            frozen_fg: Color::Rgb(0x3a, 0x3e, 0x48),

            agent_turn_bg: Color::Rgb(0xf2, 0xf0, 0xeb), // barely darker than paper
            user_turn_bg: Color::Rgb(0xde, 0xe5, 0xee),  // faint blue tint (UXI-AgentTile-23)

            turn_header_agent: Color::Rgb(0x40, 0x5d, 0x72), // steel
            turn_header_user: Color::Rgb(0x49, 0x5f, 0x4e),  // sage
            turn_rule: Color::Rgb(0xd6, 0xd2, 0xca),

            tool_card_bg: Color::Rgb(0xed, 0xeb, 0xe6), // linen
            tool_card_border: Color::Rgb(0xd6, 0xd2, 0xca),
            tool_completed: Color::Rgb(0x49, 0x5f, 0x4e), // sage
            tool_in_progress: Color::Rgb(0x8b, 0x70, 0x20), // warm amber
            tool_failed: Color::Rgb(0x8b, 0x35, 0x35),    // error
            tool_pending: Color::Rgb(0xb5, 0xa4, 0x83),   // tan

            tool_body_bg: Color::Rgb(0xed, 0xeb, 0xe6), // linen
            tool_output_bg: Color::Rgb(0xf6, 0xf4, 0xf0), // paper
            tool_body_fg: Color::Rgb(0x52, 0x4b, 0x46), // warm-gray

            diff_add: Color::Rgb(0x49, 0x5f, 0x4e),    // sage
            diff_remove: Color::Rgb(0x8b, 0x35, 0x35), // error
            diff_header: Color::Rgb(0x2d, 0x30, 0x50), // navy

            selection_bg: Color::Rgb(0xd6, 0xdc, 0xe4), // Visual bg
            compose_separator: Color::Rgb(0xb5, 0xa4, 0x83),
            cursor: Color::Rgb(0x8b, 0x35, 0x35),

            sidebar_bg: Color::Rgb(0xed, 0xeb, 0xe6),
            sidebar_border: Color::Rgb(0xd6, 0xd2, 0xca),
            sidebar_header: Color::Rgb(0x40, 0x5d, 0x72), // steel
            warm_accent: Color::Rgb(0x8b, 0x70, 0x20),

            jump_header: Color::Rgb(0xff, 0x6b, 0x6b),
            jump_subheader: Color::Rgb(0x3b, 0x9e, 0xff),
            jump_working: Color::Rgb(0xff, 0x9e, 0x64),
        }
    }
}
