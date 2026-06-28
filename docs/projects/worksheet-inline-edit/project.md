# Project: Worksheet inline-edit (restore the editable buffer on Model C)

## Problem / why

The Model C **UX** (read-only transcript + an always-present separate compose
box) is an unusable regression. The user wants the worksheet to be an **editable
conversation buffer** again: navigate the whole conversation with the cursor, and
type a reply *in place*. The Model C **data architecture** (read-only transcript
as the single ordered source of truth, draft in a separate `Compose`, only text
crosses the seam) is correct for durability and **stays** — it is the substrate,
not the UX.

Authoritative behavioral contract: **`docs/specs/spec-worksheet.md`** +
**`docs/ux-invariants.md` INV-UX-9**. The 7 rules (verbatim intent):

1. Normal mode: cursor moves freely over the whole transcript (read nav).
2. Insert opens a **You-block** (a `You` delimiter + editable region) at the caret.
3. Empty insert (only whitespace) is a no-op: the You-block disappears,
   transcript byte-identical.
4. Non-empty You-block persists in place; the next Submit sends it + freezes it.
5. Insert is bounded: only within the **most-recent agent turn**, only **after an
   agent newline**. Frozen content is not editable.
6. One You-block at a time.
7. Mid-turn → a **bottom chatbox** (the only time the chatbox exists); idle = no
   chatbox.

## The model (how it maps onto Model C)

- The draft text lives in the existing **`Compose`** buffer (Model C separation
  preserved — the transcript never holds an uncommitted draft, so undo isolation,
  replay-without-burial, and streaming-at-EOF all keep holding).
- A **You-block** = the `Compose` rendered **inline at a transcript anchor**
  (`LineAnchor`, identity not offset), with a `You` delimiter. When idle + no
  block: the transcript is pure-navigable; no compose chrome.
- Mid-turn (`turn_phase.is_awaiting()`): the `Compose` renders as the bottom
  **chatbox**; input routes there (steers/queues, INV-UX-7). Derived from turn
  phase, NOT a user-selected `InputModeKind`.
- Submit of a You-block freezes the text **at the anchor** via the in-place
  `commit_worksheet_turn` path (revived from its `#[cfg(test)]` seam) →
  `register_user_turn` for dedup/numbering; tail-anchored submit stays the EOF
  `insert_user_turn`.

Durability holds because editing only happens while idle (rule 7): streaming
never lands in a buffer being edited, so the ordering-corruption class Model C
killed stays unrepresentable.

## Tickets

| # | Ticket | Status |
|---|---|---|
| 001 | Worksheet edit-state model + idle/mid-turn routing + You-block lifecycle (tail anchor) | DONE (rules 1–4,6,7 tail; 275+143 tests) |
| 002 | Between-lines anchoring: open/render/freeze a You-block at an arbitrary legal point in the latest turn (render injection into TranscriptView) | DONE (FlatItem::YouBlock inline render + freeze_as_user_turn_at + legal guard; 276+145 tests) |
| 003 | Retire the user-selected Worksheet⇄Chatbox toggle; chatbox is purely mid-turn-derived; default new sessions to Worksheet | open |

## Links

- `docs/specs/spec-worksheet.md` (authoritative behavior)
- `docs/ux-invariants.md` INV-UX-9
- ADR-0024 (Model C — the substrate that stays)
- `docs/specs/spec-agent-window.md` §9–§15 (original inline-edit mechanics; baseline)
</content>
