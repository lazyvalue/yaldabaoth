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
