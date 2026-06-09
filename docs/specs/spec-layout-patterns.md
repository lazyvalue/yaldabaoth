# Layout Patterns — Tags, Automatic Layouts, and Marks

**Status:** DRAFT
**Last updated:** 2026-06-06
**Builds on:** `spec-tabs-and-splits.md` (workspace / tab / layout tree), `spec-workspaces-tagging.md` (research — Option C adopted here), `spec-rail.md` (per-tab chrome)

## Overview

This spec adds three tiling-WM primitives to sketch's workspace model:

1. **Tags** — buffers carry named tags; tabs filter the buffer pool by tag.
   A buffer tagged `docs` and `ref` appears in any tab viewing either tag.
   Tags are the dwm/awesome "set-theoretic window management" idea, applied
   to the buffer pool rather than to layout nodes (per the Option C
   recommendation in `spec-workspaces-tagging.md`).

2. **Automatic layouts** — per-tab layout algorithms that arrange windows
   without manual splitting. Three modes: *master/stack* (one primary pane +
   N secondaries), *monocle* (one pane fullscreen, cycle through the rest),
   and *columns* (equal-width vertical splits). A fourth mode, *manual*,
   preserves the existing hand-built split tree. Switching modes re-tiles
   the tab's windows; switching back to manual restores the prior tree.

3. **Marks** — single-character named bookmarks on windows. `m` + key sets
   a mark; `'` + key jumps to the marked window (across tabs). Marks give
   stable O(1) access to frequently-used panes regardless of layout churn.

Together these turn sketch's workspace into a keyboard-driven tiling window
manager where the "applications" are file buffers, agent sessions, and file
browsers.

### Vocabulary

- **Tag** — a short string label (e.g., `docs`, `ref`, `agent`) carried by
  a `FileBuffer`. Tags are user-assigned, not content-derived. Tags are the
  membership mechanism; the layout tree is the geometry mechanism.
- **Tag view** — the set of tags a tab is currently displaying. A tab's view
  is a set of tag names; a buffer is visible iff its tag set intersects the
  view (or the view is empty, meaning "show everything").
- **Layout mode** — the algorithm a tab uses to arrange its windows:
  `Manual | MasterStack | Monocle | Columns`.
- **Mark** — a `char` key (a–z, A–Z) bound to a `WindowId`. Marks are
  workspace-global (cross-tab).

## Behaviors

### Tags

#### 1 · Tag assignment [DRAFT]

`:tag {name}` adds tag `{name}` to the focused window's underlying
`FileBuffer`. `:untag {name}` removes it. `:tag` with no argument lists the
focused buffer's current tags in the footer. Tag names are case-sensitive,
alphanumeric + hyphens, max 32 characters.

Only file-backed windows (`Doc`, `Edit`) can be tagged — their tags live on
the pooled `FileBuffer`. Agent and Browser windows are untaggable; `:tag`
on them prints a footer message: `agent/browser windows cannot be tagged`.

A buffer can carry any number of tags. Tags are not unique across
buffers — many buffers can share the tag `docs`.

#### 2 · Tag view per tab [DRAFT]

Each tab has a `tag_view: TagSet` (a set of tag names). When `tag_view` is
non-empty, the tab's layout shows only windows whose buffer carries at
least one tag in the view. Windows with untagged buffers, and Agent/Browser
windows, are hidden when a tag filter is active (they reappear when the
filter is cleared).

When `tag_view` is empty (the default), all windows are visible — no
filtering. This is the "show everything" state and is backwards-compatible
with today's behavior.

#### 3 · View tag [DRAFT]

`:view-tag {name}` sets the active tab's `tag_view` to `{name}` — show
only buffers tagged `{name}`. `:view-tag` with no argument clears the
filter (show all).

Keybinding: `Ctrl-W t` enters a one-key chord that reads the next
character as a tag-name shortcut (tags are mapped to single keys via
Behavior 6). `Ctrl-W T` clears the tag filter.

#### 4 · Toggle tag into view [DRAFT]

