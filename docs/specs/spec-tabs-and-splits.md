# Tabs and Splits (GPUI Workspace Model)

**Status:** PARTIAL — workspace data model, tabs, splits, focus motion, resize, and persistence SHIPPED; shared buffer pool (Behaviors 9–11, 21–22) and `SessionRing` dissolution (Constraint §2) DRAFT.

**Last updated:** 2026-05-13

## Builds On

- **`spec-multi-session.md`** — Multi-session is the prior art for running several Claude agents at once. This spec **supersedes its sidebar (§9–§14)** and **dissolves the `SessionRing` data structure**: each Claude session becomes a `ClaudeWindow` inside some tab's layout tree, and the session-list UI is replaced by the workspace tab strip. The session *lifecycle* (`session/new`, `session/load`, detach/attach, persistence of `cwd → [session-ids]`) is retained — it shifts from `SessionRing` ownership into `ClaudeWindow` ownership. `spec-multi-session.md` §15 (`resume_id` stability across `session/load` fallback) is unshipped; this spec inlines its contract for `ClaudeWindow` rather than depending on it landing first (see Behavior 21).

## Overview

Sketch's GPUI frontend currently renders a single full-width `Screen` (one of `Doc | Edit | Claude | Browser`) and switches between open files via a flat `open_buffers` list. This spec replaces that with a tabbed workspace where each tab is the root of a tree of vim-style window splits, and any window can host any content kind.

Splits act like vim: a horizontal split adds a window above/below the focused window in a stacked region; a vertical split adds one to the left/right. Splits nest arbitrarily. Closing a window prunes it from the tree and collapses any resulting single-child split into its child. Focus moves between windows with `Ctrl-W h|j|k|l`.

The spec introduces six named artifacts:

1. **Workspace** — top-level GPUI App container. Owns the tab strip, the file-buffer pool, and the active tab pointer.
2. **Tab** — one entry in the tab strip. Owns a `Layout` tree, a focused-window pointer, and a user-facing name.
3. **Window** — a leaf in a `Layout`. Holds one of four content kinds (`DocWindow`, `EditWindow`, `BrowserWindow`, `ClaudeWindow`) and a stable `WindowId`.
4. **Layout** — an n-ary split tree: `Leaf(Window) | Split { dir, children: Vec<(Layout, weight)> }`. A `Split` holds ≥2 children; pruning that would leave 1 child collapses the split.
5. **EditorCore** — the shared document substrate for a single file path: rope text, undo/redo stack, frozen-line ranges, file path, dirty flag. Pooled in `Workspace.file_buffers` (keyed by canonical path). One `EditorCore` may have many active `EditorView`s.
6. **EditorView** — per-window cursor, selection anchor, search state, and lockable-through-line bookmark for the `*claude*` flow. An `EditorView` holds a `FileBufferId` and reaches into the pooled `EditorCore` to mutate. Mutations route through the core, which broadcasts position-shift events to all sibling views so cursors stay consistent across windows.

Today's `Editor` (~920 LOC) splits along this seam: the document-and-undo half becomes `EditorCore`, the cursor-and-selection half becomes `EditorView`. The split is a core-module refactor consumed by both frontends — see Constraints §1 and §3 for scope and TUI-compatibility rules. Outside the editor module, this spec is GPUI-only.

