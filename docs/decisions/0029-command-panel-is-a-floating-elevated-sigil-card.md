# ADR-0029: The command panel is a floating, elevated "Sigil Card"

**Status:** Accepted
**Date:** 2026-07-23
**Related:** `UXI-Menu-1..5`, `docs/components/common/menu.md` (the visual contract),
`docs/specs/spec-menu-scopes.md` (the unchanged behavior/state machine),
`render_menu_overlay` + `menu_trail_crumbs` + `menu_panel_bg` (`src/bin/yalda-gpui/main.rs`),
`jump_panel_bg` (the recessed counterpart). Designed autonomously via `/new-ux`
with **Fable** consulted for UX / aesthetics / architecture.

## Context

The leader menus (`space` = tile/app-local, `.` = workspace, `?` = global) opened
one overlay — `render_menu_overlay`. It painted an **opaque, full-window-width bar**
pinned `.absolute().top_0().left_0().w_full()` with a bottom border. On a wide
monitor the entries hugged the left third and the entire right half was dead space;
it read as an intrusive drop-down rather than a deliberate command surface. The
owner asked for something "more aesthetic. Possibly floating… weird on a wide
monitor for them to drop down and have so much empty space on the right," citing
which-key / Helix as inspiration but wanting something Yaldabaoth-specific.

Only the *rendering* was at issue. The behavior — leaders, descent, single-key
dispatch, breadcrumb state, Esc-pops-a-level, disabled-entry dimming, dismiss on
focus change — is owned by `spec-menu-scopes.md` and was correct; it must not change.

## Decisions

