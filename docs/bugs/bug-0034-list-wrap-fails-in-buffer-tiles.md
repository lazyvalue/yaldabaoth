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
2. A long UNBROKEN token (path/URL, no spaces) does NOT wrap in the doc view —
   paragraph AND bullet both stay one line (22.5px) and overflow. This is a
   SHARED gap (no char/overflow-wrap), not bullet-specific — but a user whose
   bullets hold long paths would notice it there first. Candidate root cause.
3. Narrow-width headless numbers are UNRELIABLE: `simulate_resize(360×700)`
   leaves the probe x-origin at 368 (unchanged) and collapses BOTH paragraph and
   bullet to min-content (~91px, word-per-line). Artifact of the virtualized
   list not reflowing under `simulate_resize`, not a real signal. Could NOT use
   it to isolate a bullet-only collapse.

**Could not localize a bullet-specific wrap failure on the real render path.**
Per the anti-circling rules, no speculative fix was shipped.

Two plausible real causes remain, both needing a concrete repro to confirm:
- (A) unbroken-token char-wrap gap (finding #2) — real, shared by all prose.
- (B) `ListSizingBehavior::Auto` on the edit lists min-content-collapsing at
  narrow (split-pane) widths — suspected but not cleanly reproduced headlessly
  because `simulate_resize` doesn't reflow the list.

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

Outcome: could not reproduce a bullet-specific wrap failure at a valid width;
found a shared unbroken-token char-wrap gap as the leading candidate. Left OPEN
pending a concrete repro (view + markdown + pane width). No code committed —
shipping a fix here would be a guess against an un-localized symptom.
