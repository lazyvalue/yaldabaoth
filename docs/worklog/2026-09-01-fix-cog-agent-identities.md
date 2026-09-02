# Worklog: fix Cog agent identities everywhere

**Date:** 2026-09-01
**Branches touched:** `fix-cog-agent-identities` (pending commit)

## Cog execution evidence

- Graph id: `m72`

### Initial render

```text
graph fix-cog-agent-identities-everywhere (frontiers)
frontier 0: inventory-identities [open]
frontier 1: guard-identities [open]
frontier 2: format-identities [open]
frontier 3: verify-identities [open]
frontier 4: document-identities [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `0obs` `inventory-identities`: claimed → closed; output: `{"result":"Inventoried every semantic identity field and expanded UXI-Cog-19."}`
- `pfgj` `guard-identities`: claimed → closed; output: `{"result":"Real Home-to-Chat paint guard observed RED on raw creator ncz."}`
- `32dm` `format-identities`: claimed → closed; output: `{"result":"Applied shared name-first identity and list formatting across Cog surfaces."}`
- `n81r` `verify-identities`: claimed → closed; output: `{"result":"Focused GPUI tests and release build pass; lookup-bypass negative control fails as intended."}`
- `po6w` `document-identities`: claimed → closed; output: `{"result":"Reconciled UXI-Cog-19, bug-0065, manifest, and worklog."}`
- `ehdj` `omega`: claimed → closed; output: `{"result":"Integrated the verified repair into main and rebuilt release."}`

### Notes

- `n81r`, deviation: `cargo-mutants` is not installed on this host. The mandatory manual lookup-bypass negative control was observed RED instead.

### Final status

- Status: `complete`

```text
graph fix-cog-agent-identities-everywhere (frontiers)
frontier 0: inventory-identities [done]
frontier 1: guard-identities [done]
frontier 2: format-identities [done]
frontier 3: verify-identities [done]
frontier 4: document-identities [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- Every semantic Cog agent identity uses the retained Home address directory and displays `name · id`; optimized `yalda-gpui` builds successfully.

## Open / unresolved

- Exact glyph/color inspection remains runtime gap #1. No GUI or session server was restarted.
- `cargo-mutants` is unavailable on this host; the targeted manual mutation was verified RED.

## Decisions

- No ADR required: this centralizes an existing view projection without changing data ownership or routing identity.

## Verification status

- Real reducer/click paths and label-keyed paint probes cover Chat, graph transitions/notes, and agent mail; neighboring typed-detail coverage passes.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-01-fix-cog-agent-identities.md` passes.

## Next

- Restart the GUI only when the operator chooses to activate the rebuilt release binary.
