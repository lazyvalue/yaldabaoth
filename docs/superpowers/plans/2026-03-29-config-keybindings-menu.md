# Config: Keybindings and Command Menu Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand config.kdl to support user-defined keybindings and command menus with a unified key notation parser.

**Architecture:** A new `keys` module provides shared key-sequence parsing/formatting used by both keybindings and menus. `KeybindManager` maps to command strings instead of `Action` enums, resolved through `CommandRegistry` at dispatch time. Config loading is fail-hard — any error exits with a clear message.

**Tech Stack:** Rust, crossterm (KeyCode/KeyEvent/KeyModifiers), kdl 6, ratatui

---

### Task 1: Key Notation Parser (`src/keys.rs`)

**Files:**
- Create: `src/keys.rs`
- Modify: `src/lib.rs`
- Test: `tests/keys_test.rs`

- [ ] **Step 1: Write failing tests for key parsing**

Create `tests/keys_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sketch::keys::{parse_key_sequence, format_key_sequence, KeyParseError};

#[test]
fn test_parse_single_char() {
    let keys = parse_key_sequence("j").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('j'));
    assert_eq!(keys[0].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_uppercase_char() {
    let keys = parse_key_sequence("G").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('G'));
    assert_eq!(keys[0].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_ctrl_modifier() {
    let keys = parse_key_sequence("ctrl-d").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('d'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_ctrl_shift() {
    let keys = parse_key_sequence("ctrl-shift-k").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('k'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
}

#[test]
fn test_parse_alt_modifier() {
    let keys = parse_key_sequence("alt-x").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('x'));
    assert_eq!(keys[0].modifiers, KeyModifiers::ALT);
}

#[test]
fn test_parse_multi_key_sequence() {
    let keys = parse_key_sequence("g g").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].code, KeyCode::Char('g'));
    assert_eq!(keys[1].code, KeyCode::Char('g'));
}

#[test]
fn test_parse_mixed_sequence() {
    let keys = parse_key_sequence("ctrl-k h").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].code, KeyCode::Char('k'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
    assert_eq!(keys[1].code, KeyCode::Char('h'));
    assert_eq!(keys[1].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_named_keys() {
    let keys = parse_key_sequence("space").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(' '));

    let keys = parse_key_sequence("enter").unwrap();
    assert_eq!(keys[0].code, KeyCode::Enter);

    let keys = parse_key_sequence("tab").unwrap();
    assert_eq!(keys[0].code, KeyCode::Tab);

    let keys = parse_key_sequence("esc").unwrap();
    assert_eq!(keys[0].code, KeyCode::Esc);

    let keys = parse_key_sequence("backspace").unwrap();
    assert_eq!(keys[0].code, KeyCode::Backspace);
}

#[test]
fn test_parse_arrow_keys() {
    assert_eq!(parse_key_sequence("up").unwrap()[0].code, KeyCode::Up);
    assert_eq!(parse_key_sequence("down").unwrap()[0].code, KeyCode::Down);
    assert_eq!(parse_key_sequence("left").unwrap()[0].code, KeyCode::Left);
    assert_eq!(parse_key_sequence("right").unwrap()[0].code, KeyCode::Right);
}

#[test]
fn test_parse_function_keys() {
    assert_eq!(parse_key_sequence("f1").unwrap()[0].code, KeyCode::F(1));
    assert_eq!(parse_key_sequence("f12").unwrap()[0].code, KeyCode::F(12));
}

#[test]
fn test_parse_home_end_page() {
    assert_eq!(parse_key_sequence("home").unwrap()[0].code, KeyCode::Home);
    assert_eq!(parse_key_sequence("end").unwrap()[0].code, KeyCode::End);
    assert_eq!(parse_key_sequence("pageup").unwrap()[0].code, KeyCode::PageUp);
    assert_eq!(parse_key_sequence("pagedown").unwrap()[0].code, KeyCode::PageDown);
}

#[test]
fn test_parse_ctrl_named_key() {
    let keys = parse_key_sequence("ctrl-space").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(' '));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_case_insensitive_modifiers() {
    let keys = parse_key_sequence("Ctrl-D").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('d'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_shift_k_equivalent() {
    // shift-k and K should both produce uppercase K with no explicit SHIFT modifier
    let keys1 = parse_key_sequence("shift-k").unwrap();
    let keys2 = parse_key_sequence("K").unwrap();
    assert_eq!(keys1[0].code, keys2[0].code);
}

#[test]
fn test_parse_symbols() {
    let keys = parse_key_sequence("/").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('/'));

    let keys = parse_key_sequence(":").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(':'));

    let keys = parse_key_sequence("$").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('$'));

    let keys = parse_key_sequence("{").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('{'));

    let keys = parse_key_sequence("}").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('}'));
}

#[test]
fn test_parse_error_empty() {
    let err = parse_key_sequence("").unwrap_err();
    assert!(err.reason.contains("empty"));
}

#[test]
fn test_parse_error_trailing_modifier() {
    let err = parse_key_sequence("ctrl-").unwrap_err();
    assert!(err.reason.contains("missing key"));
}

#[test]
fn test_parse_error_unknown_modifier() {
    let err = parse_key_sequence("super-k").unwrap_err();
    // "super" is not a known modifier, so it should be treated as unknown key name
    assert!(err.reason.len() > 0);
}

#[test]
fn test_format_single_char() {
    let keys = parse_key_sequence("j").unwrap();
    assert_eq!(format_key_sequence(&keys), "j");
}

#[test]
fn test_format_ctrl() {
    let keys = parse_key_sequence("ctrl-d").unwrap();
    assert_eq!(format_key_sequence(&keys), "ctrl-d");
}

#[test]
fn test_format_multi_key() {
    let keys = parse_key_sequence("g g").unwrap();
    assert_eq!(format_key_sequence(&keys), "g g");
}

#[test]
fn test_format_mixed_sequence() {
    let keys = parse_key_sequence("ctrl-k h").unwrap();
    assert_eq!(format_key_sequence(&keys), "ctrl-k h");
}

#[test]
fn test_format_named_key() {
    let keys = parse_key_sequence("space").unwrap();
    assert_eq!(format_key_sequence(&keys), "space");

    let keys = parse_key_sequence("enter").unwrap();
    assert_eq!(format_key_sequence(&keys), "enter");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test keys_test 2>&1 | tail -5`
Expected: compilation error — module `keys` does not exist

- [ ] **Step 3: Add `pub mod keys` to `src/lib.rs`**

Add after the `pub mod keybind;` line in `src/lib.rs`:

```rust
pub mod keys;
```

- [ ] **Step 4: Implement the key parser**

