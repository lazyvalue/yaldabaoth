# Worklog: agent context-window progress bar

**Date:** 2026-06-12
**Branch:** agent-context-bar — untitled.md Agent UX "Context Window full
progress bar + indicator of how many tokens used".

## Root cause (why there was no visible bar)

A context-usage bar + numeric already existed in the agent **header strip**
(`screens.rs`), but it renders only when `AgentState.usage` is `Some`. `usage`
is populated by `ReplyEvent::UsageUpdated`, whose **emitter** in
`acp_channel.rs` is gated behind the `unstable_session_usage` Cargo feature —
which was **off by default**. So in every normal `cargo run` build the usage
data never flowed and the bar never appeared.

Worse, the feature didn't even compile: upstream `agent-client-protocol` 0.11
renamed the cost field, so the gated emitter referenced `Cost::amount_usd`
which no longer exists (`available fields are: amount, currency`). The feature
had bit-rotted, which is presumably why it was left off.

## Built (with status)

- **Fixed the bit-rotted emitter** — `acp_channel.rs` now reads `Cost::amount`
  and only surfaces it as `cost_usd` when `Cost::currency` is USD (other
  currencies are dropped rather than mislabeled, since `UsageSnapshot.cost_usd`
  is USD).

- **Enabled `unstable_session_usage` by default** — added
  `default = ["unstable_session_usage"]` to `[features]` so the
  `ReplyEvent::UsageUpdated` emitter actually fires and the agent view's
  context indicator has data without a special build flag. The variant stays
  unconditional, so non-default builds are unaffected.

- **Prominent context bar in the agent info bar** — the dedicated agent status
  bar's `ctx` segment was text-only (`12.3k / 200k (6%)`); it now renders a
  proportional fullness bar (`ctx [▓▓▓░░] 12.3k / 200k (6%)`) that shifts to the
  warm accent at ≥85% full, with the numeric reading beside it. Computed from
  `usage.tokens_used / tokens_total`, clamped to `[0,1]`; absent data → no bar +
  em dash, exactly as before. The compact header-strip indicator is left in
  place (it also carries cost).

## Verification

- `cargo build` (all bins, default features) clean; lib + gpui + session-server
  + channel all compile with the feature on.
- `cargo test --lib` → 136 passed; `cargo test --bin yalda-gpui` → 187 passed.
- **Runtime check owed** (no headless paint): that the bar renders + fills, and
  — the real unknown — that the connected ACP agent actually emits
  `session/update` usage notifications. If it doesn't, the bar shows `—` and the
  data side needs a separate look (the UI is correct regardless).

## Decision flagged

Turning an upstream **unstable** feature on by default is a real call (it's why
the feature existed as opt-in). Justification: the agent view's context
indicator has no other data source, and the variant is unconditional so the
blast radius is just the one emitter. Worth an ADR if we want it recorded
durably — not yet written.

## Artifacts

- `untitled.md`: Agent UX context-bar bullet ticked with the root-cause note.
- Worktree `agent-context-bar` ready to integrate.
