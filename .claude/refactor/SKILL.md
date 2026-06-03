---
name: refactor
description: Review a section of the codebase and produce a ranked, implementation-ready report of refactor proposals — reusable abstractions, tighter interfaces and module boundaries, domain types that encode invariants, inefficient algorithms, anti-patterns, concurrency and transaction hazards, and non-terminating-loop / idempotency hazards — judged against Fulcrum's functional, strongly-typed design philosophy. Use when the user asks to refactor, audit, or review the design/quality of an area of code (not a diff). Report-only; the user implements selected findings afterward.
---

# Refactor

You review **existing** code — a section the user names — and produce one ranked report of refactor proposals. You do not edit code in this skill. The user reads the report and then tells you which findings to implement; that implementation is separate, tracked work (`/plan` or `/implement`).

This is design-level review, not diff review. It is distinct from `/code-review` (diff-scoped, bug-focused) and `/simplify` (mechanical cleanup of changed code).

The work runs as a Workflow: one finder per lens in parallel, then one adversarial verifier per lens that batch-reviews all of that lens's findings at once, then a synthesizer that dedups and ranks. Cost scales with the number of lenses (each lens is one finder plus one verifier), not with the file count — so lens selection is the cost lever. The canonical script is `workflow.js`, colocated in this skill directory. Your job in the main loop is the parts the workflow cannot do for itself: **resolve the scope, agree the lenses with the user, launch the workflow, and land the report.**

## Inputs

The user invokes `/refactor <scope>`. The scope is free-form:
- a path or glob — `services/lever/src/ingest/`
- a module name — `ingest`, `context-engine` (maps to `docs/specs/{module}/` and its code dirs)
- a concept / cross-cutting question — `"database writes in queue handlers"`, `"all of ingest"`

If no scope is given, ask one question: "What should I review? A path, a module, or a concept like 'db writes in queue handlers'." Do not guess.

## Process

### 1. Resolve the scope to a work-list

Turn the free-form scope into a concrete set of files plus a one-paragraph map.

- **Path / glob** → enumerate the matching source files. Skip generated code, vendored deps, lockfiles, and migrations unless the scope is explicitly about them.
- **Module name** → read `docs/specs/{module}/` to find the module's code roots, then enumerate those.
- **Concept / cross-cutting** → run a discovery sweep. Use `Grep` for the obvious signals and, when the concept spans naming conventions or multiple locations, dispatch an `Explore` subagent ("very thorough") to find the relevant files and call-sites. Collect the union.

Produce three things:
- `files`: repo-relative paths (the work-list).
- `notes`: one paragraph describing what is in the scope and how the pieces relate — this orients every finder.
- `migrationActive`: true if the scope touches the Python↔Rust boundary (`services/workers`, PyO3 bindings, or logic that has both a Python and a Rust implementation). When true, every finding is scored for migration impact.

Honor the breadth the user asked for — a narrow path stays narrow, a whole-tree scope stays whole-tree. Do not force-narrow a large scope. The file list orients the finders' starting point; they read outward to callers and specs regardless, so breadth shapes the report's framing more than its cost. Cost is governed by the lens count at step 2, not the file count — that is the throttle, so there is no need for a separate "whole-tree" mode. For a very large scope, still report the file count and rough LOC at the step-2 gate so the user sees what they pointed at.

### 2. Propose lenses and confirm — always ask

There are twelve lenses. Cost scales with how many run (one finder plus one batch-verifier per lens), so this step is also the cost gate. **Always present the menu and ask the user — never auto-select and launch.**

Classify the scope and propose a default of **five high-value lenses**:

| Scope shape | Default five |
|---|---|
| Queue / worker / pipeline | `mutation_authority`, `state_lifecycle`, `concurrency_resources`, `liveness_termination`, `observability` |
| Domain model / data layer | `type_modeling`, `mutation_authority`, `state_lifecycle`, `abstraction_reuse`, `interfaces_coupling` |
| API / boundary surface | `type_modeling`, `interfaces_coupling`, `effects_purity`, `mutation_authority`, `anti_patterns` |
| Algorithm / compute-heavy | `algorithms`, `type_modeling`, `effects_purity`, `abstraction_reuse`, `anti_patterns` |
| General / mixed | `architecture`, `type_modeling`, `abstraction_reuse`, `interfaces_coupling`, `anti_patterns` |

