# Worklog: Worksheet reply first-paint wrapping (bug-0040)

**Date:** 2026-08-17
**Branches touched:** `fix-worksheet-reply-wrap` (`b95eba3`), fast-forwarded
to `main` (`b95eba3`); release binary rebuilt.

## Cog execution evidence

- Graph id: `s8w`
- Graph name: `fix-worksheet-reply-wrap`

### Initial render

Shown to the user before tracked-file edits:

```text
graph fix-worksheet-reply-wrap (frontiers)
frontier 0: reproduce [open]
frontier 1: fix [open]
frontier 2: verify [open]
frontier 3: finalize [open]
frontier 4: omega [open] (omega)
```

### Node execution

Each node was claimed before its work and closed only after its acceptance
evidence was available:

- `jrj6` `reproduce`: claimed → closed `done`; output: real `r` dispatch plus
  first-paint layout probe reproduced the defect RED at a 1574px transcript /
  142px-tall reply; root cause was the unmeasured 40-column fallback.
- `zi49` `fix`: claimed → closed `done`; output:
  `inline_you_block_wrap_cols` makes exact compose measurement win, otherwise
  derives first-paint columns from the real transcript viewport; UX and bug
  records updated; focused guards GREEN.
- `vcqr` `verify`: claimed → closed `done`; output: restored-40 negative
  control RED for the reported reason, full GUI suite 572 pass / 2 ignored,
  12/12 unique helper mutants caught, `git diff --check` clean.
- `z0nk` `finalize`: claimed → closed `done`; output: commit `b95eba3`,
  fast-forwarded to `main`; full main suite GREEN; release build GREEN.
- `ci5b` `omega`: claimed → closed `done`; output: aggregate fix, test,
  mutation, integration, release, and artifact evidence.

### Notes

- Node `vcqr`, seq `4`, topic `deviation`: the first mutation run was
  sandbox-unviable because GPUI's Metal build could not write clang's module
  cache. The permitted rerun was viable (11 caught / 1 missed); the missed
  `> 1.0` boundary mutant caused an exact one-pixel sentinel assertion to be
  added, after which all 12 unique mutants were caught.
- Node `z0nk`, seq `2`, topic `deviation`: final status and an omega-done render
  do not exist until omega closes, so this durable worklog was deliberately
  written and validated immediately post-omega rather than fabricating final
  evidence inside the predecessor node.

### Final status

- Status: `complete`

```text
graph fix-worksheet-reply-wrap (frontiers)
frontier 0: reproduce [done]
frontier 1: fix [done]
frontier 2: verify [done]
frontier 3: finalize [done]
frontier 4: omega [done] (omega)
```

## Built (with status)

- Fixed the intermittent narrow wrapping of newly seeded `r` replies. Before
  the inline You-block has its own painted bounds, wrapping now uses the
  already-painted transcript viewport minus conservative inline chrome.
- Preserved exact compose-bound precedence after measurement, shared the
  resolved width with caret-reveal math, and left intentional blockquote italics
  unchanged.
- Added painted real-path regression coverage, exact pure width-selection
  coverage, bug-0040 + manifest entry, and UXI-AgentTile-9 enforcement evidence.
- Shipped on `main` at `b95eba3`; `target/release/yalda-gpui` rebuilt.

## Open / unresolved

- The running GUI process must be restarted to load the rebuilt release binary.
- Exact glyph appearance is still the normal harness gap #1; the faulty wrapping
  itself is verified through painted geometry and is not runtime-only.

## Decisions

- No ADR. This is a localized correction under existing UXI-AgentTile-9 and
  UXI-Blockquote-1, not an architectural change.

## Verification status

- Negative control: forcing the unmeasured branch back to `40` makes
  `worksheet_r_first_paint_uses_transcript_width` fail at 1574px / 142px.
- Focused painted guard and pure helper guard pass with the fix.
- `cargo test --bin yalda-gpui` on the branch and again on `main`: 572 passed,
  0 failed, 2 ignored.
- Targeted mutation gate: 12/12 unique `inline_you_block_wrap_cols` mutants
  caught.
- `cargo build --release --bin yalda-gpui` on `main`: pass.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-17-worksheet-reply-wrap.md`
  passes.

## Next

- Restart the running GUI and confirm the next long `r` reply uses the full
  worksheet column immediately.
