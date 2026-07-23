# bug-0018: external-link-does-not-open-browser

**Status:** IN-PROGRESS
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

- <none yet — first attempt>

---

## Log

### 2026-07-23 15:32 — opened; localized to the wiki-only link filter

Localized: `render_blocks.rs::build_wrapped_line` only wires `wiki:`-prefixed
spans to a click handler, and `open_wiki_link` has no external-URL branch — so
URL links are inert. Fix in progress per Planned solution.
