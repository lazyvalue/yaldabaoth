# Agent compatibility bridge

The canonical repository instructions live in [CLAUDE.md](CLAUDE.md), because
this project's reusable orchestration assets must remain compatible with Claude
Code. Every agent operating in this repository must read and follow `CLAUDE.md`
in full.

In particular, its **Mandatory Cog orchestration** policy applies to non-trivial
feature and process changes: create and show a real Cog graph before tracked-file
edits, execute by claiming and closing nodes, record deviations, verify the
worklog, and finish with a complete graph. Host-native plan tools cannot replace
Cog. If Cog is unavailable, stop unless the user explicitly opts out.

Use the stable actor for the current host (`claude-code` for Claude Code,
`codex` for Codex). Do not duplicate or independently evolve the policy here;
`CLAUDE.md` is the single source of truth and this file is only a discovery
bridge for tools that read `AGENTS.md`.
