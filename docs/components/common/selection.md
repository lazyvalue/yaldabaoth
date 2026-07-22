# Component: Selection (common)

**Status:** implemented
**Component token:** `Selection` (⇒ `UXI-Selection-N`)

## Description

X11-style copy-on-select: finishing a mouse drag-selection over a read-only
reading surface writes the selected text to the system clipboard automatically —
no `Cmd-C` required — so the very next `Cmd-V` pastes it. The behavior is shared
by the two selectable reading surfaces: the buffer doc view (`YaldaView`) and the
agent transcript (`TranscriptView`). It follows the X11 "select = copy"
convention, using the ordinary macOS clipboard since macOS has no separate PRIMARY
buffer.

## References

- INV-UX-14 in `docs/ux-invariants.md` → migrated here.
- `docs/components/agent-tile/README.md` — the transcript facet consuming this.

## UX invariants

### UXI-Selection-1 — Selecting text auto-copies it to the clipboard (X11-style)

**Statement.** Finishing a **mouse drag-selection** over a read-only reading
surface writes the selected text to the **system clipboard automatically** — no
`Cmd-C` — so the very next `Cmd-V` pastes it. This is the X11 "select = copy"
convention (macOS has no separate PRIMARY buffer, so the ordinary clipboard is
used). Applies to both selectable reading surfaces:

- **Buffer doc (Viewing / `YaldaView`)** — the existing click-drag selection
  (`doc_selection`, `doc_mouse_*`, hit-tested via per-line `TextLayout`s in
  `line_layouts`) copies on `doc_mouse_up` when the finalized selection is
  non-empty. `Cmd-C` still works; auto-copy is additive.
- **Agent transcript (`TranscriptView`)** — a drag selects transcript text and
  copies on release. Because each transcript line is rendered as a `flex_wrap`
  row of many **monospace** tokenized `styled_line_element`s (not one hittable
  `StyledText`), hit-testing uses a **paint-time token sink**: each painted token
  registers its window-space bounds + covered `(line, start_char, count)` via
  `register_token_on_paint`, and `hit_test_tokens` maps a point → `(line, char)`
  by the token's own width (`width / char_count`, exact for monospace). The drag
  drives the transcript editor's anchor/head selection (the SAME model the
  keyboard selection band renders from, gated on `AgentFocus::Transcript`), so a
  mouse-down also focuses the transcript. The caret is **suppressed while
  dragging** (`TranscriptView.dragging`) so every visible line takes the uniform,
  registerable non-cursor render path.

**Applies to.** `main.rs`: `doc_mouse_up` (buffer). `transcript_view.rs`:
`transcript_mouse_down` / `_move` / `_up` + the `token_hits` sink cleared in
`build_body` and refilled by `RegisterTokenOnPaint` (`render_blocks.rs`).
`agent.rs`: `build_wrapped_line` (`token_sink` / `line_idx` params).

**Why.** The transcript and doc view are reading surfaces; the muscle-memory of
"drag to grab a line of agent output, paste it elsewhere" should not require a
second chord. Copy-on-select is the lowest-friction path.

**Non-goals / bounds.** The buffer raw **Edit** view (Code/WP) has keyboard
selection only — no mouse drag, so nothing to auto-copy there yet. Character
precision on the transcript relies on the surface being monospace; a
proportional transcript font would need per-token `index_for_position` instead.

**Status.** `implemented` (headless — the drag is driven through the real
`simulate_mouse_*` path and the clipboard is read back).

**Enforcement.** `verify_harness.rs`: `doc_drag_autocopies_selection_to_clipboard`
(buffer) and `transcript_drag_autocopies_selection_to_clipboard` (agent) — each
seeds a sentinel clipboard value, drags across a known line via real mouse
events, and asserts the clipboard now holds the dragged text (negative control:
disabling the `write_to_clipboard` leaves the sentinel ⇒ RED).

### UXI-Selection-2 — A drag-selection never moves the content under the pointer

**Statement.** While a mouse drag-selection is in flight over the transcript, the
painted layout of the content being selected MUST NOT move. Concretely: the
transcript's flat-item list keeps a **stable item count** for the whole gesture,
so pressing / dragging inside a multiline block (code fence, list, table) cannot
reflow the block under the pointer.

**Why this is load-bearing (bug-0015, a repeat offender).** The transcript's
blank-line collapse is cursor-dependent: it protects the line the caret sits on
(the Worksheet editable tail). A press moves the caret to the clicked line, which
*un*-protects the previously-protected blank; that blank collapses away, the list
loses one item, and the whole block repaints ~25px lower — **mid-drag, under the
pointer**. 25px > the 20px line height, so every subsequent `hit_test_tokens`
comes back a line off and the drag selects the wrong lines. To the user this
reads as "I can't select in a code block" — the text is right there, but the drag
grabs the wrong span (or leaves the block entirely on a short block).

