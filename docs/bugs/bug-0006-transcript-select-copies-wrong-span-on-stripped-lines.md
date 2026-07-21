# bug-0006: transcript-select-copies-wrong-span-on-stripped-lines

**Status:** FIXED
**First seen:** 2026-07-16
**Component:** docs/components/common/selection.md (`UXI-Selection-1`) + agent-tile transcript

## Symptom

Selecting text in an agent transcript and having it auto-copy puts the WRONG text on
the clipboard on lines that contain markdown. Reported example: the visible selection
was an email, but the clipboard got `:** scott+coralpoint@fulcrumo` — a shifted,
garbled fragment (stray `**`, truncated email).

## Context / root cause

(Localized analytically on the real path by a Fable subagent; confirmed by code read.)

Frozen agent lines render markdown-**stripped** segments: `transcript_view.rs:863`
uses `hl.stripped` for `is_frozen` lines (e.g. `**Email:** scott@x` renders as
`Email: scott@x`). `build_wrapped_line` (agent.rs) registers each painted token's
`TokenHit.start_char` as an offset into that **stripped/rendered** text. But the mouse
handlers (`transcript_mouse_down/move`, transcript_view.rs:324-363) feed the hit-test
column straight into the editor cursor/anchor, which live in **raw document** space,
and `selection_text()` slices the raw document. So on a stripped line the rendered
column is used to index raw text: for `**Email:** scott+coralpoint@fulcrumo.com`
(renders `Email: scott+coralpoint@fulcrumo.com`), selecting the rendered email = cols
7..36, and `raw[7..36]` = `:** scott+coralpoint@fulcrumo` — the exact reported garbage.

bug-0003 fixed hit-test *coverage* (the caret line registered no tokens); this is a
distinct **coordinate-space** defect: the registered offsets are in stripped space but
consumed as raw space.

## Planned solution

Register each token's **raw** `start_char` (keeping `char_count` in rendered units so
the monospace width math stays correct): a per-line stripped→raw alignment (stripping
only deletes chars, so rendered is a subsequence of raw — greedy left-match). Then the
hit-test returns a raw column, the editor selection is raw, and `selection_text()`
slices correctly. Symmetrically, the painted selection band (`apply_selection_bg`,
applied to the stripped segs) must convert the raw selection cols → rendered cols.
Non-stripped surfaces (`stripped == raw`) are identity → unaffected.

## Approaches already tried (do NOT repeat)

- <none — first attempt held>

---

## Log

### 2026-07-16 — register raw offsets + band conversion (FIXED)

**Localized by** a Fable subagent (analytic, on the real path); it was blocked from
writing files, so the code + tests were applied by the main session.

**What changed.**
- `agent.rs`: new `stripped_to_raw_cols(raw, stripped)` (greedy subsequence alignment —
  rendered is a subsequence of raw since stripping only deletes) + `raw_to_stripped_col`.
  `build_wrapped_line`'s `reg` closure now registers each token's **raw** `start_char`
  (mapped through that alignment) while keeping `char_count` in rendered units (so the
  monospace width math is unchanged). `None`/identity when nothing was stripped — the
  common editable/raw case and every non-transcript surface.
- `transcript_view.rs`: the selection band (`apply_selection_bg`) now converts the raw
  selection cols → rendered cols on frozen lines so the painted band still matches the
  stripped text.

**How verified.** Guard `transcript_drag_on_frozen_markdown_line_copies_visual_span`:
freezes `**Email:** scott+coralpoint@fulcrumo.com` (renders `Email: <email>`), drags
across the painted email token via real `simulate_mouse_*`, asserts the clipboard
holds the full email and no `*`. **Reproduced RED first** (clipboard =
`:** scott+coralpoint@fulcrumo` — the exact reported garbage). **Negative control:**
reverting the raw-offset map (register stripped `start_char` again) reproduces that
garbage → asserts fail RED; restored → green. Existing
`transcript_drag_autocopies_selection_to_clipboard` +
`transcript_drag_on_focused_caret_line_copies_that_line` still pass. Full suite: 379.

**Known limitation (documented, not fixed here).** Selecting a rendered range that
SPANS an internal stripped marker (e.g. dragging the whole `**Email:** <email>` line)
copies the contiguous raw slice, which includes the interior `**` — because
`selection_text()` returns raw document text, not the rendered projection. Selecting a
token that has no interior stripping (the reported email case) is exact. Making copy
return the rendered projection across a multi-line selection is a larger, separate
change. Runtime-unverified (headless-green; real macOS drag pixels = gap #1).
