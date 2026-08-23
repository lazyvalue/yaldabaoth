# Session lifecycle hardening

**Date:** 2026-08-22
**Branches touched:**

- `codex/session-client-timeouts` (`b5192b7`)
- `codex/session-terminal-ui` (`4fed80c`)
- `codex/session-lifecycle-server` (`39406ab`, `b62f421`)
- `codex/session-lifecycle-integration` (`2968a6e`)
- merged to `main` (`c354664`)

## Cog execution evidence

- Graph id: `epz`

### Initial render

```text
graph session-lifecycle-hardening (frontiers)
frontier 0: setup-worktrees [open]
frontier 1: client-timeouts [open], gui-terminal [open], server-lifecycle [open]
frontier 2: durable-state [open]
frontier 3: integrate-verify [open]
frontier 4: records-worklog [open]
frontier 5: omega [open] (omega)
```

### Node execution

Every node was claimed → closed with output using actor `claude-code`:

- `1e3r` `setup-worktrees`: created four isolated worktrees from `cae1848` and
  recorded the pre-existing dirty main state.
- `conb` `client-timeouts`: centralized request cleanup; commit `b5192b7`; three
  client timeout/late-response guards passed with observed-RED controls.
- `dpzc` `gui-terminal`: settled detach/reject to Idle and blocked archived
  optimistic submit; commit `4fed80c`; three real GUI-path guards passed RED/green.
- `obty` `server-lifecycle`: added explicit lifecycle, terminal pending-work
  handling, restart fencing, retryable unarchive, and connection-local duplicate
  attach cleanup; commit `39406ab`; server and real-wire guards passed.
- `zjxh` `durable-state`: persisted settings and prompt transactions, recovered
  fresh-spawn sessions, and made Close delete-before-drop; commit `b62f421`;
  durability negative controls and suites passed.
- `ohb4` `integrate-verify`: merged all branches, corrected the observer attach
  interaction found by the real-wire suite, ran combined tests and mutation
  testing, and merged integration to main as `c354664`.
- `fuoe` `records-worklog`: added bug-0054 through bug-0057 and this validated
  worklog, then merged the documentation to main.
- `nnqo` `omega`: confirmed fixes, verification, records, and main merge.

### Notes

- Node `ohb4`, seq `8`, topic `deviation`: unconditional actor-level forwarder
  release broke a legitimate observer and starved the healthy owner. Cleanup was
  narrowed to the connection-local subscription task map; the failing real-wire
  slow-subscriber test then passed.
- The first all-target check was sandbox-blocked because GPUI's Metal compiler
  could not write `~/.cache/clang`; the approved host-cache rerun passed.
- Repository `cargo fmt` with the host's newer rustfmt rewrites broad pre-existing
  formatting. Verification used `git diff --check`; no repository-wide formatting
  rewrite was merged.

### Final status

- Status: `complete`

```text
graph session-lifecycle-hardening (frontiers)
frontier 0: setup-worktrees [done]
frontier 1: client-timeouts [done], gui-terminal [done], server-lifecycle [done]
frontier 2: durable-state [done]
frontier 3: integrate-verify [done]
frontier 4: records-worklog [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- Terminal lifecycle failures now settle busy state, queued work, and the Agent
  Tile instead of leaving sessions Thinking forever.
- Session-client timeouts, write failures, disconnects, and late responses no
  longer retain stale pending request senders.
- Restart/unarchive/spawn/attach transitions are generation-fenced and retryable,
  without disconnecting legitimate observer connections.
- Permission/model choices and queued prompt text/images are durable. Explicit
  Close cannot resurrect a session after a WAL deletion failure.
- All implementation branches are committed and merged to `main`.

## Open / unresolved

- The release binaries still need activation after the documentation merge;
  runtime session counts and reconnect state will be appended before handoff.
- Existing compiler warnings and repository-wide rustfmt drift are outside this
  lifecycle repair.

## Decisions

- Lifecycle is explicit server state; absence of a channel is not itself a state.
- Prompt durability is write-ahead: intent must fsync before admission, and only
  an intent without a terminal outcome is recoverable.
- Close favors a visible error with live state retained over deleting memory and
  guaranteeing resurrection from undeleted durable state.
- Multiple connections may observe one session; duplicate cleanup is owned by
  each connection, not the session actor.

## Verification status

- `git diff --check cae1848..2968a6e`: passed.
- `cargo check --all-targets --features test-support`: passed with existing warnings.
- `cargo test --lib`: **181 passed, 0 failed, 2 ignored**.
- `cargo test --bin yalda-session-server --features test-support`: **65 passed**.
- `cargo test --bin yalda-gpui`: **688 passed, 0 failed, 2 ignored**.
- `cargo test --test session_resilience_test`: **10 passed**.
- `cargo test --test session_transcript_test`: **14 passed**.
- `cargo mutants --config mutants.toml --in-diff /tmp/session-lifecycle.diff
  --baseline skip --jobs 2`: **3 mutants tested, 3 caught**.
- Server/client/durability fixes additionally have eight observed-RED manual
  negative controls on their exact predicates.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-22-session-lifecycle-hardening.md`:
  passes.

## Next

- Build and activate both release binaries, confirm the GUI reconnects, and
  inspect live/archived session counts without deleting any WAL.
