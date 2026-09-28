# `/clear` worksheet-invisible — root cause + fix spec

**Status:** independent investigation (second engineer, Fable), 2026-07-02,
confirmed by the lead (Opus).
**Inputs:** `failure-log.md`, full read of the real code paths (file:line cited),
the lead's three headless repros (all GREEN), the `show_compose` gate
(screens.rs:1188, verified), the proven producer `force_restart_agent`
(agent_ui.rs:3602, verified) + mid-turn Esc (agent_ui.rs:4142, verified).

---

## 1. Executive summary

The symptom is produced by exactly one state, **the hole**:

```
focus == AgentFocus::Compose
∧ compose.mode == Insert
∧ you_block_open == false
∧ !turn_phase.is_awaiting()
∧ !input_surface.is_chatbox()
```

In the hole, keystrokes route into the compose buffer but **no surface paints
the compose** and **no cache-bust fires** — the text is in the buffer and
invisible until a chatbox toggle rebuilds everything.

Every prior fix added another `settle_input_focus()` at another *guessed
producer*. The hole was never made unrepresentable, and it has **multiple
producers**. The fix is producer-independent: derive the render gate from the
SAME fact the key-router uses (`focus == Compose`), at the one predicate every
consumer already shares — `AgentState::inline_you_block_active()`.

## 2. The symptom decoded (forced by the user's facts + real code)

1. Text landed in the compose ⇒ keys reached the compose dispatch
   (agent_ui.rs:4231) ⇒ `focus == Compose`, compose `Insert`.
2. Nothing painted ⇒ NOT awaiting and NOT chatbox: the bottom compose renders
   iff `is_chatbox() || is_awaiting()` (**screens.rs:1188**, verified).
3. Inline block not painted ⇒ `inline_you_block_active()` false ⇒ with
   idle+worksheet established, **`you_block_open == false`**.
4. No repaint either: the keystroke's session-notify is gated on the same
   predicate (agent_ui.rs:4259-4268) and `TranscriptSeqs::of` zeroes the compose
   fields when the gate is false (transcript_view.rs:104-124) — typing moves no
   observed seq. The chatbox toggle re-opens the block from the non-empty draft
   (agent_ui.rs:3356-3364) — exactly the reveal the user sees.

## 3. Mechanism

The single predicate `inline_you_block_active()` (agent.rs:3977) gates: flat-list
You-block injection (agent.rs:2892), `YouBlockSnap` build (transcript_view.rs:564),
`TranscriptSeqs::of` compose fields (transcript_view.rs:111), the keystroke
session-notify (agent_ui.rs:4259), submit anchor, reveal. The bottom panel is
gated `is_chatbox() || is_awaiting()` (screens.rs:1188). **Key routing keys on
`focus`; painting keys on `you_block_open`. The bug class is the disagreement set
of those two predicates.**

## 4. `/clear` orchestration audits clean

Every drivable transition on the `/clear` path lands gate-TRUE (sync placeholder
settle agent_ui.rs:2875; async bind settle :476; reset_for_replay→settle
agent.rs:4080; ChannelOpened skipped by the §9 gate :1987; submit paths return
before the awaiting flip :3677/:3748). Consistent with the lead's three GREEN
repros. The `/clear`-native producer is a live interleaving; the fix does not
depend on naming it.

## 5. Producers of the hole

**Proven, reachable TODAY (verified file:line):** `force_restart_agent` sets
`turn_phase = Idle` with **no settle** (agent_ui.rs:3602 direct / :3561 server),
combined with mid-turn Esc-from-nav which sets `focus=Compose` while the block is
closed (agent_ui.rs:4142-4145, gated on `is_chatbox() || is_awaiting()`). Sequence
`submit → mid-turn Esc → ⌘. ⌘. (force-restart) → type` = the hole. `/clear` is
what users type right after stopping a runaway agent. Same family: pump
stale-channel drop (:2215), restart/cwd Idles (:3015/:1117).

## 6. Root cause

