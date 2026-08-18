# 2026-08-18 — Menu restructure: three leaders → two

## What shipped

Collapsed the GPUI leader command menus from three (`space` per-App, `.`
workspace, `?` global) to **two**, organized by the **object of the verb**, not
by scope:

- **`space`** — verbs on the focused tile's App (polymorphic per App type).
- **`.`** — verbs on the shell (tiles, workspaces, appearance, system). Absorbed
  the retired `?` menu.

Placement is decided by **shape + object**, not frequency: motion → direct key;
verb → menu (object picks which); escape hatch → `:` (deferred). ADR-0032;
`UXI-Menu-6`/`-7` in `docs/components/common/menu.md`.

### Changes (T1)

- `gpui_menu` (`.` shell) gains `w` (workspace: new/rename/close + new-project)
  and `s` (system: rebuild-gui/all, console) submenus, plus `j` toggle-jump-panel.
  Numbered workspace jump stays on `ctrl-1..0` (never a menu item).
- `agent_local_menu` regrouped: flat conversation verbs (`c/e/w/m/S/C`) + `s`
  session-lifecycle submenu (`n/N/r/t/x/R` + dynamically-grafted archive) + `v`
  interim submenu (`f`/`j` motions, `h` heading-markers) + `M` model submenu.
  `.` stop removed (Esc interrupts).
- DOC: dropped `b` file-browser (Cmd-O dup) and the duplicate `g h` next-heading.
  EDIT: dropped `b`.
- Removed the `?` leader route, `OpenGlobalMenu` action, `global_menu` /
  `open_global_menu_inner` / `open_global_menu`, and two `screens.rs` wirings.

### Open / follow-ups

- **T2** — motions → direct nav keys, then strip the DOC `n`/`g` and agent `v`
  motion submenus. `docs/projects/menu-restructure/001-ticket-motions-to-direct-keys.md`.
- **T3** — build the `:` escape-hatch command line (no `:` surface exists in the
  GPUI app yet); move `reset-all` / `toggle-heading-markers` there. Needs an
  explicit go-ahead. `002-ticket-colon-command-line.md`.
- `docs/specs/spec-menu-scopes.md` marked SUPERSEDED (it documented an inverted,
  since-abandoned three-scope model).

### Verification

- `cargo test --bin yalda-gpui`: **589 passed, 0 failed, 2 ignored**.
- `cargo clippy --bin yalda-gpui`: **0 errors**, no new warnings.
- Negative control (anti-circling rule 2): restoring the `?` route made
  `question_mark_leader_is_inert_*` fail RED for the right reason
  (`left: (true, true)` != `right: (false, false)`), then reverted.
- Runtime: NEEDS-RUNTIME only for exact painted glyphs/colors of the new
  submenus (documented gap 1 — the layout probe gives rects, not a bitmap).

## Cog execution evidence

- Graph id: `v30`

### Initial render

```
graph menu-restructure-2menu (frontiers)
frontier 0: spec-and-adr [open]
frontier 1: agent-space-reorg [open], merge-global-into-shell [open], prune-other-space-menus [open]
frontier 2: tests-build-verify [open]
frontier 3: omega [open] (omega)
```

### Node execution

Every node was claimed (actor `claude-code`), worked, and closed with JSON
output:

- `spec-and-adr` (`0w6z`) — claimed → closed; output: ADR-0032 + `UXI-Menu-6/-7`
  + project record.
- `merge-global-into-shell` (`jqfo`) — claimed → closed; output: `?` folded into
  `.`; 6 verify_harness + 2 tests.rs updated.
- `agent-space-reorg` (`65o1`) — claimed → closed; output: flat + `s`/`v`/`M`
  submenus; tests updated.
- `prune-other-space-menus` (`osgx`) — claimed → closed; output: DOC/EDIT dups
  removed.
- `tests-build-verify` (`7szy`) — claimed → closed; output: no-dup coverage
  extended; `?`-inert guard with observed-RED negative control; 589 pass.
- `omega` (`f5ye`) — claimed → closed; output: T1 complete + merged to main.

### Notes

- Graph note (topic `decision`, seq 14): the approved shape+object model + the
  three code constraints found (no `:` surface in GPUI → T3; workspace-jump on
  macOS-unreliable `ctrl`+digit → left as-is; doc motions have no direct keys →
  T2 so nothing is stranded).

### Final status

- Status: `complete`

```
graph menu-restructure-2menu (frontiers)
frontier 0: spec-and-adr [done]
frontier 1: agent-space-reorg [done], merge-global-into-shell [done], prune-other-space-menus [done]
frontier 2: tests-build-verify [done]
frontier 3: omega [done] (omega)
```
