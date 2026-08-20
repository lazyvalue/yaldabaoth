# bug-0052: jump-workspace-cards-collapse-to-bands

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Jump Panel

## Symptom

In a project with several workspaces and enough jump-panel content to exceed
the viewport, every workspace card collapses into a thin empty blue band. The
workspace name, count, and attached tile rows are clipped away. The cards
should retain their intrinsic content height and the panel should scroll.

## Context / root cause

`compact_bounded_group` is a direct child of the jump panel's vertical flex
column. The column is scrollable, but the new group primitive did not opt out of
flex shrink. GPUI therefore satisfies the constrained viewport by shrinking the
cards toward their border-only minimum; `overflow_hidden` then correctly clips
the header and body outside that collapsed box. The original headless guard
used only one workspace, so it never put the scroll container under pressure.
This violates UXI-JumpPanel-27's bounded, readable workspace ownership contract.

## Planned solution

Make `compact_bounded_group` a non-shrinking flex item at the reusable yux
boundary. Add a crowded production-paint guard with many expanded workspaces
that requires each group to retain enough height for its header and tile row,
forcing overflow to be handled by scrolling rather than flex compression.

## Approaches already tried (do NOT repeat)

- The original single-workspace geometry guard was insufficient: it proved
  containment only when all content fit in the viewport and could not exercise
  parent flex shrink.

---

## Log

### 2026-08-19 — Screenshot localized to flex shrink

The reported screenshot shows one border-height blue band per workspace while
later Detached content remains legible. The shared workspace card is the only
new shrinkable, clipped child in that path. A crowded real-paint guard was added
before the fix to reproduce the constrained scroll-column geometry. At a
900×360 viewport the guard measured a 4px group around a 29px header and 29px
tile row, exactly localizing the empty-band symptom to parent flex shrink.

### 2026-08-19 — Fixed at the reusable group boundary

`compact_bounded_group` now applies `flex_none`, so the scrolling column cannot
compress the card below its intrinsic header/body height. The crowded guard now
mixes sixteen expanded and collapsed workspaces in the same 900×360 viewport;
every expanded card retains its header plus tile row and every collapsed card
retains its header. Removing `flex_none` restores the exact 4px-band RED. All 19
adjacent jump-panel tests pass.
