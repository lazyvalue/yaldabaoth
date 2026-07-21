# bug-0011: agent-picker-eats-workspace-nav-keys

**Status:** FIXED
**First seen:** 2026-07-20
**Component:** docs/components/agent-tile

## Symptom

While an agent tile is showing the **session selector / picker** (an unbound tile,
`bound: None`, rendered by `render_agent_picker`), pressing `ctrl-<n>` (GotoWorkspace1-10)
or the `cmd-shift-[` / `cmd-shift-]` cycle keys does nothing — the picker "eats"
workspace-switch keys. Every other screen (doc, edit, browser, bound agent) switches
workspaces fine; only the picker is dead.

## Context / root cause

Workspace navigation actions (`GotoWorkspace1-10`, `NextTab`, `PrevTab`) are wired via
the `WorkspaceNavExt::workspace_nav` extension (`main.rs:333`), which each screen root
must call on itself — there is **no** window-level or canvas-level fallback handler
(the canvas root wires only plane-camera actions; `render()` returns the bare
`screen_view` when no overlay is open). GPUI dispatches an action by bubbling from the
focused element up its ancestors; if no ancestor wired the handler, the action is
dropped.

`render_agent_picker` (`screens.rs`) was the **one** screen root that never called
`.workspace_nav(cx)` — every sibling root (doc `:251`, edit `:398`, bound agent
`:2093`, browser `:2238`/`:2307`, keymap `:2905`) does. So when the picker is the
focused leaf, `ctrl-<n>` bubbles into a dead chain.

## Planned solution

Add `.workspace_nav(cx)` (and `toggle_jump_panel`, the other workspace-nav global the
picker also lacked) to the `AgentPickerView` root in `render_agent_picker`, matching the
bound `AgentView` root.

## Approaches already tried (do NOT repeat)

- **First guard attempt was a false pass — the test never rendered the picker.** The
  test set the picker via `install_agent_picker` (→ `set_screen`) but `set_screen`
  does NOT `cx.notify()`, so the next `run_until_parked` did not repaint. The stale
  dispatch tree still held the boot-browser leaf (which HAS `workspace_nav`), so
  `ctrl-3` switched workspaces regardless of the picker — negative control PASSED with
  the fix reverted (guarded nothing). Fix for the TEST: force `view.update(.., |_,cx|
  cx.notify())` after `install_agent_picker` so the picker actually renders and becomes
  the focused node in the dispatch tree. (Diagnosed via eprintln in `render_agent_picker`
  / `render_desktop` / `goto_workspace_number`: `render_agent_picker BUILT` never fired
  in the first version.)

---

## Log

### 2026-07-20 — Wired workspace_nav onto the picker root

- **Changed:** `screens.rs` `render_agent_picker` — added `.on_action(toggle_jump_panel)`
  + `.workspace_nav(cx)` to the `AgentPickerView` root (mirrors the bound `AgentView`
  root at `screens.rs:2092-2093`).
- **Guard:** `verify_harness.rs::agent_picker_does_not_eat_workspace_switch_keys` — boots
  four real workspaces, replaces workspace 1's tile with an UNBOUND agent tile
  (`install_agent_picker`), **forces a repaint** (see the false-pass note above) so the
  picker is the focused dispatch node, then drives the REAL keymap: `simulate_keystrokes
  ("ctrl-3")` must land on workspace 3, and `cmd-shift-]` must advance from the picker.
- **Negative control (observed RED):** with the two picker lines reverted AND the
  forced repaint in place, `ctrl-3` leaves `active_tab=0` (asserted, test FAILED for the
  right reason — the picker rendered, `render_agent_picker BUILT`, and the action fell
  into a dead chain). Restored the fix → GREEN.
- **Suite:** `cargo test --bin yalda-gpui` → 387 passed, 0 failed.
- **Outcome:** FIXED. Not yet committed (awaiting user). NEEDS-RUNTIME only for the
  macOS OS-delivery gap: `simulate_keystrokes` proves the WIRING, but a bare `Ctrl`+digit
  is OS-mangled on real macOS (documented 4th genuine gap) — the `cmd-shift-[]` path is
  the reliable one and is covered by the same test.
