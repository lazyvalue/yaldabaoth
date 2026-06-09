# Worklog: quality gate + state-overhaul Phase B (5c/8b/10)

**Date:** 2026-06-08
**Branches touched:**
- `finish-arch-overhaul` → folded to `master` (`44fef2f`, `4c27c77`) — fmt/clippy + CI quality gate + doc corrections.
- `phase-b` → folded to `master` (`9e8f3ce`) — 5c shared rope + 8b/10 status resolution.

Both branches fast-forward-merged to `master` and deleted; their worktrees removed.

## Built (with status)

### Quality gate (`44fef2f`, `4c27c77`)
- **`cargo fmt --all`** — first tree-wide format (the tree was never fully
  fmt-clean, which is why the gate was off). Dominated the diff.
- **Cleared all clippy warnings** (191 → 0). Mechanical fixes where safe
  (doc-comment formatting, `field_reassign_with_default`, `.min().max()`→`.clamp()`,
  needless-range-loop, deprecated `try_next`→`try_recv`, `&mut Vec`→`&mut [_]`);
  targeted `#[allow]` with reasons for judgment calls (`too_many_arguments`,
  `large_enum_variant` on wire/event enums, `type_complexity`, intentionally-dead
  buffer-pool accessors).
- **Enabled the `quality` CI job** in `.github/workflows/ci.yml`
  (`cargo fmt --all --check` + `cargo clippy --all-targets --features test-support -D warnings`).
- **Doc corrections** — the state-first overhaul was NOT `NEEDS-DECISION`: Phase A
  is essentially complete and decisions D1–D6 are written (ADRs 0006–0011). Flipped
  the stale `[~]` markers in `spec-state-architecture.md`.
- Verified: clippy `-D` exit 0, fmt `--check` exit 0, full suite green.

### Phase B — 5c Doc/Edit shared rope (`9e8f3ce`) — LANDED
- Investigation found the foundation was **already live** (not a greenfield
  49-site rewrite): `DocState.source`/`DocSource`/`SharedEditor`,
  `open_and_retain` dedup-by-canonical-path, `refresh_blocks` per-frame re-derive
  on `edit_seq`; open/split/restore already bind the pooled core → Doc+Edit and
  splits of one file share a rope with unified undo.
- **Fixed the remaining real defect:** `re_render_layout_docs` (theme switch) read
  from **disk**, silently reverting unsaved shared-core edits — and because
  `rendered_seq` didn't advance, the per-frame `refresh_blocks` couldn't
  self-correct. New `re_render_one_doc` sources the live core (stamping
  `rendered_seq`); disk only for string-backed Docs (`source == None`).
- Wired the buffer pool (folds in `ff-buffer-pool`); refreshed the now-false
  "intentionally unwired" comments + the module scope note.
- **Headless tests added (both pass):**
  - `workspace::pool_dedups_by_path_so_two_views_share_one_core` — same path → one
    core; edit via one handle is live via the other; undo via either is unified.
  - `re_render_one_doc_sources_live_core_not_disk` — theme re-render reflects the
    live core (3 blocks), not disk (1), and advances `rendered_seq`.
- Verified: clippy `-D` exit 0, fmt `--check` exit 0, full suite **522 passed / 0 failed**.

## Open / unresolved

- **5c cross-pane paint** — `NEEDS-RUNTIME`. The model/pool seam is proven
  headlessly; the *visible* per-frame update of two simultaneous panes (Edit pane
  keystroke → Doc pane re-renders live) needs a GPUI eyeball. Runtime checklist:
  `docs/runtime-checks/5c-shared-rope.md`.
- **8b delete turn-end inference** — `HELD BY DESIGN` (not by decision). The
  architectural goal is already met by the phase-8 `AgentEvent` stream
  (sourced-once boundary, total reducer, `(generation,turn)` exactly-once ledger;
  agreement pinned by `agent_stream_agrees_*`). Deleting the legacy inference is
  the **content-application cutover** (the double-render risk the §9 gate prevents)
  and making the worker emit unconditionally would inject `TurnEnded` into the
  durable `event_log`/WAL (server records every reply) — both perturb
  freshly-stabilized machinery. Runtime+soak-gated; left as-is.
- **10 `Arc<Core>` swap-in-place** — `DEFERRED` per ADR-0008 (HIGH risk, rare path,
  trigger "stranded-handle vanish after reconnect" not fired). A recorded
  non-goal, not unfinished work. The decided D3 scope (surface re-attach failures
  via `spawn_attach_sessions`) is already in the tree.
- **`ChannelAttachState` faithful enum** — still held; refactors the live
  reconnect-storm path, stabilize that first.

## Decisions
- No new ADRs this session. Confirmed/leaned on existing ones: ADR-0006 (D1
  turn-end), ADR-0007 (D2 doc/edit rope — 5c), ADR-0008 (D3 reconnect — 10's
  swap-deferral). The 8b "held by design" and 10 "swap is a non-goal"
  determinations are recorded in the spec §6/§7 and backlog, pointing at
  ADR-0006 / ADR-0008.

## Verification status
- **Headless/CI-green:** quality gate (clippy `-D` + fmt), full suite 522/0,
  the two new 5c seam tests, the existing `agent_stream_agrees_*` (8b agreement).
- **Needs-human (GPUI not headlessly drivable):** 5c cross-pane paint (see
  checklist); plus the standing phase-8 NEEDS-RUNTIME items (spinner-clears paint,
  App-Nap eviction reconnect) carried from the prior session.
- The verification-harness gap remains the top backlog item — it's what would turn
  all of the above NEEDS-RUNTIME items into machine-checked ones.

## Next
- Run the 5c cross-pane paint check (`docs/runtime-checks/5c-shared-rope.md`) on
  the next GPUI launch; if clean, drop 5c's NEEDS-RUNTIME flag.
- 8b/10 remainder: only revisit when (a) the verification harness can drive a real
  session soak (8b), or (b) the ADR-0008 stranded-handle trigger actually fires (10).
- Verification harness (top priority) — would unblock the whole NEEDS-RUNTIME pile.
