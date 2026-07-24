# 007 — `tab`→`workspace` / `Workspace`→`Frame` rename; eradicate `tab`

**Goal.** Complete the rename ADR-0002 deferred, now unblocked by the `Project`/
`Frame` naming. Eradicate the workspace-concept `tab` from code + docs. **Do this
LAST** — one mechanical sweep after the earlier tickets stop churning call-sites.
(ADR-0028 §5.)

## Subtasks

- [ ] Type rename: `Tab<C>` → `Workspace<C>`; old container `Workspace<C>` →
      `Frame<C>`; `PersistedTab` → `PersistedWorkspaceLayout` (or similar,
      distinct from the existing `PersistedWorkspace` root — resolve the name).
- [ ] Fields/methods: `active_tab`→`active_workspace`, `tabs`→`workspaces`,
      `select_tab`/`next_tab`/`prev_tab`/`new_tab`/`close_tab`/`rename_tab`/
      `set_active_tab`/`open_ephemeral_tab`/`tab_containing`/`auto_tab_name` →
      `*_workspace`; ~340 call-sites move with them.
- [ ] Actions: `NewTab`/`CloseTab`/`NextTab`/`PrevTab`/`RenameTab` →
      `*Workspace`; keep the `ctrl-tab` **keystroke** (physical Tab key) — rename
      only the action name it maps to.
- [ ] User strings: keymap label `"Tabs & workspaces"`→`"Workspaces"`; descs
      "New/Close/Rename tab"→"…workspace"; test literal `"tab-1"`→`"workspace-1"`.
- [ ] Comments (~200) + docs (~250, incl. renaming/retiring
      `spec-tabs-and-splits.md` prose) — workspace-concept only.
- [ ] **DO NOT TOUCH:** physical Tab key (`Key::Tab`, `ctrl-tab` keystroke,
      `NextBuffer`/`PrevBuffer` on `tab`), tab-character/indentation, markdown
      `Table*` types.
- [ ] Mark ADR-0002's deferral superseded by ADR-0028 §5.

## Verification

Full suite green after the sweep (the rename is behavior-preserving; the existing
~340 call-site tests are the guard). `cargo build` + `cargo test --bin yalda-gpui`
clean. A grep for the workspace-concept `\bTab\b` / `active_tab` / `NewTab` returns
only the physical-Tab-key + table + doc-bridge keeps.

## Links

ADR-0028 §5,§6 · ADR-0002 (superseded deferral) · the tab-enumeration map (in the
session record).
