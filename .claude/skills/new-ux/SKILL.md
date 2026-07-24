---
name: new-ux
description: Capture a new UX / behavioral requirement, remove all ambiguity, check whether it's already planned or built, spec it as a component UX invariant (UXI-<Component>-N), implement, and reconcile the spec with what actually shipped. Use when the user hands over a new behavior a tile/view/surface must have ("clicking a tile should focus it", "on restart prompt to resume").
---

# New UX

The front door for a fresh behavioral requirement. It captures the requirement,
interrogates it to zero ambiguity, routes it into the **component spec** harness
(`docs/components/`), implements it, and reconciles the spec with reality. It does
not replace `/spec` (deep design), `/decision` (rationale), or `/plan` (multi-session
decomposition) — it invokes them when a step calls for it.

Read `docs/components/README.md` before starting — it defines the component-spec
format, the `UXI-<Component>-N` id scheme, and the migration rules.

## Checklist

Create a task for each; complete in order. Do NOT skip the interrogation or the
already-built check — they are what stop wasted or duplicate work.

1. **Capture verbatim.** Restate the requirement in the user's own terms so there's
   a durable record before anything else. Add a `docs/backlog.md` entry now (status
   `NEEDS-DECISION` while ambiguous, else `READY`).
2. **Interrogate to zero ambiguity (spec-style).** Ask clarifying questions **one at
   a time**. Do not proceed while any behavior is undefined. Pin down: the exact
   trigger, every surface it applies to, edge/empty/error states, what must NOT
   change, and the observable success criterion (how you'd test it). Surface any
   embedded fork (e.g. resume-prompt vs auto-resume) and — if a real choice is being
   made — offer `/decision` to record the why.
3. **Check if already planned or built — in code AND in specs.** Search the code for
   the behavior (it may already exist, like tile click-focus did) and search
   `docs/components/`, `docs/ux-invariants.md`, `docs/specs/`, `docs/backlog.md`,
   `docs/projects/` for an existing UXI / spec / ticket. Report one of: *already
   built* (then this is a verify or bug, not new work — consider `/bug`), *planned
   not built*, or *net-new*. Reuse/extend before creating.
4. **Write the spec.** Identify the owning component (`Workspace`, `Tile`,
   `AgentTile`, `TextEditing`, …). Add or extend its component spec under
   `docs/components/` with a new `UXI-<Component>-N` at status `not implemented`
   (Statement / Applies-to / Why / Enforcement-seam-named). Shared behavior →
   `docs/components/common/`. Big component → decompose into a `<component>/` subdir.
   Name the enforcement seam now (reducer / layout-probe / simulate-keystrokes), per
   `CLAUDE.md` § Verification harness.
5. **Implement.** Do the work (worktree per `CLAUDE.md` if substantial). Ship the
   headless guard test the UXI named, and **observe it RED with the fix reverted**
   (negative control — mandatory, per the anti-circling rules). Run build + full
   suite.
6. **Reconcile the spec with reality.** Flip the UXI status `not implemented → implemented`, fill
   the real test name in Enforcement, and record any **deviation from plan** (what
   you intended in step 4 vs what actually shipped, and why). Update the backlog
   entry (`NEEDS-RUNTIME` / done) and the crosswalk in `docs/components/README.md` if
   an `INV-UX-N` was migrated.

## Constraints

- **No ambiguity survives step 2.** If you can't write the success-criterion test,
  the requirement isn't specified yet — keep interrogating.
- **A UXI without enforcement is a gap** — every shipped invariant names its test or
  the specific genuine runtime gap.
- Prose interrogation only — walk the user through decisions conversationally; do not
  use a choice/question tool.
- Commit when the work is verified (builds + tests + observed-RED negative
  control) — no need to ask. Push still needs an explicit ask.
