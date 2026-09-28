# UXP-1 — The caret is always visible, and moving it moves the visible text

**Statement.** On every surface with a caret or navigation cursor (editors, the
agent compose, pickers, palettes, comment boxes, browse lists), the cursor is inside
the painted viewport after every keystroke, and a motion that moves it scrolls the
content so the cursor stays visible. No surface lets its cursor leave the viewport
or clip behind chrome.

**Applies to.** Any caret-bearing surface; the chokepoint is the caret-window model
in `spec-chatbox-caret-containment.md`.

**Why.** A caret you can't see is a caret you can't use — the single most-regressed
property in the app (legacy INV-UX-1).

**Status.** `implemented` on the text surfaces; new caret surfaces must add a guard.

**Enforcement.** Layout-probe guards that assert the caret painted inside the
viewport with non-vacuous overflow, e.g.
`verify_harness.rs::compose_caret_row_painted_inside_box_when_wrapped`.

**Realized by.** `UXI-TextEditing-1`, `UXI-AgentTile-9` (compose word-wrap),
`UXI-Keybindings-1` (browse cursor).