Proximate: the hole (§1). Root: "focus=Compose ⇒ a visible compose surface" is a
*maintained* invariant, holding only if every `turn_phase`/`focus`/`you_block_open`
writer remembers to settle; each regression was a writer that didn't. The gate
re-derives visibility from `you_block_open` instead of the routing fact `focus`,
so the disagreement set is representable. Six green fixes coexisted with the broken
app because every test asserted the settled state at a guessed producer (or the
BUFFER, not paint), never the invariant over all writers, and never entered the hole.

## 7. The fix — make the hole unrepresentable

```rust
// agent.rs:3977 — the ONE gate all consumers share. An idle worksheet whose
// focus is on the compose ALWAYS has an active inline block: routing-to-compose
// ⇒ painted.
pub(crate) fn inline_you_block_active(&self) -> bool {
    (self.you_block_open || self.focus == AgentFocus::Compose)
        && !self.turn_phase.is_awaiting()
        && !self.input_surface.is_chatbox()
}
```

Align the two consumers that read the raw field:
- Flat-list injection agent.rs:2892: `if c.you_block_open` →
  `if c.you_block_open || c.focus == AgentFocus::Compose` (or call the predicate).
- View-model memo key agent.rs:3350: hash the derived gate (else a focus-only flip
  reuses a flat list without the YouBlock row).

Everything else routes through the predicate and self-aligns. In the hole,
`effective_you_block_anchor()` is `None` ⇒ tail placement — correct.

**Why this layer:** it expresses routing⇒painting structurally; the disagreement
set is empty for every current and future producer; minimal (no new settle, no
new owner). `settle_input_focus` keeps choosing anchors/mode but stops being
load-bearing for visibility.

**Edge sweep:** chatbox/mid-turn excluded by unchanged conjuncts; idle nav
(focus=Transcript) unchanged; persisted non-empty block in nav (open +
focus=Transcript) kept by `you_block_open ||`; panel-exit to
`panel_return_focus=Compose` with no block — today a latent focus-into-the-void,
with the fix renders a tail block (strictly better; flag for review). INV-UX-9
"a block exists only while idle in the worksheet" preserved.

**Rejected:** a 7th settle (whack-a-mole); route keys by the render predicate
(dead typing instead of invisible — worse); always render the bottom box in idle
worksheet (contradicts Model C/INV-UX-9); delete `you_block_open` (rule-4
persistence needs it; out of scope).

## 8. Verification (anti-circling compliant)

1. **Headless RED guard** `hole_state_types_and_paints`: enter the hole via a
   REAL producer (mid-turn awaiting → real `handle_claude_key(Esc)` from nav →
   real `force_restart_agent`), type via `handle_claude_key`, assert
   `perf_render_count("transcript")` advanced AND `layout_probe_get("you-block")`
   painted inside `"transcript-viewport"`. **Mandatory negative control:** revert
   the predicate → RED (flat count / no you-block probe).
2. Keep the lead's three repros as layer-regression pins.
3. Fuzzer oracle clause: `focus==Compose ∧ idle ∧ worksheet ⇒
   inline_you_block_active()`; add stop/force-restart + `/clear` constituents to
   the op list.
4. Mutation-test the predicate + injection gate + memo key.
5. Live `YALDA_CLEAR_DEBUG=1` trace names the `/clear` producer for the log.
6. Runtime screenshot harness as the acceptance gate (gaps #1 pixels + #2 live
   loop): temp-socket server + agent stub + scripted keys + `screencapture`,
   assert the typed text visible (OCR or pixel-diff vs a chatbox-toggle capture).

## 9. Incidental findings (fix alongside / note)

- `connect_session_server()` (persist.rs:104) has **no `cfg(test)` guard** — the
  harness connects to the user's LIVE server; the earlier
  `repro_..._real_path` failed for THIS artifact (real attach of a fake sid →
  dead-sid unbind), not the bug. Gate it under `cfg(test)`.
- `apply_open_agent_resolution`'s `Attached` branch never settles
  (agent_ui.rs:484-513) while `Created` does (:476) — asymmetry worth a note.

## 10. Definition of done

Green on `main`; §8.1 observed RED with the predicate reverted (flat count / no
probe); mutation gate clean on the changed predicate; `docs/ux-invariants.md`
updated with a new INV-UX-N naming the guard; release rebuilt; user confirms typed
text visible after `/clear` with no toggle; producer finding appended to the log.
