# Tech Design — Worksheet Mode Redesign

Status: **Revised after adversarial review** · Implements: `PRD.md` · Surface: `yalda-gpui`

## 0. Review outcome — decision reversed to Model C

Three adversarial reviewers (architecture, correctness, responsiveness) read the
pre-review draft below. They converged, independently, on overturning its central
recommendation. **The verdict is Model C, not Model A.** This section records why;
§§1–8 below describe the mechanics (still largely valid), and §9 holds the
original A-vs-C framing for the record.

**Two base-state facts the review surfaced:**

1. The pre-review draft was written against the **dirty main checkout**, which
   carries ~282 lines of *uncommitted* work-in-progress already attempting a
   Model-A streaming rework — `append_llm_chunk_floored(tag, chunk, floor)` =
   `find_llm_insertion_point(tag).min(floor)`, with the floor computed by the
   fragile `agent_tail_floor_char` backward-scan. This WIP is the **start of the
   very approach this redesign exists to replace**, and it confirms the root
   cause: a scan-located, identity-less draft.
2. The worktree (clean HEAD) does **not** have that WIP, so the reviewers
   correctly observed the floor API "does not exist" there. Either base, the
   conclusion holds: a floor-clamped `find_llm_insertion_point` is a Model-A
   mechanism, and Model A is the wrong model.

**Why Model C wins on the user's own stated priorities** (boundaries,
encapsulation, DRY, *eliminate* the agent-buffer bug class):

- **Encapsulation is real, not cosmetic.** Under A, `WorksheetDraft { start:
  Option<LineAnchor> }` owns an *anchor* while the draft *text* lives in the
  shared transcript rope. The "draft state exists iff in Worksheet mode" claim is
  then false: toggling away leaves orphan draft text in the transcript that must
  be hand-cleared — the exact two-field hand-sync the `Chatbox(Chatbox)` design
  eliminated. Under C, `WorksheetDraft { editor: Editor, … }` owns its own buffer,
  structurally identical to `Chatbox`; the "iff" becomes true.
- **DRY is real, not forced.** Under A, chatbox freeze (append-then-freeze at EOF)
  and worksheet freeze (freeze pre-existing lines in place, blanks and all) are
  *inverse* operations; merging them into one `commit_user_turn` papers over a
  genuine divergence. Under C, worksheet submit appends its editor's text at EOF
  exactly like chatbox → `freeze_as_user_turn` is genuinely the single primitive.
- **The bug class is deleted, not constrained.** A keeps a mixed frozen/editable
  rope and merely *narrows* it (INV-DRAFT-TAIL); failure modes #7/#8/#10 and a
  family of anchor-lifecycle hazards survive. C makes the transcript write-once /
  read-only and the draft a separate editor — the mixed-editing surface, and the
  whole hazard family, become unrepresentable.

**Decisive correctness hazards that are A-specific** (all dissolve under C):

- **Anchor annihilation on undo/redo.** `EditorView::undo`/`redo` call
  `reset_line_anchors`, nuking *all* anchors including the draft's. A's
  `start.line_for_anchor → None` "falls back to EOF" — silently mis-placing the
  next stream and freezing the wrong range on submit. There is no hook to re-seat
  the draft after undo, and INV-DRAFT-IDENTITY ("never recompute by scanning")
  forbids the only fix. Under C the draft is its own editor with its own,
  independent undo stack — no shared-rope anchor to lose.
- **Send/freeze re-entrancy.** A's submit collects line indices / a range, sends,
  then freezes — and the submit path re-borrows the session three times, so a
  server pump landing a chunk in between shifts line numbers and mis-tags streamed
  output as `User(k)`. Under C, submit reads a self-contained editor's text;
  there's no transcript range to invalidate mid-flight.
- **Replay buries or duplicates the draft.** Replay is multi-tick and
  `reset_for_replay` wipes the editor + `reconciler.reset()`. A's "re-seed the
  draft at EOF after rebuild" has no single "after" moment (it streams), so the
  re-seeded draft lands mid-history (violating INV-DRAFT-TAIL) or a pipelined,
  already-submitted turn appears twice. Under C the draft never lives in the
  transcript, so replay rebuilds the transcript and the draft editor is untouched.