`:view-tag-toggle {name}` toggles `{name}` in the active tab's `tag_view`.
If the tag is already in the view, it is removed; if absent, added. This
is the union-view primitive: `:view-tag-toggle docs` then
`:view-tag-toggle ref` shows all buffers tagged either `docs` or `ref`.

When the view set becomes empty (all tags toggled off), the tab returns to
"show everything."

#### 5 · Tag-filtered layout [DRAFT]

When a tag view is active, the tab's layout mode (Behavior 11) arranges
only the visible (tag-matched) windows. Hidden windows are *not* removed
from the layout tree — they are skipped by the layout algorithm and the
render pass. This preserves their tree position for when the filter is
cleared or their tag is toggled back in.

For `Manual` mode: hidden windows' split slots collapse visually (their
weight is redistributed among visible siblings) but the tree structure is
unchanged. For automatic modes (`MasterStack`, `Monocle`, `Columns`): the
layout is computed over only the visible set.

Focus: if the currently focused window becomes hidden by a tag filter
change, focus moves to the first visible window in tree order. If no
windows are visible, the tab shows an empty state with a hint:
`no buffers match tags: {view}`.

#### 6 · Tag shortcuts [DRAFT]

`:tag-bind {key} {name}` maps a single character `{key}` to tag `{name}`.
After binding, `Ctrl-W t {key}` sets the view to that tag;
`Ctrl-W Ctrl-T {key}` toggles that tag in the view. Up to 9 shortcuts
(matching dwm's `Mod+[1-9]`); default bindings:

| Key | Tag |
|-----|-----|
| `1` | (first user-created tag) |
| `2` | (second) |
| … | … |

Tag shortcuts are workspace-global and persisted. If no shortcuts are
bound, `Ctrl-W t` falls back to a prompt in the footer asking for a tag
name.

#### 7 · Send to tag [DRAFT]

`:send-tag {name}` is shorthand for: tag the focused buffer with `{name}`,
then switch the active tab's view to `{name}`. This is dwm's
`Mod+Shift+[1-9]` — "move this window to that tag."

`:also-tag {name}` tags the focused buffer with `{name}` without changing
the view. This is dwm's `Mod+Ctrl+Shift+[1-9]` — "this window should
also appear in that tag."

#### 8 · Tag bar [DRAFT]

When any buffer in the workspace has tags, the tab strip gains a
**tag indicator row** — a thin bar below the tab strip showing the
available tags as clickable/pressable labels. Tags present in the active
tab's `tag_view` are highlighted (accent background); tags with buffers
but not in the view are dimmed; tags with no buffers are hidden.

The tag bar is chrome (fixed font size, does not scale with `text_scale`).
It is hidden when no buffers have tags (backwards-compatible: no visual
change until the user starts tagging).

### Automatic layouts

#### 9 · Layout mode per tab [DRAFT]

Each tab has a `layout_mode: LayoutMode` field. Four modes:

- **`Manual`** — the existing hand-built split tree. All `Ctrl-W s/v/c/o`
  operations work as today. This is the default.
- **`MasterStack`** — one primary (master) window on the left, remaining
  windows stacked vertically on the right. The master gets 60% width by
  default.
- **`Monocle`** — all windows occupy the full tab area. Only the focused
  window is visible; `j`/`k` (or `Ctrl-W w`/`Ctrl-W W`) cycle through
  the stack. A counter in the status bar shows position: `[2/5]`.
- **`Columns`** — all windows split into equal-width vertical columns.

#### 10 · Switching modes [DRAFT]

`:layout {mode}` sets the active tab's layout mode. `Ctrl-W Space` cycles
through modes in order: Manual → MasterStack → Monocle → Columns → Manual.

When switching *from* Manual to an automatic mode, the current tree is
saved in `Tab.saved_manual_layout`. When switching *back* to Manual, the
saved tree is restored (windows re-slotted by `WindowId`; windows that
were created while in automatic mode are appended as new leaves).

When switching between automatic modes, the window list is preserved and
re-arranged by the new algorithm. No tree is saved (automatic modes don't
have a meaningful manual tree to preserve).

#### 11 · MasterStack layout [DRAFT]

The master/stack layout divides the tab into two regions:

```
┌──────────────┬─────────┐
│              │ stack-1  │
│    master    ├─────────┤
│              │ stack-2  │
│              ├─────────┤
│              │ stack-3  │
└──────────────┴─────────┘
```

- The **master** region occupies the left portion (default 60% width,
  adjustable with `Ctrl-W h`/`Ctrl-W l` in 5% increments, clamped to
  [20%, 90%]).
- The **stack** region occupies the right portion; windows are tiled
  vertically with equal heights.
- The master window is the first in the tab's window list (tree order).

**Promote to master:** `Ctrl-W Return` swaps the focused window with the
current master window. The focused window moves to position 0; the former
master takes the focused window's position in the stack. This is dwm's
`Mod+Return`.

**Add/remove master slots:** `Ctrl-W i` increases the number of master
windows by one (stacked vertically in the master region); `Ctrl-W d`
decreases it (minimum 1). Default: 1 master window. This matches dwm's
`Mod+i`/`Mod+d`.

Window ordering: the window list for automatic layouts is the depth-first
tree-order of leaves in the layout tree. New windows (from `:split`,
`:open`, etc.) append to the end of the list and appear in the last stack
position.

#### 12 · Monocle layout [DRAFT]

All windows are fullscreen within the tab's content area. Only the focused
window is rendered. Navigation:

- `Ctrl-W w` / `Ctrl-W W` — cycle forward/backward (same as today's
  focus-next/prev, but in monocle the visual effect is a full window swap).
- `j`/`k` in monocle's key context — same cycle (vim-style).
- The status bar shows `[n/N]` (1-indexed position / total count).

Splitting (`:split`, `Ctrl-W s/v`) in monocle mode adds a new window to
the list and focuses it (it becomes the visible window). Closing
(`Ctrl-W c`) removes the focused window and shows the next.

#### 13 · Columns layout [DRAFT]

All windows are arranged as equal-width vertical columns:

```
┌────────┬────────┬────────┐
│        │        │        │
│  col-1 │  col-2 │  col-3 │
│        │        │        │
└────────┴────────┴────────┘
```

Each column is full-height. Columns are ordered by the window list.
`Ctrl-W h`/`Ctrl-W l` moves focus left/right between columns.
`Ctrl-W H`/`Ctrl-W L` swaps the focused window with its left/right
neighbor (reorder within the list).

#### 14 · Manual mode — no change [DRAFT]

Manual mode is the existing behavior: the user builds the split tree
with `Ctrl-W s/v/c/o` and the tree persists as-is. Layout algorithms
do not touch the tree. All existing workspace operations
(`split_focused`, `close_focused`, `resize_focused`, `equalize_focused`,
`focus_motion`, `only`) work exactly as specified in
`spec-tabs-and-splits.md`.

#### 15 · Automatic mode constraints [DRAFT]

In automatic modes (`MasterStack`, `Monocle`, `Columns`):

- **Split and close still work** — they add/remove windows from the
  window list and the layout re-tiles. The user does not need to think
  about tree geometry.
- **Resize** — `Ctrl-W h/l` adjusts the master ratio in MasterStack;
  no-op in Monocle; no-op in Columns (equal-width enforced).
  `Ctrl-W -/+/</>` are no-ops in automatic modes (weights are
  algorithm-controlled).
- **Equalize** (`Ctrl-W =`) — resets master ratio to 60% in MasterStack;
  no-op in others.
- **Only** (`Ctrl-W o`) — works as normal (closes all but focused).
- **Focus motion** — `Ctrl-W h/j/k/l` works spatially as usual (the
  automatic layout produces a real split tree that focus_motion navigates).

#### 16 · Layout mode in status bar [DRAFT]

The status bar shows the current layout mode with a dwm-style sigil:

| Mode | Sigil |
|------|-------|
| Manual | `[]=` |
| MasterStack | `[M]=` |
| Monocle | `[n/N]` |
| Columns | `\|\|\|` |

The sigil appears at the left end of the status bar, before the file path.

### Marks

#### 17 · Set mark [DRAFT]

`m` followed by a single character (`a`–`z`, `A`–`Z`) marks the focused
window. The mark maps the character to the window's `WindowId`. Setting a
mark that already exists overwrites it silently (the mark moves to the new
window). Marks are valid in Doc view (`SketchView` context) and Edit
normal mode — not in Insert mode (where `m` is a text character) or in
Agent/Browser views.

`:mark {key}` is the command-mode equivalent.

#### 18 · Jump to mark [DRAFT]

`'` (single quote) followed by a character jumps to the marked window. If
the marked window is in a different tab, the tab switches first, then
focus moves to the window. If the marked window no longer exists (closed),
the jump fails silently and the mark is cleared (stale mark GC).

`:jump {key}` is the command-mode equivalent.

#### 19 · Mark scope [DRAFT]

Marks are **workspace-global** — they span all tabs. A mark set in tab 1
is reachable from tab 3. This matches vim's uppercase-mark behavior
(global across buffers) and the tiling-WM "jump to any window" ethos.

Lowercase marks (`a`–`z`) and uppercase marks (`A`–`Z`) are in the same
namespace — 52 possible marks. No distinction in scope (vim distinguishes
local/global by case; sketch has no compelling reason to, since there's
one workspace).

#### 20 · Mark indicator [DRAFT]

A marked window shows its mark character in the window's status chrome —
e.g., a small `[a]` badge in the top-right corner of the pane, rendered
at chrome font size (does not scale with `text_scale`). The badge is
visible in all layout modes.

#### 21 · Mark persistence [DRAFT]

Marks are persisted in `workspace.json` as part of the workspace snapshot.
On restore, marks are re-bound by matching `WindowId`s from the restored
layout. Marks whose `WindowId` didn't survive restoration (e.g., a Claude
session that failed to resume and got a new id) are dropped.