Create `src/keys.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
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

    pub fn from_event(event: crossterm::event::KeyEvent) -> Self {
        Self {
            code: event.code,
            modifiers: event.modifiers & !KeyModifiers::SHIFT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyParseError {
    pub position: usize,
    pub reason: String,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at position {}: {}", self.position, self.reason)
    }
}

pub fn parse_key_sequence(input: &str) -> Result<Vec<KeyPress>, KeyParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(KeyParseError {
            position: 0,
            reason: "empty key sequence".into(),
        });
    }

    let mut keys = Vec::new();
    for (i, combo) in input.split_whitespace().enumerate() {
        let key = parse_key_combo(combo, i)?;
        keys.push(key);
    }
    Ok(keys)
}

fn parse_key_combo(combo: &str, seq_index: usize) -> Result<KeyPress, KeyParseError> {
    let parts: Vec<&str> = combo.split('-').collect();

    let mut modifiers = KeyModifiers::NONE;
    let mut key_part = None;

    // Walk parts: modifiers first, last part is the key
    // But we need to handle single-char cases like "-" itself
    if combo == "-" {
        return Ok(KeyPress::new(KeyCode::Char('-'), KeyModifiers::NONE));
    }

    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        let lower = part.to_lowercase();

        if i < parts.len() - 1 {
            // Not the last part — must be a modifier
            match lower.as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => {
                    // Not a modifier — could be a multi-char key name containing a dash?
                    // No — our grammar says modifiers are dash-separated.
                    // This is an unknown modifier.
                    return Err(KeyParseError {
                        position: seq_index,
                        reason: format!("unknown modifier \"{}\"", part),
                    });
                }
            }
        } else {
            // Last part — this is the key
            key_part = Some(part);
        }
        i += 1;
    }

    let key_str = key_part.ok_or_else(|| KeyParseError {
        position: seq_index,
        reason: format!("missing key name after modifier in \"{}\"", combo),
    })?;

    if key_str.is_empty() {
        return Err(KeyParseError {
            position: seq_index,
            reason: format!("missing key name after modifier in \"{}\"", combo),
        });
    }

    let code = parse_key_name(key_str, seq_index)?;

    // Handle shift modifier for single chars: shift-k -> 'K' with no SHIFT flag
    // (crossterm represents uppercase as Char('K') without SHIFT modifier)
    if modifiers.contains(KeyModifiers::SHIFT) {
        if let KeyCode::Char(c) = code {
            if c.is_ascii_alphabetic() {
                return Ok(KeyPress::new(
                    KeyCode::Char(c.to_ascii_uppercase()),
                    modifiers & !KeyModifiers::SHIFT,
                ));
            }
        }
    }

    Ok(KeyPress::new(code, modifiers))
}

fn parse_key_name(name: &str, seq_index: usize) -> Result<KeyCode, KeyParseError> {
    // Single character
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        return Ok(KeyCode::Char(c));
    }

    // Named keys (case-insensitive)
    match name.to_lowercase().as_str() {
        "space" => Ok(KeyCode::Char(' ')),
        "enter" | "return" | "cr" => Ok(KeyCode::Enter),
        "tab" => Ok(KeyCode::Tab),
        "esc" | "escape" => Ok(KeyCode::Esc),
        "backspace" | "bs" => Ok(KeyCode::Backspace),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),
        "home" => Ok(KeyCode::Home),
        "end" => Ok(KeyCode::End),
        "pageup" | "pgup" => Ok(KeyCode::PageUp),
        "pagedown" | "pgdn" => Ok(KeyCode::PageDown),
        "delete" | "del" => Ok(KeyCode::Delete),
        "insert" | "ins" => Ok(KeyCode::Insert),
        s if s.starts_with('f') && s.len() <= 3 => {
            if let Ok(n) = s[1..].parse::<u8>() {
                if (1..=12).contains(&n) {
                    return Ok(KeyCode::F(n));
                }
            }
            Err(KeyParseError {
                position: seq_index,
                reason: format!("unknown key name \"{}\"", name),
            })
        }
        _ => Err(KeyParseError {
            position: seq_index,
            reason: format!("unknown key name \"{}\"", name),
        }),
    }
}

pub fn format_key_sequence(keys: &[KeyPress]) -> String {
    keys.iter()
        .map(format_key_combo)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_key_combo(key: &KeyPress) -> String {
    let mut parts = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl".to_string());
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift".to_string());
    }

    let key_name = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(n) => format!("f{}", n),
        _ => "?".to_string(),
    };

    parts.push(key_name);
    parts.join("-")
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test keys_test 2>&1 | tail -5`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/keys.rs src/lib.rs tests/keys_test.rs
git commit -m "feat: add key notation parser and formatter (keys module)"
```

---

### Task 2: Expand Command Registry

**Files:**
- Modify: `src/command.rs`
- Test: existing `src/command.rs` inline tests

- [ ] **Step 1: Write failing tests for new commands and resolve method**

Add to the `#[cfg(test)] mod tests` block in `src/command.rs`:

```rust
#[test]
fn test_all_actions_have_commands() {
    let reg = CommandRegistry::default_registry();
    // Every action that has a default keybind should be reachable by command name
    let expected = vec![
        "scroll-down", "scroll-up", "half-page-down", "half-page-up",
        "full-page-down", "full-page-up", "next-heading", "prev-heading",
        "next-heading-same-level", "prev-heading-same-level",
        "search-forward", "search-backward", "search-next", "search-prev",
        "toggle-view", "open-link", "yank-line", "open-menu",
        "move-left", "move-right", "move-up", "move-down",
        "move-word-forward", "move-word-backward", "move-word-end",
        "move-line-start", "move-line-end",
        "insert-mode", "insert-after", "open-line-below", "open-line-above",
        "delete-char", "delete-line", "undo", "redo", "enter-command",
        "save", "quit", "force-quit", "save-quit", "save-as",
        "goto-top", "goto-bottom", "goto-heading", "file-browser",
    ];
    for name in &expected {
        assert!(reg.lookup(name).is_some(), "missing command: {}", name);
    }
}

#[test]
fn test_resolve_bare_command() {
    let reg = CommandRegistry::default_registry();
    let (action, args) = reg.resolve("save").unwrap();
    assert_eq!(action, Action::Save);
    assert!(args.is_none());
}

#[test]
fn test_resolve_command_with_args() {
    let reg = CommandRegistry::default_registry();
    let (action, args) = reg.resolve("goto-heading 2").unwrap();
    assert_eq!(action, Action::NextHeading);
    assert_eq!(args.as_deref(), Some("2"));
}

#[test]
fn test_resolve_colon_prefix_stripped() {
    let reg = CommandRegistry::default_registry();
    let (action, args) = reg.resolve(":goto-heading 2").unwrap();
    assert_eq!(action, Action::NextHeading);
    assert_eq!(args.as_deref(), Some("2"));
}

#[test]
fn test_resolve_alias() {
    let reg = CommandRegistry::default_registry();
    let (action, args) = reg.resolve("w").unwrap();
    assert_eq!(action, Action::Save);
    assert!(args.is_none());
}

#[test]
fn test_resolve_unknown() {
    let reg = CommandRegistry::default_registry();
    assert!(reg.resolve("nonexistent").is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib command::tests 2>&1 | tail -10`
