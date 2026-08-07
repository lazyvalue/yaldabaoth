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

- `docs/ux-invariants.md` INV-UX-1 (cursor always visible), INV-UX-2 (compose
  word-wraps) — **to be migrated into `UXI-TextEditing-N` here.**
- `docs/specs/spec-chatbox-caret-containment.md` — the caret-window chokepoint.
- Naming: View Mode / Edit Mode / Normal / Insert — see root `CLAUDE.md`.

## UX invariants

### UXI-TextEditing-1 — The cursor is always visible, and moving it moves the text

**Statement.** In any surface with a caret, the caret is always within the visible
viewport; moving the caret scrolls content so it stays visible (vertically and
horizontally). The caret is never stranded off-screen, and the viewport never shows
a region the caret has left.

**Applies to.** Every editable/navigable surface (see Description) + any future one.
`editor.rs` splice cursor-shift; `compute_window` / `ScrollAnchoredList`.

**Why.** A caret you can't see is a caret you can't use. The single most-regressed
property in the app.

**Status.** `implemented` — this is INV-UX-1, hosted here during migration; that entry
remains authoritative until fully moved.

**Enforcement.** `verify_harness.rs::compose_caret_row_painted_inside_box_when_wrapped`
+ the caret-containment model guards (see INV-UX-1 for the full list).

<!-- TODO(migration): move INV-UX-2 (word-wrap) here as UXI-TextEditing-2, then
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
