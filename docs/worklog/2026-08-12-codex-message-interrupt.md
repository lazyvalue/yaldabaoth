# Worklog: Codex normal-message interruption

**Date:** 2026-08-12
**Branches touched:** bug-codex-message-interrupt (`f9b3049` — fix/spec/tests;
worklog commit follows), then `main` (merge and release rebuild)

## Cog execution evidence

- Graph id: `bkb`

### Initial render

```text
graph codex-message-interrupts-turn (frontiers)
frontier 0: localize-contract [open]
frontier 1: implement-interrupt [open]
frontier 2: verify-ship [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `jlui` `localize-contract`: claimed → closed; output:
  `{"summary":"Localized the omission to submit_agent -> submit_compose -> send_prompt_to_session; specified provider-aware UXI-AgentTile-13 and added the real-path guard.","negative_control":"Before the production fix, the guard failed at mid-turn Codex submit must interrupt the running turn after the prompt path passed."}`
- `shqz` `implement-interrupt`: claimed → closed; output:
  `{"summary":"Factored the graceful cancel transport and invoked it for clean Awaiting Codex normal submits before the replacement prompt; Claude, idle Codex, and StopRequested controls are unchanged.","verification":["targeted guard passed","steering regressions passed","Stop regression passed"]}`
- `icsh` `verify-ship`: claimed → closed; output:
  `{"summary":"Verified, documented, committed, merged, and rebuilt the Codex normal-message interrupt fix.","verification":["569 GPUI tests passed; 1 ignored","2 predicate mutants caught","release build passed","git diff --check passed","Cog worklog validator passed"]}`
- `s6st` `omega`: claimed → closed; output:
  `{"outcome":"A normal message now gracefully interrupts a clean in-flight Codex turn and becomes its replacement prompt; the verified fix is on main."}`

### Notes

- Graph, seq `5`, topic `deviation`: updated the frozen legacy
  `docs/ux-invariants.md` duplicate alongside authoritative UXI-AgentTile-13;
  leaving it unchanged would directly contradict the provider-aware contract.
- Graph, seq `14`, topic `deviation`: cargo-mutants 27 defaults to
  `.cargo/mutants.toml`, so the root `mutants.toml` needed `--config`. Its
  whole-package baseline also reaches pre-existing `AgentSpawner` signature
  drift in `tests/agent_transport_fake_test`; the affected `yalda-gpui` binary
  target was isolated explicitly and both changed predicate mutants were caught.
- Graph, seq `15`, topic `deviation`: repository-wide `cargo fmt --check` has
  extensive pre-existing drift. No bulk formatting rewrite was applied;
  `git diff --check` and scoped build/test gates are clean.

### Final status

- Status: `complete`

```text
graph codex-message-interrupts-turn (frontiers)
frontier 0: localize-contract [done]
frontier 1: implement-interrupt [done]
frontier 2: verify-ship [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **FIXED — bug-0036 / UXI-AgentTile-13.** A normal submit on a clean
  `Awaiting` Codex session sends one graceful ACP `session/cancel` before the
  typed replacement prompt.
- The submit remains a normal user turn and does not enter `StopRequested` or
  the Stop button's second-press force-restart policy.
- Claude keeps its promptQueueing steer. Idle Codex and a Codex turn already in
  `StopRequested` do not emit a new cancel.
- The Stop action and normal-message interruption share the transport-only
  cancellation helper, while each caller retains its own lifecycle policy.

## Open / unresolved

- `NEEDS-RUNTIME`: restart the currently running Yalda app so it loads the
  rebuilt binary, then confirm the interaction against a live Codex subprocess.
  The GUI's real submit path and production cancellation/prompt channel surfaces
  are covered headlessly; the external subprocess behavior was not driven here.

## Decisions

- No ADR needed. This corrects the provider-specific behavior of the existing
  turn-steering contract rather than introducing a new architectural choice.
- A replacement prompt after `StopRequested` does not enqueue a duplicate
  cancel, avoiding a stale cancellation racing the new prompt.

## Verification status

- Mandatory negative control: removing the Codex-awaiting interrupt call made
  `codex_normal_message_interrupts_in_flight_turn` fail exactly at
  `mid-turn Codex submit must interrupt the running turn`; preceding prompt
  delivery assertions passed. Restoring the call returned green.
- `cargo test --bin yalda-gpui --features test-support`: 569 passed, 0 failed,
  1 ignored.
- Focused steering and Stop regressions passed.
- `cargo mutants --config mutants.toml --in-place --in-diff <diff> --re
  'replace (&&|==)' -- --bin yalda-gpui`: 2 caught, 0 missed.
- `cargo build --release --bin yalda-gpui`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-12-codex-message-interrupt.md`
  passes.

## Next

- Restart Yalda and confirm that submitting a normal message while Codex is
  working visibly stops the old response and begins the replacement request.
