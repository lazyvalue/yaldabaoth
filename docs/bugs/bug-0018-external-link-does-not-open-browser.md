# bug-0018: external-link-does-not-open-browser

**Status:** RECURRED→FIXED
**First seen:** 2026-07-23
**Component:** docs/components/buffer (rendered markdown) — link rendering path

## Symptom

Clicking a regular URL link (`[text](https://…)`) in a rendered markdown
document does nothing — it does not open the default browser (Chrome). Wiki
links (`[[foo]]`) work; external URLs don't.

## Context / root cause

Rendered markdown lines are built in `render_blocks.rs::build_wrapped_line`. Only
spans whose `link` begins with `WIKI_LINK_PREFIX` (`"wiki:"`) are collected into
the clickable `wiki_link_ranges` and wired to an `InteractiveText` `on_click`
that calls `open_wiki_link` (`edit_ui.rs`). A regular markdown link stores its
raw `dest_url` in `span.link` (see `render.rs` `Event::Start(Tag::Link)`), with
**no** `wiki:` prefix — so it is filtered out at collection time and gets **no
click handler at all**. Clicking it is inert.

Even if it were wired, `open_wiki_link` only resolves a target to a **local
`.md`/file** relative to the doc dir — it has no branch that opens an external
URL. So there is no path anywhere that launches the browser for a URL link.

Root cause: external URL links are (1) never made clickable and (2) never routed
to an "open externally" action.

## Planned solution

1. In `build_wrapped_line`, collect **every** linked span (not just `wiki:`
   ones) into the clickable ranges, keeping the raw link string.
2. Add a pure classifier `classify_link(raw) -> LinkTarget::{Wiki, External}`
   (`edit_ui.rs`): `wiki:`-prefixed or scheme-less/relative → `Wiki`;
   `http://` / `https://` / `mailto:` → `External`. Restricting the external set
   to those schemes keeps `open` from launching arbitrary local handlers.
3. Add `open_external_link(url)` that shells `open <url>` on macOS (the default
   handler → default browser). The `on_click` dispatches via `classify_link`:
   `External` → `open_external_link`, `Wiki` → `open_wiki_link` (unchanged).

Guard: a `classify_link` unit test (URLs → External, `wiki:` → Wiki, relative →
Wiki) with a negative control (revert the URL branch → URLs misclassify as Wiki,
test RED). The `open` spawn itself is harness gap #2 (live subprocess / external
app — can't verify a browser actually launched headlessly); flagged, not faked.

## Approaches already tried (do NOT repeat)

- Wiring pulldown-cmark link spans only in the rendered-document block path.
  That fixed document links but cannot fix Agent Tile transcript lines:
  `highlight_markdown_lines_stripped` removes the destination before that
  renderer exists. The recurrence must recover/wire links in
  `TranscriptView` / `build_wrapped_line`.

---

## Log

### 2026-07-23 15:32 — opened; localized to the wiki-only link filter

Localized: `render_blocks.rs::build_wrapped_line` only wires `wiki:`-prefixed
spans to a click handler, and `open_wiki_link` has no external-URL branch — so
URL links are inert. Fix in progress per Planned solution.

### 2026-07-23 15:40 — fixed (commit 12b498d)

**Changed:**
- `render_blocks.rs`: `LinkTarget` enum + pure `classify_link(raw)` (near
  `WIKI_LINK_PREFIX`); `build_wrapped_line` now collects EVERY linked span (not
  just `wiki:`-prefixed) into `link_ranges`, and the `on_click` dispatches via
  `classify_link` — `External` → `open_external_link`, `Wiki` → `open_wiki_link`.
- `edit_ui.rs`: `open_external_link(url)` = `open <url>` (macOS default handler
  → default browser), best-effort.
- `tests.rs`: `classify_link_routes_urls_external_and_notes_wiki`.

**Verified:** guard test passes; **negative control observed RED** — disabling
the http/https/mailto branch (`if false && …`) makes `classify_link` return
`Wiki("https://…")`, failing the External asserts (`left: Wiki`, `right:
External`), i.e. a URL would be mis-routed to the local-file resolver. Full suite
412 passed. Committed in isolation (staged only this bug's hunks; a shared tree
held unrelated paragraph-spacing + jump-panel work — stashed with `--keep-index`,
confirmed the committed state compiles + the guard passes, then popped).