**Mechanism of the guarantee.** `AgentState.drag_protect_line: Option<usize>` is
captured at `transcript_mouse_down` to the caret's **pre-press** line and cleared
at `transcript_mouse_up` (and on a no-hit press). `rebuild_agent_view_model`'s
collapse protects `drag_protect_line.unwrap_or(cursor.line)`, so for the duration
of the gesture the protected line — and therefore the item count — is frozen at
its pre-press value. The field is part of the view-model memo key (a real build
input).

**Bounds.** This freezes the count only for the drag. A bare click that parks the
caret inside frozen content still statically reflows the block once after
mouse-up (the caret legitimately moved); that is a cosmetic post-click shift, not
a selection failure, and is out of scope here. The durable elimination of the
whole class (decouple the transcript selection anchor/head from the document
caret so a press has no layout side effect at all) is recorded as the preferred
long-term shape in `docs/bugs/bug-0015-*.md` if this ever recurs.

**Status.** `implemented` — headless on the REAL paint path.

**Enforcement.** `verify_harness.rs::code_block_does_not_shift_when_clicked`:
seeds a frozen fenced code block with a trailing editable blank, records the
block's four painted band tops, drives the REAL `transcript_mouse_down` inside
the block, and asserts the band tops are IDENTICAL after — then drags to the
second code line and asserts the clipboard holds BOTH lines. Non-vacuous (asserts
the block painted its CONTENT-line bands first — since bug-0017 that is the two
`let …` lines, not the fence-inclusive four). **Negative control (observed RED):**
revert `protect_line` to the bare `cursor().line` ⇒ the bands shift +25px and the
equality assert fires.

### UXI-Selection-3 — Selecting inside a parsed code block is aligned and visible

**Statement.** A mouse drag inside a fenced **code block** in the agent transcript
MUST (a) hit-test to the raw source line under the pointer — the hit band for each
rendered code line is that line's OWN painted bounds — and (b) paint the selection
**highlight** on the selected span, exactly like prose. Selecting code is not
"model-only": the user sees the highlight and the copied text matches it.

**Why this is load-bearing (bug-0017; the third face of "can't select code",
after bug-0008 and bug-0015).** A code block renders as a single `FlatItem::Block`,
not per-line `FlatItem::Line`s, so it bypassed BOTH prose selection mechanisms:

- **No highlight was ever painted.** The block's `RenderCtx` was built with
  `doc_selection: None`, and nothing else draws a selection background inside a
  block. The editor selection + copy-on-release worked, so every headless
  band/clipboard probe passed — while the user saw *nothing move* and reported
  "cannot select." (This is why bug-0008's hit bands and bug-0015's reflow freeze
  never changed the experience: both fixed invisible layers.)
- **The hit bands were misaligned.** They were an EVEN split of the block's full
  outer height over its raw line range — but that range is **fence-inclusive**
  while the block paints only the *content* lines plus a `p_2` pad and an optional
  `[lang]` header. So bands drifted off the glyphs and a click landed a line away.

**Mechanism of the guarantee.** `RenderCtx::block_hits` (`BlockHits { sink,
raw_base, selection, sel_bg }`) is set only for a transcript code block (`raw_base
= range.start + 1`, skipping the opening fence). `doc_styled_line_element` then, per
content line: registers a `TokenHit` from the line's **own painted bounds** into the
`token_hits` sink (correct alignment, immune to padding/header), and applies the
selection background to the run range covering the raw-line selection
(`line_selection_range`). The even-split `register_block_hits_on_paint` path is
retained only for tables. A companion correctness fix pairs `FlatItem::Block`s
against **parsed-only** ranges (`resolved_blocks` filtered), so a detected-but-
unparsed range above a code block can't shift its `raw_base`.

**Bounds.** Tables (the other `FlatItem::Block`) still use the even-split band path
and paint no in-cell highlight yet — not reported, tracked as follow-up. Nested code
blocks inside a blockquote/list (`block_hits: None` in the recursion) are also out
of scope.

**Status.** `implemented` — headless on the REAL paint path.

**Enforcement.** `verify_harness.rs::code_block_selection_is_painted_and_aligned`:
freezes a fenced ```` ```rust ```` block **with a language header**, asserts the hit
bands cover the CONTENT lines and NOT the ``` fence lines, then drives the REAL
`transcript_mouse_down`/`_move` across two code lines and asserts (via the paint tap
`DocRenderTap.block_selection`) that the selection highlight PAINTED on those lines,
and that release copies the code. **Two negative controls, each observed RED:** (a)
`block_hits: None` at the block arm ⇒ the fence lines reappear in the bands; (b)
disable `apply_selection_bg_to_runs` ⇒ the highlight tap is empty ("no selection
highlight was painted").
