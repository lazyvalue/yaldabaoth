# bug-0019: project-context-menu-items-do-nothing

**Status:** FIXED
**First seen:** 2026-07-24
**Component:** `docs/components/jump-panel` (UXI-JumpPanel-8)

## Symptom

Clicking a project name in the jump panel opens the project context menu (New
workspace / New agent session / Delete project), but clicking any of the items
does nothing — the menu vanishes and no action runs. Keyboard accelerators
(`w`/`a`/`d`) do work, so the actions themselves are fine; the mouse path is
dead.

## Context / root cause

`render_project_menu` (`main.rs`) paints two siblings: a full-window transparent
click-away **backdrop** with `on_mouse_down` → `clear_overlay()`, and the
positioned **popup** whose items carry `on_click`. The code comment asserts "a
click on the popup hits the popup, not the backdrop" — that assumption is wrong
in GPUI 0.2.2:

- `Frame::hit_test` (`window.rs:775`) collects **every** hitbox containing the
  point, front to back; it only stops at a hitbox with
  `HitboxBehavior::BlockMouse` (`InteractiveElement::occlude`). The popup does
  not occlude, so the backdrop's hitbox is *also* hovered under the popup.
- `Interactivity::on_mouse_down` fires on any hovered hitbox, so pressing on a
  menu item ALSO fires the backdrop handler → `clear_overlay()` + `notify` on
  mouse **down**.
- `on_click` (`div.rs:2122+`) is mouse-down-then-mouse-**up** on the same
  element. The overlay is gone by the time the button is released, so the item's
  mouse-up listener isn't registered in the new frame and the click listener
  never fires.

The existing guard `project_menu_opens_on_name_click_and_actions_dispatch` calls
`project_menu_action` directly — a hand-built proxy for the click (anti-circling
rule 1), which is why it stayed green while the real mouse path was broken.

## Planned solution

Make the popup **occlude** (`.occlude()`), so its hitbox blocks the backdrop's
hitbox behind it. A press on the popup no longer reaches the backdrop; the
overlay survives until mouse-up and `on_click` fires. Clicks anywhere else still
land on the backdrop and dismiss.

Guard on the REAL path: probe the painted bounds of the "New workspace" item and
`vcx.simulate_click()` at its centre through the actual window mouse dispatch,
asserting a workspace was created. Negative control: drop `.occlude()` → the
click is swallowed and the assert fails.

## Approaches already tried (do NOT repeat)

- <none yet>

---

## Log

### 2026-07-24 — occlude the popup so the click-away backdrop can't steal the press

**Changed**

- `main.rs` `render_project_menu`: the popup now `.occlude()`s (with a comment
  stating why), so GPUI's front-to-back hit test stops at the popup and the
  full-window backdrop below is no longer hovered under it.
- `main.rs` `render_project_menu`: each item is wrapped in `probe_bounds_dyn(id)`
  so the harness can click the item's REAL painted rect (no-op in production).
- `verify_harness.rs::project_menu_item_click_runs_the_action`: new guard on the
  REAL mouse path — open the menu, probe `proj-menu-new-ws`'s painted bounds,
  `vcx.simulate_mouse_move` + `vcx.simulate_click` at its centre (through the
  window's actual mouse dispatch, not a hand-called handler), assert a workspace
  was created in that project and the menu dismissed; then re-open and click far
  outside to assert click-away still works.
- `docs/components/jump-panel.md` `UXI-JumpPanel-8`: records the occlude
  requirement and moves the mouse gesture out of the "harness gap" column.

**Verified**

- `cargo test --bin yalda-gpui project_menu` → 2 passed. Full suite:
  `cargo test --bin yalda-gpui` → 467 passed, 0 failed; whole workspace
  `cargo test` all green.
- **Negative control (observed RED twice, for the right reason):** the guard was
  written and run BEFORE the fix and failed with
  `clicking 'New workspace' did NOTHING — the menu item's on_click never fired
  (bug-0019) left: 0 right: 1`; after the fix landed, commenting `.occlude()` out
  reproduced the same failure, and restoring it went green. The probe assert
  (`item painted with area > 4x4`) keeps the test non-vacuous — a menu that never
  paints fails loudly rather than passing by accident.
- Not runtime-checked by a human yet (the fix is behavioral, not pixel): the
  headless click drives the same `Frame::hit_test` + `on_click` machinery the real
  mouse does.

**Why the old guard missed it.** `project_menu_opens_on_name_click_and_actions_dispatch`
called `project_menu_action` directly — the hand-built proxy anti-circling rule 1
warns about. It could never observe the backdrop/popup hit-test interaction.
