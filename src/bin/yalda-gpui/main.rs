//! `yalda-gpui` — GPU-accelerated desktop frontend for yalda.
//!
//! Rendered-markdown viewer + file browser using Zed's GPUI framework. This
//! binary consumes only the framework-neutral core (`document`, `render`,
//! `theme`, `blocks`, `style`, `file_browser`).
//!
//! Run:
//!     cargo run --bin yalda-gpui                       # opens browser at cwd
//!     cargo run --bin yalda-gpui -- <path/to/file.md>  # opens file directly
//!
//! Document view keys:
//!   j / Down / Ctrl-N    scroll down one block
//!   k / Up / Ctrl-P      scroll up one block
//!   h                    move cursor to previous block
//!   l                    move cursor to next block
//!   g                    top of document
//!   G                    bottom of document
//!   Space                open the tile/app (content-kind) command menu
//!   .                    open the workspace command menu
//!   ?                    open the global (Yaldabaoth) menu
//!   Ctrl-E               edit current file (raw markdown)
//!   Ctrl-W               edit current file (word-processor view)
//!   Ctrl-K               open Claude (ACP) chat screen
//!   Ctrl-O               open file browser
//!   Tab / Shift-Tab      next / previous buffer
//!   q / Esc              quit
//!
//! Menu (Space → tile/app menu, `.` → workspace menu, anywhere not in text entry):
//!   * Single-key picker over a small set of commands (open browser,
//!     enter edit/wp, open claude, back-to-doc, quit).
//!   * Submenu nodes drill in; Esc pops one level (closes from root).
//!   * Same `MenuState` model as the TUI (`src/menu.rs`); the menu tree
//!     itself is the GPUI-specific subset (`gpui_menu()`).
//!
//! Claude (ACP) chat screen:
//!   * Spawns a local `claude-agent-acp` (or $YALDA_ACP_AGENT) process and
//!     talks to it via the Agent Client Protocol.
//!   * Claude's prior turns are frozen (read-only) and marked with a left
//!     bar; you can keep typing in the editable region below the last turn,
//!     or insert inline replies between frozen blocks via `o`/`O`.
//!   * `Ctrl-Enter` sends the editable inserts as your turn (locks them).
//!   * `Ctrl-V` returns to whatever screen you came from.
//!   * Same Helix-style normal/insert dispatch as the Edit screen.
//!
//! Edit screen — two views over the same buffer (toggle with Ctrl-W):
//!   * RAW view: monospace + per-line markdown syntax highlighting + gutter.
//!   * WP view: proportional font, headings at variable sizes, inline
//!     bold/italic styling, list/blockquote decoration. Source markers
//!     (`#`, `*`, `_`, `-`) stay visible (rendered dim) so they're editable.
//!
//! Edit keys (Helix-style — every motion mutates a selection):
//!   Normal motions: hjkl · w/b/e word · 0/$ line · gg/G doc top/bot
//!   Selection:      v extend mode · x line-extend · ; collapse · , flip
//!                   % select-all · d delete · c change · y yank
//!   Mode switch:    i insert at sel-start · a insert after sel-end
//!                   o/O open line below/above
//!   Edits:          u undo · (no redo binding by default; see config)
//!   Both views:     Ctrl-S save · Ctrl-W toggle wp/raw · Ctrl-V back to Doc
//!   Insert mode:    type to insert · Esc → Normal · Backspace · Tab (2 spaces)
//!
//! File-browser view keys:
//!   j / Down / Ctrl-N    next entry
//!   k / Up / Ctrl-P      previous entry
//!   Enter / l            open entry (descend into dir, or open file)
//!   - / h                go to parent directory
//!     .                    toggle hidden files
//!     s                    cycle sort order (name / date↓ / date↑); remembered per tile
//!     q / Esc              close browser (returns to doc, or quits)

mod agent;
mod agent_naming;
mod agent_roster;
mod agent_sessions;
mod agent_ui;
mod browser_ui;
mod chrome;
mod edit_ui;
mod highlight_cache;
mod jump_palette;
mod jump_panel_view;
mod keymap_registry;
mod keymap_tile;
mod keymap_ui;
mod keymap_view;
mod linear;
mod linear_ui;
mod linear_view;
mod persist;
mod project;
mod render_blocks;
mod screens;
mod tool_body;
mod transcript_view;
#[cfg(test)]
mod verify_harness;
/// yux — reusable UX component layer (cached-view infra + view primitives).
/// All UX work is built from here; see `yux/CLAUDE.md`.
mod yux;
pub(crate) use agent::*;
pub(crate) use agent_naming::*;
pub(crate) use agent_roster::*;
pub(crate) use agent_sessions::*;
pub(crate) use jump_palette::*;
pub(crate) use jump_panel_view::*;
pub(crate) use keymap_registry::*;
pub(crate) use keymap_tile::*;
pub(crate) use keymap_view::*;
pub(crate) use linear::*;
pub(crate) use linear_view::*;
pub(crate) use persist::*;
pub(crate) use project::*;
pub(crate) use render_blocks::*;
pub(crate) use tool_body::*;
pub(crate) use transcript_view::*;
pub(crate) use yux::*;
mod workspace;

pub(crate) use highlight_cache::{HighlightCache, LineHl};
pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::path::PathBuf;
pub(crate) use std::process;
pub(crate) use std::rc::Rc;
pub(crate) use std::time::Duration;

pub(crate) use gpui::{
    AnyElement, App as GpuiApp, AppContext, Application, Bounds, ClipboardItem, Context, Element,
    ElementId, Entity, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, Keystroke,
    LayoutId, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, Task, TextLayout, TextRun, TitlebarOptions,
    UnderlineStyle, WeakEntity, Window, WindowBounds, WindowOptions, actions, div, point, px, rgb,
    rgba, size,
};

pub(crate) use yalda::acp_channel::{AcpChannelClient, AgentProvider};
pub(crate) use yalda::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine, StyledSpan};
pub(crate) use yalda::cursor::CursorPos;
pub(crate) use yalda::document::Document;
pub(crate) use yalda::editor::{EditAccess, Editor, EditorCore, EditorView, LineAnchor};
pub(crate) use yalda::file_browser::{BrowserEntry, FileBrowser, SortOrder};
pub(crate) use yalda::keybind::KeybindManager;
pub(crate) use yalda::keys::{Key, KeyPress, Modifiers as KMods};
pub(crate) use yalda::md_highlight::{
    Segment, highlight_markdown_lines_stripped_syn, highlight_markdown_lines_syn,
};
pub(crate) use yalda::menu::{MenuAction, MenuNode, MenuNodeKind, MenuState};
pub(crate) use yalda::render;
pub(crate) use yalda::session_client::SessionServerClient;
pub(crate) use yalda::session_proto::Notification as ServerNotification;
pub(crate) use yalda::style::{Color as NColor, Modifier, Style as NStyle};
pub(crate) use yalda::theme::{OverlayTheme, Theme, ThemeName};
pub(crate) use yalda::worktree;

// ----------------------------------------------------------------------------
// Render performance knobs (env-gated, read once)
// ----------------------------------------------------------------------------

/// `true` when `YALDA_PERF` is set to anything other than `0`/empty. Enables
/// per-frame timing breakdowns from the agent tile render path (extract /
/// highlight / snapshot / total), printed to stderr. Read once and cached.
fn perf_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("YALDA_PERF"), Ok(v) if v != "0" && !v.is_empty()))
}

/// `false` only when `YALDA_HL_CACHE` is explicitly `0`/`off`/`false`. The
/// incremental highlight cache is ON by default; this lets us A/B it against
/// the old full-recompute path at runtime without a rebuild.
fn hl_cache_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("YALDA_HL_CACHE").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}

// ----------------------------------------------------------------------------
// Actions
// ----------------------------------------------------------------------------

actions!(
    yalda,
    [
        // Document view
        ScrollDown,
        ScrollUp,
        ScrollPageDown,
        ScrollPageUp,
        CursorNextBlock,
        CursorPrevBlock,
        CursorTop,
        CursorBottom,
        OpenBrowser,
        EnterEdit,
        EnterWp,
        OpenAgent,
        OpenLinear,
        OpenKeymap,
        OpenMenu,
        OpenLocalMenu,
        OpenGlobalMenu,
        Quit,
        Restart,
        // Buffer cycling
        NextBuffer,
        PrevBuffer,
        // Workspace cycling (frame-level — independent of buffer list)
        NextWorkspace,
        PrevWorkspace,
        NewWorkspace,
        CloseWorkspace,
        // Move the focused tile to another workspace (Ctrl-W m). Opens the
        // workspace picker; selecting a target relocates the focused leaf
        // (content travels with it). See spec-workspaces-tagging.md Phase 1.
        MoveTile,
        // Also-show the focused (file-backed) tile in another workspace
        // (Ctrl-W M / shift). Opens the same picker; selecting a target
        // creates a second view onto the same file there, leaving the
        // original in place. Agent/Browser tiles are single-home (rejected).
        AlsoShowTile,
        // Splits (Ctrl-W chord prefix per spec-workspaces-and-splits.md §12)
        SplitH,
        SplitV,
        CloseWindow,
        OnlyWindow,
        // Focus motion within the active workspace's split tree
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        FocusNext,
        FocusPrev,
        // Browser view
        BrowserDown,
        BrowserUp,
        BrowserEnter,
        BrowserParent,
        BrowserToggleHidden,
        BrowserCycleSort,
        BrowserClose,
        BrowserWorktrees,
        BrowserFilter,
        BrowserRename,
        // Document text zoom (scales body + headings; chrome stays fixed)
        ZoomIn,
        ZoomOut,
        ZoomReset,
        // Flip between the Nightfox (dark) and Folio (light) themes.
        ToggleTheme,
        // Copy selected text to system clipboard (all screens)
        CopySelection,
        // View-mode mouse text selection (doc view only, legacy alias)
        CopyDocSelection,
        // Paste from system clipboard into active editor
        PasteFromClipboard,
        // Open the rename input overlay for the active workspace.
        RenameWorkspace,
        // Agent window: send the current draft. Worksheet sweep (§12) or
        // Chatbox submit (§18) depending on `AgentState::input_mode`.
        SubmitAgent,
        // Agent window: flip the input mode between Worksheet and
        // Chatbox (§5). Bound to `Ctrl-Alt-Enter`.
        ToggleAgentInputMode,
        // Agent window: open/close the Tasklist sidebar (§32). Cmd-1.
        ToggleTasklist,
        // Agent window: open/close the Subagents sidebar (§32). Cmd-2.
        ToggleSubagents,
        // Agent window: focus + enlarge the bottom-panel region (Plan +
        // Subagents) for vim-key selection (UXI-AgentTile-3). Cmd-0 in AgentView,
        // overriding the global zoom-reset there. Esc leaves it.
        FocusAgentPanel,
        // Agent window: force-hide/show the whole right sidepanel (UXI-AgentTile-20).
        // Cmd-B in AgentView, shadowing the global ToggleFileBrowserRail there.
        ToggleAgentSidepanel,
        // Agent window: interrupt the in-flight turn (ACP session/cancel).
        // Bound to Cmd-. and surfaced as a Stop button while a reply is
        // pending.
        StopAgent,
        // Rail (persistent side column, spec-rail.md).
        // Toggles (global, `None` context):
        ToggleFileBrowserRail,
        ToggleOutlineRail,
        FlipRailSide,
        // Jump panel (jump-panel; spec-jump-panel.md). Global, `None` context.
        ToggleJumpPanel,
        // Jump palette (UXI-JumpPanel-9): Cmd-P fuzzy jump over workspaces +
        // agent sessions. Global, `None` context.
        OpenJumpPalette,
        // Rail-focused navigation (`RailView` context):
        RailDown,
        RailUp,
        RailSelect,
        RailClose,
        RailParent,
        RailToggleHidden,
        RailCycleSort,
        RailWorktrees,
        RailFilter,
        // Plane camera (spec-infinite-plane-workspace.md Interfaces). Each is a
        // `Ctrl-W`+plain-key SEQUENCE (reliable on macOS, unlike a bare
        // `Ctrl`+digit chord). Anchor = focused tile's slot, else viewport center.
        ZoomOutWorkspace,
        ZoomInWorkspace,
        ResetWorkspaceView,
        DesktopTileSize,
        // Phase 3: tags
        ClearTagView,
        TagViewChord,
        TagToggleChord,
        // Workspace number switching (`ctrl-<n>`): jump straight to the Nth
        // non-ephemeral workspace, the number shown in the jump panel. `0` is
        // the 10th (mirrors the goto-workspace menu's digit convention).
        GotoWorkspace1,
        GotoWorkspace2,
        GotoWorkspace3,
        GotoWorkspace4,
        GotoWorkspace5,
        GotoWorkspace6,
        GotoWorkspace7,
        GotoWorkspace8,
        GotoWorkspace9,
        GotoWorkspace10,
    ]
);

/// Fluent helper to wire the ten `ctrl-<n>` workspace-jump actions onto a
/// screen root in one call. The bindings are app-global (`None` context), but
/// the action still needs a handler in the focused element's ancestry — so each
/// screen root (`YaldaView`, `EditView`, `AgentView`, …) calls `.workspace_nav`
/// the same way it wires `toggle_jump_panel`. Avoids repeating ten near-
/// identical `on_action` lines per screen.
pub(crate) trait WorkspaceNavExt: Sized {
    fn workspace_nav(self, cx: &mut Context<YaldaGpuiView>) -> Self;
}

