# bug-0043: jump-tag-items-oversized

**Status:** FIXED
**First seen:** 2026-08-19
**Component:** Jump Panel / Unbound tag folders

## Symptom

Items in the jump panel sometimes look larger than the surrounding navigation,
most visibly when an Unbound tile has a tag and therefore appears beneath a tag
folder.

## Root cause

The tile-native Unbound renderer added a production tag-folder header that sets
only its color. Unlike ordinary jump rows and the older tag-folder renderer, it
does not set the jump panel's monospace font or a fixed chrome text size. GPUI
therefore supplies its larger inherited/default typography. Tagged child rows
already route through the fixed `jump_nav_row`; the folder immediately above
them is the inconsistent element.

## Fix

The production Unbound tag-folder header now pins the jump panel's monospace
font, semibold weight, and compact fixed subheader size (`st.pt * 0.82`). Tagged
child tiles continue through the same fixed navigation-row component as loose
tiles.

## Verification

`verify_harness::jump_panel_tagged_items_keep_fixed_chrome_size` measures the
real painted tag folder, tagged tile, loose tile, and ordinary navigation row,
then repeats after 2× document zoom. Before the explicit typography was added,
the guard was observed RED at 34px for the folder versus 29px for a normal row;
it now passes and all chrome heights remain zoom-invariant.
