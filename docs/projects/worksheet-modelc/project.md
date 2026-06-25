# Project: Worksheet Model C — uncorruptible ordering

**Status:** in progress (worktree `worksheet-modelc`, branch `worksheet-modelc`)
**Started:** 2026-06-24
**Supersedes:** the old shared-rope worksheet machinery and the open
`worksheet-frozen-blocks` floor tickets (F1 etc. — deleted, not fixed).

## Problem / why

Worksheet mode corrupts conversation ordering: a turn's continuation/replay
content lands **mid-document** (above content that is newer than it), so a newer
agent turn renders above an older exchange. "Fixed" 15+ times; it keeps
regressing.

**Root cause (architectural, not a stray bug).** The old worksheet uses **one
flat mutable `Document` rope** to represent **two different things at once**:

1. the immutable, ordered conversation log (frozen lines + `TurnId` tags via
   `line_anchors`/`line_metadata`), and
2. the live, mutable user draft.

These are reconciled by **position-based heuristics** — `frozen_lines`,
`lockable_through_line`, `last_llm_line`/`last_llm_open`,
`agent_tail_floor_char`, `find_llm_insertion_point` — with **no single ordering
invariant** and **no test enforcing one**. Order is implicit (whatever order
lines happen to sit in the rope, re-derived from tags at render). Insertion is
by **character offset, not identity**, so a stale tag or floor puts bytes in the
wrong place *permanently*. Resume replays the whole history back through this
same mutate-a-shared-rope path. Every prior fix is a local patch; nothing pins
the invariant, so it always comes back.

## The model (Model C — the durable fix)

There is **one** model: a **read-only, append-only transcript** + a **separate
`Compose` draft buffer**. Worksheet vs Chatbox is *not* two models — it is one
model rendered at two **placements**:

- `Placement::Inline` (worksheet): `Compose` rendered flush below the transcript,
  in conversation typography, with the presence-driven "You" divider as boundary.
- `Placement::Pinned` (chatbox): `Compose` in a boxed control at window bottom.

Design source of truth: `docs/projects/worksheet-redesign/design-c.md` (the
complete prior design + 3-reviewer round-2 findings). This project ports that
design onto current `main` (the prior `worksheet-redesign` branch implemented it
but went 27 commits stale and was never merged).

## The ordering invariant (the spine — pin it with a test)

> **INV-ORDER.** The transcript is append-only and read-only; the draft is a
> separate buffer. The only cross-buffer transfer is **text**, never a position.
> ⇒ a turn's chunks can only extend the transcript at EOF; a draft is never
> inside history; replay rebuilds the transcript in event order. Ordering
> corruption is **unrepresentable**, not handled.

This subsumes design-c's INV-1…INV-4. The keystone deliverable is a **headless
replay test** that asserts it (chronological order after stream + reconnect;
draft never in history) — the test that was missing for every prior regression.

## What this deletes (the fragile machinery)

`agent_tail_floor_char`, `append_llm_chunk_floored`, the worksheet per-line
freeze (`commit_worksheet_turn`), `submit_worksheet`, the worksheet→transcript
key routing, and the `should_follow_tail` worksheet `cursor_at_eof` arm. The
recent floor/freeze/F2/C3 fixes were patches on this machinery; Model C removes
the machinery, so they go with it. Streaming appends at transcript EOF via
`find_llm_insertion_point` only.

## What this must preserve (orthogonal recent work — do NOT revert)

Jump panel, universal `AgentRoster`, typed `WorkspaceCwd`/live agent cwd,
session-recall integrity (typed `/clear` boundary + `PromptRejected` drain), and
the **presence-driven "You" divider** (it becomes the Inline-placement boundary
marker between read-only transcript and the inline compose). All live on `main`
and post-date the Model C branch base, so this is a re-derivation on `main`, not
a merge of the stale branch.

## Tickets

| # | Ticket | Status |
|---|--------|--------|
| M1 | InputSurface struct + Compose rename | ✅ done |
| M2 | unify submit_compose; delete rope machinery | ✅ done |
| M3 | read-only transcript call-site fixes (paste/copy/status/follow-tail/seqs) | ✅ done |
| M4 | compose_draft persistence (4-site) | ✅ done |
| M5 | inline compose render + "You" divider | ✅ done |
| M6 | keystone ordering/replay invariant test + port suite | ✅ done |
| M7 | build + full test + worklog/ADR + merge | ✅ done |

**Outcome:** re-derived onto `main` via a conflict-surfacing 3-way merge (11
hunks). 256 gpui + 142 lib tests pass. INV-ORDER pinned by
`inv_order_*` tests (drive the real floor path; would fail against the old
shared-rope model). See ADR-0024 + `docs/worklog/2026-06-24-worksheet-modelc-ordering.md`.
Runtime-unverified (GPUI headless gap) — see the worklog's human-check list.

## Definition of done

`cargo build` + `cargo test` green; the INV-ORDER replay test passes and would
**fail** against the old shared-rope path; chatbox regression tests unchanged and
green; recent orthogonal work intact; worklog + ADR written. Runtime-unverified
is expected (GPUI can't be driven headlessly) — flag the human runtime check
(enter worksheet, multiturn resume, type a draft, toggle modes, send) explicitly.
