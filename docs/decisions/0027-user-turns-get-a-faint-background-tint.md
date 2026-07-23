# ADR-0027: User turns get a faint background tint (agent turns do not)

**Status:** Accepted
**Date:** 2026-07-22
**Related:** `UXI-AgentTile-4` (no per-turn card background — amended here),
`UXI-AgentTile-23` (the new user-turn tint), `docs/components/agent-tile/transcript.md`,
`src/theme.rs` (`AgentTheme::user_turn_bg`)

## Context

`UXI-AgentTile-4` said the agent transcript has **no per-turn background tint**:
agent and user turns both sit on the plain tile/desktop background, distinguished
only by the gutter label, the foreground author tint, and the left bar. That rule
came from a real problem — a tinted card *per turn* made the transcript read as a
separate surface floating on the desktop instead of blending into the tile — and
the fix removed the then-existing `agent_turn_bg` / `user_turn_bg` tints (set the
row background transparent for every committed turn). The theme fields survived but
went unused.

The user now wants their **own** turns to be "a slightly different color, possibly
a faint blue" — to pick out, at a glance, what *they* said versus what the agent
said in a long transcript. That is a per-turn background tint, which `-4` bans.

## Decision

**Reverse `UXI-AgentTile-4` for user turns only.** Agent turns keep the plain
tile background (the original concern — the *agent's* prose should blend into the
tile — is fully preserved). **User turns** (`TurnId::User`) get a **faint** blue
background band, subtle enough to read as "slightly different," not a floating card.

- `UXI-AgentTile-4` is narrowed to *agent* turns (its statement + tests updated).
- A new `UXI-AgentTile-23` owns the user-turn tint.
- The color reuses the existing per-theme `AgentTheme::user_turn_bg` field
  (previously a warm green, retuned to a faint blue per theme so it stays legible
  on light themes too), rather than one hardcoded blue.

## Why this doesn't reopen the old bug

The `-4` concern was specifically that *agent* text (the bulk of the transcript)
looked like a card floating on the desktop. Agent turns stay card-less, so the
transcript still reads as one continuous surface. The user's turns are a small
fraction of the content, and a *faint* tint is a legibility cue (like the left
bar), not a heavy card. The asymmetry is the point: only the user's contributions
are highlighted.

## Alternatives rejected

- **Keep `-4` unchanged, distinguish user turns some other way.** They already
  have a distinct left bar + `U<n>` gutter label + foreground tint; the user
  looked at that and still wanted a background difference. The bar is too thin to
  scan a long transcript by.
- **Tint agent turns too (restore both `*_turn_bg`).** That is exactly the
  floating-cards problem `-4` fixed; rejected.
- **One hardcoded blue for all themes.** Would glare on dark themes and wash out
  on light ones; the per-theme field lets each theme tune contrast.

## Consequences

- `UXI-AgentTile-4`'s statement and its (runtime) enforcement note now scope to
  agent turns; the transcript row-background selector returns the tint for
  `TurnId::User` lines and transparent otherwise.
- Each theme's `user_turn_bg` is retuned to a faint blue; a future theme must set
  it (it already exists on every theme).
- The exact painted hue remains a human-eye check (harness gap #1), as `-4`
  already was; the *decision* (which turns get a tint) is headless-tested.
