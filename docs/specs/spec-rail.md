# Rail — persistent side columns

**Status:** DRAFT
**Last updated:** 2026-06-01
**Builds on:** `spec-tabs-and-splits.md` (workspace / layout tree)

## Overview

A **rail** is a fixed-width column rendered beside the main content area of a
tab. It is *not* a split-tree leaf — it cannot be split, focused via
`focus_motion`, or resized with `resize_focused`. It is chrome: always
anchored to one edge, always the same width, always showing derived or
navigational content rather than editable documents.

Rail content is **pluggable**: `RailContent` is an enum that new variants can
extend without structural changes. Two initial kinds ship in v1:

- **File browser** — directory listing for quick navigation. Independent of
  the focused window; drives `open_file` on selection.
- **Doc outline** — heading tree extracted from the focused window's rendered
  blocks. Contextual: updates when focus moves between Doc/Edit windows,
  blank when the focused window has no outline (Agent, Browser).

Future kinds (search results, symbol index, git status, diagnostics, etc.)
add a variant to `RailContent`, a render arm in `render_rail`, and a toggle
keybinding. No changes to the rail framework itself.

A tab may have **at most one rail open** at a time. Opening a second kind
replaces the first. The rail can render on **either side** — controlled by
`RailSide` (`Left | Right`), defaulting to `Left`.

Artifacts: `RailContent`, `RailState`, `Tab::rail`, `render_rail`,
`wrap_with_rail`.

## Behaviors

### Lifecycle

#### 1 · Open rail [DRAFT]

`Cmd-B` toggles the file-browser rail. `Cmd-Shift-O` toggles the outline
rail. If the requested kind is already open, the rail closes (toggle
behavior). If a *different* kind is open, it is replaced in-place — no
close-then-open flicker.

#### 2 · Close rail [DRAFT]

`Esc` while the rail is focused closes it and returns focus to the main
content. `Cmd-B` / `Cmd-Shift-O` toggle it closed from anywhere. Closing
the rail does not affect the split tree or any window content.

#### 3 · Rail survives tab switches [DRAFT]

Each tab owns its own `rail: Option<RailState>`. Switching tabs shows/hides
the rail of the arriving/departing tab independently. The *open/closed*
preference is per-tab, not global — an outline rail in tab 1 does not force
an outline rail in tab 2.

#### 4 · Rail survives content changes [DRAFT]

Opening a file, switching modes (Doc ↔ Edit), or splitting does not close
the rail. The outline rail re-derives its heading list from whatever is now
focused; the file-browser rail is unaffected.

### Focus

#### 5 · Rail focus [DRAFT]

The rail is focusable. `Cmd-B` (or `Cmd-Shift-O`) when the rail is already
open and unfocused moves focus *into* the rail. When already focused, it
closes the rail (three-state toggle: closed → open+unfocused → focused →
closed).

Alternatively, a simpler two-state model: the toggle always opens-and-
focuses or closes. The user can return focus to the main content with `Esc`
or by clicking a content pane. **v1 uses two-state** — simpler to implement
and reason about.

#### 6 · Key context [DRAFT]

The rail registers its own key context: `RailView`. When focused, it
attaches `track_focus(&self.focus_handle)` to the rail's root div, which
means the main content's context-scoped bindings (`SketchView`, `EditView`,
etc.) do *not* match — identical to how overlays suppress leaf bindings
today.

Rail-specific bindings inside `RailView`:

| Key       | Action          | Effect                               |
|-----------|-----------------|--------------------------------------|
| `j` / `↓` | `RailDown`      | Move cursor down one entry           |
| `k` / `↑` | `RailUp`        | Move cursor up one entry             |
| `Enter`   | `RailSelect`    | Open file / jump to heading          |
| `Esc`     | `RailClose`     | Close rail, return focus to content  |
| `-`       | `RailParent`    | File browser: go up one directory    |
| `.`       | `RailToggleHidden` | File browser: toggle dotfiles     |
| `s`       | `RailCycleSort` | File browser: cycle sort order       |

Global bindings (`Cmd-Q`, `Cmd-O`, `Cmd-K`, zoom, tab/split management)
remain active via `None`-context registration + `on_action` forwarding on
the rail root, same pattern as every other screen.

#### 7 · Focus return [DRAFT]

When the rail closes (Esc, toggle-off, or `RailSelect` that opens a file),
focus returns to `tab.focused` — the previously-focused split-tree leaf.
No separate "last focused" stack; the split tree's `focused: WindowId` is
the single source of truth.

### Rendering

#### 8 · Layout slot [DRAFT]

The rail is injected by `wrap_with_rail()`, called between
`render_focused_window()` and `wrap_with_tab_strip()` in the render
pipeline:

```
render_focused_window()   →  the split tree
  ↓
wrap_with_rail()          →  flex_row(rail + split_tree)  [NEW]
  ↓
wrap_with_tab_strip()     →  flex_row(tab_strip + above)
```

When the rail is closed (`tab.rail.is_none()`), `wrap_with_rail` is a
no-op passthrough.

#### 9 · Sizing [DRAFT]

Default rail width: `200px`, `flex_none`. The main content area is `flex_1`,
`min_w_0`, `min_h_0` — it shrinks to accommodate the rail, same as it does
for the tab strip.

Width is stored in `RailState::width_px: f32` (default `200.0`). v1 does
not implement drag-to-resize, but the width field is there so a future
resize handle can mutate it without model changes.

