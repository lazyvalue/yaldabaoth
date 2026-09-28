# Worklog: jump-panel-ui-font

**Date:** 2026-09-27 · **Graph:** `4vi` · **Branch:** `jump-panel-ui-font` → `main` (`bfd3e1e`)

## Shipped
- Jump panel renders in the UI font (`jump_panel_style`: both font slots = `body_font`). Root cause: d9c4c6d made `code_font` resolve to JetBrains Mono on Linux; before, "Menlo" fell back to the proportional Ubuntu face, which the panel had always actually shown. Guard `jump_panel_rows_use_the_ui_font_not_the_code_font` (RED with code_font); `yalda-gpui` 890 passed on `main`. UXI-JumpPanel-24 amended.

## Caveats
- NEEDS-RUNTIME (gap 1, glyph appearance): release GUI built 20:32, not restarted.
- Other chrome that reads `code_font` (e.g. Cmd-P palette, menus) likely turned monospace too; not changed here.

## Decisions
- Navigation chrome uses the UI font; the code font is for code.

## Cog
- Status: `complete`

```text
graph jump-panel-ui-font (frontiers)
frontier 0: jump-panel-ui-font [done]
frontier 1: omega [done] (omega)
```
