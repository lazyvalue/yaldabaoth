# Component: Keybindings Tile

**Status:** living
**Component token:** `Keybindings` (⇒ `UXI-Keybindings-N`)

## Description

The keybindings tile (`App::Keymap`) is a **live** keymap reference, not a static
cheat-sheet: it renders the SAME `KeymapRegistry` that `register_keymap` applies to
the app, so every keystroke it shows is a real binding, grouped by context then
category. Rebinds happen **in place** — committing a new chord mutates that one
registry, re-applies the whole keymap atomically, and persists the diff to
`~/.yalda/keymap-overrides.json`, so the row updates the moment it commits and
"what the sheet says" and "what the app does" stay the same object.

## References

- Migrated from `docs/ux-invariants.md` INV-UX-17 (live keymap + in-place rebind).
  That entry is now `→ migrated here`.

## UX invariants

### UXI-Keybindings-1 — The keybindings tile shows the LIVE keymap and rebinds it in place

**Statement.** The `App::Keymap` reference tile is not a static cheat-sheet: it
renders the SAME `KeymapRegistry` that `register_keymap` applies to the app
(`keymap_registry.rs`, `DEFAULT_BINDINGS` → `KeymapRegistry::apply`). Two
consequences that must hold:

1. **Truthfulness (dynamic).** Every keystroke the tile displays is a live
   binding, grouped by context (the GPUI `key_context`) then theme (category).
   There is no second copy of the keymap to drift from — `register_keymap` is
   `KeymapRegistry::load().apply(app)`, and the tile reads the registry off the
   root. A rebind updates that one registry, so the row updates the moment it
   commits.
2. **Rebind = apply + persist, and capture grabs the keyboard.** Committing a
   rebind (`keymap_ui.rs::keymap_commit_rebind`) mutates the registry,
   re-`apply`s the whole keymap atomically (`clear_key_bindings` + `bind_keys`,
   so GPUI precedence is unchanged from the ported defaults), and persists the
   diff to `~/.yalda/keymap-overrides.json` (reloaded next launch). While
   capturing the new chord the app keymap is CLEARED, so the pressed chord is
   **recorded, not fired** (pressing `cmd-t` during capture must not open a tab);
   commit/cancel re-apply the registry, restoring bindings with the new one live.

**Applies to.** `keymap_registry.rs` (the table + `apply`/`rebind`/`reset`/
`persist`/`conflicts`), `keymap_view.rs` (`KeymapView` — the cached body; the
browse cursor is always on a marked row via the `›` gutter, INV-UX-1's spirit for
this surface), `keymap_ui.rs` (the key handler + capture grab), and `main.rs`
`register_keymap` (now data-driven from the table).

**Why.** Bindings were ~120 inline `KeyBinding::new` calls with no introspection
and no rebind path. Lifting them into one declarative table that BOTH drives the
live keymap AND backs the reference makes "what the sheet says" and "what the app
does" the same object, so they cannot disagree.

**Status.** `implemented` (headless).

**Enforcement.** `verify_harness.rs`: `keymap_registry_table_is_valid` (every
action builds / context + keystrokes parse — nothing silently unbound),
`keymap_rebind_via_real_keystrokes` (drive the REAL `handle_keymap_key` capture →
commit; the live registry entry changes; negative control verified),
`keymap_rebind_persists_and_reloads` (override survives a reload; garbage keys
rejected), `keymap_conflict_detection`, and
`keymap_body_is_cached_and_self_invalidates` (the render-count perf guard for the
new cached surface).
