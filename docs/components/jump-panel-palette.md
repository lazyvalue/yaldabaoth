# Jump panel — color scheme

The full palette of the jump-panel sidebar (`jump_panel_view.rs`,
`render_jump_panel`). Almost every color is **derived from the active theme**
(`AgentTheme`) so the panel re-tints per theme; only two are fixed constants.
Concrete hexes below are for the default **Dracula** theme (`AgentTheme::dracula`).

The design intent: the panel reads as **one cool-blue family** (cyan active
marks + electric-blue headers), with warm notes reserved for the status dots'
traffic-light semantics.

## Fixed constants (theme-independent)

| Token        | Value      | Role |
|--------------|------------|------|
| `st.err`     | `0xff6b6b` | Top-level section headers (PINNED / WORKSPACES / AGENT SESSIONS) — red. |
| `electric`   | `0x3b9eff` | Per-cwd subheaders — electric blue, real path casing. Vivid theme-neutral blue. |
| `working_orange` | `0xff9e64` | The "working" status dot (reply in flight). Warm orange, distinct from `warm_accent` gold. |

## Theme-derived colors

| Token          | Source (`theme.agent.*`) | Dracula hex | Role |
|----------------|--------------------------|-------------|------|
| `st.fg`        | `editor_fg()`            | —           | Default row label text. |
| `panel_bg`     | `editor_bg()`            | —           | Panel background. |
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
