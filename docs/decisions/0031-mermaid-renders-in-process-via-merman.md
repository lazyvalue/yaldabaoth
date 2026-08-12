# ADR-0031: Mermaid diagrams render in-process via merman (not an mmdc/Chromium shell-out)

**Status:** Accepted
**Date:** 2026-08-12
**Related:** `UXI-Diagram-1`, `docs/components/common/diagram.md`, `src/bin/yalda-gpui/diagram.rs`

## Context

`UXI-Diagram-1` renders a ` ```mermaid ` fence inline as its diagram on the agent
transcript and the buffer `Viewing` surface. Mermaid is a browser JavaScript
library, so the first implementation ("mechanism A") shelled out to `mmdc`
(`@mermaid-js/mermaid-cli`) → PNG, painted via `img()`.

That shipped and worked, but the shell-out carried real costs:

- **A Chromium runtime.** `mmdc` drives Puppeteer + a headless Chromium
  (hundreds of MB), installed out-of-band via `npm i -g`.
- **`PATH` fragility.** A GUI process launched from Finder/Dock inherits a minimal
  `PATH` that excludes the nvm/Homebrew node bins, so bare `Command::new("mmdc")`
  silently fell back to raw source even when `mmdc` worked in a terminal. We added
  a `resolve_mmdc()` probe (nvm/Homebrew/volta/…) to paper over this.
- **Per-render subprocess latency** and temp-file staging.
- **A genuine runtime-only test gap** (the live subprocess couldn't be exercised
  headlessly).

As of 2026, native-Rust mermaid engines exist. `merman` (Latias94/merman) parses,
lays out, and rasterizes mermaid entirely in Rust (SVG via `resvg`), needs no JS
runtime, and is used by Zed — also a GPUI app.

## Decision

Render mermaid **in-process** via `merman` (`0.6.2`, `raster` feature). The seam
is unchanged: `render_mermaid_png` still returns `Result<Vec<u8>, String>` PNG
bytes on the background executor, feeding the same `DiagramCache` → `img()` paint →
raw-source fallback. Only the body changed —
`merman::render::HeadlessRenderer::new().with_site_config(theme).render_png_sync(src, &RasterOptions)`
replaces the `mmdc` subprocess. All `mmdc`/`PATH`-resolution code was deleted.

## Consequences

- **No external dependency.** No Node, no Chromium, no `PATH` resolution, no global
  install. The app renders diagrams out of the box.
- **The live-subprocess runtime gap is gone.** Because merman is in-process, a plain
  unit test renders a real flowchart to a valid PNG headlessly
  (`diagram::merman_tests::real_merman_renders_flowchart_to_png`). Only the pixel/
  theme-color gap (a human eye) remains `NEEDS-RUNTIME`.
- **Diagram-type coverage is a subset.** merman covers flowchart, sequence, class,
  ER, and XY charts — not all of mermaid. Unsupported types return `Ok(None)` →
  `Err` → the existing raw-source fallback (never blank). Full mermaid syntax is the
  price of dropping the browser; acceptable for agent/doc diagrams.
- **A transitive pin.** merman `0.7` needs rustc ≥ 1.95 (we are on 1.94), so we pin
  `0.6.2`; its `merman-render 0.6.2` needs `roughr-merman` ≤ `0.12.1` (0.12.2 dropped
  `OptionsBuilder::seed`), pinned in `Cargo.lock` via
  `cargo update -p roughr-merman --precise 0.12.0`. Re-apply on lockfile regen, or
  bump to merman `0.7` once the toolchain moves to 1.95.

## Alternatives rejected

- **Keep the `mmdc` shell-out.** Rejected: the Chromium dependency and `PATH`
  fragility are exactly what motivated the change.
- **Kroki / mermaid.ink service.** Removes local Chromium but adds a network
  dependency (and sends diagram text off-machine for the hosted one). Rejected for a
  local editor.
- **Hand-rolled native renderer (original "mechanism C").** Only ever covers a
  hand-built subset and diverges from real mermaid. merman gives a maintained native
  engine instead.
