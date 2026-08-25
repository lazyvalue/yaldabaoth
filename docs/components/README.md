# Component Specs

**Status:** LIVING — authoritative. The per-component home for what each part of
the app *is* and how it must *behave*.

## What this is

A **component spec** describes one component of the app — `Workspace`, `Tile`,
`AgentTile`, `TextEditing`, … — in one place: a prose description, the elements it
references, and its list of **UX invariants** (`UXI-<Component>-N`). It answers
"what is this component and what must always be true of it?"

This is a reframing of the old split between `docs/ux-invariants.md` (a flat global
`INV-UX-N` list) and `docs/specs/` (design docs). Instead of one giant invariant
file, **each component owns its invariants**, next to its description. See
**Migration** below for how the two coexist during the transition.

## Layout

```
docs/components/
  README.md              # this file — the model + conventions
  _template.md           # copy this to start a component spec
  common/                # shared elements/behaviors referenced by components
    README.md
    text-editing.md      # e.g. helix-style editing rules, referenced by any editable surface
  <component>.md         # a single-file component spec (Workspace, Tile, …)
  <component>/           # a DECOMPOSED component (when it's big/complicated)
    README.md            # the component index: description + references + the UXI list
    <facet>.md           # one facet's detail (e.g. agent-tile/sidepanel.md)
```

A component starts as a single `<component>.md`. When it grows too big for one
screen-and-a-bit, decompose it into a `<component>/` subdir: the `README.md` is the
index (description, references, and the authoritative UXI list), and each `<facet>.md`
holds the detail for a slice. The UXI ids stay owned by the component, not the facet.

## Index of components

- [agent-tile/](agent-tile/README.md) — `AgentTile` (decomposed): sidepanel,
  transcript, compose, recap, model, picker. `UXI-AgentTile-1..34` (tile-local
  session tags are `-33`; independent session/workspace state is `-34`).
- [buffer.md](buffer.md) — `Buffer` (Picking / Viewing / Editing).
- [linear.md](linear.md) — `Linear`.
- [cog.md](cog.md) — `Cog` (read-only Cog graph explorer tile). `UXI-Cog-1..11`.
- [jump-panel.md](jump-panel.md) — `JumpPanel`. `UXI-JumpPanel-1..25` (the
  sidebar navigator, its `Cmd-P` fuzzy palette `UXI-JumpPanel-9`, and the
  workspace-attached / tag-grouped Detached tree in `UXI-JumpPanel-23/-25`).
- [workspace.md](workspace.md) — `Workspace` (the infinite-plane model: signed
  all-directions slot grid + pan/semantic-zoom camera + reset-to-origin;
  layout-mode/split surface retired; attachment independent of visibility;
  workspace close detaches tiles and never quits). `UXI-Workspace-1..24`.
- [project.md](project.md) — `Project` (top-level org primitive: name+cwd-keyed
  store, workspaces/sessions hold a `ProjectId` FK, `Frame → Project → Workspace →
  Window`). `UXI-Project-1..8` — all implemented (ADR-0028).
- [rail.md](rail.md) — `Rail`.
- [keybindings.md](keybindings.md) — `Keybindings`. `UXI-Keybindings-1`.
- [system-console.md](system-console.md) — `SystemConsole`. A drop-down,
  persistent operational log and self-rebuild/relaunch surface.
- [agent-stats.md](agent-stats.md) — `AgentStats`. The singleton system tile for
  live agent summaries and repository-efficiency evidence. `UXI-AgentStats-1..4`.
- [common/](common/README.md) — shared behaviors: `TextEditing`, `Selection`,
  `TextZoom`, `Blockquote`, `ParagraphSpacing`, `Menu` (leader command panel),
  `Diagram` (inline mermaid rendering).

## Terminology (use these words)

The vocabulary component specs are written in. These are the user's words — prefer
them in specs, code comments, and UI copy over ad-hoc synonyms.

- **attached tile** — a durable tile owned by exactly one workspace. It is
  either **visible** in that workspace's layout or **hidden** while retaining
  its workspace association.
- **Detached tile** — a durable tile outside every workspace. It keeps its
  `WindowId`, App state, project, and tags; it is directly reachable from the
  jump panel and Cmd-P and can later be attached without recreation (ADR-0034).
- **empty Agent tile** — an Agent tile with no session selected (the picker).
  This replaces the old overloaded phrase “unbound Agent tile.”
- **solo tile presentation** — focusing a Detached or attached-hidden tile in
  the content area without changing attachment or visibility. It is navigation
  state, not an ephemeral workspace.

The old terms **bound tile**, **unbound tile**, **direct unbound view**,
**scratchpad**, **stash**, **free session**, and **bare agent view** are retired. Agent
sessions remain project-owned runtime entities, but every navigable roster
session is represented by one stable Agent tile, either Attached or Detached.

## Format of a component spec

Every component spec has three parts:

1. **Description** — prose. What the component is, its role, its states. Enough that
   a reader understands the component without reading code.
