# Project: Menu restructure (three leaders → two)

## Problem / why

The GPUI app had three leader command menus — `space` (per-App local), `.`
(workspace), `?` (global). The owner found them unwieldy, with noise entries and
a scope split (tile / workspace / global) that leaks to the user (workspace ops
were spread across `.` and `?`; themes/zoom sat in the "workspace" menu).

## The model (authoritative: ADR-0032)

Collapse to **two** menus, organized by the **object of the verb**, triaged by
**shape** (not frequency):

- **`space`** — verbs whose object is the **focused tile's App** (polymorphic per
  App type: doc / session / listing / issue / graph / bindings).
- **`.`** — verbs whose object is the **shell** (tiles, workspaces, appearance,
  system). Absorbs the retired `?` menu.
- **Shape triage:** motion → direct key (never a menu); verb → menu (object picks
  which); escape hatch (rare/destructive, invoked by name) → `:` command line.

Invariants: `UXI-Menu-6` (two leaders, object split), `UXI-Menu-7` (no duplicate
key per menu level). Component spec: `docs/components/common/menu.md`.

## Constraints found in code

- **No `:` command surface in the GPUI app** (`command.rs` is TUI-only) → the
  escape-hatch tier has no home yet → **T3**.
- **Workspace jump already bound `ctrl-1..0`** (`keymap_registry.rs`), but macOS
  eats `ctrl`+digit (anti-circling rule 4) → left as-is this pass.
- **Doc/edit motions exist only as menu entries** (no direct keys) → removing them
  strands them → **T2** adds nav keys first.

## Tickets

| # | Ticket | Status |
|---|--------|--------|
| T1 | Menu reorganization (merge `?`→`.`; Agent flat+lifecycle; prune dups) | in progress (Cog graph `v30`) |
| T2 | Motions → direct nav keys, then strip motion submenus from `space` | open (`001-ticket-motions-to-direct-keys.md`) |
| T3 | Build the `:` escape-hatch command line; move rare/destructive there | open (`002-ticket-colon-command-line.md`) |

## Links

- ADR: `docs/decisions/0032-two-leader-menus-shape-object-model.md`
- Component spec: `docs/components/common/menu.md` (`UXI-Menu-6`, `UXI-Menu-7`)
- Superseded: `docs/specs/spec-menu-scopes.md`
- Cog graph (T1): `v30` (`menu-restructure-2menu`), omega `f5ye`
