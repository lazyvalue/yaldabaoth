# 012 — Phase 0/1 deferred follow-ups

Small, independent items split out of tickets 010/011 (file-scope or risk
reasons). Each is build+test gated; none need the `CachedPanel` helper.

## Subtasks

- [x] **WP-classify cache.** DONE (2026-06-25, `4b025f8`). `EditState::
      wp_kinds_snapshot` caches the `Vec<WpLineKind>` keyed by `edit_seq` (mirrors
      `lines_cache`/`highlight_snapshot`); `render_edit` reads it. Idle frames now
      recompute zero instead of O(document).
- [ ] **Browser filter debounce + background walk** (audit #2). Each filter
      keystroke runs a synchronous recursive `fs::read_dir`/`metadata` walk
      (depth 8, 200 cap) on the input thread (`file_browser.rs` `rebuild_filtered`
      → `search_recursive`; triggered `browser_ui.rs:270/684`). Add a ~100ms
      cancellable debounce keyed on filter text + run the walk on
      `cx.background_executor()`, apply via notify. Riskier than the 010/011
      edits (cancellable task) — its own pass.
- [ ] **Finish clipboard conversion.** 010/011 converted the 4 `main.rs` copy/paste
      handlers to in-process `cx` clipboard, but the vim yank/put paths still
      shell out to `pbcopy`/`pbpaste` (`edit_ui.rs:780-842`, `agent_ui.rs`
      `apply_paste` ~`edit_ui.rs:882`). Convert these too; then the
      `Self::yank_to_clipboard`/`read_from_clipboard` helpers can be removed.

## Verification

Build + full test suite. Browser debounce + clipboard need a human runtime check
(filter responsiveness on a large dir; copy/paste in doc/edit/agent views).

## Links

`project.md` (tickets 010/011), `audit-report.md` Phase 0/1.
