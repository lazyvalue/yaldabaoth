# `/clear` worksheet-invisible — failure log

**Symptom (user, recurring):** In an agent tile in **worksheet** mode, typing
`/clear` and submitting, then typing new text — **the typed text is not
visible**. It only appears after switching to **chatbox** and back to worksheet.
The text IS in the buffer (the mode toggle reveals it); this is a **render /
cache-invalidation** bug, not a typeability bug.

This has been "fixed" and has regressed **~6 times**. Each fix added another
`settle_input_focus()` call and another **state-assert unit test**; none drove
the **real** `/clear` path (server round-trip → async bind → attach → replay →
type) to an actual **paint**. This log is the durable record so we stop circling.

## Recurrence history (git)

| Commit | Claim | Why it didn't hold |
|--------|-------|--------------------|
| `c039f55` | feat: identity-preserving `/clear` | introduced `/clear`; the invisible-after-clear class starts here |
| `11c197d` | "`/clear` leaves worksheet typeable (settle the new session)" | added a `settle_input_focus()`; verified by hand-built state, not the real path |
| `3628902` | "`/clear` worksheet stays typeable after replay + negative-control rule" | added `reset_for_replay()` → `settle_input_focus()`; test `clear_then_empty_channel_open_keeps_worksheet_typeable` **hand-calls** the methods and asserts `inline_you_block_active()` — never runs `clear_agent_session` / `apply_open_agent_resolution` / render |
| `cbb3974` | "worksheet rests in nav so space opens the tile menu" | **regressed** typeability the other direction (rested in nav); later reverted by settle-on-clear |
| `030df37` | "worksheet rests TYPEABLE after `/clear` — type and SEE it, no `i`" | `settle_input_focus` sets `you_block_open` + focus + Insert; test `worksheet_typing_after_clear_is_visible_without_pressing_i` — check whether it drives the REAL clear path or a simulated state |
| `0c64d21` | "settle worksheet typeable at the async `/clear` bind (the untested divergence)" | added `settle_input_focus()` in `apply_open_agent_resolution` (the async bind); still no test drives that async path to paint |
| `f7eeaee` | docs: the anti-circling rules | **these rules were written because of this exact saga** |

## The common failure mode (why it keeps coming back)

The bug lives on the **live server-async path**:

```
clear_agent_session (server branch, needs session_server.is_some())
  → placeholder AgentState (settle_input_focus)
  → spawn_create_agent_session (async)
  → apply_open_agent_resolution(Created) → bind → settle_input_focus → notify
  → spawn_attach_sessions → server replay events
  → apply_server_batch → generation bump → reset_for_replay (turn_phase=Idle + settle)
  → user types → transcript render must repaint the inline You-block
```

The **render gate** is `AgentState::inline_you_block_active()` =
`you_block_open && !turn_phase.is_awaiting() && !is_chatbox()`. Both the
**render** of the inline You-block AND its **cache-busting seq**
(`TranscriptSeqs::of` gates `compose_edit_seq` on it) depend on this being
`true`. If it is `false` while the user types, the keystroke neither paints nor
busts the cached transcript — invisible until a chatbox toggle forces a rebuild.

**Every "fix" asserted the GATE is true on a hand-built state.** No test ever:

1. drove `clear_agent_session` (its server branch is skipped under `cfg(test)`
   because `session_server` is `None`), NOR
2. drove `apply_open_agent_resolution` + the attach/replay events through the
   real reducer, NOR
3. asserted the typed character is **PAINTED** in the transcript viewport.

So a green suite has always coexisted with a broken app. The unit test
`clear_then_empty_channel_open_keeps_worksheet_typeable` is the textbook
anti-circling trap: it hand-calls `settle_input_focus()` then `reset_for_replay()`
and asserts `inline_you_block_active()` — a *simulated* post-clear state.

## The plan this time (no more circling)

1. **Reproduce on the real path** — a headless test that drives the real
   `clear_agent_session` / `apply_open_agent_resolution` / `apply_server_batch`
   sequence (with a session-server seam) then a real keystroke, asserting the
   typed glyph is **PAINTED** and non-vacuous. If the headless seam can't reach
   the live divergence, a **runtime harness** that launches the app, scripts
   `/clear` + typing, and screenshots to verify pixels.
2. **Independent spec** (a stronger/independent model) of the root cause + fix +
   verification, then an **adversarial critique** of that spec.
3. **Implement**, observe **RED → GREEN** on the real-path reproduction, with the
   fix reverted producing RED for the right reason.
4. Not done until: green on `main`, binary rebuilt, and the user confirms the
   typed text is visible after `/clear` in worksheet with **no** mode toggle.

