# T3 — The `:` escape-hatch command line

**Status:** open (needs owner go-ahead — it is a new surface, not a menu edit)
**Depends on:** T1

## Goal

Give the GPUI app a `:` command line — the home for the escape-hatch tier in the
shape+object model (ADR-0032). Escape-hatch = actions invoked by knowing their
name: rare, administrative, or destructive.

## Context

`command.rs` is a TUI-only command registry; the GPUI surface has no `:` palette.
Until this exists, escape-hatch actions stay parked in submenus (T1 keeps
`keymap-reset-all` and `agent-toggle-heading-markers` reachable there).

## Subtasks

- [ ] Decide the surface: reuse the floating sigil-card overlay machinery with a
      text-input row, or a dedicated bottom command line.
- [ ] Wire dispatch to the existing `dispatch_menu_command` command-name space
      (names already exist; the palette just needs to route to them).
- [ ] Move `keymap-reset-all` and `agent-toggle-heading-markers` out of their
      menus into `:` only.
- [ ] Guard: `:` opens on the real screen; a typed command dispatches the real
      action; negative control.

## Open question for the owner

Is a `:` command line wanted, or should rare/destructive actions just live in a
deep submenu? Recommendation in ADR-0032: build `:` — a typed name is the right
shape for "I know the exact rare thing I want."