- **Reintroduced scroll-jump (responsiveness P2).** A puts the draft *below*
  streaming output in one editor; each streamed line changes the transcript item
  count and (because `block_ranges_active` is ~always true once a convo starts)
  forces `ListState::reset()` → scroll offset nulled → viewport jumps to top while
  the user types in the draft, with the follow-tail mask defeated (the user isn't
  following tail). This is a known-fixed, MEMORY-documented bug structurally
  re-exposed by the A layout. Under C the transcript append path is unchanged from
  today's (already-solved) behavior and the draft is a separate element.

**Net:** Model A's only advantage is the "one continuous cursor across sent +
draft" feel — which the PRD does not require, which A's own tail-only narrowing
largely defines away, and which costs the entire hazard family above. **Proceed
with Model C.** The Model-C design (component shapes, flows, and test plan) is
specified in `design-c.md` once the product owner ratifies the reversal; the
sections below are retained as the superseded Model-A design and the source of the
failure-mode catalog (§1) and the DRY/test intent that carry over.

---

Status (original): **Draft (pre-review)** · Implements: `PRD.md` · Surface: `yalda-gpui`

This is the implementation design for the worksheet redesign. It is grounded in
the current code (`agent.rs`, `agent_ui.rs`, `editor.rs`, `transcript_view.rs`,
`persist.rs`) and a failure-mode trace of the shipped worksheet. Read `PRD.md`
first for product intent.

## 1. Root cause (one sentence)

