# bug-0063: empty-workspace-menu-leaders-inert

**Status:** RECURRED→FIXED  
**First seen:** 2026-08-30  
**Component:** `docs/components/common/menu.md` (`UXI-Menu-6`)  
**Cog:** `nod`

## Symptom

In an empty workspace, pressing `.` or Space did not open a menu. The same
leaders worked as soon as a tile was present.

## Root cause

The empty-layout chrome branch returned a bare visual root. Unlike App roots, it
did not install leader interception or global action handlers. Additionally, the
shell menu represented its opening focus as a mandatory tile id, so it refused
to open when the workspace correctly had no focused tile.

## Fix

The empty workspace is now a first-class shell input surface: its root installs
the shared workspace navigation, shell actions, menu actions, and leader key
handler. Shell menus record an optional focused tile, preserving stale-focus
dismissal while permitting a legitimate no-tile origin. Space normally opens
the focused App menu; with no App it deliberately falls back to the shell menu.

## Attempts

### 2026-08-30 — route the empty shell surface

- Added a real GPUI/keymap regression guard covering `.` and Space on the painted
  production empty-workspace root.
- Negative control: before the root and optional-origin changes, `.` left the
  overlay unset; with only `.` fixed, Space still left it unset.

### 2026-09-10 — RECURRED: new `Layout::Empty` render branch bypassed the fix

- **Symptom (Scott):** "command menu keys don't work on an empty workspace. no
  menu is summoned." Same as the original bug.
- **Root cause:** the original fix (`4fa4205`) installed the leader handlers on
  the branch that rendered the empty workspace *at that time* (the
  presented-tile branch, `id("empty-workspace-root")`). Commit `c0b9128`
  ("refactor workspace tile visibility") added a NEW dedicated
  `matches!(layout, Layout::Empty)` branch in `render_focused_window`
  (`chrome.rs`) that returns a **bare** `div().size_full().ctrl_w_shell_actions(cx)`
  — no `key_context`, no `on_key_down`, no `open_menu`/`open_local_menu` actions,
  and crucially **no `track_focus`**. A truly empty workspace (`presented_tile()`
  is `None`) hits this new branch, so the shared focus handle held by the
  just-closed tile lands on nothing painted and every key dispatches into a
  handler-less window root — no menu.
- **Fix:** give the `Layout::Empty` branch the same routing as the presented-tile
  branch **plus** `.track_focus(&self.focus_handle)` (the static empty content
  carries no focusable App leaf, unlike the presented branch whose `content`
  attaches the handle internally), under `key_context("EmptyWorkspaceView")` /
  `id("empty-layout-root")`.
- **Why the old guard didn't catch it:** `empty_workspace_dot_and_space_open_the_shell_menu`
  reached the empty workspace by `push_empty_workspace` + `set_active_workspace(1)`
  **beside a still-focused browser** — the browser's focus lingered, so the
  keystroke never exercised the empty root's own focus. It false-passed with the
  bug live (anti-circling rule 1: a hand-arranged proxy state, not the real
  path). Replaced it with `empty_workspace_after_close_last_tile_keeps_menu_leaders`,
  which reaches `Layout::Empty` by CLOSING the last tile (`close-window`) — the
  real user path — then presses `.` and `space` via the real keymap.
- **Negative control:** with the `chrome.rs` routing reverted, the new guard
  panics at the `.`-opens-menu assertion (menu unset). Restored → green. Full
  `cargo test --bin yalda-gpui`: 766 passed, 0 failed, 1 ignored.
- **Activation:** code + guard on `main`; the running GUI is unchanged — rebuild
  + restart is Scott's call (`NEEDS-RUNTIME` per the no-restart rule).
