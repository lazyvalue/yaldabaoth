# Component: Buffer

**Status:** living
**Component token:** `Buffer` (⇒ `UXI-Buffer-N`)

## Description

`App::Buffer(BufferApp)` — a tile that is a view onto the shared file-buffer pool
(ADR-0007), always in exactly one `BufferMode`. `Viewing ⇄ Editing` toggle over the
same pooled `SharedCore`; `Picking` is reached via `Cmd+O` (Buffer-scoped). The
three modes:

- **`Picking` (Browser view, `BrowserView`)** — file/buffer browser: directory
  navigation, parent, hidden-file toggle, sort cycle, worktree mode, filter input.
- **`Viewing` (Doc view, `YaldaView`)** — rendered markdown, block-by-block: a left
  orange cursor-bar on the focused block; `j/k`/arrows move block focus; `g`/`G`
  top/bottom; page scroll; wiki-links; marks. Built from `RenderedBlock`s.
- **`Editing` (Edit view, `EditView`)** — raw markdown source in two submodes toggled
  with `Ctrl-W`: **Code (RAW)** — monospace, line-number gutter, `md_highlight`
  source colors; **WordProcessor (WP)** — proportional font with per-line
  typographic classification (`classify_wp_line`). Vim-style Normal/Insert submodes
  (`AppMode`).

Buffer and Agent are orthogonal — a Buffer tile never nests an agent, and vice
versa. Primary code home: `screens.rs::render_doc` / `render_edit` /
`render_browser`, `edit_ui.rs`, `browser_ui.rs`, `render_blocks.rs`.

## References

- `docs/specs/spec-tiles-and-apps.md` — `App::Buffer` and the tile/app model (ADR-0019).
- `docs/components/common/text-editing.md` — the Edit view obeys `TextEditing`.
- `docs/components/common/text-zoom.md` — the Doc/Edit views obey `TextZoom`.

## UX invariants

_(none migrated yet — add via /new-ux as behavior is specified.)_
