# Menu Scopes — Global Leader and Local Leader

**Status:** DRAFT
**Last updated:** 2026-06-06
**Builds on:** `spec-workspaces-and-splits.md` (workspace / screens), `spec-layout-patterns.md` (tags, marks, layouts), `spec-rail.md` (per-tab chrome)

## Overview

Yalda's current `Space` menu is a flat bag of everything: open a file,
split a tile, change the theme, send to Claude, toggle a rail. Every
command is reachable from every screen. This works when the menu is small,
but it doesn't scale, and it conflates two different questions:

1. **"What can I do to the workspace?"** — open tiles, move focus, switch
   tabs, quit. These are the same regardless of what's focused.
2. **"What can I do to *this* tile?"** — enter edit mode, reload from
   disk, toggle word-processor view, show the outline, navigate headings.
   These depend on what the focused window is showing.

vim/neovim solves this with two leader keys: `<leader>` (global
operations) and `<localleader>` (buffer-local / filetype-specific
operations). which-key.nvim then makes both discoverable by showing a
popup when you pause after pressing either leader, with context-appropriate
entries.

This spec adopts the same split:

- **`Space`** — **global leader**. Workspace-scope commands: open, close,
  split, focus, tag, mark, layout, theme, quit. Same entries on every
  screen.
- **`.` (dot)** — **local leader**. Tile-scope commands: entries change
  based on the focused window's content kind (`Doc`, `Edit`, `Agent`,
  `Browser`). The menu is different in each context because the available
  operations are different.

Both leaders open the same overlay mechanism (`MenuOverlay`), but they
build their menu tree from different sources. The overlay UI, key
processing, breadcrumb display, and escape-to-dismiss behavior are
identical.

### Relationship to which-key

Yalda's menu and neovim's which-key solve the same problem — progressive
disclosure of key hierarchies — but the mechanisms differ in important
ways:

| | which-key.nvim | yalda menu |
|---|---|---|
| **When it appears** | After a timeout (200ms default); only shows if you pause mid-chord | Immediately on leader press; the overlay *is* the chord |
| **Key source** | Reads from neovim's live keymap table; any plugin can register bindings | Static `Vec<MenuNode>` built at compile time per leader/context |
| **Dynamic entries** | `expand` nodes generate children at display time; plugins add keymaps at runtime | Not yet; menu trees are fixed. Future: dynamic entries (Behavior 13) |
| **Context model** | Buffer-local keymaps overlay globals; `<localleader>` prefix is a display heuristic | Two separate trees with two separate leader keys; context is structural, not a heuristic |
| **Proxy / delegation** | `proxy` nodes redirect one prefix's children to another's (e.g., `<leader>w` → `<c-w>`) | Not yet; `Space W` is a manually-duplicated submenu of `Ctrl-W` chords |
| **Hydra mode** | `loop = true` keeps the popup open for repeated actions (e.g., window resize) | Not yet; every command closes the menu |
| **Backend** | Lua plugin riding on neovim's keymap API; the popup is a floating window | Built into the binary; the overlay is a GPUI element; `MenuState` is shared with the TUI |

The key design difference: which-key is a *viewer* over an existing
keymap system — it discovers bindings that already exist. Yalda's menu is
the *primary* binding surface for leader-key commands — the menu tree
*defines* what's available, and the dispatch table in
`dispatch_menu_command()` is the canonical mapping from name to action.

This means yalda doesn't need which-key's dynamic discovery machinery
(keymap scanning, auto-triggers, timeout-vs-nowait ambiguity). But it
should adopt which-key's best UX ideas: progressive disclosure,
breadcrumb navigation, context-dependent entries, and the two-leader
(global/local) split.

## Behaviors

### Leader keys

#### 1 · Global leader: Space [DRAFT]

`Space` opens the global menu overlay. The menu tree contains
workspace-scope commands organized by group:

