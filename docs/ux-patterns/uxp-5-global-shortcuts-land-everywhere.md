# UXP-5 — Global shortcuts work from every screen and focus state

**Statement.** A shortcut registered as global (`None` key context — quit, zoom,
tile/workspace management, the palettes) triggers from every screen, tile type,
and focus state, including an empty workspace. Every screen root wires the matching
`on_action` handler. The only surfaces that may hold a global chord are transient
overlays, which take input in the capture phase while open.

**Applies to.** `register_keymap` global bindings and every screen root's
`on_action` list (`YaldaView`, `EditView`, `BrowserView`, `AgentView`, and every
App's root).

**Why.** A global binding without a handler on the focused root does nothing —
repeatedly shipped as "shortcut dead in tile X" (e.g. empty-workspace leaders,
graph `nod`).

**Status.** `partial` — enforced per binding, not yet by one sweep over all roots.

**Enforcement.** `simulate_keystrokes` from the real focused root, e.g.
`verify_harness.rs::ctrl_w_hide_unhide_and_workspace_back_and_forth_are_global`.
OS-mangled chords (macOS `Ctrl`+digit, `Ctrl-Tab`) are a genuine gap — prefer
`Cmd` bindings.

**Realized by.** `UXI-Menu-*` (leader menus), `UXI-Workspace-*` (workspace chords).
