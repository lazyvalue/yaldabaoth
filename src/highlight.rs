use ratatui::style::{Color, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SynTheme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::blocks::{StyledLine, StyledSpan};

pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: SynTheme,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set.themes["base16-ocean.dark"].clone();
        Self { syntax_set, theme }
    }

    pub fn highlight(
        &self,
        language: &str,
        code: &str,
        bg_style: Style,
    ) -> Option<Vec<StyledLine>> {
        let syntax = self.syntax_set.find_syntax_by_token(language)?;
        let mut h = HighlightLines::new(syntax, &self.theme);
        let mut lines = Vec::new();

        for line in code.lines() {
            let ranges = h.highlight_line(line, &self.syntax_set).ok()?;
            let spans: Vec<StyledSpan> = ranges
                .into_iter()
                .map(|(style, text)| {
                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    StyledSpan::new(text, bg_style.fg(fg))
                })
                .collect();
            lines.push(StyledLine::new(if spans.is_empty() {
                vec![StyledSpan::new("", bg_style)]
            } else {
                spans
            }));
        }

        Some(lines)
    }
}
