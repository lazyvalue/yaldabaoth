# UXP-2 — A keystroke routed to an input is painted (routing ⇒ painting)

**Statement.** If a keystroke is delivered to an input surface (it mutates that
surface's buffer), the result is visible on the next frame. The predicate that
decides whether a surface *renders* is derived from the same fact that decides
whether it *receives input* — never from a parallel flag that can disagree.

**Applies to.** Every input surface and its render gate: key routing
(`handle_claude_key` and the per-screen key handlers) and the render predicate of
the surface it routes to.

**Why.** The `/clear` "can't type" saga (legacy INV-UX-16;
`docs/bugs/saga-clear-worksheet-invisible/`): input landed in a buffer whose render
gate was computed separately, so typing was invisible.

**Status.** `implemented` for the agent compose; any new input surface must derive
its render gate from its routing.

**Enforcement.** Drive the REAL key path, then assert paint:
`verify_harness.rs::clear_worksheet_hole_types_and_paints`,
`verify_harness.rs::worksheet_inline_typing_rerenders_you_block_not_transcript`
(since D11 the inline block paints as its own cached overlay, `YouBlockView`).

**Realized by.** `UXI-AgentTile-12`.
