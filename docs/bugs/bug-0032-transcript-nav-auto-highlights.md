# bug-0032 — keyboard nav in the transcript auto-highlights text

**Status:** FIXED
**First seen:** 2026-08-07
**Component:** AgentTile / transcript nav + selection (`UXI-AgentTile-34`).

## Symptom

"Sometimes when scrolling around with keyboard, text is automatically highlighted
for some reason." Navigating the worksheet transcript with `j`/`k`/`h`/`l` grows a
selection band the user never asked for.

## Context / root cause

Text only auto-extends when the editor's **`extend_mode` is ON** (in `pre_move`,
extend mode keeps the anchor so every motion grows the selection; with it OFF a
motion clears the anchor and moves cleanly). `extend_mode` is turned ON by `v`
(`toggle-extend-mode`) and `V` (`select-line`, added for multi-line select), but in
the transcript **nothing turns it OFF**:

- `dispatch_normal_core` clears it on `Esc` (`edit_ui.rs:640`), but the worksheet
  transcript's OWN `Esc` branch (`agent_ui.rs:5982`) returns early and never reaches
  it — so `Esc` in the transcript does not exit extend mode.
- `reply_quote_at_cursor` collapses the selection but leaves `extend_mode` ON, so
  after `V`→`r`→submit the user returns to nav still in extend mode.

So once you use `V`/`v` (or the reply feature), extend mode is stuck ON with no
exit, and all subsequent navigation highlights.

## Planned solution

Give the transcript the missing exit + stop the reply path stranding it:

1. **`Esc` in transcript nav exits extend mode + collapses the selection** (when one
   is active), matching `dispatch_normal_core`. Falls through to the existing
   focus-to-compose logic only when there is no selection to cancel.
2. **`reply_quote_at_cursor` clears `extend_mode`** when it collapses, so a reply is
   not a lingering selection gesture.

No change to `V`/`v` extend behavior WHILE selecting (visual mode still extends);
this only adds the exit. Copy/selection ranges untouched.

## Log

### 2026-08-07 — FIXED (Esc exit + reply clears extend mode)

- **Change.** `agent_ui.rs`: the transcript-nav `Esc` branch now first cancels an
  active selection / extend mode (`extend_mode() || selection_range().is_some()` →
  `set_extend_mode(false)` + `clear_selection()`, consume the Esc) before the
  focus-to-compose fallback. `agent.rs`: `reply_quote_at_cursor` calls
  `set_extend_mode(false)` after collapsing.
- **Verified.** `verify_harness.rs`:
  `worksheet_esc_exits_extend_mode_stops_autohighlight` (real `v`/`l`/`escape`/`j`:
  after Esc, extend mode off + selection cleared + a following `j` does NOT
  highlight) and `worksheet_reply_clears_extend_mode` (`V`→`r` leaves extend mode
  off). **Both NCs observed RED:** disable the Esc cancel → "Esc exited extend mode"
  fails; drop the reply `set_extend_mode(false)` → "r cleared extend mode" fails.
  Full suite 539 green.
- **Note.** `V`/`v` remain sticky visual modes WHILE selecting (V+j / v+l still
  extend, as the multi-line select feature intends); the fix supplies the missing
  EXIT. If persistent visual mode still surprises, a follow-up could add a mode
  indicator or make `V` a one-shot.
