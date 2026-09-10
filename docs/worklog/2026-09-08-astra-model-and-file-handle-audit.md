# Worklog: Astra model and file-handle audit

**Date:** 2026-09-08

> **Correction (2026-09-10):** The Astra portion of this worklog targeted the
> wrong provider. Astra is Codex model `gpt-6-astra`, not a Claude model. The
> Claude allowlist code and filesystem setting were reverted. Bug 0068 records
> the corrected Codex diagnosis and real-path verification. The file-handle
> audit below is unaffected.
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

This original conclusion was incorrect and is superseded by bug 0068. Astra is
Codex model `gpt-6-astra`; no Astra entry belongs in Claude session metadata.

## Verification status

- `cargo test --lib acp_channel::tests::session_meta_advertises_fable_5_1_only_to_claude`
- `cargo test --lib second_live_writer_is_refused_without_changing_wal`
- `cargo test --bin yalda-session-server archive_releases_runtime_state_and_wal_but_keeps_durable_session`
- `cargo build --release --bin yalda-gpui`
- `git diff --check`
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-08-astra-model-and-file-handle-audit.md`

`cargo-mutants` is not installed on this host, so the repository's mutation
command could not run. The manual negative control directly removed the changed
model entry and observed the intended guard failure.

## Runtime status

The GUI and session server were not restarted. The original activation claim
about new Claude sessions receiving Astra is withdrawn; see bug 0068.

### Final status

- Status: `complete`

```text
graph astra-model-and-file-handle-audit (frontiers)
frontier 0: add-astra [done], audit-handles [done]
frontier 1: verify-log [done]
frontier 2: omega [done] (omega)
```
