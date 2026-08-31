# Worklog: readable Cog communication authors

**Date:** 2026-08-30
**Branches touched:** `cog-readable-agent-authors` (pending commit)

## Cog execution evidence

- Graph id: `k4v`

### Initial render

```text
graph k4v (frontiers)
frontier 0: fcb spec-authors [open]
frontier 1: cz8 build-authors [open]
frontier 2: 522 verify-authors [open]
frontier 3: xgt document-authors [open]
frontier 4: wvm omega [open] (omega)
```

### Node execution

- `fcb` `spec-authors`: claimed → closed; output: `{"result":"Defined UXI-Cog-19 with name-first labels, stable-id retention, and fallbacks."}`
- `cz8` `build-authors`: claimed → closed; output: `{"result":"Resolved communication authors from the retained Cog home address directory and instrumented painted labels."}`
- `522` `verify-authors`: claimed → closed; output: `{"result":"Focused author, topic-detail, and agent-mail GPUI tests pass; negative control fails on bare ncz as intended."}`
- `xgt` `document-authors`: claimed → closed; output: `{"result":"Updated UXI status and recorded execution, verification, and runtime gap."}`
- `wvm` `omega`: claimed → closed; output: `{"result":"Integrated implementation, tests, documentation, and release build."}`

### Notes

- `522`, verification: the first paint probe began after invalidation and missed the author; it was corrected to begin before an explicit GPUI repaint. No production behavior changed.

### Final status

- Status: `complete`

```text
graph k4v (frontiers)
frontier 0: fcb spec-authors [done]
frontier 1: cz8 build-authors [done]
frontier 2: 522 verify-authors [done]
frontier 3: xgt document-authors [done]
frontier 4: wvm omega [done] (omega)
```

## Built (with status)

- Cog communication headers now show `name · id` for registered agents across typed Topic Chat and agent-mail cards, with stable fallbacks for unknown or empty actors.
- Focused GPUI author, typed-detail, and agent-mail tests pass; release build passes.

## Open / unresolved

- Exact typography, colours, and pixel placement remain runtime gap #1. No running GUI was restarted without operator permission.

## Decisions

- No ADR required: this reuses the already-loaded Cog address directory and existing communication card rather than introducing a new data source or architectural boundary.

## Verification status

- Headless GPUI coverage verifies reducer flow, label semantics, and painted author/card bounds. Human visual inspection remains optional for exact styling.
- Negative control verified that bypassing address resolution fails with bare `ncz`; the implementation was restored and the guard passed again.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-30-cog-readable-agent-authors.md` passes.

## Next

- Optionally restart `yalda-gpui` when convenient to inspect the name-first author labels in the live Cog window.
