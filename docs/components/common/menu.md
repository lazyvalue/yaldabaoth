# Component: Menu (leader command panel)

**Status:** living
**Component token:** `Menu` (⇒ invariants are `UXI-Menu-N`)

## Description

The **command panel** is the overlay that opens on a leader key when the focused
tile is NOT in text entry — the progressive-disclosure surface for leader-key
commands (which-key-style, but the *primary* binding surface, not a viewer over an
existing keymap; see `spec-menu-scopes.md`). Three leaders open it:

- **`space`** — tile/app-local menu (entries depend on the focused content kind:
  Doc / Edit / Agent / Browser / Linear / Keys).
- **`.`** — workspace menu (new tile, theme, plane view, marks, restart, …).
- **`?`** — global menu (workspace list + workspace/project ops + system console).

All three share one mechanism: `ActiveOverlay::Menu(MenuOverlay)` on
`YaldaGpuiView`, a `MenuState` (descent `path: Vec<usize>` over a static
`Vec<MenuNode>` tree) shared with the TUI, and one renderer — `render_menu_overlay`
(`src/bin/yalda-gpui/main.rs`). A keypress at the current depth either **descends**
into a submenu or **dispatches** a command (`MenuState::process_key`); `Esc` pops one
level or closes at root; a focus change dismisses (no stale dispatch).

This spec owns the **visual/rendering contract** — the panel's shape, placement,
anatomy, and identity. The *behavioral* state machine (leaders, descent, dispatch,
disabled entries, scopes) is owned by `spec-menu-scopes.md` and must not change.

### The design — "The Sigil Card"

A **floating, content-sized card**, not a full-width drop-down bar. It floats in the
content region (right of the jump panel), horizontally centered, pinned a fixed
distance below the top chrome. Its width tracks its content within a
`[MENU_PANEL_MIN_W, MENU_PANEL_MAX_W]` band, so on a wide monitor the screen stays
visible on all four sides instead of a left-hugging bar with dead space on the right.

Each leader wears a **scope hue** on a 2px left accent bar + a sigil in the header —
the house "the focused thing wears a left bar" language (doc-view cursor, agent
frozen bar). The header's breadcrumb is the literal **keystroke trail** you typed,
rendered as key chips, so every visit teaches the exact chord that reaches this
level — a menu whose goal is its own obsolescence.

## References

- `docs/decisions/0029-command-panel-is-a-floating-elevated-sigil-card.md` — the ADR
  recording *why* each of these visual decisions was made (placement, elevation,
  scope hues, the keystroke trail, keyboard-only) and the alternatives rejected.
- `docs/specs/spec-menu-scopes.md` — the leader model + behavior (global `.` /
  local `space` / global `?`, breadcrumb, Esc, disabled entries, scopes). This spec
  renders that state machine; it does not alter it.
- `docs/components/jump-panel.md` — the permanent left sidebar the panel floats to
  the right of (`JUMP_PANEL_WIDTH`).
- `render_project_menu` (`main.rs`) — the aesthetic donor (rounded popup, shadow,
  hover pill, glyph vocabulary `⊞ ✦ ✕`) this generalizes.

## UX invariants

### UXI-Menu-1 — the panel floats content-sized in the workspace region

**Statement.** When a leader menu is open the panel is an absolutely-positioned,
**content-sized floating card** — NOT a full-window-width bar. Its width is clamped
to `[MENU_PANEL_MIN_W (340px), MENU_PANEL_MAX_W (720px)]` and is strictly less than
the window width on any monitor wider than `MENU_PANEL_MAX_W`. It is **left-anchored**
just past the jump panel — its left edge sits at
`JUMP_PANEL_WIDTH + MENU_PANEL_LEFT_PAD (30px)` when the panel is visible (else
`MENU_PANEL_LEFT_PAD`), just inside the tile region but offset enough not to line up
flush with the first tile's edge — and pinned a fixed `MENU_PANEL_TOP (48px)` below
the top of the window. It grows down/right from
that anchor; it is not centered.

