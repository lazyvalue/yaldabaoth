# Worklog: import Cog workflows

**Date:** 2026-08-09
**Branches touched:** main (working tree)

## Cog execution evidence

- Graph id: `bxp`

### Initial render

```text
graph import-cog-workflows-yaldabaoth (frontiers)
frontier 0: install-cog-skills [open], merge-cog-policy [open]
frontier 1: validate-compatibility [open]
frontier 2: omega [open] (omega)
```

### Node execution

- `62k` `install-cog-skills`: claimed → closed; output: `{"artifacts":[".claude/skills/cog-plan",".claude/skills/cog-execute",".agents/skills/cog-plan",".agents/skills/cog-execute"],"verification":["quick_validate passed on Claude and Codex paths","Codex links resolve to canonical Claude skill bodies","OpenAI UI metadata generated"]}`
- `cu9` `merge-cog-policy`: claimed → closed; output: `{"artifacts":["CLAUDE.md","AGENTS.md","README.md",".claude/skills/plan/SKILL.md",".claude/skills/spec/SKILL.md",".claude/skills/worklog/SKILL.md","docs/worklog/template.md","scripts/check-cog-worklog.sh"],"verification":["canonical policy markers present","worklog checker shell syntax valid","existing Yaldabaoth workflows preserved and linked"]}`
- `jwg` `validate-compatibility`: claimed → closed; output: `{"verification":["four quick_validate checks passed","Claude and Codex discovery paths resolve","worklog checker passed","cargo test --workspace passed","git diff --check passed","user backlog edit preserved"]}`
- `mof` `omega`: claimed → closed; output: `{"outcome":"Cog planning and execution are integrated for Claude Code and Codex with auditable workflow evidence."}`

### Notes

- Node `62k`, seq `3`, topic `deviation`: adapted the reusable skills for host-specific actors, host-policy-aware delegation, and request-authorized materialization.
- Graph, seq `7`, topic `decision`: `.claude/skills/cog-*` is canonical; `.agents/skills/cog-*` links provide Codex discovery; `CLAUDE.md` owns policy and `AGENTS.md` bridges other hosts.

### Final status

- Status: `complete`

```text
graph import-cog-workflows-yaldabaoth (frontiers)
frontier 0: install-cog-skills [done], merge-cog-policy [done]
frontier 1: validate-compatibility [done]
frontier 2: omega [done] (omega)
```

## Built (with status)

- Imported `/cog-plan` and `/cog-execute` as Claude Code project skills with
  Codex discovery links and generated OpenAI interface metadata.
- Merged the fail-closed Cog lifecycle into the canonical repository policy and
  connected the existing plan, spec, and worklog skills without replacing their
  Yaldabaoth-specific responsibilities.
- Added auditable worklog evidence, a shell validator, and repository-facing
  setup documentation.

## Open / unresolved

- None. The pre-existing uncommitted `docs/backlog.md` change was preserved and
  excluded from this work.

## Decisions

- Keep Claude Code's `.claude/skills` layout canonical because Claude
  compatibility is a hard requirement; expose the same files to Codex through
  `.agents/skills` links so the two copies cannot drift.

## Verification status

- Both skills pass the skill-creator validator through all four discovery paths.
- Claude paths, Codex links, metadata, shell syntax, policy markers, worklog
  evidence, and repository diff hygiene are verified.
- `cargo test --workspace` passes; no runtime UI behavior changed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-09-import-cog-workflows.md` passes.

## Next

- Start `cogd`, then use `/cog-plan <goal>` and `/cog-execute <graph-id>` for
  the next non-trivial change.