**Unverified (harness gap #2):** the actual browser launch from `open` is a live
subprocess side effect — not headlessly testable. The routing decision (the bug)
IS guarded. Needs a human click on a URL link to confirm Chrome opens.

**Outcome:** FIXED on `main`. Binary rebuild + restart needed for the user to get
it (they run `main` release).

### 2026-07-29 22:10 — recurred in Agent Tile transcripts

**Reported:** *"Hyperlinks don't seem to work when they're in agent list
tiles."* Interpreted as rendered links inside the committed Agent Tile
transcript; document-view links from the first attempt still work.

**Different root cause/surface from the first attempt:** bug-0018 originally
wired pulldown-cmark link spans in rendered document blocks. Agent transcript
lines do not use that renderer: `TranscriptView` sends raw committed lines
through `highlight_agent_line` / `build_wrapped_line`, where Markdown syntax and
the destination are stripped before paint. Consequently the document-view
`InteractiveText` handler never exists on an agent line. Repeating the original
`classify_link` change cannot help; routing is already correct once a target
reaches it.

**Planned solution:** recover Markdown label/destination ranges against the
stripped transcript text, split highlighted segments at link boundaries so an
inline link inside ordinary prose owns a real clickable element, and route that
click through the existing `open_link_target` dispatcher using the session cwd
for local paths. Guard the real painted/clicked Agent Tile path with an inline
prose link—not a link-only line—and preserve the source tile when a local target
opens beside it. Observe RED with transcript link wrapping disabled, then run
the full suites and ship on `main`.

### 2026-07-29 22:18 — fixed on the real Agent Tile click path

**Changed:**

- `agent.rs`: recovers `[label](target)` ranges after stripped Markdown
  highlighting, wraps the rendered link segment in a pointer/click element, and
  stops the transcript caret/drag handler from consuming the press. The click
  uses the existing `classify_link` routing: web/mail targets go to
  `open_external_link`; local targets go to `open_wiki_link` relative to the
  session cwd.
- `transcript_view.rs`: supplies that link context only for committed/frozen
  transcript rows; editable compose text remains ordinary editor content.
- `screens.rs`: the non-transcript `build_wrapped_line` callers explicitly pass
  no link context.
- `verify_harness.rs`: `agent_markdown_link_opens_local_file_in_buffer_tile`
  paints an inline link inside ordinary agent prose, clicks its real probed
  bounds, and proves the destination opens as a buffer.

**Negative control observed RED:** replaced the committed-row
`TranscriptLinkCtx` with `None`; the guard failed at
`"inline agent link did not paint"`. Restoring the context made the same real
mouse-click guard green. This differs from attempt one: it verifies the Agent
Tile renderer and interaction tree, not only the shared URL classifier.

**Verification:** `cargo test --features test-support --bin yalda-gpui
--no-fail-fast` → 513 passed, 1 ignored; `cargo test --lib --no-fail-fast` →
161 passed, 2 ignored; `cargo check --bins` → passed. Repository-wide
`cargo fmt --check` still reports extensive pre-existing formatting drift; the
new link hunks have no formatter-reported changes.

**Mutation gate:** deleting link recovery was caught. Broadening the segment
range predicate initially survived because the guard clicked only the intended
label; the guard now also proves the preceding ordinary prose has no link
hitbox. Re-running that exact `&&` → `||` mutant was caught. Two generated
replacement mutants were unviable Rust rather than behavioral survivors.

**Deviation from plan:** `tokenize_inline` already emits the stripped link label
as its own `theme.link` segment, so no second segment-splitting layer was
necessary. The transcript wrapper dispatches through the same
`classify_link`/open methods directly instead of depending on the parallel
`open_link_target` refactor, keeping this bug commit isolated.

**Outcome:** FIXED on `main`. The actual OS browser launch remains the existing
bug-0018 harness gap #2; the recurrence's missing Agent Tile paint/click path is
headlessly covered end to end.
