# Editing Profiles Design

## Overview

A pluggable editing profile system that defines per-file-type editing behaviors: auto-close pairs, list/blockquote continuation on Enter, smart indentation, and closing character skip-over. Profiles are data-driven, loaded from KDL files, with built-in defaults for markdown and KDL. This is the first of three sub-projects (editing profiles → selection model → structural operations).

## EditProfile Struct

```rust
struct EditProfile {
    name: String,
    extensions: Vec<String>,
    auto_close_pairs: Vec<AutoClosePair>,
    continuation_rules: Vec<ContinuationRule>,
    indent: IndentConfig,
}

struct AutoClosePair {
    open: String,
    close: String,
    skip_if_before_word: bool,
}

struct ContinuationRule {
    /// Regex pattern matching the current line
    pattern: String,
    /// What to insert on the next line ($0 = repeat matched prefix, $next = increment number)
    continuation: String,
    /// If the current line matches this pattern (just the marker, no content), remove the marker instead
    empty_pattern: Option<String>,
}

struct IndentConfig {
    width: usize,
    use_tabs: bool,
    /// Patterns that trigger an extra indent on the next line
    increase_patterns: Vec<String>,
    /// Patterns that trigger a dedent on the current line
    decrease_patterns: Vec<String>,
}
```

Each `Buffer` gets a cloned `EditProfile` at open time, resolved by file extension.

## Built-in Profiles

**Markdown:**
- Auto-close pairs: `(/)`, `[/]`, `{/}`, `` `/` ``, `"/"`, `*/*`, `_/_`
- Continuation rules:
  - `^\s*[-*+] ` → continue with same marker (unordered list)
  - `^\s*(\d+)\. ` → continue with incremented number (ordered list)
  - `^\s*[-*+] \[[ x]\] ` → continue with `- [ ] ` (checkbox)
  - `^\s*> ` → continue with `> ` (blockquote)
  - Empty versions of each: if line is just the marker with no text, remove the marker
- Indent: 2 spaces, increase after `{`, decrease on `}`

**KDL:**
- Auto-close pairs: `(/)`, `{/}`, `"/"`
- No continuation rules
- Indent: 4 spaces, increase after `{`, decrease on `}`

**Default (fallback):**
- Auto-close pairs: `(/)`, `[/]`, `{/}`, `"/"`
- No continuation rules
- Indent: 4 spaces

## Profile Resolution and Registry

A `ProfileRegistry` is loaded at startup:

1. Load built-in profiles (markdown, kdl, default)
2. Scan `~/.config/sketch/profiles/*.kdl` for user profiles — override built-ins by name
3. Load extension mapping from `config.kdl` if present, otherwise use defaults from profiles

When a buffer is opened, the registry resolves the profile by file extension. If no match, use "default". Each buffer stores a cloned `EditProfile`.

Extension mapping in `config.kdl`:

```kdl
extensions {
    "mdx" "markdown"
    "txt" "default"
}
```

## Profile File Format

User profiles at `~/.config/sketch/profiles/<name>.kdl`:

```kdl
profile "markdown" {
    extensions "md" "mdx" "markdown"

    auto-close {
        pair open="(" close=")"
        pair open="[" close="]"
        pair open="{" close="}"
        pair open="`" close="`"
        pair open="\"" close="\""
        pair open="*" close="*"
        pair open="_" close="_"
    }

    continuation {
        rule pattern=r"^\s*[-*+] " continuation="$0" empty=r"^\s*[-*+] $"
        rule pattern=r"^\s*(\d+)\. " continuation="$next. " empty=r"^\s*\d+\. $"
        rule pattern=r"^\s*[-*+] \[[ x]\] " continuation="- [ ] " empty=r"^\s*[-*+] \[[ x]\] $"
        rule pattern=r"^\s*> " continuation="> " empty=r"^\s*> $"
    }

    indent {
        width 2
        use-tabs false
        increase r"\{$"
        decrease r"^\s*\}"
    }
}
```

Substitution tokens in `continuation`:
- `$0` — repeat the matched prefix
- `$next` — increment the captured number (for ordered lists)

## Integration with Editing Engine

Behaviors hook into three places:

**On character insert (insert mode):** After inserting a character, check auto-close pairs. If the character matches an `open`, insert the `close` after the cursor (cursor stays between). Skip if `skip_if_before_word` and next char is alphanumeric. If the typed character is a `close` and the character after the cursor is already that same `close` (from auto-close), skip over it instead of inserting a duplicate.

**On Enter (insert mode):** Before inserting the newline, check the current line against continuation rules in order. First matching rule wins:
- If the line matches `empty_pattern`, delete the marker text from the current line instead of continuing
- Otherwise, insert a new line prefixed with the `continuation` text
Also check indent rules: if the current line matches an `increase_pattern`, add one indent level to the new line.

**On Tab/Shift-Tab (normal mode):** Indent/outdent the current line by `indent.width` spaces (or a tab). In insert mode, Tab inserts spaces to the next indent stop.

## Module Structure

**New files:**
- `src/profile.rs` — `EditProfile`, `AutoClosePair`, `ContinuationRule`, `IndentConfig`, `ProfileRegistry`, built-in profiles, KDL profile parser
- `tests/profile_test.rs` — test profile loading, continuation matching, auto-close logic

**Modified files:**
- `src/buffer.rs` — Add `profile: EditProfile` field, set at buffer creation
- `src/app.rs` — Hold `ProfileRegistry` in App, pass profile to new buffers, integrate profile behaviors into `handle_insert_key` and normal mode indent
- `src/config.rs` — Parse `extensions` mapping from config.kdl
- `src/lib.rs` — Add `pub mod profile`

**Unchanged:** `view.rs`, `keybind.rs`, `command.rs`, `menu.rs`, `keys.rs`, `theme.rs`, `viewport.rs`
