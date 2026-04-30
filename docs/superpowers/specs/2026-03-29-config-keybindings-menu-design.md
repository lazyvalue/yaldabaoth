# Config: Keybindings and Command Menu Design

## Overview

Substantially expand the `config.kdl` configuration file to support user-defined keybindings and command menu trees. Both systems share a unified key notation parser that handles modifier keys and multi-key sequences.

## Key Notation

### Grammar

A **key sequence** is one or more **key combos** separated by spaces.

A **key combo** is zero or more modifiers followed by a key name, all dash-separated.

Examples:
- `ctrl-d` — Ctrl+D
- `g g` — press G, then G
- `ctrl-shift-k h` — press Ctrl+Shift+K, then H
- `space` — spacebar
- `K` and `shift-k` are equivalent

### Modifiers

`ctrl`, `alt`, `shift` — case-insensitive.

### Key Names

- Single characters: `a`-`z`, `0`-`9`, symbols
- Named keys: `space`, `enter`, `tab`, `esc`, `backspace`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `f1`-`f12`

### API

New module `src/keys.rs`:

```rust
pub fn parse_key_sequence(input: &str) -> Result<Vec<KeyPress>, KeyParseError>
pub fn format_key_sequence(keys: &[KeyPress]) -> String
```

`KeyParseError` includes position and reason for the fail-hard config behavior.

## Config File Schema

```kdl
display {
    max-line-width 80
}

theme "nightfox"

keybindings {
    reset-defaults false

    "ctrl-d" "half-page-down"
    "g g" "goto-top"
    "ctrl-k h" ":goto-heading 2"
    "shift-j" "scroll-down"
}

menu {
    reset-defaults false

    entry key="f" label="file browser" action="file-browser"
    entry key="/" label="search" action="search"
    separator
    submenu key="g" label="goto" {
        entry key="g" label="top" action="goto-top"
        entry key="h" label="next heading" action=":goto-heading"
        separator
        entry key="ctrl-h" label="prev heading" action="prev-heading"
    }
    entry key="q" label="quit" action="quit"
}
```

### Keybinding Nodes

The node name is the key sequence string, the first argument is the command string. Commands starting with `:` are treated as command-line invocations with arguments (e.g., `:goto-heading 2`). Otherwise it's a bare command name looked up in the registry.

### Menu Node Types

- `entry` — leaf item with `key`, `label`, `action` attributes
- `submenu` — has `key`, `label`, and children block
- `separator` — visual divider, no attributes
- `label` — non-interactive heading text (e.g., `label "Navigation"`)

### `reset-defaults`

- `false` (default): user entries merge on top of built-in defaults. User bindings override conflicts; defaults fill the rest.
- `true`: only user-defined entries exist.

**Menu merge strategy** (when `reset-defaults=false`): User entries are appended to defaults. If a user entry has the same key as a default entry at the same menu level, the user entry replaces it. Submenus matched by key replace the default's children entirely (no deep merge).

## Keybinding Integration

### Action Resolution

`KeybindManager` maps key sequences to `String` (command strings) instead of `Action` enum variants. When a keybind fires, the command string is passed to `CommandRegistry` for resolution. The registry returns an `Action` plus optional argument string.

This decouples `KeybindManager` from `Action` — it maps keys to command strings. The app layer does the resolution.

### Default Bindings

The current hardcoded defaults in `KeybindManager::default()` are preserved but expressed as command strings internally (e.g., `key('j')` maps to `"scroll-down"` instead of `Action::ScrollDown`).

### Config Loading

1. If `reset-defaults` is `true`, start with empty maps
2. If `false`, start with `KeybindManager::default()` (using command strings)
3. For each user keybinding, parse the key sequence and insert — single-key sequences go in `single`, multi-key go in `multi`, overriding any existing entry for that key

## Menu Integration

### MenuNode Changes

`MenuNode.key` changes from `char` to `Vec<KeyPress>` to support modifier combos and sequences.

```rust
pub enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(String),
    Separator,
    Label(String),
}
```

### Key Processing

`MenuState` uses a shared `KeySequenceMatcher` (extracted from `KeybindManager`) that handles the pending-key-with-timeout pattern. Both `KeybindManager` and `MenuState` use this shared component.

### Menu Rendering

