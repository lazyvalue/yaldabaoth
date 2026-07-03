# Adversarial critique of the `/clear` fix spec (independent, Opus)

**VERDICT: SHIP-WITH-CHANGES** — applied. The core predicate change is correct and
the root-cause model survives adversarial reading of the real code. The changes
below (all applied) are verification/precision hardening so this is the LAST round.

## Axis 1 — False positives: NONE (fix is safe)
Every writer of `focus = AgentFocus::Compose` in an idle worksheet also sets
`you_block_open=true` in the same notify-atomic closure, or is chatbox/awaiting
(excluded by the unchanged conjuncts): submit (agent_ui.rs:3794-3805, one notify),
mid-turn Esc (:4142-4145, only when chatbox||awaiting), `open_you_block_at_cursor`
(agent.rs:3907-3915), `finalize_agent_turn_idem` (:3757-3770), `settle_input_focus`
(:3706-3728). Panel-return-to-Compose (default) captures the true prior focus
first and panel is modal. **The `|| focus==Compose` clause fires ONLY in the
anomalous hole — exactly the intent.**

## Axis 2 — Consumer completeness: exactly TWO raw-field readers
`TranscriptSeqs::of` (transcript_view.rs:111,137), reveal (:550), YouBlockSnap
(:585), keystroke notify (agent_ui.rs:4260) all read the PREDICATE → self-align.
Only **agent.rs:2892 (injection)** and **agent.rs:3350 (memo key)** read the raw
field — both changed. The `i/a/o` open-guard (agent_ui.rs:4125) reads the raw
field but is a "may I open a NEW block" gate, not a render gate — correctly left.

## Axis 3 — Memo-key hazard: REAL, must hash the predicate
On a `view_model_fingerprint` hit, `rebuild_agent_view_model` (with the injection)
never runs. The fingerprint (agent.rs:3286-3359) hashed raw `you_block_open` but
NOT `focus`; a focus-only Transcript→Compose flip yields an identical fingerprint,
reusing a stale flat list without the YouBlock row — **the row wouldn't appear even
with the predicate fixed.** `TranscriptSeqs` busts on the focus flip (the
`transcript_focused` seq), so the view re-renders — but that re-render memo-HITS.
**Fix: hash `inline_you_block_active()` verbatim at :3350** (applied). Verified
load-bearing: reverting only the memo edit ⇒ the guard test's PAINT assertion RED
(render count advances but the block isn't in the reused list).

## Axis 4 — Fixes the `/clear` path: YES, forced by routing
The compose dispatch (agent_ui.rs:4231) is reached only when `focus != Transcript`
(and panel is modal). `AgentFocus ∈ {Transcript,Compose,Panel}`. So text-in-buffer
⟹ `focus==Compose` — the user's own fact pins it. No `/clear` state has
`focus != Compose ∧ you_block_open==false` that also lands text in the buffer.

## Axis 5 — Verification hardening (applied)
- Pre-assert the four-part hole before typing (non-vacuous). ✔
- Paint assertion non-vacuous: YouBlock rect inside the `transcript-viewport` rect. ✔
- **Negative-control each of the three edits INDEPENDENTLY** — predicate ⇒ flat
  render count RED; injection ⇒ no-paint RED; memo ⇒ stale-list no-paint RED. Each
  RED for its own reason. ✔ (The most important item — a single all-reverted
  control would not prove the memo edit is load-bearing, the exact 7th-round trap.)
- Extend `inline_you_block_active_truth_table` with the `focus==Compose` rows +
  fix its stale doc comment. ✔

## Residual risks
- **R1** memo edit is the fragile one — covered by the independent negative control
  + the truth-table rows.
- **R2** no `focus` in the fingerprint was a latent trap beyond this bug — now the
  DERIVED gate (which includes focus) is hashed, closing the class.
- **R4** `connect_session_server()` (persist.rs:104) has no real `cfg(test)` guard
  and connects to the LIVE server — DOCUMENTED as a follow-up (forcing None under
  test breaks steering tests that fragilely rely on the live connection; the proper
  fix is a `test-support` in-process session-server, a separate ticket).

## Bottom line
The design is right and the root-cause model holds. With the hardening above, the
disagreement set is empty for every producer — this is the last round.
