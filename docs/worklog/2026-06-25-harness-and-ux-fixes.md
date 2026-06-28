# 2026-06-25 — Verification harness + UX regression fixes + subagent features

A long session driving toward stopping the recurring-regression cycle, then
clearing the open UX work. Everything below is on `main`, builds, tests green.

## Verification harness (the meta-fix)

- **#3.2 painted-bounds layout probe** (`ef0a599`). `probe_bounds` /
  `layout_probe_*` (render_blocks.rs) record an element's PAINTED bounds during
  the real `run_until_parked` paint pass, readable by a `#[gpui::test]`. First use:
  `compose_caret_row_painted_inside_box_when_wrapped` proves the caret row is
  painted inside the compose box. Validated by injecting the regression → fails.
  This closes "GPUI paint can't be asserted headlessly" for the caret/visibility
  class. Reusable for any painted-geometry assertion.
- **#3.3 release perf gate** (`4056b97`). `benches/render_bench.rs` (criterion)
  over render + highlight; `cargo bench --bench render_bench`.
- **#3.1 in-process GUI↔server↔agent loop** — still scoped only
  (`docs/projects/headless-e2e/`), the remaining gap.
- The PreToolUse hook (`.claude/settings.json`) injects the UX-invariants contract
  on every edit to a UX-bearing file; `ux-invariants.md` is the contract.

## UX regressions fixed (each pinned by a headless guard that fails without the fix)

- **Caret below the fold under word-wrap** (`6265a2d`, INV-UX-1). The wrap change
  computed the compose's vertical window over logical lines; once lines wrap a
  logical line spans multiple visual rows. Fixed to compute in visual-row space
  (`compose_visual_metrics` / `compose_item_for_visual_row`). Guard
  `compose_wrapped_caret_never_below_the_fold` + the painted-bounds proof.
- **Agent text on a tinted card** (`dfa8d90`, INV-UX-3). Dropped the per-turn
  `claude_turn_bg`/`user_turn_bg`; agent text sits on the tile background. The
  focus-row highlight (transcript-focus only) stays.
- **"Blank turns"** (`7d7cc9e`, INV-UX-4). The transcript emitted empty alternating
  `You`/`Claude` dividers (blank-tagged separator / resume-artifact lines minting
  phantom headers). `rebuild_agent_view_model` now drops any TurnHeader with no
  content before the next header. Guard `rebuild_drops_empty_turn_headers`.

## Subagent features (`896d11b`, INV-UX-5)

- **#1 Harness detection.** `classify_subagent` keyed on `kind == ToolKind::Other`,
  but claude-code-acp maps `Task` → `ToolKind::Think` — so real subagents were
  NEVER detected. Rewrote it to detect the structured emission (Think + `prompt`/
  `subagent_type`, excluding TodoWrite's `todos`) + a name fallback; captures the
  prompt. Guards `classify_subagent_detects_the_harness_task_shape` +
  `subagents_surfaces_registered_task_with_prompt`.
- **#2 Bottom panes.** Replaced the right Subagents sidebar with a strip of small
  panes below the compose; each shows status + label + spawn prompt; click focuses
  the subagent (transcript shows its output). Auto-shown; Cmd-2 collapses.

## Other fixes this session

- Worksheet **Model C** ordering (ADR-0024), the leader-key regression, compose
  word-wrap (INV-UX-2), the adapter zombie reaper, WP-classify cache, theme-cache
  (already fixed) + reparse (confirmed unused) backlog corrections.

## Runtime-unverified (GPUI paint can't be fully driven headlessly) — human check

- Subagent panes render below the compose + click-to-focus shows output.
- Caret visible while typing in worksheet + chatbox (model + paint tested, but a
  live eyeball is still worth it).
- Agent text reads on the tile background; no blank turns in a real long session.

## Open

- #3.1 in-process e2e loop (scoped). The markdown "blank turns" report turned out
  to be INV-UX-4 (empty headers), fixed — no separate markdown bug found.
