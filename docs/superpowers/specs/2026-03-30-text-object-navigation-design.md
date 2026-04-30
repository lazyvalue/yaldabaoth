# Text Object Navigation Design

## Overview

Add semantic cursor navigation to Rendered mode. Instead of moving character-by-character, the user can switch to a navigation mode that moves the cursor between document objects: links, headings, list items, or code blocks. The cursor highlights the full object span, and Enter performs a context-dependent action.

## Navigation Modes

Five modes:

```rust
enum NavMode {
    Character,   // default — current behavior, single-char cursor
    Link,        // move between links
    Heading,     // move between headings
    ListItem,    // move between list items
    CodeBlock,   // move between code blocks
}
```

Stored per-buffer in `Buffer` (like `rendered_cursor_row`). When in a non-Character mode, j/k snap to objects of that type. On entering a mode, the cursor jumps to the nearest object (closest to current `rendered_cursor_row`).

## Entry Methods

Three ways to enter a navigation mode:

**Cycle key:** `m` cycles Character → Link → Heading → ListItem → CodeBlock → Character.

**Direct entry (multi-key):**
- `gl` — Link mode
- `gh` — Heading mode
- `gi` — List Item mode
- `gc` — Code Block mode

**Commands:** `:nav links`, `:nav headings`, `:nav list-items`, `:nav code-blocks`, `:nav character`

**Exit:** Esc in any non-Character mode returns to Character mode. Cursor stays where it is.

## Object Discovery

The app builds a list of navigable objects from `rendered_cache` using `view::render_block_to_lines` — the same function the view uses for drawing. This ensures coordinates agree (no document-to-view translation).

```rust
struct NavObject {
    rendered_row: usize,
    col_start: usize,
    col_end: usize,
    kind: NavMode,
    action_data: String,  // link URL, code block text, or empty
}
```

Objects are discovered by examining rendered blocks:
- **Links**: spans with `link.is_some()` — one NavObject per link span
- **Headings**: `RenderedBlock::Heading` — one NavObject spanning the heading text
- **List items**: `RenderedBlock::List` — one NavObject per item's first line
- **Code blocks**: `RenderedBlock::CodeBlock` — one NavObject spanning the first line of each block

The object list is rebuilt when the render cache is rebuilt or when entering a nav mode. Stored per-buffer as `nav_objects: Vec<NavObject>` with `nav_object_index: usize` tracking the selected object.

## Cursor Movement

When `NavMode` is not Character:

**j** — next object of current type (wrapping at end). Viewport scrolls to follow.

**k** — previous object (wrapping at start).

**h/l** — for Links and ListItems where multiple can exist on the same row or nearby, h/l moves between them laterally. For Headings and CodeBlocks, h/l are no-ops.

**Enter** — context-dependent action:
- Link: open it (local file → new buffer, URL → system browser)
- Heading: scroll to content below the heading, return to Character mode
- ListItem: toggle checkbox if task list, otherwise no-op
- CodeBlock: yank entire block to clipboard

**Esc** — return to Character mode, cursor stays where it is.

## Visual

The cursor in object mode highlights the full object span (`col_start..col_end`), not a single character. Only the selected object is highlighted — other objects of the same type are not visually marked.

## Top Bar Indicator

When in a non-Character mode, the top bar shows the mode name:

```
 src/app.rs                    line 42/500 12% [LINKS]
```

Uses the theme's mode_indicator style. Character mode shows nothing (current behavior).

## Keybindings and Commands

New Action variants: `NavLinks`, `NavHeadings`, `NavListItems`, `NavCodeBlocks`, `NavCycle`, `NavCharacter`.

Default keybindings:
- `m` — `nav-cycle`
- `gl` — `nav-links`
- `gh` — `nav-headings`
- `gi` — `nav-list-items`
- `gc` — `nav-code-blocks`

Commands:
| Command | Aliases | Action |
|---|---|---|
| `nav-cycle` | | `NavCycle` |
| `nav-links` | | `NavLinks` |
| `nav-headings` | | `NavHeadings` |
| `nav-list-items` | | `NavListItems` |
| `nav-code-blocks` | | `NavCodeBlocks` |
| `nav-character` | | `NavCharacter` |

Menu: Add submenu under space > n "navigate" with entries for each mode.

## Module Structure

**Modified files:**
- `src/buffer.rs` — Add `nav_mode: NavMode`, `nav_objects: Vec<NavObject>`, `nav_object_index: usize`. Method `rebuild_nav_objects(theme)` builds the list using `view::render_block_to_lines`.
- `src/app.rs` — Handle new actions, override j/k/h/l/Enter when nav_mode != Character, jump to nearest on mode entry.
- `src/view.rs` — Draw object highlight (full span) when in object mode. Show nav mode in top bar.
- `src/keybind.rs` — New Action variants, default bindings for `m`, `gl`, `gh`, `gi`, `gc`.
- `src/command.rs` — Register nav commands.
- `src/menu.rs` — Add navigate submenu.

**No new files.** `NavMode` and `NavObject` types go in `buffer.rs` since they're per-buffer state.
