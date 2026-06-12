# Project: GPUI responsiveness — "super fast GUI"

Umbrella project to make `yalda-gpui` consistently fast on the hot path
(typing, scrolling, dragging). Born from a whole-surface multi-agent audit
(20 verified findings) — see `audit-report.md` (ranked roadmap) and
`audit-findings.json` (raw verified findings with file:line + verdicts).

## Root cause (the one model every ticket assumes)

`YaldaGpuiView` is the **only** GPUI entity, so **any `cx.notify()` dirties the
root and re-runs the entire render + layout tree — there is no subtree skip.**
Every "typing/interacting in surface A re-lays-out unrelated subtree B" symptom
(chatbox→transcript, overlay→transcript, worksheet→compose+strip, drag→all
desktop tiles) is this one cause. Prior virtualization + S1 memoization capped
most costs at O(visible), so they're bounded — but still fire on the most-touched
surface (typing in the Message Box).

### GPUI 0.2.2 facts (verified against framework source — load-bearing)

- Root view ALWAYS re-renders on any notify (`window.rs` `draw_roots` → root
  `render()` every frame). You cannot make the root skip.
- **`AnyView::cached(style)` is the ONLY render-skip lever** (`view.rs:103`; no
  element-level cache). A child embedded as `child.into_any().cached(style)` has
  its `render()` skipped + prepaint/paint reused when its entity-id is NOT in
  `window.dirty_views` AND bounds/content_mask/text_style are unchanged. The
  slot is sized from the *style*, so it must carry `size_full`/`flex_1`.
- ⇒ Durable fix for "A re-lays-out B": make B a cached child Entity, notify it
  only when B's own render-fingerprint changes. "Split the compose into its own
  entity" (an earlier framing) was backwards — the root holding the transcript
  still re-renders; it's the **transcript** that must become the cached child.

## The shared helper (the keystone — answer to "what else benefits")

Findings 1, 7, 8, 9, 14 all want ONE mechanism. Build it once:
`src/bin/yalda-gpui/cached_panel.rs` — `FingerprintedPanel: Render { fn render_fp(&self) -> u64 }`
+ `CachedPanel<V>` that (a) embeds `view.into_any().cached(size_full)` and
(b) `notify_if_changed` notifies the child only when its fingerprint moved.
Consumers: transcript (flagship), compose/Message-Box, status strip, thinking
indicator, then every split/desktop leaf. See `audit-report.md` §3 for the API.

Orthogonal fixes (do NOT use the helper): blocking I/O off paint thread
(clipboard `pbpaste`/`pbcopy` → in-process; browser recursive `fs` walk →
debounce + background executor); local memo guards; list virtualization.

## Phases / tickets

| #   | Ticket                                                        | Phase | Risk | Status |
|-----|---------------------------------------------------------------|-------|------|--------|
| 010 | Cheap wins: cwd OnceLock + dup frozen to_vec dedup + thinking-tick gate (compose lines_cache dropped — already covered by shipped virtualization) | 0 | low | **done** → `perf-phase01` |
| 011 | Clipboard in-process for the 4 main.rs handlers (#4/#5) | 1 | low | **done** → `perf-phase01` |
| 012 | Deferred follow-ups: WP-classify cache; browser filter debounce + bg walk (#2); remaining vim yank/put clipboard paths | 0/1 | low-med | todo (`012-ticket-deferred.md`) |
| 020 | `CachedPanel`/`FingerprintedPanel` helper + headless render-skip proof test | 2 | med | **done** → `transcript-entity` |
| 021 | Transcript → cached child entity (flagship; closes #1) — needs runtime profile | 2 | med-high | **next** (`021-ticket-transcript-entity.md`) |
| 022 | Compose/Message-Box → separate cached child (closes #1/#7) | 2 | med | todo |
| 023 | Status strip + thinking indicator → cached children (#14, #7) | 2 | low-med | todo |
| 030 | Generalize: each split/desktop leaf a cached child (#8, #9 sibling); resize coalesce | 3 | high | todo |
| 040 | Opportunistic: expanded tool-group virtualization (#3), agent-picker window (#11), workspace-save off-thread (#15) | 4 | low | todo |

Effort/leverage detail and exact file:line per ticket are in `audit-report.md`.

### Branch state (uncommitted, nothing on `main`)

- `perf-phase01` — tickets 010 + 011. Builds; 174 tests pass.
- `transcript-entity` — ticket 020 keystone. Builds; proof test passes. Ticket
  021 (transcript extraction) builds on this branch, consuming `CachedPanel`.
- Both held uncommitted pending owner OK. Phase 0/1 needs a runtime copy/paste
  check; clipboard/thinking-gate flagged for human verification.

## Verification

- Headless (`verify_harness.rs`): per-keystroke surface rebuilds its static
  neighbor **0 times** (build on `VIEW_MODEL_REBUILDS` counter idiom +
  `edit_view_keystroke_is_o_changed`). The helper ships with a render-skip test.
- Runtime (human, cannot self-verify): `sample` the live process while typing in
  a large transcript; confirm no per-keystroke transcript layout. GUI can't be
  driven headlessly for paint (per dev-system).

## Links

- `audit-report.md`, `audit-findings.json` (this dir)
- ADR-0019 (Tiles & Apps), `spec-agent-session-ownership.md`
- Memory: input-latency-profiling, durable-architecture
- Worktree: `.claude/worktrees/transcript-entity` (Phase 2 flagship)
