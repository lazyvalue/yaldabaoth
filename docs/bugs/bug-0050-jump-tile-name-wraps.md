# bug-0050: jump-tile-name-wraps

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Jump Panel

## Symptom

Very long tile names wrap across multiple lines in the jump panel, making some
rows much taller than neighboring navigation rows.

## Context / root cause

The shared navigation row gives its label flexible width but does not set
single-line whitespace, overflow clipping, or text ellipsis. GPUI therefore
wraps ordinary multi-word labels when the fixed-width panel is exhausted.

## Planned solution

Add a reusable yux single-line ellipsis primitive and use it for primary tile
and workspace labels. Preserve summaries as separate prose; only identity names
are constrained.

## Approaches already tried (do NOT repeat)

- None.

---

## Log

### 2026-08-19 — Reproduced and specified

`jump_panel_long_tile_names_stay_single_line` paints an intentionally long
detached Linear title through the production panel and fails because its row is
taller than the standard jump navigation row.

### 2026-08-19 — Fixed with reusable single-line identity labels

Yux now owns `single_line_ellipsis`, used by jump navigation and picker option
labels. The production paint guard proves the long label itself remains present
while its row stays within half a pixel of short and standard rows. Removing the
nowrap/ellipsis pair restores the 92.5px wrapping failure.