Expected: `test_all_actions_have_commands` fails (missing commands), `test_resolve_*` fail (method doesn't exist)

- [ ] **Step 3: Add all missing command registrations and resolve method**

Replace the `default_registry` method and add `resolve` in `src/command.rs`:

```rust
    /// Resolve a command string like "save", ":goto-heading 2", or "w" into
    /// an (Action, optional args) pair.
    pub fn resolve(&self, input: &str) -> Option<(Action, Option<String>)> {
        let input = input.strip_prefix(':').unwrap_or(input).trim();
        let (cmd_name, args) = match input.split_once(' ') {
            Some((name, rest)) => (name, Some(rest.to_string())),
            None => (input, None),
        };
        let cmd = self.lookup(cmd_name)?;
        Some((cmd.action, args))
    }

    /// Build the default registry with all built-in commands.
    pub fn default_registry() -> Self {
        Self::new(vec![
            CommandDef {
                name: "save".into(),
                aliases: vec!["w".into()],
                action: Action::Save,
                description: "Save file".into(),
            },
            CommandDef {
                name: "save-as".into(),
                aliases: vec![],
                action: Action::SaveAs,
                description: "Save to new path".into(),
            },
            CommandDef {
                name: "quit".into(),
                aliases: vec!["q".into()],
                action: Action::Quit,
                description: "Quit (warns if modified)".into(),
            },
            CommandDef {
                name: "force-quit".into(),
                aliases: vec!["q!".into()],
                action: Action::ForceQuit,
                description: "Quit without saving".into(),
            },
            CommandDef {
                name: "save-quit".into(),
                aliases: vec!["wq".into()],
                action: Action::SaveQuit,
                description: "Save and quit".into(),
            },
            CommandDef {
                name: "toggle-view".into(),
                aliases: vec![],
                action: Action::ToggleView,
                description: "Switch rendered/raw".into(),
            },
            CommandDef {
                name: "file-browser".into(),
                aliases: vec![],
                action: Action::OpenFileBrowser,
                description: "Open file browser".into(),
            },
            CommandDef {
                name: "search-forward".into(),
                aliases: vec!["search".into()],
                action: Action::SearchForward,
                description: "Search forward".into(),
            },
            CommandDef {
                name: "search-backward".into(),
                aliases: vec![],
                action: Action::SearchBackward,
                description: "Search backward".into(),
            },
            CommandDef {
                name: "search-next".into(),
                aliases: vec![],
                action: Action::SearchNext,
                description: "Next search match".into(),
            },
            CommandDef {
                name: "search-prev".into(),
                aliases: vec![],
                action: Action::SearchPrev,
                description: "Previous search match".into(),
            },
            CommandDef {
                name: "goto-top".into(),
                aliases: vec![],
                action: Action::JumpTop,
                description: "Go to top".into(),
            },
            CommandDef {
                name: "goto-bottom".into(),
                aliases: vec![],
                action: Action::JumpBottom,
                description: "Go to bottom".into(),
            },
            CommandDef {
                name: "goto-heading".into(),
                aliases: vec!["next-heading".into()],
                action: Action::NextHeading,
                description: "Next heading".into(),
            },
            CommandDef {
                name: "prev-heading".into(),
                aliases: vec![],
                action: Action::PrevHeading,
                description: "Previous heading".into(),
            },
            CommandDef {
                name: "next-heading-same-level".into(),
                aliases: vec![],
                action: Action::NextHeadingSameLevel,
                description: "Next heading at same level".into(),
            },
            CommandDef {
                name: "prev-heading-same-level".into(),
                aliases: vec![],
                action: Action::PrevHeadingSameLevel,
                description: "Previous heading at same level".into(),
            },
            CommandDef {
                name: "scroll-down".into(),
                aliases: vec![],
                action: Action::ScrollDown,
                description: "Scroll down one line".into(),
            },
            CommandDef {
                name: "scroll-up".into(),
                aliases: vec![],
                action: Action::ScrollUp,
                description: "Scroll up one line".into(),
            },
            CommandDef {
                name: "half-page-down".into(),
                aliases: vec![],
                action: Action::HalfPageDown,
                description: "Scroll half page down".into(),
            },
            CommandDef {
                name: "half-page-up".into(),
                aliases: vec![],
                action: Action::HalfPageUp,
                description: "Scroll half page up".into(),
            },
            CommandDef {
                name: "full-page-down".into(),
                aliases: vec![],
                action: Action::FullPageDown,
                description: "Scroll full page down".into(),
            },
            CommandDef {
                name: "full-page-up".into(),
                aliases: vec![],
                action: Action::FullPageUp,
                description: "Scroll full page up".into(),
            },
            CommandDef {
                name: "open-link".into(),
                aliases: vec![],
                action: Action::OpenLink,
                description: "Open link under cursor".into(),
            },
            CommandDef {
                name: "yank-line".into(),
                aliases: vec![],
                action: Action::YankLine,
                description: "Yank current line".into(),
            },
            CommandDef {
                name: "open-menu".into(),
                aliases: vec![],
                action: Action::OpenMenu,
                description: "Open command menu".into(),
            },
            CommandDef {
                name: "move-left".into(),
                aliases: vec![],
                action: Action::MoveLeft,
                description: "Move cursor left".into(),
            },
            CommandDef {
                name: "move-right".into(),
                aliases: vec![],
                action: Action::MoveRight,
                description: "Move cursor right".into(),
            },
            CommandDef {
                name: "move-up".into(),
                aliases: vec![],
                action: Action::MoveUp,
                description: "Move cursor up".into(),
            },
            CommandDef {
                name: "move-down".into(),
                aliases: vec![],
                action: Action::MoveDown,
                description: "Move cursor down".into(),
            },
            CommandDef {
                name: "move-word-forward".into(),
                aliases: vec![],
                action: Action::MoveWordForward,
                description: "Move to next word".into(),
            },
            CommandDef {
                name: "move-word-backward".into(),
                aliases: vec![],
                action: Action::MoveWordBackward,
                description: "Move to previous word".into(),
            },
            CommandDef {
                name: "move-word-end".into(),
                aliases: vec![],
                action: Action::MoveWordEnd,
                description: "Move to end of word".into(),
            },
            CommandDef {
                name: "move-line-start".into(),
                aliases: vec![],
                action: Action::MoveLineStart,
                description: "Move to start of line".into(),
            },
            CommandDef {
                name: "move-line-end".into(),
                aliases: vec![],
                action: Action::MoveLineEnd,
                description: "Move to end of line".into(),
            },
            CommandDef {
                name: "insert-mode".into(),
                aliases: vec![],
                action: Action::InsertMode,
                description: "Enter insert mode".into(),
            },
            CommandDef {
                name: "insert-after".into(),
                aliases: vec![],
                action: Action::InsertAfter,
                description: "Insert after cursor".into(),
            },
            CommandDef {
                name: "open-line-below".into(),
                aliases: vec![],
                action: Action::OpenLineBelow,
                description: "Open line below".into(),
            },
            CommandDef {
                name: "open-line-above".into(),
                aliases: vec![],
                action: Action::OpenLineAbove,
                description: "Open line above".into(),
            },
            CommandDef {
                name: "delete-char".into(),
                aliases: vec![],
                action: Action::DeleteChar,
                description: "Delete character".into(),
            },
            CommandDef {
                name: "delete-line".into(),
                aliases: vec![],
                action: Action::DeleteLine,
                description: "Delete current line".into(),
            },
            CommandDef {
                name: "undo".into(),
                aliases: vec![],
                action: Action::Undo,
                description: "Undo last change".into(),
            },
            CommandDef {
                name: "redo".into(),
                aliases: vec![],
                action: Action::Redo,
                description: "Redo last change".into(),
            },
            CommandDef {
                name: "enter-command".into(),
                aliases: vec![],
                action: Action::EnterCommand,
                description: "Enter command mode".into(),
            },
        ])
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib command::tests 2>&1 | tail -10`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/command.rs
git commit -m "feat: expand command registry with all actions and resolve method"
```

---

### Task 3: Refactor KeybindManager to Use Command Strings

**Files:**
- Modify: `src/keybind.rs`
- Modify: `tests/keybind_test.rs`

- [ ] **Step 1: Update tests to use new API**

Replace `tests/keybind_test.rs` entirely:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::keybind::KeybindManager;

#[test]
fn test_single_key_binding() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some("move-down".to_string()));
}

#[test]
fn test_multi_key_sequence_gg() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result2, Some("goto-top".to_string()));
}

#[test]
fn test_multi_key_timeout_resets() {
    let mut mgr = KeybindManager::default();
    let _ = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    mgr.reset_pending();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some("move-down".to_string()));
}

#[test]
fn test_ctrl_modifier() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(result, Some("half-page-down".to_string()));
}

#[test]
fn test_unknown_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(result, None);
}

#[test]
fn test_space_opens_menu() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(result, Some("open-menu".to_string()));
}

#[test]
fn test_insert_mode_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(result, Some("insert-mode".to_string()));
}

#[test]
fn test_dd_delete_line() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(result2, Some("delete-line".to_string()));
}

#[test]
fn test_gx_open_link() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(result2, Some("open-link".to_string()));
}

#[test]
fn test_enter_command() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert_eq!(result, Some("enter-command".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test keybind_test 2>&1 | tail -5`
Expected: type mismatch — `process_key` returns `Option<Action>` not `Option<String>`

- [ ] **Step 3: Rewrite `KeybindManager` to use command strings and public `KeyPress`**

Replace `src/keybind.rs` entirely:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::keys::KeyPress;

const MULTI_KEY_TIMEOUT: Duration = Duration::from_secs(1);

/// Shared key-sequence matching logic used by both KeybindManager and MenuState.
pub struct KeySequenceMatcher {
    pending: Vec<KeyPress>,
    pending_since: Option<Instant>,
}

impl KeySequenceMatcher {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_since: None,
        }
    }

    /// Feed a key event. Returns:
    /// - Some(matched_sequence) if a binding matched
    /// - None if still accumulating or no match
    pub fn feed<V>(
        &mut self,
        event: KeyEvent,
        single: &HashMap<KeyPress, V>,
        multi: &HashMap<Vec<KeyPress>, V>,
    ) -> Option<Vec<KeyPress>> {
        if let Some(since) = self.pending_since {
            if since.elapsed() > MULTI_KEY_TIMEOUT {
                self.pending.clear();
                self.pending_since = None;
            }
        }

        let press = KeyPress::from_event(event);
        self.pending.push(press.clone());
        self.pending_since = Some(Instant::now());

        // Check multi-key match
        if multi.contains_key(&self.pending) {
            let matched = self.pending.clone();
            self.pending.clear();
            self.pending_since = None;
            return Some(matched);
        }

        // Check if pending is a prefix of any multi-key binding
        let is_prefix = multi
            .keys()
            .any(|seq| seq.len() > self.pending.len() && seq.starts_with(&self.pending));

        if is_prefix {
            return None;
        }

        // No multi-key match or prefix — try single
        self.pending.clear();
        self.pending_since = None;
        if single.contains_key(&press) {
            return Some(vec![press]);
        }

        None
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_since = None;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

pub struct KeybindManager {
    single: HashMap<KeyPress, String>,
    multi: HashMap<Vec<KeyPress>, String>,
    matcher: KeySequenceMatcher,
}

impl KeybindManager {
    pub fn new(
        single: HashMap<KeyPress, String>,
        multi: HashMap<Vec<KeyPress>, String>,
    ) -> Self {
        Self {
            single,
            multi,
            matcher: KeySequenceMatcher::new(),
        }
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<String> {
        let matched = self.matcher.feed(event, &self.single, &self.multi)?;
        if matched.len() == 1 {
            self.single.get(&matched[0]).cloned()
        } else {
            self.multi.get(&matched).cloned()
        }
    }

    pub fn reset_pending(&mut self) {
        self.matcher.reset();
    }

    pub fn has_pending(&self) -> bool {
        self.matcher.has_pending()
    }
}

impl Default for KeybindManager {
    fn default() -> Self {
        let mut single = HashMap::new();
        let mut multi = HashMap::new();

        single.insert(key('h'), "move-left".into());
        single.insert(key('j'), "move-down".into());
        single.insert(key('k'), "move-up".into());
        single.insert(key('l'), "move-right".into());
        single.insert(key('w'), "move-word-forward".into());
        single.insert(key('b'), "move-word-backward".into());
        single.insert(key('e'), "move-word-end".into());
        single.insert(key('0'), "move-line-start".into());
        single.insert(key('$'), "move-line-end".into());
        single.insert(key('i'), "insert-mode".into());
        single.insert(key('a'), "insert-after".into());
        single.insert(key('o'), "open-line-below".into());
        single.insert(key('O'), "open-line-above".into());
        single.insert(key('x'), "delete-char".into());
        single.insert(key('u'), "undo".into());
        single.insert(key(':'), "enter-command".into());
        single.insert(ctrl('r'), "redo".into());
        single.insert(ctrl('d'), "half-page-down".into());
        single.insert(ctrl('u'), "half-page-up".into());
        single.insert(ctrl('f'), "full-page-down".into());
        single.insert(ctrl('b'), "full-page-up".into());
        single.insert(key('G'), "goto-bottom".into());
        single.insert(key('}'), "goto-heading".into());
        single.insert(key('{'), "prev-heading".into());
        single.insert(key('/'), "search-forward".into());
        single.insert(key('?'), "search-backward".into());
        single.insert(key('n'), "search-next".into());
        single.insert(key('N'), "search-prev".into());
        single.insert(key('y'), "yank-line".into());
        single.insert(key(' '), "open-menu".into());

        multi.insert(vec![key('g'), key('g')], "goto-top".into());
        multi.insert(vec![key('g'), key('x')], "open-link".into());
        multi.insert(vec![key('d'), key('d')], "delete-line".into());
        multi.insert(vec![key(']'), key(']')], "next-heading-same-level".into());
        multi.insert(vec![key('['), key('[')], "prev-heading-same-level".into());

        Self::new(single, multi)
    }
}

fn key(c: char) -> KeyPress {
    KeyPress::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyPress {
    KeyPress::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
```

- [ ] **Step 4: Run keybind tests to verify they pass**

Run: `cargo test --test keybind_test 2>&1 | tail -5`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/keybind.rs tests/keybind_test.rs
git commit -m "refactor: KeybindManager maps to command strings, extract KeySequenceMatcher"
```

---

### Task 4: Update App to Resolve Commands Through Registry

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Update imports in `src/app.rs`**

Replace the keybind import line:

```rust
// Old:
use sketch::keybind::{Action, KeybindManager};

// New:
use sketch::keybind::KeybindManager;
use sketch::command::CommandRegistry;
```

Note: `Action` is still used in `execute_action`. Import it from `command` or keep it from `keybind` — but `Action` stays in `keybind.rs`... Actually, `Action` needs to move or stay accessible. Since the `Action` enum is used by `CommandRegistry` and `execute_action`, keep it in `keybind.rs` and import it:

```rust
use sketch::keybind::{Action, KeybindManager};
```

Wait — `Action` is no longer in `keybind.rs` in the new version. We need to decide where `Action` lives. It's used by `CommandRegistry` (which maps command names to `Action`). The natural home is `command.rs`. Let's move it there.

Actually, looking at the current code more carefully: `Action` is defined in `keybind.rs` and used in `command.rs` via `use crate::keybind::Action`. The cleanest approach is to leave `Action` in `keybind.rs` (it's still a keybind concept — it represents what happens when a key is pressed). Both `command.rs` and `app.rs` import it from there.

Since the new `keybind.rs` no longer defines `Action` (it was removed in Task 3), we need to keep `Action` somewhere. Let's put it in `command.rs` since that's where it's resolved now.

- [ ] **Step 2: Move `Action` enum to `command.rs`**

Add the `Action` enum at the top of `src/command.rs` (before `CommandDef`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    FullPageDown,
    FullPageUp,
    JumpTop,
    JumpBottom,
    NextHeading,
    PrevHeading,
    NextHeadingSameLevel,
    PrevHeadingSameLevel,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    Quit,
    Save,
    SaveAs,
    ForceQuit,
    SaveQuit,
    ToggleView,
    OpenLink,
    YankLine,
    OpenMenu,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordForward,
    MoveWordBackward,
    MoveWordEnd,
    MoveLineStart,
    MoveLineEnd,
    InsertMode,
    InsertAfter,
    OpenLineBelow,
    OpenLineAbove,
    DeleteChar,
    DeleteLine,
    Undo,
    Redo,
    EnterCommand,
    OpenFileBrowser,
    FileBrowserDown,
    FileBrowserUp,
    FileBrowserEnter,
    FileBrowserParentDir,
    FileBrowserFilter,
    FileBrowserClose,
    None,
}
```

Remove the `use crate::keybind::Action;` import from `src/command.rs`.

- [ ] **Step 3: Update `src/app.rs` to resolve keybind commands through registry**

Change the import in `src/app.rs`:

```rust
// Old:
use sketch::keybind::{Action, KeybindManager};

// New:
use sketch::command::Action;
use sketch::keybind::KeybindManager;
```

Change `handle_normal_key` to resolve through registry:

```rust
    fn handle_normal_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        if self.search_input_mode {
            match key.code {
                KeyCode::Enter => {
                    self.search_query = self.search_input_buffer.clone();
                    self.search_input_mode = false;
                    self.perform_search();
                    self.jump_to_match(viewport_height);
                }
                KeyCode::Esc => {
                    self.search_input_mode = false;
                    self.search_input_buffer.clear();
                }
                KeyCode::Backspace => {
                    self.search_input_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.search_input_buffer.push(c);
                }
                _ => {}
            }
            return;
        }

        if let Some(cmd_string) = self.keybinds.process_key(key) {
            self.dispatch_command(&cmd_string, viewport_height, content_width);
        }
    }
```

Update `dispatch_command` to use `resolve`:

```rust
    fn dispatch_command(
        &mut self,
        cmd_input: &str,
        viewport_height: usize,
        content_width: usize,
    ) {
        if let Some((action, _args)) = self.registry.resolve(cmd_input) {
            self.execute_action(action, viewport_height, content_width);
        } else {
            self.command_error = format!("Unknown command: {}", cmd_input);
        }
    }
```

- [ ] **Step 4: Build and run all tests**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -10`
Expected: compiles and all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/command.rs src/keybind.rs src/app.rs
git commit -m "refactor: move Action to command.rs, resolve keybinds through registry"
```

---

### Task 5: Update Menu to Use `KeyPress` and `KeySequenceMatcher`

**Files:**
- Modify: `src/menu.rs`
- Modify: `tests/menu_test.rs`

- [ ] **Step 1: Update menu tests for new API**

Replace `tests/menu_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::menu::{MenuState, MenuNode, MenuAction, default_menu};

#[test]
fn test_menu_starts_inactive() {
    let state = MenuState::new();
    assert!(!state.is_active());
}

#[test]
fn test_menu_open_close() {
    let mut state = MenuState::new();
    state.open();
    assert!(state.is_active());
    state.close();
    assert!(!state.is_active());
}

#[test]
fn test_menu_command_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("file-browser".to_string()));
    assert!(!state.is_active());
}

#[test]
fn test_menu_submenu_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, None);
    assert!(state.is_active());
}