| Group | Keys | Operations |
|-------|------|------------|
| **Open** | `f`, `b`, `c` | File browser, buffer list, Claude submenu |
| **Window** | `W` → ... | Split, close, focus, resize, equalize |
| **Workspace** | `W` → ... | New/close/next/prev tab, rename, move tile |
| **Tag** | `t` → ... | Tag operations (from `spec-layout-patterns.md`) |
| **Mark** | `m`, `'` | Set/jump to marks (direct, not via menu) |
| **Layout** | `L` → ... | Layout mode cycle, master promote/count |
| **View** | `v`, `s` | Back to doc, status bar toggle |
| **Theme** | `T` → ... | Theme submenu |
| **Rail** | `B`, `O`, `S` | File browser rail, outline rail, flip side |
| **Quit** | `q` | Quit |

The global menu is **identical regardless of focused content kind.** A
`Space` press in a Doc tile, an Edit tile, an Agent tile, or a Browser
tile all show the same tree.

Entries that are not applicable in the current context (e.g., `reload
from disk` when an Agent tile is focused) are either hidden or shown
as disabled (greyed out, non-interactive). Behavior 10 specifies the
rule.

#### 2 · Local leader: dot [DRAFT]

`.` (dot) opens the local menu overlay. The menu tree is **determined by
the focused window's content kind.** Four content kinds, four different
local menus:

**Doc local menu (`YaldaView`):**

| Key | Label | Command |
|-----|-------|---------|
| `e` | edit (raw markdown) | `enter-edit` |
| `w` | edit (word processor) | `enter-wp` |
| `r` | reload from disk | `reload-file` |
| `o` | outline | `rail-outline` |
| **n** | **navigate** (submenu) | |
| ` l` | links | `nav-links` |
| ` h` | headings | `nav-headings` |
| ` i` | list items | `nav-list-items` |
| ` c` | code blocks | `nav-code-blocks` |
| **g** | **goto** (submenu) | |
| ` g` | top | `goto-top` |
| ` e` | bottom | `goto-bottom` |
| ` h` | next heading | `goto-heading` |

**Edit local menu (`EditView`):**

| Key | Label | Command |
|-----|-------|---------|
| `v` | back to doc view | `back-to-doc` |
| `w` | toggle code/word-processor | `wp-toggle` |
| `r` | reload from disk | `reload-file` |
| `a` | select all | `select-all` |
| `y` | yank selection | `yank-selection` |
| `d` | delete selection | `delete-selection` |
| **e** | **edit** (submenu) | |
| ` v` | extend mode | `toggle-extend-mode` |
| ` ;` | collapse selection | `collapse-selection` |
| ` ,` | flip selection | `flip-selection` |
| ` x` | extend by line | `extend-line` |

**Agent local menu (`ClaudeView`):**

| Key | Label | Command |
|-----|-------|---------|
| `n` | new session | `claude-new` |
| `l` | list sessions | `claude-list` |
| `x` | close session | `claude-close` |
| `r` | rename session | `claude-rename` |
| `w` | toggle worksheet/chatbox | `agent-input-toggle` |
| `s` | send buffer | `claude-send` |
| `S` | send selection | `claude-send-selection` |
| `d` | detach | `claude-detach` |
| `a` | attach | `claude-attach` |
| `p` | promote (build candidate) | `dev-build-candidate` |

**Browser local menu (`BrowserView`):**

| Key | Label | Command |
|-----|-------|---------|
| `s` | cycle sort | `browser-sort` |
| `.` | toggle hidden files | `browser-toggle-hidden` |
| `-` | go up | `browser-parent` |
| `w` | open in new workspace | `browser-open-workspace` |
| `v` | open in split | `browser-open-split` |

#### 3 · Dot in Edit insert mode [DRAFT]

In Edit insert mode, `.` is a text character and must not open the menu.
The local leader only activates in contexts where `.` is not a text
input key:

- **YaldaView** (Doc view) — `.` opens local menu.
- **EditView Normal mode** — `.` opens local menu.
- **EditView Insert mode** — `.` inserts a literal dot.
- **ClaudeView** — `.` opens local menu (outside the compose box);
  literal dot inside the compose box.
- **BrowserView** — `.` opens local menu.

This parallels vim's `<localleader>` which only fires in normal mode.

#### 4 · Overlay mechanics are shared [DRAFT]

