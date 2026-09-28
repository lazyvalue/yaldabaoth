# Worklog: cog-tile-yalda-graphs

**Date:** 2026-09-27 · **Graph:** `sy6` · **Branch:** `cog-unfiled-graphs` → `main` (`0074efc`)

## Shipped
- Diagnosis: one cogd (127.0.0.1:7666); the Cog tile's Home lists only Topic bindings and no Yaldabaoth graph was bound.
- 34 Yaldabaoth graphs bound at `yaldabaoth/<area>::<graph-name>` (agent, models, workspace, jump, cog, session-server, diff, editor, platform, process); interim flat/`::plan` addresses tombstoned — `cog topic list yaldabaoth`.
- Unbound graphs listed under `unfiled graphs` (UXI-Cog-20) — `cog_home_lists_unbound_graphs_under_unfiled_folder`, negative control RED.
- Open graphs newest-first; finished graphs and all-finished subfolders in a collapsed `✓ done` (UXI-Cog-21) — `cog_home_groups_finished_graphs_in_collapsed_done_folder`, negative control RED.
- CLAUDE.md step 3 + cog-plan skill: bind every new graph. `yalda-gpui` 884 passed on `main`.

## Caveats
- NEEDS-RUNTIME: release GUI built 20:13, not restarted; live `cog` subprocess is harness gap 2.
- `ba0` (sealed duplicate of `fgb`) left unbound → appears under `unfiled graphs`.
- Graph rows now sort ahead of chat/note rows within a folder. `cargo-mutants` not installed locally.

## Decisions
- Leaf-style Topic addresses for Yaldabaoth graphs (first `<area>/<name>::plan` made one folder per graph — recorded as a graph deviation note); status grouping derived client-side with a done-status cache.

## Cog
- Status: `complete`

```text
graph cog-tile-shows-yalda-graphs (frontiers)
frontier 0: bind-existing [done], workflow-binding [done], tile-unfiled-graphs [done], rebind-by-area [done]
frontier 1: tile-status-grouping [done]
frontier 2: merge-log [done]
frontier 3: omega [done] (omega)
```
