# Worksheet reply: select-to-quote, older-turn replies, replied-to marker

Standing context for three worksheet-reply behaviors requested 2026-08-06 via
`/new-ux`. Builds on the existing `r` reply (UXI-AgentTile-21) and the worksheet
inline-edit model (UXI-AgentTile-11).

## Problem / why

Replying to agent text in the worksheet was weak on three axes:

1. **Selecting agent text looked broken.** `V` was unbound; `v` (extend-mode)
   paints nothing until you also move; the highlight color is faint. The user
   never saw a working selection.
2. **`r` could only quote the caret line's first N sentences**, and only within
   the latest turn — no way to quote an exact span or older text.
3. **No visual back-link** from a pending reply to the agent text it quotes.

## The model

- Selection is the transcript editor's own anchor/head selection (same model the
  drag-select band + copy-on-select render from). `V` = line-wise visual (whole
  line + extend-mode on so `V`/`j`/`k` grow it); `v` = char-wise visual.
- `r` seeds the reply You-block. Quote source = an active selection if any, else
  first-N-sentences of the caret line. Multi-line → `>`-per-line blockquote.
- A reply always lands in the **current turn**: `open_you_block_at_cursor` snaps a
  legal (latest-turn) anchor, else coerces to the tail (`None`); the freeze path
  sends a `None`-anchor reply at EOF. So lifting the older-turn refusal is enough
  to "reply past the boundary, sent in the current turn."

## Tickets

| # | Ticket | Status |
|---|--------|--------|
| 001 | `V`/`v` selection + selection-feeds-`r` + reply past the turn boundary | DONE (UXI-AgentTile-34/-35/-36) |
| 002 | The `>` replied-to source marker (when not editing) | DONE (UXI-AgentTile-37) |

## Links

- `docs/components/agent-tile/transcript.md` — UXI-AgentTile-34 (selection), -37 (marker).
- `docs/components/agent-tile/compose.md` — UXI-AgentTile-35 (selection→quote), -36 (past boundary).
- `docs/components/agent-tile/compose.md` UXI-AgentTile-21/-24 — the base `r` reply + `u` back-out.
