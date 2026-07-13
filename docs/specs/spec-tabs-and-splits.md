# Tabs and Splits — the workspace layout tree

**Status:** describes SHIPPED behavior (retroactive spec). The whole tabs /
splits / persistence model documented here is implemented in
`src/bin/yalda-gpui/workspace.rs` and wired into the live view. The single
exception is **§10 (cross-view edit broadcast), which is DRAFT / unshipped** —
it is documented as a design target, not a description of running code.

**Last updated:** 2026-07-12
**Builds on:** ADR-0002 (workspaces model), ADR-0005 (shared content pool),
ADR-0007 (doc/edit shared rope), ADR-0021 (ephemeral virtual workspaces),
ADR-0023 (typed workspace cwd)
**Related:** `docs/decisions/0019-tiles-contain-apps.md` +
`docs/specs/spec-tiles-and-apps.md` (what a leaf *holds* — one App),
`docs/specs/spec-layout-patterns.md` (marks, automatic layouts, tags),
`docs/specs/spec-desktop-mode.md` (the free-placement layout mode),
`docs/specs/spec-rail.md` (persistent side columns), `docs/UX.md`
(the reader-facing "Window & layout taxonomy" summary), `spec-agent-cwd.md`
(reuses Constraint §11 + Behaviors 23–24), `spec-agent-window.md` (the leaf as
one window-kind; partially inlines §10's editor substrate).

## Overview

This spec covers the **structure** the GPUI frontend arranges its content in —
tabs, the n-ary split tree beneath each tab, focus, the pooled shared-rope file
buffers, persistence — and NOT what a leaf holds. What a leaf holds is one
**App** (`App::{Buffer, Agent, Linear}`); that content model is
`spec-tiles-and-apps.md` + ADR-0019 and is linked, not re-derived, here. The
tree code is deliberately **generic over the content type** (`Workspace<C>`,
`Tab<C>`, `Layout<C>`, `Window<C>`): it gained no App-kind knowledge when
`WindowContent{Doc,Edit,Browser,Agent}` collapsed into `App{Buffer,Agent}` —
only the type parameter changed (ADR-0019 Consequences; Constraint §1).

The containment hierarchy (see `docs/UX.md` for the reader-facing table):

| Term | Code type | Meaning |
|---|---|---|
| **Frame** | GPUI `Window` | The OS desktop window (one per process today). |
| **Workspace** | `Workspace<C>` | The tab-strip + file-buffer pool + active-tab pointer. |
| **Tab** (user-facing: *"workspace"*) | `Tab<C>` | One tab-strip entry: its own layout tree, focus pointer, rail, layout mode, and cwd. |
| **Split** | `Layout::Split` | Interior node: a `SplitDir` + weighted children (`≥ 2`). |
| **Tile** | `Window<C>` (code name `Window`) | A leaf: a stable `WindowId` + one App. |

> Naming note. The code type that owns the tab list is `Workspace<C>`, but the
> product presents each **`Tab`** as a named "workspace." This is an accepted
> internal name collision (`Tab::auto_name`/`display_name` render as the
> workspace label). The leaf type is `Window<C>` but we say **tile** in prose to
> avoid confusion with the OS frame. There is no `FocusedWindow` type; focus is
> the `WindowId` in `Tab::focused`, read via `Workspace::focused_content{,_mut}`
> / `focused_window_id`.

Substrate: `src/bin/yalda-gpui/workspace.rs`. Split render (H = stacked column,
V = side-by-side row) is `chrome.rs` (`SplitDir::V → flex_row`,
`SplitDir::H → flex_col`). Split keybindings are `keymap_registry.rs`.
Persistence is `persist.rs`.

## Data model

### The layout tree — `Layout<C>`

```rust
enum Layout<C> {
    Empty,                                        // transient sentinel only
    Leaf(Window<C>),                              // a tile
    Split { dir: SplitDir, children: Vec<(f32, Layout<C>)> },
}
enum SplitDir { H, V }   // H = children stacked top→bottom; V = left→right
struct Window<C> { id: WindowId, content: C }    // WindowId = u64, workspace-unique
```

- A `Split` holds **`≥ 2`** children. Pruning to one child collapses the split
  into that child (Behavior 14). Weights within a single `Split` sum to `1.0`
  and **renormalize proportionally** on every insert/close/resize
  (`renormalize`).
- `Empty` is a `std::mem::take` placeholder used *inside* mutation methods only.
  **It must never appear in a tree at rest** — every mutator restores a
  non-`Empty` root before returning. (`insert_leaf_into_tab` treats an `Empty`
  target as "adopt the arriving leaf as root," the one place it is observed
  externally, and only transiently.)
- The tree is walked/queried by a family of pure methods: `find_leaf{,_mut}`,
  `for_each_leaf`, `path_to` (root→leaf child-index path), `node_at_path_mut`,
  `leaf_ids`, `leaf_count`, `swap_leaf_contents`, `skeleton` (shape without
  content, for saving a manual tree across a layout-mode switch).

### The tab — `Tab<C>`

Each `Tab` owns: `auto_name` + optional `display_name` (the label), `layout`
(its `Layout<C>` root), `focused: WindowId`, an optional `rail`
(`spec-rail.md`), the `ephemeral` flag (ADR-0021 virtual workspaces),
layout-pattern state (`layout_mode`, `saved_manual_layout`, `master_ratio`,
`master_count`, `tag_view`, `desktop`; `spec-layout-patterns.md` /
`spec-desktop-mode.md`), and a **private, required** `cwd: WorkspaceCwd`
(ADR-0023 — a cwd-less workspace is unrepresentable; built only via
`Tab::with_layout`, read via `cwd()`, changed via `set_cwd()`).

### The workspace — `Workspace<C>`

Owns `tabs: Vec<Tab<C>>`, `active_tab: usize`, the file-buffer pool
(`file_buffers`, `path_index`), id counters (`next_window_id`,
`next_buffer_id`, `next_tab_index`), the workspace-global `marks`
(`spec-layout-patterns.md` Phase 1), `tag_shortcuts`, and a `default_cwd`
fallback for the root tab. Constructed via `Workspace::with_initial(content,
cwd)` (root tab holding one leaf) or `new()` + restore.

### The file-buffer pool — `FileBuffer` / `SharedCore`

`SharedCore = Rc<RefCell<EditorCore>>`. A file-backed editor lives once in the
pool as a `FileBuffer` (keyed by canonical path, Constraint §11) and its
`SharedCore` is **cloned into every tile that binds to it**, so multiple splits
of the same file mutate one rope + one undo stack while each tile keeps its own
cursor / selection / scroll (ADR-0005 / ADR-0007). Liveness is refcount +
strong-count GC: `open_and_retain(path) → (id, core)` pools-or-loads and bumps
the count; a tile drops its `Rc` on close; `gc_buffers()` reaps any buffer whose
`Rc::strong_count == 1` **and** is not modified (dirty buffers stay pooled for
`:buffers` recovery). This is the substrate for "shared edits across splits."

## Behaviors

Numbered; external docs cite specific numbers (notably **12–13** for splits and
**23–24** for persistence). Numbers are stable — extend, don't renumber.

### Tabs / workspaces

1. **A workspace is a non-empty set of tabs.** The root tab is created with
   content + a cwd (`with_initial` / `push_initial_tab`). `active_tab` always
   indexes a real tab; closing tabs clamps it into range (`close_tab`).

2. **Sole-tile / sole-tab floor.** The workspace is never left with zero usable
   surfaces. Closing the only tile in the only tab is a no-op at the callsite
   (it does not quit the app); `close_focused` returning `Ok(None)` signals the
   caller to substitute a placeholder rather than vanish. (See
   `spec-tiles-and-apps.md` B4 for the Buffer-side realization.)

3. **New tab.** `push_initial_tab(content, cwd)` appends a tab holding one leaf
   and makes it active, auto-named `workspace-N` from `next_tab_index`. The new
   tab **inherits** the active tab's cwd (`inherited_cwd`), never silently the
   process dir (ADR-0023). Bound to **Cmd-T** (`NewTab`).

4. **Close tab.** `close_tab(idx)` removes a tab and re-clamps `active_tab`.
   Closing a tab **frees, does not kill**, any Agent session its tiles showed —
   an Agent tile holds only a `SessionId` key; the session lives in the
   `AgentSessions` store and stays running, re-bindable elsewhere
   (`spec-agent-session-ownership.md`). Only `claude-close` kills a session.
   Bound to **Cmd-Shift-W** (`CloseTab`).

5. **Tab cycling + direct jump.** `next_tab` / `prev_tab` wrap and route through
   `set_active_tab` (Behavior 6). Bound to **Ctrl-Tab / Ctrl-Shift-Tab** and
   **Cmd-Shift-] / Cmd-Shift-[** (and arrow variants). **Ctrl-1..9,0** jump
   straight to the Nth non-ephemeral workspace (`GotoWorkspace1..10`; `0` = 10th)
   — see the macOS caveat in §12.

6. **Workspace-switch chokepoint (`set_active_tab`).** Every activation flows
   through this one method (ADR-0021): if the *departing* tab is an **ephemeral
   virtual workspace** it is torn down on the way out (its single Agent tile
   drops, returning the session to *free*), and `idx` is index-corrected for the
   removal. Renames go through the label; the tab strip renders `display_label`.

7. **Ephemeral virtual workspaces (ADR-0021).** `open_ephemeral_tab(content)`
   opens a transient single-tile tab (`ephemeral = true`), inheriting the
   spawning workspace's cwd *before* any teardown. At most one exists at a time
   (a second replaces the first); it is invisible to the jump panel's Workspaces
   section, the `?` menu, and persistence, and is destroyed the instant focus
   leaves it (Behavior 6). Used by the jump panel to display a free agent
   session.

### Focus

8. **Focus is a single `WindowId` per tab (`Tab::focused`).** It must always
   name a live leaf in that tab's tree. `focused_content{,_mut}` /
   `replace_focused_content` / `focused_window_id` read/mutate through it. After
   any structural mutation the mutator re-points `focused` at a surviving leaf.

9. **Focus motion — tree topology.** `focus_next` / `focus_prev` cycle leaves in
   tree order (depth-first, `children` order, wrapping). `focus_motion(dir)`
   walks up to the nearest ancestor `Split` whose direction matches
   (`Left/Right → V`, `Up/Down → H`), steps to the sibling at `idx ± 1`, and
   descends into that sibling's first leaf; no-op when there is no sibling that
   way. In **Desktop** layout mode both instead use spatial/row-major slot
   navigation (`spec-desktop-mode.md` Behavior 5). Keys: **Ctrl-W h/j/k/l**
   (directional), **Ctrl-W w / Ctrl-W Shift-W** (next/prev). See §12.

### Splits — Behaviors 12–13 (cited by `main.rs:220`)

10. *(reserved — see §10 below; the numbered top-level section §10 is the
    cross-view edit broadcast. There is intentionally no Behavior 10/11 topic
    beyond this pointer, kept so split behaviors land on 12–13.)*

11. *(reserved — see Constraint §11.)*

12. **Create a split (`split_focused(dir, content)`).** Inserts a new leaf
    adjacent to the focused leaf in the active tab, and focuses it. Placement
    (this is the "Behavior 12–13" `workspace.rs:1531` implements):
    - **Focused leaf is the root** → wrap it in a fresh 2-child `Split(dir)`
      `[(0.5, old_root), (0.5, new_leaf)]`.
    - **Parent split has the same `dir`** → insert the new leaf right after the
      focused leaf as a sibling (no new nesting); its weight seeds to the
      sibling average and all weights renormalize.
    - **Parent split is perpendicular** → wrap just the focused leaf in a nested
      2-child `Split(dir)`, preserving the focused leaf's outer weight.
    Bound to **Ctrl-W s** (`SplitH`, horizontal split) and **Ctrl-W v**
    (`SplitV`, vertical split). Returns the new `WindowId`.

13. **Navigate / manage splits.** The companion operations to Behavior 12 that
    complete the split surface:
    - **`close_focused()`** (**Ctrl-W c**) — remove the focused leaf; re-focus a
      sibling (previous index, else first remaining); **collapse** a
      now-single-child split into that child (Behavior 14); renormalize.
      Returns `Ok(None)` when the leaf was the tab root (Behavior 2 floor).
    - **`only()`** (**Ctrl-W o**) — make the focused leaf the tab's whole tree,
      closing every other tile.
    - **`resize_focused(delta)`** (**Ctrl-W < / -** shrink, **Ctrl-W > / +**
      grow) — shift weight between the focused leaf and its next sibling, each
      slot clamped to `[0.05, 0.95]`, then renormalize.
    - **`equalize_focused()`** (**Ctrl-W =**) — set all weights in the focused
      leaf's parent split to `1/n`.
    - Focus motion is Behavior 9 (also Ctrl-W-prefixed).

14. **Single-child collapse invariant.** No `Split` ever rests with `< 2`
    children. `close_focused`, `detach_focused`, and `only` all collapse a split
    down to its sole remaining child when a removal leaves one.

15. **Weights sum to 1.0.** Every insert/close/resize/equalize path renormalizes
    the affected split so its weights sum to `1.0`; `renormalize` distributes
    uniformly if a sum is non-positive.

### Moving tiles between workspaces

16. **Detach (`detach_focused`).** Removes the focused leaf and returns the owned
    `Window<C>` (content travels with it — no clone), pruning the source split
    exactly like `close_focused`. Reports whether the source tab is now empty
    (focused leaf was the root → source `layout` left `Empty` for the caller to
    remove).

17. **Insert into a target tab (`insert_leaf_into_tab`).** Adopts a detached
    leaf into another tab, focusing it there: `Empty` → adopt as root; single
    `Leaf` → wrap both in a `Split(V)`; `Split` → append as a new child +
    renormalize. Window ids are workspace-unique, so the leaf keeps its id.

18. **"Move tile to workspace" (Ctrl-W m).** `MoveTile` = detach (16) + insert
    (17) into the picked workspace — the tile relocates whole.

19. **"Also-show tile in workspace" (Ctrl-W Shift-M).** `AlsoShowTile` — for a
    *file-backed* tile, creates a **second** view onto the same pooled buffer in
    the target workspace (shared rope, per the pool), leaving the original in
    place. Agent / picker tiles are single-home and rejected.

### Layout modes

20. **Per-tab layout mode (`LayoutMode`).** `Desktop` (default; free tile
    placement, `spec-desktop-mode.md`), `Manual` (the hand-built tree IS the
    layout), and the automatic modes `MasterStack` / `Monocle` / `Columns`
    (`spec-layout-patterns.md` Phase 2). **Ctrl-W space** cycles
    (`CycleLayoutMode`). The layout tree is always the **content owner**;
    Desktop keeps geometry in a separate `DesktopState` slot map and never
    drains the tree, so mode round-trips preserve content.

21. **Retile / mode switch.** `retile_active` rebuilds the tree from its leaves
    for the automatic modes only (no-op in Manual/Desktop). `set_layout_mode`
    saves the manual tree's `skeleton` when leaving Manual and restores it
    (appending any leaves created meanwhile) on return.

### The file-buffer pool

22. **Open-or-pool a file (`open_buffer` / `open_and_retain`).** A file is loaded
    once per canonical key (Constraint §11) into a pooled `FileBuffer`;
    subsequent opens of the same path hand back the **same** `SharedCore`. A
    path that does not exist on disk opens an empty buffer at that path (new
    file). Refcount tracks bound tiles; `gc_buffers` reaps husks (see Data
    model). This is what makes a file split share one rope + undo stack.

### Persistence — Behaviors 23–24 (cited by `spec-agent-cwd.md`)

23. **The tree, tabs, and per-tab state persist to `workspace.json`; leaves store
    session ids only.** On change the whole `Workspace` is snapshotted to
    `PersistedWorkspace { tabs, active_tab, marks, tag_shortcuts, buffer_tags }`
    (`persist.rs`). Each `PersistedTab` carries `auto_name`, `display_name`,
    `focused_window` (the stable `WindowId`), the `PersistedLayout` tree
    (`Leaf | Split{dir, children:(weight, …)}`), the optional rail
    (`spec-rail.md` §14), the layout-pattern fields (`layout_mode`,
    `master_ratio`, `master_count`, `tag_view`, `desktop_slots`/`desktop_spans`
    keyed by `WindowId`, `spec-desktop-mode.md` Behavior 7), and the workspace
    `cwd`. A leaf persists a **flat, un-nested** `PersistedKind`:
    - `Buffer { mode }` with `PersistedBufferMode::{Viewing{path}, Editing{path},
      Picking{dir}}` — the file path / browser dir only;
    - **`Agent { session_id: Option<String> }` — the ACP session id ONLY**
      (JSON tag stays `"claude"` so it lines up with `acp_sessions.json`). The
      session's transcript / cwd / model live independently in
      `acp_sessions.json` + the server WAL, keyed by that id. `spec-agent-cwd.md`
      relies on exactly this: `workspace.json` keeps storing only session ids in
      its Agent leaves; per-slot cwds live next to the id in `acp_sessions.json`.
    - `Linear {}` / `Keymap {}` — stateless; restore opens fresh.
    **Not persisted:** window-local view state (scroll/cursor) and the
    `underlying` stashes — so no on-disk layout can encode an agent-behind-picker
    (`spec-tiles-and-apps.md` B7). Ephemeral virtual workspaces (Behavior 7) are
    excluded.

24. **Restore is lossy-tolerant; there is NO schema version field.** The load
    path (`restore_kind`) deserializes each leaf with `serde_json::from_value(…)
    .ok()` and discards on failure, falling back to defaults, so a stale
    `workspace.json` from an older build **silently re-opens at defaults** rather
    than corrupting the arrangement (`spec-tiles-and-apps.md` B7). Optional
    fields (`rail`, `cwd`, `desktop_*`, layout-pattern params) `#[serde(default)]`
    so older snapshots load — a missing `cwd` migrates from the legacy `kv["cwd"]`
    else the process dir (ADR-0023). Desktop slots are keyed by stable
    `WindowId`, not by position, so a mismatched entry degrades to reconciliation
    rather than scrambling tiles. Durable data (WAL / ACP session list) is keyed
    independently and is unaffected — only the remembered tile arrangement is
    lost on a first run of an incompatible build.

## Constraints

1. **The tree stays generic — no App-kind knowledge in `workspace.rs`.** Split /
   focus / persistence operate on `Layout<C>` for any `C`. Folding
   `WindowContent → App` changed only the type parameter and the content-access
   match arms elsewhere (ADR-0019). New App kinds require no changes here.

2. **Shared-rope invariant.** Two tiles bound to the same file share one
   `SharedCore` (one rope, one undo stack); each keeps its own cursor / selection
   / scroll. `Viewing ⇄ Editing` is a zero-copy mode flip over that shared core,
   never a content swap or re-parse (ADR-0007).

3. **`Empty` never rests in a tree.** It is a `mem::take` placeholder inside
   mutators only; every mutator restores a non-`Empty` root (or, for
   `detach_focused`, hands `Empty` to a caller that immediately removes the tab).

4. **Splits have `≥ 2` children; weights sum to 1.0.** Enforced by the
   collapse-on-prune (Behavior 14) and `renormalize` (Behavior 15) discipline.

5. **Focus always names a live leaf.** `Tab::focused` is re-pointed to a
   survivor by every structural mutator; `marks.gc` / desktop `reconcile` drop
   references to dead windows.

6. **`WindowId`s are workspace-unique and stable.** Allocated monotonically
   (`alloc_window_id`); they survive detach/insert and persistence, which is why
   `focused_window`, marks, desktop slots, and rail `pinned_to` can all key
   through them across a restore.

7. **Every real workspace has a cwd.** `Tab.cwd: WorkspaceCwd` is private +
   required (ADR-0023); no construction path — real or ephemeral — can omit it.

8. **Persistence stores structure + ids, not session content.** Agent leaves
   carry `session_id` only (Behavior 23); conversation state is owned elsewhere.

9. **The rail is not a leaf** (`spec-rail.md`): it cannot be split, focused via
   `focus_motion`, or resized with `resize_focused`. It is per-tab chrome pinned
   to a leaf.

### §11 — Path canonicalization (cited by `spec-agent-cwd.md`)

The canonical key for the buffer pool (and reused by any consumer that needs a
stable path identity, e.g. per-slot agent cwds) is computed by
`Workspace::canonical_key(path)`:

> If `std::fs::canonicalize(path)` succeeds (the target exists on disk), use its
> result. **Otherwise** (a path that does not exist yet — a new file, a
> not-yet-created dir), fall back to the absolute path — `path` itself if
> absolute, else `current_dir().join(path)` — with `.` (`CurDir`) dropped and
> `..` (`ParentDir`) collapsed by popping the accumulated component. The result
> is a normalized `PathBuf` usable as a stable key.

`spec-agent-cwd.md` (§2.2, §4.1) reuses this exact rule verbatim for resolving
`:claude-new <path>` / `:claude-cd <path>` cwds.

## §10 — Cross-view edit broadcast **[DRAFT — unshipped]**

> **Status: NOT IMPLEMENTED.** This section is a design target. No code today
> broadcasts edits between views of one core beyond the shared-rope mechanics of
> Constraint §2. `spec-agent-window.md` (§ Editor Extensions, note E4) **partially
> inlines** the substrate below (`LineAnchor` / per-line metadata) for its own
> Worksheet gutter + tool-call anchoring, scoped to the single agent-window view;
> the broader multi-view broadcast described here has not landed.

The problem. Constraint §2 already lets multiple tiles bind one `EditorCore`
(one rope, one undo stack) — an edit in one split is *present* in the rope the
others read. What is **not** solved is keeping each view's own **cursor,
selection, scroll, and any anchored decorations** correct when *another* view
edits the shared core: an insert/delete upstream of a view's caret must shift
that caret (and its selection, and any line-anchored markers) by the edited
span, or the view silently desynchronizes from the text it is painting.

Proposed substrate (the shape `spec-agent-window.md` builds on):

- **Position-shift events.** An edit applied to a `SharedCore` emits a
  shift descriptor (edited byte/line range + delta); every *other* `EditorView`
  bound to that core consumes it and translates its caret / selection / scroll
  anchor / decorations across the edit. The editing view already has the final
  positions and skips its own event.
- **`LineAnchor` / `LineMetadata`.** Stable per-line handles that survive
  insertions/deletions upstream (rather than raw line indices), plus a typed
  per-line metadata map. `programmatic_insert` already shifts frozen ranges,
  anchors, and metadata across an insertion in the agent-window path; a shipped
  cross-view broadcast would generalize that shifting to fire across *all* bound
  views, not just within one.

If both this and `spec-agent-window.md` ship, they share one `LineAnchor`
infrastructure; if only the agent window ships, the anchors stay scoped to it
(that spec's note E4). Until then, splitting a file gives correct shared *text*
but each view maintains its own caret independently with no cross-view shift —
acceptable for markdown editing today, the gap this section would close.

## §12 — Splits & focus: the Ctrl-W chord (cited by `main.rs:220`)

Window / split / layout management lives on a **`Ctrl-W` chord prefix**
(vim-window style), registered `GLOBAL` in `keymap_registry.rs` so it dispatches
on every screen. The full surface:

| Chord | Action | Behavior |
|---|---|---|
| `Ctrl-W s` | `SplitH` — split horizontally | 12 |
| `Ctrl-W v` | `SplitV` — split vertically | 12 |
| `Ctrl-W c` | `CloseWindow` — close tile | 13 |
| `Ctrl-W o` | `OnlyWindow` — close other tiles | 13 |
| `Ctrl-W h/l/k/j` | `FocusLeft/Right/Up/Down` | 9 |
| `Ctrl-W w` / `Ctrl-W Shift-W` | `FocusNext` / `FocusPrev` | 9 |
| `Ctrl-W < / -` | `ResizeShrink` | 13 |
| `Ctrl-W > / +` | `ResizeGrow` | 13 |
| `Ctrl-W =` | `Equalize` | 13 |
| `Ctrl-W m` | `MoveTile` — move tile to workspace | 18 |
| `Ctrl-W Shift-M` | `AlsoShowTile` — also-show in workspace | 19 |
| `Ctrl-W space` | `CycleLayoutMode` | 20 |
| `Ctrl-W p` | `DesktopTileSize` | `spec-desktop-mode.md` |
| `Ctrl-W enter` | `PromoteToMaster` | `spec-layout-patterns.md` |
| `Ctrl-W i` / `Ctrl-W d` | Increase / decrease master count | `spec-layout-patterns.md` |
| `Ctrl-W t` / `Ctrl-W Ctrl-T` / `Ctrl-W Shift-T` | Tag view / toggle / clear | `spec-layout-patterns.md` Phase 3 |

Tab / workspace switching lives on separate global chords (Behavior 5): **Cmd-T**
new, **Cmd-Shift-W** close, **Ctrl-Tab / Ctrl-Shift-Tab** and
**Cmd-Shift-] / [** cycle, **Ctrl-1..9,0** direct jump, **Cmd-J** jump panel.

> **macOS keystroke caveat.** `Ctrl`+digit and `Ctrl-Tab` are unreliable on
> macOS (the OS eats them), and `simulate_keystrokes` cannot catch that (it
> fabricates the ideal chord — the 4th verification gap in root `CLAUDE.md`).
> The `Cmd`-based tab bindings are the reliable path; the `Ctrl-W` split chord is
> delivered reliably as a prefix but the `Ctrl-<digit>` GotoWorkspace bindings
> carry this caveat.

## Verification

The split/close/focus/resize/detach/persist operations are pure functions on
`Layout<C>` / `Workspace<C>` and are unit-tested in `workspace.rs` tests +
`verify_harness.rs` (real view, real `Ctrl-W` bindings via
`register_keymap` + `simulate_keystrokes`, real persistence round-trips with the
`*_PATH_OVERRIDE` / `None`-under-`cfg(test)` seam so no test touches
`~/.yalda`). §10 has no code to verify; it is DRAFT.

## Revision history

- 2026-07-12 — Initial write. Retroactive spec for the already-shipped tabs /
  splits / pool / persistence model in `workspace.rs`, authored to resolve the
  ~15 dangling references to this file. Anchors fixed as a contract: Behaviors
  12–13 = splits, 23–24 = persistence; Constraint §11 = path canonicalization;
  §12 = the Ctrl-W chord surface. §10 (cross-view edit broadcast) documented as
  DRAFT / unshipped, matching `spec-agent-window.md`'s note E4. Content model
  (what a leaf holds) delegated to `spec-tiles-and-apps.md` + ADR-0019, not
  re-derived.
