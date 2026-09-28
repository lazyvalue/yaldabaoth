# Component: Text Editing (common)

**Status:** draft — scaffold; motion/selection rules to be migrated + specified
**Component token:** `TextEditing` (⇒ invariants are `UXI-TextEditing-N`)

## Description

The shared editing model every editable/navigable surface in the app obeys: the
file edit buffers (`EditView` Code + WordProcessor), the rendered-doc cursor, the
agent transcript navigation caret, and the agent compose buffer. Rather than each
surface re-deriving "how a cursor moves" or "what insert vs normal means", they all
conform to the invariants here and add only their surface-specific refinements.

The engine lives in `editor.rs` (operations over the ropey-backed `document.rs`) and
`keybind.rs` (binding + sequence matching); `keys.rs` / `style.rs` are the
frontend-neutral primitives. Modes today are **vim-style** (`AppMode::Normal` /
`Insert`); the target direction is **helix-style** (selection-first: a motion
carries a selection, operators act on the current selection). This spec is the home
for those rules as they are specified — encode the model here once, reference it
everywhere.

## References

- `UXI-AgentTile-9` (compose word-wraps) — to be generalized here as `UXI-TextEditing-2`.
- `docs/ux-patterns/` — `UXP-1` (the caret is always visible) is the universal law this component realizes for text surfaces.
- `docs/specs/spec-chatbox-caret-containment.md` — the caret-window chokepoint.
- Naming: View Mode / Edit Mode / Normal / Insert — see root `CLAUDE.md`.

## UX invariants

### UXI-TextEditing-1 — The cursor is always visible, and moving it moves the text

**Statement.** In any surface with a caret, the caret is always within the visible
viewport; moving the caret scrolls content so it stays visible (vertically and
horizontally). The caret is never stranded off-screen, and the viewport never shows
a region the caret has left.

**Applies to.** Every editable/navigable surface (see Description) + any future one.
`editor.rs` splice cursor-shift; `compose_first_visible_line` / `ScrollAnchoredList`.

**Why.** A caret you can't see is a caret you can't use. The single most-regressed
property in the app.

**Status.** `implemented`.

**Enforcement.** `verify_harness.rs::compose_caret_row_painted_inside_box_when_wrapped`
+ the caret-containment model guards (listed in `spec-chatbox-caret-containment.md`).

<!-- TODO(migration): move UXI-AgentTile-9 (word-wrap) here as UXI-TextEditing-2, then
     specify the helix-style selection/motion/operator model as further UXI-TextEditing-N. -->

### UXI-TextEditing-3 — Enter continues a list/quote at the same indent (nesting is preserved)

**Statement.** Pressing Enter at the end of a markdown list / TODO / blockquote
line continues it: the next line starts with the SAME leading indent plus the
same marker (bullets keep their glyph, ordered items increment, checkboxes reset
to unchecked, blockquotes repeat `> `). A **nested** (indented) item stays at its
indent level — it never jumps back to column 0. Enter on an *empty* list item
instead clears the dangling marker and drops to a blank line (ends the list).
Splitting mid-line (caret not at end-of-line) keeps the plain-newline behavior.

**Applies to.** Every editable surface, via the shared insert path
(`dispatch_insert_core` → `list_continuation_action`): the buffer editor
(Code + WP), and the agent compose in BOTH placements (worksheet + chatbox).
Bare Enter in the compose inserts a newline (it never submits — Ctrl-Enter
submits), so list continuation applies there too.

**Why.** Nesting a list is a core editing gesture; losing the indent on every
newline makes multi-level lists unusable.

**Status.** `implemented` — behavior pre-existed in `list_continuation_action`;
now guarded.

**Enforcement.** `verify_harness.rs::buffer_enter_continues_nested_list_at_same_indent`
+ `compose_enter_continues_nested_list_at_same_indent` (shared `{indent}` NC
observed RED); marker rules by `edit_ui.rs::list_continuation_tests`.

### UXI-TextEditing-4 — A long unbreakable run soft-wraps, it is never clipped

**Statement.** On a `build_wrapped_line` surface (the Code + WP edit views, the
worksheet transcript/compose), a single run with no whitespace to wrap at — a
path, URL, or hash — soft-wraps to the next line rather than overflowing the row
and being clipped by the surface's `overflow_x_hidden`. This is most visible on a
bullet whose content is a long path (the "word wrapping fails on bullet points /
lists" report). Ordinary words are unaffected; only runs longer than
`MAX_UNBROKEN_TOKEN` (40 chars) are char-chunked, into `OVERLONG_TOKEN_CHUNK`
(16-char) pieces that fit even a thin split pane.

**Applies to.** Every `build_wrapped_line` surface. (The rendered doc view
already char-wraps via gpui `StyledText`; the chatbox compose intentionally
*scrolls* long runs horizontally per its caret-containment spec, so it is out of
scope.)

**Why.** `flex_wrap` only breaks BETWEEN token children; one over-wide child
overflows and is clipped, so the tail of a long path in a bullet was invisible.
The chunks abut, so caret / selection / hit-test offsets are unchanged.

**Status.** `implemented` — `agent.rs::chunk_overlong_tokens` in
`build_wrapped_line`.

**Enforcement.** `verify_harness.rs::code_edit_wraps_unbroken_token_in_bullet`
(layout probe; the un-chunked NC paints ~1 line and fails RED).

### UXI-TextEditing-5 — Every single-line field edits the same way

**Statement.** Every single-line text field — the jump palette and Diff send
picker queries, the tag editor, the keymap / Cog / Linear / buffer-switcher
filters, the file-browser filter + rename (screen and rail), and the rename /
new-project cwd / tag-input overlays — is a shared `LineInput` with a movable
caret and one key policy:

- a typed character inserts at the caret; **Ctrl / Cmd chords never type** (an
  unbound `cmd-y` must not insert `y`); **Alt types only an OS-composed
  character** (Option-2 → `@`), never a bare ASCII letter/digit;
- Backspace / Delete delete a char; Alt- or Ctrl-Backspace / -Delete delete a
  word (path separators end a word); Cmd-Backspace / Ctrl-U delete to start,
  Ctrl-K to end;
- Left / Right move a char (Alt/Ctrl: a word; Cmd: to start/end); Home / End /
  Ctrl-A / Ctrl-E jump to start/end;
- Ctrl-W is never consumed (reserved shell prefix);
- the caret draws as `█` at its position.

Enter / Esc / Up / Down (and any key a site binds first) stay with the owning
site.

**Applies to.** All fields above + any future single-line field. Model:
`src/line_input.rs` (`LineInput`, `KeyPress::typed_char`); GUI glue:
`yux/line_input.rs` (`LINE_INPUT_CARET`). Never hand-roll `String::push`/`pop`
editing.

**Why.** The fields were 14 separate `push`/`pop` implementations with
diverging modifier rules: six typed the bare letter of any Cmd/Ctrl chord,
three blocked Option-composed symbols, and prefilled paths could only be edited
one backspace at a time from the end (review
`docs/research/2026-09-27-text-editing-review.md` A1–A5).

**Status.** `implemented` (graph `exa`).

**Enforcement.** `line_input::tests` (key policy, word motion, caret insert);
`verify_harness.rs::line_input_overlay_rejects_chords_and_edits_at_caret`
(real keystrokes on the new-project overlay; the old push/pop handler NC fails
RED with `"/tmp/ws/projyy"`).
