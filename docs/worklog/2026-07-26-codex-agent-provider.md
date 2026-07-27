# Worklog: Codex agent provider

**Date:** 2026-07-26
**Branches touched:** `codex-provider`

## Built (with status)

- Added durable per-session Claude/Codex provider identity across protocol,
  client, actor, spawner, roster, admin status, and WAL recovery.
- Added provider-aware ACP spawning and omitted Claude-only session metadata for
  Codex.
- Added explicit Claude/Codex creation rows, provider-aware labels, a `Codex`
  status-strip badge, and `N` → new Codex session in the Agent menu.
- Made Codex default to the cached ChatGPT login by scrubbing ambient API-auth
  variables; documented the explicit API-key opt-in.

## Open / unresolved

- Live Codex handshake is a human runtime check until `codex-acp` is installed on
  the machine running Yaldabaoth.

## Decisions

- ADR-0030: provider belongs to durable session identity so mixed backends resume
  correctly.

## Verification status

- `cargo test --lib`: 160 passed, 2 ignored.
- `cargo test --bin yalda-gpui`: 478 passed, 1 ignored.
- `cargo test --bin yalda-session-server`: 47 passed.
- `cargo build --bin yalda-gpui --bin yalda-session-server`: passed.
- `git diff --check`: passed.
- `codex login status`: logged in using ChatGPT.
- Runtime still needs: install `@agentclientprotocol/codex-acp`, confirm a live
  adapter handshake, start one Claude and one Codex session, restart the server,
  and verify both resume under their original providers.

## Next

- Perform the live two-provider smoke test on a machine with both ACP adapters.
