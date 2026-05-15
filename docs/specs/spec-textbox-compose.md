# Textbox Compose Mode

**Status:** SHIPPED.

**Last updated:** 2026-05-14

## Builds On

- **claude.rs** (`src/app/claude.rs`) -- the splice algorithm (`append_to_claude_buffer`), draft preservation, `lock_active_turn`, and `extract_editable_inserts`. The textbox replaces inline editing as the primary authoring surface; on toggle-off its contents feed into the same `programmatic_insert` path that the splice algorithm uses, and on send it invokes the same lock-and-send flow.
- **editor.rs** (`src/editor.rs`) -- `Editor` struct, `Document`, `frozen_lines`, `programmatic_insert`. The textbox is a standalone `Editor` instance; its text is inserted into the main claude buffer's `Editor` via `programmatic_insert` when the textbox closes.
- **view.rs** (`src/view.rs`) -- layout chunks and `draw` function. The textbox occupies a new layout slot between the content area and the bottom bar, requiring a new constraint in the vertical layout.
- **keybind.rs** (`src/keybind.rs`) -- `Action` enum and `KeybindManager`. New actions are added for toggling and sending from the textbox.

## Overview

The *claude* buffer's inline editing model works but has an ergonomic problem: while Claude is streaming a reply, new content pushes the viewport down and the user's draft insertion point shifts. The splice algorithm preserves draft text, but the visual experience is disorienting -- the user's editing context jumps with every chunk.

Textbox Compose Mode solves this by providing a **stable, fixed-position editing surface** at the bottom of the screen. The main buffer continues receiving streamed content, but the viewport freezes so the user can compose in peace. When done, the textbox contents transfer to the main buffer (same splice point, same frozen semantics) and the textbox disappears.

The feature introduces three named artifacts:

- **ComposeTextbox** -- a mini-buffer (standalone `Editor` instance) for composing messages.
- **Action::ComposeToggle** -- action to open/close the textbox.
- **Action::ComposeSend** -- action to send textbox contents and close.

## Behaviors

### Activation

1. `ComposeToggle` is only available when the active buffer is `*claude*`. In any other buffer the action is a no-op (status bar shows "Compose only available in *claude* buffer").
2. On toggle-on, a `ComposeTextbox` is created (fresh `Editor` with an empty `Document`). The cursor moves into the textbox. `AppMode` transitions to `Insert` (the user wants to type immediately).
3. The main buffer's current draft text (everything past the frozen/locked boundary) is **left in place** in the main buffer. The textbox starts empty. This avoids lossy round-tripping of partially-typed inline content.

### Editing

4. While the textbox is active, all Normal-mode and Insert-mode key dispatch targets the textbox's `Editor`, not the main buffer's. The main buffer is read-only -- no cursor movement, no edits.
5. Standard vim modal editing applies: `Esc` returns to Normal (within the textbox), `i`/`a`/`o` enter Insert, motions work on the textbox document. `Enter` in Insert mode inserts a newline -- it does not send.
6. Undo/redo operates on the textbox's own undo stack.

### Main buffer behavior while textbox is active

7. `pump_acp_replies` and `pump_claude_replies` continue appending streamed chunks to the main `*claude*` buffer via `append_to_claude_buffer`. The splice algorithm is unaffected -- it works on the main buffer's `Editor`, not the textbox.
8. Auto-scroll suppression: while `compose_textbox` is `Some`, the `ensure_buffer_cursor_visible` call at the end of `pump_acp_replies` / `pump_claude_replies` is skipped for the `*claude*` buffer. The viewport stays where the user left it.
9. The user can scroll the main buffer's viewport while in the textbox via `Ctrl-Up` / `Ctrl-Down` (mapped to scroll-up / scroll-down on the main buffer). These do not move the textbox cursor.

### Toggle off (without sending)

