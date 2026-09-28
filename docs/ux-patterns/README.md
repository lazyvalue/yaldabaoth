# UX patterns

**Status:** LIVING — authoritative.

The **universal UX laws**: rules that bind *every* surface in the app — every tile,
view, overlay, and input — without that surface having to reference them. Each is a
`UXP-N`. A change that violates one is wrong until the pattern is reconciled.

## Pattern, common component, or component invariant?

| If the rule… | It is a… | Lives in |
|---|---|---|
| binds every surface, with no surface named in the statement | **pattern** `UXP-N` | here |
| is a shared part/behavior that components opt into by reference (text editing, selection, menus) | **common component** `UXI-<Common>-N` | `docs/components/common/` |
| names a specific tile, surface, or app | **component invariant** `UXI-<Component>-N` | `docs/components/` |

Litmus: if the statement needs a tile's name, it is not a pattern. Promote a
component invariant to a pattern only when it is genuinely true of every surface
— this directory must stay short. A component invariant that *realizes* a pattern
on its surface cites the `UXP` in its References.

## Format

One file per pattern, `uxp-<N>-<slug>.md`, with the same fields as a component
invariant (see `docs/components/README.md` § Format): **Statement**, **Applies
to** (the mechanism every surface goes through), **Why**, **Status**,
**Enforcement**, plus **Realized by** (the component `UXI`s that implement it on
specific surfaces). Ids are stable and append-only.

## Index

- [UXP-1](uxp-1-caret-always-visible.md) — The caret is always visible, and moving it moves the visible text.
- [UXP-2](uxp-2-routing-implies-painting.md) — A keystroke routed to an input is painted (routing ⇒ painting).
- [UXP-3](uxp-3-no-stale-render.md) — A view re-renders whenever any input it reads changes (no stale render).
- [UXP-4](uxp-4-typing-cost-is-local.md) — Input on one surface never re-renders unrelated surfaces (cost is O(changed)).
- [UXP-5](uxp-5-global-shortcuts-land-everywhere.md) — Global shortcuts work from every screen and focus state.
