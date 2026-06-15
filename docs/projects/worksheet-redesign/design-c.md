# Tech Design — Worksheet Mode Redesign (Model C)

Status: **Draft (pre-review)** · Implements: `PRD.md` · Supersedes: `design.md` (Model A)
Surface: `yalda-gpui` · Base: worktree `worksheet-redesign` (clean HEAD `0d062db`)

Read `PRD.md` for product intent and `design.md §0` for why Model A was rejected.
This is the design we implement.

## 0. Resolved: Option B — worksheet is the base, chatbox is diminutive

The product owner chose **Option B** (§8b R1) with a reframing that is the spine of
the whole design: **Worksheet is the base model; Chatbox is the diminutive case
where the editable buffer is moved off to the side (a pinned box).** There is one
model — a focusable read-only transcript plus a `Compose` buffer — and a single
mode axis, **placement**:

- `Placement::Inline` (worksheet, the base): the `Compose` renders inline flush at
  the transcript tail, in conversation typography.
- `Placement::Pinned` (chatbox, the diminutive): the `Compose` renders in a pinned
  box at the window bottom.

Capabilities that the §8b review treated as "worksheet identity" are therefore
**base capabilities of both placements**, not worksheet-only: the transcript is
focusable/navigable/selectable, and `S` (`send_agent_selection`) sends a transcript
selection. `focus ∈ {Transcript, Compose}` (default `Compose`) is a property of the
session, shared by both placements. "Preserve chatbox" reduces to: chatbox is just
`placement = Pinned` (its existing pinned-box render and compose-focused default are
unchanged); it simply *gains* the base transcript-focus/selection capability, which
is additive.

This makes the struct encapsulation exact:
`InputSurface { compose: Compose, placement: Placement }` + a session `focus` scalar.

## 1. The model in one picture

The agent tile already has a working model for "read-only transcript + editable
compose buffer": **Chatbox**. Model C makes **Worksheet a second rendering of
that same model**, not a different model.

```
            ┌─────────────────────────────────────────┐
            │  TranscriptView (cached child)           │
            │  committed turns, append-only, READ-ONLY │   U1  llm  T1  llm …
            │  — grows only from the server pump       │
   Chatbox: ├─────────────────────────────────────────┤
            │  Compose (boxed, pinned to window bottom)│
            └─────────────────────────────────────────┘

            ┌─────────────────────────────────────────┐
            │  TranscriptView (cached child)           │
            │  committed turns, append-only, READ-ONLY │   U1  llm  T1  llm …
 Worksheet: │  ┄┄┄ Compose rendered INLINE, flush ┄┄┄  │
            │  › your live draft, conversation type    │   ›  draft
            └─────────────────────────────────────────┘
```

Both modes are: **one read-only transcript editor + one editable `Compose`
buffer.** They differ in exactly two things — **placement** (compose pinned at
window bottom vs. rendered inline flush under the transcript) and **styling**
(boxed vs. conversation typography with a `›` draft gutter). Everything else —
submit, send-failure, toggle, persistence, perf — is shared code.

This is the architecture reviewer's "a chatbox rendered higher up," adopted
deliberately: the distinctiveness worksheet loses (a single cursor spanning sent
+ draft) is the exact thing that made Model A a bug farm.

## 2. The four invariants (the spine — why there is no edge-case sprawl)

These make the coordination overhead the user worried about structurally absent.
Each makes a class of Model-A bugs **unrepresentable**, not handled.