#[test]
fn test_menu_submenu_then_command() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("goto-top".to_string()));
    assert!(!state.is_active());
}

#[test]
fn test_menu_escape_from_submenu() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    state.handle_escape();
    assert!(state.is_active());
    assert!(state.path.is_empty());
}

#[test]
fn test_menu_escape_from_root() {
    let mut state = MenuState::new();
    state.open();
    state.handle_escape();
    assert!(!state.is_active());
}

#[test]
fn test_menu_unrecognized_key_ignored() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, None);
    assert!(state.is_active());
}

#[test]
fn test_menu_separator_and_label() {
    let menu = vec![
        MenuNode::entry("f", "file", "file-browser"),
        MenuNode::separator(),
        MenuNode::label("Navigation"),
        MenuNode::entry("g", "goto top", "goto-top"),
    ];
    let mut state = MenuState::new();
    state.open();
    // 'f' should work
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("file-browser".to_string()));
}

#[test]
fn test_menu_modifier_key_entry() {
    use sketch::keys::KeyPress;
    let menu = vec![
        MenuNode {
            key: vec![KeyPress::new(KeyCode::Char('h'), KeyModifiers::CONTROL)],
            label: "prev heading".into(),
            action: MenuAction::Command("prev-heading".into()),
        },
    ];
    let mut state = MenuState::new();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        &menu,
    );
    assert_eq!(result, Some("prev-heading".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test menu_test 2>&1 | tail -5`
Expected: compilation errors — `process_key_event` doesn't exist, `MenuNode::entry` doesn't exist

- [ ] **Step 3: Rewrite `src/menu.rs`**

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keys::KeyPress;

#[derive(Debug, Clone)]
pub struct MenuNode {
    pub key: Vec<KeyPress>,
    pub label: String,
    pub action: MenuAction,
}

impl MenuNode {
    /// Convenience constructor for a simple single-char entry.
    pub fn entry(key_str: &str, label: &str, action: &str) -> Self {
        let key = crate::keys::parse_key_sequence(key_str)
            .unwrap_or_else(|e| panic!("invalid menu key \"{}\": {}", key_str, e));
        Self {
            key,
            label: label.into(),
            action: MenuAction::Command(action.into()),
        }
    }

    pub fn submenu(key_str: &str, label: &str, children: Vec<MenuNode>) -> Self {
        let key = crate::keys::parse_key_sequence(key_str)
            .unwrap_or_else(|e| panic!("invalid menu key \"{}\": {}", key_str, e));
        Self {
            key,
            label: label.into(),
            action: MenuAction::Submenu(children),
        }
    }

    pub fn separator() -> Self {
        Self {
            key: Vec::new(),
            label: String::new(),
            action: MenuAction::Separator,
        }
    }

    pub fn label(text: &str) -> Self {
        Self {
            key: Vec::new(),
            label: text.into(),
            action: MenuAction::Label(text.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(String),
    Separator,
    Label(String),
}

pub struct MenuState {
    active: bool,
    pub path: Vec<usize>,
}

impl MenuState {
    pub fn new() -> Self {
        Self {
            active: false,
            path: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn open(&mut self) {
        self.active = true;
        self.path.clear();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.path.clear();
    }

    /// Process a key event in the menu. Returns Some(command_string) if a command was selected.
    pub fn process_key_event(&mut self, event: KeyEvent, menu: &[MenuNode]) -> Option<String> {
        let press = KeyPress::from_event(event);
        let nodes = self.current_nodes(menu);

        for (i, node) in nodes.iter().enumerate() {
            // Skip non-interactive nodes
            if matches!(node.action, MenuAction::Separator | MenuAction::Label(_)) {
                continue;
            }

            // For single-key menu entries, match directly
            if node.key.len() == 1 && node.key[0] == press {
                match &node.action {
                    MenuAction::Command(cmd) => {
                        let cmd = cmd.clone();
                        self.close();
                        return Some(cmd);
                    }
                    MenuAction::Submenu(_) => {
                        let idx = self.resolve_node_index(menu, &press);
                        if let Some(idx) = idx {
                            self.path.push(idx);
                        }
                        return None;
                    }
                    _ => {}
                }
            }
        }
        None
    }

    pub fn handle_escape(&mut self) {
        if self.path.is_empty() {
            self.close();
        } else {
            self.path.pop();
        }
    }

    pub fn current_nodes<'a>(&self, menu: &'a [MenuNode]) -> &'a [MenuNode] {
        let mut nodes = menu;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                } else {
                    return &[];
                }
            } else {
                return &[];
            }
        }
        nodes
    }

    pub fn current_label(&self, menu: &[MenuNode]) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        let mut nodes = menu;
        let mut label = None;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                label = Some(node.label.clone());
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                }
            }
        }
        label
    }

    fn resolve_node_index(&self, menu: &[MenuNode], press: &KeyPress) -> Option<usize> {
        let mut target = menu;
        for &idx in &self.path {
            if let Some(node) = target.get(idx)
                && let MenuAction::Submenu(children) = &node.action
            {
                target = children;
            }
        }
        target.iter().position(|n| n.key.len() == 1 && n.key[0] == *press)
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default menu tree.
pub fn default_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("f", "file browser", "file-browser"),
        MenuNode::entry("/", "search", "search-forward"),
        MenuNode::entry("q", "quit", "quit"),
        MenuNode::entry("s", "save", "save"),
        MenuNode::entry("v", "toggle view", "toggle-view"),
        MenuNode::submenu("g", "goto", vec![
            MenuNode::entry("g", "top", "goto-top"),
            MenuNode::entry("e", "bottom", "goto-bottom"),
            MenuNode::entry("h", "next heading", "goto-heading"),
        ]),
    ]
}
```

- [ ] **Step 4: Run menu tests to verify they pass**

Run: `cargo test --test menu_test 2>&1 | tail -5`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/menu.rs tests/menu_test.rs
git commit -m "refactor: menu uses KeyPress, adds Separator/Label variants"
```

---

### Task 6: Update App and View for New Menu API

**Files:**
- Modify: `src/app.rs`
- Modify: `src/view.rs`

- [ ] **Step 1: Update app.rs menu handling**

In `src/app.rs`, update the `handle_menu_key` method:

```rust
    fn handle_menu_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        match key.code {
            KeyCode::Esc => {
                self.menu_state.handle_escape();
                if !self.menu_state.is_active() {
                    self.mode = AppMode::Normal;
                }
            }
            _ => {
                if let Some(cmd_string) = self.menu_state.process_key_event(key, &self.menu_tree) {
                    self.mode = AppMode::Normal;
                    self.dispatch_command(&cmd_string, viewport_height, content_width);
                }
            }
        }
    }
```

Update the menu_nodes collection in the `run` method's draw closure. Change:

```rust
                let menu_nodes: Vec<(char, String, bool)> = if self.menu_state.is_active() {
                    self.menu_state
                        .current_nodes(&self.menu_tree)
                        .iter()
                        .map(|n| {
                            let is_sub = matches!(n.action, menu::MenuAction::Submenu(_));
                            (n.key, n.label.clone(), is_sub)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
```

To a new representation that passes the formatted key string instead of a char:

```rust
                let menu_nodes: Vec<(String, String, MenuNodeKind)> = if self.menu_state.is_active() {
                    self.menu_state
                        .current_nodes(&self.menu_tree)
                        .iter()
                        .map(|n| {
                            let key_display = sketch::keys::format_key_sequence(&n.key);
                            let kind = match &n.action {
                                menu::MenuAction::Submenu(_) => MenuNodeKind::Submenu,
                                menu::MenuAction::Command(_) => MenuNodeKind::Command,
                                menu::MenuAction::Separator => MenuNodeKind::Separator,
                                menu::MenuAction::Label(_) => MenuNodeKind::Label,
                            };
                            (key_display, n.label.clone(), kind)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
```

Add the `MenuNodeKind` enum at the top of `src/app.rs` (after `AppMode`):

```rust
#[derive(Debug, PartialEq, Clone)]
pub(crate) enum MenuNodeKind {
    Command,
    Submenu,
    Separator,
    Label,
}
```

- [ ] **Step 2: Update `ViewState` in `src/view.rs`**

Change the `menu_nodes` field type:

```rust
// Old:
    pub menu_nodes: Vec<(char, String, bool)>, // (key, label, is_submenu)

// New:
    pub menu_nodes: Vec<(String, String, MenuNodeKind)>, // (key_display, label, kind)
```

Add at the top of `src/view.rs`:

```rust
use crate::app::MenuNodeKind;
```

Wait — `view.rs` is in the library crate (`sketch`), and `app.rs` is in the binary crate. So `MenuNodeKind` can't be in `app.rs` if `view.rs` needs to import it. Let's put `MenuNodeKind` in `src/menu.rs` instead:

Add to `src/menu.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuNodeKind {
    Command,
    Submenu,
    Separator,
    Label,
}
```

Then in `src/view.rs`:

```rust
use crate::menu::MenuNodeKind;
```

And in `src/app.rs`, change the `menu_nodes` mapping to use `menu::MenuNodeKind`:

```rust
use sketch::menu::{self, MenuNode, MenuState, MenuNodeKind};
```

- [ ] **Step 3: Update `draw_menu_popup` in `src/view.rs`**

Replace the `draw_menu_popup` function:

```rust
fn draw_menu_popup(frame: &mut Frame, area: Rect, state: &ViewState) {
    // Count visible rows: label row + entries + separators
    let popup_height = 2u16;
    let popup_area = Rect::new(area.x, area.y, area.width, popup_height.min(area.height));

    // Opaque background
    let bg = Paragraph::new("").style(Style::default().bg(Color::Rgb(30, 30, 58)));
    frame.render_widget(bg, popup_area);

    // Label row
    let label_text = state.menu_label.as_deref().unwrap_or("Commands");
    let label_line = Line::from(Span::styled(
        format!("  {}", label_text.to_uppercase()),
        Style::default().fg(Color::Rgb(98, 114, 164)),
    ));
    if popup_area.height >= 1 {
        frame.render_widget(
            Paragraph::new(label_line),
            Rect::new(popup_area.x, popup_area.y, popup_area.width, 1),
        );
    }

    // Entries row
    if popup_area.height >= 2 {
        let mut spans = vec![Span::raw("  ")];
        for (i, (key_display, label, kind)) in state.menu_nodes.iter().enumerate() {
            match kind {
                MenuNodeKind::Separator => {
                    spans.push(Span::styled(
                        " \u{2502} ",
                        Style::default().fg(Color::Rgb(98, 114, 164)),
                    ));
                    continue;
                }
                MenuNodeKind::Label => {
                    if i > 0 {
                        spans.push(Span::raw("   "));
                    }
                    spans.push(Span::styled(
                        label.clone(),
                        Style::default().fg(Color::Rgb(98, 114, 164)),
                    ));
                    continue;
                }
                _ => {}
            }

            if i > 0 && !matches!(
                state.menu_nodes.get(i - 1).map(|(_, _, k)| k),
                Some(MenuNodeKind::Separator) | Some(MenuNodeKind::Label)
            ) {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(
                key_display.clone(),
                Style::default()
                    .fg(Color::Rgb(189, 147, 249))
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            if *kind == MenuNodeKind::Submenu {
                spans.push(Span::styled(
                    format!("{} \u{25b8}", label),
                    Style::default().fg(Color::Rgb(139, 233, 253)),
                ));
            } else {
                spans.push(Span::styled(
                    label.clone(),
                    Style::default().fg(Color::Rgb(204, 204, 204)),
                ));
            }
        }
        let entries_line = Line::from(spans);
        frame.render_widget(
            Paragraph::new(entries_line),
            Rect::new(popup_area.x, popup_area.y + 1, popup_area.width, 1),
        );
    }
}
```

- [ ] **Step 4: Build and run all tests**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -10`
Expected: compiles and all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/view.rs src/menu.rs
git commit -m "feat: menu rendering supports key sequences, separators, labels"
```

---

### Task 7: Config Parsing for Keybindings and Menu

**Files:**
- Modify: `src/config.rs`
- Create: `tests/config_test.rs`

- [ ] **Step 1: Write failing tests for config parsing**

Create `tests/config_test.rs`:

```rust
use sketch::config::Config;
use sketch::theme::ThemeName;

fn parse_config(content: &str) -> Config {
    Config::load_from_str(content).unwrap()
}

#[test]
fn test_empty_config() {
    let config = parse_config("");
    assert_eq!(config.max_line_width, 80);
    assert_eq!(config.theme, ThemeName::Dracula);
    assert!(config.keybinds.is_none());
    assert!(config.menu.is_none());
}

#[test]
fn test_keybindings_basic() {
    let config = parse_config(r#"
        keybindings {
            "ctrl-d" "half-page-down"
            "g g" "goto-top"
        }
    "#);
    let kb = config.keybinds.unwrap();
    assert!(!kb.reset_defaults);
    assert_eq!(kb.bindings.len(), 2);
    assert_eq!(kb.bindings[0].1, "half-page-down");
    assert_eq!(kb.bindings[1].1, "goto-top");
}

#[test]
fn test_keybindings_reset_defaults() {
    let config = parse_config(r#"
        keybindings {
            reset-defaults true
            "j" "move-down"
        }
    "#);
    let kb = config.keybinds.unwrap();
    assert!(kb.reset_defaults);
    assert_eq!(kb.bindings.len(), 1);
}

#[test]
fn test_keybindings_command_with_args() {
    let config = parse_config(r#"
        keybindings {
            "ctrl-k h" ":goto-heading 2"
        }
    "#);
    let kb = config.keybinds.unwrap();
    assert_eq!(kb.bindings[0].1, ":goto-heading 2");
}

#[test]
fn test_menu_basic() {
    let config = parse_config(r#"
        menu {
            entry key="f" label="file browser" action="file-browser"
            entry key="q" label="quit" action="quit"
        }
    "#);
    let menu = config.menu.unwrap();
    assert!(!menu.reset_defaults);
    assert_eq!(menu.nodes.len(), 2);
}

#[test]
fn test_menu_with_submenu() {
    let config = parse_config(r#"
        menu {
            submenu key="g" label="goto" {
                entry key="g" label="top" action="goto-top"
                entry key="e" label="bottom" action="goto-bottom"
            }
        }
    "#);
    let menu = config.menu.unwrap();
    assert_eq!(menu.nodes.len(), 1);
}

#[test]
fn test_menu_separator_and_label() {
    let config = parse_config(r#"
        menu {
            entry key="f" label="files" action="file-browser"
            separator
            label "Navigation"
            entry key="g" label="goto top" action="goto-top"
        }
    "#);
    let menu = config.menu.unwrap();
    assert_eq!(menu.nodes.len(), 4);
}

#[test]
fn test_menu_reset_defaults() {
    let config = parse_config(r#"
        menu {
            reset-defaults true
            entry key="q" label="quit" action="quit"
        }
    "#);
    let menu = config.menu.unwrap();
    assert!(menu.reset_defaults);
}

#[test]
fn test_invalid_key_sequence_fails() {
    let result = Config::load_from_str(r#"
        keybindings {
            "ctrl-" "half-page-down"
        }
    "#);
    assert!(result.is_err());
}

#[test]
fn test_unknown_command_fails() {
    let result = Config::load_from_str(r#"
        keybindings {
            "ctrl-d" "nonexistent-command"
        }
    "#);
    assert!(result.is_err());
}

#[test]
fn test_missing_menu_entry_action_fails() {
    let result = Config::load_from_str(r#"
        menu {
            entry key="f" label="file browser"
        }
    "#);
    assert!(result.is_err());
}

#[test]
fn test_unknown_theme_fails() {
    let result = Config::load_from_str(r#"
        theme "nonexistent"
    "#);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test config_test 2>&1 | tail -5`
Expected: compilation error — `load_from_str`, `keybinds`, `menu` fields don't exist

- [ ] **Step 3: Implement config parsing**

Replace `src/config.rs`:

```rust
use std::fmt;
use std::path::PathBuf;

use crate::command::CommandRegistry;
use crate::keys::{self, KeyPress};
use crate::menu::MenuNode;
use crate::theme::ThemeName;

#[derive(Debug)]
pub struct ConfigError {
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Debug)]
pub struct KeybindConfig {
    pub reset_defaults: bool,
    /// Pairs of (parsed key sequence, command string).
    pub bindings: Vec<(Vec<KeyPress>, String)>,
}

#[derive(Debug)]
pub struct MenuConfig {
    pub reset_defaults: bool,
    pub nodes: Vec<MenuNode>,
}

pub struct Config {
    pub max_line_width: usize,
    pub theme: ThemeName,
    pub keybinds: Option<KeybindConfig>,
    pub menu: Option<MenuConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_line_width: 80,
            theme: ThemeName::default(),
            keybinds: None,
            menu: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path();
        match path {
            Some(p) if p.exists() => Self::load_from_file(&p),
            _ => Ok(Self::default()),
        }
    }

    pub fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let doc: kdl::KdlDocument = content.parse().map_err(|e: kdl::KdlError| ConfigError {
            message: format!("invalid KDL syntax: {}", e),
        })?;
        Self::parse_document(&doc)
    }

    fn load_from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError {
            message: format!("{}: {}", path.display(), e),
        })?;

        let doc: kdl::KdlDocument = content.parse().map_err(|e: kdl::KdlError| ConfigError {
            message: format!("{}: invalid KDL: {}", path.display(), e),
        })?;

        Self::parse_document(&doc).map_err(|e| ConfigError {
            message: format!("{}: {}", path.display(), e.message),
        })
    }

    fn parse_document(doc: &kdl::KdlDocument) -> Result<Self, ConfigError> {
        let registry = CommandRegistry::default_registry();
        let mut config = Self::default();

        // Display settings
        if let Some(display) = doc.get("display")
            && let Some(children) = display.children()
            && let Some(node) = children.get("max-line-width")
            && let Some(val) = node.get(0).and_then(|v| v.as_integer())
        {
            config.max_line_width = val as usize;
        }

        // Theme
        if let Some(node) = doc.get("theme")
            && let Some(val) = node.get(0).and_then(|v| v.as_string())
        {
            config.theme = ThemeName::from_str(val).ok_or_else(|| ConfigError {
                message: format!("unknown theme \"{}\"", val),
            })?;
        }

        // Keybindings
        if let Some(kb_node) = doc.get("keybindings") {
            config.keybinds = Some(Self::parse_keybindings(kb_node, &registry)?);
        }

        // Menu
        if let Some(menu_node) = doc.get("menu") {
            config.menu = Some(Self::parse_menu(menu_node, &registry)?);
        }

        Ok(config)
    }

    fn parse_keybindings(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<KeybindConfig, ConfigError> {
        let children = node.children().ok_or_else(|| ConfigError {
            message: "keybindings block requires children".into(),
        })?;

        let mut reset_defaults = false;
        let mut bindings = Vec::new();

        for child in children.nodes() {
            let name = child.name().to_string();

            if name == "reset-defaults" {
                if let Some(val) = child.get(0).and_then(|v| v.as_bool()) {
                    reset_defaults = val;
                }
                continue;
            }

            // Node name is the key sequence, first arg is the command
            let key_seq = keys::parse_key_sequence(&name).map_err(|e| ConfigError {
                message: format!("invalid key sequence \"{}\": {}", name, e),
            })?;

            let cmd_string = child
                .get(0)
                .and_then(|v| v.as_string())
                .ok_or_else(|| ConfigError {
                    message: format!(
                        "keybinding \"{}\" missing command string",
                        name
                    ),
                })?
                .to_string();

            // Validate command name (strip : prefix for validation)
            let cmd_name = cmd_string.strip_prefix(':').unwrap_or(&cmd_string);
            let cmd_name = cmd_name.split_whitespace().next().unwrap_or(cmd_name);
            if registry.lookup(cmd_name).is_none() {
                return Err(ConfigError {
                    message: format!(
                        "unknown command \"{}\" in keybinding \"{}\"",
                        cmd_name, name
                    ),
                });
            }

            bindings.push((key_seq, cmd_string));
        }

        Ok(KeybindConfig {
            reset_defaults,
            bindings,
        })
    }

    fn parse_menu(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<MenuConfig, ConfigError> {
        let children = node.children().ok_or_else(|| ConfigError {
            message: "menu block requires children".into(),
        })?;

        let mut reset_defaults = false;
        let mut nodes = Vec::new();

        for child in children.nodes() {
            let name = child.name().to_string();

            if name == "reset-defaults" {
                if let Some(val) = child.get(0).and_then(|v| v.as_bool()) {
                    reset_defaults = val;
                }
                continue;
            }

            let menu_node = Self::parse_menu_node(child, registry)?;
            nodes.push(menu_node);
        }

        Ok(MenuConfig {
            reset_defaults,
            nodes,
        })
    }

    fn parse_menu_node(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<MenuNode, ConfigError> {
        let name = node.name().to_string();

        match name.as_str() {
            "separator" => Ok(MenuNode::separator()),
            "label" => {
                let text = node
                    .get(0)
                    .and_then(|v| v.as_string())
                    .unwrap_or("---")
                    .to_string();
                Ok(MenuNode::label(&text))
            }
            "entry" => {
                let key_str = node
                    .get("key")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"key\"".into(),
                    })?;
                let label = node
                    .get("label")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"label\"".into(),
                    })?;
                let action = node
                    .get("action")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"action\"".into(),
                    })?;

                // Validate key
                let key = keys::parse_key_sequence(key_str).map_err(|e| ConfigError {
                    message: format!("invalid key \"{}\" in menu entry: {}", key_str, e),
                })?;

                // Validate command
                let cmd_name = action.strip_prefix(':').unwrap_or(action);
                let cmd_name = cmd_name.split_whitespace().next().unwrap_or(cmd_name);
                if registry.lookup(cmd_name).is_none() {
                    return Err(ConfigError {
                        message: format!("unknown command \"{}\" in menu entry", cmd_name),
                    });
                }

                Ok(MenuNode {
                    key,
                    label: label.into(),
                    action: crate::menu::MenuAction::Command(action.into()),
                })
            }
            "submenu" => {
                let key_str = node
                    .get("key")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "submenu missing required attribute \"key\"".into(),
                    })?;
                let label = node
                    .get("label")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "submenu missing required attribute \"label\"".into(),
                    })?;

                let key = keys::parse_key_sequence(key_str).map_err(|e| ConfigError {
                    message: format!("invalid key \"{}\" in submenu: {}", key_str, e),
                })?;

                let mut children = Vec::new();
                if let Some(child_doc) = node.children() {
                    for child in child_doc.nodes() {
                        children.push(Self::parse_menu_node(child, registry)?);
                    }
                }

                Ok(MenuNode {
                    key,
                    label: label.into(),
                    action: crate::menu::MenuAction::Submenu(children),
                })
            }
            _ => Err(ConfigError {
                message: format!("unknown menu node type \"{}\"", name),
            }),
        }
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SKETCH_CONFIG") {
        return Some(PathBuf::from(p));
    }

    // Check ~/.config/sketch/config.kdl first (XDG-style), then platform default.
    if let Some(home) = dirs::home_dir() {
        let xdg = home.join(".config").join("sketch").join("config.kdl");
        if xdg.exists() {
            return Some(xdg);
        }
    }

    dirs::config_dir().map(|d| d.join("sketch").join("config.kdl"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config_test 2>&1 | tail -10`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_test.rs
git commit -m "feat: config parsing for keybindings and menu sections with validation"
```

---

### Task 8: Wire Config Into App Startup

**Files:**
- Modify: `src/app.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Update `main.rs` for fail-hard config loading**

In `src/main.rs`, change `Config::load()` handling:

```rust
    let mut config = match sketch::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };
```

Also update the theme override section — `config.theme` assignment already works.

- [ ] **Step 2: Update `App::new` to apply keybind and menu config**

In `src/app.rs`, update the `new` method:

```rust
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::from_name(config.theme);
        let syntect_theme = theme.name.syntect_theme();
        let editor = Editor::new(markdown, std::path::PathBuf::from(&filename));
        let viewport = Viewport::new(config.max_line_width);
        let registry = CommandRegistry::default_registry();

        // Build keybinds from config
        let keybinds = if let Some(kb_config) = &config.keybinds {
            let mut mgr = if kb_config.reset_defaults {
                KeybindManager::new(
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                )
            } else {
                KeybindManager::default()
            };
            mgr.apply_bindings(&kb_config.bindings);
            mgr
        } else {
            KeybindManager::default()
        };

        // Build menu from config
        let menu_tree = if let Some(menu_config) = &config.menu {
            if menu_config.reset_defaults {
                menu_config.nodes.clone()
            } else {
                merge_menu(menu::default_menu(), &menu_config.nodes)
            }
        } else {
            menu::default_menu()
        };

        Self {
            editor,
            viewport,
            theme,
            keybinds,
            registry,
            should_quit: false,
            search_query: String::new(),
            search_input_mode: false,
            search_input_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            mode: AppMode::Normal,
            view_mode: ViewMode::Rendered,
            menu_state: MenuState::new(),
            menu_tree,
            file_browser: None,
            command_buffer: String::new(),
            command_error: String::new(),
            highlighter: Highlighter::with_syntect_theme(syntect_theme),
            rendered_cache: Vec::new(),
            view_cache_dirty: true,
        }
    }
```

Add the `apply_bindings` method to `KeybindManager` in `src/keybind.rs`:

```rust
    /// Apply config-defined bindings on top of existing ones.
    pub fn apply_bindings(&mut self, bindings: &[(Vec<KeyPress>, String)]) {
        for (key_seq, cmd) in bindings {
            if key_seq.len() == 1 {
                self.single.insert(key_seq[0].clone(), cmd.clone());
            } else {
                self.multi.insert(key_seq.clone(), cmd.clone());
            }
        }
    }
```

Add the `merge_menu` function at the bottom of `src/app.rs`:

```rust
/// Merge user menu nodes on top of defaults.
/// User entries with the same key at the same level replace the default entry.
/// New entries are appended.
fn merge_menu(mut defaults: Vec<MenuNode>, user_nodes: &[MenuNode]) -> Vec<MenuNode> {
    for user_node in user_nodes {
        if user_node.key.is_empty() {
            // Separator or label — just append
            defaults.push(user_node.clone());
            continue;
        }
        // Find matching default entry by key
        if let Some(pos) = defaults.iter().position(|d| d.key == user_node.key) {
            defaults[pos] = user_node.clone();
        } else {
            defaults.push(user_node.clone());
        }
    }
    defaults
}
```

- [ ] **Step 3: Build and run all tests**

Run: `cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -10`
Expected: compiles and all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/main.rs src/keybind.rs
git commit -m "feat: wire config keybindings and menu into app startup"
```

---

### Task 9: Integration Test — Full Config Round-Trip

**Files:**
- Add to: `tests/config_test.rs`

- [ ] **Step 1: Write an integration test that builds a KeybindManager from config**

Add to `tests/config_test.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::keybind::KeybindManager;

#[test]
fn test_config_keybinds_override_default() {
    let config = Config::load_from_str(r#"
        keybindings {
            "j" "scroll-up"
        }
    "#).unwrap();

    let kb = config.keybinds.unwrap();
    let mut mgr = KeybindManager::default();
    mgr.apply_bindings(&kb.bindings);

    // j should now be scroll-up instead of move-down
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some("scroll-up".to_string()));

    // k should still be the default
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    assert_eq!(result, Some("move-up".to_string()));
}

#[test]
fn test_config_keybinds_reset_defaults() {
    let config = Config::load_from_str(r#"
        keybindings {
            reset-defaults true
            "j" "move-down"
        }
    "#).unwrap();

    let kb = config.keybinds.unwrap();
    let mut mgr = if kb.reset_defaults {
        KeybindManager::new(
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    } else {
        KeybindManager::default()
    };
    mgr.apply_bindings(&kb.bindings);

    // j works
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some("move-down".to_string()));

    // k should NOT work (defaults were reset)
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    assert_eq!(result, None);
}

#[test]
fn test_config_full_round_trip() {
    let config = Config::load_from_str(r#"
        theme "nightfox"

        display {
            max-line-width 120
        }

        keybindings {
            "ctrl-d" "half-page-down"
            "g g" "goto-top"
        }

        menu {
            entry key="f" label="files" action="file-browser"
            submenu key="g" label="goto" {
                entry key="g" label="top" action="goto-top"
            }
        }
    "#).unwrap();

    assert_eq!(config.theme, ThemeName::Nightfox);
    assert_eq!(config.max_line_width, 120);
    assert!(config.keybinds.is_some());
    assert!(config.menu.is_some());
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --test config_test 2>&1 | tail -10`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add tests/config_test.rs
git commit -m "test: add integration tests for config round-trip"
```

---

### Task 10: Final Build Verification and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1`
Expected: all tests pass with no warnings

- [ ] **Step 2: Run clippy**

Run: `cargo clippy 2>&1`
Expected: no errors (warnings OK)

- [ ] **Step 3: Fix any clippy warnings or test failures found**

Address issues if any.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "chore: clippy fixes and cleanup"
```
