# Fix intermittent Ctrl-W directional focus

**Date:** 2026-08-20
**Cog graph:** `sci` (fix-ctrl-w-direction-intermittency) — status `complete`
**Branch:** `codex/fix-ctrl-w-direction-intermittency` → merged to `main` (`25496c6`)

## Cog execution evidence

- Graph id: `sci`

### Initial render

```
graph fix-ctrl-w-direction-intermittency (frontiers)
frontier 0: reproduce-routing-race [open]
frontier 1: enforce-prefix-ownership [open]
frontier 2: verify-integrate [open]
frontier 3: omega [open] (omega)
```

### Node execution

Each node was claimed and closed with output (actor `codex`):

- **`kiuv` — reproduce-routing-race** → `done`. A production keymap/paint test
  deliberately contradicted visible Columns order with retained Plane geometry.
  Before the fix, `Ctrl-W h` from the center selected visibly-right `WindowId 3`
  instead of visibly-left `WindowId 2`. A staggered prefix test stayed green,
  ruling out loss of GPUI's pending chord across a render.
- **`ay63` — enforce-prefix-ownership** → `done`. The shell now reserves bare
  `Ctrl-W` before App raw-key handling, Buffer no longer assigns the prefix, and
  `Frame::focus_motion` uses one view-aware resolver shared with directional
  swaps.
- **`obu3` — verify-integrate** → `done`. Adapted to concurrent Tiling
  primary/stack work, merged later concurrent jump-panel work, ran focused,
  GUI-wide, all-target, mutation, and release verification, and merged to main.
- **`321m` — omega** → `done`. Confirmed complete graph, clean scoped branch,
  preserved user changes, green integrated tests, and both release binaries.

### Notes

- The initial timing/capture hypothesis was disproved: `Ctrl-W`, an intervening
  render, then `h` remains a valid pending chord. The recurrence was target
  resolution against stale Plane coordinates while another layout was visible.
- Main advanced twice during verification. The first change made Tiling a true
  dwm primary/vertical-stack layout, so navigation was adapted to its actual
  painted geometry. The later jump-panel reorder work merged without conflict.
- The first sandboxed mutation run could not write the system Metal/clang cache;
  the approved rerun completed its baseline. Five original focus resolver/router
  mutations were caught. After the Tiling adaptation, a second focused run caught
  six more placement/tiling resolver mutations before it was stopped once the
  required negative-control evidence was established. The tool's function filter
  also admitted one unrelated existing `DesktopState::restored` field-deletion
  mutant; that unrelated mutant missed and is not coverage of this change.
- A repository-wide formatter check exposed substantial pre-existing formatting
  drift. No bulk formatter output was retained; an accidental earlier formatting
  spill was fully reversed, and only scoped source/docs changes were committed.

### Final status

- Status: `complete`

```
{"status":"complete","islands":"none","sealed":false}
```

```
graph fix-ctrl-w-direction-intermittency (dependency tree)
reproduce-routing-race [done] (f0)
└─ enforce-prefix-ownership [done] (f1)
   └─ verify-integrate [done] (f2)
      └─ omega [done] (f3, omega)
```

## What shipped

`Ctrl-W h/j/k/l` now resolves focus against the arrangement actually painted:

- **Columns:** `h/l` move across full-height columns; `j/k` are boundaries.
- **Tiling:** `j/k` move within the primary or stack pane; `h/l` cross panes at
  the closest available row.
- **Monocle:** `h/k` move backward and `l/j` move forward through non-wrapping
  reading order.
- **Plane:** all four directions retain true two-dimensional spatial navigation.

The shell owns bare `Ctrl-W` centrally and intercepts it before App-local raw-key
handling. Buffer Code/WP switching remains available from the Buffer tile menu.
Lower-case focus and upper-case tile swaps share the same visible-layout target
resolver, preventing those command families from disagreeing.

## Verification

- Exact RED before fix: visible Columns `Ctrl-W h` returned `WindowId 3`; expected
  visible-left `WindowId 2`.
- `cargo test --bin yalda-gpui ctrl_w_`: **13 passed, 0 failed** after each
  concurrent-main reconciliation.
- `cargo test --bin yalda-gpui`: **674 passed, 0 failed, 2 ignored** on the final
  feature branch.
- `cargo test --all-targets --features test-support --no-fail-fast`: passed on
  integrated main, including **173 library**, **682 GUI**, **49 session-server**,
  all integration suites, and the render benchmark smoke run; only intentional
  ignored/live tests remained ignored.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed.
- `git diff --check`: passed for the scoped feature diff.
- Mutation controls: five original focus/router mutations and six subsequent
  placement/Tiling mutations were caught, including whole-function replacement,
  empty/non-empty inversion, focused-id lookup inversion, and boundary changes.

## Open / caveats

- The running GUI was not restarted; the new release binary will take effect on
  the next app launch.
- Existing user changes in `.claude/scheduled_tasks.lock`, `Cargo.lock`,
  `Cargo.toml`, and untracked `docs/design/` were preserved untouched.