impl<E: InteractiveElement> WorkspaceNavExt for E {
    fn workspace_nav(self, cx: &mut Context<YaldaGpuiView>) -> Self {
        self
            // Workspace CYCLING must be wired on EVERY screen root (it was only on the doc
            // view — so Ctrl-Tab was dead whenever an agent/edit/browser tile was
            // focused, the reported "ctrl-tab does nothing" bug). Folded into
            // `workspace_nav` so it can never again be present on some screens and not
            // others. (Bindings: reliable `cmd-shift-[`/`]` + legacy ctrl-tab.)
            .on_action(cx.listener(YaldaGpuiView::next_workspace))
            .on_action(cx.listener(YaldaGpuiView::prev_workspace))
            .on_action(cx.listener(|t, _: &GotoWorkspace1, _w, cx| t.goto_workspace_number(1, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace2, _w, cx| t.goto_workspace_number(2, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace3, _w, cx| t.goto_workspace_number(3, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace4, _w, cx| t.goto_workspace_number(4, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace5, _w, cx| t.goto_workspace_number(5, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace6, _w, cx| t.goto_workspace_number(6, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace7, _w, cx| t.goto_workspace_number(7, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace8, _w, cx| t.goto_workspace_number(8, cx)))
            .on_action(cx.listener(|t, _: &GotoWorkspace9, _w, cx| t.goto_workspace_number(9, cx)))
            .on_action(
                cx.listener(|t, _: &GotoWorkspace10, _w, cx| t.goto_workspace_number(10, cx)),
            )
    }
}

// ----------------------------------------------------------------------------
// gpui::Keystroke → yalda::keys::KeyPress bridge
// ----------------------------------------------------------------------------

/// Convert a GPUI keystroke to our framework-neutral `KeyPress` so the
/// `KeybindManager` + `Action` vocabulary can drive the GPUI edit mode.
/// SHIFT is omitted by convention — uppercase chars are encoded as
/// `Key::Char('G')` with no SHIFT modifier.
fn keystroke_to_keypress(ks: &Keystroke) -> KeyPress {
    let mut mods = KMods::NONE;
    if ks.modifiers.control {
        mods |= KMods::CONTROL;
    }
    if ks.modifiers.alt {
        mods |= KMods::ALT;
    }
    if ks.modifiers.platform {
        // Cmd on macOS. Preserved so the edit/compose dispatch can reject
        // unbound Cmd chords instead of typing/executing their bare letter.
        mods |= KMods::PLATFORM;
    }
    let key = match ks.key.as_str() {
        "enter" => Key::Enter,
        "tab" if ks.modifiers.shift => Key::BackTab,
        "tab" => Key::Tab,
        "escape" => Key::Esc,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" => Key::Char(' '),
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => Key::F(s[1..].parse().unwrap()),
        s => {
            // Prefer key_char when present (handles shifted chars properly:
            // shift-g → key="g", key_char=Some("G")).
            let ch_str = ks.key_char.as_deref().unwrap_or(s);
            ch_str.chars().next().map(Key::Char).unwrap_or(Key::Other)
        }
    };
    KeyPress::new(key, mods)
}

// ----------------------------------------------------------------------------
// Theme palette
// ----------------------------------------------------------------------------

/// `flex_none()` is essential — without it the caret can be shrunk to 0px
/// inside the flex_wrap row when other items want more space, making the
/// cursor appear to vanish. The bar is also a few pixels wider than a
/// typical text caret because, on a wrapped row of monospace text, a 1-2px
/// strip is easy to miss between adjacent glyphs.
fn make_caret(mode: EditMode, cursor_char: char, cursor_color: Hsla) -> AnyElement {
    // Block cursor in both modes. In insert mode the block is a solid
    // rectangle (character stays in the after-stream); in normal mode the
    // character under the cursor is drawn inside the block.
    div()
        .flex_none()
        .w(px(8.0))
        .h(px(18.0))
        .bg(cursor_color)
        .text_color(rgb(BG))
        .child(if mode == EditMode::Normal {
            cursor_char.to_string()
        } else {
            " ".into()
        })
        .into_any_element()
}

/// The two-theme toggle decision (Nightfox ⇄ Folio): from Folio go to Nightfox,
/// from anything else go to Folio. Pure so it's unit-testable without touching
/// `set_theme`'s persistence/render side effects.
fn next_toggle_theme(current: ThemeName) -> ThemeName {
    if current == ThemeName::Folio {
        ThemeName::Nightfox
    } else {
        ThemeName::Folio
    }
}

/// Format a menu node's key sequence for display (`"f"`, `"g g"`,
/// `"Ctrl-K"`). Single keys with no modifiers render as the bare char;
/// modifiers get capitalized prefixes; multi-key sequences are space-joined.
fn format_menu_key(seq: &[KeyPress]) -> String {
    seq.iter()
        .map(|kp| {
            let mut parts: Vec<String> = Vec::new();
            if kp.modifiers.contains(KMods::CONTROL) {
                parts.push("Ctrl".into());
            }
            if kp.modifiers.contains(KMods::ALT) {
                parts.push("Alt".into());
            }
            let key_str = match kp.key {
                Key::Char(c) => c.to_string(),
                Key::Enter => "Enter".into(),
                Key::Tab => "Tab".into(),
                Key::Esc => "Esc".into(),
                Key::Backspace => "Backspace".into(),
                Key::Delete => "Del".into(),
                Key::Up => "Up".into(),
                Key::Down => "Down".into(),
                Key::Left => "Left".into(),
                Key::Right => "Right".into(),
                Key::Home => "Home".into(),
                Key::End => "End".into(),
                Key::PageUp => "PgUp".into(),
                Key::PageDown => "PgDn".into(),
                Key::F(n) => format!("F{}", n),
                _ => "?".into(),
            };
            parts.push(key_str);
            parts.join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- Command panel ("The Sigil Card") layout constants (UXI-Menu-1) ---------
//
// The leader menu floats as a content-sized card in the workspace region rather
// than a full-width drop-down bar. Width tracks content within this band; the top
// edge is pinned so descent reads as the card breathing, not teleporting.
pub(crate) const MENU_PANEL_MIN_W: f32 = 340.0;
pub(crate) const MENU_PANEL_MAX_W: f32 = 720.0;
pub(crate) const MENU_PANEL_TOP: f32 = 48.0;
const DEFAULT_DESKTOP_GRID_COLS: u32 = 4;
const DEFAULT_DESKTOP_GRID_ROWS: u32 = 4;
const DESKTOP_GRID_DEFAULTS_VERSION: u8 = 3;
const TWO_BY_TWO_MIGRATION_VERSION: u8 = 2;

/// Restore one desktop-grid axis, including the one-time migration from the
/// original 2×2 built-in default. Once version 2 has been saved, `2` is an
/// explicit user choice and remains untouched.
fn restore_desktop_grid_axis(saved: Option<u32>, version: Option<u8>, default: u32) -> u32 {
    match saved {
        Some(2) if version.unwrap_or(1) < TWO_BY_TWO_MIGRATION_VERSION => default,
        Some(value) => value.clamp(1, 12),
        None => default,
    }
}

/// Restore both axes together so the shipped 3×3 density can migrate to 4×4
/// without rewriting a deliberate asymmetric choice such as 5×3. Version 3 is
/// written on the next settings save; after that, an explicit 3×3 remains 3×3.
fn restore_desktop_grid(
    saved_cols: Option<u32>,
    saved_rows: Option<u32>,
    version: Option<u8>,
) -> (u32, u32) {
    if version.unwrap_or(1) < DESKTOP_GRID_DEFAULTS_VERSION
        && saved_cols == Some(3)
        && saved_rows == Some(3)
    {
        return (DEFAULT_DESKTOP_GRID_COLS, DEFAULT_DESKTOP_GRID_ROWS);
    }
    (
        restore_desktop_grid_axis(saved_cols, version, DEFAULT_DESKTOP_GRID_COLS),
        restore_desktop_grid_axis(saved_rows, version, DEFAULT_DESKTOP_GRID_ROWS),
    )
}

/// Left gutter between the jump panel and the card, so the card sits just inside
/// the tile region — offset enough that it doesn't line up flush with the first
/// tile's edge (which read as an accidental alignment).
pub(crate) const MENU_PANEL_LEFT_PAD: f32 = 30.0;

/// Command-panel background: an **elevated** surface — a touch LIGHTER than the
/// editor/tile background at the same hue + saturation — so the card stands out
/// from both the workspace/tiles (which use `editor_bg`) and the recessed jump bar
/// (`jump_panel_bg`, which goes the other way). A fixed ΔL, clamped, so it lifts on
/// dark themes and reads as a near-white note on light ones without muddying the hue.
pub(crate) fn menu_panel_bg(editor: Hsla) -> Hsla {
    let l = if editor.l >= 0.5 {
        (editor.l + 0.04).min(0.985) // light themes: a lifted near-white card
    } else {
        (editor.l + 0.055).min(1.0) // dark themes: a soft lift so it glows above the bg
    };
    Hsla { l, ..editor }
}

/// Build the keystroke trail for the current menu depth (UXI-Menu-3): the leader
/// glyph followed by each descended submenu key, plus the name of the level you're
/// now in. Returns `(crumbs, current_label)` where `crumbs[0]` is the leader glyph,
/// `crumbs[1..]` are the descended submenu keys (formatted for display), and
/// `current_label` is the scope name at root or the deepest submenu's label.
///
/// Pure over `(menu, path, leader_glyph, scope)` — no view state — so it unit-tests
/// directly (`menu_trail_crumbs_tracks_descent`). Mirrors `MenuState::current_label`'s
/// walk but also records the key at each step.
pub(crate) fn menu_trail_crumbs(
    menu: &[MenuNode],
    path: &[usize],
    leader_glyph: &str,
    scope: &str,
) -> (Vec<String>, String) {
    let mut crumbs = vec![leader_glyph.to_string()];
    let mut nodes = menu;
    let mut label = scope.to_string();
    for &idx in path {
        if let Some(node) = nodes.get(idx) {
            crumbs.push(format_menu_key(&node.key));
            label = node.label.clone();
            if let MenuAction::Submenu(children) = &node.action {
                nodes = children;
            }
        }
    }
    (crumbs, label)
}

// ----------------------------------------------------------------------------
// Claude (ACP) helpers — port of app::claude splice/lock logic
// ----------------------------------------------------------------------------

/// Convert a rope char index to (line, col). Document doesn't expose this
// ----------------------------------------------------------------------------
// Root view
// ----------------------------------------------------------------------------

/// State held while the user is viewing a rendered markdown document.
struct DocState {
    blocks: Vec<RenderedBlock>,
    file_label: SharedString,
    cursor_block: usize,
    /// Variable-height virtualized list driving the doc body. Only the visible
    /// block window is built/laid-out per frame (not one element per block), so
    /// render is O(visible). j/k/g/G/ctrl-d/u nav reveals the focused block via
    /// `scroll_to_reveal_item`. Reconciled by splicing the changed block range
    /// (never `reset()`) so scroll stays anchored across a live edit-flush — see
    /// `ScrollAnchoredList`. Gated on `blocks_seq` (the `reconcile`'s version).
    list: ScrollAnchoredList<RenderedBlock>,
    /// Monotonic version of `blocks`, bumped by `set_blocks` on every
    /// reassignment. Plays the role `Document.edit_seq` plays for
    /// `EditState.lines_cache` — the key the render snapshot is memoized on,
    /// so no caller has to remember a manual invalidation step.
    blocks_seq: u64,
    /// O(1)-cloneable snapshot of `blocks` handed to the `'static` list
    /// render closure. Rebuilt lazily (a single full clone) only when
    /// `blocks_seq` advanced past the stamp it was built at, mirroring
    /// `EditState.lines_cache` keyed on `edit_seq`. Steady-state frames pay
    /// only a pointer clone, matching the agent transcript's `lines_rc`
    /// pattern. Stores the `blocks_seq` it was built at.
    blocks_snapshot: RefCell<Option<(u64, Rc<Vec<RenderedBlock>>)>>,
    /// `cursor_block` value last revealed during render. When the focused
    /// block changes, render re-issues `scroll_to_reveal_item` with the
    /// freshly-spliced item count — catching nav actions that fired before
    /// the list was populated (stale count) and keeping the cursor bar
    /// on-screen. `None` until the first render.
    last_cursor_block: std::cell::Cell<Option<usize>>,
    /// The pooled, shared source this Doc renders (D2 / 5c). `Some` for
    /// file-backed Docs — the SAME `SharedCore` an Edit view of the file
    /// binds to, so editing in Edit shows live in Doc and undo is unified.
    /// `None` for string-backed Docs (help/welcome) and transient
    /// placeholders. Replaces the old `edit_cache` stash: the shared core IS
    /// the live state, so there is nothing to shuttle across a Doc↔Edit
    /// round-trip.
    source: Option<DocSource>,
}

/// A file-backed Doc's handle onto its pooled `SharedCore` (5c). Held so the
/// Doc renders the file's *live* rope (shared with any Edit view) and so the
/// pool's `Rc`-strong-count liveness keeps the buffer alive while the Doc is
/// open.
struct DocSource {
    buffer_id: workspace::FileBufferId,
    core: workspace::SharedCore,
    /// `Document.edit_seq()` the current `blocks` were derived at. The
    /// per-frame `refresh_blocks` re-derives only when the core has advanced
    /// past this — O(1) when idle, one re-parse per change (the two-tile live
    /// path; memoized exactly like `EditState.lines_cache`).
    rendered_seq: u64,
}

impl DocSource {
    /// Build a source from a pooled `(buffer_id, core)`, stamping
    /// `rendered_seq` at the core's current `edit_seq` (caller renders the
    /// matching initial `blocks`).
    fn new(buffer_id: workspace::FileBufferId, core: workspace::SharedCore) -> Self {
        let rendered_seq = core.borrow().document().edit_seq();
        Self {
            buffer_id,
            core,
            rendered_seq,
        }
    }
    fn full_text(&self) -> String {
        self.core.borrow().document().full_text()
    }
    fn edit_seq(&self) -> u64 {
        self.core.borrow().document().edit_seq()
    }
    fn is_modified(&self) -> bool {
        self.core.borrow().document().is_modified()
    }
}

impl DocState {
    /// Build a `Viewing` Doc from rendered blocks — the SINGLE construction path
    /// for every Doc tile (load / reload / split / restore / theme re-render).
    /// Centralizing it keeps the list-reconcile bookkeeping in one place instead
    /// of re-spelling ~10 struct literals (each of which would have to stay in
    /// lockstep with field changes — the trap that made the scroll-anchor fix
    /// touch a dozen sites).
    fn viewing(
        blocks: Vec<RenderedBlock>,
        file_label: SharedString,
        source: Option<DocSource>,
    ) -> Self {
        DocState {
            blocks,
            file_label,
            cursor_block: 0,
            // Top-aligned: a doc reads from its first block (the agent transcript
            // tails the bottom). 512px default item-height estimate as before.
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, gpui::px(512.0)),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            source,
        }
    }

    /// Replace `blocks` and bump `blocks_seq`. The render snapshot is keyed on
    /// `blocks_seq` (see `blocks_rc`), so the next render rebuilds it lazily —
    /// no separate invalidation call to remember. This is the only path that
    /// mutates `blocks` in place after construction.
    fn set_blocks(&mut self, blocks: Vec<RenderedBlock>) {
        self.blocks = blocks;
        self.blocks_seq = self.blocks_seq.wrapping_add(1);
    }

    /// Re-derive `blocks` from the shared core if it has advanced since the
    /// last derivation (5c live path: an Edit view's keystroke bumps the
    /// shared `edit_seq`, and the next frame re-renders this Doc). O(1) when
    /// idle; one markdown parse per change. Uses a READ-ONLY borrow of the
    /// core — never `borrow_mut` here — so a concurrent Edit mutation on the
    /// same core cannot trigger a `RefCell` double-borrow panic. No-op for
    /// string-backed Docs (`source == None`).
    fn refresh_blocks(&mut self, theme: &Theme) {
        let (seq, text) = match &self.source {
            Some(src) => {
                let seq = src.edit_seq();
                if seq == src.rendered_seq {
                    return;
                }
                (seq, src.full_text())
            }
            None => return,
        };
        let path = PathBuf::from(self.file_label.as_ref());
        let blocks = render_with_wiki(&text, theme, Some(&path));
        self.set_blocks(blocks);
        if let Some(src) = self.source.as_mut() {
            src.rendered_seq = seq;
        }
    }

    /// O(1) pointer clone of the blocks snapshot, rebuilding it (one full
    /// clone) only when `blocks_seq` has advanced past the version the cached
    /// snapshot was built at. Mirrors `EditState.lines_cache` keyed on
    /// `edit_seq`.
    fn blocks_rc(&self) -> Rc<Vec<RenderedBlock>> {
        let mut slot = self.blocks_snapshot.borrow_mut();
        if let Some((seq, rc)) = slot.as_ref()
            && *seq == self.blocks_seq
        {
            return rc.clone();
        }
        let rc = Rc::new(self.blocks.clone());
        *slot = Some((self.blocks_seq, rc.clone()));
        rc
    }

    /// Scroll the virtualized list so `idx` is on-screen. Guarded against a
    /// stale item count (the list is spliced during render; a nav action that
    /// fires before the first render of a freshly-loaded doc would otherwise
    /// index past the registered count). The next render also re-reveals via
    /// `last_cursor_block`, so an early no-op here is harmless.
    fn reveal_block(&self, idx: usize) {
        if idx < self.list.len() {
            self.list.state().scroll_to_reveal_item(idx);
        }
    }

    /// Reconcile the virtualized block list to the current `blocks`, preserving
    /// scroll. Delegates to `ScrollAnchoredList`, gated on `blocks_seq` so an
    /// idle frame (no edit) does zero work.
    fn reconcile_list(&self) {
        self.list.reconcile(&self.blocks_rc(), self.blocks_seq);
    }
}

/// State held while the user is browsing the filesystem.
///
/// `underlying`: when the browser (picker) was opened *in place* of an
/// existing Buffer view (Cmd-O / inplace-buffer-pick from a focused Buffer
/// tile), this holds that prior `BufferApp` mode so Esc/q can restore it
/// (B4). Typed `BufferApp`, never `App` (D3/C4): a picker can only ever
/// restore a Buffer, never an Agent — browser-over-Agent is unrepresentable.
/// Invariant: never `Picking`. `None` when the picker was opened standalone
/// (new-buffer-tile, initial cwd browser, splits that fall back to a picker).
/// In-memory only — not persisted with the workspace snapshot.
struct BrowserWindow {
    fb: FileBrowser,
    underlying: Option<Box<BufferApp>>,
    /// Scroll position of the entry list, so the viewport follows the cursor
    /// (`scroll_to_item(selected)` each render). UI state, not persisted.
    scroll: ScrollHandle,
}

impl BrowserWindow {
    /// Standalone browser — no prior content to restore on Esc.
    fn standalone(dir: PathBuf) -> Self {
        Self {
            fb: FileBrowser::new(dir),
            underlying: None,
            scroll: ScrollHandle::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum EditMode {
    #[default]
    Normal,
    Insert,
}

/// Result of `dispatch_normal_core` so the calling screen (Edit / Claude)
/// can decide how to surface it (status message, quit, plain re-render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NormalOutcome {
    /// No action matched — caller should not even notify.
    Skipped,
    /// Action ran; caller should `cx.notify()`.
    Handled,
    /// `yank-selection` ran; caller may surface a "yanked" status hint.
    Yanked,
    /// User asked to quit the app.
    Quit,
    /// User pressed the `open-menu` binding (Space by default). Caller
    /// should open the menu overlay; the editor / mode were not modified.
    OpenMenu,
    /// User pressed `p`/`P` (put). The core can't reach the system
    /// clipboard (that lives on `YaldaGpuiView`), so it defers: the caller
    /// reads the clipboard and inserts it charwise. `before` is true for
    /// `P` (insert at cursor) vs `p` (insert after cursor).
    Paste {
        before: bool,
    },
}

/// Which rendering style the Edit screen uses. Both views share the same
/// `Editor` underneath — toggling is purely a display change, no buffer
/// migration. `Ctrl-W` flips between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditView {
    /// Raw markdown source with monospace + per-line syntax highlighting.
    Code,
    /// Live-preview "word processor" rendering: proportional font, headings
    /// at variable sizes, list markers, inline bold/italic styling. Markdown
    /// source markers (`#`, `*`, `_`, `-`) stay visible (rendered dim) so
    /// the user can edit them.
    WordProcessor,
}

/// A file-backed editor handle whose `EditorCore` lives in the workspace
/// buffer pool (`Rc<RefCell<EditorCore>>`) and is therefore shared by every
/// window viewing the same file. The per-window `EditorView` (cursor,
/// selection, insert flag) is owned here, so each split / also-shown tile
/// navigates independently while edits + undo land on one shared rope.
///
/// `buffer_id` is the pool key; the owning window must `buffer_release` it on
/// close so refcounting can drop clean, unreferenced buffers.
struct SharedEditor {
    /// Pool key for this file's core. Liveness is tracked via `Rc` strong
    /// count (see `Frame::gc_buffers`), so this id isn't needed for
    /// refcounting; it's kept as the stable handle for future explicit pool
    /// ops (save-to-pool, `:buffers`).
    #[allow(dead_code)]
    buffer_id: workspace::FileBufferId,
    core: workspace::SharedCore,
    view: EditorView,
}

impl SharedEditor {
    fn new(buffer_id: workspace::FileBufferId, core: workspace::SharedCore) -> Self {
        Self {
            buffer_id,
            core,
            view: EditorView::new(),
        }
    }

    // --- Read accessors (return owned snapshots; can't lend `&` out of the
    //     RefCell with a `&self` signature). ---

    fn cursor(&self) -> CursorPos {
        *self.view.cursor()
    }
    fn set_cursor(&mut self, line: usize, col: usize) {
        let c = self.view.cursor_mut();
        c.line = line;
        c.col = col;
    }
    /// Replace the character under the cursor (vim `r`). See
    /// [`EditorView::replace_char_at_cursor`].
    fn replace_char_at_cursor(&mut self, ch: char) {
        self.view
            .replace_char_at_cursor(&mut self.core.borrow_mut(), ch);
    }
    fn is_modified(&self) -> bool {
        self.core.borrow().document().is_modified()
    }
    fn line_count(&self) -> usize {
        self.core.borrow().document().line_count()
    }
    /// Monotonic change counter — bumps on every buffer mutation. Used to key
    /// the Edit view's incremental highlight cache so unchanged frames recompute
    /// zero lines.
    fn edit_seq(&self) -> u64 {
        self.core.borrow().document().edit_seq()
    }
    fn full_text(&self) -> String {
        self.core.borrow().document().full_text()
    }

    // --- Selection / mode (view-only) ---

    fn extend_mode(&self) -> bool {
        self.view.extend_mode()
    }
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        self.view.selection_range()
    }
    fn selection_text(&self) -> Option<String> {
        self.view.selection_text(&self.core.borrow())
    }
    fn clear_selection(&mut self) {
        self.view.clear_selection();
    }

    // --- Mutations that EditState drives directly ---

    fn insert_char(&mut self, ch: char) {
        self.view.insert_char(&mut self.core.borrow_mut(), ch);
    }
    fn save(&mut self) -> std::io::Result<()> {
        self.core.borrow_mut().save()
    }
}

impl EditAccess for SharedEditor {
    fn view(&self) -> &EditorView {
        &self.view
    }
    fn view_mut(&mut self) -> &mut EditorView {
        &mut self.view
    }
    fn read_core<R>(&self, f: impl FnOnce(&EditorCore) -> R) -> R {
        f(&self.core.borrow())
    }
    fn edit<R>(&mut self, f: impl FnOnce(&mut EditorView, &mut EditorCore) -> R) -> R {
        f(&mut self.view, &mut self.core.borrow_mut())
    }
}

/// The vim-ish editing vocabulary the dispatch core (`dispatch_normal_core` /
/// `dispatch_insert_core`) drives. Every method is a default implemented ONCE
/// over [`EditAccess`], so an owned [`Editor`] and a pool-backed [`SharedEditor`]
/// share the exact same bodies — no more two hand-written delegation impls kept
/// in lockstep. `EditorView` motions take `&EditorCore`; `edit()` hands them a
/// `&mut EditorCore` that reborrows down.
trait EditOps: EditAccess {
    fn cursor(&self) -> CursorPos {
        *self.view().cursor()
    }
    fn cursor_set(&mut self, line: usize, col: usize) {
        // Absolute placement (selection anchor → insert): clear the sticky
        // vertical column so a later clamp/j/k doesn't snap to a stale one.
        self.view_mut().cursor_mut().set_pos(line, col);
    }
    fn cursor_move_left(&mut self) {
        self.view_mut().cursor_mut().move_left();
    }
    fn cursor_move_up(&mut self) {
        self.view_mut().cursor_mut().move_up();
    }
    fn cursor_move_line_start(&mut self) {
        self.view_mut().cursor_mut().move_line_start();
    }
    fn cursor_jump_top(&mut self) {
        self.view_mut().cursor_mut().jump_top();
    }
    fn line_len_chars(&self, line: usize) -> usize {
        self.read_core(|c| c.document().line_len_chars(line))
    }
    fn line_text_at_cursor(&self) -> String {
        let line = self.view().cursor().line;
        self.read_core(|c| c.document().line_text(line))
    }

    fn extend_mode(&self) -> bool {
        self.view().extend_mode()
    }
    fn set_extend_mode(&mut self, on: bool) {
        self.view_mut().set_extend_mode(on);
    }
    fn toggle_extend_mode(&mut self) {
        self.view_mut().toggle_extend_mode();
    }
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        self.view().selection_range()
    }
    fn selection_anchor(&self) -> Option<CursorPos> {
        self.view().selection_anchor()
    }
    fn anchor_at_cursor(&mut self) {
        self.view_mut().anchor_at_cursor();
    }
    fn clear_selection(&mut self) {
        self.view_mut().clear_selection();
    }
    fn collapse_selection(&mut self) {
        self.view_mut().collapse_selection();
    }
    fn flip_selection(&mut self) {
        self.view_mut().flip_selection();
    }
    fn select_all(&mut self) {
        self.edit(|v, c| v.select_all(c));
    }
    fn extend_by_line(&mut self) {
        self.edit(|v, c| v.extend_by_line(c));
    }
    fn yank_selection(&self) -> Option<String> {
        self.read_core(|c| self.view().yank_selection(c))
    }

    fn pre_move(&mut self, creates_selection: bool) {
        self.view_mut().pre_move(creates_selection);
    }
    fn move_down(&mut self, insert_mode: bool) {
        self.edit(|v, c| v.move_down(c, insert_mode));
    }
    fn move_right_clamped(&mut self, insert_mode: bool) {
        self.edit(|v, c| v.move_right_clamped(c, insert_mode));
    }
    fn clamp_cursor_col(&mut self, insert_mode: bool) {
        self.edit(|v, c| v.clamp_cursor_col(c, insert_mode));
    }
    fn move_cursor_line_end(&mut self, insert_mode: bool) {
        self.edit(|v, c| v.move_cursor_line_end(c, insert_mode));
    }
    fn move_cursor_first_non_blank(&mut self) {
        self.edit(|v, c| v.move_cursor_first_non_blank(c));
    }
    fn move_cursor_word_forward(&mut self) {
        self.edit(|v, c| v.move_cursor_word_forward(c));
    }
    fn move_cursor_word_backward(&mut self) {
        self.edit(|v, c| v.move_cursor_word_backward(c));
    }
    fn move_cursor_word_end(&mut self) {
        self.edit(|v, c| v.move_cursor_word_end(c));
    }
    fn jump_cursor_bottom(&mut self) {
        self.edit(|v, c| v.jump_cursor_bottom(c));
    }
    fn jump_to_line(&mut self, line: usize) {
        self.edit(|v, c| v.jump_to_line(c, line));
    }
    fn line_count(&self) -> usize {
        self.read_core(|c| c.document().line_count())
    }

    fn begin_insert(&mut self) {
        self.edit(|v, c| v.begin_insert(c));
    }
    fn end_insert(&mut self) {
        self.edit(|v, c| v.end_insert(c));
    }
    fn insert_char(&mut self, ch: char) {
        self.edit(|v, c| v.insert_char(c, ch));
    }
    fn backspace(&mut self) {
        self.edit(|v, c| v.backspace(c));
    }
    fn delete_char_at_cursor(&mut self) {
        self.edit(|v, c| v.delete_char_at_cursor(c));
    }
    fn delete_current_line(&mut self) {
        self.edit(|v, c| v.delete_current_line(c));
    }
    fn delete_selection(&mut self) -> bool {
        self.edit(|v, c| v.delete_selection(c))
    }
    fn open_line_below(&mut self) {
        self.edit(|v, c| v.open_line_below(c));
    }
    fn open_line_above(&mut self) {
        self.edit(|v, c| v.open_line_above(c));
    }
    fn undo(&mut self) {
        self.edit(|v, c| v.undo(c));
    }
    fn redo(&mut self) {
        self.edit(|v, c| v.redo(c));
    }
}

impl EditOps for Editor {}
impl EditOps for SharedEditor {}

// Build the trimmed, wsp-expanded per-line text for an Edit tile's body,
// reading the pooled core's rope once. Mirrors the prior per-line
// `document().line_text(i)` loop but takes a single `RefCell` borrow.

/// State held while the user is editing a buffer in the GPUI frontend.
/// Raw buffer + cursor + Insert/Normal toggle + vim-style normal-mode
/// actions routed through the shared `KeybindManager`/`Action` vocabulary.
/// Source lines are syntax-highlighted via `md_highlight`.
/// Deferred: IME.
struct EditState {
    editor: SharedEditor,
    file_label: SharedString,
    mode: EditMode,
    keybinds: KeybindManager,
    /// Transient footer message — last save outcome ("saved", "save failed: …").
    /// Cleared on the next keystroke that mutates the buffer; persists across
    /// pure motion so the user sees the result for at least one render.
    last_save_msg: Option<SharedString>,
    /// Code (raw monospace + syntax highlight) or WordProcessor (live-preview
    /// proportional + typographic styling). Toggled by `Ctrl-W`.
    view: EditView,
    /// Incremental per-line highlight cache, keyed on the document's
    /// `edit_seq`. Re-highlights only changed lines instead of the whole
    /// buffer every frame, so fast typing stays O(changed) rather than
    /// O(document). Shared between the Code and WordProcessor views — both
    /// consume the `raw` segments of each `LineHl`.
    highlight_cache: HighlightCache,
    /// `edit_seq` the `lines_cache` was extracted at; `u64::MAX` = never built.
    lines_cache_seq: u64,
    /// Last extracted (tab-expanded, newline-trimmed) source lines, reused
    /// verbatim on frames where `edit_seq` is unchanged (cursor blink,
    /// selection, cross-tile notify) so we don't re-allocate a String per line.
    lines_cache: std::rc::Rc<Vec<String>>,
    /// Virtualized line list — only the visible rows are built/laid-out each
    /// frame instead of one element per document line. Variable height (lines
    /// wrap), so a `ListState` rather than a fixed-row viewport. Reconciled by
    /// splicing the changed range (never `reset()`) so scroll stays anchored
    /// across edits — see `ScrollAnchoredList`.
    list: ScrollAnchoredList<String>,
    /// `(edit_seq, cursor_line, cursor_col)` at the last render. When any
    /// changes we scroll the list to reveal the cursor line (so typing/motion
    /// keeps the caret on-screen) without fighting the user's manual scroll on
    /// idle frames. The COLUMN is part of the key because a horizontal move
    /// along a wide soft-wrapped line (e.g. a markdown table row, which is one
    /// long source line) changes the caret's *visual* row without changing
    /// `cursor_line` — without the column here that move never re-revealed and
    /// the caret drifted off the bottom of the wrapped rows.
    last_cursor_anchor: Option<(u64, usize, usize)>,
    /// Per-line WordProcessor typographic kinds, cached on `edit_seq` (mirrors
    /// `lines_cache`). `classify_wp_line` is folded over the WHOLE buffer; without
    /// this the WP render re-scanned every line on every idle frame (cursor blink,
    /// selection, theme/scroll, cross-tile notify). Now only an edit recomputes;
    /// idle frames reuse the `Rc` (O(changed), not O(document)).
    wp_kinds_cache: std::rc::Rc<Vec<WpLineKind>>,
    /// `edit_seq` the `wp_kinds_cache` was built at; `u64::MAX` = never built.
    wp_kinds_cache_seq: u64,
    /// Set after `r` in normal mode: the *next* keypress is consumed as the
    /// replacement character (vim `r{char}`) rather than a normal-mode action.
    /// Cleared after that next key (Esc / non-char cancels).
    pending_replace: bool,
}

impl EditState {
    fn new(editor: SharedEditor, file_label: SharedString, view: EditView) -> Self {
        Self {
            editor,
            file_label,
            mode: EditMode::Normal,
            keybinds: KeybindManager::default(),
            last_save_msg: None,
            view,
            highlight_cache: HighlightCache::new(),
            lines_cache_seq: u64::MAX,
            lines_cache: std::rc::Rc::new(Vec::new()),
            // Top-aligned: editing reads from the top of the buffer, unlike the
            // agent transcript which tails the bottom.
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, gpui::px(256.0)),
            last_cursor_anchor: None,
            pending_replace: false,
            wp_kinds_cache: std::rc::Rc::new(Vec::new()),
            wp_kinds_cache_seq: u64::MAX,
        }
    }

    /// Reconcile the row list to `lines` by splicing ONLY the changed range —
    /// never `reset()` (that drops scroll + measurements and snaps the viewport
    /// to the top on every newline edit) — then reveal the caret's line so it
    /// stays on-screen (UXI-TextEditing-1). Shared by the Code and WordProcessor bodies so
    /// caret-follows-scroll lives in exactly one place, not two verbatim copies.
    /// Returns the reconciled row count.
    fn reconcile_and_reveal(
        &mut self,
        lines: &std::rc::Rc<Vec<String>>,
        edit_seq: u64,
        cursor_line: usize,
        cursor_col: usize,
    ) -> usize {
        self.list.reconcile(lines, edit_seq);
        let new_count = self.list.len();
        let anchor = (edit_seq, cursor_line, cursor_col);
        if self.last_cursor_anchor != Some(anchor) {
            self.last_cursor_anchor = Some(anchor);
            if cursor_line < new_count {
                self.list.state().scroll_to_reveal_item(cursor_line);
            }
        }
        new_count
    }

    /// Per-line WordProcessor typographic kinds, cached on `edit_seq` (mirrors
    /// `highlight_snapshot`/`lines_cache`). `classify_wp_line` carries fence
    /// state so it must be folded in order over the whole buffer; this makes that
    /// fold run once per edit instead of once per frame, so idle frames (cursor
    /// blink, selection, scroll, theme, cross-tile notify) reuse the `Rc`.
    fn wp_kinds_snapshot(
        &mut self,
        lines: &std::rc::Rc<Vec<String>>,
        edit_seq: u64,
    ) -> std::rc::Rc<Vec<WpLineKind>> {
        if self.wp_kinds_cache_seq != edit_seq {
            let mut kinds = Vec::with_capacity(lines.len());
            let mut in_fence = false;
            for line_str in lines.iter() {
                let kind = classify_wp_line(line_str, in_fence);
                if matches!(kind, WpLineKind::CodeFence) {
                    in_fence = !in_fence;
                }
                kinds.push(kind);
            }
            self.wp_kinds_cache = std::rc::Rc::new(kinds);
            self.wp_kinds_cache_seq = edit_seq;
        }
        self.wp_kinds_cache.clone()
    }

    /// Extract + highlight the buffer's source lines incrementally. Returns the
    /// shared source-line vector and a per-line highlight snapshot whose `raw`
    /// segments are byte-identical to `highlight_markdown_lines_syn`. Both are
    /// keyed on the document's `edit_seq`, so a frame that didn't edit the
    /// buffer recomputes zero lines and a single-char edit recomputes ~1.
    fn highlight_snapshot(
        &mut self,
        theme: &Theme,
        hl: &yalda::highlight::Highlighter,
    ) -> (
        std::rc::Rc<Vec<String>>,
        std::rc::Rc<Vec<std::rc::Rc<LineHl>>>,
    ) {
        let line_count = self.editor.line_count();
        let edit_seq = self.editor.edit_seq();
        let lines_rc: std::rc::Rc<Vec<String>> = if self.lines_cache_seq == edit_seq {
            self.lines_cache.clone()
        } else {
            let core = self.editor.core.borrow();
            let doc = core.document();
            let built: Vec<String> = (0..line_count.max(1))
                .map(|i| {
                    doc.line_text(i)
                        .trim_end_matches('\n')
                        .replace('\t', "    ")
                })
                .collect();
            drop(core);
            let rc = std::rc::Rc::new(built);
            self.lines_cache = rc.clone();
            self.lines_cache_seq = edit_seq;
            rc
        };
        let snap = self
            .highlight_cache
            .snapshot_syn(&lines_rc, theme, edit_seq, hl);
        (lines_rc, snap)
    }
}

// wire/event enum — boxing the large variant would ripple through serialization + every match site
//
// `App` is the per-Tile content (ADR-0019 / spec-tiles-and-apps.md). A Tile
// holds exactly one App: a `Buffer` (a view onto the pooled file buffer, in one
// of three modes — see `BufferApp`) or an `Agent` (the ACP session ring).
#[allow(clippy::large_enum_variant)]
enum App {
    Buffer(BufferApp),
    Agent(AgentTile),
    Linear(LinearTile),
    /// The keybindings reference + rebind sheet (`keymap_tile.rs`).
    Keymap(KeymapTile),
}

impl App {
    /// Narrow an `App` into the `BufferApp` stash an Agent/picker can restore
    /// (D3/C4). A Buffer yields its mode; an Agent yields `None` — an Agent can
    /// never be stashed behind another Agent or behind a picker, and the
    /// no-stash case falls back to a fresh `Picking` at restore time (B6).
    fn into_buffer_stash(self) -> Option<Box<BufferApp>> {
        match self {
            App::Buffer(buffer) => Some(Box::new(buffer)),
            // Agent/Linear and Buffer are orthogonal — they stash no buffer.
            App::Agent(_) => None,
            App::Linear(_) => None,
            App::Keymap(_) => None,
        }
    }
}

/// A view onto the pooled file buffer, always in exactly one mode (B1):
/// `Picking` (file browser, no file chosen yet), `Viewing` (rendered markdown),
/// or `Editing` (raw markdown). `Viewing ⇄ Editing` toggle over the same pooled
/// `SharedCore` (ADR-0007). The `Picking` payload is the existing
/// `BrowserWindow`; `Viewing` tolerates a source-less `DocState` placeholder.
#[allow(clippy::large_enum_variant)]
enum BufferApp {
    Picking(BrowserWindow),
    Viewing(DocState),
    Editing(EditState),
}

/// Overlay popup that lets the user pick a top-level command by single
/// keypress (TUI-style — see `src/menu.rs` for the underlying tree model).
/// When `Some` on `YaldaGpuiView`, `Render::render` swaps the screen body
/// for `render_menu`; key dispatch routes to `handle_menu_key`. The menu
/// items here are GPUI-specific (a subset of the TUI default that maps to
/// actions the GPUI frontend implements).
struct MenuOverlay {
    state: MenuState,
    menu: Vec<MenuNode>,
    /// Scope tag shown in the overlay header (spec-menu-scopes.md): "MENU"
    /// for the global leader; "DOC"/"EDIT"/"AGENT"/"BROWSE" for the local one.
    header: &'static str,
    /// The leader key that opened this menu: `' '` (space, tile/app-local), `'.'`
    /// (workspace), or `'?'` (global). Drives the scope hue + sigil + trail glyph
    /// (UXI-Menu-3/-4); distinct from `header` since every local content kind shares
    /// the space leader.
    leader: char,
    /// Leaf focused when the menu was opened — if focus moves while the menu
    /// is open the overlay dismisses (Behavior 9: no stale dispatch).
    opened_from: workspace::WindowId,
    /// Command names disabled in the current context (Behavior 10). Rendered
    /// dimmed; key presses on them are ignored (menu stays open).
    disabled: HashSet<String>,
}

/// Overlay state for the buffer-list picker (TUI: `AppScreen::BufferList`).
struct BufferSwitcher {
    selected: usize,
    filter_mode: bool,
    filter_text: String,
}

/// What the workspace picker will do with the chosen target. Drives the
/// header copy and the commit branch so one picker overlay serves both
/// "move tile" (Ctrl-W m) and "also-show tile" (Ctrl-W M).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspacePickerMode {
    /// Relocate the focused leaf into the target workspace (content travels;
    /// works for every tile kind). Spec-workspaces-tagging.md Phase 1.
    Move,
    /// Open a second view onto the focused file-backed tile's file in the
    /// target workspace (file-backed tiles only). The original stays put.
    AlsoShow,
}

/// Picker overlay for "move tile to workspace" / "also-show tile in
/// workspace". Lists existing workspaces by display label, plus a trailing
/// "+ new workspace" entry that creates an empty workspace as the target.
/// The currently-active workspace is shown but selecting it is a no-op
/// (you can't move a tile to where it already lives).
struct WorkspacePicker {
    mode: WorkspacePickerMode,
    /// Index into the entry list: `0..workspaces.len()` are existing workspaces,
    /// `workspaces.len()` is the "+ new workspace" entry.
    selected: usize,
}

/// Single-line input overlay used by both Claude-session rename and
/// workspace rename. Pre-filled with the current label; Enter commits, Esc
/// cancels, empty input cancels.
struct RenameOverlay {
    text: String,
    target: RenameTarget,
}

#[derive(Clone, Copy)]
enum RenameTarget {
    /// Claude session — targeted by its stable `SessionId` so a concurrent
    /// close on another tile can't rename the wrong one.
    AgentSession { id: SessionId },
    /// Workspace — targeted by current workspace position. Workspace indices
    /// don't shift during the rename's lifetime since the overlay
    /// captures key dispatch (no structural mutations possible mid-
    /// rename), so positional addressing is safe here.
    Workspace { index: usize },
    /// Path-input overlay that, on commit, changes the bound session's
    /// cwd (spec-agent-cwd.md §4). Targeted by stable `SessionId`.
    AgentChangeCwd { id: SessionId },
    /// Path-input overlay that, on commit, writes the active workspace's
    /// registry `"cwd"` (untitled.md "Set CWD … implemented as a kv"). Agent
    /// sessions created in this workspace then inherit it. Targeted by current
    /// workspace position (safe: the overlay captures key dispatch, so no structural
    /// mutation can shift indices mid-edit, same as `Workspace`).
    WorkspaceCwd { index: usize },
    /// `{cols}x{rows}` input that sets the global desktop-mode tile size
    /// (spec-desktop-mode.md Behavior 6). Clamped to [20, 400] × [5, 200];
    /// unparseable input cancels with a footer hint.
    DesktopTileSize,
}

/// "New project" overlay (UXI-Project-4): asks only for the cwd. Its display
/// name is derived from the directory basename and uniquified by `Projects`.
struct NewProjectOverlay {
    cwd: String,
}

/// The single, mutually-exclusive overlay layered over the screen body — at
/// most one is open at a time (the spec's "make illegal states unrepresentable":
/// the five sibling `Option<…>` fields this replaces could, in principle, all be
/// `Some` at once, and one path did strand a `menu` behind a `rename`). `open()`
/// **replaces** whatever was active rather than stacking; `clear()` returns to
/// `None`. The render if-chain and key dispatch resolve the active variant in a
/// fixed precedence (rename > buffer > session > workspace > menu).
///
/// NB: the `Menu` variant's inner `MenuOverlay` has its OWN field named `menu`
/// (`Vec<MenuNode>`) — distinct from this variant; `menu_mut()` hands back the
/// whole `&mut MenuOverlay` so `MenuState::process_key`'s disjoint two-field
/// split-borrow (`m.state` + `&m.menu`) still type-checks. Do not add a
/// per-field `&mut state` accessor. Non-exclusive chrome (`transient_status`
/// toast, `splash_until`) is deliberately NOT folded in here.
/// The project-scoped actions offered by the jump-panel project context menu
/// (UXI-JumpPanel-8). Each maps to an existing project method; the menu is just a
/// discoverable, cursor-anchored entry point.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectMenuAction {
    NewWorkspace,
    NewAgentSession,
    DeleteProject,
}

#[derive(Default)]
enum ActiveOverlay {
    #[default]
    None,
    Menu(MenuOverlay),
    BufferSwitcher(BufferSwitcher),
    WorkspacePicker(WorkspacePicker),
    Rename(RenameOverlay),
    TagInput(TagInputOverlay),
    /// "New project" cwd input (UXI-Project-4).
    NewProject(NewProjectOverlay),
    /// "Delete project?" confirmation for a non-empty project (UXI-Project-5);
    /// carries the target so confirm cascades exactly it.
    ConfirmProjectDelete(ProjectId),
    /// The `Cmd-P` jump palette (UXI-JumpPanel-9): a fuzzy type-to-filter dialog
    /// over every non-ephemeral workspace + every agent session. Carries the
    /// query and the highlighted RANKED-list index.
    JumpPalette(JumpPaletteOverlay),
    /// Project context menu (UXI-JumpPanel-8): a small popup anchored at the
    /// click position, offering the project-scoped create/delete actions. Carries
    /// the target project + the window-space anchor point (already clamped to the
    /// viewport at open time).
    ProjectMenu { pid: ProjectId, x: f32, y: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagInputMode {
    Tag,
    Untag,
    ViewTag,
    SendTag,
    AlsoTag,
    TagBind,
}

#[derive(Debug, Clone)]
struct TagInputOverlay {
    mode: TagInputMode,
    text: String,
    prompt: &'static str,
}

/// GPUI menu tree. Mirrors the TUI's `default_menu` for the navigation
/// commands that exist in the GPUI frontend; omits TUI-only entries
/// (search, claude-attach via socket, save-quit, …) that have no GPUI
/// counterpart yet.
fn gpui_menu() -> Vec<MenuNode> {
    // Workspace-scoped command menu (`.` leader). Per untitled.md
    // "Workspace › Commands (12 jun)": only these commands belong in the
    // workspace scope. Tile-scoped commands live in the <space> local menus;
    // window/workspace/layout management lives on its Ctrl-W / Cmd chords;
    // quit is Cmd-Q.
    vec![
        MenuNode::entry("c", "set cwd", "workspace-set-cwd"),
        MenuNode::submenu(
            "n",
            "new",
            vec![
                MenuNode::entry("a", "agent", "new-agent-tile"),
                MenuNode::entry("b", "buffer", "new-buffer-tile"),
                MenuNode::entry("l", "linear", "new-linear-tile"),
                MenuNode::entry("k", "keybindings", "new-keymap-tile"),
            ],
        ),
        MenuNode::submenu(
            "t",
            "theme",
            vec![
                MenuNode::entry("n", "nightfox", "theme-nightfox"),
                MenuNode::entry("f", "folio", "theme-folio"),
            ],
        ),
        MenuNode::submenu(
            "l",
            "plane view",
            vec![
                MenuNode::entry("=", "zoom in", "plane-zoom-in"),
                MenuNode::entry("-", "zoom out", "plane-zoom-out"),
                MenuNode::entry("0", "reset to origin", "plane-reset-view"),
            ],
        ),
        MenuNode::entry("r", "rebuild and restart gui", "dev-restart-gui"),
        MenuNode::entry("R", "rebuild and restart all", "dev-restart-all"),
        MenuNode::entry("m", "mark tile", "mark-tile"),
        MenuNode::entry("x", "close tile", "close-window"),
    ]
}

// ---- Local menus (spec-menu-scopes.md Behavior 2) --------------------------
//
// One static tree per content kind, opened with the <space> local leader. Same
// overlay machinery as the global menu — only the tree (and header) differ.
// v1 contains only entries with existing GPUI backing; the spec's nav-*
// (links/headings/list-items/code-blocks) and browser-open-* entries are
// deferred until those features exist.

fn doc_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("e", "edit (raw markdown)", "enter-edit"),
        MenuNode::entry("w", "edit (word processor)", "enter-wp"),
        MenuNode::entry("r", "reload from disk", "reload-file"),
        MenuNode::entry("b", "file browser", "open-browser"),
        MenuNode::entry("o", "outline", "rail-outline"),
        MenuNode::separator(),
        MenuNode::submenu(
            "n",
            "navigate",
            vec![
                MenuNode::entry("l", "next link", "nav-links"),
                MenuNode::entry("h", "next heading", "nav-headings"),
                MenuNode::entry("i", "next list", "nav-list-items"),
                MenuNode::entry("c", "next code block", "nav-code-blocks"),
            ],
        ),
        MenuNode::submenu(
            "g",
            "goto",
            vec![
                MenuNode::entry("g", "top", "doc-goto-top"),
                MenuNode::entry("e", "bottom", "doc-goto-bottom"),
                MenuNode::entry("h", "next heading", "goto-heading"),
            ],
        ),
    ]
}

fn edit_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("v", "back to doc view", "back-to-doc"),
        MenuNode::entry("w", "toggle code/word-processor", "wp-toggle"),
        MenuNode::entry("r", "reload from disk", "reload-file"),
        MenuNode::entry("b", "file browser", "open-browser"),
        MenuNode::separator(),
        MenuNode::entry("a", "select all", "select-all"),
        MenuNode::entry("y", "yank selection", "yank-selection"),
        MenuNode::entry("d", "delete selection", "delete-selection"),
        MenuNode::submenu(
            "e",
            "edit",
            vec![
                MenuNode::entry("v", "extend mode", "toggle-extend-mode"),
                MenuNode::entry(";", "collapse selection", "collapse-selection"),
                MenuNode::entry(",", "flip selection", "flip-selection"),
                MenuNode::entry("x", "extend by line", "extend-line"),
            ],
        ),
    ]
}

fn agent_local_menu() -> Vec<MenuNode> {
    vec![
        // The four core agent commands (spec-agent-session-ownership.md).
        MenuNode::entry("c", "select session", "claude-session-picker"),
        MenuNode::entry("e", "send message", "claude-send"),
        MenuNode::entry(".", "stop", "claude-stop"),
        MenuNode::entry("w", "switch worksheet ⇄ message box", "agent-input-toggle"),
        MenuNode::separator(),
        MenuNode::entry("n", "new Claude session", "claude-new"),
        MenuNode::entry("N", "new Codex session", "codex-new"),
        MenuNode::entry("x", "close session", "claude-close"),
        MenuNode::entry("C", "clear session", "claude-clear"),
        MenuNode::entry("r", "rename session", "claude-rename"),
        MenuNode::entry("f", "focus transcript ⇄ compose", "agent-focus-toggle"),
        MenuNode::entry("S", "send selection", "claude-send-selection"),
        MenuNode::entry("m", "cycle permission mode", "claude-mode-cycle"),
        MenuNode::entry("h", "toggle heading markers", "agent-toggle-heading-markers"),
        MenuNode::entry("j", "jump between user turns (j/k)", "agent-toggle-jump-mode"),
        MenuNode::entry("R", "recap this session", "recap-session"),
    ]
}

fn linear_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("i", "edit query", "linear-edit"),
        MenuNode::entry("o", "open in browser", "linear-open-url"),
        MenuNode::entry("y", "copy URL", "linear-copy-url"),
    ]
}

fn keymap_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("i", "filter", "keymap-filter"),
        MenuNode::entry("r", "rebind selected", "keymap-rebind"),
        MenuNode::entry("x", "reset selected", "keymap-reset"),
        MenuNode::entry("R", "reset all to defaults", "keymap-reset-all"),
    ]
}

fn browser_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("s", "cycle sort", "browser-sort"),
        MenuNode::entry(".", "toggle hidden files", "browser-hidden"),
        MenuNode::entry("-", "go up", "browser-up"),
        MenuNode::separator(),
        MenuNode::entry("w", "open in new workspace", "browser-open-workspace"),
        MenuNode::entry("v", "open in split", "browser-open-split"),
    ]
}

struct YaldaGpuiView {
    theme: Theme,
    body_font: SharedString,
    code_font: SharedString,
    /// Multiplier applied to document body / heading font sizes (Cmd+= / Cmd+-
    /// / Cmd+0). Chrome (status bar, workspaces, file browser) stays fixed. 1.0 is
    /// the unzoomed default; clamped to [MIN_TEXT_SCALE, MAX_TEXT_SCALE] on
    /// every adjustment.
    text_scale: f32,
    /// Agent-chat-only: when true, headings in the transcript render with their
    /// literal markdown markers (`## `, `### `) shown before the rendered text.
    /// Default on; toggled via the agent `.` menu ("heading markers"). A global
    /// (all transcripts), pushed to `TranscriptView`s via `notify_transcript_
    /// views` (not a seq). The doc/edit views never show markers.
    show_agent_heading_markers: bool,
    /// The live keybinding registry — the single source of truth for every GPUI
    /// binding (`keymap_registry.rs`). Built from the default table + persisted
    /// user overrides at boot; the `App::Keymap` reference tile reads it to
    /// display bindings and mutates it (then re-applies to the app + persists)
    /// when the user rebinds a key.
    keymap_registry: KeymapRegistry,
    /// Desktop-mode tile size in mono cells (spec-desktop-mode.md
    /// Behavior 6) — one global setting for all tiles in all workspaces,
    /// persisted in `Preferences`, clamped to [20, 400] × [5, 200].
    desktop_grid_cols: u32,
    desktop_grid_rows: u32,
    /// Per-tile remembered file-explorer sort order, keyed by the tile's
    /// `WindowId`. A picker is short-lived (it's replaced by the picked file /
    /// the restored underlying buffer when closed), so its `SortOrder` would
    /// otherwise reset to `Name` every time the explorer is reopened in the
    /// same tile. Recorded on `browser_cycle_sort`, seeded back when the tile
    /// re-enters Picking (`open_browser_inner`). In-memory only; sparse (only
    /// tiles whose sort was changed from the default appear). Stale entries for
    /// closed tiles are harmless and tiny.
    browser_sort: HashMap<workspace::WindowId, SortOrder>,
    /// Desktop canvas bounds `(x, y, w, h)` in window coordinates, captured
    /// during paint (same idiom as `line_layouts`). Mouse listeners use it
    /// to convert window coords → desktop coords; the render pass uses the
    /// size for culling, pan clamping, and the drop-time effective width.
    desktop_canvas_bounds: std::rc::Rc<std::cell::Cell<(f32, f32, f32, f32)>>,
    /// Cached viewport height, sibling of `viewport_width_px` below.
    viewport_height_px: f32,
    /// Cached viewport width in pixels, updated every render frame from
    /// `Window::viewport_size()`. Used by the chatbox to compute visible
    /// columns for horizontal scroll tracking.
    viewport_width_px: f32,
    focus_handle: FocusHandle,
    /// The single mutually-exclusive overlay (menu / buffer-list / session /
    /// workspace picker / rename input). At most one open at a time — see
    /// [`ActiveOverlay`]. Was five sibling `Option<…>` fields.
    active_overlay: ActiveOverlay,
    /// One-shot footer message (e.g. "Only documents can be shown in multiple
    /// workspaces (yet)"). Rendered as a small toast in the bottom-right;
    /// cleared on the next overlay dismissal. Display-only. NOT part of
    /// `active_overlay` — a toast can coexist with (and outlive) an overlay.
    transient_status: Option<SharedString>,
    /// Tabs + n-ary split tree (spec-workspaces-and-splits.md). The focused
    /// window's content is the authoritative live state for the workspace.
    workspace: workspace::Frame<App>,
    /// The top-level **Project** registry (ADR-0028, `docs/components/project.md`).
    /// Owns every project's name + cwd + params; workspaces hold a `ProjectId`
    /// foreign key into this and resolve their cwd here at the point of use.
    /// Built before the workspace at boot (loaded from `projects.json`, else
    /// migrated from existing cwds) so a real `ProjectId` is always available.
    projects: Projects,
    /// Active mouse-driven text selection in the doc view. Spans block/line/char.
    /// `None` when nothing is selected. Cleared on Esc, on a fresh MouseDown
    /// without modifier, and when entering edit mode.
    doc_selection: Option<DocSelection>,
    /// Per-render scratch — populated by `render_doc` as it emits each line's
    /// StyledText, cleared at the top of every doc render. Mouse handlers on
    /// the doc body look up `(block_idx, line_idx)` here to hit-test against
    /// the layout's bounds and to map pixels → char offsets.
    /// Shared via `Rc` so the virtualized doc `gpui::list` render closure (which
    /// must be `'static`) can hold a clone and populate it as it builds visible
    /// lines, while `doc_pos_at` reads the same map between frames.
    line_layouts: Rc<RefCell<HashMap<(usize, usize), TextLayout>>>,
    /// Session server client. When `Some`, agent sessions are created and
    /// managed through the session server (owned subprocesses survive GUI
    /// restarts). Activated by `YALDA_SESSION_SERVER=1`. When `None`, the
    /// GUI spawns `AcpChannelClient` directly (legacy path).
    session_server: Option<SessionServerClient>,
    /// Splash screen shown at startup. `Some(deadline)` while visible;
    /// `None` after dismissal (auto-timeout or keypress).
    splash_until: Option<std::time::Instant>,
    // Layout patterns (spec-layout-patterns.md)
    /// Pending mark chord: `Some('m')` = set mark, `Some('\'')` = jump to mark.
    /// Next keystroke completes the chord.
    pending_mark_chord: Option<char>,
    /// Pending tag chord: `Some('t')` = view tag, `Some('T')` = toggle tag.
    /// Next keystroke is looked up in tag_shortcuts.
    pending_tag_chord: Option<char>,
    /// Shared syntect highlighter for code block syntax coloring in Edit Mode
    /// and the agent transcript tile. Loaded once at startup. `Rc` so each
    /// `TranscriptView` (ticket 021) can hold a cheap clone for its highlight
    /// pass — the `Highlighter` owns a full `SyntaxSet`, far too costly to deep-
    /// clone per render; swapped wholesale (not mutated in place) on theme change.
    syntect_hl: Rc<yalda::highlight::Highlighter>,
    /// The ONE server-notification pump for this view (`start_server_pump`),
    /// singleton like the heartbeat above. It MUST live on the view, never on
    /// an agent slot: the pump owns the `SessionServerClient`'s notification
    /// receiver, so parking it in whichever slot started it meant any flow
    /// that replaced that slot's state (set_screen over a restored ring, slot
    /// churn) cancelled the pump, dropped the receiver, and killed the whole
    /// connection — every later attach failed with "session server
    /// disconnected".
    _server_pump: Option<Task<()>>,
    /// THE owner of all agent-session state (spec-agent-session-ownership.md).
    /// Tiles (`App::Agent(AgentTile)`) hold only a `SessionId` key into this
    /// store; the store enforces strict 1:1 session↔sid and is the single
    /// source of truth for session existence (placement is the tiles).
    sessions: AgentSessions,
    /// Universal agent-session roster (universal-agent-list): a live cache of
    /// EVERY session the server knows about (incl. ones never opened here),
    /// keyed by sid. Seeded by `list_sessions`, kept live by the server's
    /// Created/Closed/Renamed broadcasts. The single source the jump panel + the
    /// per-tile selector both project from. See `agent_roster.rs`.
    agent_roster: AgentRoster,
    /// One [`TranscriptView`] per session (ticket 021): the cached, self-
    /// invalidating transcript widget. Lazily created on the first
    /// `render_agent` of a bound tile (the constructor registers the
    /// `cx.observe(&session)` subscription) and dropped on
    /// `AgentSessions::close`. The 1:1 session↔tile invariant means one view
    /// per session suffices — multi-tile splits need no extra logic.
    transcript_views: HashMap<SessionId, Entity<TranscriptView>>,
    /// Scroll state for the root-level jump panel (jump-panel;
    /// spec-jump-panel.md). The panel itself is rendered inline (it's cheap —
    /// see `render_jump_panel`), so only its scroll position is retained here.
    jump_panel_scroll: ScrollHandle,
    /// Whether the jump panel is shown. Toggled by `cmd-j` / the `?` menu and
    /// persisted (`Preferences::jump_panel_visible`). Defaults to `true`.
    jump_panel_visible: bool,
    /// User's drag-reordered order of jump-panel cwd group headers
    /// (jump-reorder; `Preferences::jump_cwd_order`). Empty = alphabetical.
    /// Rewritten on a cwd-header drop; groups not listed sort after, alpha.
    jump_cwd_order: Vec<String>,
    /// User's drag-reordered order of jump-panel sessions within their cwd group
    /// (jump-reorder; `Preferences::jump_session_order`). Ordered server sids;
    /// empty = by-label. A session never crosses cwd groups (drop is cwd-gated).
    jump_session_order: Vec<String>,
    /// Project names whose jump-panel children are hidden. Names, rather than
    /// runtime-local ProjectIds, make the preference durable across restart.
    jump_folded_projects: std::collections::HashSet<String>,
    /// Jump-panel **order succession**: a placeholder session that is the
    /// continuation of a killed one (today: `/clear`, which closes the server
    /// session and creates a fresh one) maps to its PREDECESSOR's sid, so it
    /// keeps the predecessor's slot in `jump_session_order` — before it binds
    /// (via `AgentRow::order_sid`) and after (the sid is substituted in place in
    /// `jump_session_order` at bind time). Without this a cleared session's new
    /// sid is unranked and drops to the bottom of its cwd group — bug-0007's
    /// recurrence. Entries are consumed at bind and dropped on close.
    jump_order_succession: HashMap<SessionId, String>,
    /// Pinned session recaps (recap-panel), keyed by the session they summarize —
    /// one per session, so a recap is SPECIFIC to its agent tile (UXI-AgentTile-15). An
    /// entry appears when summoned (`recap-session`), is re-runnable and dismissed
    /// (`recap-dismiss`), and owns the throwaway recap worker + pump for the life
    /// of a generation. Rendered inside the agent tile, above the subagents/tasks
    /// panels (`render_agent`), NOT in the global jump panel.
    recaps: HashMap<SessionId, RecapState>,
    /// Sessions this GUI has NO local state for that finished a turn while you
    /// were elsewhere (`bug-0022`), keyed by server sid. Fed by the server's
    /// `SessionBusy` busy→idle broadcast, cleared when you jump to the session.
    /// This is the roster-side twin of `AgentState::unread`: without it, "your
    /// turn" could only ever light up for sessions open in this GUI, which is
    /// what made the jump panel's status marks look arbitrary.
    roster_unread: std::collections::HashSet<String>,
    /// Durable autoname summaries, keyed by SERVER session id (`bug-0020`).
    /// Loaded once at construction from the id-keyed sidecar
    /// (`session_summaries.json`) and written at `finish_autoname`. The live
    /// `AgentState::summary` is authoritative when a session is open here; this
    /// map is what makes the jump panel's italic explainer line survive a GUI
    /// restart, including for sessions no tile is bound to (which
    /// `acp_sessions.json` — cwd-keyed, tile-bound-only — never persisted).
    session_summaries: HashMap<String, String>,
}