**Applies to.** `render_menu_overlay` (`main.rs`); the wrapper that composites it
over `screen_view` at the end of `render`. Constants `MENU_PANEL_MIN_W`,
`MENU_PANEL_MAX_W`, `MENU_PANEL_TOP`, `MENU_PANEL_LEFT_PAD`.

**Why.** The old `.w_full().top_0()` bar wasted the entire right half of a wide
monitor and read as an intrusive drop-down rather than a deliberate command surface.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::menu_panel_floats_in_content_region` — on the
1920px test display, opens a leader menu, layout-probes the card, and asserts
`x ≥ JUMP_PANEL_WIDTH`, `width ≤ MENU_PANEL_MAX_W`, `width < window_width`, and the
card's horizontal center ≈ content-region center. Negative control: restore
`.w_full()` ⇒ width == 1920 and center off ⇒ RED.

### UXI-Menu-2 — every entry is a key-chip + label row on a fixed grid

**Statement.** Command/submenu entries render as a fixed-height row: a right-aligned
**key chip** (mono, tinted `overlay.key` background) at a shared left gutter, then the
label (`overlay.fg`). Submenu rows carry the `overlay.accent` label color and a
right-edge `▸` chevron. Disabled entries dim (chip + label to `overlay.label`) and do
not dispatch. Sections group under a mono uppercase section label and never split
across columns; column count scales to item count (`≤10→1`, `≤20→2`, else `3`).

**Applies to.** `render_menu_overlay` entry/section rendering; `OverlayTheme`
(`key`, `fg`, `accent`, `label`, `border`); `agent.frozen_bar` (hover pill).

**Why.** A shared key-gutter + fixed row height makes the panel scan as a grid and
keeps the label left-edge stable across submenu levels (the static-render substitute
for motion); the chip vocabulary matches `render_project_menu`.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::menu_panel_rows_and_sections_paint` (structural:
rows/section labels paint within the card bounds; column count matches item count).
Exact chip colors/tints are the documented paint/pixel gap (`NEEDS-RUNTIME`).

### UXI-Menu-3 — the header shows the keystroke trail that reached this level

**Statement.** The panel header renders the **literal chord** taken to reach the
current depth as a sequence of key crumbs: the leader glyph (`␣` / `.` / `?`) followed
by each descended submenu key, ending in the current level's name. At root the trail
is just the leader glyph + scope name; each descent appends its key. The trail is
derived from `MenuState::path` over the menu tree (no new state).

**Applies to.** `menu_trail_crumbs` (pure builder in `main.rs`, over `menu` + `path`
+ leader); `render_menu_overlay` header.

**Why.** A leader-key UI's entire skill ceiling is learning the chords; showing the
exact sequence back to the user each visit trains them to stop needing the menu.
Functional, not decoration — it replaces a bare "Commands" breadcrumb.

**Status.** `implemented`

**Enforcement.** `tests.rs::menu_trail_crumbs_tracks_descent` — unit-tests the pure
builder: root yields `[leader, scope]`; after descending one submenu the trail
appends that submenu's key and its label. Negative control: return only
`current_label` ⇒ the descended-key crumb is missing ⇒ RED.

### UXI-Menu-4 — the scope-hued left accent bar and stable top edge

**Statement.** The card carries a 2px full-height **left accent bar** in the leader's
scope hue (`space`→`agent.frozen_bar`, `.`→`overlay.key`, `?`→`agent.jump_header`),
and the card's **top edge and left edge are invariant across submenu descent** —
descending into or escaping out of a submenu never moves the card's top or left
anchor (only its height/width may change, growing down/right).

**Applies to.** `render_menu_overlay` (accent bar element `menu-accent-bar`; the
fixed `top`/centered wrapper).