The user's own phrasing overrides the default — `"audit db writes where queueing is involved"` points at `{mutation_authority, concurrency_resources, liveness_termination, observability}`.

Present, **inline in chat (not via a prompt tool)**: the resolved work-list (file count, rough LOC), `migrationActive`, the proposed five lenses each with a one-line rationale, and the remaining seven as an à-la-carte menu. State the cost: roughly `2 × lenses + 1` agents, and a token band (about 300-500k tokens for five lenses, scaling with lens count and how many findings each surfaces). Ask: run the five, add specific lenses, or take the whole menu?

Wait for the answer. Accept any subset or all twelve. The chosen set becomes the `lenses` arg. All twelve lens keys: `architecture`, `mutation_authority`, `state_lifecycle`, `effects_purity`, `type_modeling`, `abstraction_reuse`, `interfaces_coupling`, `concurrency_resources`, `liveness_termination`, `algorithms`, `observability`, `anti_patterns`.

### 3. Launch the workflow

Resolve the absolute path of `workflow.js` in this skill directory, then launch:

```
Workflow({
  scriptPath: "<this-skill-dir>/workflow.js",
  args: { scopeLabel, files, lenses, migrationActive, notes }
})
```

The workflow runs in the background and returns the report body (markdown) as its result. Scale follows the lens count (one finder plus one batch-verifier per lens); the file list constrains where finders start but not the agent count.

### 4. Land the report

When the workflow completes:
1. Write its returned markdown verbatim to `docs/research/refactor-review-{slug}.md` (slug derived from the scope label — lowercase, non-alphanumeric → `-`). This follows the existing `docs/research/` precedent for analysis artifacts.
2. In chat, print the ranked one-line list of findings (rank, title, lens, effort, confidence) and the report path.
3. End with: "Say which findings to implement (e.g., 'do 1, 3') and I'll turn them into tracked work."

Do not start implementing. The user picks.

## The finding contract

Every finding the report carries has this shape (the workflow enforces it):

```
Finding · Location (file:line) · Lens · Evidence · Invariant violated/absent ·
Risk · Refactor move · Why architecturally better · Migration impact ·
Enforcement hook · Effort (S/M/L) · Confidence
```

The **enforcement hook** is load-bearing: the artifact that keeps the improvement true over time — a type that excludes the bad state, a `@pytest.mark.invariant` test, an import-linter contract, a pyright-strict boundary, a schema/DB constraint, or a new `INV-…` EARS invariant. A finding that cannot name a hook is treated as taste: kept, but flagged and down-ranked, not dropped.

Findings are ranked by `(provability/simplicity impact × confidence) ÷ effort`. A change that makes a class of bug unrepresentable, or removes a non-termination / data-race hazard, outranks a local cleanup.

## Constraints

- **Report-only.** This skill never edits code. Selected findings are implemented as separate, tracked work after the user chooses.
- **Lens selection is always confirmed with the user.** Propose five high-value lenses for the scope, but never auto-launch — the user accepts, adds à la carte, or takes the whole menu. This is the cost gate; state the agent/token estimate when proposing.
- **Guard against false dedup.** "These look similar, merge them" is a defect when the call sites are different concepts that should evolve independently. The `abstraction_reuse` lens and its verifier default to *not* merging unless the sites are the same concept that changes together.
- **Verify, don't just collect.** Findings the verifier judges simply wrong are dropped. Taste-leaning ones survive but are flagged. This is annotation and filtering, not a harsh kill gate.
- **Migration awareness is conditional.** Only score migration impact when the scope is actually in play for the Python → Rust migration.
- **Specs are first-class.** If a finding implies a contract, ADR, or shared-interface change, the report must say so, and the eventual implementation task list must include the spec/PRD edit (CLAUDE.md rule).
