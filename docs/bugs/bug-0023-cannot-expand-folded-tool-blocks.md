# bug-0023: cannot-expand-folded-tool-blocks

**Status:** FIXED
**First seen:** 2026-07-26
**Component:** docs/components/agent-tile/transcript.md (UXI-AgentTile-29)

## Symptom

In an agent session's worksheet/transcript, a folded tool-use block (the `▶ ● bash
…` group header) could no longer be expanded. Clicking the header did nothing — the
arrow stayed `▶` and no body appeared. It used to work.

Reported alongside a UX ask, implemented in the same change: `j`/`k` navigation in
the transcript should HOP OVER a tool-use block instead of resting on it.

## Context / root cause

**Confirmed, on the real path.** `render_agent` keys the cached transcript's wrapper
element id on the render fingerprint — `div().id(("transcript-fp", fp))` — as the
dropped-self-notify backstop (UXI-AgentTile-7). That id is an ANCESTOR of everything
in the transcript, so a moved fingerprint re-keys every descendant's
`GlobalElementId` and gpui drops their element state.

`TranscriptSeqs` includes `cursor_line`, `cursor_col` and `transcript_focused`. The
transcript's own select-to-clipboard gesture (`#claude-body`'s `on_mouse_down` →
`transcript_mouse_down`) sets all three on the press. gpui's `on_click` is
down-then-up on the same hitbox: the down stores `pending_mouse_down` in the header's
element state, the frame between down and up re-keys that state away, and the up
finds nothing → no click, ever. (Measured in the harness: fp `11959100105169333304`
→ `2842411632126244694` across the press; the header's painted rect did NOT move, so
it was not a bug-0015-style reflow.)

This killed every `on_click` inside the transcript, not just the fold header.

`transcript_021_tool_expand_busts_cache` stayed green throughout because it
hand-calls `toggle_expanded` — the proxy-state trap of anti-circling rule 1.

## Planned solution

Freeze the fingerprint used for the element id at its pre-press value for the
duration of the mouse gesture — the same gesture-scoped stability bug-0015's
`drag_protect_line` gives the flat-item count. The self-notify path still
invalidates during the gesture, so only the rare dropped-notify backstop is deferred
(by one press).

## Approaches already tried (do NOT repeat)

- <none — fixed on the first attempt; do not "fix" this by removing the fingerprint
  from the element id, which reopens UXI-AgentTile-7's stale-tail class.>

---

## Log

### 2026-07-26 — root-caused + fixed (element-id freeze), plus the `j`/`k` hop

**What changed**

- `transcript_view.rs`: new `TranscriptView::element_fp_freeze` + `element_fp(live)`.
  Set in `transcript_mouse_down` to the PRE-press `TranscriptSeqs::…fingerprint_hash()`,
  cleared unconditionally at the top of `transcript_mouse_up` (so a press that never
  became a drag, or whose up lands outside, can't strand the freeze).
- `screens.rs` `render_agent`: the wrapper id now uses
  `transcript_view.read(cx).element_fp(live_fp)` instead of the live fingerprint.
- `agent.rs`: `AgentState::tool_anchor_lines` / `hop_cursor_over_tool_anchors` + the
  pure `hop_over_tool_anchors`; `agent_ui.rs` calls the hop after
  `dispatch_normal_core` in the transcript-nav branch (UXI-AgentTile-29 part 2).
- `transcript_view.rs`: `probe_bounds_dyn("tool-group-header-{anchor}")` under
  `cfg(test)` so the harness can click the header's REAL rect.
- `docs/components/agent-tile/transcript.md`: UXI-AgentTile-29 added; the
  UXI-AgentTile-7 mechanism reconciled with the freeze exception.

**How verified**

- `verify_harness.rs::tool_group_header_click_expands_the_fold` — probes the header's
  real painted rect, `simulate_click`s it through the window's real mouse dispatch,
  asserts `tools.expanded` flips; also asserts the press really moves the fingerprint
  so the guard can't pass vacuously.
  **Negative control observed RED** (`let transcript_fp = live_fp;`): "clicking the
  folded tool-use header did NOTHING — it never expanded (bug-0023)".
- `verify_harness.rs::transcript_jk_hops_over_tool_blocks` — real
  `handle_claude_key("j"/"k")` over a two-anchor run.
  **Negative control observed RED** (drop the `hop_cursor_over_tool_anchors` call):
  "j must HOP OVER the tool block … (landed 1, anchors {1, 2})".
- Full suite: `cargo test --bin yalda-gpui` → 476 passed, 0 failed.

**Outcome**

Fixed. Runtime-unverified by a human as of this entry — the click path is guarded at
the real painted rect through real mouse dispatch, but exact pixels/hover feedback is
harness gap #1. The user must restart the rebuilt binary to get it.
