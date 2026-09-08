# Worklog: Astra model and file-handle audit

**Date:** 2026-09-08
## Cog execution evidence

- Graph id: `8a8`

### Initial render

```text
graph astra-model-and-file-handle-audit (frontiers)
frontier 0: add-astra [open], audit-handles [open]
frontier 1: verify-log [open]
frontier 2: omega [open] (omega)
```

### Node execution

- `mr00` `add-astra`: claimed → closed; output: added the adapter alias `astra` to Claude-only session
  metadata, updated UXI-AgentTile-16, and guarded the real serialized
  `session/new` payload.
- `x26h` `audit-handles`: claimed → closed; output: audited WAL, scoped file, and ACP subprocess handle
  ownership. No leak was reproduced or localized, so no speculative lifecycle
  change was made.
- `ylt6` `verify-log`: claimed → closed; output: ran the focused guards and release build and recorded the
  results here.
- `lg4x` `omega`: claimed → closed; output: confirmed both requested outcomes,
  focused verification, release build, and worklog validation are complete.

### Notes

- No handle leak was localized; changing lifecycle code without a failing real-path
  guard would violate the repository's anti-circling policy.
- `cargo-mutants` is unavailable on this host. The changed Astra entry received a
  direct manual negative control instead.

## File-handle finding

Production file handles follow Rust ownership and RAII. Short-lived
`File`/`BufReader` values close when they leave scope. Each active managed
session intentionally retains one exclusively locked `SessionWal` descriptor;
archiving takes and drops it, and closing consumes the WAL before removing the
session. ACP child stdin/stdout and the `kill_on_drop` child are owned by the
worker runtime and close when that runtime exits.

The existing guards passed:

- `session_wal::tests::second_live_writer_is_refused_without_changing_wal`
  proves a second writer is rejected and dropping the owner releases its OS
  lock.
- `lifecycle_tests::archive_releases_runtime_state_and_wal_but_keeps_durable_session`
  proves archive drops the live WAL handle while preserving its file.

The local `yalda-session-server` and `yalda-gpui` systemd units were inactive,
so there was no live process for a descriptor census. This audit therefore
establishes the code ownership paths and their guards, not a live FD-count time
series.

## Astra model availability

Claude sessions now include `astra` in
`_meta.claudeCode.options.settings.availableModels`. The existing adapter still
owns validation, labels, ordering, and the advertised ACP model selector; Codex
sessions remain free of Claude-specific metadata.

The focused metadata test passed. For the mandatory negative control, removing
the `astra` entry made the same test fail at its Astra allowlist assertion; the
entry was restored and the test passed again.

## Verification status

- `cargo test --lib acp_channel::tests::session_meta_advertises_yalda_models_only_to_claude`
- `cargo test --lib second_live_writer_is_refused_without_changing_wal`
- `cargo test --bin yalda-session-server archive_releases_runtime_state_and_wal_but_keeps_durable_session`
- `cargo build --release --bin yalda-gpui`
- `git diff --check`
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-08-astra-model-and-file-handle-audit.md`

`cargo-mutants` is not installed on this host, so the repository's mutation
command could not run. The manual negative control directly removed the changed
model entry and observed the intended guard failure.

## Runtime status

The GUI and session server were not restarted. The release binary was rebuilt
only; newly created Claude sessions will receive Astra after the operator next
activates that binary.

### Final status

- Status: `complete`

```text
graph astra-model-and-file-handle-audit (frontiers)
frontier 0: add-astra [done], audit-handles [done]
frontier 1: verify-log [done]
frontier 2: omega [done] (omega)
```
