# Worklog: folio-bone-nightfox-steel

**Date:** 2026-08-20
**Branches touched:** folio-bone-nightfox-steel (382b5b2) — merged to main via no-ff

## Cog execution evidence

- Graph id: `db7`

### Initial render

```text
graph folio-bone-nightfox-steel (frontiers)
frontier 0: folio-bone-surfaces [open], nightfox-steel [open]
frontier 1: verify-tests [open]
frontier 2: omega [open] (omega)
```

### Node execution

- `a37l` folio-bone-surfaces: claimed → closed; output: Folio editor_bg=Soft White #f5f5f0 (desktop margin), new `Theme.tile_bg=Some(Bone #f0ede4)` (tiles), editor_fg #262a2f; OverlayTheme::folio bg=#fcfbfa pinned card, border #dcd8ce, label #8a929b, key steel #40607a, accent teal #406764 kept, selected_bg #dbe3ec, modified #a24638, input #40607a; chrome.rs uses `theme.tile_bg` else derived tint; other 7 themes `tile_bg: None`.
- `b1i4` nightfox-steel: claimed → closed; output: OverlayTheme::nightfox replaced in place — key + input steel-blue #7aa7d6 (was purple #9d79d6 / yellow #dbc074), label #73859c fg #cdd2d8 cooled, cyan accent + yellow status kept.
- `s94j` verify-tests: claimed → closed; output: extracted `chrome::resolve_tile_bg`; guard test `folio_bone_surfaces_and_nightfox_steel_accent` drives it; negative control observed RED then restored green; 675 bin + 173 lib tests pass.

### Notes

- `node` `s94j`: negative control — set Folio `tile_bg=None`, test went RED (resolver fell back to derived tint h0.5 l0.97 vs Bone h0.125 l0.918); restored → green.
- `graph` `db7`: pin-vs-derive for the loved menu/jump card — set `overlay.bg=#fcfbfa` literally for the `nc(ov.bg)` menus; the `menu_panel_bg(editor_bg)` surfaces (space menu, jump panel) derive ~#fcfcf8 from Soft White, sub-perceptually identical. Accepted rather than decoupling `menu_panel_bg` from `editor_bg` (would touch dark-theme elevation logic).

### Final status

- Status: `complete`

```text
graph folio-bone-nightfox-steel (frontiers)
frontier 0: folio-bone-surfaces [done], nightfox-steel [done]
frontier 1: verify-tests [done]
frontier 2: omega [done] (omega)
```

## Built (with status)
- New `Theme.tile_bg: Option<Color>` splits the tile surface from the desktop margin. Folio: Soft White desktop, Bone tiles (a step darker), cool ink/label, pinned near-white cards, steel key + teal accent. Nightfox: Steel overlay (steel key/caret, no purple). Merged to main; `cargo build --bin yalda-gpui` clean; 675 bin + 173 lib tests pass.
- Design artifacts committed under `docs/design/` (palette-explorations, folio-paper, folio-final).

## Open / unresolved
- Runtime look is unverified by a human eye — exact theme colors are harness gap #1 (bounds, not bitmap). Needs a `./dev-gui.sh` restart to eyeball Folio + Nightfox.
- Folio `cursor_line` (#edebe6 linen) and `top_bar` left as-is; may want cooling in a follow-up for full consistency.
- Nightfox markdown syntax (h2 magenta, etc.) untouched — Steel scope was chrome only.

## Decisions
- No ADR. Palette values chosen interactively with the user via the `docs/design/` mockups; the tile-vs-desktop split is the one structural change (theme-owned `tile_bg`).

## Verification status
- Headless: `folio_bone_surfaces_and_nightfox_steel_accent` drives the real `resolve_tile_bg` chrome resolver + overlay palette; negative control observed RED. Full suite green.
- NEEDS-RUNTIME (harness gap #1, pixels/colors): human confirmation of the actual rendered Folio/Nightfox surfaces.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-20-folio-bone-nightfox-steel.md` passes.

## Next
- User restarts the GUI, eyeballs Folio Bone + Nightfox Steel; tune `cursor_line`/`top_bar` if the desktop margin reads off.
