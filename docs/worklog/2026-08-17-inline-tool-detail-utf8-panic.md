# Worklog: Inline tool-detail UTF-8 panic (bug-0041)

**Date:** 2026-08-17
**Branches touched:** `main` (`91213a7`); release binary rebuilt.

## Cog execution evidence

- Graph id: `8zc`
- Graph name: `fix-tool-inline-utf8-panic`

### Initial render

Shown to the user before tracked-file edits:

```text
graph fix-tool-inline-utf8-panic (frontiers)
frontier 0: localize [open]
frontier 1: fix-and-guard [open]
frontier 2: verify-and-record [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `jear` `localize`: claimed → closed `done`; output: localized both unsafe
  branches in `tool_inline_detail`; the reported `🖼` spans bytes 57–61 across
  the hard byte-60 slice.
- `68t7` `fix-and-guard`: claimed → closed `done`; output: shared
  character-boundary-safe truncation added for command and pattern details; the
  exact reported command plus a Unicode pattern case pass.
- `kj9v` `verify-and-record`: claimed → closed `done`; output: exact negative
  control RED, full GUI suite GREEN, 2/2 targeted mutants caught, bug records
  written, release build passed, and commit `91213a7` landed on `main`.
- `tg6n` `omega`: claimed → closed `done`; output: aggregate root cause, fix,
  verification, release, and record evidence confirmed.

### Notes

- Node `kj9v`, seq `3`, topic `deviation`: the first configured mutation run
  produced eight sandbox-unviable scratch builds. The permitted filesystem
  rerun was narrowed to `truncate_inline_detail` and caught both viable mutants.
- Node `kj9v`, seq `5`, topic `deviation`: final graph status and the omega-done
  render do not exist until omega closes, so this worklog was deliberately
  written and validated immediately post-omega rather than fabricating final
  evidence in its predecessor.

### Final status

- Status: `complete`

```text
graph fix-tool-inline-utf8-panic (frontiers)
frontier 0: localize [done]
frontier 1: fix-and-guard [done]
frontier 2: verify-and-record [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- Replaced byte-indexed inline command and pattern truncation with a shared
  Unicode-scalar boundary lookup. ASCII limits and ellipsis behavior are
  preserved.
- Added the exact crash input and an adjacent search-pattern boundary case to
  the unit suite.
- Added bug-0041 and its manifest entry.
- Landed the fix on `main` at `91213a7`; `target/release/yalda-gpui` rebuilt.

## Open / unresolved

- The crashed/running GUI process must be restarted to load the rebuilt release
  binary.
- No runtime-only verification gap remains for the truncation logic; the helper
  is deterministic and exercised directly on the production path.

## Decisions

- No ADR. This is a localized correctness fix under the existing Agent Tile
  transcript behavior.

## Verification status

- Negative control: restoring byte slicing made the focused guard panic with
  `byte index 60` inside `🖼` at bytes 57–61; restoring the fix passed.
- `cargo test --bin yalda-gpui`: 578 passed, 0 failed, 2 ignored.
- Targeted `cargo mutants`: 2/2 viable helper mutants caught.
- `cargo build --release --bin yalda-gpui`: pass.
- `git diff --check`: pass.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-17-inline-tool-detail-utf8-panic.md`
  passes.

## Next

- Restart `yalda-gpui`; the same emoji-bearing command can then render in a tool
  group header without crashing.
