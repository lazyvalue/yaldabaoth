//! Frontend-neutral key types.
//!
//! `Key`, `Modifiers`, and `KeyPress` model "a key was pressed" without
//! depending on any specific TUI/GUI framework. The crossterm `From` impls
//! at the bottom of this file are the bridge for `runtime.rs` (the only
//! place that talks to crossterm directly). A GUI frontend (GPUI, web,
//! etc.) writes its own `From<their::KeyEvent> for KeyPress` adapter and
//! then drives the same `keybind.rs` matcher and `Action` enum.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A logical key, independent of any terminal/GUI framework's enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    /// Anything we don't model (media keys, exotic function keys, etc.).
    Other,
}

/// A bitset of modifier keys held during a press. Hand-rolled rather than
/// pulling in `bitflags` — the surface is tiny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const CONTROL: Modifiers = Modifiers(0b001);
    pub const ALT: Modifiers = Modifiers(0b010);
    pub const SHIFT: Modifiers = Modifiers(0b100);

    pub fn contains(self, other: Modifiers) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Modifiers) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for Modifiers {
    type Output = Modifiers;
    fn bitand(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 & rhs.0)
    }
}

impl std::ops::Not for Modifiers {
    type Output = Modifiers;
    fn not(self) -> Modifiers {
        Modifiers((!self.0) & 0b111)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyPress {
    pub fn new(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    /// Convert from a crossterm `KeyEvent`. SHIFT is stripped — by convention
    /// our bindings encode shifted chars as the uppercase char itself
    /// (`KeyPress { key: Key::Char('G'), modifiers: NONE }`), not as
    /// `Key::Char('g') + SHIFT`. See `apply_shift` in `parse_key_combo`.
    pub fn from_event(event: KeyEvent) -> Self {
        let modifiers = Modifiers::from(event.modifiers) & !Modifiers::SHIFT;
        Self {
            key: Key::from(event.code),
            modifiers,
        }
    }
}

impl From<KeyEvent> for KeyPress {
    fn from(event: KeyEvent) -> Self {
        KeyPress::from_event(event)
    }
}

impl From<KeyCode> for Key {
    fn from(code: KeyCode) -> Self {
        match code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Enter => Key::Enter,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Esc => Key::Esc,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Insert => Key::Insert,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::F(n) => Key::F(n),
            _ => Key::Other,
        }
    }
}

impl From<KeyModifiers> for Modifiers {
    fn from(m: KeyModifiers) -> Self {
        let mut out = Modifiers::NONE;
        if m.contains(KeyModifiers::CONTROL) {
            out |= Modifiers::CONTROL;
        }
        if m.contains(KeyModifiers::ALT) {
            out |= Modifiers::ALT;
        }
        if m.contains(KeyModifiers::SHIFT) {
            out |= Modifiers::SHIFT;
        }
        out
    }
}

#[derive(Debug)]
pub struct KeyParseError {
    pub position: usize,
    pub reason: String,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "key parse error at position {}: {}", self.position, self.reason)
    }
}

impl std::error::Error for KeyParseError {}

/// Parse a space-separated sequence of key combos into a Vec<KeyPress>.
pub fn parse_key_sequence(input: &str) -> Result<Vec<KeyPress>, KeyParseError> {
    if input.is_empty() {
        return Err(KeyParseError {
            position: 0,
            reason: "empty key sequence".to_string(),
        });
    }

    let mut result = Vec::new();
    let mut position = 0;

    for token in input.split_whitespace() {
        let kp = parse_key_combo(token, position)?;
        result.push(kp);
        position += token.len() + 1;
    }

    Ok(result)
}

/// Parse a single key combo like "ctrl-d", "alt-shift-x", "enter", "j".
fn parse_key_combo(token: &str, position: usize) -> Result<KeyPress, KeyParseError> {
    let lower = token.to_lowercase();
    let parts_lower: Vec<&str> = lower.splitn(10, '-').collect();

    // Scan for leading modifier prefixes.
    let mut modifiers = Modifiers::NONE;
    let mut key_part_index = 0;

    for (i, part) in parts_lower.iter().enumerate() {
        match *part {
            "ctrl" | "control" => {
                modifiers |= Modifiers::CONTROL;
                key_part_index = i + 1;
            }
            "alt" | "meta" => {
                modifiers |= Modifiers::ALT;
                key_part_index = i + 1;
            }
            "shift" => {
                modifiers |= Modifiers::SHIFT;
                key_part_index = i + 1;
            }
            _ => break,
        }
    }

    let has_modifiers = key_part_index > 0;

    // Trailing dash after modifier(s): "ctrl-"
    if has_modifiers && key_part_index >= parts_lower.len() {
        return Err(KeyParseError {
            position,
            reason: "missing key after modifier".to_string(),
        });
    }

    if has_modifiers {
        // Remaining parts (lowercased) form the key name.
        let key_name = parts_lower[key_part_index..].join("-");
        if key_name.is_empty() {
            return Err(KeyParseError {
                position,
                reason: "missing key after modifier".to_string(),
            });
        }
        let code = parse_key_name_lower(&key_name, position)?;

        // Shift handling:
        // - shift alone: uppercase the char, remove SHIFT from modifiers
        // - shift combined with other modifiers: keep SHIFT, keep char lowercase
        let (final_code, final_modifiers) = apply_shift(code, modifiers);
        Ok(KeyPress::new(final_code, final_modifiers))
    } else {
        // No modifiers. Check for unknown modifier-like prefix.
        // e.g. "super-k" — "super" is alphabetic >1 char but not a known modifier.
        if token.contains('-') && token != "-" {
            let first = &parts_lower[0];
            if first.len() > 1 && first.chars().all(|c| c.is_alphabetic()) {
                return Err(KeyParseError {
                    position,
                    reason: format!("unknown modifier '{}'", first),
                });
            }
        }

        // Use the original token to preserve case for bare chars.
        let code = parse_key_name_original(token, position)?;
        Ok(KeyPress::new(code, Modifiers::NONE))
    }
}