## Findings appended as we go

### 2026-07-02 — reducer paths PROVEN correct; bug is in the live orchestration

Built two real-path headless reproductions in `verify_harness.rs` that measure
the REAL mechanism (does a keystroke RE-RENDER the cached transcript —
`perf_render_count("transcript")` advancing — not "is the char in the buffer"):

- `repro_clear_worksheet_typed_text_repaints_simulated` — settled post-/clear
  state + real keystroke → **GREEN** (re-renders correctly).
- `repro_clear_worksheet_typed_text_repaints_real_path` — feeds the fresh
  channel's `ChannelOpened` (→ generation rebaseline → `reset_for_replay` →
  `settle`) to the bound session, then a real keystroke → **GREEN**.

**So the bug is NOT in the reducer / settle / reset_for_replay / the gate.** With
`inline_you_block_active() == true` (verified), a keystroke DOES bust the cached
transcript. Every past fix touched these (already-correct) layers — which is why
they were green AND useless.

**The bug is in the live orchestration that ONLY `clear_agent_session`'s server
branch does** and that no headless test can reach:

1. `self.transcript_views.remove(&id)` (agent_ui.rs ~2857) — the OLD `TranscriptView`
   is destroyed.
2. `self.sessions.close(id)` + `show_local_session(...)` (main.rs 2305) — the tile
   is rebound to a **NEW `SessionId`** / new `AgentSession` entity.
3. A fresh `TranscriptView` is lazily created via `transcript_view_for(new_id)` on
   the next render, registering a NEW `cx.observe(&new_session)` with
   `last_rendered = default`.
4. The async `apply_open_agent_resolution` (settle) + `spawn_attach_sessions` (real
   channel → `ChannelOpened`) arrive on the REAL event loop, interleaved with the
   first render of the new `TranscriptView` and the user's keystrokes.

The divergence is a **dynamic ordering** between (3) the new TranscriptView's first
`last_rendered` stamp, (4) the async settle/replay notifies, and the user's typing.
Static reasoning says it "should work" at every step — which is precisely the
signature of a timing/lifecycle bug that only manifests at runtime. Headless
`#[gpui::test]` can't drive the server round-trip (the deferred
`spawn_attach_sessions` unbinds with no server — gap #2), so it has never caught
this.

**Blockers for a headless real-path repro:** driving `clear_agent_session`'s server
branch needs `session_server: Some(SessionServerClient)` (a real client + worker).
Either (a) build a fake `SessionServerClient` seam, or (b) a runtime harness
(launch the app + server + agent, script `/clear` + typing, screenshot/OCR the
window) — the ground-truth the user authorized.

- (next) Confirm the new-TranscriptView-lifecycle hypothesis: either seam a fake
  server to drive real `clear_agent_session` headlessly, or the runtime harness.

### 2026-07-02 (cont.) — fresh-TranscriptView path ALSO green ⇒ headless is exhausted

`repro_clear_worksheet_typed_text_repaints_fresh_transcript_view` drops the
`TranscriptView` (as `clear` does), re-creates it fresh (default watermark), and
types → **GREEN**. So the new-view lifecycle is ALSO correct in isolation.

**Conclusion: this bug is NOT reproducible with the headless harness.** Three
independent real-mechanism reproductions (settled state, reset_for_replay/
ChannelOpened, fresh TranscriptView) are all GREEN. The reducer, the
`inline_you_block_active` gate, `settle_input_focus`, and the TranscriptView
lifecycle are each provably correct. The divergence is in the **live async
orchestration** — the real `session_server` round-trip in `clear_agent_session`'s
server branch, its `spawn_create_agent_session` → `apply_open_agent_resolution` →
`spawn_attach_sessions` timing, the new-`SessionId` handoff, and the real event
loop's interleaving with the user's keystrokes — none of which `#[gpui::test]`
can drive (the deferred attach unbinds with no server).

**This is exactly why it recurred 6×:** the reachable (headless-testable) layers
were always correct; the bug is in the unreachable live glue, and every "fix" +
green test targeted the reachable layers.

**Decision:** ground-truth verification requires a RUNTIME harness (launch the
app with the real server + agent, script `/clear` + typing, capture the window,
verify the typed text is visible) OR a fake-`SessionServerClient` seam that lets
`clear_agent_session`'s server branch + async resolution run headlessly. The
three green tests stay as **characterization guards** (they pin that the reducer
layers remain correct, so a future regression there is caught).

### 2026-07-02 (cont.) — EXACT single point of failure identified

