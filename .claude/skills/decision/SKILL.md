---
name: decision
description: Record a design decision as an ADR (context, options considered, decision, rationale, alternatives rejected, consequences) in docs/decisions/. Use when a non-obvious design choice is made or about to be locked in — when the user says "let's go with X", "we decided", picks between approaches, or when a spec/plan resolves a fork. Captures the "why" before it evaporates into chat history.
---

# Decision (ADR)

A spec says *what* we're building; an ADR says *what path we chose and why* — the
reasoning that should survive so it isn't relitigated next session. Lightweight,
one per decision.

## When to write one

- A fork was resolved (e.g. move-default vs also-show; rename strings vs type).
- An alternative was deliberately rejected and someone might later wonder why.
- Something was deferred for a non-obvious reason (so it isn't "discovered" as a
  gap and re-debated).
- A constraint was found that shapes future work (e.g. "the buffer pool is dead
  code") — though a pure factual gotcha may belong in agent memory instead.

## Process

1. **Number it.** Next `NNNN` after the highest in `docs/decisions/` (zero-padded).
2. **Write** `docs/decisions/NNNN-<slug>.md` from `docs/decisions/0000-template.md`:
   Status, Date (absolute), Related (specs/ADRs), Context, Decision, Rationale,
   Alternatives rejected, Consequences.
3. **Be specific in Alternatives rejected and Consequences** — those are the
   load-bearing sections. "Why not X" and "what this now forces/enables" are what
   readers actually come back for. Note any new invariant or backlog item created.
4. **Cross-link** the spec it implements and any ADR it supersedes/relates to.

## Constraints

- One decision per ADR; keep it to ~a screen.
- ADRs are append-only history: don't rewrite an accepted one — supersede it with
  a new ADR and set the old one's Status to "Superseded by ADR-XXXX".
- Don't commit unless asked.