The rail column is `overflow_hidden` to clip long filenames/headings.
Text size follows the body font at the base `12px` — it does *not*
scale with `text_scale` (it's chrome, like the tab strip and status bar).

#### 10 · Side placement [DRAFT]

`RailSide` controls which edge the rail sits on. `wrap_with_rail()` emits
`flex_row(rail, content)` for `Left` or `flex_row(content, rail)` for
`Right`. The border is always on the content-facing edge (right border
when `Left`, left border when `Right`).

A keybinding or command toggles side — e.g., `Cmd-Shift-B` flips
`rail.side`. Persisted in the workspace snapshot.

#### 11 · Visual treatment [DRAFT]

- Background: `theme.top_bar.bg` (same as tab strip) for visual
  continuity when both are visible on the left edge.
- Content-facing border: `1px`, same separator color as split borders.
- Outer border: none (flush with window edge or tab strip).
- Selected-entry highlight: `editor_bg` background (same as active tab
  in the strip) with `STATUS_FG` text.
- Unselected entries: `top_bar.bg` background, `overlay.label` text.
- Section headers (outline: depth-0 headings): `overlay.accent`, bold.

#### 12 · File browser rail content [DRAFT]

Reuses the existing `FileBrowser` struct from `BrowserWindow`. The rail
instantiates its own `FileBrowser` rooted at `cwd`. Selecting an entry
calls `open_file(path)` on the main workspace — the file opens in the
focused split-tree leaf (replacing its content), and the rail stays open.

Navigating into a directory updates the rail's `FileBrowser` in place
(same as the existing browser pane). The rail's directory state is
independent of any `BrowserWindow` that might exist in the split tree.

#### 13 · Outline rail content [DRAFT]

Derives a flat list of `(depth, heading_text, block_index)` from the
focused window's content:

- `WindowContent::Doc(d)` → walk `d.blocks`, collect
  `RenderedBlock::Heading { level, .. }` entries.
- `WindowContent::Edit(e)` → scan the rope for ATX heading lines
  (`^#{1,6}\s`). Cheaper than a full pulldown-cmark parse; good enough
  for an outline.
- `WindowContent::Agent(_)` / `WindowContent::Browser(_)` → empty
  outline. The rail renders a "(no outline)" placeholder.

Selecting a heading scrolls the focused Doc view to that block
(`scroll_handle.scroll_to_item(block_index)`) or the Edit view to that
line.

The outline re-derives on every render frame where the focused window
changed (new `focused: WindowId` or content mutation). Derivation is
O(n) in block/line count — fast enough for documents up to tens of
thousands of lines.

### Persistence

#### 14 · Workspace snapshot [DRAFT]

`workspace.json` gains an optional per-tab field:

```json
{
  "tabs": [
    {
      "auto_name": "tab-1",
      "layout": { "..." },
      "focused": 1,
      "rail": { "kind": "file_browser", "side": "left", "width": 200, "cwd": "/Users/scott/ws/sketch" }
    }
  ]
}
```

On restore, the rail is reconstructed from the persisted kind + state.
Outline rails persist only as `{ "kind": "outline" }` — the heading list
re-derives from whatever file is focused on restore.

## Data model

```rust
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RailSide {
    Left,
    Right,
}

/// What the rail is showing. Extend this enum to add new rail kinds —
/// each variant needs a render arm in `render_rail` and a toggle binding.
enum RailContent {
    FileBrowser(FileBrowser),
    Outline(OutlineState),
}

/// Derived outline: heading entries from the focused window.
struct OutlineState {
    /// (heading depth 1–6, display text, block index or line number)
    entries: Vec<(u8, String, usize)>,
    selected: usize,
}

/// Per-tab rail state.
struct RailState {
    content: RailContent,
    side: RailSide,
    /// Column width in px. Default 200.0. Stored for future drag-resize;
    /// v1 does not expose a resize handle.
    width_px: f32,
    /// True when the rail div holds `track_focus`. When false, the main
    /// content leaf holds focus as usual.
    focused: bool,
}
```

`Tab<C>` gains:

```rust
pub struct Tab<C> {
    pub auto_name: String,
    pub display_name: Option<String>,
    pub layout: Layout<C>,
    pub focused: WindowId,
    pub rail: Option<RailState>,   // ← new
}
```

## Constraints

1. **Not a split-tree participant.** The rail is not a `Layout` node. It
   cannot be the target of `split_focused`, `close_focused`, `only`,
   `resize_focused`, `equalize_focused`, or any `focus_motion` direction.
   It lives outside the tree entirely.

2. **One rail per tab.** No stacking two rails side-by-side. Opening a
   second kind replaces the first.

3. **Either side.** `RailSide` (`Left | Right`) is stored per-tab and
   persisted. Default `Left`. Togglable at runtime.

4. **No rail in overlays.** When a modal overlay is open (menu, buffer
   switcher, session switcher, rename), the rail is still *rendered* (it's
   part of the background) but is not focusable — the overlay owns focus.

5. **Chrome font size.** Rail text does not scale with `Cmd-+` /
   `Cmd--`. It tracks `theme.top_bar` styling, not document body styling.

6. **No drag-to-resize (v1).** Width defaults to 200px, stored in
   `RailState::width_px` for future resize support. v1 does not render a
   drag handle.

7. **Pluggable content.** `RailContent` is designed as an open enum.
   Adding a new rail kind requires: (a) a new variant, (b) a render arm
   in `render_rail`, (c) a toggle keybinding. No framework changes needed.

## Revision history

- 2026-06-01 — Initial draft.
