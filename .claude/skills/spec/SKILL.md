---
name: spec
description: Draft or revise a technical spec under docs/specs/ — explore approaches, present design sections, and write the spec file. Use when the user asks to draft, write, or revise a spec, or design a part of the system.
---

# Spec Drafter Agent

You are drafting or revising a technical specification for yalda.

## Checklist

You MUST create a task for each of these items and complete them in order:

1. **Explore project context** — check files, docs, recent commits
2. **Ask clarifying questions** — one at a time, understand purpose/constraints/success criteria
3. **Propose 2-3 approaches** — with trade-offs and your recommendation
4. **Present design** — in sections scaled to their complexity, get user approval after each section
5. **Write spec** — save to `docs/specs/{module}/spec-{name}.md` (or `docs/specs/spec-{name}.md` for top-level / cross-module specs)
6. **Spec self-review** — quick inline check for placeholders, contradictions, ambiguity, scope (see below)
7. **Adversarial review** — spawn a subagent via the Agent tool using the prompt at `.claude/skills/spec/adversarial-review.md`. Pass the spec path and the paths of any cross-referenced specs. Present the reviewer's checklist output to the user **verbatim**, alongside the spec path. Do not summarize, triage, or rebut it. Do not loop it back into the drafter — the user decides what to act on.
8. **User reviews written spec** — ask user to review the spec file and the reviewer's checklist. Stop there — yalda does not have a separate plan/implement flow; the user takes the approved spec to a coding session directly.

## Inputs

Always start by reading:

1. Existing specs in `docs/specs/` (including module subdirs) — understand current technical state and avoid contradiction. Pay attention to module overview specs (`spec-{module}.md`) — they define the module-level interface.
2. `docs/governance.md` — read in full. Spec authoring rules and the module/component concept live there.
3. `docs/architecture.md` — top-level system architecture and module map.
4. `CLAUDE.md` — project conventions and invariants.

The human describes what to spec; ask clarifying questions if scope or purpose is unclear. Yalda does not use PRDs — the human's description is the source of intent.

## Module overview vs component spec

A **module** is a major area of the application (e.g., `editor`, `render`, `claude-channel`); a **component** is a part of a module (e.g., wrap math inside `editor`, or the markdown highlighter inside `render`). See `docs/governance.md` § Modules and Components for definitions.

Decide which shape applies before drafting:

- **Module overview spec** (`docs/specs/{module}/spec-{module}.md` at the root of a module subdir) — defines the module-level interface. The Interfaces section MUST include all three required subsections (API surface, events / messages, data ownership). Mark "None" explicitly when a subsection doesn't apply; absence must be explicit.
- **Component spec** (any spec inside a module that's not the overview) — defines a single component within the module. Interfaces is flat; no required subsections.
  - **Components own data; data-owning components SHOULD publish a module-internal API in their component spec.** Other components in the same module call this API rather than reading or writing the owning component's tables directly. The Interfaces section is where this API is published — list the read and write functions other components in the module call into. Mark each entry as *external* (called by other modules) or *module-internal* (called by other components in this module) when the distinction is load-bearing for a reader.

A new module starts with an overview spec. Component specs come second.

## Process

**Understand the idea and scope:**

- Read all input documents before producing anything.
- Check out current project state: files, recent commits, etc.
- Determine if this is a new spec or a revision. If an existing spec covers this area, revise it rather than creating a duplicate. Ask the human if unclear.
- Before asking detailed questions, assess scope: if the request describes multiple independent subsystems, flag this immediately. Don't spend questions refining details of a project that needs to be decomposed first.
- If the project is too large for a single spec, help the user decompose into sub-specs: what are the independent pieces, how do they relate, what order should they be built? Create subspec stubs (summary + "stub" note); focus the current session on the first one.

**Exploring approaches:**

- For appropriately-scoped projects, ask questions one at a time to refine the idea.
- Only one question per message - if a topic needs more exploration, break it into multiple questions.
- Propose 2-3 different approaches with trade-offs
- Present options conversationally with your recommendation and reasoning
- Lead with your recommended option and explain why

**Presenting the design:**

- Once you believe you understand what you're building, present the design
- Scale each section to its complexity: a few sentences if straightforward, up to 200-300 words if nuanced
- Ask after each section whether it looks right so far
- Cover: architecture, components, data flow, error handling, testing
- Be ready to go back and clarify if something doesn't make sense

**Design for isolation and clarity:**

- Break the system into smaller units that each have one clear purpose, communicate through well-defined interfaces, and can be understood and tested independently
- For each unit, you should be able to answer: what does it do, how do you use it, and what does it depend on?
- Can someone understand what a unit does without reading its internals? Can you change the internals without breaking consumers? If not, the boundaries need work.
- Smaller, well-bounded units are also easier for you to work with - you reason better about code you can hold in context at once, and your edits are more reliable when files are focused. When a file grows large, that's often a signal that it's doing too much.

**Writing the spec:**

3. **Write declaratively.** Describe what the system *is* or *will be* — its behaviors, data model, interfaces, and constraints. Do not write implementation steps.
4. **Use Builds On with WHY/HOW.** List the specs this spec depends on. For each, one sentence on WHY it's relevant and HOW this spec interacts with it. The HOW sentence is what prevents the section from decaying into a bibliography.
5. **Section order is fixed:** Builds On → Overview → Behaviors → Data Model → Interfaces → State Machine (optional) → Constraints → Revision History. Do not reorder. See `docs/governance.md` § Spec Authoring.
6. **Introduce named entities in Overview, not Behaviors.** If the spec describes multiple named entities (state machines, endpoints, types), list them in Overview before any later section references them. Forward references are a structural failure — readers shouldn't meet "Machine 1" in Behaviors when Machine 1 hasn't been introduced yet.
7. **No Components subsections.** If a spec feels like it has multiple components, it's two specs. Force fan-out into sibling specs in the same module rather than dumping them under a single Components heading.
8. **Distinguish current state from target state.** Use status markers (SHIPPED, ACTIVE, DRAFT) on bullets / subsections so an agent reading the spec knows what's built vs. proposed.
9. **Lean.** Target 1–2 pages. If the spec runs longer, that's a signal it should split into sibling specs (or a module overview + components). Multi-machine, multi-entity specs may be longer; that's defensible only when the entities are part of one coherent contract.

## Output

### New spec

- **Module overview spec:** write to `docs/specs/{module}/spec-{module}.md`.
- **Component spec inside an existing module:** write to `docs/specs/{module}/spec-{name}.md`.
- **Top-level spec (no module):** write to `docs/specs/spec-{name}.md`. Prefer placing in a module unless the spec is genuinely cross-module foundational work.

Mark status as DRAFT. The spec is not final until a human reviews and approves it.

If this is a new module, also update `CLAUDE.md` to add the module to the index.

### Revised spec

Edit the existing spec file in place. Add or update sections as needed. Update the status, the Last updated date, and the Revision History.

### In either case, the spec must include:

- A `Builds On` section linking other specs with WHY/HOW per item (or omitted entirely if no dependencies).
- A clear `Behaviors` section that names the contract a reader needs to use this module/component correctly.
- Status markers (SHIPPED, ACTIVE, DRAFT) on bullets or subsections that distinguish current from target state.
- For module overview specs: all three required Interfaces subsections.

## Constraints

- Do not prescribe implementation steps. A spec says "the system uses X" not "first, install X, then configure Y."
- Do not invent requirements. If you think something is missing from the human's description, ask rather than filling the gap yourself.
- Do not duplicate content from other specs. Reference them in Builds On instead.
- Follow naming and module conventions from `docs/governance.md`.
