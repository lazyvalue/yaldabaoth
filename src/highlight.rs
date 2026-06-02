use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SynTheme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::blocks::{StyledLine, StyledSpan};
use crate::style::{Color, Style};

pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: SynTheme,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::with_syntect_theme("base16-ocean.dark")
    }
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_syntect_theme(name: &str) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set.themes[name].clone();
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

    /// Highlight a single line of code without cross-line state.
    ///
    /// Each call creates a fresh `HighlightLines`, so multi-line constructs
    /// (block comments, heredocs) won't carry state across calls. This is the
    /// right trade-off for the incremental per-line cache in `md_highlight`,
    /// where re-highlighting from the fence opener would be O(block_size).
    pub fn highlight_line_stateless(
        &self,
        language: &str,
        line: &str,
        bg_style: Style,
    ) -> Option<Vec<(String, Style)>> {
        let syntax = self.syntax_set.find_syntax_by_token(language)?;
        let mut h = HighlightLines::new(syntax, &self.theme);
        let ranges = h.highlight_line(line, &self.syntax_set).ok()?;
        let segs: Vec<(String, Style)> = ranges
            .into_iter()
            .map(|(style, text)| {
                let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                (text.to_string(), bg_style.fg(fg))
            })
            .collect();
        if segs.is_empty() {
            Some(vec![(String::new(), bg_style)])
        } else {
            Some(segs)
        }
    }
}
