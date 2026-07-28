# Jump panel — color scheme

The full palette of the jump-panel sidebar (`jump_panel_view.rs`,
`render_jump_panel`). Colors derive from the active `AgentTheme` and
`OverlayTheme`, so the panel re-tints per theme.

The operational language is literal: **orange = working**, **green = ready for
input**, and **neutral gray = selected**. Cool blues remain supporting prose and
subheader colors, never status or selection.

## Jump-panel colors are now theme-owned (`UXI-JumpPanel-7`)

The three formerly-fixed accents were lifted into `AgentTheme` so each theme owns
them (the AgentTheme "every hardcoded hex lifted here" pattern). The **default**
values below preserve the old theme-neutral look for every theme; **Nightfox**
art-directs its own palette-native set (see its `AgentTheme::nightfox`).

| Token        | Source (`theme.agent.*`) | Default (most themes) | Nightfox | Role |
|--------------|--------------------------|-----------------------|----------|------|
| `st.err`     | `jump_header`            | `0xff6b6b`            | `0xc94f6d` | Top-level section headers (PINNED / UNFILED) + project names — red. |
| `electric`   | `jump_subheader`         | `0x3b9eff`            | `0x719cd6` | Per-cwd "Unfiled" subheaders — real path casing. |
| `working_orange` | `jump_working`       | `0xff9e64`            | `0xf4a261` | Working glyph, word, and row wash. |

## Theme-derived colors

| Token | Source | Role |
|-------|--------|------|
| `st.fg` | `editor_fg()` | Default row and tab label text. |
| `panel_bg` | menu/overlay surface | Same elevated material as command menus. |
| `border` / `selection_mark` | `overlay.border` | Panel/control boundaries and the selected-row left mark. |
| `sel_bg` | `overlay.selected_bg` | Neutral selected/hover background for rows and tabs. |
| `st.dim` | `agent.dim` | Disconnected glyph/row and muted placeholders. |
| `supporting_text` | `agent.agent_tint` | Session summaries and supporting copy. |
| `ready` | `agent.tool_completed` | Ready glyph/word and green wash. |
| `working_orange` | `agent.jump_working` | Working glyph/word and orange wash. |

## Where each color lands

### Section headers
- **Top-level** (`PINNED` / `WORKSPACES` / `AGENT SESSIONS`) — `st.err` (**red**),
  bold, uppercase, underlined (`section_heading` + `.text_color(st.err)`).
- **Per-cwd subheaders** — `electric` (**blue**), path's real casing, no underline,
  no italic. Reads as a secondary tier.

### Rows
- **Label text** — `st.fg`, overridden to `st.dim` when disconnected.
- **Ready** — green background wash at α 0.08, with no outline or italic.
- **Working** — orange background wash at α 0.07, with no outline.
- **Selected (UXI-JumpPanel-5)** — `sel_bg` neutral background plus a 2px
  `overlay.border` left mark. It wins over the status wash.
- **Hover** — the same neutral `sel_bg`.

### Status dots — shape + color together = what the agent is doing (UXI-JumpPanel-1/6)
| State | Glyph | Color | Token |
|-------|-------|-------|-------|
| Working (reply in flight) | ◆ | orange | `working_orange` |
| Ready for input (every connected non-working agent) | ✦ | green | `ready` |
| Disconnected / connecting | ✦ | dim | `st.dim` |

Binding (in-use vs free) is no longer shown by the dot — the dot is purely an
activity signal.

### Placeholders
- "Nothing pinned yet." / "No sessions." — `st.dim`, mono (not italic).

## History
- Selection tint & active mark were originally built from `warm_accent`
  (muddied to brown/olive at low α) and a bright-red `0xff6b6b` bounding box.
  They moved through a cool `frozen_bar` treatment and now use neutral overlay
  gray so state colors keep one meaning.
- Headers were briefly all-electric-blue; settled on **red top-level headers +
  electric-blue cwd subheaders** for a two-tier hierarchy.
- Status was simplified from outlined chips and unread-dependent italic to quiet
  washes: orange Working, green ready-for-input, and dim unavailable. Unread is
  internal state and no longer fragments the Waiting presentation.