Buffers follow a hybrid model: file-backed `EditorCore`s live in the global `Workspace.file_buffers` pool and may be referenced by many `EditorView`s simultaneously (cross-window shared edits). `ClaudeState` and `FileBrowser` are window-owned and exclusive — they live inside their window and die with it. (`ClaudeState`'s transcript editor is an `EditorCore` + `EditorView` pair that is not pooled — exclusive to its window.)

## Behaviors

### Workspace lifecycle

1. **Bootstrap.** [SHIPPED] On launch, the GPUI app constructs a `Workspace` with exactly one tab containing one window. The window's initial content matches today's startup behavior — `Doc` over the file the user opened, or `Browser` if launched into a directory. Tab is auto-named (`tab-1`).

2. **Quit.** [DRAFT] Closing the last tab does not auto-quit — the workspace shows an empty placeholder tab (one window, `Browser` rooted at cwd). Quitting the app is `:q` / `Cmd-Q` as today.

### Tab lifecycle

3. **Create.** [SHIPPED] `:tabnew [path]` and `Cmd-T` (or menu: `Space → w → t`) create a new tab. With no argument, the new tab contains one `BrowserWindow` rooted at cwd. With a path argument, it contains one `DocWindow` over the (newly-pooled, if needed) `EditorCore`. The new tab is auto-named (`tab-{N}` with monotonic `N`, stored in `Tab.auto_name`) and becomes active.

4. **Switch.** [SHIPPED] `:tabnext` / `:tabprev` (`Ctrl-Tab` / `Ctrl-Shift-Tab`, menu: `Space → w → ]` / `Space → w → [`) cycle through tabs. Clicking a tab in the strip switches to it. Switching restores the tab's last-focused window.

5. **Close.** [SHIPPED] `:tabclose` (`Cmd-Shift-W` from inside the tab, menu: `Space → w → x`) closes the active tab. All windows in the tab are closed in tree order — `ClaudeWindow`s drop their `AcpChannelClient`s (subprocess killed); `BrowserWindow`s drop their `FileBrowser`s; `DocWindow` / `EditWindow` instances drop their `EditorView` and decrement the referenced `EditorCore`'s refcount. An `EditorCore` is removed from the pool when its refcount reaches zero AND it has no unsaved changes; dirty cores remain in the pool with zero references (recoverable via `:buffers`). Per behavior 2, closing the last tab opens an empty placeholder, not the app.

6. **Rename.** [DRAFT] `:tabrename {name}` sets the active tab's `display_name`. Names are cosmetic; the strip falls back to `auto_name` when `display_name` is empty.

### Window content

7. **Window kinds.** [DRAFT] A `Window` carries one of:

   - **`DocWindow { view: EditorView, scroll_handle, cursor_block }`** — rendered markdown view of a file. The `EditorView` references the pooled `EditorCore` by `FileBufferId`.
   - **`EditWindow { view: EditorView, mode, keybinds, scroll_handle, edit_view, last_save_msg }`** — raw/edit view of a file. `edit_view: Code | WordProcessor`. Same buffer-sharing rules.
   - **`BrowserWindow { fb: FileBrowser }`** — owns its `FileBrowser` exclusively. Selecting a file replaces the window's content with a `DocWindow` over the chosen `EditorCore` (pooled on demand). The `BrowserWindow` is consumed.
   - **`ClaudeWindow { state: ClaudeState, resume_id: Option<String>, load_outcome: LoadOutcome }`** — owns one ACP session exclusively. `state` is today's `ClaudeState` (channel, transcript editor, tool calls, compose box, turn timer). `resume_id` is the persisted id this window was created from; `load_outcome` is one of `Fresh | Resumed | FallbackToNew` (set on creation/restore, see Behavior 21).

8. **Content kind transitions.** [DRAFT] A window can swap kinds in place without changing position or `WindowId`:

   - `DocWindow ↔ EditWindow` over the same `FileBufferId` — bound to `:edit-toggle` (default chord `Ctrl-E`; rebound from today's `Ctrl-W` because `Ctrl-W` becomes the split chord-prefix in this spec). Both kinds share the same `EditorCore`; only the new window's `EditorView` is constructed (cursor inherits from the old one).
   - `BrowserWindow → DocWindow` — when the user selects a file. The old `BrowserWindow` is consumed.
   - `DocWindow | EditWindow → BrowserWindow` — fallback path when `:bdelete` removes the underlying `EditorCore`. The new browser is rooted at the deleted file's parent dir.

   `ClaudeWindow` cannot transition into another kind in place: opening a doc inside a Claude window's tab means splitting (`:split path`) or creating a separate Claude window (`:claude-new`), not replacing the Claude window's content.

### Cross-window buffer sharing

9. **Cursor and selection are per-view.** [DRAFT] Each `DocWindow` and `EditWindow` owns its `EditorView`. Two windows over the same `FileBufferId` have independent cursors, selections, search positions, and (for the `*claude*` flow) `lockable_through_line` bookmarks. Switching focus between windows changes only which `EditorView` receives keystrokes; the underlying `EditorCore` is unchanged.

10. **Edit propagation.** [DRAFT] When an `EditorView` mutates its `EditorCore` (insert, delete, programmatic splice, undo, redo), the core notifies every sibling view registered against it. Each sibling view runs a shift function over its cursor and selection anchor: positions before the edit are unchanged; positions inside the replaced range collapse to the edit's start; positions after the edit shift by `inserted_len - deleted_len`. The shift function is the same algorithm undo uses to restore cursor positions today (`editor.rs:773, 789`).

    Frozen-range tracking lives in `EditorCore` and is broadcast the same way: when `splice_claude_chunk` extends a frozen region, all sibling views see the new boundaries simultaneously.

11. **Undo contract.** [DRAFT] The undo stack is **shared** at the `EditorCore` level (one stack per file). Any `EditorView` over a given core can invoke undo/redo and the core unwinds its own most-recent group regardless of which view created it. Two consequences:

    - **No edit attribution.** Window B can undo a keystroke Window A typed. This matches how a single tail-spliced log behaves and avoids the per-view undo stacks problem (which would require a CRDT-ish merge to remain consistent).
    - **Cursor jumps to the undone edit.** When undo restores a position recorded in the group, the position writes to whichever view is currently focused on this core (the view that invoked undo). Sibling views' cursors shift per Behavior 10 if the undo crosses their cursor position, but the focused view's cursor lands on the restored mark. This matches vim's single-cursor-per-buffer behavior — the focused window "owns" the visible jump.

    Disallowing two writable windows on the same core is out of scope (rejected — kills the side-by-side editing use case).

### Layout — splits, close, resize

12. **Split.** [SHIPPED] From a focused leaf window:

   - `:split [path]` / `Ctrl-W s` adds a horizontal split — a new leaf below the focused one.
   - `:vsplit [path]` / `Ctrl-W v` adds a vertical split — a new leaf to the right.
   - With a `path` argument, the new window is a `DocWindow` over the (pooled) `EditorCore`. Without, it's a `DocWindow` cloning the focused window's content (same buffer, new `EditorView` with a fresh cursor at the same position as the source); if the focused window is a `ClaudeWindow` or `BrowserWindow`, the clone-without-path form opens a `BrowserWindow` rooted at cwd instead.
   - The new window inherits the parent split's direction if compatible; otherwise the focused leaf is wrapped in a new `Split` with two children.
   - **Creating a `ClaudeWindow` via split:** `:claude-new` (carried over from `spec-multi-session.md`) now splits the focused tab — vertical split by default, configurable — and inserts a new `ClaudeWindow` on the new leaf. `:claude-attach <id>` likewise splits and inserts a `ClaudeWindow` bound to the given session id.

13. **Split with children count.** [SHIPPED] Splits are always n-ary with `children.len() >= 2`. Adding a window adjacent to an existing split of the same direction appends to that split's children (no nesting). Adding a window in the perpendicular direction creates a new nested split. Initial weight for a newly inserted child is the average of its siblings; existing siblings are renormalized proportionally so weights still sum to 1.0.

14. **Close window.** [SHIPPED] `:close` / `Ctrl-W c` removes the focused leaf. The pruning algorithm:

    1. Remove the leaf from its parent split's `children`.
    2. If the parent split now has 1 child, replace the parent with that child (collapse).
    3. If the parent split has 0 children — impossible by invariant (a split always had ≥2 children, removing one leaves ≥1).
    4. Renormalize sibling weights to sum to 1.0.
    5. Per behavior 5, removing the last window in the tab triggers tab-close.

    Focus moves to the spatially-nearest sibling (heuristic: previous index in the parent split's `children`, else first remaining child of the grandparent).

15. **Only.** [SHIPPED] `:only` / `Ctrl-W o` closes every window in the tab except the focused one. The focused leaf becomes the tab's root `Layout`.

16. **Resize.** [SHIPPED] `Ctrl-W <` / `Ctrl-W >` / `Ctrl-W -` / `Ctrl-W +` shift weight between the focused window and its immediate sibling within the parent split (5% of the parent's allocation per keypress). Resizing across splits (e.g., resizing the parent split's outer boundary) is out of scope; weights apply only inside a single `Split` node. Mouse-drag resize is **out of scope for v1** (see Constraint §8).

17. **Equalize.** [SHIPPED] `Ctrl-W =` resets all weights in the focused window's parent split to equal. Does not recurse into nested splits.

### Focus

18. **Focused window per tab.** [SHIPPED] Each tab tracks `focused: WindowId`. Switching tabs restores that tab's focused window. Creating a new window (split, browser-open, etc.) moves focus to the new window. Closing the focused window moves focus per behavior 14.

19. **Vim-style focus motions.** [PARTIAL — topological motion shipped; screen-rect cache is a follow-up] `Ctrl-W h | j | k | l` moves focus to the spatially-nearest window in that direction. The algorithm uses **cached leaf screen-rects** populated by the paint pass (`Tab.last_paint_rects: HashMap<WindowId, Rect>`); a keypress never triggers a layout pass. The algorithm projects each leaf rect onto the screen, picks the leaf whose center is closest along the target axis among those that overlap the focused leaf's perpendicular extent, and breaks ties by most-recently-focused. If no overlapping leaf exists in that direction, focus does not move. `Ctrl-W w` cycles forward through windows in tree order; `Ctrl-W W` cycles backward.

20. **Key dispatch within a window.** [SHIPPED] After workspace-level shortcuts (tab switching, `Ctrl-W <motion>`, etc.) are matched, all other keys route to the focused window's content kind, which dispatches to the existing handlers (`handle_doc_key`, `handle_edit_key`, `handle_claude_key`, `handle_browser_key` — unchanged in semantics).

### Buffer pool semantics

21. **EditorCore pool.** [DRAFT] `Workspace.file_buffers: HashMap<FileBufferId, FileBuffer>` is the global pool. `FileBuffer` wraps one `EditorCore` plus the refcount of `EditorView`s pointing at it.

    - **Open.** Opening a path (`:open path`, browser select, `:split path`) canonicalizes the path with `std::fs::canonicalize` for existing files; for files that don't yet exist (a new file), the absolute path (`cwd.join(path)` with `..` resolution) is used as the canonical key. The pool is searched by canonical key; if a `FileBuffer` exists, its id is returned; otherwise a new `EditorCore` is constructed and pooled.
    - **Share semantics.** Two windows pointing at the same `FileBufferId` hold independent `EditorView`s. Inserts, deletes, undo/redo from any view mutate the shared `EditorCore`; the core broadcasts a position-shift event (Behavior 10) so all sibling views' cursors and anchors stay consistent.
    - **Close.** A `FileBuffer` is dropped from the pool when its refcount hits 0 AND `core.is_dirty() == false`. Dirty cores persist with refcount 0; `:buffers` lists them, and they can be re-bound via `:buffer {id}`.
    - **Delete.** `:bdelete` removes a buffer regardless of refcount or dirty state. Affected windows transition per Behavior 8 (`DocWindow | EditWindow → BrowserWindow` rooted at the deleted file's parent dir). Footer logs `:bdelete <path>: N windows reset`.

22. **Buffer listing.** [DRAFT] `:buffers` / menu: `Space → b → l` lists every `FileBuffer` in the pool with id, canonical path, dirty marker, and refcount. Selecting one replaces the focused window's content with a `DocWindow` over that buffer.

### Persistence

23. **Workspace persistence.** [SHIPPED — every structural mutation triggers a write; focus-change debounce is a follow-up] On change, the workspace is serialized to `~/.sketch/workspace.json` keyed by `cwd`. Format:

    ```json
    {
      "/Users/scott/ws/sketch": {
        "tabs": [
          {
            "auto_name": "tab-1",
            "display_name": null,
            "focused_window": 3,
            "layout": {
              "split": {
                "dir": "v",
                "children": [
                  { "weight": 0.5, "layout": { "leaf": { "id": 1, "kind": { "doc": { "path": "/Users/scott/ws/sketch/README.md" } } } } },
                  { "weight": 0.5, "layout": { "leaf": { "id": 3, "kind": { "claude": { "session_id": "ses_abc123" } } } } }
                ]
              }
            }
          }
        ],
        "active_tab": 0
      }
    }
    ```

    Saved per-leaf state:

    - `DocWindow` / `EditWindow` — canonical file path. Viewport state (scroll position, cursor) is **not** persisted (resets to top on restore).
    - `ClaudeWindow` — session id (the `resume_id` rule below).
    - `BrowserWindow` — current_dir only.

    **Write trigger and debouncing.** Structural changes (tab add/remove/rename, window split/close, content kind swap) write immediately. Focus changes (active tab, focused window) are coalesced behind a 250ms debounce so rapid `Ctrl-W h|j|k|l` or `Ctrl-Tab` runs produce at most one write per quiescent period. Best-effort writes, silent failure, last-writer-wins for concurrent sketch instances on the same cwd.

24. **Restore.** [PARTIAL — Doc / Edit / Browser leaves restore; Claude leaves come back as Browser stubs and the user reattaches via the existing Claude commands] On launch, the workspace loader reads `workspace.json[cwd]`. Each leaf reconstructs:

    - `DocWindow` / `EditWindow` — opens the canonical path into the buffer pool (pooled on first reference; subsequent references share). If the file is missing or unreadable, the leaf is replaced with a `BrowserWindow` rooted at the file's parent dir and a one-line message lands in the footer.
    - `ClaudeWindow` — spawns an `AcpChannelClient` with `session/load(session_id)`. On load success, `load_outcome = Resumed`. On load failure, the leaf survives — the channel falls back to `session/new`; `load_outcome = FallbackToNew`, `resume_id` is **preserved unchanged** so the next reboot retries the original id. The window header shows a `[new]` suffix after its label (e.g., `claude-1 [new]`) and the footer logs `claude-1: session not resumable, started fresh`. This rule is the same as `spec-multi-session.md` §15 — inlined here because that contract is unshipped.
    - `BrowserWindow` — constructs a `FileBrowser` rooted at the saved dir. If the dir is missing, falls back to cwd.

    A missing or unparseable workspace file produces the bootstrap state (behavior 1).

## Data Model

```rust
pub struct Workspace {
    tabs: Vec<Tab>,
    active_tab: usize,
    file_buffers: HashMap<FileBufferId, FileBuffer>,
    path_index: HashMap<PathBuf, FileBufferId>,   // canonical path → id
    next_buffer_id: u64,
    next_window_id: u64,
    next_tab_index: usize,                        // monotonic for tab auto-naming
}

pub struct Tab {
    auto_name: String,                            // "tab-{N}", set at create
    display_name: Option<String>,                 // user-set via :tabrename; falls back to auto_name
    layout: Layout,
    focused: WindowId,
    last_paint_rects: HashMap<WindowId, Rect>,    // cached by the paint pass; read on Ctrl-W h|j|k|l
}

pub enum Layout {
    Leaf(Window),
    Split {
        dir: SplitDir,                       // H | V
        children: Vec<(f32, Layout)>,        // (weight, child); weights sum to 1.0
    },
}

pub struct Window {
    id: WindowId,
    content: WindowContent,
}

pub enum WindowContent {
    Doc(DocWindow),
    Edit(EditWindow),
    Browser(BrowserWindow),
    Claude(ClaudeWindow),
}

pub struct DocWindow {
    view: EditorView,
    scroll_handle: ScrollHandle,
    cursor_block: usize,
}

pub struct EditWindow {
    view: EditorView,
    mode: EditMode,
    keybinds: KeybindManager,
    scroll_handle: ScrollHandle,
    edit_view: EditView,                          // Code | WordProcessor
    last_save_msg: Option<SharedString>,
}

pub struct BrowserWindow { fb: FileBrowser }

pub enum LoadOutcome { Fresh, Resumed, FallbackToNew }

pub struct ClaudeWindow {
    state: ClaudeState,                           // existing struct, unchanged
    resume_id: Option<String>,
    load_outcome: LoadOutcome,
}

pub struct FileBuffer {
    id: FileBufferId,
    canonical_path: PathBuf,
    core: EditorCore,
    file_label: SharedString,
    refcount: usize,                              // active EditorViews referencing this core
}

pub struct EditorCore {
    document: Document,                           // rope + line cache + frozen ranges + file path
    undo_stack: UndoStack,
    dirty: bool,
    view_subscriptions: SlotMap<ViewSubId, ()>,   // tokens for sibling-shift broadcast
}

pub struct EditorView {
    buffer: FileBufferId,
    sub_id: ViewSubId,                            // registration handle in EditorCore
    cursor: Cursor,
    selection_anchor: Option<Cursor>,
    search_state: Option<SearchState>,
    lockable_through_line: Option<usize>,         // *claude* editable-region bookmark, per-view
}

pub type FileBufferId = u64;
pub type WindowId = u64;
pub type ViewSubId = u64;

pub enum SplitDir { H, V }
```

### Relation to existing GPUI state

- `Screen` enum is removed. `App.screen: Screen` becomes `App.workspace: Workspace`.
- `App.open_buffers: Vec<OpenBuffer>` is removed. The buffer pool subsumes it.
- `SessionRing` is removed (per `spec-multi-session.md` revision — see Constraint §2). `ClaudeState` is unchanged.
- `DocState`, `EditState`, `BrowserScreen` are renamed to `DocWindow`, `EditWindow`, `BrowserWindow` and their `Editor` field becomes an `EditorView` pointing at a pooled `EditorCore`.

### Relation to today's `Editor`

`Editor` (today, ~920 LOC) is split at the cursor/document seam:

- Document (rope), undo stack, frozen ranges, dirty flag, file path → `EditorCore`.
- Cursor, selection anchor, search state, `lockable_through_line` → `EditorView`.

Mutation methods on `Editor` today (`insert_char`, `backspace`, `delete_char_at_cursor`, `delete_current_line`, `open_line_below/above`, `undo`, `redo`, `select_all`, …) move to `EditorView` and take `&mut EditorCore`:

```rust
impl EditorView {
    pub fn insert_char(&mut self, core: &mut EditorCore, c: char) { ... }
    pub fn undo(&mut self, core: &mut EditorCore) { ... }
    // ...
}
```

The TUI's `Buffer` holds one `EditorCore` and one `EditorView` together (`Buffer { core, view, … }`), preserving today's 1:1 buffer-to-cursor relationship. TUI call sites that today read `buffer.editor.cursor()` become `buffer.view.cursor()`; sites that mutate (`buffer.editor.insert_char(c)`) become `buffer.view.insert_char(&mut buffer.core, c)` (a tiny wrapper on `Buffer` can preserve the old surface).

## Interfaces

Workspace API (DRAFT, GPUI-only, in `src/bin/sketch-gpui.rs` or a new `src/bin/sketch-gpui/workspace.rs`):

- `Workspace::new(cwd: &Path) -> Self` — bootstrap with one tab, one window.
- `Workspace::active_tab(&self) -> &Tab` / `active_tab_mut`
- `Workspace::focused_window(&self) -> &Window` / `focused_window_mut`
- `Workspace::open_buffer(&mut self, path: &Path) -> FileBufferId` — canonicalize, pool lookup or insert.
- `Workspace::buffer(&self, id: FileBufferId) -> Option<&FileBuffer>` / `buffer_mut`
- `Workspace::with_core_and_view(&mut self, view: &mut EditorView, f: impl FnOnce(&mut EditorCore, &mut EditorView))` — borrow helper that resolves a view's core from the pool and passes both to the closure (needed because `EditorView`'s mutation methods take `&mut EditorCore` separately).
- `Workspace::split(&mut self, dir: SplitDir, content: WindowContent)` — split the focused window.
- `Workspace::close_focused(&mut self)` — apply behavior 14.
- `Workspace::only(&mut self)` — apply behavior 15.
- `Workspace::focus_motion(&mut self, dir: FocusDir)` — apply behavior 19.
- `Workspace::resize_focused(&mut self, delta: f32)` — apply behavior 16.
- `Workspace::new_tab(&mut self, content: WindowContent)` / `close_tab(idx)` / `next_tab()` / `prev_tab()` / `rename_tab(idx, name)`
- `Workspace::save(&self, cwd: &Path)` / `Workspace::load(cwd: &Path) -> Option<Self>` — persistence (behaviors 23–24).

EditorCore / EditorView API (DRAFT, in `src/editor.rs` and shared by both frontends):

- `EditorCore::new(text: String, path: PathBuf) -> Self`
- `EditorCore::subscribe(&mut self) -> ViewSubId` / `unsubscribe(sub_id)`
- `EditorCore::iter_views(&self) -> impl Iterator<Item = ViewSubId>` — used by mutation paths to enumerate siblings for shift broadcast.
- `EditorView::new(buffer: FileBufferId, sub_id: ViewSubId) -> Self`
- `EditorView::cursor() -> Cursor` / `selection() -> Option<(Cursor, Cursor)>` / `set_cursor(Cursor)`
- `EditorView::insert_char(&mut self, core: &mut EditorCore, c: char)` — mutates core, calls `core.broadcast_shift(edit_event, self.sub_id)` to update siblings.
- `EditorView::shift_for_remote_edit(&mut self, evt: EditEvent)` — sibling shift handler (Behavior 10).
- (… and the rest of today's `Editor` mutation methods, moved over.)

Command table (DRAFT):

| Command | Aliases | Effect |
|---|---|---|
| `:tabnew [path]` | — | Create new tab (behavior 3) |
| `:tabnext` | `:tn` | Next tab |
| `:tabprev` | `:tp` | Previous tab |
| `:tabclose` | `:tc` | Close active tab (behavior 5) |
| `:tabrename <name>` | — | Rename active tab |
| `:split [path]` | `:sp` | Horizontal split (behavior 12) |
| `:vsplit [path]` | `:vsp` | Vertical split |
| `:close` | — | Close focused window (behavior 14) |
| `:only` | — | Keep only focused window (behavior 15) |
| `:bdelete [id]` | `:bd` | Drop a buffer (behavior 21) |
| `:buffer <id>` | `:b` | Bind focused window to a pooled buffer by id |
| `:buffers` | `:ls` | List pooled buffers (behavior 22) |
| `:edit-toggle` | — | Swap focused window between Doc/Edit on the same buffer |
| `:wp-toggle` | — | Cycle `EditWindow.edit_view` Code → WordProcessor → Code |

Keybind table (DRAFT, split chord-prefix `Ctrl-W`):

| Binding | Action |
|---|---|
| `Ctrl-W s` | Horizontal split |
| `Ctrl-W v` | Vertical split |
| `Ctrl-W c` | Close window |
| `Ctrl-W o` | Only |
| `Ctrl-W h\|j\|k\|l` | Focus motion |
| `Ctrl-W w\|W` | Cycle focus forward/backward |
| `Ctrl-W <\|>\|-\|+` | Resize focused vs. sibling |
| `Ctrl-W =` | Equalize parent split |
| `Cmd-T` | New tab |
| `Cmd-W` | `:close` (single window) |
| `Cmd-Shift-W` | `:tabclose` (active tab) |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | Next / prev tab |
| `Ctrl-E` | `:edit-toggle` (Doc ↔ Edit on focused window) |
| `Ctrl-Shift-E` | `:wp-toggle` (Code ↔ WordProcessor in EditWindow) |

`Cmd-T`, `Cmd-W`, `Cmd-Shift-W`, `Ctrl-Tab`, `Ctrl-Shift-Tab`, `Ctrl-E`, `Ctrl-Shift-E` are registered as **app-global GPUI actions** so menus, overlays, and the compose box do not swallow them.

Menu (DRAFT, new top-level `w` submenu — "windows"):

- `Space → w → s` — horizontal split
- `Space → w → v` — vertical split
- `Space → w → c` — close window
- `Space → w → t` — new tab
- `Space → w → x` — close tab
- `Space → w → ]` / `[` — next / prev tab

## Constraints

1. **Scope.** The workspace, tab, and window model (Workspace, Tab, Layout, Window content kinds, focus motions, tab strip) is GPUI-only — the TUI (`src/app/`) is untouched. The `EditorCore` / `EditorView` split is a **core-module refactor** in `src/editor.rs` consumed by both frontends; the TUI keeps a 1:1 view-per-buffer pairing inside `Buffer` (Behavior 9 applies only when more than one view exists, which the TUI never produces today). TUI behavioral parity is required — see Constraint §3.

2. **`spec-multi-session.md` revision.** Adopting this spec requires revising `spec-multi-session.md`: §9 (sidebar), §10 (chat body refers to ring), §11–§12 (header/footer), §13–§14 (session-level keys / menu entries) are superseded by the tab strip + workspace key dispatch defined here. §1–§8 (session lifecycle), §16–§17 (reboot/clear), and §18 (soft cap) remain valid, lifted into `ClaudeWindow`. §15's `resume_id` semantics are inlined into Behavior 24 because §15 is unshipped. The `SessionRing` data structure is removed. The revision is part of this spec's implementation — `spec-multi-session.md` cannot be left contradicting it.

3. **`EditorCore` / `EditorView` refactor — TUI parity.** Splitting today's `Editor` (~920 LOC, ~87 `self.cursor` references, undo groups storing cursor positions at `editor.rs:773, 789`) into core + view is a large refactor touching ~50 method signatures and ~117 call sites across 9 files (TUI handlers, claude pipeline, command/keybind dispatch). The constraint is **TUI behavioral parity**: every TUI test must pass unchanged after the split. The TUI uses `Buffer { core, view }` 1:1 and a thin wrapper preserves old call-site ergonomics (`buffer.insert_char(c)` → `buffer.view.insert_char(&mut buffer.core, c)`). This refactor is the prerequisite landing change for the workspace spec; the workspace work cannot start until the editor split is stable on `main`.

4. **No window-local view state in persistence.** Scroll position and cursor are not persisted across restarts. This avoids a fragile "restore cursor at byte offset N" path that breaks when files change on disk. Re-opening a tab restores the file but starts at the top.

5. **Dirty buffer recovery.** An `EditorCore` with unsaved changes and refcount 0 stays in the pool until `:bdelete` removes it explicitly. The user can re-bind a window to it via `:buffers`. There is no auto-save and no swap file; this is the same loss model as today's `open_buffers`.

6. **N-ary tree invariants.** A `Split` always has `children.len() >= 2`. Insertions adjacent to a same-direction split append to that split; perpendicular insertions create a nested split. The `close-and-collapse` rule (behavior 14) preserves the invariant. Weights always sum to 1.0; after any structural change, weights renormalize proportionally.

7. **Concurrent agent spawn on restore.** A workspace with N `ClaudeWindow`s spawns N concurrent `AcpChannelClient`s on restore — same model as `spec-multi-session.md` Constraint §6. Each is ~100MB RSS during startup. The soft cap (advisory warning at 6+ Claude windows) from `spec-multi-session.md` §18 applies workspace-wide.

8. **Mouse-drag resize is out of scope for v1.** Resize gutters render as 1px-wide visible separators between sibling windows, but they are non-interactive in v1. Keyboard `Ctrl-W <|>|-|+|=` is the only resize affordance shipped. Mouse-drag is a follow-up spec — it requires GPUI hit-test wiring not present today.

9. **Tab strip placement.** The tab strip renders as a horizontal bar at the top of the window, above the per-tab content area and below any OS-provided titlebar / macOS traffic-light region (verify at implementation that GPUI's window-decoration model doesn't overlap; on macOS, account for the title-bar inset). Width is the full window; tabs are equal-width up to a max-width threshold (then truncated with `…`); overflow scrolls horizontally. **The strip is always visible whenever the workspace has ≥1 tab** — even when only one tab exists. (This differs from `spec-multi-session.md`'s hide-when-single-session sidebar; always-show makes `:tabclose`-on-last-tab visibly distinct from app quit.)

10. **Persistence write rate.** Focus changes coalesce behind a 250ms debounce (Behavior 23). Structural changes write synchronously. The worst-case write rate is one write per 250ms of sustained focus churn — acceptable for SSD-backed disks.

11. **Path canonicalization.** `Workspace.path_index` keys by `std::fs::canonicalize`'d paths for files that exist, falling back to `cwd.join(path)` with `..`/`.` collapsed for files that do not yet exist. On macOS this resolves `/Users/...` vs `/private/var/...` firmlinks and symlink trees correctly; on Linux it resolves symlinks. Two paths that canonicalize to the same buffer hit the same pool entry.

12. **Out of scope.** Detaching a window into a separate OS window, drag-and-drop window rearrangement, tab reordering by drag, per-tab cwd, IME for `EditWindow`, and read-only "mirror" windows (an `EditorView` that refuses input) are not in this spec. They can be added without invalidating the data model.

## Revision History

- 2026-05-13 — First implementation wave landed (commits `190b896`..`3576b3d`). SHIPPED: `EditorCore` / `EditorView` split with `Editor` wrapper preserving TUI surface; `Workspace<C>` + `Tab` + n-ary `Layout` with split/close/only/resize/equalize and 15+ unit tests; binary directory layout; full structural pivot in `SketchGpuiView` (old `screen` / `open_buffers` / `active_buffer_idx` deleted, `OpenBuffer` struct gone, content routed via `workspace.focused_content_*`); tab strip + `Ctrl-Tab` cycling + `Cmd-T` / `Cmd-Shift-W`; recursive layout renderer (weighted flex grids, focused-leaf border); `Ctrl-W` chord prefix bound to `s` / `v` / `c` / `o` / `h` / `j` / `k` / `l` / `w` / `W` / `<` / `>` / `-` / `+` / `=`; topological focus motion; `workspace.json` autosave + restore (Doc / Edit / Browser leaves; Claude restored as Browser stub); clone-focused-content on split (Doc → Doc, Edit → Edit re-reading from disk); proper Edit-mode reconstruction on restore. DRAFT still: shared `FileBuffer` pool (so Doc / Edit in two windows share an editor — Behaviors 9–11, 21–22); `SessionRing` dissolution (Constraint §2 — Claude session per window); placeholder-tab on last close (Behavior 2); `:tabrename` binding (Behavior 6); screen-rect-based focus motion (Behavior 19 currently topological); 250ms focus-change debounce on persistence (Behavior 23 currently writes on every focus change); cross-window edit propagation broadcast (Behavior 10 — pending shared pool).
- 2026-05-11 (2) — Adversarial review pass. Cursor lift expanded into `EditorCore` / `EditorView` split (new Overview artifacts §5–§6, Data Model section, Constraint §3 honest about scope and TUI parity). Cross-window buffer sharing detailed in new Behaviors §9–§11 (per-view cursor, edit propagation via shift broadcast, shared-undo contract). `:tabn` alias collision removed. Persistence write trigger debounced (250ms on focus changes; structural changes synchronous). `:bdelete` Doc/Edit→Browser transition added to Behavior 8. `Ctrl-W` rebinding documented; `:wp-toggle` added on `Ctrl-Shift-E`. `Cmd-W` swapped to `:close`; `Cmd-Shift-W` is `:tabclose`. Tab strip always-visible (Constraint §9). `ClaudeWindow` load fallback exposes `LoadOutcome` + `[new]` label suffix + footer hint (Behavior 24). Path canonicalization spelled out (Constraint §11). Leaf rects cached at paint for focus motion (Behavior 19). Mouse-drag resize marked explicitly out-of-scope for v1 (Constraint §8). `Tab.auto_name` / `display_name` separation. Inlined `spec-multi-session.md` §15's `resume_id` semantics here rather than depending on unshipped contract.
- 2026-05-11 — Initial draft.