10. The textbox's full text is extracted (`compose_textbox.editor.document().full_text()`).
11. If non-empty, the text is appended to the main buffer **at EOF** via `programmatic_insert`. This puts the compose contents *after* any existing draft so the user sees their just-typed compose text where they expect it (at the bottom). The send flow (`extract_editable_inserts`) picks up the entire editable range — both prior draft and new compose text — as one prompt, so order in the prompt is `<existing draft>\n<compose text>`. (Earlier drafts of this spec said "splice point (end of frozen content)" — that wording put compose text *before* the draft, which read as a UX regression in practice. Implementation went with EOF; this spec follows the implementation.)
12. The cursor in the main buffer moves to the end of the just-inserted text.
13. Auto-scroll suppression is lifted -- the next tick's `ensure_buffer_cursor_visible` will scroll the main buffer to show the cursor.
14. `compose_textbox` is set to `None`. `AppMode` stays `Normal` (the user was in Normal mode when they toggled off, since toggle-off is a Normal-mode action).
15. If the textbox is empty on toggle-off, nothing is inserted -- the textbox simply disappears.

### Sending

16. `ComposeSend` (triggered from Normal or Insert mode within the textbox):
    - Extract the textbox text.
    - If empty, show "Nothing to send" in the status bar and do nothing.
    - Toggle off (steps 10-14 above -- insert text into main buffer).
    - Call the active channel's send flow (`acp_send_buffer` or `claude_send_buffer`). Because the text was just inserted into the main buffer as editable content, `extract_editable_inserts` will pick it up.
    - `lock_active_turn` runs as part of the send flow, locking the just-sent content.

### Edge cases

17. If the user switches buffers while the textbox is active (`NextBuffer`/`PrevBuffer`), the textbox is toggled off (contents appended to main buffer) before the buffer switch occurs. The textbox is not carried across buffers.
18. If the user runs `:q` or `:q!` while the textbox is active, toggle-off occurs first (contents preserved in the buffer, which is not saved to disk anyway since `*claude*` is a virtual buffer).
19. If a channel disconnects while the textbox is active, the textbox remains open. The user can still toggle off (appending to the buffer) or attempt to send (which will fail with the normal channel-error message).

## Data Model

### ComposeTextbox

```rust
pub(super) struct ComposeTextbox {
    /// Standalone editor instance with its own Document, cursor, and undo stack.
    pub editor: Editor,
    /// The textbox's own modal state: Normal or Insert.
    /// Tracked separately from App::mode because the main buffer's mode
    /// is irrelevant while the textbox is active.
    pub mode: AppMode,
}
```

Stored as `Option<ComposeTextbox>` on `App`. `None` means the textbox is closed.

The `Editor` is constructed with an empty document and a synthetic file path (e.g., `PathBuf::from("*compose*")`). It has no frozen lines, no lockable prefix -- it is a plain text editor.

### App fields

```rust
pub struct App {
    // ... existing fields ...

    /// Optional compose textbox for the *claude* buffer. When Some,
    /// key dispatch and rendering target the textbox instead of the
    /// main buffer.
    pub(super) compose_textbox: Option<ComposeTextbox>,
}
```

## Interfaces

- **`ComposeTextbox::new() -> Self`** -- create a fresh textbox with an empty document in Insert mode.
- **`ComposeTextbox::text() -> String`** -- extract the full text from the textbox's editor.
- **`App::compose_toggle()`** -- if `compose_textbox` is `None` and the active buffer is `*claude*`, create one. If `Some`, toggle off: append text to main buffer, set to `None`.
- **`App::compose_send()`** -- extract text, toggle off, then send via the active channel. No-op if empty.
- **`App::compose_is_active() -> bool`** -- shorthand for `self.compose_textbox.is_some()`.

## State Machine