Both leaders open `ActiveOverlay::Menu(MenuOverlay { ... })` with
different menu trees. `MenuState`, `process_key()`, `handle_escape()`,
breadcrumb rendering, and the overlay container are identical. The only
difference is which `Vec<MenuNode>` is passed in.

The header line shows the leader origin for orientation:

- Global: `MENU — [breadcrumb]`
- Local: `LOCAL — [breadcrumb]` (or the content kind: `DOC — [breadcrumb]`,
  `EDIT — [breadcrumb]`, etc.)

### Menu structure

#### 5 · Global menu cleanup [DRAFT]

Moving context-specific entries to the local menu lets the global menu
shrink. The following entries move **out** of the global `Space` menu
and into local menus:

| Entry | Current location | New location |
|-------|-----------------|--------------|
| `e` edit (raw markdown) | Space root | Doc local |
| `w` edit (word processor) | Space root | Doc local |
| `r` reload from disk | Space root | Doc/Edit local |
| `v` back to doc | Space root | Edit local |
| `s` toggle status bar | Space root | Global (stays — it's app-wide) |
| `c` claude submenu | Space root | Agent local (session management) |

After cleanup the global menu contains only workspace-scope operations.
The `c` key in the global menu can be reassigned (e.g., to `:close` or
left as a submenu for `claude-new` only, since creating a new session
is arguably workspace-scope).

#### 6 · Local menu is empty for unknown content [DRAFT]

If a future content kind is added without a local menu definition, `.`
opens an empty menu with a hint: `(no local commands)`. The overlay
dismisses on any key.

#### 7 · Duplicate entries are allowed [DRAFT]

A command can appear in both the global and local menus if it makes
sense. For example, `rail-outline` might appear as `Space O` (global —
"toggle outline") and as `. o` (Doc local — "outline for this doc").
The dispatch is the same; the menu is just an access path.

However, the design intent is: **prefer one canonical location.** If a
command is naturally tile-scoped, put it in the local menu. If it's
naturally workspace-scoped, put it in the global menu. Duplicate only
when discoverability benefits outweigh the clutter.

### Context and applicability

#### 8 · Content-kind dispatch for local menu [DRAFT]

`open_local_menu()` reads the focused window's content kind and selects
the matching menu tree:

```rust
fn local_menu_for(content: &WindowContent) -> Vec<MenuNode> {
    match content {
        WindowContent::Doc(_)     => doc_local_menu(),
        WindowContent::Edit(_)    => edit_local_menu(),
        WindowContent::Agent(_)   => agent_local_menu(),
        WindowContent::Browser(_) => browser_local_menu(),
    }
}
```

Each `*_local_menu()` function is a static builder (like `gpui_menu()`
today), returning a `Vec<MenuNode>`. They live alongside `gpui_menu()`
in `main.rs`.

#### 9 · Local menu updates on content swap [DRAFT]

If the user opens a local menu, then somehow the focused content changes
(e.g., a Claude session finishes and auto-focuses a different tile — not
a current behavior but defensive), the menu should close. The menu
captures the `WindowId` at open time; if the focused window changes while
the menu is open, the overlay dismisses. This prevents stale entries from
dispatching against the wrong content.

#### 10 · Disabled entries in global menu [DRAFT]

Global menu entries that require a specific content kind show as
**disabled** (greyed text, non-interactive) rather than hidden. This
preserves spatial stability — the menu layout doesn't jump around
depending on what's focused — and teaches the user where commands live
even when they're not available.

Disabled entries skip dispatch in `process_key()` (treated as
unrecognized). The render path uses `overlay.label` color (dimmed)
instead of `overlay.fg`.

Disabled-when rules:

| Entry | Disabled when |
|-------|--------------|
| `reload-file` | Focused window is Agent or Browser |
| `enter-edit` | Focused window is Agent or Browser |
| `back-to-doc` | Focused window is not Edit |

This list is expected to be short — most global entries are always valid.

### Proxy pattern (future)

#### 11 · Ctrl-W proxy [DRAFT]

The global menu's `W` (window) submenu duplicates every `Ctrl-W` chord
binding by hand. This is the maintenance problem which-key's `proxy`
pattern solves.

**Future:** add a `MenuNode::proxy(key_str, label, prefix)` variant that
populates its children from the bindings registered under `prefix` at
display time. `Space W` would become `MenuNode::proxy("W", "window",
"ctrl-w")` and automatically reflect any new `Ctrl-W` bindings.

Not in v1: the binding registry would need to be queryable at runtime,
and the current `on_action` / GPUI action dispatch path doesn't expose
a list of registered chords. This is a quality-of-life improvement, not
a blocker.

#### 12 · Hydra mode for resize [DRAFT]

which-key's `loop = true` keeps the popup open after executing a command.
This is useful for window resize: press `Space W -` to shrink, then
keep pressing `-` without re-entering the menu.

**Future:** add a `MenuNode.repeatable: bool` flag. When a repeatable
command executes, the menu re-opens at the same depth instead of closing.
`Esc` breaks the loop. Applies to resize keys (`-`, `+`), focus motions
(`h/j/k/l`), and zoom (`=`, `-`).

Not in v1: the current `process_key` → `close()` path makes this a
structural change to `MenuState`.

#### 13 · Dynamic entries [DRAFT]

which-key supports `expand` nodes that generate children at display time.
Yalda equivalents:

- **Buffer list:** `Space b` could expand to show open buffers inline
  rather than opening a separate overlay.
- **Session list:** `Space c l` could expand to show sessions.
- **Tag list:** `. t` (Doc local) could expand to show the buffer's
  current tags with toggle actions.
- **Mark list:** `Space '` could expand to show all marks with
  jump targets.

**Future:** add `MenuAction::Expand(fn() -> Vec<MenuNode>)`. The
function is called when the user enters the submenu, generating children
dynamically. The expand function is `Fn`, not `FnOnce`, so it can be
called repeatedly (menu re-entry).

Not in v1: static menus are sufficient and simpler to reason about.

### Migration from single menu

#### 14 · Backwards compatibility [DRAFT]

During the transition, both `Space` and `.` are active. Users who are
accustomed to `Space e` (enter edit) can be migrated:

- **Phase 1:** Add `.` as local leader with per-content menus. Keep all
  existing entries in `Space`. The global menu has duplicates; that's OK
  temporarily.
- **Phase 2:** Remove context-specific entries from `Space` (Behavior 5).
  Add a transient footer hint the first N times a user presses a removed
  key in the global menu: `"e" moved to local menu (press "." then "e")`.

Phase 1 can ship independently; Phase 2 is a follow-up.

## Data model

### Menu source tag on `MenuOverlay`

```rust
/// Which leader opened this menu instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuScope {
    Global,
    Local,
}

pub struct MenuOverlay {
    pub state: MenuState,
    pub menu: Vec<MenuNode>,
    pub scope: MenuScope,          // ← new
    pub opened_from: WindowId,     // ← new: for stale-check (Behavior 9)
}
```

### No changes to `MenuNode` or `MenuState`

The existing `MenuNode`, `MenuAction`, and `MenuState` types are
sufficient. The two menus are two different `Vec<MenuNode>` values
passed to the same processing machinery.

### Local menu builders

```rust
fn doc_local_menu() -> Vec<MenuNode> { /* Behavior 2, Doc column */ }
fn edit_local_menu() -> Vec<MenuNode> { /* Behavior 2, Edit column */ }
fn agent_local_menu() -> Vec<MenuNode> { /* Behavior 2, Agent column */ }
fn browser_local_menu() -> Vec<MenuNode> { /* Behavior 2, Browser column */ }
```

These are pure functions (no state dependencies in v1). They live next
to `gpui_menu()` in `main.rs`.

## Interfaces

### Commands

| Command | Effect |
|---|---|
| `:menu` | Open global menu (same as Space) |
| `:local-menu` | Open local menu (same as `.`) |

### Keybindings

| Binding | Context | Action |
|---|---|---|
| `Space` | YaldaView, EditView (normal), ClaudeView, BrowserView | Open global menu |
| `.` | YaldaView, EditView (normal), BrowserView | Open local menu |
| `.` | ClaudeView (outside compose box) | Open local menu |
| `.` | EditView (insert) | Insert literal dot (no menu) |

### Overlay header format

| Scope | Header |
|---|---|
| Global | `MENU — [breadcrumb]` |
| Local (Doc) | `DOC — [breadcrumb]` |
| Local (Edit) | `EDIT — [breadcrumb]` |
| Local (Agent) | `AGENT — [breadcrumb]` |
| Local (Browser) | `BROWSE — [breadcrumb]` |

## Constraints

1. **Two leaders, one overlay.** Opening a global menu while a local menu
   is open (or vice versa) replaces the current overlay. Only one menu
   can be open at a time (existing `ActiveOverlay` exclusivity).

2. **Dot is context-safe.** `.` never opens a menu when text input is
   expected (Edit insert mode, Agent compose box). The binding is
   registered only in non-insert key contexts.

3. **Local menus are static in v1.** Each content kind has one fixed menu
   tree. Dynamic entries (Behavior 13) and expand nodes are future work.

4. **Global menu is content-agnostic.** The global menu tree does not
   change based on focused content. Entries may be disabled (Behavior 10)
   but never hidden or rearranged.

5. **No timeout.** Unlike which-key (200ms delay before popup), yalda's
   menu appears immediately on leader press. There is no ambiguity to
   resolve — `Space` and `.` have no other bindings in the relevant
   contexts, so there's no reason to wait.

6. **TUI parity.** The TUI's `default_menu()` in `src/menu.rs` should
   eventually split into global and local menus too. The `MenuNode` /
   `MenuState` types are shared; only the menu-tree builders differ.
   TUI migration is out of scope for v1 of this spec but the shared
   types are designed to support it.

7. **Chrome font size.** Menu overlay text does not scale with
   `text_scale`. Consistent with the tab strip, rail, and status bar.

8. **which-key features explicitly deferred.** Proxy nodes (Behavior 11),
   hydra/loop mode (Behavior 12), dynamic expand (Behavior 13), and
   runtime keymap scanning are interesting but not v1. The menu is the
   binding definition, not a viewer over external bindings, so the
   dynamic discovery machinery isn't needed yet.

## Revision history

- 2026-06-06 — Initial draft.
- 2026-06-09 — v1 (Phase 1) implemented in `yalda-gpui`: `.` local leader
  with per-content menus (Behaviors 1–4, 6–10), scope-aware headers, stale
  dismissal, disabled global entries. Global menu kept intact (Phase 2
  cleanup not done). Browser `.` rebound from toggle-hidden to local
  leader; toggle-hidden is now `. .`.
- 2026-06-09 — Full local-menu command set landed: doc `navigate` submenu
  (`nav-links`/`nav-headings`/`nav-list-items`/`nav-code-blocks` = jump to
  next block of kind, wrapping) + `goto-heading`; `claude-send-selection`
  (sends the transcript selection as a prompt, input surface untouched);
  `browser-open-workspace` / `browser-open-split` (selected file → Doc in a
  new tab / vertical split; directories rejected with a toast).
- 2026-06-09 — Phase 2 cleanup (Behavior 5): Edit group (`enter-edit` /
  `enter-wp` / `reload-file`) removed from the global menu — tile-scoped,
  lives in the `.` local menus. Rail group and the agent status-bar
  position toggle removed from the menu entirely (rails keep their global
  chords). Menu overlay now lays sections out in 1–3 columns (≤8 rows → 1,
  ≤18 → 2, else 3; separator-delimited sections never split mid-group).
- 2026-06-09 — Global root restructured into four submenus: `n` new
  (file-browser tile / buffer list / claude session tile — the tile
  entries SPLIT and create a new tile via `new-browser-tile` /
  `new-agent-tile` instead of replacing the focused one), `w` windows
  (split/close/focus/size), `s` workspace (tabs/move/also-show), `l`
  layout (layouts/marks/tags, keys lowercased). Theme submenu killed.
  Claude session management (incl. mode-cycle and the build loop) is now
  Agent-local only (`. m`, `. p`, `. P`, `. g`).
