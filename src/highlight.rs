use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem, ThemeSet,
    ThemeSettings,
};
use syntect::parsing::SyntaxSet;

use crate::blocks::{StyledLine, StyledSpan};
use crate::style::{Color, Style};

/// Sentinel name (not a syntect bundled theme) selecting the hand-built Folio
/// syntax palette. `ThemeName::syntect_theme()` returns this for `Folio`.
pub const FOLIO_SYNTECT_THEME: &str = "folio";

const fn syn(r: u8, g: u8, b: u8) -> SynColor {
    SynColor { r, g, b, a: 0xff }
}

fn folio_item(scope: &str, fg: SynColor) -> ThemeItem {
    ThemeItem {
        scope: scope.parse::<ScopeSelectors>().expect("valid scope selector"),
        style: StyleModifier {
            foreground: Some(fg),
            background: None,
            font_style: None,
        },
    }
}

/// Warm, high-contrast syntax palette for the Folio light theme.
///
/// The bundled `base16-ocean.light` accents (pale yellow types, pale-blue
/// functions, mid-green strings) sit at low contrast on Folio's linen code
/// background `#edebe6`. This builds Folio's own designed palette instead —
/// steel keywords, sage strings, teal types, navy functions, comment grey,
/// ink default — all dark-on-linen and readable.
pub fn folio_theme() -> SynTheme {
    // Palette (mirrors the Folio comment block in theme.rs):
    // Ink #342d1f · Comment #756f61 · Steel #405d72 · Sage #495f4e
    // Teal #406764 · Navy #2d3050 · Rust #9a5b2e
    let ink = syn(0x34, 0x2d, 0x1f);
    let comment = syn(0x75, 0x6f, 0x61);
    let steel = syn(0x40, 0x5d, 0x72);
    let sage = syn(0x49, 0x5f, 0x4e);
    let teal = syn(0x40, 0x67, 0x64);
    let navy = syn(0x2d, 0x30, 0x50);
    let rust = syn(0x9a, 0x5b, 0x2e);

    SynTheme {
        name: Some("Folio".to_string()),
        author: Some("yaldabaoth".to_string()),
        settings: ThemeSettings {
            foreground: Some(ink),
            background: Some(syn(0xed, 0xeb, 0xe6)),
            ..Default::default()
        },
        // syntect resolves by selector specificity, not list order.
        scopes: vec![
            folio_item("comment", comment),
            folio_item("string, constant.character, constant.other.symbol", sage),
            folio_item(
                "keyword, storage, storage.type, storage.modifier",
                steel,
            ),
            folio_item(
                "entity.name.type, entity.name.class, support.type, support.class, entity.other.inherited-class",
                teal,
            ),
            folio_item(
                "entity.name.function, support.function, meta.function-call, entity.name.macro",
                navy,
            ),
            folio_item(
                "constant.numeric, constant.language, constant.other, support.constant",
                rust,
            ),
        ],
    }
}

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
        let theme = if name == FOLIO_SYNTECT_THEME {
            folio_theme()
        } else {
            ThemeSet::load_defaults().themes[name].clone()
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    const RUST_SNIPPET: &str =
        "// a note\nfn add(x: usize) -> String {\n    let s = \"hi\";\n    s\n}\n";

    /// Perceived luminance (ITU-R 601) — used to prove tokens are dark enough to
    /// read on Folio's linen code background (`#edebe6`, luminance ~234).
    fn luma(c: Color) -> f32 {
        let Color::Rgb(r, g, b) = c else { unreachable!() };
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
    }

    /// Flatten a highlight into (token_text, fg_color) pairs for the given theme.
    fn tokens(syntect_name: &str) -> Vec<(String, Color)> {
        let hl = Highlighter::with_syntect_theme(syntect_name);
        let bg = Style::default();
        let lines = hl
            .highlight("rust", RUST_SNIPPET, bg)
            .expect("rust syntax present");
        lines
            .into_iter()
            .flat_map(|l| l.spans)
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| (s.text.clone(), s.style.fg.expect("token has fg")))
            .collect()
    }

    fn color_of(toks: &[(String, Color)], needle: &str) -> Color {
        toks.iter()
            .find(|(t, _)| t.contains(needle))
            .unwrap_or_else(|| panic!("no token containing {needle:?}"))
            .1
    }

    #[test]
    fn folio_is_selected_via_theme_name() {
        // The real path the GUI uses: theme name -> syntect theme selector.
        assert_eq!(ThemeName::Folio.syntect_theme(), FOLIO_SYNTECT_THEME);
    }

    #[test]
    fn folio_rust_tokens_are_readable_dark_on_linen() {
        // Route through the real mapping so reverting it also fails this guard.
        let folio = tokens(ThemeName::Folio.syntect_theme());

        // Designed Folio palette (theme.rs Folio block).
        assert_eq!(
            color_of(&folio, "\""),
            Color::Rgb(0x49, 0x5f, 0x4e),
            "string should be sage"
        );
        assert_eq!(
            color_of(&folio, "note"),
            Color::Rgb(0x75, 0x6f, 0x61),
            "comment should be comment-grey"
        );

        // Readability property: EVERY token is clearly darker than the linen bg.
        for (text, c) in &folio {
            assert!(
                luma(*c) < 150.0,
                "token {text:?} luma {} too pale for linen bg",
                luma(*c)
            );
        }
    }

    /// Negative control: the bundled `base16-ocean.light` palette this replaces
    /// paints tokens the Folio path must NOT reproduce (proves the custom theme
    /// is actually on the path — revert the mapping and this fails).
    #[test]
    fn folio_differs_from_base16_ocean_light() {
        let folio = tokens(ThemeName::Folio.syntect_theme());
        let ocean = tokens("base16-ocean.light");

        assert_ne!(
            color_of(&folio, "\""),
            color_of(&ocean, "\""),
            "Folio string color must differ from ocean.light"
        );
        // ocean.light has at least one pale (luma >= 150) token on this snippet;
        // Folio has none — the readability delta we are fixing.
        let ocean_has_pale = ocean.iter().any(|(_, c)| luma(*c) >= 150.0);
        assert!(ocean_has_pale, "expected ocean.light to have a pale token");
    }
}