- **INV-1 — Transcript is append-only and read-only to the user.** The transcript
  editor changes only by append, only from the server pump / submit-freeze. The
  user can never edit it. ⇒ no frozen/editable boundary, no `can_insert/delete`
  guards in the hot path, no mid-history editing, no draft anchor to track or
  lose. (Kills Model-A #7/#8/#10 + undo-annihilation.)
- **INV-2 — The draft is a plain `Compose`, ignorant of the transcript.** Same
  type Chatbox uses today: its own rope, cursor, undo, modal state. No anchors
  into the transcript, no frozen lines, no streaming floor. ⇒ streaming appends
  to the transcript exactly as today, oblivious to the draft. (Kills #1/#13 + the
  `agent_tail_floor_char` floor scan, which is **deleted**.)
- **INV-3 — The only cross-buffer transfer is text, never positions.** Submit
  passes `compose.text()` (a `String`) into the transcript; toggle moves the
  whole `Compose` value. No line/char index ever crosses the seam. ⇒ the entire
  "index shifted under me" family is unrepresentable. This is the
  view-is-its-own-coordinate-space principle applied to buffers.
- **INV-4 — Submit is one transactional handoff, reusing `insert_user_turn`.**
  Read `compose.text()` → send → on success `insert_user_turn` (the reconciler:
  optimistic echo + freeze at EOF, dedups the server echo) → reset compose. This
  is **literally `submit_chatbox` today** (`agent_ui.rs:3368`). No background
  sync loop; consistency is established at one call site and the buffers are
  independent again.

## 3. Components & boundaries

### `Compose` (rename of `Chatbox`, `agent.rs`)
The editable draft buffer: `{ editor: Editor, mode: EditMode, scroll_handle,
list: ScrollAnchoredList<String> }`. Unchanged in substance — just renamed so it
reads as "the compose surface for either mode." Owns only its own text + UI state.

### `InputSurface` (`agent.rs`) — symmetric, total accessor
```rust
pub(crate) enum InputSurface {
    Worksheet(Compose),
    Chatbox(Compose),
}
impl InputSurface {
    fn compose(&self) -> &Compose      { match self { Self::Worksheet(c) | Self::Chatbox(c) => c } }
    fn compose_mut(&mut self) -> &mut Compose { /* … */ }
    fn is_chatbox(&self) -> bool { matches!(self, Self::Chatbox(_)) }
    fn mode(&self) -> InputModeKind { /* unchanged */ }
}
```
`compose()` is now **total** (both arms have one) — the old
`chatbox()->Option` asymmetry (None in worksheet) is gone. Submit/render/persist
read `surface.compose()` regardless of mode. The enum carries the *whole* draft
state, so "draft exists iff in a mode" is true and toggle is lossless by moving
the value (§4.3).

### `AgentSession` (`agent.rs`)
Owns `editor` (the read-only transcript), `input_surface`, reconciler state.
Submit orchestrates `insert_user_turn`. **Deleted:** `submit_worksheet`,
`commit_worksheet_turn`, `agent_tail_floor_char`, the worksheet per-line freeze,
and the worksheet→transcript-editor key routing.

### `TranscriptView` (`transcript_view.rs`) — render only, unchanged contract
Renders committed content; it's the same cached child for both modes. In
worksheet mode it must **not** host an editable region. The optional `›` draft
gutter is a styling concern on the inline `Compose`, not on the transcript.

### `Compose` rendering (`screens.rs`/`chrome.rs`)
One compose renderer, two call sites: Chatbox renders it boxed at window bottom
(today's path, untouched); Worksheet renders it inline flush below the
`TranscriptView` cached child, in the same column/typography. v1 = **pinned-flush**
(compose is a sibling element directly under the transcript) — zero render
coupling, reuses the chatbox layout. (See §7 for the deferred in-list variant.)

### `persist.rs`
`SessionSnapshot` gains `compose_draft: Option<String>` (saved from
`surface.compose().text()` for *either* mode — chatbox drafts persist too, a
free bonus). Restore seeds the compose editor.

## 4. The flows

### 4.1 Submit (Ctrl-Enter) — unified `submit_compose`
`submit_chatbox` is generalized to `submit_compose` and called by the Ctrl-Enter
handler in **both** modes (worksheet's separate `submit_worksheet` is deleted):
```
text = surface.compose().text()
if text.trim().is_empty(): status "nothing to send"; return
if no channel and no server: status "no channel attached"; return
sent = send(text.trim_end_matches('\n'))               # send FIRST
if not sent: status "send failed — ⏎ to retry"; return # compose kept intact
insert_user_turn(text, LocalSubmit, advance_replay=false)  # reconciler: echo+freeze+dedup
turn_phase = begin()
surface = same_mode(Compose::new())                    # reset compose, keep mode
```
`insert_user_turn` already freezes at transcript EOF and routes through the
reconciler, so the server echo is deduped (no double-render) and turn numbering
is single-sourced. Identical for both modes ⇒ the DRY win is real, not forced.

### 4.2 Streaming output — unchanged
`append_llm_chunk` appends at transcript EOF / `find_llm_insertion_point` exactly
as today. There is **no draft in the transcript** to stream around, so no floor,
no clamp, no `agent_tail_floor_char`. The scroll/`ScrollAnchoredList` behavior is
today's (already-solved) transcript-append behavior. (Resolves responsiveness P2:
the Model-A streaming-above-inline-draft `ListState::reset` scroll-jump cannot
occur — the draft isn't in the scrolling transcript.)

### 4.3 Toggle Worksheet ⇄ Chatbox — `toggle_agent_input_mode`
Lossless by construction — **move the `Compose` value, change the tag**:
```rust
self.input_surface = match std::mem::replace(&mut self.input_surface, /*tmp*/) {
    InputSurface::Worksheet(c) => InputSurface::Chatbox(c),
    InputSurface::Chatbox(c)   => InputSurface::Worksheet(c),
};
```
The draft text, cursor, undo stack, scroll — all carry over untouched. No seed,
no clear, no text copy. (The Model-A toggle that stranded/abandoned the draft is
gone.)

### 4.4 Persistence & replay
- **Save**: `compose_draft = Some(surface.compose().text())` if non-empty.
- **Restore**: after the session is built, seed `compose_mut().editor` with the
  saved text.
- **Replay / reconnect**: `reset_for_replay` rebuilds the **transcript** editor
  only; it does not touch `input_surface`, so the `Compose` draft **survives a
  reconnect automatically** — no re-seed dance, no mid-history burial, no
  pipelined-turn duplication. (Resolves Model-A correctness P1/P3.)

### 4.5 Focus & navigation (the one genuinely new coordination point)
- **v1**: the `Compose` always holds keyboard focus in worksheet mode (exactly
  as the chatbox does today); the transcript is read-only and **scrollable** with
  the same affordances chatbox already provides. No new focus enum.
- **Deferred enhancement** (flagged, not built in v1): a gesture to move focus
  into the transcript for cursor navigation / range selection / copy, returning
  to the compose on Escape or first keystroke. This is the only place a focus
  scalar would be introduced; keeping it out of v1 keeps coordination at zero.

## 5. What is preserved (hard constraint)

- **Chatbox**: behavior byte-identical. `submit_compose` is `submit_chatbox`
  generalized; the chatbox render path, pinned-bottom placement, follow-output
  policy, and `Chatbox`→`Compose` rename are mechanical. Its regression tests
  (`transcript_021_chatbox_keystroke_is_render_flat`, the reconciler seam tests)
  stay green unmodified.
- **Everything else on the agent tile** — selector/binding, tool calls,
  subagents, `InputModeKind` persistence, `should_follow_tail` — untouched.

## 6. Test plan (agent-buffer invariants — headless, `tests.rs`/`verify_harness.rs`)

Never touch `~/.yalda` (the `*_PATH_OVERRIDE` seam). Each pins an invariant:

1. `submit_compose_freezes_and_resets` — submit (each mode) freezes `text` at
   transcript EOF tagged `User(k)`, compose reset empty, `turn_phase` begun.
2. `submit_compose_blank_is_noop` — whitespace/empty draft → no send, no freeze,
   no turn, compose unchanged.
3. `submit_compose_send_fail_keeps_draft` — send fails → nothing frozen, compose
   text intact, status set.
4. `submit_compose_no_double_render_vs_echo` — submit + server echo of the same
   turn → one rendered `User` turn (reuses the reconciler seam test).
5. `toggle_preserves_compose_value` — W→C→W and C→W→C keep text+cursor (assert
   the same `Compose` survives; ptr/identity or text+cursor equality).
6. `worksheet_transcript_is_read_only` — user key events in worksheet mode mutate
   the **compose** editor, never the transcript editor (transcript text invariant
   under a simulated keystroke). Pins INV-1.
7. `streaming_appends_at_eof_independent_of_draft` — with a non-empty compose
   draft, `append_llm_chunk` appends at transcript EOF; compose text byte-
   identical before/after. Pins INV-2/INV-3.
8. `compose_draft_survives_reset_for_replay` — seed draft → `reset_for_replay` →
   draft text preserved (transcript rebuilt, compose untouched). Pins §4.4.
9. `compose_draft_persist_roundtrip` — snapshot save/load preserves draft for
   both modes.
10. `worksheet_compose_keystroke_render_count` — (verify_harness) typing in the
    worksheet compose busts the **compose** surface render count by exactly one
    and leaves the **transcript** render count flat (the perf win: typing the
    draft does NOT re-render the transcript). Mirrors
    `transcript_021_chatbox_keystroke_is_render_flat`. Pins rule 5.
11. `worksheet_inline_compose_renders` — (verify_harness) worksheet mode renders
    the compose element inline below the transcript (presence + `›` styling).

## 7. Out of scope / deferred (flagged, not assumed done)

- **In-list inline compose** — rendering the draft as the last *item inside* the
  transcript's scroll list so it scrolls away with content. Adds render coupling;
  pinned-flush (§3) gets ~95% of the feel at zero coupling. Revisit only if the
  feel demands it.
- **Transcript focus / cursor navigation in worksheet** (§4.5 deferred).
- **Mode consolidation** — Worksheet and Chatbox now share their core; a future
  collapse to one mode + a placement flag is possible but explicitly *not* done
  here (the constraint is "preserve chatbox").
- Rich text, rewinding sent turns, multi-user (PRD §3).

## 8b. Review round 2 — findings folded (authoritative)

Three reviewers (architecture, correctness, responsiveness) attacked this doc
against the worktree base. Resolutions, all binding on implementation:

**The identity decision (R1) — escalated to the product owner, see below.** All
three found that v1 as written makes Worksheet *functionally identical* to
Chatbox — the only difference is CSS (inline-flush placement + typography). The
transcript becomes read-only with focus always on the compose, so worksheet's
entire current identity (focusable transcript, cursor over history, range
selection, `S`=send-selection) is deleted. This is unresolved in this doc by
design; it gates implementation.

**Doc corrections (stale Model-A carryover):**
- `agent_tail_floor_char` and `append_llm_chunk_floored` **do not exist** in this
  worktree base — strike every "deleted" reference (§2 INV-2, §3, §4.2). Streaming
  already appends at EOF via `find_llm_insertion_point`. Nothing to delete.
- `ScrollAnchoredList` does **not** exist here; the compose uses `gpui::ListState`
  directly (`screens.rs:1278`). Any `ListState::reset` scroll-jump is chatbox's
  *existing* behavior, inherited unchanged — not introduced. Correct §3/R5.

**Encapsulation — use a struct, not the enum (architecture P1):**
Replace `enum InputSurface { Worksheet(Compose), Chatbox(Compose) }` with
`struct InputSurface { compose: Compose, placement: Placement }` where
`Placement = { InlineWorksheet, PinnedChatbox }` (the `Copy` discriminant
replacing `mode()`/`is_chatbox()`). Carrying the same payload in both enum arms
makes the "exists iff" property vacuous and forces an or-pattern on every access;
the struct says the true model out loud ("one compose, two placements"). Toggle
becomes `self.placement = self.placement.flip()` — no `mem::replace`, the compose
never moves (kills correctness P1's placeholder-temp cost).

**Must-fix call sites the doc omitted (the rename/read-only fallout):**
- **paste/copy** `else`-branches write/read the transcript editor in worksheet
  mode (`main.rs:2879-2882`, `2929-2930`) → must read `compose` unconditionally.
  Violates INV-1 and *compiles* — silent. Extend test #6 to fire a paste.
- **restore sites** (`agent_ui.rs:89`, `main.rs:1809`, `main.rs:1862`) construct
  the worksheet surface and **must seed `compose_draft`** there, not just satisfy
  the compiler. Mechanically writing `Compose::new()` drops the draft silently.
- **persistence** is a 4-site change, not "gains a field": `SessionSnapshot`
  (`persist.rs:1028`) + save map + load parse + the snapshot-build site that
  fills `mode` from `input_surface` (`agent_ui.rs:1182`) must also write
  `compose.text()`. Keep the `cfg(test)` path-override seam.
- **status strip** (`screens.rs:885-910`) reads `c.mode` and `c.editor.cursor()`
  for the worksheet label/position → must read `compose` (its mode/cursor), or it
  shows stale/meaningless state.
- **`verify_harness.rs:1001` `input_surface_toggle_round_trips`** asserts the
  removed `chatbox()`-is-None-in-worksheet asymmetry → rewrite as test #5
  (`toggle_preserves_compose_value`). §5's "chatbox tests green unmodified" is
  false for this one; correct it.
- **`submit_compose` reset must preserve the mode** — do not hardcode the chatbox
  arm (a worksheet submit would silently flip to chatbox). Test #1 asserts
  `placement` unchanged across submit, both modes.

**`should_follow_tail` is NOT untouched (correctness P2-orphan, responsiveness P3):**
Its `Worksheet => cursor_at_eof` arm (`agent.rs:694`) reads the *transcript*
cursor, which Model C removes. Collapse it to `follow_output` (identical to
chatbox) and drop the `cursor_at_eof` computation in `follow_tail`
(`agent.rs:2732-2740`). Add `worksheet_follows_output_like_chatbox`. Correct §5.

**Compose must become a `cached_child` (responsiveness P0/P1):**
Today the compose is built inline inside `render_agent` every frame (mitigated to
O(visible) by the `ListState` virtualization, but still re-walked on any root
re-render). Worksheet makes it the *primary* typing surface, so promote `Compose`
to its own cached child (the `CLAUDE.md` roadmap already names it) and ship the
render-count test. Rewrite test #10: assert a worksheet compose keystroke leaves
the **transcript** render count flat (the real, catchable win — today's worksheet
busts the transcript every keystroke = O(transcript)); the "compose count +1"
half is only measurable once Compose is cached.

**`TranscriptSeqs` reconciliation (responsiveness P4, CLAUDE.md rule 2):**
The transcript currently renders an editable caret driven by `c.mode`,
`c.editor` cursor, `c.pending_reveal_cursor`, selection seqs
(`transcript_view.rs:77-90`). If the transcript stays read-only with no caret,
remove those inputs *and* their `transcript_021_*` assertions. If the identity
decision keeps a focusable transcript (caret for nav/selection), keep them and
gate the caret on transcript-focus. Decide with R1.

**DRY framing correction (architecture P1):** the unified `submit_compose` is
safe **because** INV-1 moved the draft out of the transcript — it would be wrong
against today's in-place per-line freeze. State that; don't imply `submit_chatbox`
already served worksheet.

## 8. Risk register (for the reviewers to attack)

- **R1 — Does pinned-flush actually deliver the worksheet feel,** or does it make
  worksheet visually indistinguishable from chatbox (making the mode pointless)?
- **R2 — `Compose` rename churn**: many call sites read `chatbox()`; the
  total-`compose()` accessor changes their shape. Risk of missing a site that
  assumed None-in-worksheet.
- **R3 — Submit send-failure + optimistic echo ordering** when generalized to a
  mode that previously froze in-place. Confirm `insert_user_turn` is correct for
  the worksheet entry path (it already is for chatbox).
- **R4 — Inline compose + `cached_child` sizing**: rendering the compose flush
  under a `size_full` cached transcript must not collapse either (CLAUDE.md
  rule 3).
- **R5 — `should_follow_tail`/scroll** with the compose inline: does following
  output still pin correctly when the compose sits below the transcript?
