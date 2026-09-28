# Backlog

Only what is waiting on **Scott**: `NEEDS-RUNTIME` (built + headlessly guarded;
awaiting a human check of one of the genuine harness gaps — pixels, the live
agent loop, wall-clock perf, OS key delivery) and `NEEDS-DECISION`. Everything
else lives in Cog: in-flight work in graphs, deferred/unscheduled ideas in
bulletins (see below). Remove an entry once Scott confirms it; a regression
becomes a `/bug`.

Earlier entries (open, deferred, and pre-2026-09 runtime checks presumed
exercised by daily use) were moved to Cog bulletin `yaldabaoth/docs::backlog-archive`
on 2026-09-27 (`cog bulletin get yaldabaoth/docs::backlog-archive`).

## Needs runtime

- **Text-editing code review + fixes** — `NEEDS-DECISION` (built 2026-09-27, Cog
  graph `exa`, branch `text-edit-review`, **not merged** per Scott; see
  [review](research/2026-09-27-text-editing-review.md),
  [worklog](worklog/2026-09-27-text-editing-review.md), UXI-TextEditing-5).
  36 findings fixed (shared `LineInput` for all 14 single-line fields, undo/frozen
  engine bugs, highlight cache O(changed), tab caret, clipboard via GPUI, compose
  hot path, Diff paste). Decision: review + merge to `main`. After merge,
  NEEDS-RUNTIME: gap 1 (caret/font pixels), and the Restart-path draft save order.
  Deferred follow-ups listed in the review's Outcome section (B2/B8/B9/B14, C3,
  C10/D11 cached Edit body + You-block, compose typing not undoable, tab-line
  mouse hit-test, A8 async file-filter search).

- **Diff Review rework (rev 2)** — `NEEDS-RUNTIME` (built 2026-09-27, Cog graph
  `8g7`, branch `diff-rework` → `main`; see
  [worklog](worklog/2026-09-27-diff-review-rework.md), ADR-0040, UXI-Diff-8..17).
  Worktree picker, per-file Viewed, draft comments in
  `<repo>/.yaldabaoth/reviews/<branch>.json`, send-to-any-session picker; merge
  gate + session binding removed. Gap 1: colors/card styling need a human look.
  Gap 2: roster-only session sends are fire-and-forget (late server rejection is
  logged only). Open: long lines truncate (no wrap / h-scroll); comments on a
  file that left the diff aren't shown. Needs a GUI restart by Scott to activate.

- **Config-file-driven Claude model list** — `NEEDS-RUNTIME` (built 2026-09-24,
  Cog graph `2qm`, branch `config-driven-claude-models` → `main`; see
  [worklog](worklog/2026-09-24-config-driven-claude-models.md), UXI-AgentTile-16).
  Adding a Claude picker model is now a one-line edit to
  `~/.config/yalda/claude-models.conf` (or `scripts/yalda-add-claude-model.sh
  <id>`) merged over the compiled defaults — no Rust change, recompile, or
  restart; a new session picks it up. Also upgraded the global `claude-agent-acp`
  0.75.1→0.81.2 and corrected the `7r9` claim: the adapter surfaces unknown ids
  verbatim (passthrough), it does not drop them. Gap 2: the live
  server-reads-conf → spawn-adapter → picker path is human-verified. Rebuild +
  restart the server to run the new lib code (the running server still has the
  `7r9` compiled list, which already includes `claude-opus-5-5`); human check:
  add an id via the script, open a new Claude session, confirm it appears.

- **Claude Opus 5.5 in the model allowlist** — `NEEDS-RUNTIME` (built
  2026-09-22, Cog graph `7r9`, branch `add-opus-5-5-model` →`main`; see
  [worklog](worklog/2026-09-22-opus-5-5-model.md), UXI-AgentTile-16). Added
  `claude-opus-5-5` (Opus 5.5, released 2026-09-22) to
  `YALDA_CLAUDE_AVAILABLE_MODELS` so it is offered in the Claude session model
  picker. Headless test green + negative-controlled. Gap 2: the installed
  `claude-agent-acp` adapter/SDK must recognize the id over the live
  `session/new` / `set_config_option` round-trip — an older adapter build may
  drop it until updated. Rebuild + restart to pick it up; human check: Opus 5.5
  appears in the `space M` / `model ▾` picker and a switch takes effect.

- **Deploy the bug-0064 per-file corrupt-WAL skip** — `NEEDS-RUNTIME`
  (2026-09-09, Cog graph `fgb`; see
  [worklog](worklog/2026-09-09-wal-interior-corruption-skip.md)). On `main`
  (a0af02c), release server binary built; the running systemd service is the
  00:46 PRE-fix binary and will crash-loop again on any future
  interior-corrupt WAL. Scott: `./deploy-server.sh` (restarts the service —
  every attached GUI session reconnects; WAL replay is lossless). Gap 2: the
  fixed binary has not run against the real `~/.yalda/wal`. Afterwards delete
  `~/.yalda/wal-backup-torn-20260909T005200/` (488 MB, the two pre-repair
  files) once satisfied.

- **Agent header redesign: pixel pass** — `NEEDS-RUNTIME` (gap 1,
  pixels/colors; shipped 2026-09-01, graph `y8m`, main e1aa51f). The two-deck
  header's look — dot halo, state-tinted hairline, chip colors, truncation at
  narrow widths — needs Scott's eye after a GUI restart (release binary built;
  process not touched).

## Needs decision

_(none)_
