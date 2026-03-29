use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyPress {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn from_event(event: KeyEvent) -> Self {
        Self {
            code: event.code,
            modifiers: event.modifiers & !KeyModifiers::SHIFT,
        }
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
    let mut modifiers = KeyModifiers::NONE;
    let mut key_part_index = 0;

    for (i, part) in parts_lower.iter().enumerate() {
        match *part {
            "ctrl" | "control" => {
                modifiers |= KeyModifiers::CONTROL;
                key_part_index = i + 1;
            }
            "alt" | "meta" => {
                modifiers |= KeyModifiers::ALT;
                key_part_index = i + 1;
            }
            "shift" => {
                modifiers |= KeyModifiers::SHIFT;
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
        Ok(KeyPress::new(code, KeyModifiers::NONE))
    }
}

/// Apply shift semantics:
/// - shift-only: uppercase the char, remove SHIFT
/// - shift + other: keep SHIFT, keep char lowercase
fn apply_shift(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if !modifiers.contains(KeyModifiers::SHIFT) {
        return (code, modifiers);
    }

    let other = modifiers & !KeyModifiers::SHIFT;
    if other.is_empty() {
        // shift only: uppercase char and drop SHIFT
        if let KeyCode::Char(c) = code {
            let upper = c.to_uppercase().next().unwrap_or(c);
            (KeyCode::Char(upper), KeyModifiers::NONE)
        } else {
            (code, KeyModifiers::NONE)
        }
    } else {
        // shift + others: keep SHIFT, ensure char is lowercase
        let code = if let KeyCode::Char(c) = code {
            KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
        } else {
            code
        };
        (code, modifiers)
    }
}

/// Parse a key name from a lowercased string (used when modifiers are present).
fn parse_key_name_lower(name: &str, position: usize) -> Result<KeyCode, KeyParseError> {
    match name {
        "space" => return Ok(KeyCode::Char(' ')),
        "enter" | "return" => return Ok(KeyCode::Enter),
        "tab" => return Ok(KeyCode::Tab),
        "esc" | "escape" => return Ok(KeyCode::Esc),
        "backspace" | "bs" => return Ok(KeyCode::Backspace),
        "delete" | "del" => return Ok(KeyCode::Delete),
        "insert" | "ins" => return Ok(KeyCode::Insert),
        "up" => return Ok(KeyCode::Up),
        "down" => return Ok(KeyCode::Down),
        "left" => return Ok(KeyCode::Left),
        "right" => return Ok(KeyCode::Right),
        "home" => return Ok(KeyCode::Home),
        "end" => return Ok(KeyCode::End),
        "pageup" | "pgup" => return Ok(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => return Ok(KeyCode::PageDown),
        _ => {}
    }

    // Function keys: f1-f12
    if let Some(rest) = name.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Ok(KeyCode::F(n));
    }

    // Single character
    let mut chars = name.chars();
    if let Some(c) = chars.next()
        && chars.next().is_none()
    {
        return Ok(KeyCode::Char(c));
    }

    Err(KeyParseError {
        position,
        reason: format!("unknown key '{}'", name),
    })
}

/// Parse a key name from the original (potentially mixed-case) token (no modifiers path).
fn parse_key_name_original(token: &str, position: usize) -> Result<KeyCode, KeyParseError> {
    let lower = token.to_lowercase();
    match lower.as_str() {
        "space" => return Ok(KeyCode::Char(' ')),
        "enter" | "return" => return Ok(KeyCode::Enter),
        "tab" => return Ok(KeyCode::Tab),
        "esc" | "escape" => return Ok(KeyCode::Esc),
        "backspace" | "bs" => return Ok(KeyCode::Backspace),
        "delete" | "del" => return Ok(KeyCode::Delete),
        "insert" | "ins" => return Ok(KeyCode::Insert),
        "up" => return Ok(KeyCode::Up),
        "down" => return Ok(KeyCode::Down),
        "left" => return Ok(KeyCode::Left),
        "right" => return Ok(KeyCode::Right),
        "home" => return Ok(KeyCode::Home),
        "end" => return Ok(KeyCode::End),
        "pageup" | "pgup" => return Ok(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => return Ok(KeyCode::PageDown),
        _ => {}
    }

    // Function keys
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Ok(KeyCode::F(n));
    }

    // Single character — use original to preserve case
    let mut chars = token.chars();
    if let Some(c) = chars.next()
        && chars.next().is_none()
    {
        return Ok(KeyCode::Char(c));
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

    if kp.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if kp.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if kp.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift");
    }

    let key_str = format_key_code(&kp.code);

    if parts.is_empty() {
        key_str
    } else {
        format!("{}-{}", parts.join("-"), key_str)
    }
}

fn format_key_code(code: &KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::F(n) => format!("f{}", n),
        _ => format!("{:?}", code),
    }
}
