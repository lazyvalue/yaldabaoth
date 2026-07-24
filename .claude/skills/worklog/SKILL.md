---
name: worklog
description: Write a session worklog entry (what was built, what's open/unresolved, decisions made) and update the backlog — higher fidelity than git messages. Use at the end of a work session, when the user asks to "log", "checkpoint", "wrap up", or capture session state before context is lost.
---

# Worklog

Capture a session as a durable record so the next session (or agent) can pick up
without re-deriving state from git. Worklog = the *past* (what happened);
`docs/backlog.md` = the *future* (what's open); `docs/decisions/` = the *why*.

## Process

1. **Gather the session's reality** — don't guess. Check:
   - `git branch` / `git worktree list` and `git log --oneline` per touched branch.
   - What actually built/tested (re-verify "it's green" claims if unsure).
   - What's runtime-verified vs only unit-tested (be honest — this is the
     highest-value field; the GPUI app can't be driven headlessly).
   - Decisions made this session that deserve an ADR (offer to run `/decision`).
2. **Write the entry** to `docs/worklog/YYYY-MM-DD-<slug>.md` using
   `docs/worklog/template.md`. Slug = a few words on the session's theme.
   Convert relative dates to absolute. One line per branch with its commit.
3. **Update `docs/backlog.md`** — move finished items out, add new open/deferred/
   flagged ones with status (`IN-FLIGHT`/`READY`/`DEFERRED`/`NEEDS-DECISION`/
   `NEEDS-RUNTIME`) and a reason. Cross-link ADRs.
4. **Link, don't duplicate** — point at ADRs for rationale and the backlog for
   open work; the worklog narrates, it doesn't restate them.

## Constraints

- Faithful over flattering: record what failed, what was skipped, what's
  unverified. A worklog that only lists wins is useless for recovery.
- Commit the files when done — no need to ask (push still needs an explicit ask).
- Keep it scannable — statuses, commit shas, one-liners. Not prose walls.