**Why.** The left bar is the app's "focused thing wears a left bar" identity and
glances the active scope before a word is read; the stable top edge makes the
static-render descent read as the card breathing rather than teleporting.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::menu_panel_top_stable_across_descent` — opens
the `.` workspace menu, probes the card top, descends into a submenu, and asserts the
top + left edges are unchanged. The accent-bar *color* is the documented pixel gap.

### UXI-Menu-5 — the card is an elevated (lighter) surface

**Statement.** The card background is an **elevated surface**: derived from the live
`editor_bg` (what tiles + the workspace paint) and lifted in lightness at the same
hue + saturation, so it stands out from the tiles/workspace. It lifts on both dark
and light themes.

> **Amended by `UXI-JumpPanel-11`.** The original statement also required the card to
> diverge from the *recessed* jump bar (`jump_panel_bg`, which shifted the opposite
> way). That clause is **retired**: the jump panel now paints on this very surface
> (`jump_panel_surface == menu_panel_bg`) on purpose — sidebar, command card and
> palette are one material. The elevation-above-the-editor half still holds.

**Applies to.** `menu_panel_bg` (pure fn, `main.rs`); `render_menu_overlay`'s
`menu_bg` (was `overlay.bg`, now `menu_panel_bg(self.editor_bg())`);
`jump_panel_surface` (the jump panel, which now shares it).

**Why.** With the flat `overlay.bg` the card didn't read as distinct from the
workspace on some themes. Tying it to the live editor bg guarantees the separation on
every theme, and elevating (vs. the jump bar recessing) gives a coherent depth model:
jump bar sunk, tiles level, command card raised.

**Status.** `implemented`

**Enforcement.** `verify_harness.rs::menu_panel_bg_is_elevated_above_the_editor`
— the pure fn lifts lightness above `editor` on dark + light themes and preserves
hue/sat/alpha. Negative control: return `editor` unchanged ⇒ RED. The shared-surface
half is pinned by `jump_panel_surface_matches_the_command_menu`. The exact resulting
color per theme is the documented pixel gap.

## Deviations from the design brief (Fable, "The Sigil Card")

What shipped vs. what was designed in step 4, and why:

- **No click-away backdrop / hover pills.** Fable proposed mouse parity with
  `render_project_menu` (hover pill, click-away). Dropped: the leader menu is
  *keyboard-driven* (the `MenuView` capture handler owns dispatch + Esc; the project
  menu is mouse-driven because a click opens it). A hover that implies clickability
  without a click handler is worse than none, and adding real click dispatch would
  pull in the "resolve interactive state at event time" caching rule for no user-asked
  benefit. Kept the render pure (`_cx` unused, no notify path).
- **No inter-column hairlines.** Fable suggested 1px vertical rules between columns;
  shipped whitespace-only column separation (restraint in a small card). Section
  grouping is a mono uppercase caption + an 8px gap, replacing the old `border_b`
  divider rule.
- **Trail rendering.** Crumbs are past-key chips joined tightly, then a `›`, then the
  current level name in the **scope hue** (not a separate accent token) — one hue per
  scope, reinforcing the accent bar. Scope display name maps `.`→"WORKSPACE",
  `?`→"GLOBAL"; the stored `header` string is unchanged (other code/tests read it).
- **Placement refined post-review:** Fable briefed a top-*center* float; shipped
  **left-anchored** just past the jump panel + a 16px gutter (`MENU_PANEL_LEFT_PAD`),
  about where the first workspace tile renders — reads more intentional than centered.
- **Everything else as briefed:** float right of the jump panel, fixed 48px
  top, `[340, 720]px` width band, 2px scope-hued left accent bar, sigil vocabulary
  (`✦ ▣ ▤ ◈ ⌘ ⊞ ◉`), key chips (mono, right-aligned in a 34px gutter, `overlay.key`
  tint), retuned column thresholds (`≤10→1, ≤20→2, else 3`), esc chip replacing the
  footer, no scrim.