impl YaldaGpuiView {
    fn new_doc(
        blocks: Vec<RenderedBlock>,
        theme: Theme,
        file_label: String,
        focus_handle: FocusHandle,
    ) -> Self {
        let syntect_hl =
            Rc::new(yalda::highlight::Highlighter::with_syntect_theme(theme.name.syntect_theme()));
        let label: SharedString = file_label.into();
        let initial =
            App::Buffer(BufferApp::Viewing(DocState::viewing(blocks, label.clone(), None)));
        // ADR-0028: build the Project registry before the workspace so the root
        // workspace has a real `ProjectId` to belong to.
        let cwd = process_cwd();
        let (projects, seed_project) = boot_projects(&cwd, [cwd.clone()]);
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            show_agent_heading_markers: true,
            keymap_registry: KeymapRegistry::load(),
            desktop_grid_cols: DEFAULT_DESKTOP_GRID_COLS,
            desktop_grid_rows: DEFAULT_DESKTOP_GRID_ROWS,
            browser_sort: HashMap::new(),
            desktop_canvas_bounds: std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0, 0.0, 0.0))),
            viewport_height_px: 0.0,
            viewport_width_px: 800.0,
            focus_handle,
            active_overlay: ActiveOverlay::None,
            transient_status: None,
            workspace: workspace::Frame::with_initial(initial, seed_project),
            projects,
            doc_selection: None,
            line_layouts: Rc::new(RefCell::new(HashMap::new())),
            session_server: connect_session_server(),
            splash_until: Some(std::time::Instant::now() + Duration::from_millis(1500)),
            syntect_hl,
            _server_pump: None,
            pending_mark_chord: None,
            pending_tag_chord: None,
            sessions: AgentSessions::new(),
            agent_roster: AgentRoster::default(),
            transcript_views: HashMap::new(),
            jump_panel_scroll: ScrollHandle::new(),
            jump_panel_visible: true,
            jump_cwd_order: Vec::new(),
            jump_session_order: Vec::new(),
            jump_folded_projects: std::collections::HashSet::new(),
            jump_order_succession: HashMap::new(),
            recaps: HashMap::new(),
            roster_unread: std::collections::HashSet::new(),
            // bug-0020: id-keyed autoname summaries, durable across restarts.
            session_summaries: crate::persist::load_session_summaries(),
        }
    }

    fn new_browser(start_dir: PathBuf, theme: Theme, focus_handle: FocusHandle) -> Self {
        let syntect_hl =
            Rc::new(yalda::highlight::Highlighter::with_syntect_theme(theme.name.syntect_theme()));
        let initial = App::Buffer(BufferApp::Picking(BrowserWindow::standalone(start_dir.clone())));
        // ADR-0028: projects before workspace.
        let (projects, seed_project) = boot_projects(&start_dir, [start_dir.clone()]);
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            show_agent_heading_markers: true,
            keymap_registry: KeymapRegistry::load(),
            desktop_grid_cols: DEFAULT_DESKTOP_GRID_COLS,
            desktop_grid_rows: DEFAULT_DESKTOP_GRID_ROWS,
            browser_sort: HashMap::new(),
            desktop_canvas_bounds: std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0, 0.0, 0.0))),
            viewport_height_px: 0.0,
            viewport_width_px: 800.0,
            focus_handle,
            active_overlay: ActiveOverlay::None,
            transient_status: None,
            workspace: workspace::Frame::with_initial(initial, seed_project),
            projects,
            doc_selection: None,
            line_layouts: Rc::new(RefCell::new(HashMap::new())),
            session_server: connect_session_server(),
            splash_until: Some(std::time::Instant::now() + Duration::from_millis(1500)),
            syntect_hl,
            _server_pump: None,
            pending_mark_chord: None,
            pending_tag_chord: None,
            sessions: AgentSessions::new(),
            agent_roster: AgentRoster::default(),
            transcript_views: HashMap::new(),
            jump_panel_scroll: ScrollHandle::new(),
            jump_panel_visible: true,
            jump_cwd_order: Vec::new(),
            jump_session_order: Vec::new(),
            jump_folded_projects: std::collections::HashSet::new(),
            jump_order_succession: HashMap::new(),
            recaps: HashMap::new(),
            roster_unread: std::collections::HashSet::new(),
            // bug-0020: id-keyed autoname summaries, durable across restarts.
            session_summaries: crate::persist::load_session_summaries(),
        }
    }

    /// Replace the focused window's content (old `self.screen = X` writes).
    fn set_screen(&mut self, content: App) {
        self.workspace.replace_focused_content(content);
    }

    /// Persist the current workspace snapshot for the active cwd. Called
    /// after every structural mutation (workspace add/remove, split, close,
    /// focus change, etc.). Best-effort — failures are silent so a
    /// read-only cache_dir or full disk doesn't break the editor.
    pub(crate) fn save_workspace_state(&mut self) {
        // Reap pooled buffers no window references anymore. This is the buffer
        // pool's liveness sweep — called after every structural mutation, so a
        // closed/relocated Edit tile's clean buffer is dropped promptly while
        // dirty ones stay pooled for recovery.
        self.workspace.gc_buffers();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // Resolve each Bound tile's durable server id from the store (single source
        // of truth — ADR-0026). `sid_of` is cx-free, so the layout snapshot needs no
        // cached copy on the tile. Borrows `self.sessions` (disjoint from the
        // `&self.workspace` snapshot borrow).
        let sessions = &self.sessions;
        let resolve = |id| sessions.sid_of(id).cloned();
        save_persisted_workspace(&cwd, &self.workspace, &self.projects, &resolve);
    }

    /// Replace `self.workspace` with one rebuilt from the persisted snapshot
    /// for `cwd`, if any. Doc/Edit windows reload their files; Browser
    /// windows reattach to their saved dir; Claude windows are temporarily
    /// restored as Browser stubs, then replaced with live agent sessions in
    /// a post-pass. Returns `true` if a snapshot was loaded.
    pub(crate) fn restore_workspace_from_disk(&mut self, cx: &mut Context<Self>) -> bool {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let Some(snap) = load_persisted_workspace(&cwd) else {
            return false;
        };
        // Rebuild onto the already-migrated project registry; each restored
        // workspace re-points at the project rooting its persisted cwd
        // (ADR-0028 §7 self-healing load). `default_project` is the first project
        // (migration guarantees ≥1 exists).
        let default_project = self.projects.first().unwrap_or(ProjectId(0));
        let mut ws: workspace::Frame<App> = workspace::Frame::new(default_project);
        // Each agent leaf carries its persisted session id (identity), so restore
        // rebinds it to ITS OWN session (UXI-AgentTile-18), not by index.
        let mut agent_leaf_ids: Vec<(workspace::WindowId, Option<ServerSid>)> = Vec::new();
        for pws in snap.workspaces {
            let (layout, max_id, agents) = restore_layout(&mut ws, &self.theme, pws.layout);
            ws.next_window_id = ws.next_window_id.max(max_id + 1);
            agent_leaf_ids.extend(agents);
            // Working directory → project: resolve the persisted cwd (typed field
            // if present, else the legacy `kv["cwd"]`, else the process dir) to a
            // project in the registry, creating one if the migration hasn't
            // (ADR-0028 §7 self-heal — the cwd the record carries is always
            // enough to recover membership).
            let workspace_cwd = pws
                .cwd
                .map(PathBuf::from)
                .or_else(|| pws.legacy_kv.get("cwd").map(PathBuf::from))
                .unwrap_or_else(process_cwd);
            let project = self
                .projects
                .ensure_at_cwd(workspace_cwd.clone(), &project_name_for_cwd(&workspace_cwd));
            let mut wsp = workspace::Workspace::with_layout(
                pws.auto_name,
                layout,
                pws.focused_window,
                project,
            );
            wsp.display_name = pws.display_name;
            wsp.rail = pws.rail.map(|r| restore_rail(r, pws.focused_window));
            // IGNORE the persisted layout mode (spec-infinite-plane-workspace.md
            // Behavior 7): every workspace is a Plane now. `pws.layout_mode`
            // deserializes any old mode string to `Plane` already; force it here
            // so the intent is explicit and a retired-mode snapshot can never
            // resurrect a mode. Content (tree leaves) is preserved; geometry
            // reflows once via the first render's seed/reconcile below.
            wsp.layout_mode = workspace::LayoutMode::Plane;
            wsp.master_ratio = pws.master_ratio;
            wsp.master_count = pws.master_count;
            wsp.tag_view = pws.tag_view;
            wsp.desktop = workspace::DesktopState {
                // Restored leaves keep their persisted WindowIds, so the
                // id-keyed slots round-trip with no mapping. Stale ids (or an
                // absent field) are handled by the first desktop render's
                // reconcile/seed (spec Behavior 7).
                slots: {
                    let mut v: Vec<(workspace::WindowId, workspace::Slot)> = pws
                        .desktop_slots
                        .into_iter()
                        // Slots are signed on the plane (D4). Old snapshots
                        // stored non-negative values, which deserialize as the
                        // same positive `i32` (the old top-right quadrant).
                        .map(|(id, row, col)| (id, workspace::Slot::new(row, col)))
                        .collect();
                    v.sort_by_key(|&(_, s)| s);
                    v
                },
                spans: pws
                    .desktop_spans
                    .into_iter()
                    .map(|(id, rows, cols)| (id, workspace::Span::new(rows, cols)))
                    .collect(),
                // Restore the plane's saved camera (D4 / Behavior 7); an absent
                // field (old snapshot) falls back to the origin at Full.
                camera: pws
                    .camera
                    .map(|c| workspace::Camera {
                        pan: c.pan,
                        zoom: c.zoom,
                    })
                    .unwrap_or_default(),
                drag: None,
                resize: None,
                pan_drag: None,
                last_reveal: None,
            };
            ws.workspaces.push(wsp);
            ws.next_workspace_index += 1;
        }
        if !ws.workspaces.is_empty() {
            ws.active_workspace = snap.active_workspace.min(ws.workspaces.len() - 1);
        }
        if ws.workspaces.is_empty() {
            return false;
        }
        // Restore marks — load from snapshot, then GC stale window ids.
        for (ch, wid) in snap.marks {
            ws.marks.set(ch, wid);
        }
        let live_ids = ws.all_window_ids();
        ws.marks.gc(&live_ids);
        ws.tag_shortcuts = snap.tag_shortcuts;
        // Restore buffer tags: match by canonical path.
        for (path_str, tags) in &snap.buffer_tags {
            let path = PathBuf::from(path_str);
            if let Some(&buf_id) = ws.path_index.get(&path)
                && let Some(buf) = ws.file_buffers.get_mut(&buf_id)
            {
                buf.tags = tags.iter().cloned().collect();
            }
        }
        // No retile-on-restore (infinite-plane, Stage D): every workspace is a Plane
        // (Behavior 1/7). The `Layout<C>` tree restored above is the CONTENT
        // owner verbatim; geometry comes from the restored `desktop` slot map (a
        // workspace with `desktop_slots` round-trips its arrangement) or, for a
        // retired-mode snapshot that has none, from the first desktop render's
        // seed/reconcile — which bulk-seeds every tree leaf onto the plane by
        // origin ring-spiral (chrome.rs `render_desktop` → `reconcile`). Content
        // is preserved; only geometry reflows once.
        self.workspace = ws;

        // Post-pass: replace Browser stubs with live agent sessions.
        if !agent_leaf_ids.is_empty() {
            self.restore_agent_leaves(&agent_leaf_ids, cx);
        }
        true
    }

    /// Replace Browser stubs at the given leaf IDs with live agent sessions.
    /// Called as a post-pass after `restore_workspace_from_disk` installs the
    /// layout — by that point `self.workspace` is populated and we have `cx`.
    fn restore_agent_leaves(
        &mut self,
        leaves: &[(workspace::WindowId, Option<ServerSid>)],
        cx: &mut Context<Self>,
    ) {
        let proc_cwd = process_cwd();
        let persisted = load_persisted_acp_sessions(&proc_cwd);

        if self.session_server.is_some() {
            self.start_server_pump(cx);
            // Identity, not index: each leaf rebinds to ITS OWN persisted session
            // (UXI-AgentTile-18). Details (mode/draft/cwd) come from the id-keyed
            // side-channel; the leaf's own id is authoritative for the binding.
            // Bind up front + attach the bound sids once, together (no per-leaf
            // re-list, which would race every tile onto the first sid).
            // `attach_sids` leaves to the server's attach (wire); it stays a
            // `Vec<String>`, filled from `ServerSid` via `.to_string()`.
            let mut attach_sids: Vec<String> = Vec::new();
            // bug-0005: no two restored sessions in one cwd may share a label. The
            // loaded slots are already deduped, but a by-id-MISS fabricated slot (or
            // a leaf pulled positionally) can still collide, so uniquify each label
            // against a running set as the leaves bind.
            let mut used_labels: std::collections::HashSet<String> = std::collections::HashSet::new();
            let by_id: std::collections::HashMap<ServerSid, PersistedSlot> =
                persisted.iter().cloned().map(|s| (s.id.clone(), s)).collect();
            // Positional fallback ONLY for an old (pre-identity) workspace.json
            // where every leaf's persisted id is None; a fresh save writes ids.
            let any_identity = leaves
                .iter()
                .any(|(_, sid)| sid.as_ref().is_some_and(|s| !s.as_str().is_empty()));
            eprintln!(
                "[yalda-gpui] restore(server): {} agent leaves, {} persisted sessions, any_identity={}; leaf ids: {:?}",
                leaves.len(),
                persisted.len(),
                any_identity,
                leaves
                    .iter()
                    .map(|(w, sid)| {
                        (w, sid.as_ref().map(|s| s.as_str()[..s.as_str().len().min(8)].to_string()))
                    })
                    .collect::<Vec<_>>(),
            );
            for (i, (leaf_id, persisted_sid)) in leaves.iter().enumerate() {
                self.install_agent_tile(*leaf_id, AgentTile::new());
                self.focus_window_for_restore(*leaf_id);

                let slot: Option<PersistedSlot> =
                    match persisted_sid.as_ref().filter(|s| !s.as_str().is_empty()) {
                        Some(s) => Some(by_id.get(s).cloned().unwrap_or_else(|| {
                            // Layout knows the id but the details side-channel doesn't
                            // (e.g. cwd changed) — still bind the id, with defaults.
                            PersistedSlot {
                                id: s.clone(),
                                // Empty → the dedupe below assigns a unique claude-N
                                // instead of a bare "claude" (bug-0005).
                                label: String::new(),
                                active: false,
                                mode: InputModeKind::Worksheet,
                                tasklist_open: false,
                                subagents_open: false,
                                sidepanel_hidden: false,
                                cwd: None,
                                compose_draft: None,
                                summary: None,
                            }
                        })),
                        _ if any_identity => None,
                        _ => persisted.get(i).cloned(),
                    };

                match slot {
                    Some(slot) => {
                        // Bind this leaf to its OWN persisted sid via the store's
                        // idempotent choke. `Created` ⇒ this leaf owns the sid.
                        // `AlreadyOpen` ⇒ a DUPLICATE sid across persisted leaves;
                        // strict 1:1 forbids binding a second tile to it (that is
                        // exactly how the same session showed up in two
                        // workspaces), so this leaf falls to the free selector.
                        let slot_cwd = slot.cwd.clone().unwrap_or_else(|| proc_cwd.clone());
                        // bug-0005: uniquify against the labels already bound this
                        // restore so two leaves never end up both "claude".
                        let label = crate::persist::unique_label(&slot.label, &used_labels);
                        used_labels.insert(label.clone());
                        let make_cwd = slot_cwd.clone();
                        // UXI-AgentTile-27: carry the persisted autoname summary
                        // across the restart. The session is NOT re-armed for
                        // autonaming (property 1 — a restored session already has
                        // history and is never retro-named), so this is the only
                        // way its jump-panel summary line survives.
                        // bug-0020: prefer the slot's copy, fall back to the
                        // id-keyed sidecar (the durable home — `acp_sessions.json`
                        // only ever held tile-bound sessions, and old files have
                        // no `summary` key at all).
                        let slot_summary = slot.summary.clone().or_else(|| {
                            self.session_summaries
                                .get(slot.id.as_str())
                                .filter(|s| !s.trim().is_empty())
                                .cloned()
                        });
                        let provider = self
                            .agent_roster
                            .get(slot.id.as_str())
                            .map(|info| info.provider)
                            .unwrap_or_default();
                        let bind = self.sessions.open_or_focus(&slot.id, |_id| {
                            cx.new(|_| {
                                let mut state = AgentState::new_server_managed_for(
                                    provider,
                                    Some("reconnecting…".into()),
                                );
                                state.summary = slot_summary;
                                AgentSession {
                                    state,
                                    label,
                                    cwd: make_cwd,
                                    resume_id: Some(slot.id.clone()),
                                }
                            })
                        });
                        match bind {
                            agent_sessions::Bind::Created(sid_id) => {
                                self.with_session(sid_id, cx, |state| {
                                    // Model C (design-c.md §4.4): restore the
                                    // persisted compose draft + its placement into
                                    // the separate Compose buffer (the transcript is
                                    // rebuilt by replay, untouched here).
                                    state.input_surface = InputSurface::with_draft(
                                        slot.mode,
                                        slot.compose_draft.as_deref().unwrap_or(""),
                                    );
                                    state.tasklist_open = slot.tasklist_open;
                                    state.subagents_open = slot.subagents_open;
                                    state.sidepanel_hidden = slot.sidepanel_hidden;
                                });
                                if let Some(tile) = self.agent_tile_mut() {
                                    // `open_or_focus` bound `slot.id` in the store, so
                                    // the next save resolves it via `sid_of` — no cache.
                                    tile.bind(sid_id);
                                }
                                // bug-0021: a restored session that never got a
                                // name (still `claude-N`, one-shot unspent) arms
                                // here; replay's end is what makes it nameable.
                                self.maybe_arm_autoname(sid_id, cx);
                                // Wire boundary: the bound sid leaves to attach.
                                attach_sids.push(slot.id.to_string());
                                eprintln!(
                                    "[yalda-gpui] restore leaf {leaf_id}: BOUND+resume {}",
                                    &slot.id.as_str()[..slot.id.as_str().len().min(8)]
                                );
                            }
                            agent_sessions::Bind::AlreadyOpen(_) => {
                                if let Some(tile) = self.agent_tile_mut() {
                                    tile.show_picker();
                                }
                                // Selector projects from the roster; seed it.
                                self.refresh_roster(cx);
                                eprintln!(
                                    "[yalda-gpui] restore leaf {leaf_id}: PICKER (sid {} already open — duplicate)",
                                    &slot.id.as_str()[..slot.id.as_str().len().min(8)]
                                );
                            }
                        }
                    }
                    None => {
                        // More agent leaves than persisted sessions: open this
                        // one straight into the free-session selector (it
                        // projects from the universal roster — seed it).
                        if let Some(tile) = self.agent_tile_mut() {
                            tile.show_picker();
                        }
                        self.refresh_roster(cx);
                        eprintln!(
                            "[yalda-gpui] restore leaf {leaf_id}: PICKER (no persisted id for this leaf)"
                        );
                    }
                }
            }
            if !attach_sids.is_empty() {
                // resuming = true: a gone remembered session → unavailable notice.
                self.spawn_attach_sessions(attach_sids, true, cx);
            }
        } else {
            // Legacy direct-spawn path (no session server). One tile shows one
            // session; still positional here — identity restore is the
            // server-managed path above. Fresh claude-N past the persisted list.
            let mut used_labels: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (i, (leaf_id, _persisted_sid)) in leaves.iter().enumerate() {
                self.install_agent_tile(*leaf_id, AgentTile::new());
                self.focus_window_for_restore(*leaf_id);
                let id = match persisted.get(i).cloned() {
                    None => {
                        let state = self
                            .create_agent_session(None, proc_cwd.clone(), cx)
                            .armed_for_autoname();
                        // bug-0005: unique label, never a bare/duplicate "claude".
                        let label = crate::persist::unique_label("", &used_labels);
                        used_labels.insert(label.clone());
                        self.show_local_session(
                            AgentSession {
                                state,
                                label,
                                cwd: proc_cwd.clone(),
                                resume_id: None,
                            },
                            cx,
                        )
                    }
                    Some(slot) => {
                        let slot_cwd = slot.cwd.clone().unwrap_or_else(|| proc_cwd.clone());
                        let mut state =
                            self.create_agent_session(Some(slot.id.clone()), slot_cwd.clone(), cx);
                        state.input_surface = InputSurface::with_draft(
                            slot.mode,
                            slot.compose_draft.as_deref().unwrap_or(""),
                        );
                        state.tasklist_open = slot.tasklist_open;
                        state.subagents_open = slot.subagents_open;
                        state.sidepanel_hidden = slot.sidepanel_hidden;
                        // bug-0005: uniquify against labels already bound this restore.
                        let label = crate::persist::unique_label(&slot.label, &used_labels);
                        used_labels.insert(label.clone());
                        self.show_local_session(
                            AgentSession {
                                state,
                                label,
                                cwd: slot_cwd,
                                resume_id: Some(slot.id),
                            },
                            cx,
                        )
                    }
                };
                self.start_session_pump(id, cx);
            }
        }
        cx.notify();
    }

    /// Replace the content at `leaf_id` (any workspace) with `tile`.
    fn install_agent_tile(&mut self, leaf_id: workspace::WindowId, tile: AgentTile) {
        for wsp in &mut self.workspace.workspaces {
            if let Some(win) = wsp.layout.find_leaf_mut(leaf_id) {
                win.content = App::Agent(tile);
                return;
            }
        }
    }

    /// Point the workspace focus at `leaf_id` so the bind-choke methods (which
    /// act on the FOCUSED tile) target the leaf being restored.
    fn focus_window_for_restore(&mut self, leaf_id: workspace::WindowId) {
        for (i, wsp) in self.workspace.workspaces.iter_mut().enumerate() {
            if wsp.layout.find_leaf(leaf_id).is_some() {
                wsp.focused = leaf_id;
                self.workspace.active_workspace = i;
                return;
            }
        }
    }

    /// `Some(doc)` if currently viewing a document, else `None`.
    fn doc_mut(&mut self) -> Option<&mut DocState> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Buffer(BufferApp::Viewing(d)) => Some(d),
            _ => None,
        }
    }

    fn browser_mut(&mut self) -> Option<&mut BrowserWindow> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Buffer(BufferApp::Picking(b)) => Some(b),
            _ => None,
        }
    }

    /// Run `f` against the `AgentState` of session `id`, inside the session
    /// entity's `update` (spec-agent-session-ownership.md). The session entity
    /// is notified at the mutation site (timing-correct, `project.md` fact 4);
    /// the root is also notified to preserve today's whole-app invalidation
    /// (load-bearing only after the per-session observation lands in 021).
    /// Returns `None` if no session `id` exists.
    pub(crate) fn with_session<R>(
        &mut self,
        id: SessionId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut AgentState) -> R,
    ) -> Option<R> {
        let ent = self.sessions.get(id)?.clone();
        let r = ent.update(cx, |s, scx| {
            let r = f(&mut s.state);
            scx.notify();
            r
        });
        cx.notify();
        Some(r)
    }

    /// Read `f` against the `AgentState` of session `id` via the entity's
    /// `read` (spec-agent-session-ownership.md). `None` if no such session.
    pub(crate) fn read_session<R>(
        &self,
        id: SessionId,
        cx: &GpuiApp,
        f: impl FnOnce(&AgentState) -> R,
    ) -> Option<R> {
        let ent = self.sessions.get(id)?;
        Some(f(&ent.read(cx).state))
    }

    /// The session entity bound to `id`, cloned (cheap handle clone). `None`
    /// if no session `id` exists. Callers that need both `self` and the entity
    /// simultaneously clone the handle first to sidestep the borrow overlap.
    pub(crate) fn session_entity(&self, id: SessionId) -> Option<Entity<AgentSession>> {
        self.sessions.get(id).cloned()
    }

    /// The [`TranscriptView`] for session `id`, created lazily on the first
    /// `render_agent` of a bound tile (ticket 021). The constructor registers
    /// the `cx.observe(&session)` subscription; the view is dropped on
    /// `AgentSessions::close` (each close site `remove`s it). The 1:1
    /// session↔tile invariant means one view per session is exactly right —
    /// a re-bound tile reuses the same view, multi-tile splits need no extra
    /// logic. `session_ent` is the already-cloned handle the caller holds.
    pub(crate) fn transcript_view_for(
        &mut self,
        id: SessionId,
        session_ent: Entity<AgentSession>,
        cx: &mut Context<Self>,
    ) -> Entity<TranscriptView> {
        if let Some(v) = self.transcript_views.get(&id) {
            return v.clone();
        }
        let weak = cx.entity().downgrade();
        let view = cx.new(|vcx| TranscriptView::new(session_ent, weak, vcx));
        self.transcript_views.insert(id, view.clone());
        // A brand-new TranscriptView often REUSES the GPUI entity slot a just-dropped
        // one freed (e.g. `/clear` removes the old session's view, then the rebind
        // creates this one). GPUI keys a cached `AnyView`'s prepaint by tree POSITION
        // and dirties it only via `mark_view_dirty`, which walks the COMMITTED frame's
        // dispatch tree (`window.rs`). Embedded at the same position, the fresh view
        // inherits the dropped one's stale prepaint AND — never having been painted
        // into the committed dispatch tree — its self-notifies are dropped by
        // `mark_view_dirty` (no `view_path`), so it FREEZES: typed text never repaints
        // until an unrelated event forces a full refresh (a mouse click). This is the
        // 7×-recurring "/clear worksheet invisible until I click" bug. Force ONE full
        // window refresh (deferred, outside this render pass) so the new view is
        // painted fresh into the dispatch tree — after which its observe-notifies land.
        // Binds are rare, so the refresh cost is irrelevant. Pinned by
        // `real_clear_server_branch_then_type_paints`.
        cx.defer(|app| app.refresh_windows());
        view
    }

    /// Mutable `AgentState` for the focused tile's bound session, leased from
    /// the session entity (spec-agent-session-ownership.md). Reads the tile's
    /// `bound` (a `Copy` SessionId) FIRST so the workspace borrow ends, then
    /// routes through the store. The returned guard derefs to `&mut AgentState`;
    /// dropping it ends the lease. Notifies neither the session nor the root —
    /// callers that mutate through it `cx.notify()` themselves, exactly as the
    /// old `&mut AgentState` accessor required.
    fn agent_mut<'a>(
        &mut self,
        cx: &'a mut Context<Self>,
    ) -> Option<gpui::GpuiBorrow<'a, AgentSession>> {
        let id = self.focused_bound_session()?;
        let ent = self.sessions.get(id)?.clone();
        Some(ent.as_mut(cx))
    }

    /// Read-only view of the focused tile's bound `AgentState` (no notify, no
    /// lease). The read-side counterpart of [`agent_mut`] — use it for pure
    /// queries so a mutable lease's drop-notify can't spuriously invalidate.
    fn agent_read<R>(&self, cx: &GpuiApp, f: impl FnOnce(&AgentState) -> R) -> Option<R> {
        let id = self.focused_bound_session()?;
        self.read_session(id, cx, f)
    }

    /// Like [`with_session`] but does NOT notify — the caller decides whether
    /// the mutation warrants invalidation (e.g. key dispatch where a `Skipped`
    /// outcome must not trigger a render). `None` if no session `id` exists.
    pub(crate) fn with_session_silent<R>(
        &mut self,
        id: SessionId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut AgentState) -> R,
    ) -> Option<R> {
        let ent = self.sessions.get(id)?.clone();
        Some(ent.update(cx, |s, _scx| f(&mut s.state)))
    }

    /// The `SessionId` bound to the focused Agent tile, if any. `Copy`, so the
    /// caller can drop the workspace borrow before touching `self.sessions`.
    fn focused_bound_session(&self) -> Option<SessionId> {
        match self.workspace.focused_content().expect("no focused window") {
            App::Agent(tile) => tile.session(),
            _ => None,
        }
    }

    /// Return the focused session's server session id (cloned), or `None`.
    fn active_server_session_id(&self) -> Option<String> {
        let id = self.focused_bound_session()?;
        self.sessions.sid_of(id).map(|s| s.to_string())
    }

    fn agent_tile(&self) -> Option<&AgentTile> {
        match self.workspace.focused_content().expect("no focused window") {
            App::Agent(tile) => Some(tile),
            _ => None,
        }
    }

    fn agent_tile_mut(&mut self) -> Option<&mut AgentTile> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Agent(tile) => Some(tile),
            _ => None,
        }
    }

    /// THE single bind choke (spec-agent-session-ownership.md). Show the
    /// session for `sid` in the focused tile, creating it via `make` if no
    /// session already carries that sid. If a session already exists for the
    /// sid it is FOCUSED (its `bound` reused), never bound twice (INV-1/INV-2).
    /// Returns the `SessionId` now bound to the focused tile.
    ///
    /// This is the choke for paths that know the sid UP FRONT. Paths whose sid
    /// resolves later (the bind-before-attach flow that the replay routing
    /// requires) instead go through [`show_local_session`] + [`bind_session_sid`]
    /// (the store's `bind_sid`), which enforces the same 1:1 invariants.
    #[allow(dead_code)]
    fn show_session(
        &mut self,
        sid: &str,
        label: String,
        cwd: PathBuf,
        resume_id: Option<String>,
        make_state: impl FnOnce() -> AgentState,
        cx: &mut Context<Self>,
    ) -> SessionId {
        // The payload entity is built lazily only when a NEW session is minted
        // (so the entity is created exactly once per sid, never on the focus
        // path where `open_or_focus` returns the existing one).
        // Wire boundary: `sid` / `resume_id` arrive as raw strings; type them.
        let bind = self.sessions.open_or_focus(&ServerSid::new(sid), |_id| {
            let session = AgentSession {
                state: make_state(),
                label,
                cwd,
                resume_id: resume_id.map(ServerSid::new),
            };
            cx.new(|_| session)
        });
        let id = bind.id();
        if let Some(tile) = self.agent_tile_mut() {
            tile.bind(id);
            tile.set_pending(None);
        }
        id
    }

    /// Bind a fresh LOCAL (pre-attach) session to the focused tile and return
    /// its id. Used by the placeholder / direct-spawn paths that don't yet have
    /// a server sid; `bind_sid_to` later attaches the sid once it resolves.
    fn show_local_session(&mut self, session: AgentSession, cx: &mut Context<Self>) -> SessionId {
        let ent = cx.new(|_| session);
        let id = self.sessions.create_local(|_id| ent);
        if let Some(tile) = self.agent_tile_mut() {
            tile.bind(id);
        }
        crate::clear_log(&format!("show_local_session: new_id={id:?} bound to tile"));
        id
    }

    /// Open `path` as a doc. If it's already in a wsp, switch to that workspace.
    /// Otherwise push a new workspace containing the doc. Returns false on read error.
    /// Build a Doc `App` for `path`, bound to the shared buffer
    /// pool (5c: dedup by canonical path so Edit views of the same file
    /// share the exact same rope + undo and edits show live in this Doc).
    /// `None` if the file can't be read.
    fn make_doc_content(&mut self, path: &std::path::Path) -> Option<App> {
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string();
        let (buf_id, core) = match self.workspace.open_and_retain(path) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path.display(), e);
                return None;
            }
        };
        let blocks = render_with_wiki(
            &core.borrow().document().full_text(),
            &self.theme,
            Some(path),
        );
        Some(App::Buffer(BufferApp::Viewing(DocState::viewing(
            blocks,
            canon.into(),
            Some(DocSource::new(buf_id, core)),
        ))))
    }

    fn open_file(&mut self, path: PathBuf) -> bool {
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();

        // Already open? Switch to that workspace.
        if let Some(idx) = self.find_workspace_by_doc_label(&canon) {
            if idx != self.workspace.active_workspace {
                self.workspace.set_active_workspace(idx);
            }
            return true;
        }

        let Some(new_content) = self.make_doc_content(&path) else {
            return false;
        };

        // If the current workspace is a transient Browser, replace its content
        // (matches today's "browser disappears when you pick a file"). For
        // Doc/Edit/Claude, push a new workspace so the existing work isn't lost.
        let replace_in_place = matches!(
            self.workspace.focused_content(),
            Some(App::Buffer(BufferApp::Picking(_)))
        );
        if replace_in_place {
            self.set_screen(new_content);
        } else {
            let project = self.workspace.inherited_project();
            self.workspace.push_initial_workspace(new_content, project);
        }
        self.save_workspace_state();
        true
    }

    /// Find a workspace whose focused content is a Doc/Edit with the given file
    /// label. Returns the workspace index, or None.
    fn find_workspace_by_doc_label(&self, label: &str) -> Option<usize> {
        for (i, wsp) in self.workspace.workspaces.iter().enumerate() {
            if let workspace::Layout::Leaf(w) = &wsp.layout {
                match &w.content {
                    App::Buffer(BufferApp::Viewing(d)) if d.file_label.as_ref() == label => {
                        return Some(i);
                    }
                    App::Buffer(BufferApp::Editing(e)) if e.file_label.as_ref() == label => {
                        return Some(i);
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Switch the workspace to the workspace at `idx`. Used by the buffer-list
    /// picker. No-op if idx is out of range.
    fn switch_to_buffer(&mut self, idx: usize) {
        if idx >= self.workspace.workspaces.len() || idx == self.workspace.active_workspace {
            return;
        }
        self.workspace.set_active_workspace(idx);
    }

    /// Close the workspace at `idx`. Returns false if the workspace's content has unsaved
    /// modifications (refusing to close). If it's the last wsp, quits.
    fn close_buffer_at(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        if idx >= self.workspace.workspaces.len() {
            return true;
        }
        // Check if the workspace's focused content is modified.
        let is_modified = match &self.workspace.workspaces[idx].layout {
            workspace::Layout::Leaf(w) => screen_is_modified(&w.content),
            _ => false,
        };
        if is_modified {
            return false;
        }
        if self.workspace.workspaces.len() <= 1 {
            cx.quit();
            return true;
        }
        self.workspace.close_workspace(idx);
        true
    }

    // ---- Document actions ---------------------------------------------------

    fn scroll_down(&mut self, _: &ScrollDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut()
            && d.cursor_block + 1 < d.blocks.len()
        {
            d.cursor_block += 1;
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn scroll_up(&mut self, _: &ScrollUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut()
            && d.cursor_block > 0
        {
            d.cursor_block -= 1;
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn page_down(&mut self, _: &ScrollPageDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = (d.cursor_block + 8).min(d.blocks.len().saturating_sub(1));
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn page_up(&mut self, _: &ScrollPageUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = d.cursor_block.saturating_sub(8);
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn cursor_next(&mut self, _: &CursorNextBlock, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut()
            && d.cursor_block + 1 < d.blocks.len()
        {
            d.cursor_block += 1;
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn cursor_prev(&mut self, _: &CursorPrevBlock, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut()
            && d.cursor_block > 0
        {
            d.cursor_block -= 1;
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    fn cursor_top(&mut self, _: &CursorTop, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = 0;
            d.reveal_block(0);
            cx.notify();
        }
    }
    fn cursor_bottom(&mut self, _: &CursorBottom, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut()
            && !d.blocks.is_empty()
        {
            d.cursor_block = d.blocks.len() - 1;
            d.reveal_block(d.cursor_block);
            cx.notify();
        }
    }
    /// Move the doc cursor to the next block (wrapping past EOF) matching
    /// `pred`. Local-menu `navigate`/`goto` commands (spec-menu-scopes.md).
    fn doc_jump_next_matching(
        &mut self,
        label: &str,
        pred: fn(&RenderedBlock) -> bool,
        cx: &mut Context<Self>,
    ) {
        let target = match self.doc_mut() {
            Some(d) if !d.blocks.is_empty() => {
                let n = d.blocks.len();
                let start = d.cursor_block.min(n - 1);
                (1..=n)
                    .map(|off| (start + off) % n)
                    .find(|&i| pred(&d.blocks[i]))
            }
            _ => return,
        };
        match target {
            Some(idx) => {
                if let Some(d) = self.doc_mut() {
                    d.cursor_block = idx;
                    d.reveal_block(idx);
                }
            }
            None => {
                self.transient_status = Some(format!("no {label} in document").into());
            }
        }
        cx.notify();
    }

    fn open_browser(&mut self, _: &OpenBrowser, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_browser_inner(cx);
    }

    /// B2: Cmd+O is Buffer-app-scoped. On an `Agent` tile it is inert — a
    /// transient status hint, no tile mutation (browser-over-Agent is removed).
    /// On a `Buffer` tile it transitions the focused `BufferApp` to `Picking`,
    /// stashing the prior mode (`Viewing`/`Editing`) on `BrowserWindow.underlying`
    /// so Esc/q restores it (B4). Already-`Picking` is a no-op.
    /// The directory a freshly-opened file browser should land in.
    ///
    /// Continuity rule (untitled.md): when the picker opens over a Buffer that
    /// is viewing/editing a real file, land in that file's parent dir so the
    /// browser sits where the file lives. Otherwise fall back to the workspace
    /// base — the registry `"cwd"` if set, else Yalda's process dir.
    fn browser_start_dir(&self) -> PathBuf {
        if let Some(label) = self.workspace.focused_content().and_then(screen_file_label) {
            let parent = PathBuf::from(label.as_ref())
                .parent()
                .map(std::path::Path::to_path_buf);
            if let Some(parent) = parent.filter(|p| p.is_dir()) {
                return parent;
            }
        }
        self.active_workspace_cwd().unwrap_or_else(process_cwd)
    }

    fn open_browser_inner(&mut self, cx: &mut Context<Self>) {
        match self.workspace.focused_content().expect("no focused window") {
            // Already picking — nothing to do.
            App::Buffer(BufferApp::Picking(_)) => return,
            // Agent/Linear/Keymap tile: out of scope. No buffer here to pick into.
            App::Agent(_) | App::Linear(_) | App::Keymap(_) => {
                self.transient_status = Some("no buffer here".into());
                cx.notify();
                return;
            }
            App::Buffer(_) => {}
        }
        // Where the picker lands: parent dir of the file we're leaving for
        // continuity, else the workspace cwd, else the process dir. Computed
        // BEFORE `replace_focused_content` moves the prior buffer out.
        let dir = self.browser_start_dir();
        // Transition the focused Buffer to Picking IN PLACE, stashing the prior
        // mode on `BrowserWindow.underlying` so Esc/q restores it (B4). Picking
        // a file discards the underlying and replaces the picker with the picked
        // file in this same tile (see `open_file`'s `replace_in_place` branch).
        // This keeps the picker tile-scoped instead of workspace-scoped so
        // splits/workspaces aren't disrupted by file picking.
        let placeholder = App::Buffer(BufferApp::Picking(BrowserWindow::standalone(dir.clone())));
        let prior = self
            .workspace
            .replace_focused_content(placeholder)
            .expect("workspace has no focused window");
        // Seed the picker from this tile's remembered sort order (set last time
        // its explorer was open). Absent = the FileBrowser default (Name).
        let mut fb = FileBrowser::new(dir);
        if let Some(&order) = self
            .workspace
            .focused_window_id()
            .and_then(|id| self.browser_sort.get(&id))
        {
            fb.set_sort_order(order);
        }
        // Narrow the prior App to its BufferApp mode. The match above
        // guarantees `prior` is a Buffer (Viewing/Editing), so the stash is
        // typed `BufferApp` (D3/C4) and an Agent can never end up behind a
        // picker.
        self.set_screen(App::Buffer(BufferApp::Picking(BrowserWindow {
            fb,
            underlying: prior.into_buffer_stash(),
            scroll: ScrollHandle::new(),
        })));
        self.save_workspace_state();
        cx.notify();
    }
    fn quit(&mut self, _: &Quit, _w: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    /// Re-exec the current binary with the same arguments, then quit.
    /// Useful for picking up a freshly compiled build without leaving
    /// the window manager flow.
    fn restart(&mut self, _: &Restart, _w: &mut Window, cx: &mut Context<Self>) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        for arg in std::env::args().skip(1) {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(_) => cx.quit(),
            Err(e) => {
                if let Some(mut c) = self.agent_mut(cx) {
                    c.status = Some(format!("restart failed: {e}").into());
                }
            }
        }
    }

    /// Set the focused agent slot's status line (build-loop feedback). No-op
    /// if the focused window isn't an agent screen, but always logs so the
    /// message isn't lost when triggered from a doc/edit view.
    fn set_agent_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        eprintln!("[yalda-gpui] {msg}");
        if let Some(mut c) = self.agent_mut(cx) {
            c.status = Some(msg.to_string().into());
        }
        cx.notify();
    }

    /// Dev hot-restart: rebuild `yalda-gpui` and replace THIS instance with a
    /// fresh one. A full self-restart — build, spawn a fresh instance, then
    /// quit so the new process re-attaches to every server-managed session. The
    /// session server (and its live agent sessions) is left running, so agents
    /// survive the bounce.
    ///
    /// NEEDS-RUNTIME: GPUI can't be driven headlessly, so this is compile-
    /// verified only — the actual rebuild/relaunch/re-attach must be checked by
    /// a human.
    /// `dev-restart-gui` — rebuild + relaunch just the GUI, mirroring
    /// `./dev-gui.sh`: build RELEASE (a debug
    /// GPUI build stutters on text input), leave the running session server
    /// untouched, and relaunch the freshly-built release binary. The new GUI
    /// reconnects to the existing server and re-attaches its sessions, so live
    /// agents survive the bounce.
    fn dev_rebuild_restart_gui(&mut self, cx: &mut Context<Self>) {
        self.dev_rebuild_restart(false, cx);
    }

    /// `dev-restart-all` — rebuild + restart BOTH the GUI and the session
    /// server, mirroring `./dev-server.sh` + `./dev-gui.sh`: build RELEASE for
    /// both bins, kill the
    /// running server + clear its stale socket/pid so the relaunched GUI spawns
    /// the freshly-built server, then relaunch. Live sessions reconnect from the
    /// WAL on the new server's startup (ADR-0009/-0018); only an in-flight,
    /// un-persisted turn is at risk.
    fn dev_rebuild_restart_all(&mut self, cx: &mut Context<Self>) {
        self.dev_rebuild_restart(true, cx);
    }

    /// Shared rebuild-and-relaunch loop behind `dev-restart-gui` /
    /// `dev-restart-all`. Always RELEASE and always relaunches the freshly-built
    /// `target/release/yalda-gpui` (NOT `current_exe`, which may be a debug
    /// build), so this matches the `dev-*.sh` scripts regardless of how the
    /// running process was started. With `restart_server`, it also rebuilds
    /// `yalda-session-server` and tears the old one down.
    fn dev_rebuild_restart(&mut self, restart_server: bool, cx: &mut Context<Self>) {
        let what = if restart_server { "gui + server" } else { "gui" };
        self.set_agent_status(&format!("rebuilding {what} (release): cargo build…"), cx);

        let manifest_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let gui_bin = PathBuf::from(&manifest_dir).join("target/release/yalda-gpui");
        let args: Vec<String> = std::env::args().skip(1).collect();

        cx.spawn(async move |this, cx| {
            // Run the (slow, blocking) build on a background thread, then — for
            // the "all" path — tear the old server down so the relaunched GUI
            // brings up the newly-built one (mirrors `dev-server.sh`).
            let built = cx
                .background_executor()
                .spawn(async move {
                    let mut build_args: Vec<&str> =
                        vec!["build", "--release", "--bin", "yalda-gpui"];
                    if restart_server {
                        build_args.extend_from_slice(&["--bin", "yalda-session-server"]);
                    }
                    let out = std::process::Command::new("cargo")
                        .args(&build_args)
                        .current_dir(&manifest_dir)
                        .output();

                    if restart_server
                        && let Ok(o) = &out
                        && o.status.success()
                    {
                        // Kill any running server (both profiles) and clear the
                        // stale socket/pid so the fresh GUI launches the server
                        // it just built instead of reconnecting to the old one.
                        for pat in [
                            "target/debug/yalda-session-server",
                            "target/release/yalda-session-server",
                        ] {
                            let _ = std::process::Command::new("pkill")
                                .args(["-f", pat])
                                .status();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        let _ = std::fs::remove_file(yalda::session_proto::socket_path());
                        let _ = std::fs::remove_file(yalda::session_proto::pid_file_path());
                    }
                    out
                })
                .await;

            let _ = this.update(cx, |this, cx| match built {
                Ok(out) if out.status.success() => {
                    // Relaunch the freshly-built RELEASE binary. stderr is
                    // inherited so post-restart logs reach the dev terminal
                    // (unlike reboot_into_claude's fully-detached stdio).
                    let mut cmd = std::process::Command::new(&gui_bin);
                    cmd.args(&args);
                    cmd.stdin(std::process::Stdio::null());
                    cmd.stdout(std::process::Stdio::null());
                    cmd.stderr(std::process::Stdio::inherit());
                    match cmd.spawn() {
                        Ok(_) => {
                            this.set_agent_status(
                                "rebuilt — relaunching, this window will close",
                                cx,
                            );
                            // Quit promptly: the new instance re-attaches to
                            // every server session on startup (strict 1:1).
                            cx.quit();
                        }
                        Err(e) => this.set_agent_status(&format!("relaunch spawn failed: {e}"), cx),
                    }
                }
                Ok(out) => {
                    // Surface the tail of stderr so the failure is actionable.
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
                    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
                    this.set_agent_status(&format!("build failed: {tail}"), cx);
                }
                Err(e) => this.set_agent_status(&format!("build error: {e}"), cx),
            });
        })
        .detach();
    }

    fn zoom_in(&mut self, _: &ZoomIn, _w: &mut Window, cx: &mut Context<Self>) {
        self.set_text_scale(self.text_scale * TEXT_SCALE_STEP, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _w: &mut Window, cx: &mut Context<Self>) {
        self.set_text_scale(self.text_scale / TEXT_SCALE_STEP, cx);
    }

    fn zoom_reset(&mut self, _: &ZoomReset, _w: &mut Window, cx: &mut Context<Self>) {
        self.set_text_scale(1.0, cx);
    }

    fn set_text_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        let clamped = scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE);
        if (clamped - self.text_scale).abs() > f32::EPSILON {
            self.text_scale = clamped;
            self.save_settings();
            // Text-zoom is GLOBAL, not session state (ticket 021): the action
            // handler notifies each live transcript view directly so the cached
            // subtree re-renders with the new scale. UXI-TextZoom-1: the transcript's
            // conversation prose + markdown blocks scale by `text_scale` (read
            // from the root in `RootSnapshot`), like the buffer doc view; this is
            // the audited invalidation path that makes that re-read take effect.
            self.notify_transcript_views(MissReason::TextStyle, cx);
            self.notify_linear_views(MissReason::TextStyle, cx);
            self.notify_keymap_views(MissReason::TextStyle, cx);
            cx.notify();
        }
    }

    /// Notify every live [`TranscriptView`] (ticket 021). Theme and text-zoom
    /// are GLOBAL, not session state, so their action handlers — which run in
    /// event context, outside any draw (timing-correct, fact 4) — bust each
    /// cached transcript directly rather than relying on a session observe.
    /// `reason` is logged for the `YALDA_PERF` notify-reason counter.
    /// Flip the agent-chat heading-marker toggle (agent `.` menu → "toggle
    /// heading markers"). Global across all transcripts; pushed to every live
    /// `TranscriptView` via `notify_transcript_views` (a global render input,
    /// not a per-session seq — see the `RootSnapshot` note). Default on.
    fn toggle_agent_heading_markers(&mut self, cx: &mut Context<Self>) {
        self.show_agent_heading_markers = !self.show_agent_heading_markers;
        let on = self.show_agent_heading_markers;
        self.transient_status = Some(if on {
            "heading markers on".into()
        } else {
            "heading markers off".into()
        });
        self.notify_transcript_views(MissReason::Refresh, cx);
        cx.notify();
    }

    /// Toggle the focused agent tile's user-turn jump mode (agent `.` menu →
    /// "jump between user turns"). While on, `j`/`k` in Normal mode step the
    /// viewport between the user's input turns (see `on_key_down`). Turning it
    /// ON jumps straight to the most recent user turn ("what I wrote last").
    /// Per-session state on `AgentState` — toggling in one tile leaves others
    /// untouched.
    fn toggle_agent_jump_mode(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            self.transient_status = Some("no agent here".into());
            cx.notify();
            return;
        };
        let Some(ent) = self.sessions.get(id).cloned() else {
            return;
        };
        let now_on = ent.update(cx, |s, scx| {
            s.state.user_turn_jump_mode = !s.state.user_turn_jump_mode;
            scx.notify();
            s.state.user_turn_jump_mode
        });
        if now_on {
            // Jump straight to the most recent user input.
            self.jump_user_turn(0, true, cx);
            self.transient_status = Some("user-turn jump: j/k to move".into());
        } else {
            self.transient_status = Some("user-turn jump off".into());
        }
        cx.notify();
    }

    /// Move the focused agent transcript's jump cursor between user turns and
    /// queue a reveal. `to_last` ignores `delta` and parks on the most recent
    /// user turn; otherwise the ordinal steps by `delta` (clamped to the live
    /// count). Parks `follow_output` so streaming output doesn't yank the
    /// viewport back, then notifies the session so its cached transcript
    /// re-renders and resolves+scrolls the reveal (INV-RV; see `build_body`).
    fn jump_user_turn(&mut self, delta: i32, to_last: bool, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let Some(ent) = self.sessions.get(id).cloned() else {
            return;
        };
        ent.update(cx, |s, scx| {
            let c = &mut s.state;
            let count = user_turn_item_indices(&c.view_model.flat_items_cache).len();
            if count == 0 {
                c.status = Some("no user turns yet".into());
                scx.notify();
                return;
            }
            let prev = c.user_turn_jump_ord;
            let next = next_jump_ord(prev, count, delta, to_last);
            // A `j` pressed while already parked on the newest user turn (the
            // ordinal can't advance) means "go past the last turn" — drop the
            // viewport at the page end of the buffer instead of re-revealing
            // the last header. `k`/toggle-on keep their per-turn behavior.
            let to_end = jump_lands_at_page_end(prev, next, count, delta, to_last);
            c.user_turn_jump_ord = next;
            c.pending_jump_ord = Some(next);
            c.pending_jump_end = to_end;
            c.follow_output.set(false);
            c.status = Some(if to_end {
                "page end".into()
            } else {
                format!("user turn {}/{}", next + 1, count).into()
            });
            scx.notify();
        });
        cx.notify();
    }

    fn notify_transcript_views(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        for v in self.transcript_views.values() {
            let label = v.read(cx).perf_label;
            record_notify(label, reason);
            v.update(cx, |_tv, vcx| vcx.notify());
        }
    }

    /// Notify every live [`LinearView`] (yux cached body). Theme and text-zoom
    /// are GLOBAL, not per-tile state — and the Linear body reads both (it
    /// scales with zoom), so their action handlers bust each cached body
    /// directly, the same pushed-invalidation contract as the transcript views
    /// (`yux/CLAUDE.md` rule 4). Linear views live in the layout tree (owned by
    /// their tile), so collect handles in an immutable walk, then update each.
    fn notify_linear_views(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        let mut views: Vec<Entity<LinearView>> = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.layout.for_each_leaf(&mut |w| {
                if let App::Linear(tile) = &w.content
                    && let Some(v) = &tile.view
                {
                    views.push(v.clone());
                }
            });
        }
        for v in views {
            let label = v.read(cx).perf_label();
            record_notify(label, reason);
            v.update(cx, |_lv, vcx| vcx.notify());
        }
    }

    /// Notify every live [`KeymapView`] cached body — same global-invalidation
    /// contract as [`notify_linear_views`] (the keymap body reads theme + zoom
    /// off the root, so a theme/zoom change must bust it directly).
    fn notify_keymap_views(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        let mut views: Vec<Entity<KeymapView>> = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.layout.for_each_leaf(&mut |w| {
                if let App::Keymap(tile) = &w.content
                    && let Some(v) = &tile.view
                {
                    views.push(v.clone());
                }
            });
        }
        for v in views {
            record_notify("keymap", reason);
            v.update(cx, |_kv, vcx| vcx.notify());
        }
    }

    /// Tick the thinking-indicator clock (ticket 021). The `Thinking… mm:ss`
    /// label + 30s stall warning live INSIDE the cached `TranscriptView`, so the
    /// ~1Hz anim tick must bust each *awaiting* session's cached transcript
    /// directly — a root `cx.notify()` cannot dirty a cached child (facts 3/6),
    /// and no session seq moves during a stall (the elapsed/quiet timers are
    /// `last_event_at`-relative, not in `TranscriptSeqs`), so the session
    /// observe never fires. The anim tick runs in timer context, outside any
    /// draw (timing-correct, fact 4). Returns whether any view was notified.
    pub(crate) fn tick_awaiting_transcript_views(&mut self, cx: &mut Context<Self>) -> bool {
        let awaiting: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.read(cx).state.turn_phase.is_awaiting())
            .map(|(id, _)| id)
            .collect();
        let mut ticked = false;
        for id in awaiting {
            if let Some(v) = self.transcript_views.get(&id) {
                let label = v.read(cx).perf_label;
                record_notify(label, MissReason::Refresh);
                v.update(cx, |_tv, vcx| vcx.notify());
                ticked = true;
            }
        }
        ticked
    }

    /// Snapshot the persistable UI settings (theme, agent info-bar placement,
    /// text zoom) and write them in ONE place. Each settings mutation just calls
    /// this instead of re-listing every field at its own `save_preferences(...)`
    /// site — the structural cause of "added a setting, forgot to persist it at
    /// one of N sites" drift. Fonts are not yet user-settable, so not persisted.
    fn save_settings(&self) {
        save_preferences(&Preferences {
            theme: Some(self.theme.name.as_kebab().to_string()),
            text_scale: Some(self.text_scale),
            desktop_grid_cols: Some(self.desktop_grid_cols),
            desktop_grid_rows: Some(self.desktop_grid_rows),
            desktop_grid_defaults_version: Some(DESKTOP_GRID_DEFAULTS_VERSION),
            jump_panel_visible: Some(self.jump_panel_visible),
            jump_cwd_order: (!self.jump_cwd_order.is_empty()).then(|| self.jump_cwd_order.clone()),
            jump_session_order: (!self.jump_session_order.is_empty())
                .then(|| self.jump_session_order.clone()),
            jump_folded_projects: (!self.jump_folded_projects.is_empty()).then(|| {
                let mut names: Vec<_> = self.jump_folded_projects.iter().cloned().collect();
                names.sort();
                names
            }),
        });
    }

    /// Toggle the jump panel's visibility (`cmd-j` / `?` menu). Global action —
    /// wired with `on_action` on every screen root. Persisted via `save_settings`.
    fn toggle_jump_panel(&mut self, _: &ToggleJumpPanel, _w: &mut Window, cx: &mut Context<Self>) {
        self.toggle_jump_panel_impl(cx);
    }

    pub(crate) fn toggle_jump_panel_impl(&mut self, cx: &mut Context<Self>) {
        self.jump_panel_visible = !self.jump_panel_visible;
        self.save_settings();
        cx.notify();
    }

    /// Screen background pulled from the active theme. Used by every
    /// top-level `.bg(...)` in the render pipeline so light themes
    /// (Solarized Light, Financial Times) actually look light.
    fn editor_bg(&self) -> Hsla {
        ncolor_to_hsla(self.theme.editor_bg, BG)
    }

    /// Default foreground from the active theme. Used in place of the
    /// hardcoded `DEFAULT_FG` constant on every top-level `.text_color(...)`
    /// call so the editor text contrasts the theme's background.
    fn editor_fg(&self) -> Hsla {
        ncolor_to_hsla(self.theme.editor_fg, DEFAULT_FG)
    }

    /// Swap the active theme. Walks every Doc window in the workspace and
    /// re-renders its block list against the new palette (Edit / Browser /
    /// Claude pick the theme up on their next paint via `md_highlight` /
    /// direct theme reads). Persisting the choice across restarts is a
    /// follow-up — for now the theme resets to Dracula on launch.
    /// Flip between the two everyday themes: Nightfox (dark) and Folio (light).
    /// From Folio → Nightfox; from anything else (Nightfox or any other theme)
    /// → Folio, so the toggle always lands on one of the pair and alternates.
    /// `set_theme` does the heavy lifting (syntect swap, doc re-render,
    /// transcript bust, persist).
    fn toggle_theme_now(&mut self, cx: &mut Context<Self>) {
        self.set_theme(next_toggle_theme(self.theme.name), cx);
    }

    /// `Cmd-Shift-T` action handler (and the `theme-toggle` command).
    fn toggle_theme(&mut self, _: &ToggleTheme, _w: &mut Window, cx: &mut Context<Self>) {
        self.toggle_theme_now(cx);
    }

    fn set_theme(&mut self, name: ThemeName, cx: &mut Context<Self>) {
        if self.theme.name == name {
            return;
        }
        self.theme = Theme::from_name(name);
        // Keep the edit-view syntect highlighter in lockstep with the theme:
        // light themes need light syntect colors. The highlight cache keys on
        // theme name, so stale dark-on-light lines are invalidated next paint.
        self.syntect_hl = Rc::new(yalda::highlight::Highlighter::with_syntect_theme(
            self.theme.name.syntect_theme(),
        ));
        for wsp in self.workspace.workspaces.iter_mut() {
            re_render_layout_docs(&mut wsp.layout, &self.theme);
        }
        // Agent transcripts cache parsed code/table blocks with their span
        // colors baked in at the old theme (keyed by content, not theme), so
        // invalidate every live session's block + S1 caches to force a
        // re-parse under the new theme — otherwise a code block parsed under
        // the prior theme renders with stale colors (e.g. a light-on-light box
        // surviving Folio → Nightfox). See `AgentViewModel::invalidate_theme`.
        let session_ids: Vec<SessionId> = self.sessions.iter().map(|(id, _)| id).collect();
        for id in session_ids {
            if let Some(ent) = self.sessions.get(id) {
                ent.update(cx, |s, _| s.state.view_model.invalidate_theme());
            }
        }
        // Theme is GLOBAL, not session state (ticket 021): the transcript reads
        // the theme's agent palette in its render, so the theme-swap handler
        // busts each live transcript view directly (event context, fact 4).
        self.notify_transcript_views(MissReason::Refresh, cx);
        self.notify_linear_views(MissReason::Refresh, cx);
        self.notify_keymap_views(MissReason::Refresh, cx);
        self.save_settings();
        cx.notify();
    }

    // ---- View-mode mouse selection ----------------------------------------

    /// Hit-test a window-space position against the per-line `TextLayout`s
    /// captured during the most recent render. Returns the doc position
    /// (block_idx, line_idx, char_offset) if the point falls on a tracked
    /// line, else `None`. For points off the right edge of a line, returns
    /// the line's end (caller may treat as past-the-end selection).
    fn doc_pos_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<DocPos> {
        let layouts = self.line_layouts.borrow();
        // Choose the line whose vertical band contains `position.y`. Ties are
        // broken by the smaller (block_idx, line_idx) — the map iteration
        // order doesn't matter because the bounds bands don't overlap.
        let mut hit: Option<(&(usize, usize), &TextLayout)> = None;
        for (key, layout) in layouts.iter() {
            let b = layout.bounds();
            if position.y >= b.top() && position.y <= b.bottom() {
                hit = Some((key, layout));
                break;
            }
        }
        let (key, layout) = hit?;
        // Map pixel position → byte index. `index_for_position` returns Ok
        // for in-line hits and Err for points past the right edge (it
        // still gives a valid index).
        let byte_idx = match layout.index_for_position(position) {
            Ok(i) => i,
            Err(i) => i,
        };
        let text = layout.text();
        let char_offset = text
            .char_indices()
            .position(|(b, _)| b >= byte_idx)
            .unwrap_or_else(|| text.chars().count());
        Some(DocPos {
            block_idx: key.0,
            line_idx: key.1,
            char_offset,
        })
    }

    fn doc_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(pos) = self.doc_pos_at(ev.position) else {
            // Click on chrome / empty area: clear any existing selection.
            if self.doc_selection.is_some() {
                self.doc_selection = None;
                cx.notify();
            }
            return;
        };
        self.doc_selection = Some(DocSelection {
            anchor: pos,
            head: pos,
            dragging: true,
        });
        cx.notify();
    }

    fn doc_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.doc_selection.map(|s| s.dragging).unwrap_or(false) {
            return;
        }
        let Some(pos) = self.doc_pos_at(ev.position) else {
            return;
        };
        if let Some(sel) = self.doc_selection.as_mut()
            && sel.head != pos
        {
            sel.head = pos;
            cx.notify();
        }
    }

    fn doc_mouse_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(sel) = self.doc_selection.as_mut() else {
            return;
        };
        sel.dragging = false;
        if sel.is_empty() {
            self.doc_selection = None;
        } else {
            // X11-style select-to-clipboard: finalizing a non-empty drag copies
            // the selection to the system clipboard automatically (no Cmd-C).
            let sel = *sel;
            if let Some(text) = self.collect_doc_selection_text(&sel)
                && !text.is_empty()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
        cx.notify();
    }

    /// Read the doc-view text covered by `doc_selection` and write it to
    /// the system clipboard. Walks blocks/lines in document order using
    /// the focused window's DocState as the source of truth for line text.
    fn copy_doc_selection(
        &mut self,
        _: &CopyDocSelection,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.doc_selection else {
            return;
        };
        let Some(text) = self.collect_doc_selection_text(&sel) else {
            return;
        };
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn collect_doc_selection_text(&self, sel: &DocSelection) -> Option<String> {
        let (start, end) = sel.normalized();
        let content = self.workspace.focused_content()?;
        let blocks = match content {
            App::Buffer(BufferApp::Viewing(d)) => &d.blocks,
            _ => return None,
        };
        let mut out = String::new();
        for bi in start.block_idx..=end.block_idx {
            let block = blocks.get(bi)?;
            let lines = block_selectable_lines(block);
            if lines.is_empty() {
                continue;
            }
            let l_start = if bi == start.block_idx {
                start.line_idx
            } else {
                0
            };
            let l_end = if bi == end.block_idx {
                end.line_idx
            } else {
                lines.len().saturating_sub(1)
            };
            for li in l_start..=l_end {
                let Some(line) = lines.get(li) else { continue };
                let line_text: String = line
                    .spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                let chars: Vec<char> = line_text.chars().collect();
                let s = if bi == start.block_idx && li == start.line_idx {
                    start.char_offset.min(chars.len())
                } else {
                    0
                };
                let e = if bi == end.block_idx && li == end.line_idx {
                    end.char_offset.min(chars.len())
                } else {
                    chars.len()
                };
                if s < e {
                    out.extend(chars[s..e].iter());
                }
                if li < l_end {
                    out.push('\n');
                }
            }
            if bi < end.block_idx {
                out.push_str("\n\n");
            }
        }
        Some(out)
    }

    /// Paste system clipboard contents into the active editor at the cursor.
    /// Works in Edit, Agent (worksheet + chatbox) screens — anywhere there's
    /// an editor in Insert mode.
    fn paste_from_clipboard(
        &mut self,
        _: &PasteFromClipboard,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        // Find the active editor + mode. Chatbox takes priority in chatbox mode.
        // Agent tiles route through `self.sessions`, so read the bound id first
        // and drop the workspace borrow before touching the store.
        let agent_bound = self.focused_bound_session();
        let pasted = if let Some(id) = agent_bound {
            self.with_session(id, cx, |c| {
                // Model C: paste always targets the compose buffer (the transcript
                // is read-only in both placements — INV-1).
                let cb = c.input_surface.compose_mut();
                if cb.mode == EditMode::Insert {
                    for ch in text.chars() {
                        cb.editor.insert_char(ch);
                    }
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
        } else {
            match self.workspace.focused_content_mut() {
                Some(App::Buffer(BufferApp::Editing(e))) => {
                    if e.mode == EditMode::Insert {
                        for ch in text.chars() {
                            e.editor.insert_char(ch);
                        }
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        };
        if pasted {
            cx.notify();
        }
    }

    /// Copy the current selection to the system clipboard. Dispatches based
    /// on which screen is active: doc view uses mouse selection, edit/agent
    /// views use editor selection.
    fn copy_selection(&mut self, _: &CopySelection, _w: &mut Window, cx: &mut Context<Self>) {
        // Doc view: delegate to existing mouse-selection copy.
        if let Some(sel) = self.doc_selection
            && let Some(text) = self.collect_doc_selection_text(&sel)
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        // Edit / Agent views: copy editor selection. Agent sessions live in
        // the store, so resolve the bound id first.
        let text = if let Some(id) = self.focused_bound_session() {
            self.read_session(id, cx, |c| {
                // Prefer the compose selection (the editable surface); fall back to
                // a transcript selection (read-only copy is fine — INV-1 forbids
                // writes, not reads).
                c.input_surface
                    .compose()
                    .editor
                    .selection_text()
                    .or_else(|| c.editor.selection_text())
            })
            .flatten()
        } else {
            match self.workspace.focused_content() {
                Some(App::Buffer(BufferApp::Editing(e))) => e.editor.selection_text(),
                _ => None,
            }
        };
        if let Some(t) = text
            && !t.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
    }

    /// Drop every live `AcpChannelClient` we hold so its `Drop` impl can
    /// run the explicit teardown (signal worker, join thread, kill child)
    /// before the rest of the GPUI shutdown clears windows. Without this,
    /// the join races with `GpuiApp::shutdown` clearing entities; the worker
    /// usually finishes in time but the order is non-deterministic and
    /// lingering child agents have been observed at exit. Called from
    /// `on_app_quit` in `main`.
    fn shutdown_acp(&mut self, cx: &mut Context<Self>) {
        // Drop every session's channel so the worker thread shuts down its
        // child agent before GPUI's window teardown races with us. Sessions
        // are now owned centrally, so this is a single store walk; each session
        // lives in its own entity, so the channel drop goes through `update`.
        let ids: Vec<SessionId> = self.sessions.ids().collect();
        for id in ids {
            if let Some(ent) = self.session_entity(id) {
                ent.update(cx, |s, _| {
                    let _dropped = s.state.channel.take();
                });
            }
        }
    }

    fn next_buffer(&mut self, _: &NextBuffer, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.workspaces.len() > 1 {
            let next = (self.workspace.active_workspace + 1) % self.workspace.workspaces.len();
            self.switch_to_buffer(next);
            cx.notify();
        }
    }

    fn prev_buffer(&mut self, _: &PrevBuffer, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.workspaces.len() > 1 {
            let prev = if self.workspace.active_workspace == 0 {
                self.workspace.workspaces.len() - 1
            } else {
                self.workspace.active_workspace - 1
            };
            self.switch_to_buffer(prev);
            cx.notify();
        }
    }

    fn next_workspace(&mut self, _: &NextWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.workspaces.len() > 1 {
            self.workspace.next_workspace();
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Activate the workspace at `idx`. Mouse-click entry point from the workspace
    /// strip — no-ops if the index is out of range or already active.
    fn select_workspace(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.workspaces.len() || idx == self.workspace.active_workspace {
            return;
        }
        self.workspace.set_active_workspace(idx);
        self.save_workspace_state();
        cx.notify();
    }

    /// Jump to the `n`-th (1-based) workspace as numbered in the jump panel /
    /// goto-workspace menu — i.e. the `n`-th non-ephemeral workspace. `ctrl-<n>`
    /// entry point. No-ops if there is no such workspace (e.g. `ctrl-7` with
    /// four workspaces). Ephemeral virtual workspaces (ADR-0021) are skipped so the
    /// numbering matches what the panel shows.
    fn goto_workspace_number(&mut self, n: usize, cx: &mut Context<Self>) {
        if n == 0 {
            return;
        }
        if let Some((idx, _)) = self
            .workspace
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.ephemeral)
            .nth(n - 1)
        {
            self.select_workspace(idx, cx);
        }
    }

    fn prev_workspace(&mut self, _: &PrevWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.workspaces.len() > 1 {
            self.workspace.prev_workspace();
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Open a new workspace containing a Browser rooted at cwd. Spec Behavior 3:
    /// no-arg `:tabnew` / `Cmd-T` creates a browser tab so the user can pick
    /// what to load.
    fn new_workspace(&mut self, _: &NewWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        let project = self.workspace.inherited_project();
        let cwd = self.active_workspace_cwd().unwrap_or_else(process_cwd);
        self.workspace.push_initial_workspace(
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
            project,
        );
        self.save_workspace_state();
        cx.notify();
    }

    /// Close the active workspace. Spec Behavior 5: ClaudeWindows drop their ACP
    /// channels (subprocess killed via kill_on_drop). When the last workspace is
    /// closed, quit the app for now (placeholder-workspace Behavior 2 is a
    /// follow-up).
    fn close_workspace(&mut self, _: &CloseWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.workspaces.len() <= 1 {
            cx.quit();
            return;
        }
        let idx = self.workspace.active_workspace;
        self.workspace.close_workspace(idx);
        self.save_workspace_state();
        cx.notify();
    }

    fn rename_workspace(&mut self, _: &RenameWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_rename_active_workspace_overlay(cx);
    }

    /// `Ctrl-W m` — open the workspace picker to MOVE the focused tile.
    fn move_tile(&mut self, _: &MoveTile, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_picker(WorkspacePickerMode::Move, cx);
    }

    /// `Ctrl-W M` — open the workspace picker to ALSO-SHOW the focused tile.
    fn also_show_tile(&mut self, _: &AlsoShowTile, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_picker(WorkspacePickerMode::AlsoShow, cx);
    }

    /// `Ctrl-W s` — horizontal split: new tile below the focused one.
    fn split_h(&mut self, _: &SplitH, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::H);
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W v` — vertical split: new tile to the right of the focused one.
    fn split_v(&mut self, _: &SplitV, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::V);
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    /// Shared helper. The new tile mirrors the focused content kind:
    ///
    /// - Doc → new Doc over the same file (independent scroll/cursor).
    /// - Edit → new Edit over the same file path; the new editor reads
    ///   from disk so unsaved changes in the source tile don't carry over
    ///   (a shared buffer pool would fix that — separate stage).
    /// - Browser → new Browser at cwd.
    /// - Claude → new Browser at cwd (Claude is exclusive per spec).
    ///
    /// Browser is the universal fallback when the focused content has no
    /// natural file tile to clone (Claude) or when reading the source
    /// file fails.
    fn split_focused_with_browser(&mut self, dir: workspace::SplitDir) {
        let cwd = self.active_workspace_cwd().unwrap_or_else(process_cwd);
        let content = self.clone_focused_for_split(&cwd);
        let _ = self.workspace.split_focused(dir, content);
    }

    fn clone_focused_for_split(&mut self, cwd: &std::path::Path) -> App {
        let (label, is_edit) = match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(d))) => (Some(d.file_label.clone()), false),
            Some(App::Buffer(BufferApp::Editing(e))) => (Some(e.file_label.clone()), true),
            _ => (None, false),
        };
        let browser_fallback = || {
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                cwd.to_path_buf(),
            )))
        };
        let Some(label) = label else {
            return browser_fallback();
        };
        let path = PathBuf::from(label.as_ref());
        if is_edit {
            // Bind the new tile to the SAME pooled core as the source tile:
            // open_and_retain returns the existing buffer id, so unsaved text
            // + undo are shared. Only cursor/scroll/selection are independent.
            match self.workspace.open_and_retain(&path) {
                Ok((id, core)) => App::Buffer(BufferApp::Editing(EditState::new(
                    SharedEditor::new(id, core),
                    label,
                    EditView::Code,
                ))),
                Err(_) => browser_fallback(),
            }
        } else {
            // 5c: bind the cloned Doc to the SAME pooled core (open_and_retain
            // dedups by canonical path), so it tracks live edits from any other
            // view of the file — the multi-home / also-show live case.
            match self.workspace.open_and_retain(&path) {
                Ok((id, core)) => {
                    let blocks = render_with_wiki(
                        &core.borrow().document().full_text(),
                        &self.theme,
                        Some(&path),
                    );
                    App::Buffer(BufferApp::Viewing(DocState::viewing(
                        blocks,
                        label,
                        Some(DocSource::new(id, core)),
                    )))
                }
                Err(_) => browser_fallback(),
            }
        }
    }

    /// `Ctrl-W c` — close the focused window. If it was the only window in
    /// the wsp, close the workspace instead.
    fn close_window(&mut self, _: &CloseWindow, _w: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.close_focused() {
            Ok(Some(_new_focus)) => {
                self.workspace.retile_active();
                self.save_workspace_state();
                cx.notify();
            }
            Ok(None) => {
                // Focused leaf is the only one in its workspace. Close the workspace
                // if there are other workspaces; otherwise no-op — closing the
                // absolute last tile would leave the app with nothing to
                // render. Cmd-Q is the only quit path now.
                if self.workspace.workspaces.len() <= 1 {
                    return;
                }
                let idx = self.workspace.active_workspace;
                self.workspace.close_workspace(idx);
                self.save_workspace_state();
                cx.notify();
            }
            Err(()) => {}
        }
    }

    /// `Ctrl-W o` — keep only the focused window.
    fn only_window(&mut self, _: &OnlyWindow, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.only();
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W h/j/k/l` — move focus to a sibling split in that direction.
    fn focus_left(&mut self, _: &FocusLeft, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Left);
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_right(&mut self, _: &FocusRight, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Right);
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_up(&mut self, _: &FocusUp, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Up);
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_down(&mut self, _: &FocusDown, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Down);
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W w` / `Ctrl-W W` — cycle focus through leaves in tree order.
    fn focus_next(&mut self, _: &FocusNext, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_next();
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_prev(&mut self, _: &FocusPrev, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_prev();
        self.sync_rail_focus_after_motion();
        self.save_workspace_state();
        cx.notify();
    }

    /// The slot a KEYBOARD-driven semantic-zoom step re-anchors on
    /// (spec-infinite-plane-workspace.md Behavior 3): the focused tile's slot,
    /// or the viewport-center slot when nothing is focused / the focused tile has
    /// no slot. Mirrors the pixel-context setup `desktop_scroll` builds for
    /// `desktop_zoom_anchor`, but sourced from the active workspace + captured canvas
    /// bounds (there's no pointer event here).
    fn plane_keyboard_zoom_anchor(&self) -> workspace::Slot {
        let workspace_idx = self.workspace.active_workspace;
        let full_tile = self.desktop_tile_px();
        let (_, _, mut cw, mut ch) = self.desktop_canvas_bounds.get();
        if cw <= 0.0 {
            cw = self.viewport_width_px.max(1.0);
        }
        if ch <= 0.0 {
            ch = self.viewport_height_px.max(1.0);
        }
        let cam = self.workspace.workspaces[workspace_idx].desktop.camera;
        let scale = workspace::detail_scale(cam.zoom);
        let tile = (full_tile.0 * scale, full_tile.1 * scale);
        let g = 12.0 * scale; // DESKTOP_GUTTER (chrome.rs); pitch-independent.
        let pitch = (tile.0 + g, tile.1 + g);
        let pan = (cam.pan.0 * pitch.0, cam.pan.1 * pitch.1);
        let focused_id = self.workspace.workspaces[workspace_idx].focused;
        self.desktop_zoom_anchor(workspace_idx, focused_id, tile, g, pan, cw, ch)
    }

    /// `Ctrl-W -` — step the active plane's semantic zoom one level OUT
    /// (`Full → Card → Minimap`, clamped), re-anchored on the focused tile / view
    /// center (spec-infinite-plane-workspace.md Behavior 3). Reclaimed from the
    /// retired `ResizeShrink`.
    fn zoom_out_workspace(
        &mut self,
        _: &ZoomOutWorkspace,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = self.plane_keyboard_zoom_anchor();
        let workspace_idx = self.workspace.active_workspace;
        self.workspace.workspaces[workspace_idx].desktop.zoom_out(anchor);
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W =` — step the active plane's semantic zoom one level IN
    /// (`Minimap → Card → Full`, clamped). Reclaimed from the retired `Equalize`.
    fn zoom_in_workspace(&mut self, _: &ZoomInWorkspace, _w: &mut Window, cx: &mut Context<Self>) {
        let anchor = self.plane_keyboard_zoom_anchor();
        let workspace_idx = self.workspace.active_workspace;
        self.workspace.workspaces[workspace_idx].desktop.zoom_in(anchor);
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W 0` — reset the active plane's camera to the origin
    /// (`pan=(0,0)`, `zoom=Full`; spec-infinite-plane-workspace.md Behavior 6).
    /// View-only: no tile moves or is re-seeded.
    fn reset_workspace_view(
        &mut self,
        _: &ResetWorkspaceView,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_idx = self.workspace.active_workspace;
        self.workspace.workspaces[workspace_idx].desktop.reset_view();
        self.save_workspace_state();
        cx.notify();
    }

    // ---- Layout patterns (spec-layout-patterns.md) -------------------------

    // Phase 1: marks

    /// Check if a bare key press starts a mark chord (`m` or `'`).
    /// Returns true if the key was consumed.
    fn try_start_mark_chord(
        &mut self,
        key: &Key,
        modifiers: &KMods,
        cx: &mut Context<Self>,
    ) -> bool {
        if !modifiers.is_empty() {
            return false;
        }
        match key {
            Key::Char('m') => {
                self.pending_mark_chord = Some('m');
                cx.notify();
                true
            }
            Key::Char('\'') => {
                self.pending_mark_chord = Some('\'');
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// `on_key_down` handler for the Doc view — intercepts bare `m`/`'` to
    /// start a mark chord.
    fn handle_doc_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Universal leaders: the doc view is never text entry, so `<space>`/`.`/
        // `?` always open the menus (with top priority).
        if self.leader_intercept(&press, cx) {
            return;
        }

        // Ctrl-S: save the backing buffer from doc view (same as edit view).
        if press.modifiers.contains(KMods::CONTROL)
            && matches!(press.key, Key::Char('s') | Key::Char('S'))
        {
            if let Some(d) = self.doc_mut() {
                if let Some(source) = d.source.as_ref() {
                    let msg: SharedString = match source.core.borrow_mut().save() {
                        Ok(()) => "saved".into(),
                        Err(e) => format!("save failed: {}", e).into(),
                    };
                    self.transient_status = Some(msg);
                } else {
                    self.transient_status = Some("no file to save".into());
                }
            }
            cx.notify();
            return;
        }

        if self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
            cx.stop_propagation();
        }
    }

    /// Complete a pending mark chord (the follow-up key after `m` or `'`).
    fn complete_mark_chord(&mut self, key: char, cx: &mut Context<Self>) {
        let chord_type = match self.pending_mark_chord.take() {
            Some(c) => c,
            None => return,
        };
        cx.notify();

        match chord_type {
            'm' => {
                // Set mark
                if let Some(wid) = self.workspace.focused_window_id() {
                    self.workspace.marks.set(key, wid);
                    self.transient_status = Some(format!("mark '{key}' set").into());
                    self.save_workspace_state();
                }
            }
            '\'' => {
                // Jump to mark
                if let Some(target_wid) = self.workspace.marks.get(key) {
                    self.jump_to_window(target_wid);
                    self.save_workspace_state();
                } else {
                    self.transient_status = Some(format!("mark '{key}' not set").into());
                }
            }
            _ => {}
        }
    }

    /// Jump focus to a specific window (cross-workspace). Updates `prev_jump`.
    fn jump_to_window(&mut self, target_id: workspace::WindowId) {
        let Some(workspace_idx) = self.workspace.workspace_containing(target_id) else {
            // Stale mark — GC
            let live = self.workspace.all_window_ids();
            self.workspace.marks.gc(&live);
            self.transient_status = Some("mark target no longer exists".into());
            return;
        };

        let current_wid = self.workspace.focused_window_id();
        let cross_workspace = workspace_idx != self.workspace.active_workspace;

        if cross_workspace {
            if let Some(wid) = current_wid {
                self.workspace.marks.prev_jump = Some(wid);
            }
            // Route through the switch chokepoint so a departing virtual
            // workspace is torn down (ADR-0021); the index math inside accounts
            // for the removal so `active_workspace` still lands on `target_id`'s workspace.
            self.workspace.set_active_workspace(workspace_idx);
        }

        if let Some(wsp) = self.workspace.active_workspace_mut() {
            wsp.focused = target_id;
        }
    }

    // Phase 3: tags

    fn tag_view_chord(&mut self, _: &TagViewChord, _w: &mut Window, cx: &mut Context<Self>) {
        self.pending_tag_chord = Some('t');
        cx.notify();
    }

    fn tag_toggle_chord(&mut self, _: &TagToggleChord, _w: &mut Window, cx: &mut Context<Self>) {
        self.pending_tag_chord = Some('T');
        cx.notify();
    }

    fn clear_tag_view(&mut self, _: &ClearTagView, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(wsp) = self.workspace.active_workspace_mut() {
            wsp.tag_view.clear();
        }
        self.transient_status = Some("tag filter cleared".into());
        self.save_workspace_state();
        cx.notify();
    }

    /// Complete a pending tag chord (the follow-up key after `Ctrl-W t`/`Ctrl-W Ctrl-T`).
    fn complete_tag_chord(&mut self, key: char, cx: &mut Context<Self>) {
        let chord_type = match self.pending_tag_chord.take() {
            Some(c) => c,
            None => return,
        };
        cx.notify();

        let tag_name = match self.workspace.tag_shortcuts.get(&key) {
            Some(name) => name.clone(),
            None => {
                self.transient_status = Some(format!("no tag bound to '{key}'").into());
                return;
            }
        };

        match chord_type {
            't' => {
                // View tag (replace)
                if let Some(wsp) = self.workspace.active_workspace_mut() {
                    wsp.tag_view.clear();
                    wsp.tag_view.insert(tag_name.clone());
                }
                self.adjust_focus_for_tag_view();
                self.transient_status = Some(format!("viewing tag: {tag_name}").into());
            }
            'T' => {
                // Toggle tag in view
                if let Some(wsp) = self.workspace.active_workspace_mut() {
                    if wsp.tag_view.contains(&tag_name) {
                        wsp.tag_view.remove(&tag_name);
                    } else {
                        wsp.tag_view.insert(tag_name.clone());
                    }
                }
                self.adjust_focus_for_tag_view();
            }
            _ => {}
        }
        self.save_workspace_state();
    }

    /// Get the FileBufferId for the focused window, if file-backed.
    fn focused_buffer_id(&self) -> Option<workspace::FileBufferId> {
        match self.workspace.focused_content()? {
            App::Buffer(BufferApp::Viewing(d)) => d.source.as_ref().map(|s| s.buffer_id),
            App::Buffer(BufferApp::Editing(e)) => Some(e.editor.buffer_id),
            _ => None,
        }
    }

    /// Tag the focused buffer. Returns false if not file-backed.
    fn tag_focused(&mut self, tag: String) -> bool {
        let id = match self.focused_buffer_id() {
            Some(id) => id,
            None => return false,
        };
        if let Some(buf) = self.workspace.file_buffers.get_mut(&id) {
            buf.tags.insert(tag);
            true
        } else {
            false
        }
    }

    /// Untag the focused buffer.
    fn untag_focused(&mut self, tag: &str) -> bool {
        let id = match self.focused_buffer_id() {
            Some(id) => id,
            None => return false,
        };
        if let Some(buf) = self.workspace.file_buffers.get_mut(&id) {
            buf.tags.remove(tag);
            true
        } else {
            false
        }
    }

    /// Collect all tags across all buffers in the pool.
    fn all_tags(&self) -> std::collections::BTreeSet<String> {
        let mut tags = std::collections::BTreeSet::new();
        for buf in self.workspace.file_buffers.values() {
            tags.extend(buf.tags.iter().cloned());
        }
        tags
    }

    /// Check if a window should be visible given the active workspace's tag_view.
    fn window_visible_for_tag_view(
        content: &App,
        tag_view: &workspace::TagSet,
        file_buffers: &HashMap<workspace::FileBufferId, workspace::FileBuffer>,
    ) -> bool {
        if tag_view.is_empty() {
            return true;
        }
        let buf_id = match content {
            App::Buffer(BufferApp::Viewing(d)) => d.source.as_ref().map(|s| s.buffer_id),
            App::Buffer(BufferApp::Editing(e)) => Some(e.editor.buffer_id),
            _ => return false, // Agent/Browser hidden when filter active
        };
        let Some(id) = buf_id else { return false };
        let Some(buf) = file_buffers.get(&id) else {
            return false;
        };
        buf.tags.iter().any(|t| tag_view.contains(t))
    }

    /// If the focused window is hidden by the tag filter, move focus to the
    /// first visible window.
    fn adjust_focus_for_tag_view(&mut self) {
        let tag_view = match self.workspace.active_workspace() {
            Some(t) => t.tag_view.clone(),
            None => return,
        };
        if tag_view.is_empty() {
            return;
        }

        // Check if currently focused window is visible.
        let focused = match self.workspace.focused_window_id() {
            Some(id) => id,
            None => return,
        };

        let focused_visible = self
            .workspace
            .active_workspace()
            .and_then(|t| t.layout.find_leaf(focused))
            .map(|w| {
                Self::window_visible_for_tag_view(
                    &w.content,
                    &tag_view,
                    &self.workspace.file_buffers,
                )
            })
            .unwrap_or(false);

        if focused_visible {
            return;
        }

        // Find first visible window.
        let ids = self
            .workspace
            .active_workspace()
            .map(|t| t.layout.leaf_ids())
            .unwrap_or_default();

        for id in ids {
            let visible = self
                .workspace
                .active_workspace()
                .and_then(|t| t.layout.find_leaf(id))
                .map(|w| {
                    Self::window_visible_for_tag_view(
                        &w.content,
                        &tag_view,
                        &self.workspace.file_buffers,
                    )
                })
                .unwrap_or(false);
            if visible {
                if let Some(wsp) = self.workspace.active_workspace_mut() {
                    wsp.focused = id;
                }
                return;
            }
        }
        // No visible windows
        let tags: Vec<_> = tag_view.iter().cloned().collect();
        self.transient_status = Some(format!("no buffers match tags: {}", tags.join(", ")).into());
    }

    // ---- Browser actions ----------------------------------------------------

    // ---- Edit mode ---------------------------------------------------------

    // ---- Menu (TUI-style picker) ------------------------------------------

    /// Action wrapper bound to `Space` in screens without an Insert mode
    /// (Doc / Browser). For Edit / Claude, the menu opens via the
    /// `NormalOutcome::OpenMenu` path inside their key handlers.
    fn open_menu(&mut self, _: &OpenMenu, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_menu_inner(cx);
    }

    // ---- ActiveOverlay accessors (the single mutually-exclusive overlay) ----
    //
    // The per-variant `_ref`/`_mut` accessors return `Option<&T>`/`Option<&mut T>`
    // so existing `match &self.X { Some(x) => .. }` / `if let Some(x) = &mut self.X`
    // shapes survive as the smallest textual swap. They borrow the whole
    // `active_overlay` field for the duration of the `if let`/`match`, exactly
    // reproducing the old per-field borrow (one field, no cross-variant aliasing).

    fn has_overlay(&self) -> bool {
        !matches!(self.active_overlay, ActiveOverlay::None)
    }
    fn overlay_is_menu(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::Menu(_))
    }
    fn overlay_is_buffer(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::BufferSwitcher(_))
    }
    fn overlay_is_workspace(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::WorkspacePicker(_))
    }
    fn overlay_is_rename(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::Rename(_))
    }
    fn overlay_is_tag_input(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::TagInput(_))
    }
    fn overlay_is_new_project(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::NewProject(_))
    }
    fn overlay_is_confirm_delete(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::ConfirmProjectDelete(_))
    }
    fn new_project_ref(&self) -> Option<&NewProjectOverlay> {
        if let ActiveOverlay::NewProject(o) = &self.active_overlay {
            Some(o)
        } else {
            None
        }
    }
    fn new_project_mut(&mut self) -> Option<&mut NewProjectOverlay> {
        if let ActiveOverlay::NewProject(o) = &mut self.active_overlay {
            Some(o)
        } else {
            None
        }
    }
    fn confirm_delete_ref(&self) -> Option<ProjectId> {
        if let ActiveOverlay::ConfirmProjectDelete(id) = &self.active_overlay {
            Some(*id)
        } else {
            None
        }
    }
    pub(crate) fn overlay_is_jump_palette(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::JumpPalette(_))
    }
    pub(crate) fn jump_palette_ref(&self) -> Option<&JumpPaletteOverlay> {
        if let ActiveOverlay::JumpPalette(p) = &self.active_overlay {
            Some(p)
        } else {
            None
        }
    }
    pub(crate) fn jump_palette_mut(&mut self) -> Option<&mut JumpPaletteOverlay> {
        if let ActiveOverlay::JumpPalette(p) = &mut self.active_overlay {
            Some(p)
        } else {
            None
        }
    }
    fn overlay_is_project_menu(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::ProjectMenu { .. })
    }
    /// The open project context menu's `(project, anchor x, anchor y)`, if any.
    fn project_menu_ref(&self) -> Option<(ProjectId, f32, f32)> {
        if let ActiveOverlay::ProjectMenu { pid, x, y } = &self.active_overlay {
            Some((*pid, *x, *y))
        } else {
            None
        }
    }

    fn menu_ref(&self) -> Option<&MenuOverlay> {
        if let ActiveOverlay::Menu(m) = &self.active_overlay {
            Some(m)
        } else {
            None
        }
    }
    /// Hands back the WHOLE `&mut MenuOverlay` (never a per-field accessor) so
    /// `m.state.process_key(press, &m.menu)`'s disjoint two-field split-borrow
    /// keeps type-checking.
    fn menu_mut(&mut self) -> Option<&mut MenuOverlay> {
        if let ActiveOverlay::Menu(m) = &mut self.active_overlay {
            Some(m)
        } else {
            None
        }
    }
    fn buffer_ref(&self) -> Option<&BufferSwitcher> {
        if let ActiveOverlay::BufferSwitcher(b) = &self.active_overlay {
            Some(b)
        } else {
            None
        }
    }
    fn buffer_mut(&mut self) -> Option<&mut BufferSwitcher> {
        if let ActiveOverlay::BufferSwitcher(b) = &mut self.active_overlay {
            Some(b)
        } else {
            None
        }
    }
    fn workspace_picker_ref(&self) -> Option<&WorkspacePicker> {
        if let ActiveOverlay::WorkspacePicker(p) = &self.active_overlay {
            Some(p)
        } else {
            None
        }
    }
    fn workspace_picker_mut(&mut self) -> Option<&mut WorkspacePicker> {
        if let ActiveOverlay::WorkspacePicker(p) = &mut self.active_overlay {
            Some(p)
        } else {
            None
        }
    }
    fn rename_ref(&self) -> Option<&RenameOverlay> {
        if let ActiveOverlay::Rename(o) = &self.active_overlay {
            Some(o)
        } else {
            None
        }
    }
    fn rename_mut(&mut self) -> Option<&mut RenameOverlay> {
        if let ActiveOverlay::Rename(o) = &mut self.active_overlay {
            Some(o)
        } else {
            None
        }
    }
    fn tag_input_ref(&self) -> Option<&TagInputOverlay> {
        if let ActiveOverlay::TagInput(o) = &self.active_overlay {
            Some(o)
        } else {
            None
        }
    }
    fn tag_input_mut(&mut self) -> Option<&mut TagInputOverlay> {
        if let ActiveOverlay::TagInput(o) = &mut self.active_overlay {
            Some(o)
        } else {
            None
        }
    }

    /// Open an overlay, **replacing** any currently-active one (overlays never
    /// stack). The per-variant open guards are scoped to their OWN kind, so
    /// re-opening the same kind is a no-op while opening a different kind
    /// correctly supersedes.
    fn open_overlay(&mut self, overlay: ActiveOverlay) {
        self.active_overlay = overlay;
    }
    /// Dismiss the active overlay. Only ever called from inside the active
    /// variant's own handler (each `handle_*_key` is wired solely by its own
    /// render branch), so it can never clobber a sibling overlay.
    fn clear_overlay(&mut self) {
        self.active_overlay = ActiveOverlay::None;
    }

    fn open_menu_inner(&mut self, cx: &mut Context<Self>) {
        // No-op if already open (defensive — the action shouldn't fire then).
        if self.overlay_is_menu() {
            return;
        }
        let Some(opened_from) = self.workspace.focused_window_id() else {
            return;
        };
        // Opening the menu dismisses any lingering toast.
        self.transient_status = None;
        let mut state = MenuState::new();
        state.open();
        self.open_overlay(ActiveOverlay::Menu(MenuOverlay {
            state,
            menu: gpui_menu(),
            header: "MENU",
            leader: '.',
            opened_from,
            disabled: self.global_menu_disabled(),
        }));
        cx.notify();
    }

    /// Behavior 10: global-menu entries inapplicable to the focused content
    /// kind are disabled (dimmed, non-dispatching) rather than hidden, so the
    /// menu layout stays spatially stable.
    fn global_menu_disabled(&self) -> HashSet<String> {
        // Every workspace-scoped command (set cwd, new agent/buffer, theme,
        // rebuild, mark tile) applies regardless of the focused content kind,
        // so nothing is context-disabled in the pruned menu.
        HashSet::new()
    }

    /// <space> — open the content-kind-specific local menu (spec-menu-scopes.md
    /// Behavior 2). Same overlay machinery as the global menu; only the tree
    /// and header differ.
    /// The agent tile's local menu with a live "switch model" submenu grafted
    /// on. The base entries are static (`agent_local_menu`); the submenu's
    /// children are built from the focused session's advertised model list so
    /// the picker reflects exactly what the agent offers (UXI-AgentTile-16). Each
    /// child dispatches `set-model:<id>`; the active model is marked `✓`. The
    /// "switch model" entry is ALWAYS present for discoverability — when the
    /// agent hasn't advertised a picklist yet it drills into a single disabled
    /// "(models not available yet)" label rather than vanishing.
    fn agent_local_menu_dynamic(&self, cx: &mut Context<Self>) -> Vec<MenuNode> {
        let mut menu = agent_local_menu();
        let (models, current) = self
            .focused_bound_session()
            .and_then(|id| {
                self.read_session(id, cx, |s| {
                    (s.available_models.clone(), s.agent_model.clone())
                })
            })
            .unwrap_or((Vec::new(), None));
        // Keys 1..=9 then 0 for a tenth — same digit convention as the global
        // workspace menu. More than ten models is not a real case.
        let keys = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
        let children: Vec<MenuNode> = if models.is_empty() {
            vec![MenuNode::label("(models not available yet)")]
        } else {
            models
                .iter()
                .take(keys.len())
                .enumerate()
                .map(|(i, m)| {
                    let is_current = current.as_deref() == Some(m.id.as_str());
                    let label = if is_current {
                        format!("{} ✓", m.label)
                    } else {
                        m.label.clone()
                    };
                    MenuNode::entry(keys[i], &label, &format!("set-model:{}", m.id))
                })
                .collect()
        };
        menu.push(MenuNode::submenu("M", "switch model", children));
        menu
    }

    fn open_local_menu_inner(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_menu() {
            return;
        }
        let Some(opened_from) = self.workspace.focused_window_id() else {
            return;
        };
        let (menu, header) = match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(_))) => (doc_local_menu(), "DOC"),
            Some(App::Buffer(BufferApp::Editing(_))) => (edit_local_menu(), "EDIT"),
            Some(App::Agent(_)) => (self.agent_local_menu_dynamic(cx), "AGENT"),
            Some(App::Buffer(BufferApp::Picking(_))) => (browser_local_menu(), "BROWSE"),
            Some(App::Linear(_)) => (linear_local_menu(), "LINEAR"),
            Some(App::Keymap(_)) => (keymap_local_menu(), "KEYBINDINGS"),
            None => return,
        };
        self.transient_status = None;
        let mut state = MenuState::new();
        state.open();
        self.open_overlay(ActiveOverlay::Menu(MenuOverlay {
            state,
            menu,
            header,
            leader: ' ',
            opened_from,
            disabled: HashSet::new(),
        }));
        cx.notify();
    }

    fn open_local_menu(&mut self, _: &OpenLocalMenu, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_local_menu_inner(cx);
    }

    /// `?` — open the global (Yaldabaoth) scope menu (untitled.md "Yalda aka
    /// Global Scope › Commands"). Yalda owns the set of workspaces, so the global
    /// commands manage them: jump to a workspace by number, name the current
    /// one, or make a new one. Built dynamically from the live workspace list.
    fn global_menu(&self) -> Vec<MenuNode> {
        let mut items = Vec::new();
        // Workspaces by number — 1..=9 then `0` for a tenth. Every workspace
        // holds ≥1 tile (so it's "inhabited"); a named-but-empty one would show
        // too. The active workspace is marked. Ephemeral virtual workspaces
        // (ADR-0021) are excluded — they're transient and always last, so the
        // surviving `i` values stay contiguous and match the real workspace indices.
        for (i, wsp) in self
            .workspace
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.ephemeral)
            .take(10)
        {
            let digit = if i == 9 { '0' } else { (b'1' + i as u8) as char };
            let marker = if i == self.workspace.active_workspace { "● " } else { "  " };
            items.push(MenuNode::entry(
                &digit.to_string(),
                &format!("{marker}{}: {}", i + 1, wsp.display_label()),
                &format!("goto-workspace-{i}"),
            ));
        }
        items.push(MenuNode::separator());
        items.push(MenuNode::entry("n", "name workspace", "rename-workspace"));
        items.push(MenuNode::entry("c", "new workspace", "new-workspace"));
        // New project lives here now that the jump panel dropped its top-level
        // ＋ row (UXI-JumpPanel-7); the per-project create/delete actions moved to
        // the project name's context menu (UXI-JumpPanel-8).
        items.push(MenuNode::entry("p", "new project", "new-project"));
        // (Agent sessions are now created only inside a project — the jump panel's
        // per-project context menu, not a global cwd overlay; UXI-Project-7.)
        let jp_label = if self.jump_panel_visible {
            "hide jump panel"
        } else {
            "show jump panel"
        };
        items.push(MenuNode::entry("j", jp_label, "toggle-jump-panel"));
        items
    }

    fn open_global_menu_inner(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_menu() {
            return;
        }
        let Some(opened_from) = self.workspace.focused_window_id() else {
            return;
        };
        self.transient_status = None;
        let mut state = MenuState::new();
        state.open();
        let menu = self.global_menu();
        self.open_overlay(ActiveOverlay::Menu(MenuOverlay {
            state,
            menu,
            header: "GLOBAL",
            leader: '?',
            opened_from,
            disabled: HashSet::new(),
        }));
        cx.notify();
    }

    fn open_global_menu(&mut self, _: &OpenGlobalMenu, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_global_menu_inner(cx);
    }

    /// Does the focused tile currently want raw text input? This is a property
    /// of the App/tile (untitled.md "tiles flag whether they are in an insert
    /// mode"): leaders are intercepted as menu-openers ONLY when this is false,
    /// so a tile in a navigation/selection state always reaches the menus while
    /// one in text entry keeps those characters. The session / project pickers
    /// are navigation, never text entry.
    fn focused_in_insert_mode(&self, cx: &GpuiApp) -> bool {
        match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Editing(e))) => e.mode == EditMode::Insert,
            Some(App::Buffer(BufferApp::Picking(b))) => {
                b.fb.filter_mode || b.fb.rename.is_some()
            }
            Some(App::Agent(tile)) => {
                if tile.session().is_none() {
                    false // unbound = session picker = navigation
                } else {
                    // Model C: a bound agent tile is "in text entry" iff focus is
                    // on the COMPOSE buffer AND it's in Insert — in BOTH placements.
                    // Transcript focus is read-only navigation, so leaders must
                    // work there. The transcript editor's own `mode` is irrelevant
                    // (it's read-only). The old worksheet arm read that transcript
                    // `mode` — which defaults to Insert — so leaders were wrongly
                    // suppressed in worksheet and `<space>` fell into the compose's
                    // normal dispatch, opening the workspace menu instead of the
                    // tile menu.
                    self.agent_read(cx, |c| {
                        // Focused compose in Insert is text entry. ALSO: mid-turn in
                        // the worksheet, input routes to the bottom chatbox even though
                        // focus stays on the transcript — but ONLY once the user has
                        // started a steer. Rule 7 (revised — runtime report: "the
                        // <space>/. leader menus don't work mid-turn"): with an EMPTY
                        // steering draft the worksheet is resting in nav, so the leaders
                        // MUST fire (open the menu); once the draft is non-empty the
                        // keystrokes belong to the chatbox so spaces stay spaces and the
                        // leaders are suppressed. Pinned by
                        // `real_midturn_worksheet_empty_draft_space_opens_menu` /
                        // `real_midturn_worksheet_typed_draft_space_is_suppressed`.
                        let draft_empty =
                            c.input_surface.compose().text().trim().is_empty();
                        // Focused compose in Insert is text entry — EXCEPT an empty
                        // WORKSHEET block: a fresh/cleared worksheet rests focused +
                        // Insert (so typing lands immediately, no `i`), but while its
                        // draft is still empty the `<space>`/`.`/`?` leaders must open
                        // the tile menu (typing a letter lands in the compose; a bare
                        // space on the blank block opens the menu). The chatbox is a
                        // real box: empty or not, it's text entry. Once the worksheet
                        // draft is non-empty, space types (multi-word). Same empty-draft
                        // rule as the mid-turn steer below (UXI-AgentTile-11 rule 7).
                        let compose_insert = c.focus == AgentFocus::Compose
                            && c.input_surface.compose().mode == EditMode::Insert
                            && (c.input_surface.is_chatbox() || !draft_empty);
                        let midturn_steer = c.turn_phase.is_awaiting()
                            && !c.input_surface.is_chatbox()
                            && !draft_empty;
                        compose_insert || midturn_steer
                    })
                    .unwrap_or(false)
                }
            }
            Some(App::Linear(tile)) => {
                let picker = tile
                    .view
                    .as_ref()
                    .map(|v| v.read(cx).is_picker())
                    .unwrap_or(false);
                !picker && tile.mode == LinearMode::Insert
            }
            // The keymap tile captures text while filtering or rebinding — the
            // leaders must be suppressed then so keys reach the box.
            Some(App::Keymap(_)) => self.keymap_captures_text(cx),
            Some(App::Buffer(BufferApp::Viewing(_))) | None => false,
        }
    }

    /// Universal leader dispatch: when the focused tile is NOT in text entry, the
    /// bare leader keys open their menus with TOP priority, before any
    /// tile-specific key handling. Returns true if it consumed the key (the
    /// caller then returns early). Every tile's `on_key_down` calls this first,
    /// so the menus are reachable from any tile that isn't capturing text —
    /// including the agent session picker and the Linear project picker.
    fn leader_intercept(&mut self, press: &KeyPress, cx: &mut Context<Self>) -> bool {
        if !press.modifiers.is_empty() || self.focused_in_insert_mode(cx) {
            return false;
        }
        match press.key {
            // space → per-tile / per-app (content-kind) menu;
            // `.` → per-workspace menu; `?` → global (Yaldabaoth) menu.
            Key::Char(' ') => self.open_local_menu_inner(cx),
            Key::Char('.') => self.open_menu_inner(cx),
            Key::Char('?') => self.open_global_menu_inner(cx),
            _ => return false,
        }
        true
    }

    /// Menu's key handler. Esc pops a level (or closes from root). Any
    /// other key is offered to `MenuState::process_key`; if it resolves
    /// to a command, the menu closes and `dispatch_menu_command` runs it.
    fn handle_menu_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let press = keystroke_to_keypress(&ev.keystroke);

        if press.key == Key::Esc {
            // Borrow must end before `clear_overlay()` (a `&mut self` call): bind
            // the close decision in a block that drops the `menu_mut()` borrow.
            let close = match self.menu_mut() {
                Some(m) => {
                    m.state.handle_escape();
                    !m.state.is_active()
                }
                None => false,
            };
            if close {
                self.clear_overlay();
            }
            cx.notify();
            return;
        }

        // `m.state` (mut) + `&m.menu` (shared) are disjoint fields of one
        // `&mut MenuOverlay`, so the split-borrow type-checks (see menu_mut doc).
        let cmd = match self.menu_mut() {
            Some(m) => {
                // Behavior 10: a key targeting a disabled command is treated
                // as unrecognized — no dispatch, the menu stays open. Peek
                // before `process_key` (which would close the menu state).
                let hits_disabled = m.state.current_nodes(&m.menu).iter().any(|node| {
                    node.key.len() == 1
                        && node.key[0] == press
                        && matches!(&node.action,
                            MenuAction::Command(name) if m.disabled.contains(name))
                });
                if hits_disabled {
                    return;
                }
                m.state.process_key(press, &m.menu)
            }
            None => return,
        };
        if let Some(name) = cmd {
            self.clear_overlay();
            self.dispatch_menu_command(&name, cx);
        }
        cx.notify();
    }

    /// Map a menu-leaf command name to the corresponding GPUI action.
    /// Unknown names are ignored (the menu is curated; new entries here
    /// require both a `MenuNode::entry` line in `gpui_menu()` and a
    /// match arm here).
    pub(crate) fn dispatch_menu_command(&mut self, name: &str, cx: &mut Context<Self>) {
        // Dynamic model-switch entries carry the target model id in the command
        // name (`set-model:<id>`); route them before the static match.
        if let Some(model_id) = name.strip_prefix("set-model:") {
            if matches!(
                self.workspace.focused_content().expect("no focused window"),
                App::Agent(_)
            ) {
                self.set_agent_model(model_id.to_string(), cx);
            }
            return;
        }
        match name {
            "open-browser" => self.open_browser_inner(cx),
            "buffer-list" => self.open_buffer_switcher(cx),
            "enter-edit" => self.enter_edit_with(EditView::Code, cx),
            "enter-wp" => self.enter_edit_with(EditView::WordProcessor, cx),
            "open-claude" => self.open_agent_inner(cx),
            "claude-send" => {
                // Only meaningful while the claude screen is active. Surface
                // a hint via the doc/edit footer if it isn't, so the user
                // gets a visible no-op instead of silent.
                if matches!(
                    self.workspace.focused_content().expect("no focused window"),
                    App::Agent(_)
                ) {
                    // Mode-aware submit: Worksheet sweep (§12) or Chatbox
                    // submit (§18) depending on `AgentState::input_mode`.
                    self.submit_agent(cx);
                }
            }
            "claude-new" => self.new_agent_session(None, cx),
            "codex-new" => self.new_agent_session_for(AgentProvider::Codex, None, cx),
            "claude-session-picker" => self.open_session_picker_rebind(cx),
            "claude-stop" => {
                if matches!(
                    self.workspace.focused_content().expect("no focused window"),
                    App::Agent(_)
                ) {
                    self.stop_agent_inner(cx);
                }
            }
            // UXI-AgentTile-22: `x` arms a confirm, it does not close.
            "claude-close" => self.arm_close_confirm(cx),
            "claude-reboot" => self.reboot_into_claude(cx),
            "claude-mode-cycle" => self.cycle_claude_permission_mode(cx),
            "claude-clear" => self.clear_agent_session(cx),
            "claude-rename" => self.open_rename_overlay(cx),
            "claude-cd" => self.open_change_agent_cwd_overlay(cx),
            "dev-restart-gui" => self.dev_rebuild_restart_gui(cx),
            "dev-restart-all" => self.dev_rebuild_restart_all(cx),
            "rail-files" => self.toggle_file_browser_rail_impl(cx),
            "rail-outline" => self.toggle_outline_rail_impl(cx),
            "rail-flip" => self.flip_rail_side_impl(cx),
            "agent-toggle-heading-markers" => self.toggle_agent_heading_markers(cx),
            "agent-toggle-jump-mode" => self.toggle_agent_jump_mode(cx),
            "recap-session" => self.summon_recap(cx),
            "recap-dismiss" => self.dismiss_recap(cx),
            "compose-toggle" | "agent-input-toggle" => {
                if matches!(
                    self.workspace.focused_content().expect("no focused window"),
                    App::Agent(_)
                ) {
                    self.toggle_agent_input_mode(cx);
                }
            }
            "linear-edit" => self.linear_set_mode(LinearMode::Insert, cx),
            "linear-open-url" => self.linear_open_url(cx),
            "linear-copy-url" => self.linear_copy_url(cx),
            "keymap-filter" => self.keymap_menu_filter(cx),
            "keymap-rebind" => self.keymap_menu_rebind(cx),
            "keymap-reset" => self.keymap_menu_reset(cx),
            "keymap-reset-all" => self.keymap_menu_reset_all(cx),
            "back-to-doc" => self.back_to_doc(cx),
            "reload-file" => self.reload_focused_from_disk(cx),
            "rename-workspace" => self.open_rename_active_workspace_overlay(cx),
            "toggle-jump-panel" => self.toggle_jump_panel_impl(cx),
            "workspace-set-cwd" => self.open_set_workspace_cwd_overlay(cx),
            "theme-dracula" => self.set_theme(ThemeName::Dracula, cx),
            "theme-nightfox" => self.set_theme(ThemeName::Nightfox, cx),
            "theme-solarized-light" => self.set_theme(ThemeName::SolarizedLight, cx),
            "theme-solarized-dark" => self.set_theme(ThemeName::SolarizedDark, cx),
            "theme-gruvbox-dark" => self.set_theme(ThemeName::GruvboxDark, cx),
            "theme-financial-times" => self.set_theme(ThemeName::FinancialTimes, cx),
            "theme-financial-times-dark" => self.set_theme(ThemeName::FinancialTimesDark, cx),
            "theme-folio" => self.set_theme(ThemeName::Folio, cx),
            "theme-toggle" => self.toggle_theme_now(cx),
            // Window splits + focus + sizing — same logic the keyboard
            // handlers run, so behavior is identical via either path.
            "split-h" => {
                self.split_focused_with_browser(workspace::SplitDir::H);
                self.save_workspace_state();
                cx.notify();
            }
            "split-v" => {
                self.split_focused_with_browser(workspace::SplitDir::V);
                self.save_workspace_state();
                cx.notify();
            }
            "close-window" => {
                match self.workspace.close_focused() {
                    Ok(Some(_)) => {
                        self.save_workspace_state();
                        cx.notify();
                    }
                    Ok(None) => {
                        // Same no-quit-on-last rule as the keyboard action.
                        if self.workspace.workspaces.len() <= 1 {
                            return;
                        }
                        let idx = self.workspace.active_workspace;
                        self.workspace.close_workspace(idx);
                        self.save_workspace_state();
                        cx.notify();
                    }
                    Err(()) => {}
                }
            }
            "only-window" => {
                let _ = self.workspace.only();
                self.save_workspace_state();
                cx.notify();
            }
            "focus-left" => {
                let _ = self.workspace.focus_motion(workspace::FocusDir::Left);
                self.save_workspace_state();
                cx.notify();
            }
            "focus-right" => {
                let _ = self.workspace.focus_motion(workspace::FocusDir::Right);
                self.save_workspace_state();
                cx.notify();
            }
            "focus-up" => {
                let _ = self.workspace.focus_motion(workspace::FocusDir::Up);
                self.save_workspace_state();
                cx.notify();
            }
            "focus-down" => {
                let _ = self.workspace.focus_motion(workspace::FocusDir::Down);
                self.save_workspace_state();
                cx.notify();
            }
            "focus-next" => {
                let _ = self.workspace.focus_next();
                self.save_workspace_state();
                cx.notify();
            }
            "focus-prev" => {
                let _ = self.workspace.focus_prev();
                self.save_workspace_state();
                cx.notify();
            }
            "new-project" => {
                self.open_new_project_overlay(cx);
            }
            "new-workspace" => {
                let project = self.workspace.inherited_project();
                let dir = self
                    .active_workspace_cwd()
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                self.workspace.push_initial_workspace(
                    App::Buffer(BufferApp::Picking(BrowserWindow::standalone(dir))),
                    project,
                );
                self.save_workspace_state();
                cx.notify();
            }
            "close-workspace" => {
                if self.workspace.workspaces.len() <= 1 {
                    cx.quit();
                    return;
                }
                let idx = self.workspace.active_workspace;
                self.workspace.close_workspace(idx);
                self.save_workspace_state();
                cx.notify();
            }
            "next-workspace" => {
                if self.workspace.workspaces.len() > 1 {
                    self.workspace.next_workspace();
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "prev-workspace" => {
                if self.workspace.workspaces.len() > 1 {
                    self.workspace.prev_workspace();
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "move-tile" => self.open_workspace_picker(WorkspacePickerMode::Move, cx),
            "also-show-tile" => self.open_workspace_picker(WorkspacePickerMode::AlsoShow, cx),
            // Plane view (spec-infinite-plane-workspace.md). Routed through the
            // same camera ops as the `Ctrl-W -/=/0` bindings.
            "plane-zoom-in" => {
                let anchor = self.plane_keyboard_zoom_anchor();
                let workspace_idx = self.workspace.active_workspace;
                self.workspace.workspaces[workspace_idx].desktop.zoom_in(anchor);
                self.save_workspace_state();
                cx.notify();
            }
            "plane-zoom-out" => {
                let anchor = self.plane_keyboard_zoom_anchor();
                let workspace_idx = self.workspace.active_workspace;
                self.workspace.workspaces[workspace_idx].desktop.zoom_out(anchor);
                self.save_workspace_state();
                cx.notify();
            }
            "plane-reset-view" => {
                let workspace_idx = self.workspace.active_workspace;
                self.workspace.workspaces[workspace_idx].desktop.reset_view();
                self.save_workspace_state();
                cx.notify();
            }
            "desktop-grid" => self.open_desktop_grid_overlay(cx),
            "mark-tile" => {
                // Begin a set-mark chord on the focused tile: the next char
                // typed assigns the mark (same as the bare `m{char}` chord).
                // The full-screen chord overlay (render path) captures it.
                self.pending_mark_chord = Some('m');
                self.transient_status = Some("mark tile: press a letter".into());
                cx.notify();
            }
            "list-marks" => {
                let marks = self.workspace.marks.all_marks();
                if marks.is_empty() {
                    self.transient_status = Some("no marks set".into());
                } else {
                    let list: String = marks
                        .iter()
                        .map(|(ch, id)| format!("'{ch}' → window {id}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.transient_status = Some(list.into());
                }
                cx.notify();
            }
            // Tags
            "tag-add" => self.open_tag_input(TagInputMode::Tag, cx),
            "tag-remove" => self.open_tag_input(TagInputMode::Untag, cx),
            "tag-view" => self.open_tag_input(TagInputMode::ViewTag, cx),
            "tag-also" => self.open_tag_input(TagInputMode::AlsoTag, cx),
            "tag-send" => self.open_tag_input(TagInputMode::SendTag, cx),
            "tag-bind" => self.open_tag_input(TagInputMode::TagBind, cx),
            // New-tile commands (global `n` submenu): create a NEW tile
            // (vertical split of the focused one) instead of replacing
            // the focused tile's content.
            // B3: new-buffer-tile splits a NEW tile holding App::Buffer(Picking)
            // with no restore target (a standalone picker). Cancelling it closes
            // the tile (B4), subject to the sole-tile floor.
            "new-buffer-tile" => {
                let cwd = self.active_workspace_cwd().unwrap_or_else(process_cwd);
                let _ = self.workspace.split_focused(
                    workspace::SplitDir::V,
                    App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
                );
                self.workspace.retile_active();
                self.save_workspace_state();
                cx.notify();
            }
            "new-agent-tile" => {
                // UXI-Workspace-8: contextual placement. In a BARE AGENT VIEW (an
                // ephemeral virtual workspace) there is nothing to split beside —
                // swap that single tile in place for a fresh picker instead.
                if self.workspace.active_is_ephemeral() {
                    self.open_new_agent_selector_in_place(cx);
                    return;
                }
                // Split with a placeholder browser (focus lands on the new
                // tile), then let `open_agent_inner` swap the focused tile
                // for the agent ring — reusing all the session machinery.
                let cwd = self.active_workspace_cwd().unwrap_or_else(process_cwd);
                if self
                    .workspace
                    .split_focused(
                        workspace::SplitDir::V,
                        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
                    )
                    .is_some()
                {
                    self.workspace.retile_active();
                    self.open_agent_inner(cx);
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "new-linear-tile" => {
                // Split a new tile (focus lands on it), then swap it for a
                // Linear viewport via `open_linear_inner` — mirrors new-agent-tile.
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if self
                    .workspace
                    .split_focused(
                        workspace::SplitDir::V,
                        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
                    )
                    .is_some()
                {
                    self.workspace.retile_active();
                    self.open_linear_inner(cx);
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "new-keymap-tile" => {
                // Split a new tile, then swap it for the keybindings sheet —
                // mirrors new-linear-tile.
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if self
                    .workspace
                    .split_focused(
                        workspace::SplitDir::V,
                        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
                    )
                    .is_some()
                {
                    self.workspace.retile_active();
                    self.open_keymap_inner(cx);
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            // Open-in-place commands (global `o` submenu): replace the
            // focused tile's content instead of creating a new split.
            // B3: inplace-buffer-pick resets the FOCUSED Buffer to Picking with
            // the prior mode as restore (B4) — identical to Cmd+O (B2). Inert on
            // an Agent tile. Routes through `open_browser_inner` so the stash and
            // the inert-on-Agent behavior live in exactly one place.
            "inplace-buffer-pick" => {
                self.open_browser_inner(cx);
            }
            "inplace-agent-tile" => {
                self.open_agent_inner(cx);
                self.save_workspace_state();
                cx.notify();
            }
            // Local menus (spec-menu-scopes.md)
            "doc-goto-top" => {
                if let Some(d) = self.doc_mut() {
                    d.cursor_block = 0;
                    d.reveal_block(0);
                    cx.notify();
                }
            }
            "doc-goto-bottom" => {
                if let Some(d) = self.doc_mut()
                    && !d.blocks.is_empty()
                {
                    d.cursor_block = d.blocks.len() - 1;
                    d.reveal_block(d.cursor_block);
                    cx.notify();
                }
            }
            "wp-toggle" => self.toggle_edit_view(cx),
            "nav-headings" | "goto-heading" => self.doc_jump_next_matching(
                "heading",
                |b| matches!(b, RenderedBlock::Heading { .. }),
                cx,
            ),
            "nav-list-items" => {
                self.doc_jump_next_matching("list", |b| matches!(b, RenderedBlock::List { .. }), cx)
            }
            "nav-code-blocks" => self.doc_jump_next_matching(
                "code block",
                |b| matches!(b, RenderedBlock::CodeBlock { .. }),
                cx,
            ),
            "nav-links" => self.doc_jump_next_matching("link", block_contains_link, cx),
            "claude-send-selection" => {
                if matches!(self.workspace.focused_content(), Some(App::Agent(_))) {
                    self.send_agent_selection(cx);
                }
            }
            "agent-focus-toggle" => {
                if matches!(self.workspace.focused_content(), Some(App::Agent(_))) {
                    self.toggle_agent_focus(cx);
                }
            }
            "browser-open-workspace" => {
                let sel = self
                    .browser_mut()
                    .and_then(|b| b.fb.selected_entry().map(|e| (e.path.clone(), e.is_dir)));
                match sel {
                    Some((path, false)) => {
                        if let Some(content) = self.make_doc_content(&path) {
                            let project = self.workspace.inherited_project();
                            self.workspace.push_initial_workspace(content, project);
                            self.save_workspace_state();
                            cx.notify();
                        }
                    }
                    Some((_, true)) => {
                        self.transient_status = Some("select a file, not a directory".into());
                        cx.notify();
                    }
                    None => {}
                }
            }
            "browser-open-split" => {
                let sel = self
                    .browser_mut()
                    .and_then(|b| b.fb.selected_entry().map(|e| (e.path.clone(), e.is_dir)));
                match sel {
                    Some((path, false)) => {
                        if let Some(content) = self.make_doc_content(&path) {
                            let _ = self
                                .workspace
                                .split_focused(workspace::SplitDir::V, content);
                            self.workspace.retile_active();
                            self.save_workspace_state();
                            cx.notify();
                        }
                    }
                    Some((_, true)) => {
                        self.transient_status = Some("select a file, not a directory".into());
                        cx.notify();
                    }
                    None => {}
                }
            }
            "browser-sort" => {
                if let Some(b) = self.browser_mut() {
                    b.fb.cycle_sort();
                    cx.notify();
                }
            }
            "browser-hidden" => {
                if let Some(b) = self.browser_mut() {
                    b.fb.toggle_hidden();
                    cx.notify();
                }
            }
            "browser-up" => {
                if let Some(b) = self.browser_mut() {
                    if b.fb.worktree_mode.is_some() {
                        return; // no-op in worktree mode
                    }
                    b.fb.go_parent();
                    cx.notify();
                }
            }
            "quit" | "force-quit" => cx.quit(),
            // Global menu: jump to workspace N (the index is encoded in the name).
            name if name.starts_with("goto-workspace-") => {
                if let Ok(idx) = name["goto-workspace-".len()..].parse::<usize>() {
                    self.select_workspace(idx, cx);
                }
            }
            _ => {
                // Unknown command — keep the menu closed but no-op so the
                // user gets visual feedback that the entry "did something".
                // Future: surface a status hint somewhere.
            }
        }
    }

    // ---- Buffer switcher ---------------------------------------------------

    fn open_buffer_switcher(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_buffer() || self.workspace.workspaces.is_empty() {
            return;
        }
        self.open_overlay(ActiveOverlay::BufferSwitcher(BufferSwitcher {
            selected: self.workspace.active_workspace,
            filter_mode: false,
            filter_text: String::new(),
        }));
        cx.notify();
    }

    fn close_buffer_switcher(&mut self) {
        self.clear_overlay();
    }

    // ---- Workspace picker (move / also-show tile) -------------------------

    /// Count how many distinct **workspaces** show a view of `label` (the
    /// file path backing a Doc/Edit tile). File path is the canonical buffer-
    /// pool key (`Frame::canonical_key`), so counting by path is the exact
    /// equivalent of counting by `FileBufferId` for pooled Edit tiles — and it
    /// additionally captures Doc tiles, which render disk snapshots and aren't
    /// pooled. Hence path, not id, is the right unifying membership key
    /// (spec-workspaces-tagging.md C-derived).
    ///
    /// Counts a workspace once even if it holds several views of the same
    /// file, so the multi-home dot means "also lives in another desktop",
    /// not "appears N times here".
    fn workspaces_showing_file(&self, label: &str) -> usize {
        self.workspace
            .workspaces
            .iter()
            .filter(|wsp| {
                let mut found = false;
                wsp.layout.for_each_leaf(&mut |w| {
                    if let Some(l) = screen_file_label(&w.content)
                        && l.as_ref() == label
                    {
                        found = true;
                    }
                });
                found
            })
            .count()
    }

    /// A small accent dot element when `label` is shown in more than one
    /// workspace (multi-home indicator), else an empty placeholder.
    fn multi_home_dot(&self, label: &str) -> gpui::Div {
        if self.workspaces_showing_file(label) > 1 {
            let accent: Hsla = nc(self.theme.overlay.accent);
            div()
                .ml_2()
                .text_color(accent)
                .child(SharedString::new_static("\u{25cf}"))
        } else {
            div()
        }
    }

    /// True when the focused tile is file-backed (Doc or Edit) and so can be
    /// "also-shown" in another workspace (Agent/Browser are single-home).
    fn focused_is_file_backed(&self) -> bool {
        matches!(
            self.workspace.focused_content(),
            Some(App::Buffer(BufferApp::Viewing(_))) | Some(App::Buffer(BufferApp::Editing(_)))
        )
    }

    /// Open the workspace picker overlay. For `AlsoShow`, reject non-file
    /// tiles up front with a footer message (the picker never opens).
    fn open_workspace_picker(&mut self, mode: WorkspacePickerMode, cx: &mut Context<Self>) {
        if self.overlay_is_workspace() {
            return;
        }
        // A fresh picker attempt clears any prior toast.
        self.transient_status = None;
        if self.workspace.focused_window_id().is_none() {
            return;
        }
        if mode == WorkspacePickerMode::AlsoShow && !self.focused_is_file_backed() {
            self.transient_status =
                Some("Only documents can be shown in multiple workspaces (yet)".into());
            cx.notify();
            return;
        }
        // Pre-select the first workspace that isn't the active one (you can't
        // move/also-show into the workspace the tile already lives in); fall
        // back to the "+ new workspace" entry when there's only one.
        let active = self.workspace.active_workspace;
        let selected = (0..self.workspace.workspaces.len())
            .find(|&i| i != active)
            .unwrap_or(self.workspace.workspaces.len());
        self.open_overlay(ActiveOverlay::WorkspacePicker(WorkspacePicker {
            mode,
            selected,
        }));
        cx.notify();
    }

    fn close_workspace_picker(&mut self) {
        self.clear_overlay();
    }

    /// Number of selectable entries in the picker: every workspace plus the
    /// trailing "+ new workspace" entry.
    fn workspace_picker_entry_count(&self) -> usize {
        self.workspace.workspaces.len() + 1
    }

    fn handle_workspace_picker_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let count = self.workspace_picker_entry_count();
        let selected = match self.workspace_picker_ref() {
            Some(p) => p.selected,
            None => return,
        };
        match press.key {
            Key::Esc | Key::Char('q') => {
                self.close_workspace_picker();
            }
            Key::Char('j') | Key::Down => {
                if let Some(p) = self.workspace_picker_mut()
                    && count > 0
                {
                    p.selected = (p.selected + 1) % count;
                }
            }
            Key::Char('k') | Key::Up => {
                if let Some(p) = self.workspace_picker_mut()
                    && count > 0
                {
                    p.selected = if p.selected == 0 {
                        count - 1
                    } else {
                        p.selected - 1
                    };
                }
            }
            Key::Char('g') => {
                if let Some(p) = self.workspace_picker_mut() {
                    p.selected = 0;
                }
            }
            Key::Char('G') => {
                if let Some(p) = self.workspace_picker_mut()
                    && count > 0
                {
                    p.selected = count - 1;
                }
            }
            Key::Enter | Key::Char('l') => {
                self.commit_workspace_picker(selected, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Apply the picker selection. `entry` is the chosen index into the entry
    /// list (`workspaces.len()` means "+ new workspace").
    fn commit_workspace_picker(&mut self, entry: usize, cx: &mut Context<Self>) {
        let mode = match self.workspace_picker_ref() {
            Some(p) => p.mode,
            None => return,
        };
        let n_workspaces = self.workspace.workspaces.len();
        let active = self.workspace.active_workspace;

        // Resolve the target workspace index, creating a new workspace if "+ new"
        // was chosen. A new workspace starts Empty; the relocated/also-shown
        // leaf becomes its first tile.
        let make_new = entry >= n_workspaces;
        let target = if make_new {
            self.push_empty_workspace();
            self.workspace.workspaces.len() - 1
        } else {
            entry
        };

        // Selecting the active workspace is a no-op (the tile is already here).
        if !make_new && target == active {
            self.close_workspace_picker();
            cx.notify();
            return;
        }

        match mode {
            WorkspacePickerMode::Move => {
                self.move_focused_to_workspace(target);
            }
            WorkspacePickerMode::AlsoShow => {
                self.also_show_focused_in_workspace(target);
            }
        }
        self.close_workspace_picker();
        self.save_workspace_state();
        cx.notify();
    }

    /// Append a new empty workspace with an auto-name and an
    /// `Empty` layout. Does NOT change the active workspace — the caller picks
    /// what to do next (relocate a leaf into it, etc.).
    fn push_empty_workspace(&mut self) {
        let name = workspace::auto_workspace_name(self.workspace.next_workspace_index);
        self.workspace.next_workspace_index += 1;
        // A new workspace inherits the current one's project (ADR-0028 §3).
        let project = self.workspace.inherited_project();
        self.workspace.workspaces.push(workspace::Workspace::with_layout(
            name,
            workspace::Layout::Empty,
            0,
            project,
        ));
    }

    /// MOVE: relocate the focused leaf out of the active workspace into
    /// `target`. If the source workspace is left empty, remove it (unless it's
    /// the only workspace, which we leave empty). Focus follows the tile to
    /// the target workspace.
    fn move_focused_to_workspace(&mut self, target: usize) {
        let (window, source_empty) = match self.workspace.detach_focused() {
            Ok(v) => v,
            Err(()) => return,
        };
        // `detach_focused` may shift nothing, but if it removed the active
        // workspace's only tile the target index could still be valid (target was
        // resolved before detach and detach never removes workspaces). Insert first,
        // then prune the empty source so indices stay stable during insert.
        let _ = self.workspace.insert_leaf_into_workspace(target, window);

        let source = self.workspace.active_workspace;
        if source_empty {
            if self.workspace.workspaces.len() > 1 {
                // Removing the source shifts indices; recompute the target's
                // position so we can land focus there.
                let target_after = if target > source { target - 1 } else { target };
                self.workspace.close_workspace(source);
                self.workspace.active_workspace = target_after.min(self.workspace.workspaces.len() - 1);
            } else {
                // Only workspace: leave it empty and stay on it (matches the
                // existing single-workspace close behavior — we don't quit here).
                self.workspace.active_workspace = target.min(self.workspace.workspaces.len() - 1);
            }
        } else {
            // Source still has tiles; follow the moved tile to the target.
            self.workspace.active_workspace = target.min(self.workspace.workspaces.len() - 1);
        }
    }

    /// ALSO-SHOW: open a second view onto the focused file-backed tile's file
    /// in `target`, leaving the original in place. The new view reads the
    /// file from disk (independent cursor/scroll), mirroring how splits clone
    /// a file tile today. Switches to the target workspace so the user sees
    /// the new view.
    fn also_show_focused_in_workspace(&mut self, target: usize) {
        if !self.focused_is_file_backed() {
            self.transient_status =
                Some("Only documents can be shown in multiple workspaces (yet)".into());
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let content = self.clone_focused_for_split(&cwd);
        let id = self.workspace.alloc_window_id();
        let window = workspace::Window { id, content };
        let _ = self.workspace.insert_leaf_into_workspace(target, window);
        self.workspace.active_workspace = target.min(self.workspace.workspaces.len() - 1);
    }

    // ---- Session switcher overlay -----------------------------------------

    /// Open the free-session switcher / rebind flow on the focused agent tile
    /// (spec-agent-session-ownership.md "free sessions + rebind"). Lists the
    /// FREE sessions (no tile binds them) plus a "new session" row; Enter
    /// rebinds this tile to the chosen free session (freeing, not killing, its
    /// previous one).
    fn open_session_picker_rebind(&mut self, cx: &mut Context<Self>) {
        // Must be on an agent tile. (If not, open one first.)
        if self.agent_tile().is_none() {
            self.open_agent_inner(cx);
            if self.agent_tile().is_none() {
                return;
            }
        }
        // The free-session listing lists from a cwd: use the current session's,
        // Free the current session (kept running in the store) and land the tile
        // in the live in-tile selector — the same UI an unbound agent tile shows
        // (free sessions + "start new"). No bespoke overlay.
        self.release_focused_session_for_rebind();
        self.show_selector_on_focused_tile(cx);
        cx.notify();
    }

    fn render_workspace_picker(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let picker = match self.workspace_picker_ref() {
            Some(p) => p,
            None => unreachable!(),
        };

        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let label_fg: Hsla = nc(ov.label);
        let active_fg: Hsla = nc(ov.accent);
        let selected_bg: Hsla = nc(ov.selected_bg);
        let normal_fg: Hsla = nc(ov.fg);
        let popup_border: Hsla = nc(ov.border);

        let verb = match picker.mode {
            WorkspacePickerMode::Move => "MOVE TILE TO WORKSPACE",
            WorkspacePickerMode::AlsoShow => "ALSO-SHOW TILE IN WORKSPACE",
        };
        let header_row = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::from(verb.to_string()));

        let mut entries_col = div()
            .flex()
            .flex_col()
            .px_4()
            .py_2()
            .text_size(px(14.0))
            .font_family(self.code_font.clone());

        let active = self.workspace.active_workspace;
        let n_workspaces = self.workspace.workspaces.len();
        for (i, wsp) in self.workspace.workspaces.iter().enumerate() {
            let is_selected = i == picker.selected;
            let is_active = i == active;
            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let here = if is_active { " (here)" } else { "" };
            let label_text = format!("{}{}", workspace_strip_label(wsp), here);
            let name_color = if is_active { label_fg } else { normal_fg };

            let mut row = div().flex().flex_row().items_center().px_2().py_0p5();
            if is_selected {
                row = row.bg(selected_bg);
            }
            row = row
                .child(
                    div()
                        .text_color(label_fg)
                        .child(SharedString::from(marker.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(name_color)
                        .child(SharedString::from(label_text)),
                );
            entries_col = entries_col.child(row);
        }

        // "+ new workspace" entry.
        {
            let is_selected = picker.selected == n_workspaces;
            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let mut row = div().flex().flex_row().items_center().px_2().py_0p5();
            if is_selected {
                row = row.bg(selected_bg);
            }
            row = row
                .child(
                    div()
                        .text_color(label_fg)
                        .child(SharedString::from(marker.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(active_fg)
                        .child(SharedString::new_static("+ new workspace")),
                );
            entries_col = entries_col.child(row);
        }

        let hints_row = div()
            .px_4()
            .py_1()
            .text_size(px(11.0))
            .text_color(label_fg)
            .child("j/k move · enter select · q/esc cancel");

        div()
            .id("workspace-picker")
            .absolute()
            .top(px(34.0))
            .left(px(40.0))
            .right(px(40.0))
            .max_h(px(400.0))
            .bg(menu_bg)
            .border_1()
            .border_color(popup_border)
            .rounded_md()
            .shadow_lg()
            .overflow_y_scroll()
            .child(header_row)
            .child(entries_col)
            .child(hints_row)
    }

    // ---- Session rename overlay -------------------------------------------

    /// Open the rename input overlay for the active claude session. No-op
    /// if claude isn't focused (the command is gated by the menu but a
    /// stray dispatch shouldn't crash) or if an overlay is already open.
    /// `Ctrl-W p` action shim → [`open_desktop_grid_overlay`].
    fn desktop_tile_size_overlay(
        &mut self,
        _: &DesktopTileSize,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_desktop_grid_overlay(cx);
    }

    /// `Ctrl-W p` / menu `l g`: open the `{cols}x{rows}` input for the
    /// desktop GRID — how many tiles fit the viewport per axis; tile size
    /// derives from it (spec-desktop-mode.md Behavior 6, grid revision).
    /// Pre-filled with the current value so Enter is a no-op confirm.
    fn open_desktop_grid_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let text = format!("{}x{}", self.desktop_grid_cols, self.desktop_grid_rows);
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::DesktopTileSize,
        }));
        cx.notify();
    }

    fn open_rename_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        // No bound session (the picker) ⇒ nothing to rename.
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let Some(ent) = self.sessions.get(id) else {
            return;
        };
        let text = ent.read(cx).label.clone();
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::AgentSession { id },
        }));
        cx.notify();
    }

    // ---- Project lifecycle (UXI-Project-4 / -5) ----------------------------

    /// Open the project context menu (UXI-JumpPanel-8) anchored at the click
    /// position `pos` (window space). Offers the project-scoped actions (New
    /// workspace / New agent session / Delete project). Nudged a hair down-right
    /// so the cursor doesn't land pre-hovered on the first item, and clamped to
    /// the viewport (flipping above the anchor when near the bottom edge). No-op
    /// if any overlay is already open.
    pub(crate) fn open_project_menu(
        &mut self,
        pid: ProjectId,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if self.has_overlay() {
            return;
        }
        const MENU_W: f32 = 200.0;
        const MENU_H: f32 = 118.0;
        let (vw, vh) = (self.viewport_width_px, self.viewport_height_px);
        let mut x = pos.0 + 2.0;
        let mut y = pos.1 + 4.0;
        if vw > 0.0 && x + MENU_W > vw {
            x = (vw - MENU_W).max(0.0);
        }
        if vh > 0.0 && y + MENU_H > vh {
            y = (pos.1 - MENU_H).max(0.0);
        }
        self.open_overlay(ActiveOverlay::ProjectMenu { pid, x, y });
        cx.notify();
    }

    /// A project context-menu item was chosen (UXI-JumpPanel-8): dismiss the menu
    /// FIRST (so the per-action `has_overlay()` guards pass), then run the action
    /// scoped to `pid`.
    fn project_menu_action(&mut self, pid: ProjectId, action: ProjectMenuAction, cx: &mut Context<Self>) {
        self.clear_overlay();
        match action {
            ProjectMenuAction::NewWorkspace => self.new_workspace_in(pid, cx),
            ProjectMenuAction::NewAgentSession => self.new_agent_session_in(pid, cx),
            ProjectMenuAction::DeleteProject => self.request_delete_project(pid, cx),
        }
    }

    /// Key dispatch while the project context menu is open (UXI-JumpPanel-8): Esc
    /// closes; single-key accelerators fire the items (`w`/`a`/`d`).
    fn handle_project_menu_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((pid, _, _)) = self.project_menu_ref() else {
            return;
        };
        let press = keystroke_to_keypress(&ev.keystroke);
        match press.key {
            Key::Esc => {
                self.clear_overlay();
                cx.notify();
            }
            Key::Char('w') => self.project_menu_action(pid, ProjectMenuAction::NewWorkspace, cx),
            Key::Char('a') => self.project_menu_action(pid, ProjectMenuAction::NewAgentSession, cx),
            Key::Char('d') => self.project_menu_action(pid, ProjectMenuAction::DeleteProject, cx),
            _ => {}
        }
    }

    /// Open the `? p` "New project" overlay (UXI-Project-4). It asks for one
    /// thing—the cwd—and derives the project name from that directory.
    pub(crate) fn open_new_project_overlay(&mut self, cx: &mut Context<Self>) {
        if self.has_overlay() {
            return;
        }
        let cwd = self.agent_base_cwd().display().to_string();
        self.open_overlay(ActiveOverlay::NewProject(NewProjectOverlay { cwd }));
        cx.notify();
    }

    /// Commit the cwd-only project overlay: resolve the directory, derive and
    /// uniquify its basename display name, create an EMPTY project, and persist.
    fn commit_new_project_overlay(&mut self, cx: &mut Context<Self>) {
        let cwd = match self.new_project_ref() {
            Some(o) => o.cwd.trim().to_string(),
            None => return,
        };
        if cwd.is_empty() {
            self.clear_overlay();
            cx.notify();
            return;
        }
        match resolve_agent_cwd_arg(&cwd) {
            Ok(resolved) => {
                if self.projects.by_cwd(&resolved).is_some() {
                    self.clear_overlay();
                    self.transient_status =
                        Some("another project already roots that directory".into());
                } else {
                    let hint = project_name_for_cwd(&resolved);
                    let pid = self.projects.ensure_at_cwd(resolved, &hint);
                    let name = self.projects.name_of(pid).to_string();
                    save_persisted_projects(&self.projects);
                    self.clear_overlay();
                    self.transient_status = Some(format!("project {name} created").into());
                }
            }
            Err(msg) => {
                self.clear_overlay();
                self.transient_status = Some(msg.into());
            }
        }
        cx.notify();
    }

    fn handle_new_project_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        match press.key {
            Key::Esc => {
                self.clear_overlay();
                cx.notify();
            }
            Key::Enter => self.commit_new_project_overlay(cx),
            Key::Backspace => {
                if let Some(o) = self.new_project_mut() {
                    o.cwd.pop();
                }
                cx.notify();
            }
            Key::Char(c) => {
                if let Some(o) = self.new_project_mut() {
                    o.cwd.push(c);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Create a new workspace belonging to `pid`, rooted at its cwd (UXI-Project-4:
    /// the per-project ＋ New workspace row). No cwd prompt — the cwd is the
    /// project's. Becomes the active workspace.
    pub(crate) fn new_workspace_in(&mut self, pid: ProjectId, cx: &mut Context<Self>) {
        let Some(cwd) = self.projects.cwd_of(pid).map(|p| p.to_path_buf()) else {
            return;
        };
        self.workspace.push_initial_workspace(
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
            pid,
        );
        self.save_workspace_state();
        cx.notify();
    }

    /// Create a new FREE agent session rooted at `pid`'s cwd (UXI-Project-4: the
    /// per-project ＋ New agent session row). No cwd prompt; it lands unbound in
    /// the roster under this project's section.
    pub(crate) fn new_agent_session_in(&mut self, pid: ProjectId, cx: &mut Context<Self>) {
        let Some(cwd) = self.projects.cwd_of(pid).map(|p| p.to_path_buf()) else {
            return;
        };
        self.spawn_free_agent_session_at(cwd, cx);
    }

    /// Request deletion of `pid` (UXI-Project-5). If it still holds workspaces or
    /// sessions (live or roster-only), arm a confirmation overlay; an EMPTY
    /// project deletes directly. No-op if any overlay is already open.
    pub(crate) fn request_delete_project(&mut self, pid: ProjectId, cx: &mut Context<Self>) {
        if self.has_overlay() {
            return;
        }
        let mut nonempty = self.workspace.workspaces.iter().any(|t| t.project() == pid);
        if !nonempty {
            for (_, ent) in self.sessions.iter() {
                if self.projects.by_cwd(&ent.read(cx).cwd) == Some(pid) {
                    nonempty = true;
                    break;
                }
            }
        }
        if !nonempty {
            for info in self.agent_roster.entries_by_label() {
                if self.projects.by_cwd(&info.cwd) == Some(pid) {
                    nonempty = true;
                    break;
                }
            }
        }
        if nonempty {
            self.open_overlay(ActiveOverlay::ConfirmProjectDelete(pid));
            cx.notify();
        } else {
            self.perform_delete_project(pid, cx);
        }
    }

    /// Cascade-delete `pid` (UXI-Project-5): kill every session rooted in it
    /// (local + roster-only), close its workspaces, then drop the project +
    /// persist. Never leaves zero workspaces (spec Behavior 2) — a placeholder is
    /// seeded under a surviving project. Empty projects are otherwise NOT
    /// auto-deleted (only this explicit path removes a project).
    pub(crate) fn perform_delete_project(&mut self, pid: ProjectId, cx: &mut Context<Self>) {
        // 1. Sessions: local sessions rooted here, and roster-only sids rooted
        // here (not already represented locally).
        let mut local_kill: Vec<SessionId> = Vec::new();
        let mut local_sids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (id, ent) in self.sessions.iter() {
            if let Some(sid) = self.sessions.sid_of(id) {
                local_sids.insert(sid.as_str().to_string());
            }
            if self.projects.by_cwd(&ent.read(cx).cwd) == Some(pid) {
                local_kill.push(id);
            }
        }
        let mut roster_kill: Vec<String> = Vec::new();
        for info in self.agent_roster.entries_by_label() {
            if self.projects.by_cwd(&info.cwd) == Some(pid)
                && !local_sids.contains(&info.session_id)
            {
                roster_kill.push(info.session_id.clone());
            }
        }
        for id in local_kill {
            if let Some(sid) = self.sessions.sid_of(id).map(|s| s.to_string()) {
                self.spawn_close_session(sid, cx);
            }
            self.transcript_views.remove(&id);
            self.sessions.close(id);
        }
        for sid in roster_kill {
            self.agent_roster.remove(&sid);
            self.spawn_close_session(sid, cx);
        }
        // 2. Workspaces: close this project's workspaces (descending so indices stay
        // valid), then guarantee ≥1 workspace survives under a surviving project.
        let mut idxs: Vec<usize> = self
            .workspace
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, t)| t.project() == pid)
            .map(|(i, _)| i)
            .collect();
        idxs.sort_unstable_by(|a, b| b.cmp(a));
        for i in idxs {
            self.workspace.close_workspace(i);
        }
        // 3. Drop the project FIRST, so the survivor is derived from what REMAINS
        // (deleting the last project must not seed a workspace under the id we're
        // about to remove — bug caught in review).
        self.projects.close(pid);
        // Guarantee ≥1 workspace AND ≥1 project survive. If that was the last
        // project, mint a fresh default rooted at the process dir — the "never
        // zero projects" twin of "never zero workspaces", so the app can never
        // enter a projectless / orphaned-workspace state.
        if self.workspace.workspaces.is_empty() {
            let survivor = self.projects.first().unwrap_or_else(|| {
                let cwd = process_cwd();
                self.projects.ensure_at_cwd(cwd.clone(), &project_name_for_cwd(&cwd))
            });
            let name = workspace::auto_workspace_name(self.workspace.next_workspace_index);
            self.workspace.next_workspace_index += 1;
            self.workspace.workspaces.push(workspace::Workspace::with_layout(
                name,
                workspace::Layout::Empty,
                0,
                survivor,
            ));
            self.workspace.active_workspace = 0;
        }
        save_persisted_projects(&self.projects);
        self.save_workspace_state();
        self.clear_overlay();
        // A focused agent tile whose session we just killed falls back to its
        // live selector (never a dangling bound id).
        let dangling = match self.workspace.focused_content() {
            Some(App::Agent(tile)) => tile.session().filter(|id| !self.sessions.contains(*id)),
            _ => None,
        };
        if dangling.is_some() {
            self.show_selector_on_focused_tile(cx);
        }
        cx.notify();
    }

    fn handle_confirm_delete_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let Some(pid) = self.confirm_delete_ref() else {
            return;
        };
        match press.key {
            Key::Char('y') | Key::Enter => self.perform_delete_project(pid, cx),
            _ => {
                self.clear_overlay();
                cx.notify();
            }
        }
    }

    /// Open a path-input overlay pre-filled with the active slot's
    /// current cwd; on commit, respawn the slot at the new path
    /// (spec-agent-cwd.md §4).
    fn open_change_agent_cwd_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let Some(ent) = self.sessions.get(id) else {
            return;
        };
        let text = ent.read(cx).cwd.display().to_string();
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::AgentChangeCwd { id },
        }));
        cx.notify();
    }

    /// Open a path-input overlay that sets the active workspace's registry
    /// `"cwd"` (untitled.md "Set CWD"). Pre-fills with the current workspace
    /// cwd if set, otherwise the process cwd, so Enter confirms a sensible
    /// default. On commit the path is resolved + validated (same rules as
    /// `:claude-new <path>`) and written to the workspace's kv; new agent sessions
    /// in this workspace inherit it.
    fn open_set_workspace_cwd_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let idx = self.workspace.active_workspace;
        let Some(wsp) = self.workspace.workspaces.get(idx) else {
            return;
        };
        let text = self
            .projects
            .cwd_of(wsp.project())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::WorkspaceCwd { index: idx },
        }));
        cx.notify();
    }

    /// Open the rename overlay targeting the active workspace workspace. The
    /// input pre-fills with the workspace's current display label (display_name
    /// if set, else auto_name).
    fn open_rename_active_workspace_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let idx = self.workspace.active_workspace;
        let Some(wsp) = self.workspace.workspaces.get(idx) else {
            return;
        };
        let text = wsp.display_label().to_string();
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::Workspace { index: idx },
        }));
        cx.notify();
    }

    fn close_rename_overlay(&mut self) {
        self.clear_overlay();
    }

    /// Apply the overlay's text to the targeted slot/wsp, then close.
    /// Trims whitespace; an all-whitespace input cancels (acts like Esc) so
    /// the user can't accidentally erase the label by hammering Enter.
    fn commit_rename_overlay(&mut self, cx: &mut Context<Self>) {
        let (target, new_label) = match self.rename_ref() {
            Some(o) => (o.target, o.text.trim().to_string()),
            None => return,
        };
        if new_label.is_empty() {
            self.close_rename_overlay();
            cx.notify();
            return;
        }
        match target {
            RenameTarget::AgentSession { id } => {
                // Update the label in the store and, if this session is managed
                // by the session server, push the new label there so it
                // persists across GUI restarts.
                if let Some(ent) = self.session_entity(id) {
                    ent.update(cx, |session, scx| {
                        session.label = new_label.clone();
                        // UXI-AgentTile-27 property 3: an explicit rename latches
                        // the origin to `User`, permanently. Autonaming can never
                        // fire afterwards, and an autoname already in flight is
                        // dropped when it lands (`finish_autoname`) rather than
                        // overwriting the name just typed.
                        session.state.name_origin = NameOrigin::User;
                        scx.notify();
                    });
                }
                let server_sid = self.sessions.sid_of(id).map(|s| s.to_string());
                if let (Some(server), Some(sid)) = (&self.session_server, server_sid) {
                    let _ = server.rename_session(&sid, &new_label);
                }
                self.close_rename_overlay();
                self.save_agent_ring(cx);
                cx.notify();
            }
            RenameTarget::Workspace { index } => {
                if let Some(wsp) = self.workspace.workspaces.get_mut(index) {
                    wsp.display_name = Some(new_label);
                }
                self.close_rename_overlay();
                self.save_workspace_state();
                cx.notify();
            }
            RenameTarget::AgentChangeCwd { id } => match resolve_agent_cwd_arg(&new_label) {
                Ok(resolved) => {
                    self.close_rename_overlay();
                    self.change_agent_cwd(id, resolved, cx);
                }
                Err(msg) => {
                    self.close_rename_overlay();
                    if let Some(mut c) = self.agent_mut(cx) {
                        c.status = Some(msg.into());
                    }
                    cx.notify();
                }
            },
            RenameTarget::WorkspaceCwd { index } => match resolve_agent_cwd_arg(&new_label) {
                Ok(resolved) => {
                    self.close_rename_overlay();
                    let path = resolved.display().to_string();
                    // "Set workspace cwd" now repoints the workspace's PROJECT cwd
                    // (ADR-0028 §3 — cwd lives on the project). Refused if another
                    // project already roots there.
                    let outcome = self
                        .workspace
                        .workspaces
                        .get(index)
                        .map(|t| t.project())
                        .map(|pid| self.projects.set_cwd(pid, resolved));
                    match outcome {
                        Some(Ok(())) => {
                            save_persisted_projects(&self.projects);
                            self.transient_status = Some(format!("project cwd → {path}").into());
                        }
                        Some(Err(_)) => {
                            self.transient_status =
                                Some("another project already roots that directory".into());
                        }
                        None => {}
                    }
                    self.save_workspace_state();
                    cx.notify();
                }
                Err(msg) => {
                    self.close_rename_overlay();
                    self.transient_status = Some(msg.into());
                    cx.notify();
                }
            },
            RenameTarget::DesktopTileSize => {
                self.close_rename_overlay();
                // Accept "120x40" / "120X40" with optional spaces. Slot
                // addresses are size-independent (spec Behavior 6), so this
                // re-renders in place — no migration, no slot mutation.
                let parsed = new_label.to_lowercase().split_once('x').and_then(|(c, r)| {
                    Some((c.trim().parse::<u32>().ok()?, r.trim().parse::<u32>().ok()?))
                });
                match parsed {
                    Some((cols, rows)) => {
                        self.desktop_grid_cols = cols.clamp(1, 12);
                        self.desktop_grid_rows = rows.clamp(1, 12);
                        self.transient_status = Some(
                            format!(
                                "desktop grid: {}x{} tiles per screen",
                                self.desktop_grid_cols, self.desktop_grid_rows
                            )
                            .into(),
                        );
                        self.save_settings();
                    }
                    None => {
                        self.transient_status = Some("desktop grid: expected {cols}x{rows}".into());
                    }
                }
                cx.notify();
            }
        }
    }

    fn handle_rename_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let press = keystroke_to_keypress(&ev.keystroke);
        match press.key {
            Key::Esc => {
                self.close_rename_overlay();
                cx.notify();
            }
            Key::Enter => self.commit_rename_overlay(cx),
            Key::Backspace => {
                if let Some(o) = self.rename_mut() {
                    o.text.pop();
                }
                cx.notify();
            }
            Key::Char(c) => {
                if let Some(o) = self.rename_mut() {
                    o.text.push(c);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn handle_tag_input_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let press = keystroke_to_keypress(&ev.keystroke);
        match press.key {
            Key::Esc => {
                self.active_overlay = ActiveOverlay::None;
                cx.notify();
            }
            Key::Enter => self.commit_tag_input(cx),
            Key::Backspace => {
                if let Some(o) = self.tag_input_mut() {
                    o.text.pop();
                }
                cx.notify();
            }
            Key::Char(c) => {
                if let Some(o) = self.tag_input_mut() {
                    o.text.push(c);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn commit_tag_input(&mut self, cx: &mut Context<Self>) {
        let (mode, text) = match self.tag_input_ref() {
            Some(o) => (o.mode, o.text.clone()),
            None => return,
        };
        let tag = text.trim().to_string();
        if tag.is_empty() && mode != TagInputMode::ViewTag {
            self.active_overlay = ActiveOverlay::None;
            cx.notify();
            return;
        }
        match mode {
            TagInputMode::Tag => {
                if self.tag_focused(tag.clone()) {
                    self.transient_status = Some(format!("tagged '{tag}'").into());
                } else {
                    self.transient_status = Some("cannot tag this tile".into());
                }
            }
            TagInputMode::Untag => {
                if self.untag_focused(&tag) {
                    self.transient_status = Some(format!("untagged '{tag}'").into());
                } else {
                    self.transient_status = Some(format!("tag '{tag}' not found").into());
                }
            }
            TagInputMode::ViewTag => {
                if let Some(wsp) = self.workspace.active_workspace_mut() {
                    if tag.is_empty() {
                        wsp.tag_view.clear();
                        self.transient_status = Some("tag filter cleared".into());
                    } else {
                        wsp.tag_view.clear();
                        wsp.tag_view.insert(tag.clone());
                        self.transient_status = Some(format!("viewing tag '{tag}'").into());
                    }
                }
                self.adjust_focus_for_tag_view();
            }
            TagInputMode::SendTag => {
                self.tag_focused(tag.clone());
                if let Some(wsp) = self.workspace.active_workspace_mut() {
                    wsp.tag_view.clear();
                    wsp.tag_view.insert(tag.clone());
                }
                self.adjust_focus_for_tag_view();
                self.transient_status = Some(format!("tagged + viewing '{tag}'").into());
            }
            TagInputMode::AlsoTag => {
                if self.tag_focused(tag.clone()) {
                    self.transient_status = Some(format!("also-tagged '{tag}'").into());
                } else {
                    self.transient_status = Some("cannot tag this tile".into());
                }
            }
            TagInputMode::TagBind => {
                // Expect "x tagname" format
                let parts: Vec<&str> = tag.splitn(2, ' ').collect();
                if parts.len() == 2 && parts[0].len() == 1 {
                    let key = parts[0].chars().next().unwrap();
                    let name = parts[1].to_string();
                    self.workspace.tag_shortcuts.insert(key, name.clone());
                    self.transient_status = Some(format!("bound '{key}' → tag '{name}'").into());
                } else {
                    self.transient_status = Some("usage: <key> <tag-name>".into());
                }
            }
        }
        self.active_overlay = ActiveOverlay::None;
        self.save_workspace_state();
        cx.notify();
    }

    fn render_tag_input_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let o = self.tag_input_ref().unwrap();
        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let label_fg: Hsla = nc(ov.label);
        let input_fg: Hsla = nc(ov.input);

        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::new_static(o.prompt));

        let input_row = div()
            .px_4()
            .py_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(self.code_font.clone())
            .child(SharedString::from(format!("{}\u{2588}", o.text)));

        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static("enter:submit  esc:cancel"));

        div()
            .absolute()
            .top(px(80.0))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .w(px(360.0))
                    .bg(menu_bg)
                    .border_2()
                    .border_color(popup_border)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(input_row)
                    .child(footer),
            )
    }

    fn open_tag_input(&mut self, mode: TagInputMode, cx: &mut Context<Self>) {
        let prompt = match mode {
            TagInputMode::Tag => "TAG BUFFER",
            TagInputMode::Untag => "UNTAG BUFFER",
            TagInputMode::ViewTag => "VIEW TAG (empty = clear)",
            TagInputMode::SendTag => "TAG + VIEW",
            TagInputMode::AlsoTag => "ALSO TAG",
            TagInputMode::TagBind => "BIND: <key> <tag>",
        };
        self.active_overlay = ActiveOverlay::TagInput(TagInputOverlay {
            mode,
            text: String::new(),
            prompt,
        });
        cx.notify();
    }

    /// Return the indices of buffers matching the current filter query.
    fn filtered_buffer_indices(&self) -> Vec<usize> {
        let bs = match self.buffer_ref() {
            Some(bs) => bs,
            None => return (0..self.workspace.workspaces.len()).collect(),
        };
        if bs.filter_text.is_empty() {
            return (0..self.workspace.workspaces.len()).collect();
        }
        let query = bs.filter_text.to_lowercase();
        (0..self.workspace.workspaces.len())
            .filter(|&i| {
                let label = workspace_doc_label(&self.workspace.workspaces[i])
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                fuzzy_match_gpui(&label, &query)
            })
            .collect()
    }

    fn handle_buffer_switcher_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        let (filter_mode, selected) = match self.buffer_ref() {
            Some(bs) => (bs.filter_mode, bs.selected),
            None => return,
        };

        if filter_mode {
            match press.key {
                Key::Esc => {
                    self.close_buffer_switcher();
                    cx.notify();
                    return;
                }
                Key::Enter => {
                    let filtered = self.filtered_buffer_indices();
                    if filtered.len() == 1 {
                        let idx = filtered[0];
                        self.close_buffer_switcher();
                        self.switch_to_buffer(idx);
                    } else if !filtered.is_empty()
                        && let Some(bs) = self.buffer_mut()
                    {
                        bs.filter_mode = false;
                    }
                    cx.notify();
                    return;
                }
                Key::Backspace => {
                    if let Some(bs) = self.buffer_mut() {
                        bs.filter_text.pop();
                        bs.selected = 0;
                    }
                }
                Key::Char(c) => {
                    if let Some(bs) = self.buffer_mut() {
                        bs.filter_text.push(c);
                        bs.selected = 0;
                    }
                }
                _ => {}
            }
            cx.notify();
            return;
        }

        match press.key {
            Key::Esc | Key::Char('q') => {
                self.close_buffer_switcher();
            }
            Key::Char('j') | Key::Down => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = self.buffer_mut()
                    && count > 0
                {
                    bs.selected = (bs.selected + 1) % count;
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = self.buffer_mut()
                    && count > 0
                {
                    bs.selected = if bs.selected == 0 {
                        count - 1
                    } else {
                        bs.selected - 1
                    };
                }
            }
            Key::Char('g') => {
                if let Some(bs) = self.buffer_mut() {
                    bs.selected = 0;
                }
            }
            Key::Char('G') => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = self.buffer_mut()
                    && count > 0
                {
                    bs.selected = count - 1;
                }
            }
            Key::Enter | Key::Char('l') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(selected) {
                    self.close_buffer_switcher();
                    self.switch_to_buffer(buf_idx);
                }
            }
            Key::Char('d') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(selected) {
                    self.close_buffer_at(buf_idx, cx);
                    let count = self.filtered_buffer_indices().len();
                    if let Some(bs) = self.buffer_mut()
                        && bs.selected >= count
                        && count > 0
                    {
                        bs.selected = count - 1;
                    }
                }
            }
            Key::Char('/') => {
                if let Some(bs) = self.buffer_mut() {
                    bs.filter_mode = true;
                    bs.filter_text.clear();
                    bs.selected = 0;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The command panel — "The Sigil Card" (UXI-Menu-1..4). A floating,
    /// content-sized card in the workspace region (right of the jump panel),
    /// horizontally centered and pinned `MENU_PANEL_TOP` below the top chrome —
    /// NOT the old full-width drop-down bar. Each leader wears a scope hue on a 2px
    /// left accent bar + a header sigil; the header breadcrumb is the literal
    /// keystroke trail you typed (UXI-Menu-3). Keyboard-driven (no click-away): the
    /// `MenuView` capture handler in `render` owns dispatch + Esc.
    fn render_menu_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let m = match self.menu_ref() {
            Some(m) => m,
            None => unreachable!(),
        };

        let ov = &self.theme.overlay;
        // Elevated card bg: lighter than tiles/workspace AND the recessed jump bar,
        // derived from the live editor bg so the separation holds on every theme.
        let menu_bg: Hsla = menu_panel_bg(self.editor_bg());
        let label_fg: Hsla = nc(ov.label);
        let key_fg: Hsla = nc(ov.key);
        let label_text_fg: Hsla = nc(ov.fg);
        let submenu_fg: Hsla = nc(ov.accent);
        let popup_border: Hsla = nc(ov.border);
        let mono = self.code_font.clone();

        // Scope identity (UXI-Menu-4): the leader that opened this menu picks the
        // accent hue, the header sigil, the display scope name, and the trail's
        // leading glyph. Three leaders, three colors, glanceable before reading.
        let scope_hue: Hsla = match m.leader {
            ' ' => nc(self.theme.agent.frozen_bar),
            '.' => key_fg,
            '?' => nc(self.theme.agent.jump_header),
            _ => key_fg,
        };
        let sigil = match m.header {
            "AGENT" => "✦",
            "DOC" | "EDIT" => "▣",
            "BROWSE" => "▤",
            "LINEAR" => "◈",
            "KEYBINDINGS" => "⌘",
            "MENU" => "⊞",   // workspace (`.`)
            "GLOBAL" => "◉", // global (`?`)
            _ => "▸",
        };
        let scope_name = match m.header {
            "MENU" => "WORKSPACE",
            other => other,
        };
        let leader_glyph = match m.leader {
            ' ' => "␣".to_string(),
            other => other.to_string(),
        };

        // ---- Keystroke trail (UXI-Menu-3) ----
        // The literal chord to reach this level, as key chips: leader glyph, then
        // each descended submenu key, then the current level's name in accent.
        let (crumbs, level_label) =
            menu_trail_crumbs(&m.menu, &m.state.path, &leader_glyph, scope_name);
        let mut trail_bg = label_fg;
        trail_bg.a = 0.12;
        let trail_chip = move |text: String| {
            div()
                .flex_none()
                .px(px(5.0))
                .h(px(16.0))
                .rounded(px(4.0))
                .bg(trail_bg)
                .flex()
                .items_center()
                .font_family(mono.clone())
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(label_fg)
                .child(text)
        };
        let mut trail = div().flex().flex_row().items_center().gap(px(5.0));
        for crumb in &crumbs {
            trail = trail.child(trail_chip(crumb.clone()));
        }
        trail = trail
            .child(div().text_color(label_fg).text_size(px(11.0)).child("›"))
            .child(
                div()
                    .font_family(self.code_font.clone())
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(scope_hue)
                    .child(level_label.to_uppercase()),
            );

        // esc hint chip, far right — replaces the old repeated footer line.
        let mut esc_bg = label_fg;
        esc_bg.a = 0.10;
        let esc_chip = div()
            .flex_none()
            .px(px(6.0))
            .h(px(16.0))
            .rounded(px(4.0))
            .bg(esc_bg)
            .flex()
            .items_center()
            .font_family(self.code_font.clone())
            .text_size(px(10.0))
            .text_color(label_fg)
            .child(SharedString::new_static("esc"));

        let header_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .h(px(30.0))
            .border_b_1()
            .border_color(popup_border)
            .child(
                div()
                    .flex_none()
                    .text_color(scope_hue)
                    .text_size(px(14.0))
                    .child(sigil),
            )
            .child(trail)
            .child(div().flex_1())
            .child(esc_chip);

        // ---- Multi-column layout ----
        //
        // Partition the level into sections (separator-delimited groups,
        // each usually starting with a Label), then distribute whole
        // sections across 1–3 columns so a large menu fits the card without
        // scrolling. Sections never split mid-group.
        let nodes = m.state.current_nodes(&m.menu);
        let mut sections: Vec<Vec<&MenuNode>> = vec![Vec::new()];
        for node in nodes {
            if node.kind() == MenuNodeKind::Separator {
                if !sections.last().map(Vec::is_empty).unwrap_or(true) {
                    sections.push(Vec::new());
                }
            } else {
                sections.last_mut().unwrap().push(node);
            }
        }
        if sections.last().map(Vec::is_empty).unwrap_or(false) {
            sections.pop();
        }
        let total_rows: usize = sections.iter().map(Vec::len).sum();
        let n_cols = if total_rows <= 10 {
            1
        } else if total_rows <= 20 {
            2
        } else {
            3
        };
        let target_rows = total_rows.div_ceil(n_cols);
        let mut columns: Vec<Vec<Vec<&MenuNode>>> = vec![Vec::new()];
        let mut col_rows = 0usize;
        for sec in sections {
            if col_rows >= target_rows && columns.len() < n_cols {
                columns.push(Vec::new());
                col_rows = 0;
            }
            col_rows += sec.len();
            columns.last_mut().unwrap().push(sec);
        }

        let body_font = self.body_font.clone();
        let chip_mono = self.code_font.clone();
        let disabled = &m.disabled;
        let render_node = move |node: &MenuNode| -> AnyElement {
            match node.kind() {
                MenuNodeKind::Separator => unreachable!("separators delimit sections"),
                // Section heading: a `Label` node reads as an uppercase mono
                // caption (replacing the old bold-inline label + divider rule).
                MenuNodeKind::Label => div()
                    .pt(px(10.0))
                    .pb(px(3.0))
                    .px(px(14.0))
                    .font_family(chip_mono.clone())
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(label_fg)
                    .child(node.label.to_uppercase())
                    .into_any_element(),
                MenuNodeKind::Command | MenuNodeKind::Submenu => {
                    let key_display = format_menu_key(&node.key);
                    let is_submenu = node.kind() == MenuNodeKind::Submenu;
                    // Behavior 10: disabled entries render dimmed and don't dispatch.
                    let is_disabled = matches!(&node.action,
                        MenuAction::Command(name) if disabled.contains(name));
                    let label_color = if is_disabled {
                        label_fg
                    } else if is_submenu {
                        submenu_fg
                    } else {
                        label_text_fg
                    };
                    let chip_fg = if is_disabled { label_fg } else { key_fg };
                    let mut chip_bg = key_fg;
                    chip_bg.a = if is_disabled { 0.04 } else { 0.10 };
                    let mut row = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .h(px(26.0))
                        .px(px(14.0))
                        .child(
                            // Key chip: right-aligned in a fixed gutter so labels
                            // share a left edge and multi-char keys grow leftward.
                            div()
                                .flex_none()
                                .min_w(px(34.0))
                                .px(px(6.0))
                                .h(px(18.0))
                                .rounded(px(4.0))
                                .bg(chip_bg)
                                .flex()
                                .items_center()
                                .justify_end()
                                .font_family(chip_mono.clone())
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(chip_fg)
                                .child(key_display),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(label_color)
                                .child(node.label.clone()),
                        );
                    if is_submenu {
                        row = row.child(
                            div()
                                .flex_none()
                                .text_color(submenu_fg)
                                .child(SharedString::new_static("▸")),
                        );
                    }
                    row.into_any_element()
                }
            }
        };

        let mut entries_col = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(24.0))
            .py(px(6.0))
            .pr(px(10.0))
            .text_size(px(13.0))
            .font_family(body_font)
            .font_weight(FontWeight::MEDIUM);
        for col_sections in columns {
            let mut col_div = div().flex().flex_col().min_w(px(196.0));
            let mut first = true;
            for sec in col_sections {
                if !first {
                    // Inter-section whitespace inside a column (labels + gaps group
                    // better than a rule inside a small card).
                    col_div = col_div.child(div().h(px(8.0)));
                }
                first = false;
                for node in sec {
                    col_div = col_div.child(render_node(node));
                }
            }
            entries_col = entries_col.child(col_div);
        }

        // The card: accent bar + body column, content-sized within the width band.
        let accent_bar = div().w(px(2.0)).flex_none().bg(scope_hue);
        let body_col = div()
            .flex()
            .flex_col()
            .flex_1()
            .child(header_row)
            .child(probe_bounds("menu-entries", entries_col.into_any_element()));
        let card = div()
            .flex()
            .flex_row()
            .min_w(px(MENU_PANEL_MIN_W))
            .max_w(px(MENU_PANEL_MAX_W))
            .bg(menu_bg)
            .border_1()
            .border_color(popup_border)
            .rounded(px(8.0))
            .shadow_lg()
            .overflow_hidden()
            .child(accent_bar)
            .child(body_col);

        // Float it: cover the window, left-anchor the card just past the jump panel
        // (about where the first workspace tile renders) with a small gutter, pin its
        // top (UXI-Menu-1). No scrim (these live for ~800ms of muscle memory; a
        // per-chord dim would flash).
        let left = if self.jump_panel_visible {
            JUMP_PANEL_WIDTH + MENU_PANEL_LEFT_PAD
        } else {
            MENU_PANEL_LEFT_PAD
        };
        let wrap = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_row()
            .items_start()
            .justify_start()
            .pt(px(MENU_PANEL_TOP))
            .pl(px(left));
        probe_bounds(
            "menu-overlay-root",
            wrap.child(probe_bounds("menu-panel", card.into_any_element()))
                .into_any_element(),
        )
    }

    /// Render the buffer-list picker as a full-window overlay, mirroring the
    /// TUI's `draw_full_buffer_list`.
    fn render_buffer_switcher(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let bs = match self.buffer_ref() {
            Some(bs) => bs,
            None => unreachable!(),
        };

        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let label_fg: Hsla = nc(ov.label);
        let active_fg: Hsla = nc(ov.accent);
        let selected_bg: Hsla = nc(ov.selected_bg);
        let normal_fg: Hsla = nc(ov.fg);
        let modified_fg: Hsla = nc(ov.modified);
        let popup_border: Hsla = nc(ov.border);
        let filter_fg: Hsla = nc(ov.input);

        let filtered = self.filtered_buffer_indices();
        let total = self.workspace.workspaces.len();
        let visible = filtered.len();

        // Header
        let header_text = if bs.filter_text.is_empty() {
            format!("BUFFERS ({})", total)
        } else {
            format!("BUFFERS ({}/{}) — \"{}\"", visible, total, bs.filter_text)
        };
        let header_row = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(header_text);

        // Buffer entries
        let mut entries_col = div()
            .flex()
            .flex_col()
            .px_4()
            .py_2()
            .text_size(px(14.0))
            .font_family(self.code_font.clone());

        for (vis_idx, &buf_idx) in filtered.iter().enumerate() {
            let wsp = &self.workspace.workspaces[buf_idx];
            let is_selected = vis_idx == bs.selected;
            let is_active = buf_idx == self.workspace.active_workspace;
            let is_modified = match &wsp.layout {
                workspace::Layout::Leaf(w) => screen_is_modified(&w.content),
                _ => false,
            };

            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let active_dot = if is_active { "\u{25cf} " } else { "  " };
            let modified_mark = if is_modified { " [+]" } else { "" };

            // Shorten the path for display
            let label_owned = workspace_doc_label(wsp).unwrap_or_else(|| wsp.display_label().to_string());
            let display_path = shorten_path(&label_owned);

            let name_color = if is_active { active_fg } else { normal_fg };

            let mut row = div().flex().flex_row().items_center().px_2().py_0p5();

            if is_selected {
                row = row.bg(selected_bg);
            }

            row = row
                .child(
                    div()
                        .text_color(label_fg)
                        .child(SharedString::from(marker.to_string())),
                )
                .child(
                    div()
                        .text_color(active_fg)
                        .child(SharedString::from(active_dot.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(name_color)
                        .child(SharedString::from(display_path)),
                );

            if is_modified {
                row = row.child(
                    div()
                        .text_color(modified_fg)
                        .child(SharedString::from(modified_mark.to_string())),
                );
            }

            entries_col = entries_col.child(row);
        }

        // Filter input (shown only in filter mode)
        let filter_row = if bs.filter_mode {
            div()
                .px_4()
                .py_1()
                .h(px(24.0))
                .text_color(filter_fg)
                .text_size(px(14.0))
                .font_family(self.code_font.clone())
                .child(SharedString::from(format!("/ {}\u{2588}", bs.filter_text)))
        } else {
            div()
        };

        // Hint bar
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(22.0))
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static(
                "enter/l:switch  d:close  /:filter  g/G:top/bottom  q:close",
            ));

        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .bg(menu_bg)
            .border_b_2()
            .border_color(popup_border)
            .flex()
            .flex_col()
            .child(header_row)
            .child(entries_col)
            .child(filter_row)
            .child(footer)
    }

    /// Render the centered single-line input box for renaming a session.
    /// Visual style follows the buffer-switcher's filter row (yellow text
    /// on the popup background) but in a small centered modal rather than
    /// a full-screen tile. Pre-filled with the current label; trailing
    /// block char serves as a cursor.
    fn render_rename_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let o = match self.rename_ref() {
            Some(o) => o,
            None => unreachable!(),
        };
        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let label_fg: Hsla = nc(ov.label);
        let input_fg: Hsla = nc(ov.input);

        let header_label = match o.target {
            RenameTarget::AgentSession { .. } => "RENAME SESSION",
            RenameTarget::Workspace { .. } => "RENAME WORKSPACE",
            RenameTarget::AgentChangeCwd { .. } => "CHANGE SESSION CWD",
            RenameTarget::WorkspaceCwd { .. } => "SET WORKSPACE CWD",
            RenameTarget::DesktopTileSize => "DESKTOP GRID (COLSxROWS OF TILES)",
        };
        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::new_static(header_label));

        let input_row = div()
            .px_4()
            .py_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(self.code_font.clone())
            .child(SharedString::from(format!("{}\u{2588}", o.text)));

        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static("enter:save  esc:cancel"));

        // Centered modal: absolutely positioned, fixed width, top inset
        // to keep it out of the header strip.
        div()
            .absolute()
            .top(px(80.0))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .w(px(360.0))
                    .bg(menu_bg)
                    .border_2()
                    .border_color(popup_border)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(input_row)
                    .child(footer),
            )
    }

    /// The `? p` "New project" overlay (UXI-Project-4): one cwd input. The
    /// project name is derived from the final directory component.
    fn render_new_project_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let o = match self.new_project_ref() {
            Some(o) => o,
            None => unreachable!(),
        };
        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let label_fg: Hsla = nc(ov.label);
        let input_fg: Hsla = nc(ov.input);
        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::new_static("NEW PROJECT"));
        let cwd_row = div()
            .px_4()
            .pt_2()
            .pb_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(self.code_font.clone())
            .child(SharedString::from(format!(
                "cwd: {}{}",
                o.cwd,
                "\u{2588}"
            )));
        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static(
                "name comes from directory  enter:create  esc:cancel",
            ));

        div()
            .absolute()
            .top(px(80.0))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .w(px(420.0))
                    .bg(menu_bg)
                    .border_2()
                    .border_color(popup_border)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(cwd_row)
                    .child(footer),
            )
    }

    /// The "Delete project?" confirmation (UXI-Project-5): names the project and
    /// how many workspaces/sessions the cascade will close, `y`/enter confirms.
    fn render_confirm_delete_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pid = self.confirm_delete_ref().expect("confirm overlay open");
        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let label_fg: Hsla = nc(ov.label);
        let input_fg: Hsla = nc(ov.input);
        let name = self.projects.name_of(pid).to_string();
        let n_ws = self.workspace.workspaces.iter().filter(|t| t.project() == pid).count();
        let mut n_sess = 0usize;
        for (_, ent) in self.sessions.iter() {
            if self.projects.by_cwd(&ent.read(cx).cwd) == Some(pid) {
                n_sess += 1;
            }
        }

        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::from(format!("DELETE PROJECT {name}?")));
        let body = div()
            .px_4()
            .py_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(self.code_font.clone())
            .child(SharedString::from(format!(
                "closes {n_ws} workspace(s), kills {n_sess} session(s)"
            )));
        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static("y / enter: confirm    esc: cancel"));

        div()
            .absolute()
            .top(px(80.0))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .w(px(420.0))
                    .bg(menu_bg)
                    .border_2()
                    .border_color(popup_border)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body)
                    .child(footer),
            )
    }

    /// The project context menu (UXI-JumpPanel-8): a small popup anchored at the
    /// stored click point, offering the project-scoped actions. Rendered as two
    /// siblings — a full-window transparent click-away backdrop UNDER a
    /// positioned popup (a click on the popup hits it, not the backdrop; a click
    /// anywhere else hits the backdrop and dismisses). Item glyphs match the panel
    /// row icons (`⊞`/`✦`) so the menu teaches the icon vocabulary.
    fn render_project_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (pid, x, y) = self.project_menu_ref().expect("project menu open");
        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let popup_border: Hsla = nc(ov.border);
        let item_fg: Hsla = nc(ov.fg);
        let dim: Hsla = nc(self.theme.agent.dim);
        // One interaction hue across the whole panel + menu: the cyan selection
        // tint at low alpha, as an inset hover pill.
        let mut hover_bg: Hsla = nc(self.theme.agent.frozen_bar);
        hover_bg.a = 0.15;
        let err: Hsla = nc(self.theme.agent.jump_header);
        let mono = self.code_font.clone();

        let item = |id: &str,
                    glyph: &str,
                    glyph_color: Hsla,
                    label: &str,
                    label_color: Hsla,
                    action: ProjectMenuAction| {
            div()
                .id(SharedString::from(id.to_string()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .mx(px(4.0))
                .px_3()
                .py(px(6.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .font_family(mono.clone())
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(hover_bg))
                .child(
                    div()
                        .w(px(16.0))
                        .flex_none()
                        .text_color(glyph_color)
                        .child(SharedString::from(glyph.to_string())),
                )
                .child(div().flex_1().text_color(label_color).child(SharedString::from(label.to_string())))
                .on_click(cx.listener(move |this, _ev, _w, cx| {
                    this.project_menu_action(pid, action, cx)
                }))
        };
        // Tag each item with its painted bounds so the harness can click the REAL
        // rect (bug-0019) instead of a computed guess.
        let item = |id: &str,
                    glyph: &str,
                    glyph_color: Hsla,
                    label: &str,
                    label_color: Hsla,
                    action: ProjectMenuAction| {
            probe_bounds_dyn(
                id.to_string(),
                item(id, glyph, glyph_color, label, label_color, action).into_any_element(),
            )
        };

        let popup = div()
            .absolute()
            // MUST occlude (bug-0019). GPUI's hit test collects EVERY hitbox under
            // the pointer (front-to-back, `Frame::hit_test`) and only stops at a
            // `BlockMouse` one — so without this the full-window backdrop below is
            // ALSO hovered: pressing an item fired the backdrop's `on_mouse_down`,
            // dismissing the menu on mouse DOWN, and `on_click` (down-then-up on the
            // same element) never fired. Occluding blocks the backdrop behind the
            // popup while leaving clicks elsewhere to dismiss as before.
            .occlude()
            .left(px(x))
            .top(px(y))
            .min_w(px(184.0))
            .bg(menu_bg)
            .border_1()
            .border_color(popup_border)
            .rounded(px(6.0))
            .shadow_md()
            .py(px(4.0))
            .flex()
            .flex_col()
            .child(item("proj-menu-new-ws", "⊞", dim, "New workspace", item_fg, ProjectMenuAction::NewWorkspace))
            .child(item(
                "proj-menu-new-agent",
                "✦",
                dim,
                "New agent session",
                item_fg,
                ProjectMenuAction::NewAgentSession,
            ))
            .child(div().mx(px(4.0)).my(px(4.0)).h(px(1.0)).bg(popup_border))
            .child(item("proj-menu-delete", "✕", err, "Delete project", err, ProjectMenuAction::DeleteProject));

        // Full-window transparent backdrop (click-away). Sibling BEFORE the popup
        // so the popup paints on top; a click on the popup hits the popup, a click
        // elsewhere hits the backdrop and dismisses.
        let backdrop = div()
            .id("proj-menu-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.clear_overlay();
                    cx.notify();
                }),
            );

        div().absolute().inset_0().child(backdrop).child(popup)
    }

    /// Best-effort copy via macOS `pbcopy`. Failures are silent — yank is
    /// a convenience, and we don't want to surface system errors per keystroke.
    /// (TUI uses the same approach.)
    fn yank_to_clipboard(text: &str) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(text.as_bytes());
        }
    }

    /// Best-effort read via macOS `pbpaste`. Returns `None` on failure.
    fn read_from_clipboard() -> Option<String> {
        use std::process::Command;
        let output = Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            String::from_utf8(output.stdout).ok()
        } else {
            None
        }
    }

    // ---- Claude (ACP) screen ----------------------------------------------
}