**The worksheet draft has no identity.** It is defined by *exclusion* ("any line
that isn't frozen") and *located* by a backward scan from EOF
(`agent_tail_floor_char`, `agent.rs:745`). Because the draft is not a thing the
system can name, it cannot be placed reliably, persisted, visually marked,
survived across replay, or protected from streaming — every failure mode below
is a symptom of this one absence.

### Failure modes this fixes (from the code trace)

| # | Symptom | Underlying cause |
|---|---------|------------------|
| 1 | Streaming output lands in the wrong place with prior turns / blank lines present | `agent_tail_floor_char` backward-scan heuristic |
| 2 | Blank-only / spacer drafts never freeze; inconsistent state | "send non-blank, freeze all collected" asymmetry in `submit_worksheet` |
| 3/11 | Pipelined submit: turn rejected by reconciler tripwire, but turn starts anyway, lines left unfrozen | `commit_worksheet_turn` returns `None`, caller ignores it |
| 4 | Draft stranded/lost when toggling Worksheet→Chatbox | toggle abandons the transcript editor's draft |
| 5/12 | Draft lost on app restart and on server reconnect/replay | draft not persisted; `reset_for_replay` wipes the editor |
| 6 | Un-submitted draft lines look identical to blank/system lines | no `TurnId` for draft → blank gutter |
| 7 | Delete of a selection spanning frozen text silently no-ops | `delete_selection` returns false with no signal |
| 8 | Cannot compose anywhere but EOF | frozen-line boundary is hard (this is **correct** under the new model) |
| 13 | Blank-only draft mis-computes the streaming floor | `agent_tail_floor_char` `has_user_text` branch |

## 2. The model

The transcript is **committed content + exactly one live draft region at the
tail.**

```
┌─────────────────────────────────────┐
│  committed (frozen, immutable):      │   U1  user turn 1
│  user turns, llm output, tool blocks │   1   llm output
│  — agent-owned, read-navigable       │   T1  tool block
│  ...                                 │   2   llm output
├──────────── draft_start ─────────────┤  ← explicit LineAnchor
│  the draft: the ONLY editable region,│   ›   draft line
│  always contiguous, always the tail  │   ›   draft line
└─────────────────────────────────────┘  ← EOF
```

Two invariants, both currently unowned, now enforced by a single component:

- **INV-DRAFT-TAIL**: there is exactly one editable region and it is the suffix
  `[draft_start .. EOF)`. Nothing above `draft_start` is editable. (This makes
  failure #8 — "can't compose mid-transcript" — the *intended* behavior, not a
  bug: scattered drafts are incoherent as a prompt and were the source of the
  streaming-placement corruption.)
- **INV-DRAFT-IDENTITY**: the draft's start is a real `LineAnchor`, not a value
  recomputed by scanning. Streaming, freezing, persistence, and the gutter all
  read this one anchor.

This *narrows* PRD PR-1: you compose at the tail (rendered inline, with output
streaming in above it), not at an arbitrary mid-history point. See §9 for the
considered alternative and why this scope is the right one.

## 3. Component boundaries (the architecture)

Strict layering; each layer's responsibility and the things it must **not** know:

### `editor.rs` — generic text editor (UNCHANGED responsibilities)
Owns the rope, `frozen_lines`, `line_anchors`, `line_metadata`,
`append_llm_chunk_floored`, `freeze_as_user_turn`, `can_insert/delete`. Knows
**nothing** about drafts, turns, worksheets, or agents — it is also the markdown
buffer's editor. We do **not** add a `draft` concept here. We only *stop abusing*
"non-frozen = draft" from above. The streaming-above-draft primitive already
exists and is tested (`append_llm_chunk_within_same_turn_inserts_above_draft`,
`append_llm_chunk_preserves_editable_draft_below`).

### `WorksheetDraft` (new, in `agent.rs`) — the draft authority
The single owner of INV-DRAFT-TAIL and INV-DRAFT-IDENTITY. It is the **only**
thing that knows where the draft is or what's in it. Everything that currently
calls `agent_tail_floor_char` or scans for non-frozen tail lines goes through it
instead. `agent_tail_floor_char` is **deleted**.

```rust
/// The single live worksheet draft: the trailing editable region of the
/// transcript editor, delimited below by `start`. Sole authority on the
/// draft's location and contents (replaces the agent_tail_floor_char scan).
pub(crate) struct WorksheetDraft {
    /// Anchor on the first line of the draft region. `None` => empty draft
    /// pinned at EOF. Resolved to a live line via `editor.line_for_anchor`;
    /// a dropped anchor (consumed by a delete) falls back to EOF.
    start: Option<LineAnchor>,
}

impl WorksheetDraft {
    /// Line where the draft begins (EOF if empty/unset). The render input.
    pub fn start_line(&self, editor: &Editor) -> usize;
    /// Char index where streaming output splices in (start of draft, = floor).
    /// REPLACES agent_tail_floor_char — O(1) anchor read, no scan.
    pub fn floor_char(&self, editor: &Editor) -> usize;
    /// The pending draft text, `[start_line..EOF)` joined. Empty => no draft.
    pub fn text(&self, editor: &Editor) -> String;
    pub fn is_blank(&self, editor: &Editor) -> bool; // empty or whitespace-only
    /// Place a fresh draft anchor at the current EOF (after a submit/freeze).
    pub fn reseat_to_eof(&mut self, editor: &mut Editor);
    /// Append `text` as the draft and put the cursor at its end (toggle-in).
    pub fn seed(&mut self, editor: &mut Editor, text: &str);
    /// Delete the draft region's text and reseat the anchor (discard).
    pub fn clear(&mut self, editor: &mut Editor);
}
```

### `InputSurface` (in `agent.rs`) — symmetric compose state
Today: `Worksheet | Chatbox(Chatbox)`. Change to carry draft state in the
Worksheet arm too, so "draft state exists iff in worksheet mode" is type-enforced
(mirrors the existing "chatbox exists iff in chatbox mode" rationale):

```rust
pub(crate) enum InputSurface {
    Worksheet(WorksheetDraft),
    Chatbox(Chatbox),
}
```

`InputModeKind` (the Copy discriminant), `mode()`, `is_chatbox()` unchanged.

### `AgentSession` (in `agent.rs`) — orchestration
Owns `editor`, `input_surface`, reconciler state (`register_user_turn`).
Submit/toggle orchestrate `WorksheetDraft` + the reconciler. The unified freeze
core (below) is shared by chatbox and worksheet.

### `TranscriptView` (in `transcript_view.rs`) — render only
Reads (not owns) `draft_start_line` + gutter tags. Renders the draft marker.
Self-invalidates via a **new seq** for `draft_start_line` (cached-view rule 2).
No `cx.notify()` in render.

### `persist.rs` — durability
`SessionSnapshot` gains `worksheet_draft: Option<String>`. Saved from
`draft.text()`, restored via `draft.seed()` after replay.

## 4. The flows

### 4.1 Submit (Ctrl-Enter) — `submit_worksheet`
```
text = draft.text(editor)
if draft.is_blank(editor):           # whitespace-only or empty
    status = "nothing to send"; return            # no state change (fixes #2)
sent = channel/server.prompt(text)                # send FIRST
if not sent:
    status = "send failed — ⏎ to retry"; return   # draft kept editable (#11)
# success:
k = commit_user_turn(draft.start_line..EOF, text)   # unified freeze core
if k is None:                                       # reconciler rejected (#3/#11)
    status = "turn rejected"; return                # do NOT start a turn
draft.reseat_to_eof(editor)                         # fresh empty draft at tail
turn_phase = begin()
```
`commit_user_turn` is the **single** freeze primitive shared with chatbox: it
calls `register_user_turn` (the reconciler — one source of turn numbering, kills
double-render vs the server echo), freezes the line range, and tags every line
`TurnId::User(k)`. The `None` return is now **handled** (was the silent-fail #3).

### 4.2 Streaming output — `append_llm_chunk`
`floor = draft.floor_char(editor)` instead of `agent_tail_floor_char(editor)`.
When the draft is empty, `floor_char == EOF` → byte-identical to today. When a
draft is pending, output splices at the anchor → above the draft, draft text
untouched. (Fixes #1, #13.)

### 4.3 Toggle Worksheet ⇄ Chatbox — `toggle_agent_input_mode`
Lossless both directions by moving text between the two compose stores:
- **Worksheet → Chatbox**: `t = draft.text(); draft.clear(editor);
  input_surface = Chatbox(Chatbox::seeded(t))`. (Fixes #4.)
- **Chatbox → Worksheet**: `t = chatbox.text(); input_surface =
  Worksheet(WorksheetDraft::default()); draft.seed(editor, t)`.

### 4.4 Persistence & replay
- **Save**: snapshot `worksheet_draft = (mode == Worksheet).then(draft.text())`.
- **Restore / `reset_for_replay`**: after the editor is rebuilt from the server
  log, if a saved draft exists, `draft.seed(editor, saved)`. The draft is **not**
  part of the server log, so it must be re-applied explicitly. (Fixes #5, #12.)

### 4.5 Gutter (visual draft distinction) — `transcript_view.rs`
Lines `>= draft_start_line` that carry no `TurnId` render a distinct draft glyph
(e.g. `›`) in an accent color — never the blank `"   "` used for System/empty
lines. `draft_start_line` is threaded into the render input set with a covering
seq + a `transcript_021_*` regression test. (Fixes #6.)

## 5. DRY consolidation

- **One freeze primitive.** `commit_user_turn(range, text)` replaces the split
  between chatbox's `freeze_as_user_turn` path and worksheet's
  `commit_worksheet_turn`. Both submit flows call it; both get reconciler-based
  numbering for free.
- **One draft authority.** `WorksheetDraft` replaces `agent_tail_floor_char` and
  every ad-hoc "scan non-frozen tail" site. Delete `agent_tail_floor_char`.
- **Reuse the editor primitives.** No new frozen/streaming logic in `editor.rs`;
  we use `append_llm_chunk_floored`, `add_frozen_lines`, `anchor_for_line`,
  `line_for_anchor`, `freeze_as_user_turn` as-is.
- **Reuse `Chatbox` for compose.** Worksheet and Chatbox stay distinct surfaces,
  but the *submit* and *freeze* paths converge, so a future "compose" unification
  is one refactor, not a rewrite.

## 6. Preserving Chatbox & the rest of the agent tile (hard constraint)

- `Chatbox` struct, `submit_chatbox`, the pinned-bottom render, follow-output
  policy: **untouched** except that `submit_chatbox` calls the shared
  `commit_user_turn` (behavior-identical; covered by a regression test).
- `should_follow_tail`, `InputModeKind`, persistence of the mode flag,
  selector/binding, tool-call rendering, subagents: untouched.
- Chatbox regression tests (`transcript_021_chatbox_keystroke_is_render_flat`,
  the seam dedup tests) must stay green unmodified.

## 7. Test plan (agent-buffer invariants — headless)

Each maps to a failure mode; all run under `cargo test` (`tests.rs` /
`verify_harness.rs`, never touching `~/.yalda`).

1. `worksheet_floor_is_anchor_not_scan` — `floor_char` == draft start with
   blank lines and multiple prior turns present (kills #1, #13).
2. `worksheet_streaming_lands_above_draft` — pending draft + `append_llm_chunk`
   → output above `draft_start`; draft text byte-identical and still editable.
3. `worksheet_submit_freezes_exactly_draft_and_reseats` — post-submit: old draft
   range frozen + `User(k)`-tagged; new anchor at EOF; `draft.is_blank()`.
4. `worksheet_submit_blank_is_noop` — whitespace/empty draft → no send, no
   freeze, no turn, no anchor change (#2).
5. `worksheet_submit_send_fail_keeps_draft` — send fails → nothing frozen, draft
   intact, status set (#11).
6. `worksheet_commit_rejected_does_not_start_turn` — `register_user_turn`
   returns `None` → no freeze, no `turn_phase.begin`, status set (#3/#11).
7. `worksheet_submit_no_double_render_vs_echo` — submit + server echo of the same
   turn → one rendered `User` turn (extends the existing seam test).
8. `worksheet_draft_survives_replay` — seed draft → `reset_for_replay` + re-seed
   → draft text preserved (#12).
9. `worksheet_draft_persist_roundtrip` — snapshot save/load preserves draft text;
   chatbox mode persists no draft (#5).
10. `toggle_worksheet_chatbox_preserves_draft_roundtrip` — W→C→W keeps text;
    C→W→C keeps text (#4, #7).
11. `worksheet_gutter_marks_draft_distinct` — draft lines render the draft glyph,
    not blank/System (#6); cache busts when `draft_start_line` changes (rule 2).
12. `worksheet_keystroke_render_count_flat_elsewhere` — typing on an unrelated
    surface keeps `TranscriptView` render count flat (rule 5).
13. Editor boundary (extend existing): delete spanning into frozen is a quiet
    no-op **with a surfaced status**, not a silent drop (#7).

## 8. Out of scope (PRD non-goals + deferrals)

- Rich text, rewinding sent turns, multi-user. (PRD §3.)
- A "clear draft" command and an explicit "sending" in-flight visual state
  (failure #14/#15) — nice-to-have, deferred; called out so they're not assumed
  done.
- `pending_reveal_cursor` is line-number based and can mis-target during
  streaming (#9). Anchoring it is a follow-up ticket, not blocking — note it.

## 9. Key decision & the considered alternative

**Decision: Model A — single tail draft region inside the transcript editor,
delimited by an explicit `draft_start` anchor.** Recommended because it preserves
worksheet's distinctive value (one continuous document; navigate sent history and
compose with one cursor; output streams above an in-document draft) while fixing
the root cause with a minimal, well-bounded change (an anchor replaces a scan).

**Alternative: Model C — draft as a separate `Editor`, rendered inline at the
tail; the transcript becomes purely read-only committed content.** This is
simpler and more DRY (worksheet ≈ chatbox differing only in render location),
makes the toggle lossless by construction, and *deletes* the frozen/editable
mixed-editing surface — eliminating the boundary bug class (#7, #8, #10) outright
rather than constraining it. Its cost: it abandons the "one continuous editable
document, one cursor across sent + draft" feel — arguably reducing worksheet to
"a chatbox rendered higher up."

This A-vs-C fork is the single most consequential decision in the redesign and
the primary thing for adversarial review and the product owner to pressure-test
**before** implementation begins.
