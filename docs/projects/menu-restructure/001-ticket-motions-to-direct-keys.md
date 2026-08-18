# T2 — Motions → direct nav keys

**Status:** open (blocked on nothing; do after T1 lands)
**Depends on:** T1 (menu reorganization)

## Goal

Per `UXI-Menu-6`, motions do not belong in menus. Move the doc/edit/browser
motions out of `space` and give them direct navigation keys, then strip the
motion submenus.

## Subtasks

- [ ] DOC: bind `next-link` / `next-heading` / `next-list` / `next-code-block`
      and `goto-top` / `goto-bottom` as nav-mode direct keys; remove the `n`
      and `g` submenus from `doc_local_menu`.
- [ ] EDIT: the selection ops (`toggle-extend-mode`, `collapse-selection`,
      `flip-selection`, `extend-line`, select-all / yank / delete) are
      mode-native — bind them in the modal editor keymap; remove the `e`
      submenu and the `a`/`y`/`d` leaves from `edit_local_menu`.
- [ ] AGENT: bind `agent-toggle-jump-mode` (`j`) and `agent-focus-toggle` (`f`)
      as nav-mode direct keys; they were dropped from the menu in T1.
- [ ] BROWSE: `browser-up` (`-`) and `browser-hidden` are navigation of the
      listing — bind directly; remove from `browser_local_menu`.
- [ ] COG: `cog-graphs` (`g` back-to-list) is navigation — bind directly.
- [ ] Update `UXI-Menu-7` test coverage; add nav-key guards.

## Verification

- Each new direct key exercised via `simulate_keystrokes` on the real screen
  (prefer non-`ctrl`-digit chords; see anti-circling rule 4).
- Negative control each binding.
