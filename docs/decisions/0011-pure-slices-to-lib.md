# ADR-0011: Pure agent slices → lib; concrete line-turn map (D6)

**Status:** Accepted
**Date:** 2026-06-05
**Related:** spec-state-architecture.md (D6), agent_transcript.rs + ReplayTurns (the exemplars), editor_core

## Context

(a) The pure agent logic — `tool_calls`, the pure flat-build of
`agent_view_model`, `highlight_cache` — lives in the 17.5k-line `main.rs`, where
it **cannot be unit-tested without the GPUI harness.** That is the constant-
regression root, stated as a location. (`replay_turns` and `user_turn_reconciler`
already live in the lib and *do* have real tests — the model to follow.)
(b) `EditorCore.line_metadata` is a type-erased (`Any`) store holding only
`TurnId`, erased **only because** `TurnId` lives in the bin and the lib editor
can't name it. Erasure carries a footgun: `metadata_mut::<WrongType>()` silently
returns an empty store (no compile error).

## Decision

(a) Move the **genuinely GPUI-free** agent slices — plus `TurnId` and the turn
semantics — into the **lib crate** (unit-testable headlessly, reusable by the
TUI). Leave the GPUI-bound parts (`follow_tail`'s `ListState`,
`agent_view_model`'s per-item element build) in the bin; this is "extract the
pure core, leave the thin wrapper," not "move the whole thing."
(b) With `TurnId` in the lib, **collapse the type-erased `line_metadata` to a
concrete `line_turn: HashMap<LineAnchor, TurnId>`.** Add a named field for a
second per-line metadata type (diagnostics/comments) only if/when one actually
arrives — not speculatively.

## Rationale

(a) is *the* lever for the regression-prevention loop: pure logic in the lib =
cheap headless tests apply to it. (b) kills the silent-wrong-type-param footgun
(make illegal states unrepresentable); YAGNI on speculative erasure.

## Consequences

- Widens the lib API surface (acceptable; it's the testable surface).
- Unblocks migration steps 6/7; unifies the mirror `TurnId` enums currently
  redeclared in the editor/acp tests.
