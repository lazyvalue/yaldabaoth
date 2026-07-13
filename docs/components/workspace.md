# Component: Workspace

**Status:** living
**Component token:** `Workspace` (⇒ `UXI-Workspace-N`)

## Description

A **Workspace** (`Workspace<App>`) is a single tab-strip + buffer-pool container,
one per OS-level **Frame**. Each entry in its tab strip is a **Tab** (`Tab<App>`),
which owns an n-ary **layout tree** of tiles and a focused-tile pointer. Interior
nodes of that tree are **Splits** (`Layout::Split`) — a direction (`H` = stacked,
`V` = side-by-side) plus weighted children — and each leaf is a **Tile**
(`Window<App>`, a stable `WindowId` holding exactly one `App`: `Buffer`, `Agent`,
or `Linear`). The code-level struct is still called `Window`, but in discussion we
say **tile** to avoid confusion with the OS-level frame.

Above the tiles, the tab strip carries the workspace's tabs (per-tab label, active
marker, click-to-select, rename, next/prev, new/close, and `ctrl-<n>` jump by
number); the tag bar drives tile tags and the automatic **layout modes** (layout-
mode cycle, desktop tile size, promote-to-master, master-count +/-). Non-ephemeral
workspaces are numbered `1..N` in the jump panel, and those numbers are the jump
targets.

## References

- `docs/specs/spec-tabs-and-splits.md` — tabs + the n-ary split/layout tree.
- `docs/specs/spec-layout-patterns.md` — tile tags + automatic layout modes.
- `docs/specs/spec-desktop-mode.md` — desktop tile sizing / master layout; the
  tile/slot geometry engine (`Slot`, `Span`, `DesktopState`, occupancy,
  Block-rule edge resize, culling) that the plane model below reuses.
- `docs/specs/spec-infinite-plane-workspace.md` — DRAFT deep design for the
  **infinite-plane** model (`UXI-Workspace-2..7`): a workspace *is* one unbounded
  signed-coordinate plane with a pan/semantic-zoom camera. When those UXIs ship,
  this Description is rewritten around the plane and the split/layout-mode text
  above becomes historical.
- Migrated from `docs/ux-invariants.md` INV-UX-11 (`ctrl-<n>` workspace jump). That
  entry is now `→ migrated here`.

## UX invariants

### UXI-Workspace-1 — `ctrl-<n>` jumps to the n-th workspace (the number the panel shows)

**Statement.** The jump panel numbers **non-ephemeral** workspaces `1..N` (the
`idx + 1` badge), and `ctrl-1`…`ctrl-9` / `ctrl-0` (the 10th) jump straight to
that workspace. The displayed digit and the keystroke target always agree because
both skip ephemeral virtual workspaces (ADR-0021) — `goto_workspace_number(n)`
selects the n-th non-ephemeral tab. A digit past the last workspace is a no-op.

**Applies to.** `main.rs`: the `GotoWorkspace1..10` actions + `ctrl-<n>`
bindings (app-global, `None` context), `goto_workspace_number`, and the
`WorkspaceNavExt::workspace_nav` helper wired onto every screen root (the action
needs a handler in the focused element's ancestry — same discipline as
`toggle_jump_panel`). `jump_panel_view.rs`: the workspace-row number badge.

**Edge.** An **empty-layout** workspace renders a bare div with no action
handlers (chrome.rs), so global keys (incl. `ctrl-<n>`, `ctrl-tab`, `cmd-t`)
don't dispatch while sitting on one — a pre-existing, transient edge state, not
specific to this binding.

**Why.** Direct numeric workspace switching, matching the visible numbering.

**Status.** `implemented` (headless).

**Enforcement.** `verify_harness.rs`: `ctrl_digit_switches_workspace` (full
keymap→action→handler dispatch: `ctrl-3` then `ctrl-1`, plus past-the-end no-op)
and `workspace_number_skips_ephemeral` (numbering skips the ephemeral tab).
