# Default Codex subagents to Luna

**Date:** 2026-08-26
**Branches touched:** `codex/codex-luna-subagents` (`dc9c621`) →
`main` (`df3ec37`)

## Cog execution evidence

- Graph id: `w25`

### Initial render

```text
graph codex-luna-subagents (frontiers)
frontier 0: spec-luna-default [open]
frontier 1: implement-luna-default [open]
frontier 2: verify-and-record [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `5qzt` `spec-luna-default`: claimed → closed; output specified
  `UXI-AgentTile-44`, the Luna default, parent-model independence, and provider
  boundaries.
- `j2bc` `implement-luna-default`: claimed → closed; output recorded the merged
  `CODEX_CONFIG` overlay, unchanged Claude behavior, subprocess coverage, and
  the observed structural RED.
- `ghvq` `enable-luna-runtime`: added after live verification disproved the
  adapter-only assumption; claimed → closed; output recorded standalone runtime
  selection, explicit override preservation, and the authenticated Luna child.
- `xfm9` `verify-and-record`: claimed → closed; output recorded focused/full
  tests, live verification, release builds, diff hygiene, feature commit, and
  main integration.
- `a6tg` `omega`: claimed → closed; output confirmed the requested capability,
  evidence, documentation, and integration are complete.

### Notes

- Graph seq `10`, topic `deviation`: codex-acp 1.1.7's bundled Codex 0.145.0
  rejected Luna, while the ChatGPT app's 0.148 alpha spawned Sol despite the
  injected default, so the graph gained a runtime-selection node.
- Node `ghvq`, seq `2`, topic `decision`: preserve a non-empty explicit
  `CODEX_PATH`; otherwise resolve standalone Codex from the process PATH or the
  user's login shell. Stable Codex was updated from 0.146.0 to 0.149.1.
- Node `ghvq`, seq `3`, topic `deviation`: codex-acp reports Sol when reopening
  the spawned child, but the child's durable applied settings and actual turn
  record Luna. The live guard therefore asserts the durable runtime setting.
- Node `xfm9`, seq `6`, topic `deviation`: the worklog was drafted before graph
  closure, then validated after omega because the validator requires a complete
  graph.
- Repository-wide `cargo fmt --all -- --check` exposes pre-existing formatting
  drift. No bulk formatting was retained; scoped changes pass
  `git diff --check`.

### Final status

- Status: `complete`

```text
graph codex-luna-subagents (frontiers)
frontier 0: spec-luna-default [done]
frontier 1: implement-luna-default [done]
frontier 2: enable-luna-runtime [done]
frontier 3: verify-and-record [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

- Yalda-hosted Codex sessions merge
  `agents.default_subagent_model = "gpt-5.6-luna"` into `CODEX_CONFIG` without
  pinning the parent model or changing Claude.
- Yalda supplies `CODEX_PATH` so codex-acp uses the independently updated
  standalone Codex CLI rather than its incompatible bundled 0.145.0 runtime.
  Explicit host overrides remain authoritative.
- Added pure configuration/path guards, a real subprocess-environment guard,
  and an ignored authenticated live test that observes a spawned child's
  durable applied model.
- Feature commit `dc9c621` merged to main as `df3ec37`; pre-existing user edits
  to Cargo metadata and local lock/sidecar files were preserved.

## Open / unresolved

- codex-acp's child-resume response reports the parent model even though the
  child ran with Luna. This does not block spawning or execution; the separate
  display-fidelity follow-up is tracked in [the backlog](../backlog.md).

## Decisions

- No ADR added. This is a provider/runtime compatibility rule owned by
  `UXI-AgentTile-44` and the Cog decision note, not a repository-wide
  architectural choice.

## Verification status

- Negative control: before insertion, the config guard failed with Terra where
  Luna was required.
- Authenticated runtime controls: bundled Codex 0.145.0 rejected Luna; the app
  alpha selected Sol; standalone stable Codex 0.149.1 passed without an
  externally supplied `CODEX_PATH`.
- Live child rollout recorded `thread_settings_applied.model = gpt-5.6-luna` and
  ran the child turn with Luna.
- Focused Luna/path tests: 3 passed.
- ACP channel suite: 27 passed, 1 intentionally ignored.
- Full library suite: 213 passed, 2 intentionally ignored.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed in
  both the isolated worktree and the integrated dirty main checkout.
- Integrated-main focused Luna/path tests: 3 passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-26-codex-luna-subagents.md`
  passes.

## Next

- Restart or launch Yalda when desired; no session server was running at handoff.
- If model-label fidelity matters in the child inspector, address the tracked
  codex-acp resume-report mismatch independently from Luna execution.
