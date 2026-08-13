# Worklog: Agent Tile linewise `V` selection

**Date:** 2026-08-13
**Branches touched:** `fix-agent-v-line-selection` (`8ce2a84` — fix/spec/tests),
then `main` (`f3ca6b5` — merge and release rebuild; worklog commit follows)

## Cog execution evidence

- Graph id: `n3r`

### Initial render

```text
graph fix-agent-v-line-selection (frontiers)
frontier 0: reproduce-line-drift [open]
frontier 1: implement-linewise-mode [open]
frontier 2: verify-ship-fix [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `kuvk` `reproduce-line-drift`: claimed → closed; output:
  `{"summary":"Strengthened the existing Agent Tile V+j real-path guard with unequal line lengths and an exact selected-text assertion.","negative_control":"Actual selection was one\\ntwo instead of the complete longer second line; sticky column 3 caused the drift."}`
- `56uw` `implement-linewise-mode`: claimed → closed; output:
  `{"summary":"Implemented explicit V-style linewise selection state and post-motion logical-line normalization; lowercase v remains characterwise.","verification":["four worksheet_v_ guards passed","quote, Esc, and reply selection regressions passed","git diff --check passed"]}`
- `7yyj` `verify-ship-fix`: claimed → closed; output:
  `{"summary":"Verified and shipped the linewise V-selection fix on main.","verification":["570 passed; 0 failed; 1 ignored","2 of 2 scoped mutants caught","release build passed","implementation 8ce2a84 merged as f3ca6b5"]}`
- `gso2` `omega`: claimed → closed; output:
  `{"summary":"Uppercase V now maintains a true linewise selection across subsequent motions in both directions without changing lowercase-v behavior."}`

### Notes

- Graph, seq `16`, topic `deviation`: the generated ship-node acceptance text
  placed worklog validation before omega. The repository validator requires a
  truthful complete graph, so the final worklog was written and validated
  immediately after omega closed.
- Graph, seq `19`, topic `deviation`: two existing steering tests depend on a
  reachable session server and failed when the main-checkout full run fell back
  to direct spawn, although both passed alone. The full suite was rerun against
  a dedicated temporary `YALDA_SESSION_SOCKET`: 570 passed, 0 failed, 1 ignored.
  The isolated server stopped cleanly and removed its temporary state.

### Final status

- Status: `complete`

```text
graph fix-agent-v-line-selection (frontiers)
frontier 0: reproduce-line-drift [done]
frontier 1: implement-linewise-mode [done]
frontier 2: verify-ship-fix [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **FIXED — bug-0037 / UXI-AgentTile-34.** Uppercase `V` enters persistent
  linewise selection instead of degrading to character-column selection after
  the first motion.
- Subsequent normal motions keep the active selection edge on complete logical
  line boundaries, including forward and reverse vertical movement and Agent
  Tile tool-anchor hops.
- Lowercase `v` remains characterwise. `Esc` and reply completion retain their
  existing selection-exit behavior.
- The Agent Tile transcript contract and bug manifest now record the linewise
  distinction.

## Open / unresolved

- `NEEDS-RUNTIME`: restart the currently running Yalda app so it loads the
  rebuilt release binary. The production key-dispatch and selection-text paths
  are covered by the headless GPUI harness; live pixels were not inspected.

## Decisions

- No ADR needed. This restores Vim-style behavior already implied by the
  uppercase `V` command rather than introducing a new architecture.
- Linewise state lives in the shared editor view and normalization is invoked
  after normal motions, keeping the selection contract consistent across edit
  surfaces while leaving lowercase `v` independent.

## Verification status

- Mandatory negative control: before the fix,
  `worksheet_v_then_j_extends_selection` selected only `one\ntwo` when the
  second line was longer; its exact full-line assertion failed as intended.
- `cargo test --bin yalda-gpui worksheet_v_ -- --nocapture`: 4 passed.
- Repeated-`V`, multiline quote, lowercase-`v`, `Esc`, and reply-clear focused
  regressions passed.
- `cargo mutants --no-config --features test-support --file src/editor.rs --re
  'delete ! in EditorView::normalize_linewise_selection|replace < with == in
  EditorView::normalize_linewise_selection' --in-place --baseline skip
  --timeout 120 --cargo-arg=--bin=yalda-gpui -- worksheet_v_`: 2 caught, 0 missed.
- `cargo test --bin yalda-gpui --features test-support` on `main`, with an
  isolated temporary session server: 570 passed, 0 failed, 1 ignored.
- `cargo build --release --bin yalda-gpui`: passed.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh
  docs/worklog/2026-08-13-agent-linewise-v-selection.md` passes.

## Next

- Restart Yalda and confirm `V`, then `j`/`k`, highlights every selected line
  edge-to-edge in a live Agent Tile transcript.
