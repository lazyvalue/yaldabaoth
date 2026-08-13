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
/// background `#edebe6`. This builds a warm, SATURATED, hue-varied palette
/// instead — wine keywords, indigo functions, teal types, olive strings,
/// burnt-orange numbers, grey comments, ink default — all dark-on-linen and
/// mutually distinct (an earlier muted blue-teal attempt read near-monochrome).
pub fn folio_theme() -> SynTheme {
    // Warm, SATURATED, mutually-distinct hues, all dark on Folio's linen code
    // bg `#edebe6`. Distinct hue is the point: an earlier version used muted
    // blue-teals that read near-monochrome on linen ("no highlighting").
    let ink = syn(0x34, 0x2d, 0x1f); // default text — dark sepia
    let comment = syn(0x8a, 0x81, 0x72); // warm grey
    let wine = syn(0x9d, 0x2b, 0x4e); // keywords — deep magenta-red
    let teal = syn(0x0f, 0x6d, 0x6a); // types — dark teal
    let indigo = syn(0x38, 0x4b, 0xb0); // functions — indigo blue
    let olive = syn(0x4f, 0x6d, 0x1f); // strings — olive green
    let rust = syn(0xb0, 0x50, 0x18); // numbers/constants — burnt orange

    SynTheme {
        name: Some("Folio".to_string()),
        author: Some("yaldabaoth".to_string()),
        settings: ThemeSettings {
            foreground: Some(ink),
            background: Some(syn(0xed, 0xeb, 0xe6)),
            ..Default::default()
        },
        // syntect resolves by selector specificity, not list order: more-specific
        // selectors win, so `keyword.operator`->ink beats `keyword`->wine.
        scopes: vec![
            folio_item("comment", comment),
            folio_item(
                "string, constant.character, constant.other.symbol, string.regexp",
                olive,
            ),
            // fn / let / pub / mut and primitive types all scope as keyword/storage
            // in Rust — one wine "keyword-ish" bucket.
            folio_item("keyword, storage", wine),
            // Operators stay plain ink so code isn't a wine wash (more specific than
            // `keyword`, so this wins). No broad `punctuation` rule — it would eat
            // comment `//` and string quotes.
            folio_item("keyword.operator", ink),
            // Named types: Vec, String, user structs/enums, trait bounds.
            folio_item(
                "entity.name.type, entity.name.class, support.type, support.class, entity.other.inherited-class",
                teal,
            ),
            folio_item(
                "entity.name.function, support.function, meta.function-call, entity.name.macro, support.macro, variable.function",
                indigo,
            ),
            folio_item(
                "constant.numeric, constant.language, constant.other, support.constant, constant.character.escape",
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
    use std::collections::HashSet;

    // A snippet with keyword, primitive + named types, function, string, comment,
    // number, operators — enough to exercise every color bucket.
    const RUST_SNIPPET: &str = "// note\npub fn add(x: usize) -> u8 {\n    let n = 42;\n    let s = \"hi\";\n    Vec::new()\n}\n";

    fn rgb(c: Color) -> (i32, i32, i32) {
        let Color::Rgb(r, g, b) = c else { unreachable!() };
        (r as i32, g as i32, b as i32)
    }

    /// Perceived luminance (ITU-R 601) — proves tokens are dark enough to read on
    /// Folio's linen code background (`#edebe6`, luminance ~235).
    fn luma(c: Color) -> f32 {
        let (r, g, b) = rgb(c);
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
    }

    fn dist(a: Color, b: Color) -> f32 {
        let (ar, ag, ab) = rgb(a);
        let (br, bg, bb) = rgb(b);
        (((ar - br).pow(2) + (ag - bg).pow(2) + (ab - bb).pow(2)) as f32).sqrt()
    }

    /// Highlight the snippet through the REAL per-line path the transcript uses
    /// (`highlight_line_stateless`), returning (token, fg) for non-blank tokens.
    fn tokens(syntect_name: &str) -> Vec<(String, Color)> {
        let hl = Highlighter::with_syntect_theme(syntect_name);
        let mut out = Vec::new();
        for line in RUST_SNIPPET.lines() {
            for (t, st) in hl
                .highlight_line_stateless("rust", line, Style::default())
                .expect("rust syntax present")
            {
                if !t.trim().is_empty() {
                    out.push((t, st.fg.expect("token has fg")));
                }
            }
        }
        out
    }

    fn color_of(toks: &[(String, Color)], needle: &str) -> Color {
        toks.iter()
            .find(|(t, _)| t.trim() == needle)
            .unwrap_or_else(|| panic!("no token == {needle:?}"))
            .1
    }

    #[test]
    fn folio_is_selected_via_theme_name() {
        // The real path the GUI uses: theme name -> syntect theme selector.
        assert_eq!(ThemeName::Folio.syntect_theme(), FOLIO_SYNTECT_THEME);
    }

    #[test]
    fn folio_rust_tokens_are_distinct_and_readable() {
        // Route through the real mapping so reverting it also fails this guard.
        let folio = tokens(ThemeName::Folio.syntect_theme());

        let keyword = color_of(&folio, "fn");
        let type_name = color_of(&folio, "Vec");
        let function = color_of(&folio, "add");
        let string = color_of(&folio, "hi");
        let comment = color_of(&folio, "note");
        let number = color_of(&folio, "42");

        // Designed Folio palette (highlight::folio_theme).
        assert_eq!(keyword, Color::Rgb(0x9d, 0x2b, 0x4e), "keyword=wine");
        assert_eq!(type_name, Color::Rgb(0x0f, 0x6d, 0x6a), "type=teal");
        assert_eq!(function, Color::Rgb(0x38, 0x4b, 0xb0), "function=indigo");
        assert_eq!(string, Color::Rgb(0x4f, 0x6d, 0x1f), "string=olive");
        assert_eq!(comment, Color::Rgb(0x8a, 0x81, 0x72), "comment=grey");
        assert_eq!(number, Color::Rgb(0xb0, 0x50, 0x18), "number=rust");

        // Hue variety — the actual regression that produced "no highlighting" was
        // a near-monochrome palette. Every pair of these six must be far apart in
        // RGB space, and there must be >=6 distinct token colors overall.
        // No two roles collapse to near-identical colors. Warm palettes cluster in
        // the high-R corner, so the floor is modest — but a monochrome regression
        // (the "no highlighting" bug) drives pairs toward ~0 and trips this.
        let roles = [keyword, type_name, function, string, comment, number];
        for (i, a) in roles.iter().enumerate() {
            for b in &roles[i + 1..] {
                assert!(
                    dist(*a, *b) > 55.0,
                    "token colors too similar: {a:?} vs {b:?} (d={})",
                    dist(*a, *b)
                );
            }
        }
        let distinct: HashSet<(i32, i32, i32)> = folio.iter().map(|(_, c)| rgb(*c)).collect();
        assert!(distinct.len() >= 6, "want >=6 distinct colors, got {}", distinct.len());

        // Readability: every token clearly darker than the linen bg (luma ~235).
        for (text, c) in &folio {
            assert!(luma(*c) < 170.0, "token {text:?} luma {} too pale", luma(*c));
        }
    }

    /// Negative control: the bundled `base16-ocean.light` palette this replaces
    /// paints the string a DIFFERENT color — proves the custom theme is actually
    /// on the path (revert the mapping and the exact-color asserts above fail).
    #[test]
    fn folio_differs_from_base16_ocean_light() {
        let folio = tokens(ThemeName::Folio.syntect_theme());
        let ocean = tokens("base16-ocean.light");
        assert_ne!(
            color_of(&folio, "hi"),
            color_of(&ocean, "hi"),
            "Folio string color must differ from ocean.light"
        );
    }
}
