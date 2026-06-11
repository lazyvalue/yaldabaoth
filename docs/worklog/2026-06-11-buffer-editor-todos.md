# Worklog: buffer-editor-todos

**Date:** 2026-06-11
**Branches touched:** buffer-todos (uncommitted) — four "General Buffer TODO" items from untitled.md

## Built (with status)
- **`p` / `P` normal-mode put** — charwise put from the system clipboard. The
  shared `dispatch_normal_core` is `EditOps`-generic and `self`-less, so it
  can't reach the clipboard (that helper lives on `YaldaGpuiView`). Resolved by
  adding a `NormalOutcome::Paste { before }` variant that the `self`-ful callers
  (Edit screen + both Agent dispatch sites — chatbox and Worksheet) resolve via
  a shared `YaldaGpuiView::apply_paste` → `put_text` helper. Charwise (not
  linewise) because yank stores raw text in `pbcopy` with no register
  line/char metadata. `p` inserts after cursor, `P` at cursor; cursor lands on
  the last inserted char (vim convention). Builds + tests pass.
- **`<num>g` / `<num>G` jump-to-line** — added a numeric count-prefix to
  `KeybindManager`: digits accumulate in `pending_count` (leading `0` stays
  bound to `move-line-start` per vim), exposed via `take_count()`. The
  `goto-top`/`goto-bottom` dispatch arms honor the count (`<n>G` / `<n>gg` jump
  to line n, 1-indexed). New core primitive `CursorPos::jump_to_line` +
  `Editor`/`EditorView` delegates + `EditOps::{jump_to_line, line_count}`.
  6 new keybind unit tests + 1 editor test, all pass.
- **ctrl-d / ctrl-u (+ ctrl-f / ctrl-b) paging** — actions were already bound in
  keybind defaults but never dispatched. Wired `half-page-*`/`full-page-*` arms
  that move the cursor by a fixed line count (`HALF_PAGE_LINES=15`,
  `FULL_PAGE_LINES=30`) via a `page_cursor` helper. No viewport-height plumbing
  needed: both the Edit and Agent render paths scroll-to-reveal the cursor line
  every frame, so a cursor move is sufficient. Builds + tests pass.
- **redo** — TODO was stale. Redo already works: `Ctrl-R` is bound and
  `Document::redo` maintains a redo stack with existing roundtrip test coverage
  (`document.rs:614-651`). Ticked the box with a note; no code change.

## Open / unresolved
- Paging moves cursor to column 0 on the target line (via `jump_to_line`)
  rather than preserving the desired column. Acceptable/predictable default;
  revisit if it feels wrong in runtime use.
- Half/full page sizes are fixed constants, not viewport-derived. The
  `self`-less dispatch site can't see live viewport height; the auto-scroll
  render path makes an exact count unnecessary for correctness, but the jump
  distance won't match the visible window exactly.
- Remaining untitled.md buffer TODOs untouched: folio-theme selection styling,
  auto-TODO-on-enter, cursor-off-screen bug, wordwrap, file rename in browser.

## Decisions
- Put is **charwise**, not linewise — no register metadata exists in the
  clipboard-backed yank path; adding linewise-aware put would require a
  register-type flag threaded through yank. Deferred.
- Count prefix lives in `KeybindManager` (read-and-cleared via `take_count`)
  rather than changing `process_key`'s `Option<String>` signature, keeping the
  two non-count call sites untouched.

## Verification status
- Builds clean (`cargo check --bin yalda-gpui`; only pre-existing dead-code
  warnings). Full workspace `cargo test` green: 132 lib + 158 gpui + all crate
  suites, 0 failures. New unit coverage: count-prefix accumulation, leading-zero
  semantics, reset, paste-binding resolution, `jump_to_line` clamp.
- **Needs human runtime check** (GPUI can't be driven headlessly): actual
  keystroke behavior of `p`/`P`, `42G`, and Ctrl-D/U in the running app —
  especially put cursor placement and paging feel.

## Next
- Runtime-verify in `cargo run --bin yalda-gpui`, then commit + integrate the
  buffer-todos branch.
- Consider column-preserving paging and desired-column retention if runtime
  feel warrants it.
