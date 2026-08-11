//! Frontend-neutral styling primitives.
//!
//! `Color`, `Modifier`, and `Style` are the subset of styling the rest of
//! yalda actually uses, carrying no dependency on any rendering framework.
//! A frontend (GPUI, web, etc.) converts these to its own style types at
//! the rendering edge.

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
    /// Preserve semantic monospace typography when a visual overlay replaces
    /// the foreground/background colors that normally identify inline code.
    pub const MONOSPACE: Self = Self(1 << 6);

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
