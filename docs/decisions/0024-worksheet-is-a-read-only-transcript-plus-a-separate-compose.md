# ADR-0024: Worksheet is a read-only transcript + a separate Compose buffer (Model C)

**Status:** Accepted (2026-06-24)
**Supersedes:** the "Model A" worksheet (one shared rope for transcript + draft).
**Related:** `docs/projects/worksheet-modelc/project.md`,
`docs/projects/worksheet-redesign/design-c.md`, ADR-0020 (address-by-identity /
single-source-derived-state).

## Context

Worksheet mode corrupted conversation ordering: a turn's streamed/replayed
content landed mid-document (above content newer than it), so a newer agent turn
rendered above an older exchange, and the user's in-progress draft appeared
stranded in the middle of history. It was "fixed" 15+ times and kept regressing.

The root cause was architectural, not a stray bug. The old worksheet used **one
flat mutable `Document` rope to represent two different things at once**: the
immutable, ordered conversation log (frozen lines + `TurnId` tags) **and** the
live, mutable user draft. They were reconciled by **position-based heuristics**
(`frozen_lines`, `lockable_through_line`, `last_llm_line`, `agent_tail_floor_char`,
`find_llm_insertion_point`) with **no single ordering invariant and no test
enforcing one**. Insertion was by character offset, not identity, so a stale tag
or floor put bytes in the wrong place permanently. Resume replayed the whole
history back through this same mutate-a-shared-rope path. Every prior fix was a
local patch; nothing pinned the order, so it always came back.

A complete redesign ("Model C") had been built on a branch
(`worksheet-redesign`) but was never merged and went 27 commits stale.

## Decision

Adopt Model C: there is **one** model — a **read-only, append-only transcript**
plus a **separate `Compose` draft buffer**. Worksheet vs Chatbox is not two
models; it is one model at two **placements**:

- `InputModeKind::Worksheet` (inline): the compose renders flush below the
  transcript, accent-bordered, labeled **You**.
- `InputModeKind::Chatbox` (pinned): the compose renders in a boxed control at
  the window bottom.

`InputSurface` is a struct `{ compose: Compose, mode: InputModeKind }` (the
compose exists in **both** placements). Toggling flips `mode`; the compose value
never moves (lossless). Submit is unified (`submit_compose`): read
`compose.text()`, send, on success `insert_user_turn` (append + freeze at
transcript EOF via the reconciler), reset the compose preserving placement.

## The invariant (INV-ORDER)

> The transcript is append-only and read-only; the draft is a separate buffer.
> The only cross-buffer transfer is **text**, never a position. ⇒ a turn's
> chunks can only extend the transcript at EOF; a draft is never inside history;
> replay rebuilds the transcript in event order. **Ordering corruption is
> unrepresentable, not handled.**

Why this holds mechanically: with no user draft in the transcript,
`agent_tail_floor_char` always returns EOF (it returns a non-EOF floor *only*
when it finds untagged user text at the tail — which now lives in the separate
compose), so all agent streaming appends at the bottom; `find_llm_insertion_point`
keeps its EOF guards for old/new turns. Pinned by
`inv_order_streaming_with_draft_appends_at_eof` and
`inv_order_interleaved_turns_stay_chronological`, which drive the real floor path
and would fail against the old shared-rope model.

## Consequences

- **Deleted (Model-A machinery):** `submit_worksheet`, the live
  `commit_worksheet_turn` per-line freeze (kept `#[cfg(test)]` only as the
  reconciler-dedup seam), worksheet→transcript key routing, the presence-driven
  "You" divider **as a transcript flat-item**, and the block-paged `snap_nav`
  over the transcript. `agent_tail_floor_char` / `append_llm_chunk_floored`
  remain but are inert (floor == EOF) under Model C.
- **The "You" divider moved** from a transcript flat-item (driven by the
  transcript editor's mode) to the inline compose's own top-edge label — the
  draft is a separate buffer, so the transcript never hosts it.
- **Draft survives reconnect for free:** `reset_for_replay` rebuilds the
  transcript only and leaves `input_surface` untouched; the draft also persists
  across restart (`SessionSnapshot.compose_draft`).
- **Transcript navigation** (cursor over history, range-select, `S`=send
  selection) is a focus mode (`AgentFocus::Transcript`), entered from the local
  menu, `Esc` back to compose.
- **Runtime-unverified:** the inline render + "You" label + toggle feel need a
  human check (GPUI can't be driven headlessly).

## Alternatives rejected

- **Keep patching the shared rope** — the status quo; 15+ patches, no invariant,
  perpetual regression. Rejected.
- **Merge the stale `worksheet-redesign` branch directly** — conflicts across
  all 8 worksheet files; high risk of silently reverting 27 commits of recent
  work (jump panel, roster, typed cwd, session-recall, caret-containment). Chose
  instead to re-derive Model C onto `main`, resolving each divergence explicitly.
