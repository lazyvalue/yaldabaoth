# Worklog: Tiles & Apps — vocabulary rename + content-model restructure

**Date:** 2026-06-10
**Branches touched:**
- `tiles-rename` → folded to `master` (`5fb267c`, `76be665`; merge `e8ac31e`) — ADR-0019 + Pane/Panel→Tile, Sidepane→Sidebar rename.
- `app-model` → folded to `master` (`8ed3e2f` spec, `e83fe10` mechanical, `0ba6561` behavioral) — `WindowContent → App` restructure.

Both worktrees under `.claude/worktrees/`; removed after merge.

## Built (with status)

### Vocabulary rename (`tiles-rename`)
- **ADR-0019** — committed the IA reframing: a Tile holds one App; an App is a
  Buffer or an Agent; Browser is the Buffer app's picker state; Cmd+O is
  Buffer-scoped; browser-over-Agent removed.
- **Pane/Panel → Tile, Sidepane → Sidebar** across code + living specs. NOT
  mechanical: "pane" and "panel" each had three senses (container / chrome /
  unrelated compose+TUI panels); disambiguated per-token. `pane_bg/border/header`
  → `sidebar_*`; desktop `DesktopPanelSize` → `DesktopTileSize`. Verified: both
  bins build, 136 bin/lib tests pass. Dated records (worklog/research/old ADRs)
  left as historical.

### Content-model restructure (`app-model`)
- **`spec-tiles-and-apps.md`** (ACTIVE) — written, adversarial-reviewed (verdict
  REVISE, all items folded: B7 discard mechanism, Agent no-stash fallback,
  sole-tile floor, source-less Viewing, restyle reach-through).
- **`WindowContent → App`** — `App { Buffer(BufferApp), Agent(AgentRing) }`,
  `BufferApp { Picking, Viewing, Editing }`. 125 sites migrated; Browser folded
  into `BufferMode::Picking`; both `underlying` stashes narrowed to
  `Option<Box<BufferApp>>` (browser-over-Agent now type-unrepresentable); Cmd+O
  inert on Agent tiles; no-stash paths fall back to a fresh `Picking`, never
  close; persist tag shape changed with the existing `.ok()` discard (no version
  field). Implemented via a 2-step + 6-reviewer Workflow; verdict CLEAN.
- **Verified:** both bins build clean; 136+64 bin/lib tests pass; full suite
  shows only the 2 pre-existing `snapshot_test` failures; human runtime smoke
  passed (Cmd+O scoping, agent-inert, back-to-buffer, session-close fallback,
  Doc↔Edit toggle). All 7 spec behaviors confirmed at file:line.

## Open / unresolved
- **Stale snapshots** — `snapshot_code_block_rust` / `snapshot_complex_document`
  have failed since `c6d237c` (the `start_line` field was added to `CodeBlock`
  without regenerating snapshots). Pre-existing, unrelated to this work; needs a
  `cargo insta accept` pass. `*.snap.new` is now gitignored.
- **Command rename** — `new-browser-tile` → `new-buffer-tile`,
  `inplace-browser-tile` → `inplace-buffer-pick`. Any external keybind/doc refs
  should be re-checked.
- **`spec-workspaces-and-splits.md`** is still referenced (code + ADR-0019) but does
  not exist — the workspace tree was never given its own spec.
- **Buffer pool still unwired** (`workspace.rs` `buffer_retain`/`FileBufferId`)
  — the App restructure did not touch it; shared-doc multi-membership remains
  blocked.

## Decisions
- **ADR-0019: Tiles contain Apps (Buffer | Agent)** — came up from a UX/IA
  framing pass; renames the container to Tile and collapses the four flat content
  variants into two app kinds.

## Verification status
- Build + tests + human runtime smoke all green for both branches. The GPUI
  headless-harness gap remains: behavioral changes (Cmd+O scoping, fallbacks)
  needed a human click-through; can't be driven headlessly yet.

## Next
- `cargo insta accept` to clear the two stale snapshots (separate, pre-existing).
- Optionally write `spec-workspaces-and-splits.md` to retire the dangling reference.
- Re-check any keybindings/docs for the renamed `*-tile` / `*-buffer-pick` commands.
