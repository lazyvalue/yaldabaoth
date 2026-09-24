# Worklog: Config-file-driven Claude model advertisement

**Date:** 2026-09-24
**Branch:** `config-driven-claude-models` (to merge to `main`)

## Cog execution evidence

- Graph id: `2qm`
- Follow-up to `7r9`, which added Opus 5.5 to the compiled list.

### Initial render

```text
graph config-driven-claude-models (frontiers)
frontier 0: spec-model-source [open]
frontier 1: implement-loader [open]
frontier 2: helper-script [open]
frontier 3: worklog [open]
frontier 4: omega [open] (omega)
```

### Node execution

- `km3r` `spec-model-source`: claimed → closed; output: UXI-AgentTile-16 now specs the config-driven list (defaults ⋈ `~/.config/yalda/claude-models.conf`, adapter passthrough of unknown ids, the helper script).
- `7rak` `implement-loader`: claimed → closed; output: added `claude_models_config_path` (+ `cfg(test)` override seam), `claude_available_models` (defaults ++ file, deduped), wired into `agent_session_meta`; two negative-controlled tests.
- `anx1` `helper-script`: claimed → closed; output: `scripts/yalda-add-claude-model.sh` (append+dedup, validation, adapter-recognition report, `--refresh-adapter`).
- `s8jy` `worklog`: claimed → closed; output: this entry, validated by `check-cog-worklog.sh`.
- `r1d3` `omega`: claimed → closed; output: confirms the whole contract (spec + loader + tests + script + log).

### Notes

- Graph note `adapter`: upgraded global `claude-agent-acp` 0.75.1 → 0.81.2 (bundled `@anthropic-ai/claude-agent-sdk` 0.3.257 → 0.3.280). Neither build lists `claude-opus-5-5`.
- Graph note `passthrough` (correction to the `7r9` handoff): the adapter does **not** drop unknown model ids. `dist/session-model.js` `applyAvailableModelsAllowlist` pushes an unmatched `availableModels` entry as `{value:id, displayName:id}` and passes it verbatim to `setModel`/the API. So `claude-opus-5-5` is a working picker option today; only its label/capabilities lack SDK polish. This is why the config-file passthrough mechanism is sufficient — there is no adapter allowlist gate to satisfy.

### Final status

- Status: `complete`

```text
graph config-driven-claude-models (frontiers)
frontier 0: spec-model-source [done]
frontier 1: implement-loader [done]
frontier 2: helper-script [done]
frontier 3: worklog [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

- The Claude `availableModels` list advertised on `session/new` / `session/load` is now the compiled `YALDA_CLAUDE_AVAILABLE_MODELS` defaults **merged (ordered, de-duplicated) with an optional per-install file** — `$YALDA_CLAUDE_MODELS`, else `~/.config/yalda/claude-models.conf`, one id per line (`#`/blank ignored). Read fresh in the lib crate per call, so an edit is live on the next session with no recompile or restart. Absent file ⇒ byte-identical to the prior compiled behavior.
- `scripts/yalda-add-claude-model.sh <model-id>` performs the edit (dedup + id validation), lists the result, reports whether the installed adapter recognizes the id, and optionally (`--refresh-adapter`) upgrades `claude-agent-acp`.
- Adding a Claude model is now: **run the helper (or add a line to the config file), open a new session.** No Rust edit.

## Verification status

- `cargo test --lib acp_channel::tests::claude_available_models_merges_config_file_over_defaults` passes.
- `cargo test --lib acp_channel::tests::session_meta_reflects_config_file_addition` passes (drives the real `agent_session_meta` → `session/new` wire payload).
- `cargo test --lib acp_channel::tests::session_meta_advertises_latest_claude_models_only_to_claude` still passes (no file ⇒ defaults only).
- Negative controls observed RED: (1) neutering the config-file read ⇒ both file tests fail (config id absent from the list/wire); (2) breaking the dedup ⇒ merge test fails (duplicate `claude-opus-6`). Restored, re-green.
- `cargo check --bin yalda-gpui` and `--bin yalda-session-server` clean (pre-existing warnings only).
- `bash -n scripts/yalda-add-claude-model.sh` clean; functional run against a temp `$YALDA_CLAUDE_MODELS` verified add/dedup/validation/listing/adapter-note.
- `git diff --check` clean. `scripts/check-cog-worklog.sh docs/worklog/2026-09-24-config-driven-claude-models.md` passes.

## Runtime status

- **NEEDS-RUNTIME (verify_harness gap 2 — live subprocess/agent loop):** the end-to-end live path (session-server reads `~/.config/yalda/claude-models.conf`, spawns the real adapter, the id appears in the live picker and switches) is not headlessly testable. The `#[cfg(not(test))]` path resolver (env var / home-dir join) is not unit-covered by construction; the parse/merge substance is.
- The running GUI and session server were **not** restarted. Because the list is read per `session/new`, a **newly opened Claude session** on the already-running server picks up any `claude-models.conf` entry immediately — no restart needed for config additions. (The lib-crate code change itself is in the branch/`main`, not yet in the running server binary; the running server already advertises the graph-`7r9` compiled list including `claude-opus-5-5`.)
