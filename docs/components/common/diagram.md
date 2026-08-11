# Component: Diagram (common)

**Status:** implemented
**Component token:** `Diagram` (⇒ `UXI-Diagram-N`)

## Description

A fenced code block whose info-string first token is `mermaid` (case-insensitive)
renders **inline as its diagram** — a rendered image — instead of as raw source.
The behavior is shared by the two markdown reading surfaces, because both paint
through the same `block_inner` dispatch: the **agent transcript** (`TranscriptView`)
and the **buffer `Viewing`** surface (`YaldaView`). The buffer `Editing` (raw
source) surface is unchanged — you edit the mermaid text there as ordinary code.

The image is produced **off the paint thread** by shelling out to `mmdc`
(`@mermaid-js/mermaid-cli`), which emits a PNG; the PNG is painted via the gpui
`img()` element (the proven in-tree bitmap path — `system_console.rs` already
paints via `gpui::Image::from_bytes` + `img()`). PNG, not gpui `svg()`, because
`svg()` is an icon/mask primitive that paints a single color and cannot render a
multi-color diagram. The result is cached by `hash(source + theme + width)`.

While the image is not yet ready the block paints its **raw highlighted source**
as the placeholder. If `mmdc` is missing or rendering errors, the block keeps
painting the raw highlighted source **plus one subtle note line** (e.g.
`mermaid: <error / mmdc not found>`) — it is never blank. The mermaid theme
follows the app theme (dark vs default) and is part of the cache key, so a theme
switch re-renders the diagram.

Structurally this is a distinct block type (`RenderedBlock::Diagram`), not a
`CodeBlock`, so a renderer can paint an image and opt the bitmap OUT of the
per-line code hit-testing (there is no copy-on-select over the image). The raw
source stays selectable in `Editing`.

## References

- `docs/components/agent-tile/transcript.md` — the transcript facet that renders
  `RenderedBlock::Diagram` inline (consumes `UXI-Diagram-1`).
- `docs/components/buffer.md` — the `Viewing` surface that renders
  `RenderedBlock::Diagram` inline (consumes `UXI-Diagram-1`).
- `docs/components/common/selection.md` — the code-block hit-testing the rendered
  image opts OUT of (`UXI-Selection-3`).
- `src/blocks.rs` — `RenderedBlock::Diagram { source, lines }`.
- `src/bin/yalda-gpui/render_blocks.rs` — `block_inner`, the shared paint dispatch.
- `src/bin/yalda-gpui/system_console.rs` — the in-tree `gpui::Image::from_bytes` +
  `img()` bitmap path this reuses.

## UX invariants

### UXI-Diagram-1 — A `mermaid` fenced block renders inline as its diagram, with a safe fallback

**Statement.** A fenced code block whose info-string first token is `mermaid`
(case-insensitive) classifies to `RenderedBlock::Diagram`, not `CodeBlock`, and
renders **inline as a rendered image** on BOTH markdown reading surfaces — the
agent transcript (`TranscriptView`) and the buffer `Viewing` surface (`YaldaView`)
— because both paint through the shared `block_inner` dispatch. Concretely:

- **Mechanism (locked, "A").** The image is produced by an **async shell-out** to
  `mmdc` (`@mermaid-js/mermaid-cli`) that emits a **PNG**, painted via the gpui
  `img()` element (`gpui::Image::from_bytes`). Rendering runs **off the paint
  thread**; the result is cached by `hash(source + theme + width)`. PNG — not gpui
  `svg()` — because `svg()` is a single-color icon/mask primitive and cannot paint
  a multi-color diagram.
- **Placeholder.** Until the PNG is ready the block paints its **raw highlighted
  source** (the `lines` field).
- **Fallback (never blank).** If `mmdc` is absent or rendering errors, the block
  keeps painting the raw highlighted source **plus one subtle note line** (e.g.
  `mermaid: <error / mmdc not found>`). No regression when `mmdc` is unavailable.
- **Theme.** The mermaid theme follows the app theme (dark vs default) and is part
  of the cache key, so a theme switch re-renders the diagram.
- **Editing is unchanged.** The buffer `Editing` (raw source) surface renders the
  mermaid text as ordinary code — you edit the source there.
- **v1 scope.** No click-to-enlarge. The diagram does NOT couple to `Cmd` text-zoom
  (`TextZoom`) — it fits to the container width. The rendered image opts OUT of the
  code-block per-line hit-testing (`UXI-Selection-3`): no copy-on-select over the
  bitmap. The raw source remains selectable in `Editing`.

**Applies to.** `src/blocks.rs`: `RenderedBlock::Diagram { source, lines }` (the
distinct block type). `src/render.rs`: the classifier that maps a `mermaid`
info-string fence to `Diagram` instead of `CodeBlock`. `render_blocks.rs`:
`block_inner`'s `Diagram` arm (image / placeholder / fallback paint, opting the
bitmap out of the code `block_hits` sink). The async renderer + `hash(source +
theme + width)` cache (off-thread `mmdc` shell-out → PNG bytes), painted via
`gpui::Image::from_bytes` + `img()` as in `system_console.rs`.

**Why.** Agent replies and markdown documents frequently contain mermaid
diagrams; showing the raw source loses the diagram's value — the reader wants the
picture, not the DSL. Rendering inline on both surfaces gives it on the transcript
(where the agent draws) and in `Viewing` (where docs hold it). The raw-source
placeholder + fallback guarantee no regression when `mmdc` is absent: the block is
never blank and always shows at least the source.

**Status.** `implemented` (Cog graph `sve`; branch `mermaid-diagrams`). Classifier
in `src/render.rs` (` ```mermaid ` → `RenderedBlock::Diagram`, `src/blocks.rs`);
async render + cache in `src/bin/yalda-gpui/diagram.rs` (`DiagramCache`,
`request_diagram` off-thread `mmdc`, test seam `set_test_renderer`); paint arm +
`diagram_fallback_column` in `render_blocks.rs`; per-frame `reconcile_diagrams`
(root `render()`) + `RenderCtx.diagrams` handle wired at the doc-view (`screens.rs`)
and transcript (`transcript_view.rs`) sites.

**Enforcement.** `verify_harness.rs` (headless), three guards, each observed RED
with its own fix reverted:

- **(a) Classification** — `diagram_001_mermaid_fence_classifies_to_diagram`: a
  ` ```mermaid ` fence classifies to `RenderedBlock::Diagram` (a `rust` fence stays
  `CodeBlock`) via the real `render::render`. NC: classifier forced to `CodeBlock`.
- **(b) Fallback** — `diagram_002_render_failure_falls_back_to_source`: real doc
  open → per-frame reconcile → off-thread stub error → cache `Failed` (the
  fallback trigger); a layout probe asserts the block still paints (non-blank). NC:
  drop the `reconcile_diagrams` call ⇒ no request, stays uncached.
- **(c) Image swap** — `diagram_003_successful_render_reaches_ready`: stub returns a
  valid PNG → `request_diagram` resolves the cache to `Ready` (the decoded image the
  paint arm swaps to) after `run_until_parked()`. NC: skip the `Ready` store ⇒ stays
  `Pending`.

Genuine runtime gaps — the only `NEEDS-RUNTIME`: **gap 1** (the actual PNG pixels
beyond bounds geometry — a human eye) and **gap 2** (the live `mmdc` subprocess
against the real binary; needs `mmdc` on `PATH`).
