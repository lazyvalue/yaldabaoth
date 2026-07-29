# Component: System Console

**Status:** living
**Component token:** `SystemConsole` (⇒ invariants are `UXI-SystemConsole-N`)

## Description

The system console is Yalda's drop-down operational surface: a bounded,
monospaced stream of lifecycle messages presented in the same theme-aware
chrome as the jump panel and command menus. It is an overlay rather than a tile,
so summoning it does not replace or rearrange the current workspace.

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

### UXI-SystemConsole-4 — The console is compact, native Yalda chrome

**Statement.** The console is a centered floating panel two-thirds of the
desktop wide and one-third tall, with its top edge one-third of the way down the
screen. Its surface, border, foreground, muted text, accents, warnings, and
errors come from the active Yalda theme; the console does not impose a separate
hardcoded palette. Header and log spacing remain as compact as the rest of
Yalda's operational chrome.

**Applies to.** `SYSTEM_CONSOLE_HEIGHT_RATIO`, `SYSTEM_CONSOLE_WIDTH_RATIO`,
`SYSTEM_CONSOLE_LEFT_RATIO`, `SYSTEM_CONSOLE_TOP_RATIO`,
`render_system_console_overlay`, and `SystemConsoleView::render`.

**Why.** The console is part of Yalda, not a themed application embedded inside
it. A compact, shared visual vocabulary makes the surface useful without
dominating the desktop.

**Status.** `implemented`

**Enforcement.** `system_console.rs::system_console_geometry_stays_centered_and_compact`
guards the footprint; the existing cached-view harness verifies that theme
changes invalidate the console body.

### UXI-SystemConsole-5 — Navigation and branding match the rest of Yalda

**Statement.** Mouse-wheel scrolling works over the log, while `j`/`k` and the
arrow keys scroll by a line and `Ctrl-D`/`Ctrl-U` scroll by half a console page.
The supplied `yaldabaoth-logo.png` is embedded in the app and appears as a dim,
transparent watermark behind console output. The startup splash presents the
same image prominently, and the running macOS app uses it for its Dock and
app-switcher icon.

**Applies to.** `SystemConsoleView::scroll_by`,
`system_console_scroll_delta`, `yaldabaoth_logo_image`,
`install_yaldabaoth_app_icon`, `SystemConsoleView::render`, and `render_splash`.

**Why.** Operational output should use Yalda's established navigation muscle
memory, and the console and boot experience should share one recognizable
identity without reducing log readability.

**Status.** `implemented`

**Enforcement.** `system_console.rs::system_console_navigation_uses_standard_scroll_keys`
guards the keyboard map and embedded PNG; the scroll container's
`overflow_y_scroll`/`track_scroll` wiring provides native mouse-wheel behavior.
