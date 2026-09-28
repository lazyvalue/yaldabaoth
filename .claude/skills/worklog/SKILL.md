---
name: worklog
description: Write a short session worklog entry (what shipped, caveats, decisions, the Cog graph id + final render) and prune the backlog. Use at the end of a work session, when the user asks to "log", "checkpoint", "wrap up", or capture session state before context is lost.
---

# Worklog

A worklog is the human-readable **index card** for a finished piece of work. The
Cog graph is the full record (per-node outputs, notes, deviations) — the worklog
points at it; it does not restate it. `docs/decisions/` holds the *why*;
`docs/backlog.md` holds only what's waiting on Scott.

## Process

1. **Gather reality — don't guess.** `git log --oneline` on the merged branch;
   `cog graph status <id>` (must be `complete`) and
   `cog graph render <id> --frontiers`. Re-verify any "it's green" claim you're
   unsure of.
2. **Write** `docs/worklog/YYYY-MM-DD-<slug>.md` from `docs/worklog/template.md`:
   - **Shipped** — one line per deliverable, each with the verifying test/command.
   - **Caveats** — anything unverified (`NEEDS-RUNTIME` + which harness gap),
     skipped, or failing. Faithful over flattering. "None" if none.
   - **Decisions** — ADR one-liners (offer `/decision` for an unrecorded one);
     deferred ideas go to a Cog bulletin, cited here by address.
   - **Cog** — `Status: complete` + the final frontier render.
   No build narrative, no per-node output dumps — those live in the graph.
3. **Backlog** — add a `NEEDS-RUNTIME` / `NEEDS-DECISION` entry to
   `docs/backlog.md` only if Scott must act; remove entries this work confirmed.
   Nothing else goes there.
4. **Validate** with `scripts/check-cog-worklog.sh <worklog>`.
5. **Commit** — no need to ask (push still needs an explicit ask).
