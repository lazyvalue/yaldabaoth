# bug-0035 — Buffer reload syntax state bleeds

**Status:** FIXED
**First seen:** 2026-08-11
**Component:** Buffer (`docs/components/buffer.md`, `UXI-Buffer-3`)

## Symptom

Syntax highlighting in Buffer edit views sometimes keeps the previous text's
style after a reload. A particularly visible case is an old unclosed code fence:
after the file is replaced on disk with plain Markdown and reloaded, the Buffer
can keep painting the old fenced source instead of the new plain text.

## Context / root cause

`reload_focused_from_disk` replaced the pooled `EditorCore` in place with
`EditorCore::new`. The replacement `Document` restarted `edit_seq` at zero, but
every view sharing the core memoizes derived state by that sequence. If the new
zero matched the view's last observed generation, `EditState::highlight_snapshot`
fast-skipped both line extraction and syntax highlighting and sent the old source
and fence styles back to the virtualized paint closure. Doc and sibling Buffer
views were exposed to the same generation alias.

This is distinct from bug-0033: the Markdown fence state is correct for each
source snapshot; the wrong snapshot survived a whole-buffer reload.

## Solution

Make whole-buffer replacement an `EditorCore` operation that builds fresh text,
tree state, undo state, and metadata while advancing the existing document's
monotonic content generation. Route both Edit and Doc reload branches through
that operation so all generation-keyed consumers invalidate together.

## Approaches already tried (do NOT repeat)

- Do not reset only the focused Edit view's highlight cache. The core is shared,
  and sibling Edit and Doc views have independent derived caches that must all
  observe one generation change.

---

## Log

### 2026-08-11 — fixed and verified

- Added `buffer_reload_does_not_reuse_old_syntax_state`, which drives the real
  `reload_focused_from_disk` action and records the visible line handed to GPUI's
  Buffer paint closure.
- Observed the negative control RED: disk line `reloaded plain` still painted as
  the old `let old = 1;` inside an unclosed Rust fence.
- Added `EditorCore::replace_text`, preserving a strictly changed generation
  across whole-core replacement, and routed both reload branches through it.
- The focused production-path guard is green and checks both the reloaded text
  and removal of the stale code-block background at the virtualized paint edge.
- `replace_text_preserves_monotonic_content_generation` guards the invalidation
  contract at the shared core boundary.
- Negative control observed the intended RED before the fix: expected
  `reloaded plain`, got `let old = 1;`.
- Mutation testing caught the targeted `EditorCore::replace_text` mutant (1/1),
  and `cargo test --workspace` passed (GUI suite: 550 passed, 1 ignored).
- This is a headless render-decision check, so exact pixels and colors are not
  applicable; no live GUI runtime verification is required for the cache bug.
