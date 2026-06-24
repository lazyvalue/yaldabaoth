# Worklog: jump panel → universal roster → typed cwd → worksheet resume/undo

**Date:** 2026-06-22
**Branches touched (all merged to `main`, branches deleted, worktrees removed):**
- `jump-panel` (`4df042a`) → merged `e3fa254`
- (jump panel toggle, on `main`) `720b7a0`
- `universal-agent-list` (`cdf5caa` P1, `2102bbc` P2) → merged `4ec7a62`
- `workspace-cwd-type` (`f999a3f`) → merged `1329898`
- `agent-cwd-live` (`f1af55a`, `11554ea`) → merged `e942960`
- `worksheet-resume-fix` (`28a1fce`, `25aad27`) → merged `1560db7`
- `ws-undo` (`4c37480`) → merged `a7beb83`  ← current `main` HEAD
- NOT mine: `ce29894` + merge `0532180` (worksheet draft/divider/ordering +
  caret-to-tail) landed mid-session from outside this conversation.

## Built (with status)

- **Jump panel** — `NEEDS-RUNTIME`. Always-visible root-level left navigator
  (inline render, NOT a cached child — a root-reading cached child double-leases
  + won't dirty here). Sections: Pinned (placeholder) · Workspaces (active
  marked) · Agent sessions. `cmd-j` / `?`-menu toggle, persisted
  (`Preferences::jump_panel_visible`). Free-session select → ephemeral virtual
  workspace (`Tab::ephemeral`, ADR-0021) torn down on switch-away via the one
  `Workspace::set_active_tab` chokepoint. Spec `spec-jump-panel.md`, ADR-0021.
- **Universal agent roster** — `NEEDS-RUNTIME`. One `AgentRoster` (all
  server-known sessions, keyed by sid) seeded by `list_sessions` at boot + kept
  live by the `SessionCreated`/`Closed`/`Renamed` broadcasts (the no-op
  `SessionCreated` hook is now wired); pump starts at boot. Jump panel AND the
  per-tile selector both PROJECT from it (selector reduced to UI-only state,
  derives free/bound at render/select time) — retired the per-tile async
  `list_sessions` + INV-PR `WindowId` routing. Spec `spec-universal-agent-list.md`,
  ADR-0022. 223 gpui tests green at merge.
- **Workspace cwd is a required typed field** — `NEEDS-RUNTIME`. `Tab.kv`
  (cwd was its only consumer) → required, PRIVATE `cwd: WorkspaceCwd`; a
  cwd-less workspace is now a compile error (closed the ephemeral-tab cwd-omit
  regression). `agent_base_cwd` total; persistence migrates legacy `kv["cwd"]`.
  ADR-0023.
- **New agent inherits the LIVE workspace cwd** — `NEEDS-RUNTIME`. The selector
  cached cwd at open, so "open agent → Set CWD → start new" used the stale dir.
  Removed `SessionPicker.cwd`; reads `agent_base_cwd()` live. Verified Set-CWD
  persists across restart (`workspace_cwd_persists_across_restart`).
- **Worksheet resume — three editor fixes** (live report: "can't find cursor /
  undo erased it / tool calls at the bottom" reopening a multiturn worksheet
  session; data was always safe in the server WAL). All headless-tested:
  - **F2 streaming cursor drift** — caret lived on `EditorView` while
    `programmatic_insert` shifted only frozen ranges; now
    `Editor::splice_insert/_delete` shift the caret on every programmatic splice.
  - **C3 undo wipes metadata** — `undo/redo` called `reset_line_anchors`; now
    `Document::undo/redo` return `AnchorShift`s and `apply_anchor_shifts` SHIFTS
    anchors (metadata survives).
  - **Undo erased the whole transcript (THE repro)** — `begin_insert` opens ONE
    undo group; an agent chunk streamed mid-insert recorded INTO it, so one undo
    inverted user text + every agent turn. Fix: programmatic splices are
    non-undoable (`Document::insert_str_at_char_no_undo` / `delete_range_no_undo`
    + `shift_recorded_splices` keeps the user's own splices correct). Test
    `worksheet_resume_undo_does_not_erase_transcript`.

`main` @ `a7beb83`: 141 lib + 238 gpui tests pass; clean build.

## Open / unresolved (see `docs/backlog.md`)
- Worksheet ticket-001 remaining: **floor-only-EOF** (edge: mid-doc draft +
  stream) and **cursor/`view_model_fingerprint`** (perf-sensitive — folding
  cursor busts the flat-list memo per j/k → typing lag in huge worksheets; needs
  the move-passes-out-of-memo refactor + a live `sample`). NOT blind-patched.
- Worksheet **reveal/scroll on resume** — likely resolved by F2 (caret now at the
  right line → `item_for_line` targets correctly) but unverifiable headless.

## Decisions
- ADR-0021: ephemeral virtual workspace (jump-panel free-session display).
- ADR-0022: one universal agent roster, projected by every session list.
- ADR-0023: workspace cwd is a required, private typed field ("no cwd"
  unrepresentable).

## Verification status
- Everything above is builds + headless-tested. **Nothing is runtime-verified**
  — the whole batch needs a rebuild + human pass (GPUI can't be driven headless
  for paint). The jump panel visuals, roster live-updates, cwd inheritance, and
  the worksheet caret/undo behavior on a REAL resumed session are all
  `NEEDS-RUNTIME`.
- Process note (recorded honestly): the "undo erased the buffer" bug took THREE
  attempts — workspace-default cwd, then C3 metadata — before a headless repro of
  the full sequence (insert-open → stream → END insert to commit → undo) found
  the real cause. Reproduce before claiming a fix.

## Next
- Human runtime pass on the worksheet resume fixes (the live complaint) + the
  jump panel / roster / cwd batch.
- If reveal-on-resume still feels off after rebuild, extend the F2 repro toward
  the scroll path.
- Consider the cursor-fingerprint refactor (move cursor-sensitive passes out of
  the memoized flat build) — only with a real perf sample.
