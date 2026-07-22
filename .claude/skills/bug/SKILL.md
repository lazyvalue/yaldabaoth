---
name: bug
description: Fix a bug with memory — check the bug manifest for prior attempts so we don't repeat a failed approach, record context + planned fix, solve it, and append a timestamped log of the actual fix. Use when the user reports something broken/misbehaving, or asks to fix/debug/investigate a defect.
---

# Bug

Fixing a bug in this app has a failure mode: attacking it the same way it was
attacked before and getting the same non-fix — the app has a documented history of
bugs "fixed" 15+ times (see the chatbox-caret saga). This skill gives bug-fixing a
**memory**: every bug has one file with a growing, timestamped log of what was tried,
read before the next attempt.

Read `docs/bugs/bug-manifest.md` and `docs/bugs/_template.md` first.

## Checklist

Create a task for each; complete in order.

1. **Check the manifest.** Read `docs/bugs/bug-manifest.md`. Is this bug (or a close
   relative) already listed?
2. **If listed — read its history.** Open `docs/bugs/bug-<NNNN>-<slug>.md` in full,
   especially **"Approaches already tried (do NOT repeat)"** and the **Log**. Your
   new approach must be *different* from the ones that didn't hold — explicitly say
   how it differs. Set status `IN-PROGRESS` / `RECURRED`.
3. **If not listed — open a new bug file.** Copy `_template.md` to
   `docs/bugs/bug-<NNNN>-<slug>.md` (next zero-padded id). Fill Symptom, Context /
   root cause, Planned solution. Add a row to the manifest table.
4. **Localize before fixing (anti-circling).** Find the REAL entry point the user's
   action runs — not a hand-built proxy. If you can't make a test fail without the
   fix on that real path, you have NOT localized it; say so rather than shipping a
   guess. (`CLAUDE.md` § The anti-circling rules is mandatory here.)
5. **Solve it.** Implement the fix. Ship a headless guard on the real path and
   **observe it RED with the fix reverted** (negative control — mandatory). Assert on
   paint/behavior, not just state, for visibility/render bugs. Build + run the suite.
6. **Log the actual fix (timestamped).** Append a dated entry to the bug file's
   **Log**: what changed (files/commit), how verified (test + negative control),
   outcome. Move the tried-approach into "Approaches already tried" if it's a partial
   or it later regresses. Update the manifest row (status, times-addressed++).
7. **Commit the fix (automatic).** Once the suite is green and the negative control
   was observed RED, commit WITHOUT waiting to be asked — the fix, its guard test,
   and the bug-file + manifest updates in one commit. Stage only the files for THIS
   bug (a shared working tree may hold a parallel session's work — never `git add -A`
   blindly; stage the specific paths and confirm the diff is yours). If not already
   on a branch and the change is substantial, branch first per `CLAUDE.md`. Message:
   `fix(<area>): <what> (bug-<NNNN>)`, ending with the `Co-Authored-By` trailer. Do
   NOT push unless asked. If the guard is RED, the negative control was skipped, or
   you could not localize on the real path, do NOT commit — report instead.
8. **If it recurs later** — never rewrite the old log. Append a NEW dated entry at
   the bottom and flip status to `RECURRED`.

## Constraints

- **Different bug, different attempt.** Repeating a logged failed approach without a
  concrete reason it's different this time is the thing this skill exists to prevent.
- **Negative control is not optional** — a test that passes with the fix reverted
  guards nothing. One `cargo test` run proves it fails for the right reason.
- **The fix must reach the user** — on `main` + rebuilt binary, not stranded on a
  branch (`CLAUDE.md` anti-circling rule 5).
- Faithful over flattering in the log: record what failed and what's unverified.
- **Auto-commit a verified fix** (step 7) — a green suite + an observed-RED negative
  control is the bar; clearing it means commit without asking. A guess (couldn't
  localize on the real path, or no negative control) is never committed. Push still
  needs an explicit ask. This replaces the old "don't commit unless asked" default:
  leaving verified fixes uncommitted in a shared tree let a parallel session commit
  the guard test without its fix (HEAD red) and stranded work mid-session.
