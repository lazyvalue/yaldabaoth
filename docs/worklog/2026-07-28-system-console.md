# Worklog: Doom-style system console

**Date:** 2026-07-28
**Branches touched:** `feature/system-console`

## Built

- Replaced the empty jump-panel PINNED placeholder with a global System Console
  row and added the same entry to the `?` global menu.
- Added a cached, drop-down console overlay with a bounded 1,000-row
  `INFO` / `WARN` / `ERROR` / `CMD` lifecycle log persisted at
  `~/.yalda/system-console.log`.
- Routed the existing GUI-only and GUI+server self-rebuild commands through the
  console, streaming Cargo stdout/stderr as it arrives and rejecting concurrent
  builds.
- Relaunches the release binary with the console reopened, preserving compiler
  and process-boundary messages across replacement.
- Added `UXI-SystemConsole-1..3` and reconciled Jump Panel, Project, Menu, and
  palette documentation.

## Verification

- `cargo check --bin yalda-gpui`: passed.
- `cargo test --bin yalda-gpui`: 501 passed, 1 ignored.
- `cargo test --lib`: 160 passed, 2 ignored.
- Focused console unit + headless interaction guards: passed.
- `git diff --check`: passed.
- The headless guard drives the real global-menu dispatch, a real click on the
  painted jump-panel row, `r` / `R` key capture into the rebuild dispatcher,
  bounded persistence, cached render-flat behavior, and theme invalidation.

## Open / unresolved

- **NEEDS-RUNTIME:** execute `r` in the running GUI and confirm Cargo lines
  visibly stream, the old window closes after success, the new release window
  opens with the console still visible, and live agent sessions reattach.
- Log scope intentionally remains lifecycle + build output until the useful
  broader application logging level is known.

## Decisions

- No ADR: the overlay follows the existing global-overlay and yux cached-view
  architecture. The deliberately narrow initial log policy is captured in
  `UXI-SystemConsole-3`.

## Next

- Runtime-check both `r` and `R`, then tune the panel height/colors only if the
  live Folio/Nightfox result calls for it.
