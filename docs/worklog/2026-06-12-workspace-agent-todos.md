# Worklog: workspace + agent TODOs

**Date:** 2026-06-12
**Branch:** workspace-agent-todos — the two open TODO blocks in `untitled.md`
(Workspace KV / commands / click-to-focus, Agent commands / CWD inheritance).

Cleared every unchecked box in the "Workspace TODO", "Agent TODO", and
"Commands to delete" sections of `untitled.md`. The per-session agent CWD layer
(`spec-agent-cwd.md` — `:claude-new <path>`, `:claude-cd`, OS-level
`cmd.current_dir`) already existed; the new work is the **workspace-scoped**
layer above it plus two small UX items.

## Built (with status)

- **Workspace KV registry** — `Tab<C>` (the type the product calls a
  "Workspace": one layout + a set of tiles) gains `kv: HashMap<String,String>`
  with `kv_get` / `kv_set` / `kv_remove`. Apps read it during render, so a write
  + `cx.notify()` is the "all apps notified of kv changes" mechanism from the
  spec sketch — no separate pub/sub built. Persisted as `PersistedTab.kv`
  (`#[serde(default, skip_serializing_if = HashMap::is_empty)]`, so old
  snapshots load empty and downgrade is lossless). Snapshot + restore wired in
  `persist.rs` and the two `main.rs` Tab-restore/create sites.

- **"Set CWD" workspace command** — new workspace-menu entry `s c` "set cwd" →
  `RenameTarget::WorkspaceCwd { index }` path-input overlay, pre-filled with the
  current workspace cwd (or `process_cwd`). On commit the path is resolved +
  validated via the existing `resolve_agent_cwd_arg` (tilde, canonicalize,
  must-be-a-directory) and written to `kv["cwd"]`; invalid input surfaces a
  transient status and no-ops. This is the first concrete use of the registry.

- **Mouse click focuses a tile** — `render_layout`'s leaf branch now wraps every
  leaf when the active tab has >1 leaf (previously only the focused/marked leaf
  got a wrapper) and attaches `on_mouse_down(Left, …)` on unfocused tiles → new
  `focus_window_by_click(id, window, cx)`. Bubble phase, so a click inside an
  editor still positions the caret first, then focus follows. The method mirrors
  `focus_next`'s side effects (set `tab.focused`, re-assert view focus, sync
  rail, persist, notify) and no-ops when the tile is already focused.

- **Agent CWD inheritance from workspace** — `new_agent_session` /
  `bootstrap_fresh_agent_session` default cwd is now
  `explicit arg → active_workspace_cwd() → process_cwd`, where
  `active_workspace_cwd()` reads the active tab's `kv["cwd"]` (empty = unset).
  Implements the untitled.md Agent TODO "inherit CWD from workspace; if none,
  inherit CWD from app" (the app/process dir being the final fallback).

- **Removed detach / attach** — both off the agent `.` menu and the
  `dispatch_menu_command` table. The `detach_active_agent_session` /
  `attach_active_agent_session` methods are kept `#[allow(dead_code)]` as
  internal machinery for a future re-wiring.

## Verification

- `cargo build --bin yalda-gpui` clean (only pre-existing dead-code warnings).
- `cargo test --lib` → 136 passed; `cargo test --bin yalda-gpui` → 188 passed.
- New headless regression test `verify_harness::workspace_kv_cwd_inheritance`:
  builds the real view, asserts `active_workspace_cwd()` is `None` with no key,
  reflects a written `kv["cwd"]`, and treats an empty string as unset.
- **Runtime check still owed** (no headless GUI): click-to-focus actually moving
  focus on click; `s c` set-cwd overlay flow end-to-end; a new agent session in
  a workspace with a set cwd spawning in that directory (`pwd` in the agent).

## Artifacts

- `untitled.md`: Workspace TODO, Agent TODO, and "Commands to delete" boxes all
  ticked with per-item notes.
- Worktree `workspace-agent-todos` ready to integrate into main.