/// Apply shift semantics:
/// - shift-only: uppercase the char, remove SHIFT
/// - shift + other: keep SHIFT, keep char lowercase
fn apply_shift(code: Key, modifiers: Modifiers) -> (Key, Modifiers) {
    if !modifiers.contains(Modifiers::SHIFT) {
        return (code, modifiers);
    }

    let other = modifiers & !Modifiers::SHIFT;
    if other.is_empty() {
        // shift only: uppercase char and drop SHIFT
        if let Key::Char(c) = code {
            let upper = c.to_uppercase().next().unwrap_or(c);
            (Key::Char(upper), Modifiers::NONE)
        } else {
            (code, Modifiers::NONE)
        }
    } else {
        // shift + others: keep SHIFT, ensure char is lowercase
        let code = if let Key::Char(c) = code {
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        } else {
            code
        };
        (code, modifiers)
    }
}

/// Parse a key name from a lowercased string (used when modifiers are present).
fn parse_key_name_lower(name: &str, position: usize) -> Result<Key, KeyParseError> {
    match name {
        "space" => return Ok(Key::Char(' ')),
        "enter" | "return" => return Ok(Key::Enter),
        "tab" => return Ok(Key::Tab),
        "esc" | "escape" => return Ok(Key::Esc),
        "backspace" | "bs" => return Ok(Key::Backspace),
        "delete" | "del" => return Ok(Key::Delete),
        "insert" | "ins" => return Ok(Key::Insert),
        "up" => return Ok(Key::Up),
        "down" => return Ok(Key::Down),
        "left" => return Ok(Key::Left),
        "right" => return Ok(Key::Right),
        "home" => return Ok(Key::Home),
        "end" => return Ok(Key::End),
        "pageup" | "pgup" => return Ok(Key::PageUp),
        "pagedown" | "pgdn" | "pgdown" => return Ok(Key::PageDown),
        _ => {}
    }

    // Function keys: f1-f12
    if let Some(rest) = name.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Ok(Key::F(n));
    }

    // Single character
    let mut chars = name.chars();
    if let Some(c) = chars.next()
        && chars.next().is_none()
    {
        return Ok(Key::Char(c));
    }

    Err(KeyParseError {
        position,
        reason: format!("unknown key '{}'", name),
    })
}

/// Parse a key name from the original (potentially mixed-case) token (no modifiers path).
fn parse_key_name_original(token: &str, position: usize) -> Result<Key, KeyParseError> {
    let lower = token.to_lowercase();
    match lower.as_str() {
        "space" => return Ok(Key::Char(' ')),
        "enter" | "return" => return Ok(Key::Enter),
        "tab" => return Ok(Key::Tab),
        "esc" | "escape" => return Ok(Key::Esc),
        "backspace" | "bs" => return Ok(Key::Backspace),
        "delete" | "del" => return Ok(Key::Delete),
        "insert" | "ins" => return Ok(Key::Insert),
        "up" => return Ok(Key::Up),
        "down" => return Ok(Key::Down),
        "left" => return Ok(Key::Left),
        "right" => return Ok(Key::Right),
        "home" => return Ok(Key::Home),
        "end" => return Ok(Key::End),
        "pageup" | "pgup" => return Ok(Key::PageUp),
        "pagedown" | "pgdn" | "pgdown" => return Ok(Key::PageDown),
        _ => {}
    }

    // Function keys
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Ok(Key::F(n));
    }

    // Single character — use original to preserve case
    let mut chars = token.chars();
    if let Some(c) = chars.next()
        && chars.next().is_none()
    {
        return Ok(Key::Char(c));
    }

    Err(KeyParseError {
        position,
        reason: format!("unknown key '{}'", token),
    })
}

/// Format a key sequence back to notation string.
pub fn format_key_sequence(keys: &[KeyPress]) -> String {
    keys.iter()
        .map(format_key_combo)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_key_combo(kp: &KeyPress) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if kp.modifiers.contains(Modifiers::CONTROL) {
        parts.push("ctrl");
    }
    if kp.modifiers.contains(Modifiers::ALT) {
        parts.push("alt");
    }
    if kp.modifiers.contains(Modifiers::SHIFT) {
        parts.push("shift");
    }

    let key_str = format_key_code(&kp.key);

    if parts.is_empty() {
        key_str
    } else {
        format!("{}-{}", parts.join("-"), key_str)
    }
}

fn format_key_code(code: &Key) -> String {
    match code {
        Key::Char(' ') => "space".to_string(),
        Key::Char(c) => c.to_string(),
        Key::Enter => "enter".to_string(),
        Key::Tab => "tab".to_string(),
        Key::BackTab => "backtab".to_string(),
        Key::Esc => "esc".to_string(),
        Key::Backspace => "backspace".to_string(),
        Key::Delete => "delete".to_string(),
        Key::Insert => "insert".to_string(),
        Key::Up => "up".to_string(),
        Key::Down => "down".to_string(),
        Key::Left => "left".to_string(),
        Key::Right => "right".to_string(),
        Key::Home => "home".to_string(),
        Key::End => "end".to_string(),
        Key::PageUp => "pageup".to_string(),
        Key::PageDown => "pagedown".to_string(),
        Key::F(n) => format!("f{}", n),
        Key::Other => "?".to_string(),
    }
}