```
                   ComposeToggle (in *claude*)
    CLOSED ────────────────────────────────────► OPEN (Insert)
      ^                                               |
      |                                               | Esc
      |                                               v
      |                                          OPEN (Normal)
      |                                               |
      |              ComposeToggle                     |
      |◄──────────────────────────────────────────────/
      |   (append text to main buffer)
      |
      |              ComposeSend
      |◄──────────────────────────────────────────────/
          (append text, then send via channel)
```

Within OPEN, the textbox cycles between Normal and Insert as usual (`i`/`a`/`o` to enter Insert, `Esc` to return to Normal). The outer OPEN/CLOSED transition is orthogonal to these inner modes.

## Rendering

The textbox is rendered as a new layout slot between the content area and the bottom bar. The vertical layout becomes:

```
+-----------------------------------+
| top bar                           |  1 row
+-----------------------------------+
| [buffer list / browser / outline  |  variable
|  panels, as today]                |
+-----------------------------------+
| main content area                 |  fills remaining space
| (scrollable buffer content)       |
+-----------------------------------+
| --- compose --- (separator)       |  1 row (only when textbox open)
| textbox content                   |  dynamic height
+-----------------------------------+
| bottom bar                        |  0-1 rows
+-----------------------------------+
```

**Height policy:** the textbox starts at 3 rows and grows as the user types, up to a maximum of `min(viewport_height / 3, 12)` rows. The separator line ("compose" label centered between dashes) takes 1 additional row. The content area shrinks to accommodate.

**Separator styling:** dim horizontal rule with the word "compose" centered, matching the status bar's visual weight. Gives the user a clear boundary between streaming content and their editing surface.

**Cursor:** the terminal cursor is positioned in the textbox, not the main buffer. The main buffer does not show a cursor while the textbox is active.

**Mode label:** the top bar's mode label reflects the textbox's mode: `COMPOSE` when in Normal mode within the textbox, `COMPOSE INSERT` when in Insert mode. This replaces the usual `RAW` / `INSERT` labels while the textbox is active.

### Impact on `compute_viewport_height`

`compute_viewport_height` must subtract the textbox height (content rows + 1 separator row) from the available space, just as it does for the file browser and outline panels. This keeps the scroll math correct for the main buffer.

## Keybindings

| Action | Default binding | Context |
|--------|----------------|---------|
| `ComposeToggle` | `Ctrl-T` | Normal mode, *claude* buffer only |
| `ComposeSend` | `Ctrl-Enter` | Normal or Insert mode, while textbox is open |

`Ctrl-T` is currently unbound. `Ctrl-Enter` is distinguishable from `Enter` in most terminals (crossterm reports it as `KeyCode::Enter` with `KeyModifiers::CONTROL`).

`Ctrl-Up` and `Ctrl-Down` are mapped to scroll the main buffer viewport while in the textbox. These are handled as special cases in the textbox key dispatch -- they bypass the textbox editor and call `scroll_up` / `scroll_down` on the main buffer's viewport.

## Constraints

1. **No interference with splice algorithm.** The textbox is a separate `Editor` -- it never touches the main buffer's rope, frozen lines, or lockable prefix. The only interaction is on close, when text is programmatically inserted.
2. **No new file I/O.** The textbox is purely in-memory. `*compose*` is not a real file. Save commands are no-ops or ignored while in the textbox.
3. **Single textbox.** Only one `ComposeTextbox` can exist at a time. The field is `Option<T>`, not `Vec<T>`.
4. **Textbox is ephemeral.** Buffer switches, quit, and other lifecycle events close the textbox. Text is appended to the main buffer on close -- never silently discarded.
5. **No textbox for non-claude buffers.** The feature is scoped to the `*claude*` buffer. Attempting to toggle in a regular file buffer is a no-op.

## Revision History

- 2026-05-14 — Status SHIPPED. Reconciled §11 with implementation: insert-on-close goes to EOF (after draft), not the splice point (before draft) as earlier drafts said. The shipped behavior reads better when the user has existing draft text — their newly-composed text lands where they expect it.
