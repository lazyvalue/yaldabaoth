---
name: plan
description: Create or extend a multi-session project under docs/projects/ — a project.md holding standing context plus numbered tickets (specific tasks with subtask checkboxes), and mirror the work into the session task list. Use when starting an effort that spans sessions, breaking a large piece of work into tracked tickets, or when the user says "plan", "make a project", "make a ticket", "track this", or asks to capture a multi-stage plan durably.
---

# Plan — projects + tickets

You are scaffolding (or extending) a durable, cross-session project record under
`docs/projects/`. This is the record that survives context loss; the in-session
task list (TaskCreate) is its live mirror. This skill **records a plan — it does
not implement it.**

## Structure

```
docs/projects/<project-slug>/
  project.md             # standing context for the whole project (the umbrella)
  001-ticket-<slug>.md   # one specific, actionable task
  002-ticket-<slug>.md
  ...
```

- **`project.md` — context, not a task.** Why this project exists and the shared
  understanding every ticket assumes: problem / root cause, goals, scope (in &
  out), the model or key decisions, links (specs, ADRs, branches), and a **status
  table of its tickets**. A fresh agent should be able to read this cold and
  understand the whole effort.
- **`NNN-ticket-<slug>.md` — one actionable unit of work.** Its specific goal,
  the approach/decision, **subtasks as `- [ ]` checkboxes** (with per-subtask
  status + blockers), acceptance/verification, and links back to `project.md` and
  any specs. Numbered monotonically; never renumbered.

Live on `main` so they're durable regardless of feature branches.

The litmus test for where something goes: if you're writing **"why" or "the
model"**, it's `project.md`; if you're writing **"do X, then Y, verify Z"**, it's
a ticket.

## Steps

1. **Scope it.** Read the user's intent plus relevant context — existing
   `docs/projects/`, related specs, recent commits. Ask 1–2 clarifying questions
   ONLY if genuinely blocked, in prose (never a chooser tool).
2. **New project or existing?** `ls docs/projects/`. If this work belongs to an
   existing project, skip to step 4 and add a ticket (and update its `project.md`
   tickets table).
3. **Create the project.** `docs/projects/<slug>/project.md` with sections:
   **Status**, **Problem / Why**, **Goals**, **Scope** (in / out), **Model or Key
   decisions**, **Links**, **Tickets** (status table).
4. **Create ticket(s).** `NNN-ticket-<slug>.md` at the next free number. One
   ticket = one coherent deliverable. If it sprawls across unrelated concerns,
   split into multiple tickets. Each ticket gets subtask checkboxes.
5. **Mirror into the session task list.** One `TaskCreate` per ticket subtask;
   set `addBlockedBy` to match real dependencies. As work lands, keep them in
   sync — tick the ticket checkbox AND complete the task.
6. **Confirm.** Show the user the project/ticket paths and the subtask list.

## Conventions

- A ticket that grows a tail of newly-discovered work spawns a **new ticket
  (NNN+1)**, not silent scope creep in the current one.
- Update `project.md`'s tickets table whenever a ticket opens, advances, or
  closes.
- Keep ticket subtasks and the session task list in lock-step; a stale mirror is
  worse than none.
- Commit project/ticket files to `main` (durability is the whole point); push if
  the user wants cross-machine survival.

## Relationship to other skills

- `/spec` writes the design (the *what*) under `docs/specs/`; a ticket links to
  it. `/decision` records an ADR (the *why* of one fork). `/worklog` logs what
  happened in a session. `/plan` is the umbrella that threads these together for
  work too big for one session — it points at the specs/ADRs and tracks the
  tickets that execute them.
- `/cog-plan` is the executable dependency graph for approved multi-step work;
  `/cog-execute` drives that graph. The project/ticket record remains durable
  product context, while Cog is authoritative for live execution state.
