# Command Registry & View Mode Toggle — Design Spec

## Overview

Unify the menu system, command bar, and keybindings around a single command registry. Replace the Obsidian-style per-block reveal with a simple global view mode toggle (rendered vs raw). Every command is reachable from both the Space menu and the `:` command bar.

## Command Registry

A single registry of all commands in the application. Each command has:

```rust
struct CommandDef {
    name: String,              // canonical name, used in ":" bar (e.g., "save", "quit", "toggle-view")
    aliases: Vec<String>,      // short forms (e.g., "w" for "save", "q" for "quit")
    action: Action,            // the Action enum variant it dispatches
    description: String,       // human-readable, shown in menu
}
```

The registry is a `Vec<CommandDef>` built at startup. All three input surfaces index into it:

- **Menu** (Space → key): the menu tree maps keys to command names. When a menu entry is selected, it looks up the command by name in the registry and dispatches its action.
- **Command bar** (`:name`): typed input is matched against command names and aliases. `:w` matches the "save" command's alias "w". `:toggle-view` matches by name.
- **Keybindings**: the existing keybind system maps keys to `Action` variants. Actions are the same enum used by the registry.

### Default Commands

| Name | Aliases | Menu Key | Action | Description |
|------|---------|----------|--------|-------------|
| save | w | Space → s | Save | Save file |
| save-as | | | SaveAs | Save to new path |
| quit | q | Space → q | Quit | Quit (warns if modified) |
| force-quit | q! | | ForceQuit | Quit without saving |
| save-quit | wq | | SaveQuit | Save and quit |
| toggle-view | | Space → v | ToggleView | Switch rendered/raw |
| file-browser | | Space → f | OpenFileBrowser | Open file browser |
| search | | Space → / | SearchForward | Search forward |
| goto-top | | Space → g → g | JumpTop | Go to top |
| goto-bottom | | Space → g → e | JumpBottom | Go to bottom |
| goto-heading | | Space → g → h | NextHeading | Next heading |

### Command Bar Parsing

When the user types `:` and enters a command:
1. Split input on first space: command name + optional args
2. Look up command by name or alias in the registry
3. If found, dispatch the action. If the action needs args (e.g., save-as needs a filename), pass them.
4. If not found, show "Unknown command: X" error

Special handling:
- `:w filename` → dispatches SaveAs with the filename as argument
- `:q!` → matched as alias "q!" for force-quit
- Unknown commands show an error in the command bar area

## View Mode Toggle

Replace the per-block Obsidian-style reveal with a global view mode.

```rust
enum ViewMode {
    Rendered,  // styled markdown output (read-only display)
    Raw,       // raw markdown text (editable)
}
```

### Behavior

- **Rendered mode**: the document displays as styled rendered blocks (existing viewer behavior). Cursor movement works (j/k scroll, {/} headings). Editing keys (`i`, `a`, `o`, `O`, `x`, `dd`) auto-switch to Raw mode before executing.
- **Raw mode**: the document displays as raw markdown text from the rope. Cursor is visible (block/beam). All editing works. This is the primary editing mode.
- **Space → v** (or `:toggle-view`): toggles between Rendered and Raw. The cursor position is preserved across toggles.
- On file open, the default mode is Rendered (viewer behavior).

### Rendering in each mode

- **Rendered mode**: same as before — pulldown-cmark + syntect produce styled `RenderedBlock`s. The cached render pipeline is used. No cursor is drawn (or a subtle line-highlight cursor for position awareness).
- **Raw mode**: the rope's text is displayed line by line with minimal styling. The cursor is drawn at `CursorPos`. Syntax could be lightly highlighted in the future, but for now raw text uses the paragraph style.

### Auto-switch to Raw

When the user presses an editing key (`i`, `a`, `o`, `O`, `x`, `dd`) in Rendered mode, the app automatically switches to Raw mode before executing the command. This means you never need to manually toggle — just start editing and the view switches. `:toggle-view` (Space → v) is for when you want to see the rendered output without editing.

## Architecture Changes

### New Module

- `src/command.rs` — `CommandDef`, `CommandRegistry`, default command list, lookup by name/alias

### Modified Modules

- `src/app.rs`:
  - Add `ViewMode` enum and `view_mode` field
  - Add `CommandRegistry` field
  - Remove per-block reveal logic (`get_view_blocks` no longer mixes Raw/Rendered)
  - In Rendered mode, build view blocks from cached `RenderedBlock`s (existing pipeline)
  - In Raw mode, build view blocks as raw lines from the rope
  - Command bar execution uses registry lookup instead of hardcoded match
  - Editing actions auto-switch to Raw mode
  - Space → v dispatches `ToggleView`

- `src/view.rs`:
  - Remove `ViewBlock::Raw` variant — in Raw mode, the entire view is raw lines; in Rendered mode, the entire view is `RenderedBlock`s
  - Simplify `draw_content` — either render all blocks styled or all lines raw
  - Cursor drawing only in Raw mode (or with subtle highlight in Rendered mode)

- `src/keybind.rs`:
  - Add new Action variants: `ToggleView`, `Save`, `SaveAs`, `ForceQuit`, `SaveQuit`
  - Remove `Quit` (replaced by command-based quit with modified check)

- `src/menu.rs`:
  - Menu entries reference command names instead of `Action` variants directly
  - `MenuAction::Command` changes from `Action` to `String` (command name)
  - Menu dispatches through the registry

### Removed

- `ViewBlock` enum (no longer needed — view is fully rendered or fully raw)
- Per-block active block detection and raw/rendered mixing
- `TreeState` usage for block boundary detection during rendering (tree-sitter is still used for future incremental re-rendering, but not for per-block reveal)

## Testing

- **Unit tests for `command.rs`**: Lookup by name, lookup by alias, unknown command returns None
- **Existing tests**: menu tests updated for command-name-based dispatch, keybind tests updated for new Action variants
- **Manual testing**: toggle between rendered/raw, auto-switch on edit, command bar commands

## Future Considerations

- Command autocompletion in the `:` bar
- Command arguments (`:goto 42` to jump to line 42)
- Command history (up/down arrow in command bar)
