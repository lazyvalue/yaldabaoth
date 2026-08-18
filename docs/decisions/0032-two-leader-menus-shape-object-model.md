# ADR-0032 — Two leader menus, organized by the shape+object model

**Status:** accepted
**Date:** 2026-08-18
**Supersedes the scope model in:** `docs/specs/spec-menu-scopes.md` (DRAFT),
`docs/components/common/menu.md` (UXI-Menu descriptions that name three leaders)

## Context

The GPUI app had **three** leader command menus:

- `space` — a per-App local menu (a different tree for Buffer-viewing,
  Buffer-editing, Agent, Browser, Linear, Cog, Keymap).
- `.` — a workspace menu (new tile, theme, plane zoom, arrangement, mark, close
  tile/workspace, dev restart).
- `?` — a global menu (jump to workspace 1..9, name/new workspace, new project,
  system console, jump-panel toggle).

The owner's complaint: the three menus are unwieldy, some entries are noise, and
the tile/workspace/global scope split is questionable. Concrete evidence the
split leaks to the user:

- **Workspace operations are spread across two menus** — `new-workspace` /
  `name-workspace` live in `?`, `close-workspace` lives in `.`.
- **Genuinely global things sit in the "workspace" menu** — themes, plane zoom.
  The taxonomy is not internally consistent.
- `?`'s highest-value entries (jump to a workspace, toggle the jump panel) must
  work **while typing**, but leaders are gated off in insert mode — so they can
  never be leader-menu items regardless of scope; they must be direct chords.
  What remains of `?` is too thin to own a root key.

The tile/workspace/global distinction describes where a handler lives in the
code, not how the operator decides at the keyboard.

## Decision

**Collapse to two menus, organized by the object of the verb, not by scope.**

- **`space`** — verbs whose **object is the focused tile's App**. Polymorphic
  over App type: the mechanism is uniform, the vocabulary is per-App (document,
  session, listing, issue, graph, binding table).
- **`.`** — verbs whose **object is the shell**: tiles, workspaces, appearance,
  system. The former `?` menu folds in here.
- **`?` is retired** as a leader.

**The organizing test is shape + object, not frequency.** Every candidate action
is triaged by its *shape* first:

1. **Motion / continuous adjustment** (moves focus or a cursor, repeatable,
   reversible, changes no state) → **direct key**. Never a menu item — a menu is
   the wrong shape for a motion because it forces a reopen per repetition. This
   holds for a frequent motion and a rare one alike; frequency never enters.
2. **Verb** (a discrete, named state change in the surface's working vocabulary)
   → **menu**. *Which* menu is decided by the object: focused-App content or
   session → `space`; the shell → `.`.
3. **Escape hatch** (invoked by knowing its name — administrative, destructive,
   or one-off) → **`:` command line**. Rarity is a *consequence* of being an
   escape hatch, not the test.

Depth cap inside a menu: a top-level key for daily verbs, one submenu level for
grouped verbs, nothing deeper. A third level means the entry belongs in `:`.

## Rationale

- Frequency conflates *how often* with *what kind*. It gives the wrong answer on
  both a frequent motion (which is still not a menu item) and a monthly
  `rename-session` (which is still a first-class verb with no natural home key).
  Shape gives the right answer on both and is decidable by inspection, without
  usage stats.
- Two objects (the focused thing / the shell around it) is the distinction the
  operator actually feels. It maps to exactly two menus. Workspace-vs-global was
  a sub-distinction of the second object, and both halves are small enough to
  live under one root.
- This is a single-user tool; optimize for the owner's fluency and muscle
  memory, not newcomer discoverability. Two stable roots beat three with a
  recall tax on every use.

## Scope of this change (and what is deferred)

This ADR is realized in stages so nothing is stranded:

- **T1 (this change): menu reorganization.** Merge `?` into `.`. Regroup the
  Agent menu into flat verbs + a session-lifecycle submenu. Remove entries whose
  replacement already exists (`.` stop → Esc; `b` file-browser → Cmd-O) and the
  DOC duplicate-heading entry.
- **T2 (deferred): motions → direct keys.** Doc/edit motions currently exist
  *only* as menu entries (no direct binding). They stay in the menu until per-App
  nav keys are added, then the motion submenus are stripped.
- **T3 (deferred): the `:` escape-hatch tier.** No `:` command surface exists in
  the GPUI app yet (`command.rs` is TUI-only). Until it does, escape-hatch
  actions (`reset-all`, `toggle-heading-markers`) stay reachable in a submenu.

## Alternatives rejected

- **Keep three menus, redraw boundaries.** Rejected: the third root's only
  must-work-while-typing entries can't be leader items at all, leaving `?` too
  thin to justify a root key.
- **One mega-menu.** Rejected: the focused-App vocabulary is genuinely distinct
  per App type and would bloat a single tree; the object split is real.
- **Frequency-tiered menus.** Rejected: see rationale — frequency is a proxy that
  misclassifies both motions and rare-but-first-class verbs.

## Consequences

- `?` no longer opens anything; former `?` entries live under `.` (`w`
  workspace, `s` system submenus). Workspace jump stays on its existing
  `ctrl-1..0` direct bindings.
- The Agent menu shrinks from ~19 flat entries to ~6 flat verbs + `s`/`M`
  submenus.
- Going forward, a new action is placed by the shape+object test above; a menu
  slot is prime real estate and rarity does not earn one.
- `docs/specs/spec-menu-scopes.md` (which still documents the inverted
  Space=global / `.`=local model) is superseded by this ADR and the
  `UXI-Menu-*` invariants; it is left as a historical DRAFT with a pointer.
