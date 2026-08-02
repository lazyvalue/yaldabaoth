# ADR-0021: Ephemeral virtual workspace for direct agent-session visits

Status: Accepted (2026-06-18)
Related: spec-jump-panel.md, spec-agent-session-ownership.md, ADR-0019.

## Context

The jump panel (spec-jump-panel.md) must let you open any agent session directly,
including one that is **free** — present in the project/session domain but with
no durable workspace-tile reference. A free session has nowhere durable to render. We
do not want selecting a free session to permanently mint a workspace the user
then has to clean up; the desired behavior is "show it transiently, and when I
jump away it disappears."

User-facing "workspace" is `Tab<C>` (workspace.rs). The view owns one
`Workspace<App>` with `tabs: Vec<Tab>` + `active_tab: usize`. All render, focus,
key-dispatch, tab-switch, and persistence paths key off this vec + pointer.

## Decision

Model the virtual workspace as a **real but flagged `Tab`** pushed onto `tabs`,
not as a separate parallel slot.

- Add `Tab::ephemeral: bool` (kept a plain `bool`, not an `Option<SessionId>`,
  so `workspace.rs` stays decoupled from the agent layer — the shown session is
  recoverable from the tile's own `bound`). `true` marks an ephemeral virtual
  workspace; its layout is a single leaf — one `App::Agent` tile referencing the
  selected session.
- Creating one: `Workspace::open_ephemeral_tab(content)` pushes the flagged
  `Tab` and sets `active_tab` to it (replacing any existing ephemeral — we never
  accumulate more than one). The tile holds the same ordinary `SessionId`
  reference as a real-workspace tile; `Tab::ephemeral`, not a special tile state,
  determines that this reference does not count as durable placement.
- **One teardown chokepoint.** `Workspace::set_active_tab(idx)` is the single
  switch path: when leaving an ephemeral tab it removes that tab (dropping its
  tile → only that viewport reference disappears) and
  fixes up the index. `next_tab`/`prev_tab` and the view-level switches
  (`select_tab`, `switch_to_buffer`, `jump_to_window`, the file-open switch)
  route through it, so the lifecycle lives in exactly one place.
- Ephemeral tabs are **excluded** from: the jump panel's Workspaces section, the
  `?` workspace-switch menu (`global_menu`), and persistence (`snapshot_workspace`
  skips them and clamps `active_tab` into the surviving range).

## Alternatives rejected

- **Separate `Option<Tab>` slot on the view, rendered instead of
  `tabs[active_tab]`.** Conceptually tidy, but forces every render / focus /
  key-dispatch path to branch on "is the virtual workspace active," reintroducing
  exactly the scattered ambient special-casing ADR-0020 warns against. One flag
  + one teardown hook reuses all existing tab machinery untouched.
- **Permanent tab the user closes manually.** Violates the "disappears when I
  jump away" requirement and accumulates clutter.
- **Render the free session inline in the panel.** The panel is a navigator, not
  a content host (INV-JP2); rendering an ACP transcript there breaks the
  cached-child + 1:1 model.

## Consequences

- A single `ephemeral` flag + a single teardown chokepoint is the entire surface
  area; no new render branch.
- The teardown must run before/at every `active_tab` change — enforced by
  routing all switches through `set_active_tab` and covered by headless tests
  (create virtual → switch away → tab gone + durable placement unchanged;
  second direct session replaces). `UXI-JumpPanel-19` later extended the same
  mechanism to sessions that already have workspace placement: multiple
  viewports may reference one project-owned session, while only non-ephemeral
  workspace references define placement.
- Closing or interacting inside the ephemeral tile follows normal agent-tile
  rules; only *leaving the workspace* triggers teardown.
- Persistence and the two switch surfaces (panel, `?` menu) must skip ephemeral
  tabs. A missed filter would persist or list a ghost workspace — each reads the
  same `tab.ephemeral` flag.
