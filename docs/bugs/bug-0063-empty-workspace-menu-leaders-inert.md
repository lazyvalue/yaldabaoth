# bug-0063: empty-workspace-menu-leaders-inert

**Status:** FIXED  
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
