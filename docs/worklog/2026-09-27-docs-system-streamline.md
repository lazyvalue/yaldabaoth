# Worklog: docs-system-streamline

**Date:** 2026-09-27 · **Graph:** `od4` · **Branch:** `docs-streamline` → `main` (`c02cebb`)

## Shipped
- `docs/projects/` + `/plan` removed. The 52 open subtasks from 6 projects were moved to Cog bulletin `1pn` (`yaldabaoth/docs::open-project-threads`). The GPUI render model is kept at `docs/reference/gpui-render-model.md`; the /clear saga is kept at `docs/bugs/saga-clear-worksheet-invisible/`.
- `docs/ux-invariants.md` removed. Live `INV-UX-N` refs (in 23 files across docs, specs, and src comments) were rewritten to `UXI` ids. The crosswalk is kept in `docs/components/README.md` so history docs still resolve. The UX PreToolUse hook was retargeted.
- `docs/ux-patterns/` added: universal laws `UXP-1..5`. `/new-ux` now checks them.
- `docs/UX.md` deleted (stale). The component index is now the surface catalog.
- Backlog cut from 1427 lines to about 70: only recent `NEEDS-RUNTIME` items remain. The other 77 non-done entries were archived verbatim to bulletin `whj` (`yaldabaoth/docs::backlog-archive`).
- Worklog format slimmed; `scripts/check-cog-worklog.sh` updated to match. Verified: the template correctly fails the check and this entry passes.
- Root `CLAUDE.md` cut from 464 to 209 lines. Harness detail went to `docs/dev-system.md`; GUI detail went to `src/bin/yalda-gpui/CLAUDE.md`.
- `cargo check --bin yalda-gpui` passes on `main`. The only src changes are comments.

## Caveats
- Judgment call: `NEEDS-RUNTIME` items dated before 2026-09-01 are presumed exercised by daily use and were archived rather than kept.
- Old worklogs, bugs, and ADRs still cite `INV-UX-N`, `docs/projects/`, and `UX.md` as history. Resolve them via the crosswalk or git.
- `UXP-5` is `partial`: it is guarded per binding, with no sweep over all roots.

## Decisions
- Cog graphs and bulletins replace durable project tickets and the open-work backlog. `docs/ux-patterns/` (universal laws) is distinct from `docs/components/common/` (opt-in shared parts).

## Cog
- Status: `complete`

```text
graph docs-system-streamline (frontiers)
frontier 0: fold-ux-md [done], rm-ux-invariants [done], slim-backlog-worklog [done], rm-projects [done]
frontier 1: ux-patterns [done]
frontier 2: trim-claude-md [done]
frontier 3: verify-merge [done]
frontier 4: omega [done] (omega)
```
