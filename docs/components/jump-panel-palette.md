# Jump panel — color scheme

The full palette of the jump-panel sidebar (`jump_panel_view.rs`,
`render_jump_panel`). Almost every color is **derived from the active theme**
(`AgentTheme`) so the panel re-tints per theme; only two are fixed constants.
Concrete hexes below are for the default **Dracula** theme (`AgentTheme::dracula`).

The design intent: the panel reads as **one cool-blue family** (cyan active
marks + electric-blue headers), with warm notes reserved for the status dots'
traffic-light semantics.

## Jump-panel colors are now theme-owned (`UXI-JumpPanel-7`)

The three formerly-fixed accents were lifted into `AgentTheme` so each theme owns
them (the AgentTheme "every hardcoded hex lifted here" pattern). The **default**
values below preserve the old theme-neutral look for every theme; **Nightfox**
art-directs its own palette-native set (see its `AgentTheme::nightfox`).

| Token        | Source (`theme.agent.*`) | Default (most themes) | Nightfox | Role |
|--------------|--------------------------|-----------------------|----------|------|
| `st.err`     | `jump_header`            | `0xff6b6b`            | `0xc94f6d` | Top-level section headers (PINNED / UNFILED) + project names — red. |
| `electric`   | `jump_subheader`         | `0x3b9eff`            | `0x719cd6` | Per-cwd "Unfiled" subheaders — real path casing. |
| `working_orange` | `jump_working`       | `0xff9e64`            | `0xf4a261` | The "working" status star (reply in flight). |

## Theme-derived colors

| Token          | Source (`theme.agent.*`) | Dracula hex | Role |
|----------------|--------------------------|-------------|------|
| `st.fg`        | `editor_fg()`            | —           | Default row label text. |
| `panel_bg`     | `jump_panel_bg` (`Some`) else `jump_panel_bg(editor_bg())` | `#20222c` (derived) / Nightfox `#0d1119` (explicit) | Recessed panel background — a per-theme ΔL darken of the editor bg (near-black themes lighten instead), or an explicit override. |
| `border`       | `dim`                    | `0x6272a4`  | Panel right border. |
| `st.dim`       | `dim`                    | `0x6272a4`  | Section-header underline, badge fallback, disconnected/off dot, disconnected row label, muted placeholder text. |
| `active_accent`| `frozen_bar`             | `0x8be9fd`  | The "you are here" left accent bar, active row label, selection-tint base, "＋ New agent session" badge. |
| `sel_bg`       | `frozen_bar` @ α 0.15    | `0x8be9fd`… | Selection / hover row tint; floating drag-chip background. |
| `st.accent`    | `warm_accent`            | `0xf1fa8c`  | "Working" status dot (reply in flight). The one warm note. |
| `ready`        | `tool_completed`         | `0x50fa7b`  | "Waiting for you" status dot (turn finished, your move). |

## Where each color lands

### Section headers
- **Top-level** (`PINNED` / `WORKSPACES` / `AGENT SESSIONS`) — `st.err` (**red**),
  bold, uppercase, underlined (`section_heading` + `.text_color(st.err)`).
- **Per-cwd subheaders** — `electric` (**blue**), path's real casing, no underline,
  no italic. Reads as a secondary tier.

### Rows
- **Label text** — `active_accent` when this is the focused/active row, else
  `st.fg`; overridden to `st.dim` when the session is disconnected.
- **Italic** — carries exactly one meaning: the **"waiting on you"** session state
  (idle + unread, `dot_status == WaitingForYou`). Nothing else in the panel is
  italic.
- **Active mark (UXI-JumpPanel-5)** — 2px left border in `active_accent` +
  `sel_bg` background tint. Every row reserves a transparent 2px bar so the mark
  never shifts geometry.
- **Hover** — `sel_bg` tint.

### Status dots — shape + color together = what the agent is doing (UXI-JumpPanel-1/6)
| State | Glyph | Color | Token |
|-------|-------|-------|-------|
| Working (reply in flight)                | ● | orange | `working_orange` |
| Waiting on you (idle + unread output)    | ● | green + italic label | `ready` |
| Idle+read / disconnected / unknown phase | ○ | dim    | `st.dim` |

Binding (in-use vs free) is no longer shown by the dot — the dot is purely an
activity signal.

### Placeholders
- "Nothing pinned yet." / "No sessions." — `st.dim`, mono (not italic).

## History
- Selection tint & active mark were originally built from `warm_accent`
  (muddied to brown/olive at low α) and a bright-red `0xff6b6b` bounding box.
  Restyled to the cool `frozen_bar` accent (tint + left bar) — see
  UXI-JumpPanel-5 in `jump-panel.md`. `0xff6b6b` was then repurposed as the
  red section-header color.
- Headers were briefly all-electric-blue; settled on **red top-level headers +
  electric-blue cwd subheaders** for a two-tier hierarchy.
- The status dots were redefined around **agent activity** (not binding): orange
  working / green-italic waiting-on-you / dim read, backed by the new
  `AgentState.unread` flag (UXI-JumpPanel-6). Italic was reassigned from
  "free session" to "waiting on you".
