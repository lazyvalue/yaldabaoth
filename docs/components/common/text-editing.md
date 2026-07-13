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