#### 22 · Special marks [DRAFT]

Two marks are maintained automatically:

- **`'.'`** (dot) — the window where the last edit occurred. Updated on
  every text mutation in any `EditorView`. Jumping to `'.'` goes to the
  last-edited file, like vim's `'.`.
- **`'''`** (single-quote) — the window that was focused before the last
  cross-tab jump. `''` alternates between the current and previous window,
  like vim's `''`. Updated whenever a mark jump (Behavior 18) or tag view
  change (Behavior 3) causes a tab switch.

Special marks cannot be overwritten by `m` — the keys `.` and `'` are
reserved.

## Data model

### Tag additions to `FileBuffer`

```rust
/// A set of user-assigned tag names. Stored on the pooled FileBuffer so
/// tags survive window close/reopen and are shared across all views of
/// the same buffer.
pub type TagSet = BTreeSet<String>;

pub struct FileBuffer {
    pub id: FileBufferId,
    pub canonical_path: PathBuf,
    pub core: SharedCore,
    pub file_label: SharedString,
    pub refcount: usize,
    pub tags: TagSet,                  // ← new
}
```

### Tag view on `Tab`

```rust
pub struct Tab<C> {
    pub auto_name: String,
    pub display_name: Option<String>,
    pub layout: Layout<C>,
    pub focused: WindowId,
    pub rail: Option<RailState>,
    pub tag_view: TagSet,              // ← new: empty = show all
    pub layout_mode: LayoutMode,       // ← new
    pub saved_manual_layout: Option<Layout<C>>,  // ← new: saved tree
    pub master_ratio: f32,             // ← new: MasterStack ratio
    pub master_count: usize,           // ← new: MasterStack master slots
}
```

### Layout mode

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Manual,
    MasterStack,
    Monocle,
    Columns,
}

impl Default for LayoutMode {
    fn default() -> Self {
        LayoutMode::Manual
    }
}
```

### Marks

```rust
/// Workspace-global mark table. Maps single characters to WindowIds.
/// Stored on `Workspace<C>`, not per-tab.
pub struct MarkTable {
    marks: HashMap<char, WindowId>,
    /// The window where the last edit occurred (special mark '.').
    last_edit: Option<WindowId>,
    /// The window focused before the last cross-tab jump (special mark "'").
    prev_jump: Option<WindowId>,
}

impl MarkTable {
    pub fn set(&mut self, key: char, id: WindowId) {
        if key != '.' && key != '\'' {
            self.marks.insert(key, id);
        }
    }

    pub fn get(&self, key: char) -> Option<WindowId> {
        match key {
            '.' => self.last_edit,
            '\'' => self.prev_jump,
            c => self.marks.get(&c).copied(),
        }
    }

    /// Remove all marks pointing to windows not in `live_ids`.
    pub fn gc(&mut self, live_ids: &HashSet<WindowId>) {
        self.marks.retain(|_, id| live_ids.contains(id));
        if let Some(id) = self.last_edit {
            if !live_ids.contains(&id) { self.last_edit = None; }
        }
        if let Some(id) = self.prev_jump {
            if !live_ids.contains(&id) { self.prev_jump = None; }
        }
    }
}
```

### Tag shortcuts

```rust
/// Workspace-global mapping from shortcut keys to tag names.
pub type TagShortcuts = HashMap<char, String>;
```

### Workspace additions

```rust
pub struct Workspace<C> {
    pub tabs: Vec<Tab<C>>,
    pub active_tab: usize,
    pub file_buffers: HashMap<FileBufferId, FileBuffer>,
    pub path_index: HashMap<PathBuf, FileBufferId>,
    pub next_buffer_id: u64,
    pub next_window_id: u64,
    pub next_tab_index: usize,
    pub marks: MarkTable,              // ← new
    pub tag_shortcuts: TagShortcuts,   // ← new
}
```

## Interfaces

### Commands

| Command | Aliases | Effect |
|---|---|---|
| `:tag [name]` | — | Add tag to focused buffer, or list tags if no arg |
| `:untag {name}` | — | Remove tag from focused buffer |
| `:view-tag [name]` | `:vt` | Set tab's tag view (or clear if no arg) |
| `:view-tag-toggle {name}` | `:vtt` | Toggle a tag in the tab's view |
| `:send-tag {name}` | `:st` | Tag focused buffer + switch view to that tag |
| `:also-tag {name}` | `:at` | Tag focused buffer without changing view |
| `:tag-bind {key} {name}` | — | Map a shortcut key to a tag name |
| `:layout {mode}` | `:lo` | Set tab's layout mode (manual/master/monocle/columns) |
| `:mark {key}` | — | Set a mark on the focused window |
| `:jump {key}` | `:j` | Jump to a marked window |
| `:marks` | — | List all marks in the footer |

### Keybindings

| Binding | Context | Action |
|---|---|---|
| `Ctrl-W t {key}` | global | View tag bound to `{key}` (Behavior 6) |
| `Ctrl-W T` | global | Clear tag filter (show all) |
| `Ctrl-W Ctrl-T {key}` | global | Toggle tag bound to `{key}` in view |
| `Ctrl-W Space` | global | Cycle layout mode |
| `Ctrl-W Return` | MasterStack | Promote focused to master |
| `Ctrl-W i` | MasterStack | Increase master count |
| `Ctrl-W d` | MasterStack | Decrease master count |
| `m {key}` | SketchView, EditView (normal) | Set mark |
| `' {key}` | SketchView, EditView (normal) | Jump to mark |

### Persistence

`workspace.json` gains these fields:

```json
{
  "/Users/scott/ws/sketch": {
    "tabs": [
      {
        "auto_name": "tab-1",
        "layout": { "..." },
        "focused": 1,
        "tag_view": ["docs"],
        "layout_mode": "master_stack",
        "master_ratio": 0.6,
        "master_count": 1
      }
    ],
    "active_tab": 0,
    "marks": { "a": 3, "b": 7 },
    "tag_shortcuts": { "1": "docs", "2": "ref", "3": "agent" },
    "buffers": {
      "/Users/scott/ws/sketch/README.md": {
        "tags": ["docs", "ref"]
      }
    }
  }
}
```

Buffer tags are persisted in a separate `buffers` map keyed by canonical
path, independent of the layout tree. This means tags survive even when a
buffer has zero open views (refcount 0) — they are restored when the file
is reopened.

## Constraints

1. **Tags on buffers, not windows.** Tags live on `FileBuffer`, not on
   `Window<C>`. A tag applies to all views of a buffer across all tabs.
   This follows the Option C recommendation from
   `spec-workspaces-tagging.md` and matches vim/perspective.el semantics.

2. **Agent/Browser windows are untaggable.** They don't have a pooled
   buffer. They are always visible regardless of tag filter (Behavior 2
   hides them when a filter is active; a future revision could make them
   "tag-sticky" — always shown — if that proves more useful).

3. **Automatic layouts produce real trees.** `MasterStack`, `Monocle`, and
   `Columns` algorithms write standard `Layout<C>` trees (using `Split`
   and `Leaf` nodes) — they are not a separate render path. This means
   `focus_motion`, `find_leaf`, `path_to`, and all existing tree-walking
   code works unchanged. The difference is that the tree is
   algorithm-generated rather than user-built.

4. **Manual tree preservation.** Switching from Manual to an automatic
   mode saves the tree; switching back restores it. The saved tree is
   persisted in `workspace.json` so it survives restart. Windows created
   during automatic mode that don't exist in the saved tree are appended
   as new leaves on restore.

5. **Marks are cross-tab.** The mark table lives on `Workspace<C>`, not
   on `Tab<C>`. Jumping to a mark in another tab switches tabs first.

6. **No union-view tree merge.** Tag views show buffers from multiple tags
   by *filtering* the existing window list, not by merging two layout
   trees into one. The union happens at the membership level (buffer tags
   intersect the view set), not the geometry level. This avoids the
   intractable tree-merge problem identified in
   `spec-workspaces-tagging.md`.

7. **Tag view is a filter, not a workspace.** Applying a tag view does not
   create a new tab or rearrange the tree — it hides non-matching windows
   in the current tab. This is lighter than dwm's model (which remembers
   per-tag geometry) but avoids the complexity of per-tag layout state.
   If per-tag geometry proves necessary, the path is: one tab per tag,
   with derived membership (Phase 1 from `spec-workspaces-tagging.md`).

8. **Out of scope.** Tag-based auto-population ("open all `docs`-tagged
   files as views in a new tab"), per-tag layout memory, tag
   rename/delete commands, mouse interaction with the tag bar, and tag
   inheritance (new files in a tagged directory auto-inherit tags). These
   can be added without structural changes.

9. **Backwards compatibility.** All three features are additive. A
   workspace with no tags, in Manual layout mode, and no marks behaves
   identically to today. No existing keybinding conflicts — `m` and `'`
   are currently unbound in SketchView/EditView normal mode; `Ctrl-W t`,
   `Ctrl-W T`, `Ctrl-W Space`, `Ctrl-W Return`, `Ctrl-W i`, `Ctrl-W d`
   are currently unbound.

10. **Performance.** Tag filtering is O(leaves) per render frame (check
    each leaf's buffer tags against the view set). Automatic layout
    algorithms are O(leaves) per structural mutation (not per frame —
    the tree is rebuilt on split/close/mode-switch, not on every paint).
    Mark lookup is O(1) (HashMap). None of these are hot-path concerns
    for the expected window counts (< 50 leaves per tab).

## Implementation phasing

### Phase 1 — Marks (smallest, self-contained)

Add `MarkTable` to `Workspace<C>`, wire `m`/`'` key chords in SketchView
and EditView normal mode, add `:mark`/`:jump`/`:marks` commands, persist
in `workspace.json`. No data model changes to `Tab` or `FileBuffer`.

### Phase 2 — Automatic layouts

Add `LayoutMode`, `saved_manual_layout`, `master_ratio`, `master_count`
to `Tab<C>`. Implement the three layout algorithms as functions that take
a window list and produce a `Layout<C>` tree. Wire `Ctrl-W Space` and
`:layout`. Add status-bar sigil.

### Phase 3 — Tags

Add `tags: TagSet` to `FileBuffer`, `tag_view: TagSet` to `Tab<C>`.
Implement tag commands, tag-bar chrome, tag shortcuts, and the
visibility-filtering render path. This is the largest phase because it
touches the render pipeline (hiding/showing windows) and adds new chrome.

## Revision history

- 2026-06-06 — Initial draft.