2. **References** — what it references from `docs/components/common/` (shared
   elements/behaviors) or other component specs / `docs/specs/` design docs /
   ADRs. Link, don't restate.
3. **UX invariants** — a numbered list of `UXI-<Component>-N`. Each one describes
   **a behavior, a visual element, or a sub-component**, and carries:
   - **Statement** — declarative present, testable ("clicking a tile body focuses
     that tile"), not a task ("add click handling").
   - **Applies to** — the surfaces + real code symbols (files/functions/structs).
   - **Why** — the problem it prevents (guards against accidental removal).
   - **Status** — `implemented` (built + guarded) · `partial` (some surfaces) ·
     `not implemented` (the contract, not yet built).
   - **Enforcement** — the named guard: a `verify_harness.rs` / `tests.rs` test, or
     (for a genuine paint/subprocess/timing gap) the human runtime check. An
     invariant with neither is explicitly a gap.

## The `UXI-<Component>-N` id

- `<Component>` is a PascalCase token: `Workspace`, `Tile`, `AgentTile`,
  `TextEditing`, `Browser`, `Linear`. One token per component spec.
- `N` is the next integer within that component; ids are **stable and append-only**
  — never renumber. A retired invariant is marked `superseded` / `removed`, not
  deleted, so cross-references (code comments, tests, ADRs) don't rot.
- Reference an invariant from code the same way as before: `// UXI-AgentTile-3`.

## When to write / extend one

- **New behavior on an existing component** → extend that component's UXI list
  (next `N`). Usually driven by `/new-ux`.
- **A whole new component** → copy `_template.md` to `<component>.md`.
- **Shared behavior used by ≥2 components** (text editing, caret containment,
  copy-on-select) → put it in `common/` and have each component **reference** it
  rather than duplicating the invariant.

## Migration (how this coexists with the old harness)

`docs/ux-invariants.md` (flat `INV-UX-N`) and the design specs in `docs/specs/` are
**not deleted**. They are migrated INTO component specs incrementally, as `/new-ux`
touches each component:

- Until an `INV-UX-N` is migrated, it **remains authoritative** where it lives.
- When a component spec absorbs it, the new `UXI-<Component>-N` becomes
  authoritative and the old entry is marked `→ migrated to UXI-<Component>-N`.
- `docs/specs/spec-*.md` stay as deeper design references; component specs link to
  them under **References**.
- The crosswalk of what has moved lives at the bottom of this file.

**Do not attempt a big-bang migration.** Move invariants as you work on their
component; the value is per-component locality, earned incrementally.

### Crosswalk (INV-UX-N → UXI-<Component>-N)

All 22 `INV-UX-N` entries have been migrated into component specs. The legacy
`docs/ux-invariants.md` is frozen (kept for the `INV-UX-N` code/test references that
still point at it); the component spec is now authoritative for each behavior.

| Old | New | Home |
|-----|-----|------|
| INV-UX-1 | UXI-TextEditing-1 | `common/text-editing.md` |
| INV-UX-2 | UXI-AgentTile-9 | `agent-tile/compose.md` |
| INV-UX-3 | UXI-AgentTile-4 | `agent-tile/transcript.md` |
| INV-UX-4 | UXI-AgentTile-5 | `agent-tile/transcript.md` |
| INV-UX-5 | UXI-AgentTile-1, -2 | `agent-tile/sidepanel.md` |
| INV-UX-7 | UXI-AgentTile-13 | `agent-tile/compose.md` |
| INV-UX-8 | UXI-AgentTile-10 | `agent-tile/compose.md` |
| INV-UX-9 | UXI-AgentTile-11 | `agent-tile/compose.md` |
| INV-UX-10 | UXI-JumpPanel-1 | `jump-panel.md` |
| INV-UX-11 | UXI-Workspace-1 | `workspace.md` |
| INV-UX-12 | UXI-AgentTile-3 | `agent-tile/sidepanel.md` |
| INV-UX-13 | UXI-TextZoom-1 | `common/text-zoom.md` |
| INV-UX-14 | UXI-Selection-1 | `common/selection.md` |
| INV-UX-15 | UXI-AgentTile-6 | `agent-tile/transcript.md` |
| INV-UX-16 | UXI-AgentTile-12 | `agent-tile/compose.md` |
| INV-UX-17 | UXI-Keybindings-1 | `keybindings.md` |
| INV-UX-18 | UXI-JumpPanel-2 | `jump-panel.md` |
| INV-UX-19 | UXI-AgentTile-8 | `agent-tile/transcript.md` |
| INV-UX-20 | UXI-AgentTile-15 | `agent-tile/recap.md` |
| INV-UX-21 | UXI-AgentTile-14 | `agent-tile/compose.md` |
| INV-UX-22 | UXI-AgentTile-16 | `agent-tile/model.md` |
| INV-UX-23 | UXI-AgentTile-7 | `agent-tile/transcript.md` |

_(INV-UX-6 was never assigned. Cross-references inside migrated prose still read
`INV-UX-N`; use this table to resolve them until a later pass rewrites them.)_
