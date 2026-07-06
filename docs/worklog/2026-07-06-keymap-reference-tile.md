# Worklog: keybindings reference + rebind tile

**Date:** 2026-07-06
**Branches touched:** `keymap-tile` (worktree) → merged to `main`

## Built (with status)
- **New `App::Keymap` tile** — a dynamic, rebindable keyboard-shortcut reference
  sheet. Opens via **Cmd-/**, the workspace `. → new → keybindings` menu, the
  agent-style `space` tile menu, or the macOS menu bar ("Keyboard Shortcuts").
  Shows every GPUI binding grouped by **context** (Global / Document view / Agent
  / File browser / Rail) then **theme** (Navigation, Zoom, Splits & focus, …), plus
  a read-only **Leader menus** section walked live from the `MenuNode` trees.
  Builds + `cargo test --bin yalda-gpui` green (330 passed).
- **Central keybinding registry (the framework that didn't exist).**
  `keymap_registry.rs` lifts the ~120 inline `KeyBinding::new` calls out of
  `register_keymap` into one declarative `DEFAULT_BINDINGS` table. `register_keymap`
  is now `KeymapRegistry::load().apply(app)` — the table IS the live keymap
  (built via `App::build_action` + `KeyBinding::load`), so the reference reads the
  same object the app dispatches from. Order preserved 1:1 so GPUI precedence is
  unchanged (all 330 keystroke/dispatch tests still pass).
- **Live rebinding.** In the tile: `j/k` browse, `/` filter, `r`/Enter rebind, `x`
  reset, `R` reset-all. Rebind capture GRABS the keyboard (`clear_key_bindings`)
  so the pressed chord is recorded not fired; commit mutates the registry,
  re-`apply`s the whole keymap atomically, and persists the diff to
  `~/.yalda/keymap-overrides.json` (reloaded next launch, keyed by
  action|context|default so it survives table reordering). Conflict detection
  surfaces overlapping-context collisions as an advisory.
- **Wiring:** `App::Keymap(KeymapTile)` variant + all match sites (render dispatch,
  tab titles, persistence `PersistedKind::Keymap`, `into_buffer_stash`,
  `focused_in_insert_mode`, local-menu selection, `notify_keymap_views` for
  theme/zoom). Body is a yux cached child (`KeymapView`) with a render-count guard.
- **INV-UX-17** added (ux-invariants.md): the sheet shows the LIVE keymap and
  rebinds in place (apply + persist; capture grabs the keyboard).

## Open / unresolved
- **Menu-command keys are reference-only** — the leader-menu section is displayed
  but not rebindable (menu keys live in nested `MenuNode` trees, a separate
  dispatch path). Rebinding those is a possible follow-up.
- **Rebind capture is a modal keyboard grab.** If focus leaves the tile mid-capture
  (rare) the app keymap stays cleared until Esc/commit returns. Acceptable for v1;
  a focus-out auto-cancel could harden it.
- **Multi-key chord capture** works (space-joined), but there's no UI to bind a
  *sequence prefix* like a brand-new `ctrl-w x` beyond typing the chords.

## Decisions
- Data-driven keymap over a parallel "docs" copy: one `KeymapRegistry` backs both
  `register_keymap` and the tile, so displayed keys can't drift from live keys.
- Persistence stores only diffs from default, keyed by (action, context, default
  keystrokes) — resilient to future table reordering.

## Verification status
- Headless-verified: `keymap_registry_table_is_valid`,
  `keymap_rebind_via_real_keystrokes` (REAL `handle_keymap_key` capture→commit,
  negative-control confirmed RED), `keymap_rebind_persists_and_reloads`,
  `keymap_conflict_detection`, `keymap_body_is_cached_and_self_invalidates`.
- Full suite: 330 passed / 1 ignored — the `register_keymap` refactor preserved
  every existing binding.
- **Needs human runtime** (gap 1: pixels/colors): the exact visual layout, theme
  colors, and the feel of the rebind-capture flow in the live GUI.

## Next
- Runtime pass in the GUI: open with Cmd-/, rebind a key, confirm it takes + sticks
  across restart; sanity-check the grouped layout + filter feel.
- Consider: rebindable leader-menu keys; a "reset all" confirmation; showing the
  live conflict advisory inline while capturing.