Each is a `UXI-Menu-N` in `docs/components/common/menu.md` (the new home for the
leader menu's *visual* contract, kept separate from the behavior spec).

1. **A floating, content-sized card — not a full-width bar (UXI-Menu-1).** Width
   tracks content within a `[340, 720]px` band; on any monitor wider than 720 the
   card is strictly narrower than the window, so the screen stays visible on all
   sides. This is the direct fix for the wide-monitor dead-space complaint.

2. **Left-anchored just past the jump panel, not centered (UXI-Menu-1).** The card's
   left edge sits at `JUMP_PANEL_WIDTH + MENU_PANEL_LEFT_PAD (30px)`, pinned
   `MENU_PANEL_TOP (48px)` below the top; it grows down/right from that anchor.
   (Fable briefed a top-*center* float; in review that read as floating, and exact
   center-alignment with the first tile read as accidental — a firm left anchor with
   a deliberate gutter reads as intentional. The gutter was tuned 16 → 30px so it
   doesn't line up flush with the first tile's edge.)

3. **An elevated (lighter) background (UXI-Menu-5).** `menu_panel_bg()` derives from
   the **live `editor_bg`** (what tiles + the workspace paint) and lifts lightness at
   the same hue + saturation. This makes a coherent depth model: the **jump bar is
   sunk** (`jump_panel_bg` recesses), **tiles are level**, the **command card is
   raised**. Deriving from the live bg (rather than the flat `overlay.bg`, which
   didn't read as distinct on some themes) guarantees the separation on every theme.

4. **Scope identity: a 2px left accent bar + a header sigil, three hues by leader
   (UXI-Menu-4).** `space`→`agent.frozen_bar` (cyan), `.`→`overlay.key`,
   `?`→`agent.jump_header` (warm). This reuses the app's "the focused thing wears a
   left bar" language (the doc-view cursor, the agent frozen bar) — the command panel
   *is* the focused thing while open — and lets the active scope be glanced before a
   word is read. The sigil vocabulary (`✦ ▣ ▤ ◈ ⌘ ⊞ ◉`) extends the glyphs
   `render_project_menu` already established.

5. **The breadcrumb is the literal keystroke trail (UXI-Menu-3).** The header renders
   the exact chord taken to reach the current level as mono key chips
   (`␣ › AGENT`, `␣ n › NAVIGATE`), derived from `MenuState::path` (no new state).
   This is the signature detail: a leader-key UI's whole skill ceiling is learning
   the chords, so showing the sequence back each visit trains the user to stop
   needing the menu — a surface whose goal is its own obsolescence. Functional, not
   decoration; it replaces a bare "Commands" label.

6. **Rows are key-chip + label on a fixed 26px grid (UXI-Menu-2).** Mono key chips,
   right-aligned in a shared 34px gutter (so labels share a left edge and multi-char
   keys grow leftward), tinted with `overlay.key`; submenu rows get an
   `overlay.accent` label + a right-edge `▸`. Sections group by a mono uppercase
   caption + whitespace (replacing the old divider rule) and never split across
   columns; column count retuned to `≤10→1, ≤20→2, else 3`. The footer collapsed to
   a single `esc` chip in the header — the old "press a key · Esc back / close" line
   repeated what every user knows by day two.

7. **Keyboard-only: no scrim, no click-away, no hover pills.** The `MenuView`
   capture handler owns dispatch + Esc; these menus live for ~800ms of muscle
   memory, so a per-chord scrim would flash the screen dark and back, and a hover
   pill implies a clickability the render doesn't wire. (Fable proposed mouse parity
   with the click-opened `render_project_menu`; declined — the leader menu is a
   keyboard surface, and adding real click dispatch would pull in the "resolve
   interactive state at event time" caching rule for no asked-for benefit.)

## Why this respects the depth/identity system

The two derived-from-`editor_bg` surfaces move in *opposite* directions — the jump
bar down, the card up — so they never converge and the app reads as three layers.
The left accent bar is the same idiom used elsewhere for "what's focused," so the
panel doesn't introduce a new visual language; it speaks the existing one at a
higher elevation.

## Alternatives rejected

- **Keep the full-width bar, just restyle it.** Doesn't touch the actual complaint
  (wide-monitor dead space); the bar shape is the problem.
- **Top-center float (Fable's brief).** Read as unmoored; a firm left anchor near
  the first-tile position reads as belonging to the workspace.
- **Cursor/tile-anchored or bottom command-bar.** Bottom occludes the freshest
  content in Yaldabaoth (transcript tail, compose box); cursor-anchored jumps around
  and fights the fixed muscle-memory location. Top-left, fixed, won.
- **Flat `overlay.bg` for the card.** Didn't separate from the workspace on some
  themes; a live-derived elevated shade guarantees the delta.
- **A scrim / click-away / hover pills.** Wrong for a sub-second keyboard chord
  surface (see decision 7).
- **Put the visual rules in `spec-menu-scopes.md`.** That spec owns the *behavior*;
  mixing the render contract in would blur the two. The visual contract lives in the
  `Menu` component spec and references the behavior spec.

## Consequences

- `render_menu_overlay` is a floating card; `MenuOverlay` carries a `leader: char`
  (drives hue/sigil/trail); `menu_trail_crumbs` and `menu_panel_bg` are pure,
  unit-tested helpers; constants `MENU_PANEL_{MIN_W,MAX_W,TOP,LEFT_PAD}` are the
  layout knobs.
- Guards (all observed RED under reverted negative controls):
  `verify_harness::menu_panel_floats_in_content_region` (left-anchored, ≤720<window),
  `…menu_panel_top_stable_across_descent` (top + left fixed on descent),
  `…menu_panel_rows_and_sections_paint`, `…menu_panel_bg_is_elevated_and_distinct_from_jump_bar`,
  and `tests::menu_trail_crumbs_tracks_descent`.
- The remaining human-eye checks are the documented paint gap: exact chip colors,
  the accent-bar hue, the elevated shade, and glyph legibility on folio + nightfox.
- Behavior is unchanged: key dispatch, breadcrumb state machine, Esc, disabled
  dimming, focus-change dismissal, single mutually-exclusive overlay — all per
  `spec-menu-scopes.md`.
