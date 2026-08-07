# bug-0034: list-wrap-fails-in-buffer-tiles

**Status:** OPEN
**First seen:** 2026-08-06
**Component:** docs/components/TextEditing, docs/components/common (markdown render)

## Symptom

Reported: "Word wrapping in buffer tiles often fails on bullet points / lists."
The user sees list-item text NOT wrap to the pane width (runs off the edge / does
not reflow) in a Buffer tile. "Often" ⇒ intermittent, not every list.

Not yet reproduced with an exact document + pane width + view (Viewing vs Editing).

## Context / root cause

A Buffer tile has three text surfaces, all checked headlessly this session:

- **Doc view (Viewing, `render_doc` → `block_element` → `block_inner`/`List`)** —
  list items get a definite width via the `flex_1().min_w_0()` content column
  (block_element, `render_blocks.rs:1403`) and, per item, `list_item_element`'s
  own `flex_1().min_w_0()` column (`render_blocks.rs:1704`). This is the exact
  shape the UXI-AgentTile-26 fix added for the transcript path.
- **Code edit view (`build_edit_body_code`)** and **WP edit view
  (`build_edit_body_wp`)** — `gpui::list` with `ListSizingBehavior::Auto`, each
  row `.w_full()`, content via `build_wrapped_line` (whitespace-token
  `flex_wrap`, `agent.rs:1162`).

**What the investigation showed (headless, `layout_probe`):**

1. At normal/wide window width, a long whitespace-separated line wraps
   IDENTICALLY as a paragraph and as a bullet in all three surfaces:
   - Doc: paragraph inner 1507×45.5 vs bullet inner 1507×45.5 (both 3 lines).
   - WP: paragraph 1510×45.5 vs bullet 1510×51.5 (bullet taller only by the
     UXI-ParagraphSpacing-1 list gap).
   → **No bullet-specific whitespace-wrap failure at valid widths.**
2. ~~A long UNBROKEN token does NOT wrap in the doc view.~~ **CORRECTED
   2026-08-07:** this was a measurement error — the ~130-char test token simply
   *fit* the 1507px pane, so it never needed to wrap. Re-tested with a genuinely
   over-long token (600 `a`s, far wider than any pane): the doc view **does**
   char-wrap it — paragraph AND bullet both became ~6 lines (90.5px). So gpui
   `StyledText` breaks over-long words at char boundaries. **The rendered doc
   view has no wrap defect** (whitespace wrap AND char-wrap both work, bullets ==
   paragraphs). The edit views' `build_wrapped_line` (whitespace-token
   `flex_wrap`) would NOT char-break a single over-long token, but that is a
   niche case (a 600-char unbroken token while typing) clipped by
   `overflow_x_hidden`, and is not "wrapping fails on bullets".
3. Narrow-width headless numbers are UNRELIABLE: `simulate_resize(360×700)`
   leaves the probe x-origin at 368 (unchanged) and collapses BOTH paragraph and
   bullet to min-content (~91px, word-per-line). Artifact of the virtualized
   list not reflowing under `simulate_resize`, not a real signal. Could NOT use
   it to isolate a bullet-only collapse.

**Could not localize a bullet-specific wrap failure on the real render path.**
Per the anti-circling rules, no speculative fix was shipped.

One plausible real cause remains, needing a concrete repro to confirm:
- `ListSizingBehavior::Auto` on the EDIT lists min-content-collapsing at narrow
  (split-pane) widths — suspected but not cleanly reproduced headlessly because
  `simulate_resize` doesn't reflow the virtualized list. (Finding #2's
  unbroken-token idea is RETRACTED — the doc view char-wraps fine.)

## Planned solution

Blocked on a concrete repro from the user:
- Which view — Viewing (rendered) or Editing (Code / WP)?
- The exact list markdown (do the items contain long unbroken paths/URLs?).
- The pane width (single tile full-width, or a narrow split?).

Then either: (A) enable overflow/char wrapping for unbroken tokens on the
identified surface, or (B) fix the edit-list Auto-sizing collapse at narrow
widths (a live-app `sample`/screenshot at a narrow split is the honest oracle,
since `simulate_resize` is an unreliable headless proxy here).

## Approaches already tried (do NOT repeat)

- Assuming the doc-view list collapses like the UXI-AgentTile-26 transcript bug
  did — DISPROVEN headlessly: doc-view bullets wrap identically to paragraphs at
  valid widths (the `flex_1().min_w_0()` double-nest already holds).
- Assuming unbroken tokens don't char-wrap in the doc view — DISPROVEN: gpui
  `StyledText` char-wraps a 600-char token into ~6 lines (the earlier "1 line"
  reading was a token that FIT the pane). Do NOT ship a char-wrap "fix" for the
  doc view; there is nothing to fix there.
- Trusting `simulate_resize` narrow-width layout numbers — they are an artifact
  (x-origin unchanged, both blocks collapse to min-content). Do NOT localize a
  narrow-width bug from them; use a live-app read instead.

---

## Log

### 2026-08-06 — investigation, no fix shipped

Drove the real doc view (`test_open_doc`) and WP edit view (`test_open_edit` +
`toggle_edit_view`) headlessly with a `layout_probe` on `doc-block-inner-{i}` and
a temporary `wp-line-{i}` probe. Compared a long paragraph vs a long bullet with
(a) whitespace-separated text and (b) an unbroken path. Results as in
"Context / root cause" above. Reverted all scratch instrumentation
(`git checkout`); tree clean; `cargo build --bin yalda-gpui` green.

Outcome: could not reproduce a bullet-specific wrap failure at a valid width.
Left OPEN pending a concrete repro (view + markdown + pane width). No code
committed — shipping a fix here would be a guess against an un-localized symptom.

### 2026-08-07 — retraction of the unbroken-token candidate

Re-tested with a 600-char unbroken token (far wider than any pane): the doc view
char-wraps it into ~6 lines (paragraph AND bullet). So the doc view has NO wrap
defect — the earlier "unbroken tokens don't wrap" finding was a measurement
error (a ~130-char token that fit the 1507px pane). Corrected finding #2 above.
Only remaining suspect is edit-list `ListSizingBehavior::Auto` collapse at narrow
split-pane widths, which needs a live-app read (not `simulate_resize`). Still
OPEN; still no speculative fix.
