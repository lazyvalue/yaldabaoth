# ADR-0035 — Compose message-history recall is shell-style, top/bottom-line-gated

**Status:** accepted
**Date:** 2026-08-23
**Related:** `UXI-AgentTile-41` (docs/components/agent-tile/compose.md);
`UXI-AgentTile-9` (compose word-wrap); Model C / ADR-0024 (one compose at two
placements).

## Context

The agent compose had no way to recall a previously-sent message. Re-sending or
tweaking a message you just sent meant retyping it. The requirement: "Up arrow in
insert mode goes to the last written thing; Down arrow again restores what was
already written (but not sent), if anything."

Two design questions had to be resolved without breaking existing behavior:

1. **When does Up/Down recall vs move the caret?** The compose is a full editable
   surface that word-wraps and grows to any height (`UXI-AgentTile-9`,
   `UXI-AgentTile-11`). Overriding Up/Down unconditionally would kill vertical
   caret motion in a multi-line draft — a regression the edit_ui arrows were
   explicitly fixed to provide.
2. **What is the "draft" that Down restores?** The unsent text present when the
   user began browsing history.

## Decision

### 1. Shell-style recall, gated on the top/bottom LOGICAL line

Recall runs only in **Insert** mode.

- **Up on the top logical line** (`cursor.line == 0`) walks BACK through the sent
  ring; the first step stashes the current unsent draft. Elsewhere Up is caret
  motion.
- **Down on the bottom logical line** (`cursor.line == last`) walks FORWARD and,
  past the newest entry, restores the stashed draft and ends browsing. Elsewhere
  (and when not browsing) Down is caret motion.

Gating on the **logical** line (not visual row) matches the editor's own motion
model (`cursor_move_up`/`move_down` are logical) and keeps the rule simple: a
wrapped single logical line is still line 0, so Up recalls; a genuinely
multi-line draft navigates row-by-row until you reach the boundary.

### 2. A recalled entry is a committed baseline; editing ends browsing

`Compose::set_recalled` rebuilds the editor with the entry as its initial content
(no undo history), caret at end, Insert mode — only the text is swapped, staged
image attachments are preserved. Any keystroke that edits the draft, or leaving
Insert, calls `history_reset`: the recalled text becomes the working line and a
later Up re-stashes it. This is the familiar shell behavior and avoids a tangled
"edited-history-entry" state.

### 3. The ring is per-session, fed on successful submit, in-memory only

`AgentState::sent_history` is a `Vec<String>` (oldest→newest), fed by
`history_push` at both submit success branches (`submit_compose`,
`submit_worksheet_blocks`). It trims, skips an immediate duplicate of the newest
entry, and caps at `HISTORY_CAP` (200). It is **not persisted** across restart —
the persisted `compose_draft` already round-trips the current unsent draft; a
durable recall history was out of scope and would need a new persist field +
migration.

## Alternatives rejected

- **Override Up/Down unconditionally.** Simplest to state, but kills vertical
  caret motion in multi-line drafts (a known regression class).
- **Gate on visual row.** The editor moves by logical lines; keying recall off
  visual rows would desync from where the caret actually moves and complicate the
  wrap math for no user-visible gain.
- **Persist the ring.** Deferred — no requirement, and it adds a persist
  field + migration. Revisit if users want recall to survive restart.

## Consequences

- Vertical caret motion in multi-line drafts is preserved; recall is reachable
  from the natural top/bottom boundary.
- Recall works identically in both placements (chatbox + worksheet You-block)
  because both edit the same `Compose`.
- History is lost on restart (accepted). Browsing state is transient and never
  enters the transcript, so none of the ordering invariants are affected.
