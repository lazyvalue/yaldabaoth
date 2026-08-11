# Common components

Shared elements and bodies of behavior that **more than one** component depends on.
A component spec `References` these instead of duplicating their invariants.

A "common" entry is anything cross-cutting: a reusable **behavior** (text editing,
caret containment, copy-on-select), a reusable **visual element** (a segmented
panel, a status strip), or a reusable **interaction** (leader menus). If exactly one
component uses it, it belongs in that component's spec — promote it here only on the
second consumer (reuse before abstraction).

Invariants defined here still use `UXI-<Component>-N`, where `<Component>` is the
common component's token (e.g. `UXI-TextEditing-1`). Consuming components reference
them by id.

## Index

- [text-editing.md](text-editing.md) — `TextEditing`: the shared editing model
  (cursor, motions, insert/normal, wrapping) that every editable surface obeys.
- [selection.md](selection.md) — `Selection`: X11-style copy-on-select shared by
  the doc view and the agent transcript.
- [text-zoom.md](text-zoom.md) — `TextZoom`: document text zoom across the buffer
  doc/edit views + the agent transcript.
- [blockquote.md](blockquote.md) — `Blockquote`: `>`-quoted text renders italic on
  every surface (doc view, transcript, both edit views, compose / You-block).
- [paragraph-spacing.md](paragraph-spacing.md) — `ParagraphSpacing`: extra vertical
  gap between blocks / paragraphs / list items on the reading surfaces (doc view,
  agent transcript, WP), scaled with zoom.
- [menu.md](menu.md) — `Menu`: the leader command panel (the floating "Sigil Card"),
  shared by every tile's `space` / `.` / `?` leaders. `UXI-Menu-1..4`.
- [diagram.md](diagram.md) — `Diagram`: a `mermaid` fenced block renders inline as
  its diagram image on the agent transcript + buffer `Viewing`. `UXI-Diagram-1`.
