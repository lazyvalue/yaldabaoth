# Worklog: autonomous Phase A run (overlay enum → settings → cwd key)

**Date:** 2026-06-05 (continuation; user stepped away, "work through Phase A and B")
**Branch:** `master` — `dc50716` → `5a75d58`, four CI-green fast-forward merges.

## Built (all CI-green, worktree → ff-merge → backlog updated)

- **A.2 — `ActiveOverlay` enum** (`e5be921`). Five mutually-exclusive overlay
  `Option`s (menu/buffer/session/workspace/rename) → one `active_overlay` field +
  per-variant accessors + `open_overlay`(replaces)/`clear_overlay`/`has_overlay`.
  ~65 sites migrated (workflow-mapped + 3-lens adversarial review caught a
  borrow blocker — 4 openers hoist a live `&self` read — and the menu Esc
  let-block). `transient_status`/`splash_until` left separate. Test
  `active_overlay_open_replaces_and_clears`. ⚠️ one intentional strictly-better
  divergence (rename-behind-menu no longer strands the menu) — runtime eyeball.

- **A.3 — settings: persist text zoom + `save_settings()`** (`e66a54c`).
  `text_scale` now persists across launches (restored clamped on startup). Two
  hand-rebuilt `save_preferences{..}` sites → one snapshot method. Fonts not
  persisted (no setter yet). Round-trip + forward-compat test. ⚠️ relaunch-zoom
  runtime check owed.

- **A.4 — canonical cwd key (D5/ADR-0010)** (`c46f023`). `persist_cwd_key()`
  (canonicalize, raw fallback) routed through all 4 on-disk cwd-keyed sites
  (workspace + ACP-session save/load/remove); loads do lazy fallback-read; saves
  drop the old raw spelling. Fixes the silent symlink/`/tmp` resume-miss.
  save-all-tabs was already done. Symlink round-trip test.

(Earlier same day: A.1 fully done — worksheet dedup `a6f2829`, pipelined-crash
fix `50021fc`, `replay_turns` field-ownership `6168157`; plus mutex-poison
`d4cce77`. See `2026-06-05-keymap-worksheet-pipeline-mutex.md`.)

## Deferred (with reasons — NOT skipped silently)

- **A.5a `buffer_pool`** — the pool is **dead/unwired code** (workspace.rs
  `buffer`/`buffer_mut`/`buffer_release` warn unused; see memory
  `project_buffer_pool_unwired.md`). Extraction is entangled with D2 shared-rope
  + the shared-doc-multi-membership feature; not a clean independent pure
  extraction. Do it WITH D2, not before.
- **A.5b `DocState.blocks` auto-derive** — the high-value half (memoize on a
  version stamp, no manual snapshot invalidation) is **already done** (quick-win
  2, `blocks_seq`/`blocks_snapshot`/`blocks_rc`). The remaining "own a `Document`,
  derive blocks from its `edit_seq`" is a murky restructure (DocState holds
  rendered blocks, not the source Document) with unclear payoff. Defer.

- **A.11 — `InputSurface` enum** (`761dfe6`). `input_mode: InputMode` +
  `chatbox: Option<Chatbox>` → one `input_surface: InputSurface { Worksheet,
  Chatbox(Chatbox) }`; old `InputMode`→`InputModeKind` (Copy discriminant) kept
  only for the persisted mode string + `should_follow_tail`. ~38 live sites
  migrated compiler-driven; persistence format byte-identical. Workflow
  design+review caught the missed harness site + the `ToggleAgentInputMode`
  substring-collision trap. Test `input_surface_toggle_round_trips`.

## Stopping point (context budget) — next session starts here

Completed this run: **A.1 (full), mutex-poison, A.2, A.3, A.4, A.11** — every
high-value, headlessly-verifiable, illegal-states / persistence / robustness
item in Phase A. `master` @ `8cc54b7`.

**Remaining (deliberately NOT rushed under low context):**
- **A.6 `tool_calls`**, **A.7 `agent_view_model`** — memoization-module
  extractions, each ~A.2-sized blast radius. Do each in its own worktree with a
  map→design workflow (the pattern that worked for A.2/A.11). Pure, CI-gated.
- **A.8a additive `TurnEnded`** (emit the explicit event from the worker without
  yet deleting inference) → **A.9 server fusions** — medium.
- **Phase B (behavior-changing — REQUIRE human runtime check, can't verify
  headless):** D2 Doc/Edit shared rope (5c), 8b delete turn-end inference.
  **D3 skipped** (explicit trigger-deferral). A.5a buffer_pool belongs with D2.
- **A.5a/A.5b** deferred (see above). **clippy (164) + fmt (531)** — dedicated
  passes only.

**Owed runtime checks (GUI not headless-drivable):** pipelined worksheet turns
render separately · worksheet send-failure keeps lines editable · A.2
rename-behind-menu divergence · A.3 relaunch zoom restore · A.11 chatbox/worksheet
toggle + restore still behave.

## Runtime checks owed (GUI not headless-drivable)
pipelined worksheet turns render separately · worksheet send-failure keeps lines
editable · A.2 rename-behind-menu divergence · A.3 relaunch zoom restore.
