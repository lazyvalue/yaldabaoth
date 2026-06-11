//! Frontend-neutral styling primitives.
//!
//! `Color`, `Modifier`, and `Style` mirror the subset of `ratatui::style`
//! the rest of yalda actually uses, but carry no dependency on ratatui.
//! A future native-desktop frontend can consume these directly; the TUI
//! frontend converts to `ratatui::style::*` at the rendering edge via
//! the `From` impls at the bottom of this module.

use ratatui::style as rs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifier(u16);

impl Modifier {
    pub const EMPTY: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINED: Self = Self(1 << 2);
    pub const CROSSED_OUT: Self = Self(1 << 3);
    pub const REVERSED: Self = Self(1 << 4);
    pub const DIM: Self = Self(1 << 5);

    pub const fn bits(self) -> u16 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifier {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifier {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub modifier: Modifier,
}

impl Style {
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            modifier: Modifier::EMPTY,
        }
    }

    pub fn fg(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }

    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }

    pub fn add_modifier(mut self, m: Modifier) -> Self {
        self.modifier |= m;
        self
    }

    /// Layer `other` on top of `self`: any field set on `other` overrides
    /// the corresponding field on `self`; modifiers are unioned.
    pub fn patch(mut self, other: Self) -> Self {
        if let Some(c) = other.fg {
            self.fg = Some(c);
        }
        if let Some(c) = other.bg {
            self.bg = Some(c);
        }
        self.modifier |= other.modifier;
        self
    }
}

// ---- ratatui adapters ------------------------------------------------------
//
// Conversions live here so the rest of the codebase (theme, blocks, render,
// highlight) can stay frontend-agnostic. `view.rs` is the only place that
// actually invokes `.into()` — at the rendering boundary.

impl From<Color> for rs::Color {
    fn from(c: Color) -> rs::Color {
        match c {
            Color::Reset => rs::Color::Reset,
            Color::Black => rs::Color::Black,
            Color::Red => rs::Color::Red,
            Color::Green => rs::Color::Green,
            Color::Yellow => rs::Color::Yellow,
            Color::Blue => rs::Color::Blue,
            Color::Magenta => rs::Color::Magenta,
            Color::Cyan => rs::Color::Cyan,
            Color::Gray => rs::Color::Gray,
            Color::DarkGray => rs::Color::DarkGray,
            Color::LightRed => rs::Color::LightRed,
            Color::LightGreen => rs::Color::LightGreen,
            Color::LightYellow => rs::Color::LightYellow,
            Color::LightBlue => rs::Color::LightBlue,
            Color::LightMagenta => rs::Color::LightMagenta,
            Color::LightCyan => rs::Color::LightCyan,
            Color::White => rs::Color::White,
            Color::Indexed(i) => rs::Color::Indexed(i),
            Color::Rgb(r, g, b) => rs::Color::Rgb(r, g, b),
        }
    }
}

impl From<Modifier> for rs::Modifier {
    fn from(m: Modifier) -> rs::Modifier {
        let mut out = rs::Modifier::empty();
        if m.contains(Modifier::BOLD) {
            out |= rs::Modifier::BOLD;
        }
        if m.contains(Modifier::ITALIC) {
            out |= rs::Modifier::ITALIC;
        }
        if m.contains(Modifier::UNDERLINED) {
            out |= rs::Modifier::UNDERLINED;
        }
        if m.contains(Modifier::CROSSED_OUT) {
            out |= rs::Modifier::CROSSED_OUT;
        }
        if m.contains(Modifier::REVERSED) {
            out |= rs::Modifier::REVERSED;
        }
        if m.contains(Modifier::DIM) {
            out |= rs::Modifier::DIM;
        }
        out
    }
}

impl From<Style> for rs::Style {
    fn from(s: Style) -> rs::Style {
        let mut out = rs::Style::default();
        if let Some(c) = s.fg {
            out = out.fg(c.into());
        }
        if let Some(c) = s.bg {
            out = out.bg(c.into());
        }
        out.add_modifier(s.modifier.into())
    }
}

// Reverse adapters: ratatui → neutral. Used when view.rs reads a Style
// back out of a `ratatui::Span` it just built (the search-highlight path
// is the only spot today). Lossy: ratatui carries `underline_color` and a
// `sub_modifier` mask that the rest of yalda doesn't use, so dropping
// those round-trips correctly for this codebase.

impl From<rs::Color> for Color {
    fn from(c: rs::Color) -> Color {
        match c {
            rs::Color::Reset => Color::Reset,
            rs::Color::Black => Color::Black,
            rs::Color::Red => Color::Red,
            rs::Color::Green => Color::Green,
            rs::Color::Yellow => Color::Yellow,
            rs::Color::Blue => Color::Blue,
            rs::Color::Magenta => Color::Magenta,
            rs::Color::Cyan => Color::Cyan,
            rs::Color::Gray => Color::Gray,
            rs::Color::DarkGray => Color::DarkGray,
            rs::Color::LightRed => Color::LightRed,
            rs::Color::LightGreen => Color::LightGreen,
            rs::Color::LightYellow => Color::LightYellow,
            rs::Color::LightBlue => Color::LightBlue,
            rs::Color::LightMagenta => Color::LightMagenta,
            rs::Color::LightCyan => Color::LightCyan,
            rs::Color::White => Color::White,
            rs::Color::Indexed(i) => Color::Indexed(i),
            rs::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

impl From<rs::Modifier> for Modifier {
    fn from(m: rs::Modifier) -> Modifier {
        let mut out = Modifier::EMPTY;
        if m.contains(rs::Modifier::BOLD) {
            out |= Modifier::BOLD;
        }
        if m.contains(rs::Modifier::ITALIC) {
            out |= Modifier::ITALIC;
        }
        if m.contains(rs::Modifier::UNDERLINED) {
            out |= Modifier::UNDERLINED;
        }
        if m.contains(rs::Modifier::CROSSED_OUT) {
            out |= Modifier::CROSSED_OUT;
        }
        if m.contains(rs::Modifier::REVERSED) {
            out |= Modifier::REVERSED;
        }
        if m.contains(rs::Modifier::DIM) {
            out |= Modifier::DIM;
        }
        out
    }
}

impl From<rs::Style> for Style {
    fn from(s: rs::Style) -> Style {
        Style {
            fg: s.fg.map(Into::into),
            bg: s.bg.map(Into::into),
            modifier: s.add_modifier.into(),
        }
    }
}