impl YaldaGpuiView {
    fn render_splash(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let bg = self.editor_bg();
        let fg = self.editor_fg();

        div()
            .track_focus(&self.focus_handle)
            .key_context("SplashView")
            .size_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                // Ignore bare modifier presses (shift, ctrl, etc.) — only
                // dismiss on a real key.
                let modifier_only = ev.keystroke.key.is_empty()
                    || matches!(
                        ev.keystroke.key.as_str(),
                        "shift" | "control" | "alt" | "meta" | "fn"
                    );
                if modifier_only {
                    return;
                }
                this.splash_until = None;
                cx.notify();
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.splash_until = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .font_family(self.body_font.clone())
                            .text_size(px(48.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child("yalda"),
                    )
                    .child(
                        div()
                            .font_family(self.body_font.clone())
                            .text_size(px(14.0))
                            .text_color(Hsla {
                                h: 0.0,
                                s: 0.0,
                                l: 0.5,
                                a: 1.0,
                            })
                            .child("a markdown editor"),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for YaldaGpuiView {
    fn focus_handle(&self, _cx: &GpuiApp) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for YaldaGpuiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport_width_px = f32::from(_window.viewport_size().width);
        self.viewport_height_px = f32::from(_window.viewport_size().height);

        // Auto-clear expired splash.
        if let Some(deadline) = self.splash_until
            && std::time::Instant::now() >= deadline
        {
            self.splash_until = None;
        }
        if self.splash_until.is_some() {
            return self.render_splash(cx);
        }

        // 5c: re-derive each Doc tile's blocks from its shared core when the
        // rope has advanced (e.g. an Edit tile of the same file took a
        // keystroke). `refresh_blocks` is O(1) per Doc when the core is
        // unchanged and read-only on the core, so this is cheap and panic-safe.
        {
            let theme = &self.theme;
            for wsp in self.workspace.workspaces.iter_mut() {
                wsp.layout.for_each_leaf_content_mut(&mut |content| {
                    if let App::Buffer(BufferApp::Viewing(d)) = content {
                        d.refresh_blocks(theme);
                    }
                });
            }
        }

        // Behavior 9 (spec-menu-scopes.md): if the focused window changed
        // while a menu was open, dismiss it — stale entries must not
        // dispatch against the wrong content.
        if let ActiveOverlay::Menu(m) = &self.active_overlay
            && self.workspace.focused_window_id() != Some(m.opened_from)
        {
            self.clear_overlay();
        }

        let has_overlay = self.has_overlay();

        // Build the screen content. When an overlay is OPEN, focus moves up
        // to the wrapper so the screen's `YaldaView`/`BrowserView` action
        // bindings don't match (they would otherwise fire BEFORE our key
        // listener — for example, `k` in Doc context is bound to
        // `ScrollUp` and `k` in Browser context is bound to `BrowserUp`,
        // both of which intercept the keystroke before any `on_key_down`
        // handler runs and stop propagation as the default action behavior).
        // When no overlay is open, the focused leaf inside `render_layout`
        // attaches `track_focus(&self.focus_handle)` — that way the focus
        // handle sits INSIDE the YaldaView/EditView/etc. key context, so
        // context-scoped bindings actually match on dispatch. (Putting the
        // focus on an outer wrapper means bubble-up from focus skips the
        // context-bearing leaf, and Space → OpenMenu silently no-ops in
        // multi-leaf splits.)
        let editor_bg = self.editor_bg();
        let editor_fg = self.editor_fg();
        let screen_root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(editor_bg)
            .text_color(editor_fg);

        // When the rail holds focus, the content leaf must NOT attach the
        // shared focus handle — otherwise both the rail and the leaf would
        // claim it, and the leaf's context-scoped bindings (YaldaView, …)
        // would shadow the rail's (spec §6, two-state model §5).
        let rail_focused = !has_overlay && self.rail_is_focused();
        let leaf_attach_focus = !has_overlay && !rail_focused;
        // The rail is now injected beside the focused leaf *inside*
        // `render_focused_window` (so it's local to the focused tile, not the
        // whole window). It's focusable only when no overlay owns focus (§4).
        let screen_view: AnyElement =
            self.render_focused_window(screen_root, leaf_attach_focus, !has_overlay, cx);

        // (The side workspace/workspace strip was removed — workspaces are switched
        // from the `?` global menu now.)

        // Tag bar: thin strip above content showing tag labels when any
        // buffers have tags. Active-view tags get accent background.
        let screen_view = self.wrap_with_tag_bar(screen_view);

        // Overlay a one-shot transient status toast (e.g. an also-show
        // rejection for a non-file tile) in the bottom-right.
        let screen_view = if let Some(msg) = self.transient_status.clone() {
            let ov = &self.theme.overlay;
            let toast_bg: Hsla = nc(ov.bg);
            let toast_fg: Hsla = nc(ov.fg);
            let toast_border: Hsla = nc(ov.border);
            div()
                .size_full()
                .relative()
                .child(screen_view)
                .child(
                    div()
                        .absolute()
                        .bottom(px(16.0))
                        .right(px(16.0))
                        .max_w(px(360.0))
                        .px_3()
                        .py_2()
                        .bg(toast_bg)
                        .text_color(toast_fg)
                        .text_size(px(12.0))
                        .border_1()
                        .border_color(toast_border)
                        .rounded_md()
                        .shadow_lg()
                        .child(msg),
                )
                .into_any_element()
        } else {
            screen_view
        };

        // Jump panel (jump-panel; spec-jump-panel.md): a permanent root-level
        // left sidebar laid out as a flex row BEFORE the chord/overlay returns,
        // so every render path keeps it visible across workspaces. Rendered
        // inline (it's cheap — see `render_jump_panel`) in a fixed-width cell;
        // the existing content takes the remaining width. The panel insets the
        // content area, so a surface beneath it re-measures ONCE as geometry
        // settles (a benign one-time bounds render, not a per-keystroke cost —
        // see the settle notes on the `*_is_render_flat` harness tests).
        let screen_view: AnyElement = if self.jump_panel_visible {
            let panel_el = self.render_jump_panel(cx);
            div()
                .flex()
                .flex_row()
                .size_full()
                .child(
                    div()
                        .w(px(JUMP_PANEL_WIDTH))
                        .h_full()
                        .flex_none()
                        .child(panel_el),
                )
                .child(div().flex_1().min_w_0().h_full().child(screen_view))
                .into_any_element()
        } else {
            screen_view
        };

        // When a mark chord or tag chord is pending, capture the next
        // keypress to complete the chord before any action dispatch can fire.
        if self.pending_mark_chord.is_some() || self.pending_tag_chord.is_some() {
            let chord_label = if self.pending_mark_chord == Some('m') {
                "m …"
            } else if self.pending_mark_chord == Some('\'') {
                "' …"
            } else {
                "tag …"
            };
            let editor_bg = self.editor_bg();
            return div()
                .track_focus(&self.focus_handle)
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                    let press = keystroke_to_keypress(&ev.keystroke);
                    if let Key::Char(ch) = press.key {
                        if this.pending_mark_chord.is_some() {
                            this.complete_mark_chord(ch, cx);
                        } else if this.pending_tag_chord.is_some() {
                            this.complete_tag_chord(ch, cx);
                        }
                    } else {
                        // Non-char key cancels the chord
                        this.pending_mark_chord = None;
                        this.pending_tag_chord = None;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .child(screen_view)
                // Show a small indicator of the pending chord
                .child(
                    div()
                        .absolute()
                        .bottom(px(4.0))
                        .right(px(4.0))
                        .px_2()
                        .py_1()
                        .bg(gpui::hsla(0.08, 0.9, 0.55, 1.0))
                        .text_color(gpui::hsla(0.0, 0.0, 0.0, 1.0))
                        .text_size(px(11.0))
                        .rounded_sm()
                        .child(SharedString::from(chord_label)),
                )
                .into_any_element();
        }

        if !has_overlay {
            return screen_view;
        }

        // Rename overlay takes priority — it's a transient single-line
        // input opened from the menu, so nothing else should steal keys.
        //
        // Overlay key dispatch uses CAPTURE phase + `stop_propagation` so
        // every keystroke is consumed by the overlay before action dispatch
        // can fire. Without that, global keybindings (Cmd-T, Cmd-=, …) and
        // any letters bound globally would leak through; in particular,
        // shifted chars could shadow distinct overlay entries (e.g. `w` vs
        // `W`). The capture handler short-circuits the entire rest of the
        // pipeline.
        // Project context menu (UXI-JumpPanel-8): a lightweight popup layered over
        // the (still visible) screen + jump panel — NOT an opaque body swap. A
        // transparent backdrop inside `render_project_menu` handles click-away;
        // capture_key_down handles Esc + the `w`/`a`/`d` accelerators.
        if self.overlay_is_project_menu() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("ProjectMenuView")
                .size_full()
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_project_menu_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_project_menu(cx))
                .into_any_element();
        }

        if self.overlay_is_new_project() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("NewProjectView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_new_project_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_new_project_overlay(cx))
                .into_any_element();
        }

        if self.overlay_is_confirm_delete() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("ConfirmDeleteView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_confirm_delete_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_confirm_delete_overlay(cx))
                .into_any_element();
        }

        if self.overlay_is_rename() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("RenameOverlayView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_rename_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_rename_overlay(cx))
                .into_any_element();
        }

        // Buffer switcher takes priority over menu.
        if self.overlay_is_buffer() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("BufferSwitcherView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_buffer_switcher_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_buffer_switcher(cx))
                .into_any_element();
        }

        // Workspace picker (move / also-show tile).
        if self.overlay_is_workspace() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("WorkspacePickerView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_workspace_picker_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_workspace_picker(cx))
                .into_any_element();
        }

        // Jump palette (UXI-JumpPanel-9). Layered over the still-visible screen
        // (no opaque body swap) so you keep your bearings while jumping; the
        // capture handler owns every key, so the Cmd-P chord can't re-enter and
        // no global binding leaks through.
        if self.overlay_is_jump_palette() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("JumpPaletteView")
                .size_full()
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_jump_palette_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_jump_palette(cx))
                .into_any_element();
        }

        // Tag input overlay.
        if self.overlay_is_tag_input() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("TagInputView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_tag_input_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_tag_input_overlay(cx))
                .into_any_element();
        }

        // Menu overlay.
        div()
            .track_focus(&self.focus_handle)
            .key_context("MenuView")
            .size_full()
            .bg(editor_bg)
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_menu_key(ev, w, cx);
                cx.stop_propagation();
            }))
            .child(screen_view)
            .child(self.render_menu_overlay(cx))
            .into_any_element()
    }
}

