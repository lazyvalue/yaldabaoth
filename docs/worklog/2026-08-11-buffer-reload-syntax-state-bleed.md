# Worklog: buffer-reload-syntax-state-bleed

**Date:** 2026-08-11
**Branches touched:**

- `bug-buffer-syntax-bleed-clean` (`c33878c`)
- `main` (`f4a6678` merge)

## Cog execution evidence

- Graph id: `chl`

### Initial render

```text
graph fix-buffer-syntax-style-bleed (frontiers)
frontier 0: reproduce-buffer-bleed [open]
frontier 1: fix-buffer-bleed [open]
frontier 2: verify-and-ship [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `wnj` `reproduce-buffer-bleed`: claimed → closed; output: localized the
  reload-to-paint failure to `edit_seq` reuse and observed the production-path
  guard fail with old text `let old = 1;` instead of `reloaded plain`.
- `jzz` `fix-buffer-bleed`: claimed → closed; output: added
  `EditorCore::replace_text`, routed Edit and Doc reload through it, and added
  the render-path and generation-contract regression guards.
- `ek9` `verify-and-ship`: claimed → closed; output: focused tests, mutation
  test, full workspace suite, diff hygiene, merge to `main`, release build, and
  worklog validation all passed.
- `wwo` `omega`: claimed → closed; output: aggregated the completed graph after
  every dependency was verified and shipped.

### Notes

- Node `wnj`, seq `4`, topic `root-cause`: replacing the shared core with
  `EditorCore::new` reset `Document::edit_seq` to zero, aliasing the Edit view's
  generation-keyed source and syntax caches.
- Node `jzz`, seq `3`, topic `deviation`: `cargo fmt --all` with rustfmt 1.8.0
  changed 49 unrelated files in the first isolated worktree. Broad restoration
  was safety-rejected, so that worktree was preserved untouched and the scoped
  patch was reapplied in `bug-buffer-syntax-bleed-clean` from current `main`.

### Final status

- Status: `complete`

```text
graph fix-buffer-syntax-style-bleed (dependency tree)
reproduce-buffer-bleed [done] (f0)
└─ fix-buffer-bleed [done] (f1)
   └─ verify-and-ship [done] (f2)
      └─ omega [done] (f3, omega)
```

## Built (with status)

- Fixed Buffer reloads so whole-core replacement advances the shared content
  generation and invalidates text, syntax, WP-kind, and rendered-block caches.
- Added a test-only tap at the real virtualized Buffer row paint boundary and a
  reload regression that checks both the new text and the absence of stale
  fenced-code background.
- Landed implementation commit `c33878c` and merge commit `f4a6678` on `main`.
- Rebuilt `target/release/yalda-gpui` with `cargo build --release` successfully.

## Open / unresolved

- The earlier formatter-contaminated worktree
  `.claude/worktrees/bug-buffer-syntax-bleed` remains untouched for safe manual
  cleanup; none of its unrelated formatting changes were merged.
- The running GUI was not restarted automatically. A restart is needed for an
  already-running process to load the rebuilt release binary.

## Decisions

- No ADR required. This restores the existing invariant that `edit_seq` is the
  invalidation generation for every derived view of a shared Buffer core.

## Verification status

- Negative control: the regression guard failed before the fix because the
  second painted line was `let old = 1;`, not disk text `reloaded plain`.
- Focused guards passed:
  `replace_text_preserves_monotonic_content_generation`,
  `buffer_reload_does_not_reuse_old_syntax_state`, relevant `fence_` tests, and
  `edit_view_keystroke_is_o_changed`.
- Targeted mutation check passed: 1 mutant tested, 1 caught. The initial
  sandboxed attempt could not access Clang's module cache; the approved
  escalated rerun completed successfully.
- `cargo test --workspace` passed on merged `main` (GUI suite: 550 passed,
  0 failed, 1 ignored; live network/auth tests remain intentionally ignored).
- `cargo build --release` passed on merged `main`.
- The headless production-paint tap verifies the relevant rendered style
  decision; exact pixel/color comparison and a live GUI harness are not needed
  for this cache invalidation defect.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-11-buffer-reload-syntax-state-bleed.md` passes.

## Next

- Restart any running Yalda GUI process to load the rebuilt executable.
