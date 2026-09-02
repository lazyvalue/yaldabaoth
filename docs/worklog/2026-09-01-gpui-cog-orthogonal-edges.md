# Worklog: GPUI Cog orthogonal edges

**Date:** 2026-09-01
**Branches touched:** `gpui-cog-orthogonal-edges` (pending commit)

## Cog execution evidence

- Graph id: `frh`

### Initial render

```text
graph gpui-cog-orthogonal-edges (frontiers)
frontier 0: spec-edge-routing [open]
frontier 1: guard-edge-paint [open]
frontier 2: build-edge-canvas [open]
frontier 3: verify-edge-routing [open]
frontier 4: document-edge-routing [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `4oqc` `spec-edge-routing`: claimed → closed; output: `{"result":"Specified anchored orthogonal routes, lanes, arrowheads, overflow, and fallbacks."}`
- `js4v` `guard-edge-paint`: claimed → closed; output: `{"result":"Real Overview paint guard observed RED because old edge pills had no connector canvas."}`
- `gr6w` `build-edge-canvas`: claimed → closed; output: `{"result":"Replaced pills with GPUI canvas paths behind unchanged clickable cards."}`
- `djj7` `verify-edge-routing`: claimed → closed; output: `{"result":"Fan-in, cycle, missing endpoint, wide rank, click, cache, RED control, and release gates pass."}`
- `b81y` `document-edge-routing`: claimed → closed; output: `{"result":"Reconciled UXI-Cog-18 and recorded validated execution evidence."}`
- `yr35` `omega`: claimed → closed; output: `{"result":"Integrated verified orthogonal connectors into main and rebuilt release."}`

### Notes

- `djj7`, deviation: `cargo-mutants` is not installed. Manual removal of the production connector canvas was observed RED instead.

### Final status

- Status: `complete`

```text
graph gpui-cog-orthogonal-edges (frontiers)
frontier 0: spec-edge-routing [done]
frontier 1: guard-edge-paint [done]
frontier 2: build-edge-canvas [done]
frontier 3: verify-edge-routing [done]
frontier 4: document-edge-routing [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- Cog Overview edges are GPUI-painted orthogonal card-to-card connectors with deterministic lanes and arrowheads; cards and click behavior are unchanged.
- Wide ranks use a horizontal scroll surface; missing endpoints are omitted and cyclic/back edges use outer routes.

## Open / unresolved

- Exact anti-aliasing and theme colour inspection remain runtime gap #1.
- `cargo-mutants` is unavailable; the equivalent targeted manual mutation was RED.
- No GUI or session server was restarted.

## Decisions

- No ADR required: this replaces one diagram renderer inside the existing CogView ownership boundary.

## Verification status

- Real GPUI reducer, paint, geometry, click, fallback-layout, and cached-view guards pass; optimized release build passes.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-01-gpui-cog-orthogonal-edges.md` passes.

## Next

- Restart the GUI only when the operator chooses to activate the rebuilt release binary and visually inspect exact pixels.
