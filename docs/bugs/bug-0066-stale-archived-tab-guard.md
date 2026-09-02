# bug-0066 — stale archived-tab guard red on main

**Status:** FIXED
**Surface:** `verify_harness.rs::archived_waiting_session_is_removed_from_the_painted_waiting_tab`
**First seen:** 2026-09-01 (during the agent-header-redesign session's full-suite run on main)

## Symptom

`cargo test --bin yalda-gpui` red on main: the guard's final assertion —
"the archived row remains available in Archived" — panics. Every other
assertion in the test (real right-click → archive → durable flag → row leaves
the painted Waiting projection) still passes.

## Log

- **2026-09-01 — diagnosed + fixed (graph 78p).** Bisected `b5b9386..15d780c`
  (7 steps): first bad commit is `5607ce1` "Declutter Jump panel and segment
  Cog completion" (graph `k2z`). That change *intentionally* removed the
  Waiting/Working/All/Archived segmented widget and made the production
  sidebar paint the All projection unconditionally
  (`jump_panel_view.rs::render_jump_panel` → `jump_panel_sections_with_tab(cx,
  Some(JumpAgentTab::All))`), spec'd as `UXI-JumpPanel-32` ("Archived-only
  browsing leaves the Jump panel"). The k2z sweep updated 171 harness lines
  but missed this guard's tail, leaving a test that asserts the pre-declutter
  contract. NOT a product regression — a guard asserting a superseded spec.

  **Fix:** reconciled the guard tail with `UXI-JumpPanel-32`: after archiving,
  (1) the painted sidebar shows NO row even when a compatibility caller
  selects `JumpAgentTab::Archived` (probe `jump-session-row-0` is `None`), and
  (2) the internal Archived projection (`jump_panel_sections_with_tab(…,
  Some(Archived))`, kept for Cmd-P/compat) still carries exactly one row with
  the durable `archived` flag — archive hides, never loses.

  **Negative controls:** the old tail assertion red on main was the observed
  RED for the paint flip; the projection assert was flipped to
  `Some(JumpAgentTab::All)` → observed RED ("retains the session: []"),
  restored → green. Full suite 766/766 green in the worktree.

## Lesson

A spec-superseding UX change must sweep ALL guards that painted the removed
surface, not just the ones in the files it edits heavily; grep the harness for
the retired widget's probes/tabs (`JumpAgentTab::`) before closing the graph.
