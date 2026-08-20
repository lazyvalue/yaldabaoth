# Fix Codex stop and interruption loops

**Date:** 2026-08-20
**Branches touched:** `codex/fix-codex-stop-steering-loop` (`778abf6`) →
`main` (`ec0f6be`)

## Cog execution evidence

- Graph id: `vkh`

### Initial render

```text
graph fix-codex-stop-steering-loop (frontiers)
frontier 0: specify-recurrence [open]
frontier 1: implement-native-steering [open]
frontier 2: verify-ship [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `2bls` `specify-recurrence`: claimed → closed; output recorded the bug-0036
  recurrence, capability-gated contract, exact two-question-plus-Stop GUI guard,
  and its observed pre-fix RED.
- `i2b5` `implement-native-steering`: claimed → closed; output recorded root
  capability negotiation, native direct/server routing, compatibility fallback,
  and ordered-control coverage.
- `yym3` `verify-ship`: claimed → closed; output records full-suite, mutation,
  worklog, release-build, commit, merge, and integrated-main verification.
- `4gfw` `omega`: claimed → closed; output confirms the graph objective and all
  required shipping evidence are complete.

### Notes

- Graph seq `20`, topic `decision`: the initial capable-Codex prompt, every
  native steer, and explicit Stop use one `NativeSteeringCommand` stream. The
  subprocess wire guard proved a separate initial-prompt queue could allow a
  fast steer to overtake it.
- Graph seq `21`, topic `deviation`: an initial capability mutation survivor was
  closed with a direct legacy server-fallback regression; the expanded ACP pass
  caught 10/10 mutants. Copy-mode mutation hit sandbox cache and setup failures,
  so the receiver mismatch was corrected and warmed in-place mutation was used.
- Repository-wide formatter checking exposed pre-existing drift. No bulk
  formatting was retained; scoped changes pass `git diff --check`.

### Final status

- Status: `complete`

```text
graph fix-codex-stop-steering-loop (frontiers)
frontier 0: specify-recurrence [done]
frontier 1: implement-native-steering [done]
frontier 2: verify-ship [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- Capable Codex sessions negotiate root `_meta.steering.supported` and send the
  initial prompt, successive `_session/steering` requests, and explicit Stop
  through one ordered worker-control stream.
- Direct and session-server paths share the same semantics. Older adapters keep
  the cancel-then-prompt fallback; Claude prompt queueing is unchanged.
- The user action path is guarded through the real GPUI submit and Stop handlers,
  while a fake ACP subprocess verifies actual production JSON-RPC wire order.

## Open / unresolved

- The installed, authenticated Codex adapter was not driven live. Restart Yalda
  and its session server to load the rebuilt release binaries, then the reported
  interaction remains the final human confirmation.

## Decisions

- No ADR added. The capability-specific ordering rule is recorded in the Agent
  Tile contract, turn-steering spec, bug record, and Cog decision note.

## Verification status

- Exact negative control: the new GUI sequence failed before the fix because the
  first question did not reach native steering.
- `cargo test --lib`: 177 passed, 2 ignored.
- `cargo test --bin yalda-session-server --features test-support`: 51 passed.
- `cargo test --bin yalda-gpui --features test-support`: 688 passed, 2 ignored.
- Focused GUI/server/capability/wire guards passed, including production
  subprocess order `prompt → steer 1 → steer 2 → cancel`.
- Mutation controls caught 3/3 GUI routing mutants and 10/10 expanded ACP
  capability/routing mutants.
- `cargo build --release --bin yalda-gpui --bin yalda-session-server`: passed.
- After merge, the exact GUI action-path guard, production subprocess wire-order
  guard, both server actor guards, and both release builds passed from the actual
  `main` checkout.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-20-fix-codex-stop-steering-loop.md`
  passes.

## Next

- Restart Yalda and its session server, then confirm multiple rapid Codex
  questions followed by ⌘. are acknowledged in order before cancellation.