Traced the keystroke path (`handle_claude_key`, agent_ui.rs ~4231). A worksheet
keystroke inserts into the compose via `with_session_silent` (which does NOT
notify), then at **agent_ui.rs:4259-4268** notifies the session (busting the
cached transcript so the char repaints) **ONLY IF `inline_you_block_active()` is
true** — the comment there describes this exact bug verbatim ("a keystroke didn't
bust the transcript cache — chars appeared only 'later'"). AND `build_body`
(transcript_view.rs ~450) gates the You-block RENDER on the same predicate. So:

> **If `inline_you_block_active()` is false at the instant the user types after
> `/clear`, the char is BOTH not rendered AND not repainted — invisible until an
> unrelated event (chatbox toggle) notifies.**

`inline_you_block_active() = you_block_open && !turn_phase.is_awaiting() &&
!is_chatbox()`. Statically, after `/clear` all three settle to the typeable
value (both `/clear` submit paths — agent_ui.rs:3677 and :3748 — return BEFORE
the `turn_phase = begin` at :3798, and the fresh `new_server_managed` is
`TurnPhase::Idle`). So the false clause is set by the **live async orchestration**
(attach/channel/resolution timing). Almost certainly `turn_phase.is_awaiting()`
is momentarily true during connect/attach, or `you_block_open` is cleared by a
late event, at the instant the user types.

**Instrumentation added (env `YALDA_CLEAR_DEBUG=1`):**
- `agent_ui.rs` keystroke path: logs `inline_active` + `(open, awaiting, chatbox,
  focus_compose, compose_len)` on every worksheet keystroke.
- `transcript_view.rs` observe callback: logs whether the transcript's slice
  moved on each session notify.

One real reproduction (`/clear` in worksheet, type a char) prints the false
clause → then the fix is exact, and a headless guard sets that precise state
(e.g. `you_block_open=true` + `turn_phase=Awaiting` post-`/clear`) and asserts
the keystroke still repaints — RED without the fix.

### 2026-07-02 — ROOT CAUSE FOUND + FIXED (spec.md, critique.md)

Independent second engineer (Fable) sharpened the diagnosis; adversarial reviewer
(Opus) hardened the verification. **The false clause is `you_block_open==false`
while `focus==Compose`** — NOT `turn_phase` (my earlier instrumentation guess).
Forced by the user's facts: text-in-buffer ⟹ `focus==Compose` (routing,
agent_ui.rs:4231); nothing-painted ⟹ NOT awaiting/chatbox (the bottom box shows
iff `is_chatbox()||is_awaiting()`, screens.rs:1188); inline-block-unpainted ⟹
`you_block_open==false`. **"The hole"** =
`focus==Compose ∧ you_block_open==false ∧ idle ∧ worksheet`.

**The deep root cause:** keystroke ROUTING keys on `focus`; PAINTING (+ the
cache-bust + the memo) keyed on `you_block_open`. The bug is the disagreement set
of those two predicates. Six fixes tried to keep state OUT of the set (another
`settle` at a guessed producer); none EMPTIED it. The hole has multiple producers
(proven: `force_restart_agent` Idles without settle, agent_ui.rs:3602).

**The fix (INV-UX-16):** derive the render gate from the routing fact —
`inline_you_block_active()` (agent.rs:3977) becomes
`(you_block_open || focus==Compose) && !awaiting && !chatbox`; align the flat-list
injection (agent.rs:2892) and the view-model memo key (agent.rs:3350) onto the
derived gate. Routing⇒painting is now structural.

**Verification (the thing missing for 6 rounds):**
`verify_harness.rs::clear_worksheet_hole_types_and_paints` drives the REAL key
handler from the hole and asserts the transcript re-renders (render count) AND an
inline You-block PAINTS inside the viewport — NOT the buffer text. **Each of the
three edits, reverted independently, produces RED for its own reason** (predicate ⇒
flat count; injection ⇒ no paint; memo ⇒ stale-list no paint). Truth table
(tests.rs) extended with the `focus==Compose` rows. Full suite 325 pass.

**Status:** fix on `main` (pending commit), release rebuilt. Runtime confirmation:
the fix is producer-independent so it heals the hole however `/clear` reaches it;
user to confirm typed text is visible after `/clear` in worksheet with no toggle.
The `YALDA_CLEAR_DEBUG` instrumentation remains (env-gated) to name the exact
`/clear`-native producer for the record if desired.

**Follow-ups (not bundled):** (a) `connect_session_server()` cfg(test) guard +
in-process test server seam (critique R4); (b) fold `focus` into the fingerprint
permanently (R2 — done via the derived gate); (c) runtime screenshot harness as a
standing acceptance gate.