// Test-only counter of how many `block_element`s the virtualized doc list
// builds. The latency gate (verify_harness) asserts this stays O(visible) —
// a few dozen for a 3000-block doc — proving render is no longer O(document).
#[cfg(test)]
thread_local! {
    static DOC_BLOCK_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test-only render-decision tap (the substitute for pixel inspection — GPUI's
/// test platform discards the rendered scene). Records what the doc renderer
/// *decided* a frame: which `(block,line)`s were actually PAINTED (the
/// virtualized window), which got the selection background over which byte
/// range, and which block drew the left cursor bar. The harness asserts against
/// this to verify selection / cursor / virtualization deterministically,
/// without rasterizing. Reset + read via the `test_*_doc_render_tap` accessors.
#[cfg(test)]
#[derive(Default, Clone)]
pub(crate) struct DocRenderTap {
    /// `(block_idx, line_idx)` painted this frame — the visible/virtualized window.
    pub painted: Vec<(usize, usize)>,
    /// `(block_idx, line_idx, byte_start, byte_end)` for lines given SELECTION_BG.
    pub selection: Vec<(usize, usize, usize, usize)>,
    /// bug-0017: `(raw_line, char_start, char_end)` for parsed-BLOCK lines
    /// (transcript code blocks) painted with the selection background. Distinct
    /// from `selection` (doc-view, block-relative) because transcript blocks key
    /// selection by RAW document line via `RenderCtx::block_hits`.
    pub block_selection: Vec<(usize, usize, usize)>,
    /// The block that drew the left cursor bar, if any.
    pub cursor_bar_block: Option<usize>,
}

#[cfg(test)]
thread_local! {
    static DOC_RENDER_TAP: RefCell<DocRenderTap> = RefCell::new(DocRenderTap::default());
}

/// Concatenate a `StyledLine`'s span texts into a plain string. Used by the
/// outline rail to label headings without their styling.
fn styled_line_plain(line: &StyledLine) -> String {
    line.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Parse an ATX heading line (`#{1,6}\s+text`) into `(level, text)`. Returns
/// `None` for non-heading lines. Cheaper than a full markdown parse for the
/// Edit-view outline (spec §13).
fn atx_heading(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    // Require whitespace after the hashes (ATX requires a space).
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((hashes as u8, text))
}

/// Choose `scroll_offset` so `selected` sits inside `[scroll, scroll+rows)`.
/// Mirrors the TUI's behavior of keeping a short margin around the cursor.
fn scroll_to_keep_visible(selected: usize, rows: usize, total: usize) -> usize {
    if total <= rows {
        return 0;
    }
    // Keep the selected item centered when the list is long enough to scroll.
    let half = rows / 2;
    selected
        .saturating_sub(half)
        .min(total.saturating_sub(rows))
}

/// One row in the file-browser list.
fn browser_row(
    entry: &BrowserEntry,
    selected: bool,
    code_font: &SharedString,
    ov: &OverlayTheme,
) -> AnyElement {
    let row_bg = if selected {
        nc(ov.selected_bg)
    } else {
        nc(ov.bg)
    };
    let marker_color = nc(ov.key);
    let name_color = if entry.is_dir {
        nc(ov.accent)
    } else {
        nc(ov.fg)
    };
    let meta_color = nc(ov.label);

    let suffix = if entry.is_dir { "/" } else { "" };
    let name_text = format!("{}{}", entry.name, suffix);

    let size_str = match entry.size {
        Some(s) => format_file_size(s),
        None => "—".to_string(),
    };
    let mtime_str = match entry.modified {
        Some(t) => format_mtime(t),
        None => "—".to_string(),
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px_2()
        .py_0p5()
        .bg(row_bg)
        .child(
            div()
                .w(px(20.0))
                .flex_none()
                .text_color(marker_color)
                .child(SharedString::from(if selected { "▸ " } else { "  " })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_color(name_color)
                .child(SharedString::from(name_text)),
        )
        .child(
            div()
                .w(px(72.0))
                .flex_none()
                .text_color(meta_color)
                .font_family(code_font.clone())
                .text_size(px(11.0))
                .child(SharedString::from(format!("{:>8}", size_str))),
        )
        .child(
            div()
                .w(px(64.0))
                .flex_none()
                .text_color(meta_color)
                .font_family(code_font.clone())
                .text_size(px(11.0))
                .child(SharedString::from(format!("{:>7}", mtime_str))),
        )
        .into_any_element()
}

/// One row in the worktree-picker list.
fn worktree_row(wt: &worktree::Worktree, selected: bool, ov: &OverlayTheme) -> AnyElement {
    let row_bg = if selected {
        nc(ov.selected_bg)
    } else {
        nc(ov.bg)
    };
    let marker = if wt.is_current {
        "* "
    } else if selected {
        "▸ "
    } else {
        "  "
    };
    let path_str = wt.path.display().to_string();

    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px_2()
        .py_0p5()
        .bg(row_bg)
        .child(
            div()
                .w(px(20.0))
                .flex_none()
                .text_color(nc(ov.key))
                .child(SharedString::from(marker)),
        )
        .child(
            div()
                .min_w(px(100.0))
                .flex_none()
                .text_color(nc(ov.accent))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(wt.label.clone())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_color(nc(ov.label))
                .text_size(px(11.0))
                .child(SharedString::from(path_str)),
        )
        .into_any_element()
}

// ---- Date/size formatters (copied from TUI's view.rs) ----

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_mtime(time: std::time::SystemTime) -> String {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let (_year, month, day) = days_to_ymd(days);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = months.get(month as usize).unwrap_or(&"???");
    format!("{} {:2}", mon, day)
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m - 1, d)
}

// ----------------------------------------------------------------------------
// Buffer helpers
// ----------------------------------------------------------------------------

/// Short display label for the workspace strip. Doc/Edit workspaces show the file's
/// basename (`E ` prefix for Edit); Browser/Claude show their kind.
fn workspace_strip_label(wsp: &workspace::Workspace<App>) -> String {
    if let workspace::Layout::Leaf(w) = &wsp.layout {
        match &w.content {
            App::Buffer(BufferApp::Viewing(d)) => basename_or_full(d.file_label.as_ref()),
            App::Buffer(BufferApp::Editing(e)) => {
                format!("E {}", basename_or_full(e.file_label.as_ref()))
            }
            App::Buffer(BufferApp::Picking(_)) => format!("Browser ({})", wsp.display_label()),
            App::Agent(_) => format!("Claude ({})", wsp.display_label()),
            App::Linear(tile) => tile.title(),
            App::Keymap(_) => "Keybindings".to_string(),
        }
    } else {
        wsp.display_label().to_string()
    }
}

fn basename_or_full(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Extract the file label of a workspace's focused window, if Doc or Edit.
/// Returns `None` for Browser/Claude workspaces or non-leaf layouts.
fn workspace_doc_label(wsp: &workspace::Workspace<App>) -> Option<String> {
    if let workspace::Layout::Leaf(w) = &wsp.layout {
        match &w.content {
            App::Buffer(BufferApp::Viewing(d)) => Some(d.file_label.to_string()),
            App::Buffer(BufferApp::Editing(e)) => Some(e.file_label.to_string()),
            _ => None,
        }
    } else {
        None
    }
}

/// Extract the file label from a screen, if it's a Doc or Edit screen.
fn screen_file_label(screen: &App) -> Option<SharedString> {
    match screen {
        App::Buffer(BufferApp::Viewing(d)) => Some(d.file_label.clone()),
        App::Buffer(BufferApp::Editing(e)) => Some(e.file_label.clone()),
        _ => None,
    }
}

/// Check whether the screen's underlying editor has unsaved modifications.
fn screen_is_modified(screen: &App) -> bool {
    match screen {
        App::Buffer(BufferApp::Editing(e)) => e.editor.is_modified(),
        App::Buffer(BufferApp::Viewing(d)) => d.source.as_ref().is_some_and(|s| s.is_modified()),
        _ => false,
    }
}

/// Shorten an absolute path for display by replacing the home directory with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.display().to_string();
        if let Some(rest) = path.strip_prefix(&home_str) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Fuzzy-match `query` against `text` (same algorithm as the TUI).
fn fuzzy_match_gpui(text: &str, query: &str) -> bool {
    let mut text_chars = text.chars();
    for qc in query.chars() {
        loop {
            match text_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

// ----------------------------------------------------------------------------
// Main
// ----------------------------------------------------------------------------

/// Register every GPUI key binding for the app. Extracted from `main()`'s
/// run-closure so the headless verification harness can install the same
/// bindings on a test window — a window with no keymap can't dispatch actions,
/// so action-level smokes were impossible while this lived inline. The
/// production path calls this once from the run-closure; tests call it via
/// `cx.update(|cx| register_keymap(cx))`.
fn register_keymap(app: &mut GpuiApp) {
    // The keymap is now data-driven: every binding lives in the declarative
    // `DEFAULT_BINDINGS` table (`keymap_registry.rs`), and `apply` clears +
    // rebinds the whole set via `build_action` + `KeyBinding::load`. This is
    // the single source of truth the `App::Keymap` reference tile reads and
    // rebinds, so the displayed keys are always the live ones. User overrides
    // (from that tile) are folded in by `KeymapRegistry::load`.
    KeymapRegistry::load().apply(app);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Reap ACP adapters orphaned by a previously crashed/killed yalda (parent
    // reparented to PID 1). Graceful exits reap via kill_on_drop; this catches
    // the crash/SIGKILL path that accumulated ~70 idle adapters over weeks.
    let _ = yalda::acp_channel::reap_orphaned_adapters();
    // Load `.env` (gitignored) BEFORE anything reads the environment, so
    // `ANTHROPIC_API_KEY` for session autonaming (UXI-AgentTile-27) can live in
    // the repo root instead of the launching shell. Real env vars always win.
    load_dotenv();
    // Relocate state written by older builds under <cache_dir>/yalda into the
    // durable `~/.yalda` home (ADR-0018), BEFORE any persisted state (prefs,
    // workspace, client_id, acp_sessions) is read. One-time, idempotent.
    yalda::paths::migrate_legacy_cache_dir();
    let config = yalda::config::Config::load().unwrap_or_default();
    // GpuiApp-managed preferences override config.kdl's theme — that's where
    // the menu-driven "View → Theme" picks land. Falls back to the kdl
    // theme (or built-in default) when the user hasn't switched themes via
    // the UI yet.
    let prefs = load_preferences();
    let theme_name = prefs
        .theme
        .as_deref()
        .and_then(ThemeName::parse)
        .unwrap_or(config.theme);
    let theme = Theme::from_name(theme_name);

    // No path → launch directly into the file browser at cwd.
    let initial_doc: Option<(Vec<RenderedBlock>, String, PathBuf)> = match args.get(1) {
        Some(p) => {
            let path = PathBuf::from(p);
            let text = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read {}: {}", path.display(), e);
                    process::exit(1);
                }
            };
            let canon = path
                .canonicalize()
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string();
            let doc = Document::from_text(text, path.clone());
            let blocks = render_with_wiki(&doc.full_text(), &theme, Some(&path));
            println!("yalda-gpui: loaded {} ({} blocks)", canon, blocks.len());
            Some((blocks, canon, path))
        }
        None => {
            println!("yalda-gpui: no file given, opening browser");
            None
        }
    };

    Application::new().run(move |app: &mut GpuiApp| {
        register_keymap(app);

        // Quit when the last window closes. macOS apps typically stay
        // alive in the menu bar after every window is dismissed, but
        // yalda has no menu-bar-only mode — without this hook, closing
        // the red dot leaves a headless process running that can only
        // be killed from Activity Monitor. `detach` keeps the
        // subscription alive for the app's lifetime.
        app.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::new(point(px(120.0), px(80.0)), size(px(900.0), px(700.0)));
        let window_handle = app
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Titled window so the standard system title bar (with
                    // close/minimize/maximize buttons AND the resize affordance
                    // that comes with it) is rendered. Previously `None` →
                    // chromeless window that couldn't be resized.
                    titlebar: Some(TitlebarOptions {
                        title: Some("Yaldabaoth".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, app| {
                    app.new(|cx| {
                        let focus_handle = cx.focus_handle();
                        focus_handle.focus(window);
                        let mut view = match initial_doc.clone() {
                            Some((blocks, canon, path)) => {
                                let mut v = YaldaGpuiView::new_doc(
                                    blocks,
                                    theme.clone(),
                                    canon,
                                    focus_handle,
                                );
                                // 5c: pool-bind the startup Doc now that the
                                // workspace (and its buffer pool) exists, so the
                                // CLI file shares its core with any Edit view and
                                // live-tracks.
                                if let Ok((id, core)) = v.workspace.open_and_retain(&path)
                                    && let Some(App::Buffer(BufferApp::Viewing(d))) =
                                        v.workspace.focused_content_mut()
                                {
                                    d.source = Some(DocSource::new(id, core));
                                }
                                v
                            }
                            None => YaldaGpuiView::new_browser(
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                                theme.clone(),
                                focus_handle,
                            ),
                        };
                        // Code font: "SF Mono" is only available to third-
                        // party apps if the user installed it (the system's
                        // built-in copy is the hidden ".SF NS Mono", which
                        // can't be requested by name) — otherwise GPUI falls
                        // back to a *proportional* face and code stops
                        // looking like code. Probe the registry and fall
                        // back to Menlo, which always ships with macOS.
                        {
                            let names = cx.text_system().all_font_names();
                            if !names.iter().any(|n| n == "SF Mono") {
                                view.code_font = SharedString::new_static("Menlo");
                            }
                        }
                        // Restore the saved text zoom (clamped so a hand-edited
                        // preferences file can't push the body off-screen).
                        if let Some(scale) = prefs.text_scale {
                            view.text_scale = scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE);
                        }
                        // Desktop density v3: migrate the previously shipped 3×3
                        // density to 4×4 once, while preserving asymmetric custom
                        // grids and post-migration explicit choices.
                        let (grid_cols, grid_rows) = restore_desktop_grid(
                            prefs.desktop_grid_cols,
                            prefs.desktop_grid_rows,
                            prefs.desktop_grid_defaults_version,
                        );
                        view.desktop_grid_cols = grid_cols;
                        view.desktop_grid_rows = grid_rows;
                        if let Some(v) = prefs.jump_panel_visible {
                            view.jump_panel_visible = v;
                        }
                        // User's drag-reordered jump-panel order (jump-reorder).
                        if let Some(o) = prefs.jump_cwd_order {
                            view.jump_cwd_order = o;
                        }
                        if let Some(o) = prefs.jump_session_order {
                            view.jump_session_order = o;
                        }
                        if let Some(names) = prefs.jump_folded_projects {
                            view.jump_folded_projects = names.into_iter().collect();
                        }
                        // Universal agent roster (universal-agent-list): start
                        // the server pump + seed the roster at boot (not only
                        // when an agent tile opens), so the jump panel shows
                        // every active session from the first frame and stays
                        // live via the Created/Closed/Renamed broadcasts.
                        if view.session_server.is_some() {
                            view.start_server_pump(cx);
                            view.refresh_roster(cx);
                        }
                        // If we were launched with no explicit file arg, try to
                        // restore the saved workspace for this cwd. With an
                        // explicit arg the user wants that file, so the saved
                        // snapshot stays on disk for the next no-arg launch.
                        if initial_doc.is_none() {
                            view.restore_workspace_from_disk(cx);
                        }
                        // Reboot handoff: the previous yalda process set this
                        // env var via `reboot_into_claude` to mean "boot
                        // straight into the claude screen and resume every
                        // saved session." The downstream `open_agent_inner`
                        // consults `load_persisted_acp_sessions`, so
                        // session/load fires once per persisted slot.
                        if std::env::var("YALDA_OPEN_CLAUDE").is_ok() {
                            view.open_agent_inner(cx);
                        }
                        // Set splash deadline AFTER all init (workspace
                        // restoration, agent attach) so the countdown starts
                        // from the moment the window is ready to paint.
                        view.splash_until =
                            Some(std::time::Instant::now() + Duration::from_millis(1500));
                        // Auto-dismiss splash after 1.5s.
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(1500))
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                this.splash_until = None;
                                cx.notify();
                            });
                        })
                        .detach();
                        view
                    })
                },
            )
            .unwrap();

        // Run the ACP teardown synchronously when the app is quitting,
        // *before* `GpuiApp::shutdown` clears windows and races view Drop
        // against worker-thread joins. The hook gives us a 100ms budget
        // (`SHUTDOWN_TIMEOUT`) — comfortably enough for the worker to
        // signal its child, since the agent process has `kill_on_drop`
        // and exits as soon as the runtime drops. Returning a no-op
        // future satisfies the async signature; the real work is sync.
        app.on_app_quit(move |cx| {
            let _ = window_handle.update(cx, |view, _w, ctx| {
                view.shutdown_acp(ctx);
            });
            async move {}
        })
        .detach();

        // Install the macOS menu bar. The first menu's name is what the
        // system uses for the app menu (the bold leftmost menu); without
        // an `.app` bundle, that's also what the user sees in the
        // top-left of the screen. Keystrokes shown next to each item are
        // pulled from `bind_keys` above — the matching `cmd-*` bindings
        // make Cmd-Q / Cmd-O / Cmd-K render as proper accelerators.
        app.set_menus(vec![
            Menu {
                name: "yalda".into(),
                items: vec![MenuItem::action("Quit yalda", Quit)],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Open File Browser", OpenBrowser),
                    MenuItem::action("Open Claude Session", OpenAgent),
                    MenuItem::action("Open Linear", OpenLinear),
                    MenuItem::action("Keyboard Shortcuts", OpenKeymap),
                ],
            },
        ]);

        // TEMP autonomous reproduction harness for the recurring "/clear
        // worksheet-invisible" bug. `YALDA_SCENARIO=clear-worksheet` drives the
        // REAL production methods programmatically (no keystroke injection, no GUI
        // navigation) against an ISOLATED server (launch with YALDA_SESSION_SOCKET
        // + YALDA_ACP_AGENT=yalda-acp-stub + a temp HOME): create a fresh worksheet
        // agent, type "/clear" + submit (→ real clear_agent_session), wait for the
        // async re-create/rebind, then type "hello" through the REAL key handler.
        // The `clear_log` instrumentation captures the full causal chain to
        // /tmp/yalda-clear-debug.log, then the app quits. Removed once root-caused.
        let scenario = std::env::var("YALDA_SCENARIO").ok();
        // Two-phase autonomous reproduction of the RESTORED-session /clear bug
        // (the confirmed precondition). Run BOTH with the same HOME + the same
        // (kept-alive) isolated server:
        //   clear-setup    → create agent, converse (persist), quit.
        //   clear-restored → boot RESTORES that session from disk; then /clear + type.
        if matches!(scenario.as_deref(), Some("clear-setup") | Some("clear-restored")) {
            let phase = scenario.clone().unwrap();
            let wh = window_handle;
            app.spawn(async move |cx| {
                let mk = |k: &str| gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        modifiers: gpui::Modifiers::default(),
                        key: k.to_string(),
                        key_char: (k.chars().count() == 1).then(|| k.to_string()),
                    },
                    is_held: false,
                };
                let bg = cx.background_executor().clone();
                macro_rules! sleep {
                    ($ms:expr) => {
                        bg.timer(Duration::from_millis($ms)).await
                    };
                }
                let bound_with_sid = |wh: &gpui::WindowHandle<YaldaGpuiView>,
                                      cx: &mut gpui::AsyncApp| {
                    wh.update(cx, |v, _w, _cx| {
                        v.focused_bound_session()
                            .map(|id| v.sessions.sid_of(id).is_some())
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
                };
                clear_log(&format!("scenario[{phase}]: start"));
                sleep!(2500);
                let _ = wh.update(cx, |v, _w, _cx| v.splash_until = None);

                if phase == "clear-setup" {
                    // PHASE A: create a session, converse, persist, quit.
                    let _ = wh.update(cx, |v, _w, cx| v.new_agent_session(None, cx));
                    for _ in 0..200u32 {
                        sleep!(100);
                        if bound_with_sid(&wh, cx) {
                            break;
                        }
                    }
                    clear_log("scenario[setup]: session bound; sending 'hi'");
                    let _ = wh.update(cx, |v, w, cx| {
                        for ch in "hi".chars() {
                            v.handle_claude_key(&mk(&ch.to_string()), w, cx);
                        }
                    });
                    sleep!(200);
                    let _ = wh.update(cx, |v, _w, cx| v.submit_agent(cx));
                    sleep!(12000); // let the agent reply + settle
                    // Log what save_agent_ring will see (it only persists a session
                    // that has a resume_id or a channel sid).
                    let _ = wh.update(cx, |v, _w, cx| {
                        if let Some(id) = v.focused_bound_session() {
                            let _ = v.read_session(id, cx, |c| {
                                clear_log(&format!(
                                    "scenario[setup]: pre-save resume_id_present={} channel_present={} sid_of={:?}",
                                    // resume_id lives on the AgentSession, not state — read via entity below
                                    "?", c.channel.is_some(), "?"
                                ));
                            });
                            if let Some(ent) = v.session_entity(id) {
                                let s = ent.read(cx);
                                clear_log(&format!(
                                    "scenario[setup]: resume_id={:?} channel_sid={:?} store_sid={:?}",
                                    s.resume_id,
                                    s.state.channel.as_ref().and_then(|c| c.session_id()),
                                    v.sessions.sid_of(id).map(|x| x.to_string()),
                                ));
                            }
                        }
                    });
                    // PERSIST the session + workspace so the next launch RESTORES it.
                    let _ = wh.update(cx, |v, _w, cx| {
                        v.save_agent_ring(cx);
                        v.save_workspace_state();
                    });
                    clear_log("scenario[setup]: persisted; quitting");
                    sleep!(500);
                    let _ = cx.update(|cx| cx.quit());
                    return;
                }

                // PHASE B: the boot already ran restore_workspace_from_disk. Wait for
                // the RESTORED session to rebind + resume.
                clear_log("scenario[restored]: waiting for restore/resume of the session");
                let mut restored = false;
                for i in 0..250u32 {
                    sleep!(100);
                    if bound_with_sid(&wh, cx) {
                        clear_log(&format!("scenario[restored]: restored+bound after {}00ms", i));
                        restored = true;
                        break;
                    }
                }
                if !restored {
                    clear_log("scenario[restored]: NO restored session bound — check persistence");
                }
                sleep!(1500);
                // Log the RESTORED session's pre-/clear state.
                let _ = wh.update(cx, |v, _w, cx| {
                    if let Some(id) = v.focused_bound_session() {
                        let _ = v.read_session(id, cx, |c| {
                            clear_log(&format!(
                                "scenario[restored]: pre-/clear focus_compose={} you_block_open={} \
                                 awaiting={} chatbox={} inline_active={} lines={}",
                                c.focus == AgentFocus::Compose,
                                c.you_block_open,
                                c.turn_phase.is_awaiting(),
                                c.input_surface.is_chatbox(),
                                c.inline_you_block_active(),
                                c.editor.document().line_count(),
                            ));
                        });
                    }
                });
                // /clear the RESTORED session: press i (nav→typeable), type /clear, submit.
                let _ = wh.update(cx, |v, w, cx| {
                    v.handle_claude_key(&mk("i"), w, cx);
                    for ch in "/clear".chars() {
                        v.handle_claude_key(&mk(&ch.to_string()), w, cx);
                    }
                });
                sleep!(200);
                let _ = wh.update(cx, |v, _w, cx| v.submit_agent(cx));
                clear_log("scenario[restored]: submitted /clear on RESTORED session; waiting for rebind");
                sleep!(6000);
                // Ensure the window is FRONTMOST so a missing repaint below is a
                // real repaint-not-firing bug, NOT an occluded-window artifact.
                let _ = cx.update(|cx| cx.activate(true));
                sleep!(600);
                clear_log("scenario[restored]: window activated; typing 'hello' — a build_body SHOULD follow");
                let _ = wh.update(cx, |v, w, cx| {
                    for ch in "hello".chars() {
                        v.handle_claude_key(&mk(&ch.to_string()), w, cx);
                    }
                });
                clear_log("scenario[restored]: typed hello");
                sleep!(2500);
                // Simulate the JUMP BAR click: an UNRELATED full re-render. The user
                // reports THIS reveals the stale edits. If typing above produced no
                // build_body but this does, that IS the repaint-not-firing bug.
                clear_log("scenario[restored]: >>> jump-bar sim (root cx.notify → schedules a frame) — does THIS paint the edits?");
                let _ = wh.update(cx, |_v, _w, cx| cx.notify());
                sleep!(1500);
                clear_log("scenario[restored]: done, quitting");
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        }

        // Bring yalda to the foreground on launch. Without this the
        // process opens a window but stays behind whatever app the user
        // had focused (terminal, editor, etc.) — particularly noticeable
        // on a `cargo run` or a `reboot_into_claude` re-launch. `true`
        // = ignore other apps' "don't yield focus" hints, which is the
        // right behaviour for a user-initiated launch. (Scenario mode also
        // activates — an occluded window doesn't paint, and the scenario needs the
        // real render pass; it self-quits in ~14s.)
        app.activate(true);
    });
}

#[cfg(test)]
mod tests;
