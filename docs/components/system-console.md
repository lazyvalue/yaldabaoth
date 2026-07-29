# Component: System Console

**Status:** living
**Component token:** `SystemConsole` (⇒ invariants are `UXI-SystemConsole-N`)

## Description

The system console is Yalda's drop-down operational surface: a bounded,
monospaced stream of lifecycle messages with a dark, high-contrast
"Doom console" treatment. It is an overlay rather than a tile, so summoning it
does not replace or rearrange the current workspace.

The first deliberately narrow log policy is **Yalda lifecycle plus build output**:
startup, rebuild/relaunch state, Cargo stdout/stderr, warnings, and failures.
The store preserves recent lines across a GUI relaunch. Broader application
logging and user-selectable levels are deferred until the useful signal level is
known.

## References

- [jump-panel.md](jump-panel.md) — the permanent `SYSTEM CONSOLE` navigator row.
- [common/menu.md](common/menu.md) — the global-menu entry point.
- `src/bin/yalda-gpui/system_console.rs` — log store, cached view, persistence,
  and console rendering.
- `src/bin/yalda-gpui/main.rs` — overlay lifecycle and rebuild/relaunch command.

## UX invariants

### UXI-SystemConsole-1 — The console is summonable without changing workspace layout

**Statement.** Selecting **system console** from the `?` global menu or clicking
the first row in the jump panel opens the same overlay. It drops over the upper
part of the current screen, preserves the workspace beneath it, and `Esc`
dismisses it without changing the focused tile.

**Applies to.** `ActiveOverlay::SystemConsole`, `open_system_console`,
`render_system_console_overlay`, `global_menu`, and `render_jump_panel`.

**Why.** A system console is transient operational chrome, not user content; it
must be reachable globally without consuming or replacing a tile.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::system_console_opens_from_global_menu_and_jump_panel`.

### UXI-SystemConsole-2 — Rebuild output is live, durable, and actionable

**Statement.** The console offers two explicit commands: `r` rebuilds and
relaunches the GUI while leaving the session server running; `R` rebuilds and
relaunches both GUI and server. Cargo stdout/stderr and lifecycle messages append
to the console while the command runs. Recent lines are bounded and persisted
under Yalda's durable home, the relaunched GUI reopens the console, and the new
process shows the preceding build/relaunch messages.

Only one rebuild may run at once. While it is active, another rebuild request is
reported and ignored.

**Applies to.** `SystemConsoleView`, `ConsoleLog`, `dev_rebuild_restart`,
`YALDA_OPEN_SYSTEM_CONSOLE`, and the console key/click handlers.

**Why.** A self-rebuild that hides its compiler output is impossible to diagnose,
and quitting immediately after success would otherwise erase the only feedback
that the relaunch path worked.

**Status.** `implemented`; process replacement remains a genuine runtime check.

**Enforcement.** Unit guards cover bounded persistence and level classification;
the verification harness covers both rebuild dispatch paths and the cached-view
render-count contract. Runtime check: run `r` and confirm streamed Cargo output,
window replacement, console reopening, and session reattachment.

### UXI-SystemConsole-3 — Logging starts intentionally narrow

**Statement.** Until a broader level policy is chosen, the console shows system
lifecycle messages and build output only. Every row carries one of
`INFO`, `WARN`, `ERROR`, or `CMD`; warnings/errors have distinct colors, and
unclassified Cargo output is `INFO`. The store retains at most 1,000 recent
lines, so long compiler output cannot grow the process without bound.

**Applies to.** `ConsoleLevel`, `classify_build_line`, `ConsoleLog::push`, and
`SystemConsoleView::render`.

**Why.** This delivers the requested operational signal without dumping every
agent/transcript/internal event into a noisy surface before the useful log level
is understood.

**Status.** `implemented`

**Enforcement.** `tests.rs::system_console_log_is_bounded_and_classifies_build_output`.
