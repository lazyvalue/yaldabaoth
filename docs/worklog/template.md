# Worklog: <slug>

**Date:** YYYY-MM-DD
**Branches touched:** <branch (commit) — one line each>

## Cog execution evidence

- Graph id: `<id>`

### Initial render

Paste the output shown to the user before implementation began:

```text
graph (frontiers)
frontier 0: <node> [open]
...
```

### Node execution

- `<node-id>` `<name>`: claimed → closed; output: `<meaningful JSON summary>`

### Notes

- `<graph|node>`, seq `<seq>`, topic `<topic>`: `<decision or deviation>`
- Or: None

### Final status

- Status: `complete`

```text
graph (frontiers)
...
frontier N: omega [done] (omega)
```

## Built (with status)
- <what shipped, on which branch, verified how (builds / tests / runtime)>

## Open / unresolved
- <what's deferred, flagged, or unfinished — link backlog items>

## Decisions
- ADR-NNNN: <one-line> — <why it came up>

## Verification status
- <what's runtime-verified vs needs-human; the harness gap>
- `scripts/check-cog-worklog.sh <this-worklog>` passes.

## Next
- <the obvious next moves for the following session>