`draw_menu_popup` displays formatted key sequences via `format_key_sequence`. Separators render as visual dividers. Labels render as non-interactive headings.

## Command Registry Expansion

Every `Action` variant gets a command name so the string-based keybind system works. New commands:

| Command | Action |
|---|---|
| `scroll-down` | `ScrollDown` |
| `scroll-up` | `ScrollUp` |
| `half-page-down` | `HalfPageDown` |
| `half-page-up` | `HalfPageUp` |
| `full-page-down` | `FullPageDown` |
| `full-page-up` | `FullPageUp` |
| `next-heading` | `NextHeading` |
| `prev-heading` | `PrevHeading` |
| `next-heading-same-level` | `NextHeadingSameLevel` |
| `prev-heading-same-level` | `PrevHeadingSameLevel` |
| `search-backward` | `SearchBackward` |
| `search-next` | `SearchNext` |
| `search-prev` | `SearchPrev` |
| `toggle-view` | `ToggleView` |
| `open-link` | `OpenLink` |
| `yank-line` | `YankLine` |
| `open-menu` | `OpenMenu` |
| `move-left` | `MoveLeft` |
| `move-right` | `MoveRight` |
| `move-up` | `MoveUp` |
| `move-down` | `MoveDown` |
| `move-word-forward` | `MoveWordForward` |
| `move-word-backward` | `MoveWordBackward` |
| `move-word-end` | `MoveWordEnd` |
| `move-line-start` | `MoveLineStart` |
| `move-line-end` | `MoveLineEnd` |
| `insert-mode` | `InsertMode` |
| `insert-after` | `InsertAfter` |
| `open-line-below` | `OpenLineBelow` |
| `open-line-above` | `OpenLineAbove` |
| `delete-char` | `DeleteChar` |
| `delete-line` | `DeleteLine` |
| `undo` | `Undo` |
| `redo` | `Redo` |
| `enter-command` | `EnterCommand` |

Existing commands (`save`, `quit`, `goto-top`, `goto-bottom`, `goto-heading`, `file-browser`, `search`, `save-as`, `force-quit`, `save-quit`) remain with their current names and aliases.

### Command Argument Parsing

`CommandRegistry` gains a `resolve` method that takes a full command string, splits into command name + args, looks up the command, and returns `(Action, Option<String>)`. For example `:goto-heading 2` resolves to `(NextHeading, Some("2"))`. The app layer decides what to do with the argument.

## Error Handling

All config validation happens in `Config::load()` at startup, before the TUI initializes. Any error prints to stderr and exits with code 1.

### Error Format

- **KDL parse error**: `config.kdl:5: invalid KDL syntax: expected node terminator`
- **Bad key notation**: `config.kdl:8: invalid key sequence "ctrl-shift-": missing key name after modifier`
- **Unknown command**: `config.kdl:12: unknown command "foobar" in keybinding "ctrl-k"`
- **Missing attribute**: `config.kdl:15: menu entry missing required attribute "action"`
- **Invalid menu structure**: `config.kdl:18: separator cannot have children`

Each error includes the config file path and context. Only the first error is shown.

When a command string starts with `:`, only the command name (before the first space) is validated against the registry. Arguments are passed through without validation — the command handler decides if they're valid at runtime.

## Module Structure

### New Files

- `src/keys.rs` — key notation parser and formatter

### Modified Files

- `src/keybind.rs` — `KeyPress` becomes public. `KeybindManager` maps to `String`. `KeySequenceMatcher` extracted as shared logic.
- `src/menu.rs` — `MenuNode.key` becomes `Vec<KeyPress>`. New `Separator` and `Label` variants. `MenuState` uses `KeySequenceMatcher`.
- `src/command.rs` — All actions registered. New `resolve` method for command strings with arguments.
- `src/config.rs` — Parse `keybindings` and `menu` sections. Structured errors. `Config` gains keybind and menu config fields.
- `src/app.rs` — Build `KeybindManager` and menu tree from config. Route keybind results through command registry.
- `src/view.rs` — Menu rendering updated for formatted key sequences, separators, and labels.
- `src/lib.rs` — Add `pub mod keys`.

### Unchanged

`blocks.rs`, `cursor.rs`, `document.rs`, `editor.rs`, `file_browser.rs`, `highlight.rs`, `render.rs`, `theme.rs`, `viewport.rs`, `main.rs`.
