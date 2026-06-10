//! `sketch-gpui` — GPU-accelerated desktop frontend for sketch.
//!
//! Rendered-markdown viewer + file browser using Zed's GPUI framework. The
//! TUI frontend (`src/main.rs` + `src/app.rs`) is left untouched; this binary
//! consumes only the framework-neutral core (`document`, `render`, `theme`,
//! `blocks`, `style`, `file_browser`).
//!
//! Run:
//!     cargo run --bin sketch-gpui                       # opens browser at cwd
//!     cargo run --bin sketch-gpui -- <path/to/file.md>  # opens file directly
//!
//! Document view keys:
//!   j / Down / Ctrl-N    scroll down one block
//!   k / Up / Ctrl-P      scroll up one block
//!   h                    move cursor to previous block
//!   l                    move cursor to next block
//!   g                    top of document
//!   G                    bottom of document
//!   Space                open command menu (TUI-style picker)
//!   Ctrl-E               edit current file (raw markdown)
//!   Ctrl-W               edit current file (word-processor view)
//!   Ctrl-K               open Claude (ACP) chat screen
//!   Ctrl-O               open file browser
//!   Tab / Shift-Tab      next / previous buffer
//!   q / Esc              quit
//!
//! Menu (Space anywhere, or Edit/Claude Normal-mode):
//!   * Single-key picker over a small set of commands (open browser,
//!     enter edit/wp, open claude, back-to-doc, quit).
//!   * Submenu nodes drill in; Esc pops one level (closes from root).
//!   * Same `MenuState` model as the TUI (`src/menu.rs`); the menu tree
//!     itself is the GPUI-specific subset (`gpui_menu()`).
//!
//! Claude (ACP) chat screen:
//!   * Spawns a local `claude-agent-acp` (or $SKETCH_ACP_AGENT) process and
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
//!     s                    cycle sort order (name / date↓ / date↑)
//!     q / Esc              close browser (returns to doc, or quits)

mod agent;
mod agent_ui;
mod browser_ui;
mod chrome;
mod edit_ui;
mod highlight_cache;
mod persist;
mod render_blocks;
mod screens;
#[cfg(test)]
mod verify_harness;
pub(crate) use agent::*;
pub(crate) use persist::*;
pub(crate) use render_blocks::*;
mod workspace;

pub(crate) use highlight_cache::{HighlightCache, LineHl};
pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::path::PathBuf;
pub(crate) use std::process;
pub(crate) use std::rc::Rc;
pub(crate) use std::time::Duration;

pub(crate) use gpui::{
    AnyElement, App, AppContext, Application, Bounds, Context, Element, ElementId, FocusHandle,
    Focusable, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, Keystroke,
    LayoutId, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, Task, TextLayout, TextRun, TitlebarOptions,
    UnderlineStyle, Window, WindowBounds, WindowOptions, actions, div, point, px, rgb, rgba, size,
};

pub(crate) use sketch::acp_channel::AcpChannelClient;
pub(crate) use sketch::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine, StyledSpan};
pub(crate) use sketch::cursor::CursorPos;
pub(crate) use sketch::document::Document;
pub(crate) use sketch::editor::{Editor, EditorCore, EditorView, LineAnchor};
pub(crate) use sketch::file_browser::{BrowserEntry, FileBrowser};
pub(crate) use sketch::keybind::KeybindManager;
pub(crate) use sketch::keys::{Key, KeyPress, Modifiers as KMods};
pub(crate) use sketch::md_highlight::{
    Segment, highlight_markdown_lines_stripped_syn, highlight_markdown_lines_syn,
};
pub(crate) use sketch::menu::{MenuAction, MenuNode, MenuNodeKind, MenuState};
pub(crate) use sketch::render;
pub(crate) use sketch::session_client::SessionServerClient;
pub(crate) use sketch::session_proto::AttachMode;
pub(crate) use sketch::session_proto::Notification as ServerNotification;
pub(crate) use sketch::session_proto::SessionInfo;
pub(crate) use sketch::style::{Color as NColor, Modifier, Style as NStyle};
pub(crate) use sketch::theme::{OverlayTheme, Theme, ThemeName};
pub(crate) use sketch::worktree;

// ----------------------------------------------------------------------------
// Render performance knobs (env-gated, read once)
// ----------------------------------------------------------------------------

/// `true` when `SKETCH_PERF` is set to anything other than `0`/empty. Enables
/// per-frame timing breakdowns from the agent pane render path (extract /
/// highlight / snapshot / total), printed to stderr. Read once and cached.
fn perf_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("SKETCH_PERF"), Ok(v) if v != "0" && !v.is_empty()))
}

/// `false` only when `SKETCH_HL_CACHE` is explicitly `0`/`off`/`false`. The
/// incremental highlight cache is ON by default; this lets us A/B it against
/// the old full-recompute path at runtime without a rebuild.
fn hl_cache_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("SKETCH_HL_CACHE").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}

// ----------------------------------------------------------------------------
// Actions
// ----------------------------------------------------------------------------

actions!(
    sketch,
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
        OpenMenu,
        OpenLocalMenu,
        Quit,
        Restart,
        // Buffer cycling
        NextBuffer,
        PrevBuffer,
        // Tab cycling (workspace-level — independent of buffer list)
        NextTab,
        PrevTab,
        NewTab,
        CloseTab,
        // Move the focused pane to another workspace (Ctrl-W m). Opens the
        // workspace picker; selecting a target relocates the focused leaf
        // (content travels with it). See spec-workspaces-tagging.md Phase 1.
        MovePane,
        // Also-show the focused (file-backed) pane in another workspace
        // (Ctrl-W M / shift). Opens the same picker; selecting a target
        // creates a second view onto the same file there, leaving the
        // original in place. Agent/Browser panes are single-home (rejected).
        AlsoShowPane,
        // Splits (Ctrl-W chord prefix per spec-tabs-and-splits.md §12)
        SplitH,
        SplitV,
        CloseWindow,
        OnlyWindow,
        // Focus motion within the active tab's split tree
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        FocusNext,
        FocusPrev,
        // Resize the focused pane vs. its sibling
        ResizeShrink,
        ResizeGrow,
        Equalize,
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
        // Document text zoom (scales body + headings; chrome stays fixed)
        ZoomIn,
        ZoomOut,
        ZoomReset,
        // Copy selected text to system clipboard (all screens)
        CopySelection,
        // View-mode mouse text selection (doc view only, legacy alias)
        CopyDocSelection,
        // Paste from system clipboard into active editor
        PasteFromClipboard,
        // Open the rename input overlay for the active tab.
        RenameTab,
        // Agent window: send the current draft. Worksheet sweep (§12) or
        // Chatbox submit (§18) depending on `AgentState::input_mode`.
        SubmitAgent,
        // Agent window: flip the input mode between Worksheet and
        // Chatbox (§5). Bound to `Ctrl-Alt-Enter`.
        ToggleAgentInputMode,
        // Agent window: open/close the Tasklist sidepane (§32). Cmd-1.
        ToggleTasklist,
        // Agent window: open/close the Subagents sidepane (§32). Cmd-2.
        ToggleSubagents,
        // Agent window: interrupt the in-flight turn (ACP session/cancel).
        // Bound to Cmd-. and surfaced as a Stop button while a reply is
        // pending.
        StopAgent,
        // Rail (persistent side column, spec-rail.md).
        // Toggles (global, `None` context):
        ToggleFileBrowserRail,
        ToggleOutlineRail,
        FlipRailSide,
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
        // Layout patterns (spec-layout-patterns.md)
        // Phase 2: automatic layouts
        CycleLayoutMode,
        PromoteToMaster,
        IncreaseMasterCount,
        DecreaseMasterCount,
        // Phase 3: tags
        ClearTagView,
        TagViewChord,
        TagToggleChord,
    ]
);

// ----------------------------------------------------------------------------
// gpui::Keystroke → sketch::keys::KeyPress bridge
// ----------------------------------------------------------------------------

/// Convert a GPUI keystroke to our framework-neutral `KeyPress` so the same
/// `KeybindManager` + `Action` vocabulary the TUI uses can drive the GPUI
/// edit mode. SHIFT is omitted by convention — uppercase chars are encoded
/// as `Key::Char('G')` with no SHIFT modifier (matches `KeyPress::from_event`
/// behavior on the crossterm side).
fn keystroke_to_keypress(ks: &Keystroke) -> KeyPress {
    let mut mods = KMods::NONE;
    if ks.modifiers.control {
        mods |= KMods::CONTROL;
    }
    if ks.modifiers.alt {
        mods |= KMods::ALT;
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

// ----------------------------------------------------------------------------
// Claude (ACP) helpers — port of app::claude splice/lock logic
// ----------------------------------------------------------------------------

/// Convert a rope char index to (line, col). Document doesn't expose this
/// directly but Document::rope() is public. Mirrors editor.rs's private
/// `char_to_line_col`.
fn doc_char_to_line_col(doc: &Document, char_idx: usize) -> (usize, usize) {
    let rope = doc.rope();
    let len = rope.len_chars();
    let i = char_idx.min(len);
    let line = rope.char_to_line(i);
    let line_start = rope.line_to_char(line);
    (line, i - line_start)
}

// ----------------------------------------------------------------------------
// Root view
// ----------------------------------------------------------------------------

/// State held while the user is viewing a rendered markdown document.
struct DocState {
    blocks: Vec<RenderedBlock>,
    file_label: SharedString,
    cursor_block: usize,
    /// Variable-height virtualized list driving the doc body. Only the
    /// visible block window is built/laid-out per frame (not one element
    /// per block, as the old `overflow_y_scroll` container did), so render
    /// is O(visible) instead of O(blocks+spans). j/k/g/G/ctrl-d/u navigation
    /// reveals the focused block via `scroll_to_reveal_item`. Spliced/reset
    /// to `blocks.len()` each render.
    list_state: gpui::ListState,
    /// Item count currently registered in `list_state`; lets render splice
    /// only when the block count changed. `Cell` because `render_doc` takes
    /// `&DocState` (the list itself splices through `&self`).
    list_item_count: std::cell::Cell<usize>,
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
    /// past this — O(1) when idle, one re-parse per change (the two-pane live
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
    /// A fresh, empty variable-height `ListState` for a new Doc pane. Mirrors
    /// the agent transcript's list construction; `Top` alignment keeps the
    /// document anchored at its first block (unlike the agent's `Bottom`).
    fn new_list_state(count: usize) -> gpui::ListState {
        gpui::ListState::new(count, gpui::ListAlignment::Top, gpui::px(512.0))
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
        if idx < self.list_item_count.get() {
            self.list_state.scroll_to_reveal_item(idx);
        }
    }
}

/// State held while the user is browsing the filesystem.
///
/// `underlying`: when the browser was opened *in place* of an existing
/// Doc/Edit/Claude window (Cmd-O from a focused pane), this holds that
/// prior content so Esc/q can restore it. `None` when the browser was
/// opened standalone (new-tab open, initial cwd browser, splits that
/// fall back to a browser pane). In-memory only — not persisted with
/// the workspace snapshot, since "restore to a browser-with-stashed-
/// content" doesn't carry meaning across process restarts.
struct BrowserWindow {
    fb: FileBrowser,
    underlying: Option<Box<WindowContent>>,
}

impl BrowserWindow {
    /// Standalone browser — no prior content to restore on Esc.
    fn standalone(dir: PathBuf) -> Self {
        Self {
            fb: FileBrowser::new(dir),
            underlying: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
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
/// selection, insert flag) is owned here, so each split / also-shown pane
/// navigates independently while edits + undo land on one shared rope.
///
/// `buffer_id` is the pool key; the owning window must `buffer_release` it on
/// close so refcounting can drop clean, unreferenced buffers.
struct SharedEditor {
    /// Pool key for this file's core. Liveness is tracked via `Rc` strong
    /// count (see `Workspace::gc_buffers`), so this id isn't needed for
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

/// The dispatch core (`dispatch_normal_core` / `dispatch_insert_core`) is
/// shared by every text surface — the pooled Edit pane plus the (non-pooled)
/// Chatbox and Agent transcript. This trait lets the dispatch stay one body
/// while operating over either an owned [`Editor`] or a pool-backed
/// [`SharedEditor`]. Every method mirrors the matching `Editor` method; the
/// `SharedEditor` impl simply borrows its `Rc<RefCell<EditorCore>>` first.
trait EditOps {
    fn cursor(&self) -> CursorPos;
    fn cursor_set(&mut self, line: usize, col: usize);
    fn cursor_move_left(&mut self);
    fn cursor_move_up(&mut self);
    fn cursor_move_line_start(&mut self);
    fn cursor_jump_top(&mut self);
    fn line_len_chars(&self, line: usize) -> usize;
    fn line_text_at_cursor(&self) -> String;

    fn extend_mode(&self) -> bool;
    fn set_extend_mode(&mut self, on: bool);
    fn toggle_extend_mode(&mut self);
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))>;
    fn selection_anchor(&self) -> Option<CursorPos>;
    fn anchor_at_cursor(&mut self);
    fn clear_selection(&mut self);
    fn collapse_selection(&mut self);
    fn flip_selection(&mut self);
    fn select_all(&mut self);
    fn extend_by_line(&mut self);
    fn yank_selection(&self) -> Option<String>;

    fn pre_move(&mut self, creates_selection: bool);
    fn move_down(&mut self, insert_mode: bool);
    fn move_right_clamped(&mut self, insert_mode: bool);
    fn clamp_cursor_col(&mut self, insert_mode: bool);
    fn move_cursor_line_end(&mut self, insert_mode: bool);
    fn move_cursor_word_forward(&mut self);
    fn move_cursor_word_backward(&mut self);
    fn move_cursor_word_end(&mut self);
    fn jump_cursor_bottom(&mut self);

    fn begin_insert(&mut self);
    fn end_insert(&mut self);
    fn insert_char(&mut self, ch: char);
    fn backspace(&mut self);
    fn delete_char_at_cursor(&mut self);
    fn delete_current_line(&mut self);
    fn delete_selection(&mut self) -> bool;
    fn open_line_below(&mut self);
    fn open_line_above(&mut self);
    fn undo(&mut self);
    fn redo(&mut self);
}

impl EditOps for Editor {
    fn cursor(&self) -> CursorPos {
        *Editor::cursor(self)
    }
    fn cursor_set(&mut self, line: usize, col: usize) {
        let c = self.cursor_mut();
        c.line = line;
        c.col = col;
    }
    fn cursor_move_left(&mut self) {
        self.cursor_mut().move_left();
    }
    fn cursor_move_up(&mut self) {
        self.cursor_mut().move_up();
    }
    fn cursor_move_line_start(&mut self) {
        self.cursor_mut().move_line_start();
    }
    fn cursor_jump_top(&mut self) {
        self.cursor_mut().jump_top();
    }
    fn line_len_chars(&self, line: usize) -> usize {
        self.document().line_len_chars(line)
    }
    fn line_text_at_cursor(&self) -> String {
        self.document().line_text(Editor::cursor(self).line)
    }
    fn extend_mode(&self) -> bool {
        Editor::extend_mode(self)
    }
    fn set_extend_mode(&mut self, on: bool) {
        Editor::set_extend_mode(self, on);
    }
    fn toggle_extend_mode(&mut self) {
        Editor::toggle_extend_mode(self);
    }
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        Editor::selection_range(self)
    }
    fn selection_anchor(&self) -> Option<CursorPos> {
        Editor::selection_anchor(self)
    }
    fn anchor_at_cursor(&mut self) {
        Editor::anchor_at_cursor(self);
    }
    fn clear_selection(&mut self) {
        Editor::clear_selection(self);
    }
    fn collapse_selection(&mut self) {
        Editor::collapse_selection(self);
    }
    fn flip_selection(&mut self) {
        Editor::flip_selection(self);
    }
    fn select_all(&mut self) {
        Editor::select_all(self);
    }
    fn extend_by_line(&mut self) {
        Editor::extend_by_line(self);
    }
    fn yank_selection(&self) -> Option<String> {
        Editor::yank_selection(self)
    }
    fn pre_move(&mut self, creates_selection: bool) {
        Editor::pre_move(self, creates_selection);
    }
    fn move_down(&mut self, insert_mode: bool) {
        Editor::move_down(self, insert_mode);
    }
    fn move_right_clamped(&mut self, insert_mode: bool) {
        Editor::move_right_clamped(self, insert_mode);
    }
    fn clamp_cursor_col(&mut self, insert_mode: bool) {
        Editor::clamp_cursor_col(self, insert_mode);
    }
    fn move_cursor_line_end(&mut self, insert_mode: bool) {
        Editor::move_cursor_line_end(self, insert_mode);
    }
    fn move_cursor_word_forward(&mut self) {
        Editor::move_cursor_word_forward(self);
    }
    fn move_cursor_word_backward(&mut self) {
        Editor::move_cursor_word_backward(self);
    }
    fn move_cursor_word_end(&mut self) {
        Editor::move_cursor_word_end(self);
    }
    fn jump_cursor_bottom(&mut self) {
        Editor::jump_cursor_bottom(self);
    }
    fn begin_insert(&mut self) {
        Editor::begin_insert(self);
    }
    fn end_insert(&mut self) {
        Editor::end_insert(self);
    }
    fn insert_char(&mut self, ch: char) {
        Editor::insert_char(self, ch);
    }
    fn backspace(&mut self) {
        Editor::backspace(self);
    }
    fn delete_char_at_cursor(&mut self) {
        Editor::delete_char_at_cursor(self);
    }
    fn delete_current_line(&mut self) {
        Editor::delete_current_line(self);
    }
    fn delete_selection(&mut self) -> bool {
        Editor::delete_selection(self)
    }
    fn open_line_below(&mut self) {
        Editor::open_line_below(self);
    }
    fn open_line_above(&mut self) {
        Editor::open_line_above(self);
    }
    fn undo(&mut self) {
        Editor::undo(self);
    }
    fn redo(&mut self) {
        Editor::redo(self);
    }
}

impl EditOps for SharedEditor {
    fn cursor(&self) -> CursorPos {
        *self.view.cursor()
    }
    fn cursor_set(&mut self, line: usize, col: usize) {
        let c = self.view.cursor_mut();
        c.line = line;
        c.col = col;
    }
    fn cursor_move_left(&mut self) {
        self.view.cursor_mut().move_left();
    }
    fn cursor_move_up(&mut self) {
        self.view.cursor_mut().move_up();
    }
    fn cursor_move_line_start(&mut self) {
        self.view.cursor_mut().move_line_start();
    }
    fn cursor_jump_top(&mut self) {
        self.view.cursor_mut().jump_top();
    }
    fn line_len_chars(&self, line: usize) -> usize {
        self.core.borrow().document().line_len_chars(line)
    }
    fn line_text_at_cursor(&self) -> String {
        let line = self.view.cursor().line;
        self.core.borrow().document().line_text(line)
    }
    fn extend_mode(&self) -> bool {
        self.view.extend_mode()
    }
    fn set_extend_mode(&mut self, on: bool) {
        self.view.set_extend_mode(on);
    }
    fn toggle_extend_mode(&mut self) {
        self.view.toggle_extend_mode();
    }
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        self.view.selection_range()
    }
    fn selection_anchor(&self) -> Option<CursorPos> {
        self.view.selection_anchor()
    }
    fn anchor_at_cursor(&mut self) {
        self.view.anchor_at_cursor();
    }
    fn clear_selection(&mut self) {
        self.view.clear_selection();
    }
    fn collapse_selection(&mut self) {
        self.view.collapse_selection();
    }
    fn flip_selection(&mut self) {
        self.view.flip_selection();
    }
    fn select_all(&mut self) {
        self.view.select_all(&self.core.borrow());
    }
    fn extend_by_line(&mut self) {
        self.view.extend_by_line(&self.core.borrow());
    }
    fn yank_selection(&self) -> Option<String> {
        self.view.yank_selection(&self.core.borrow())
    }
    fn pre_move(&mut self, creates_selection: bool) {
        self.view.pre_move(creates_selection);
    }
    fn move_down(&mut self, insert_mode: bool) {
        self.view.move_down(&self.core.borrow(), insert_mode);
    }
    fn move_right_clamped(&mut self, insert_mode: bool) {
        self.view
            .move_right_clamped(&self.core.borrow(), insert_mode);
    }
    fn clamp_cursor_col(&mut self, insert_mode: bool) {
        self.view.clamp_cursor_col(&self.core.borrow(), insert_mode);
    }
    fn move_cursor_line_end(&mut self, insert_mode: bool) {
        self.view
            .move_cursor_line_end(&self.core.borrow(), insert_mode);
    }
    fn move_cursor_word_forward(&mut self) {
        self.view.move_cursor_word_forward(&self.core.borrow());
    }
    fn move_cursor_word_backward(&mut self) {
        self.view.move_cursor_word_backward(&self.core.borrow());
    }
    fn move_cursor_word_end(&mut self) {
        self.view.move_cursor_word_end(&self.core.borrow());
    }
    fn jump_cursor_bottom(&mut self) {
        self.view.jump_cursor_bottom(&self.core.borrow());
    }
    fn begin_insert(&mut self) {
        self.view.begin_insert(&mut self.core.borrow_mut());
    }
    fn end_insert(&mut self) {
        self.view.end_insert(&mut self.core.borrow_mut());
    }
    fn insert_char(&mut self, ch: char) {
        self.view.insert_char(&mut self.core.borrow_mut(), ch);
    }
    fn backspace(&mut self) {
        self.view.backspace(&mut self.core.borrow_mut());
    }
    fn delete_char_at_cursor(&mut self) {
        self.view.delete_char_at_cursor(&mut self.core.borrow_mut());
    }
    fn delete_current_line(&mut self) {
        self.view.delete_current_line(&mut self.core.borrow_mut());
    }
    fn delete_selection(&mut self) -> bool {
        self.view.delete_selection(&mut self.core.borrow_mut())
    }
    fn open_line_below(&mut self) {
        self.view.open_line_below(&mut self.core.borrow_mut());
    }
    fn open_line_above(&mut self) {
        self.view.open_line_above(&mut self.core.borrow_mut());
    }
    fn undo(&mut self) {
        self.view.undo(&mut self.core.borrow_mut());
    }
    fn redo(&mut self) {
        self.view.redo(&mut self.core.borrow_mut());
    }
}

// Build the trimmed, tab-expanded per-line text for an Edit pane's body,
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
    /// selection, cross-pane notify) so we don't re-allocate a String per line.
    lines_cache: std::rc::Rc<Vec<String>>,
    /// Virtualized line list — only the visible rows are built/laid-out each
    /// frame instead of one element per document line. Variable height (lines
    /// wrap), so a `ListState` (the agent-transcript pattern) rather than a
    /// fixed-row viewport. Spliced/reset to the line count each render.
    list_state: gpui::ListState,
    /// Item count `list_state` was last sized to; drives incremental splice.
    list_item_count: usize,
    /// `(edit_seq, cursor_line)` at the last render. When either changes we
    /// scroll the list to reveal the cursor line (so typing/motion keeps the
    /// caret on-screen) without fighting the user's manual scroll on idle
    /// frames.
    last_cursor_anchor: Option<(u64, usize)>,
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
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Top, gpui::px(256.0)),
            list_item_count: 0,
            last_cursor_anchor: None,
        }
    }

    /// Extract + highlight the buffer's source lines incrementally. Returns the
    /// shared source-line vector and a per-line highlight snapshot whose `raw`
    /// segments are byte-identical to `highlight_markdown_lines_syn`. Both are
    /// keyed on the document's `edit_seq`, so a frame that didn't edit the
    /// buffer recomputes zero lines and a single-char edit recomputes ~1.
    fn highlight_snapshot(
        &mut self,
        theme: &Theme,
        hl: &sketch::highlight::Highlighter,
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
#[allow(clippy::large_enum_variant)]
enum WindowContent {
    Doc(DocState),
    Edit(EditState),
    Agent(AgentRing),
    Browser(BrowserWindow),
}

/// Overlay popup that lets the user pick a top-level command by single
/// keypress (TUI-style — see `src/menu.rs` for the underlying tree model).
/// When `Some` on `SketchGpuiView`, `Render::render` swaps the screen body
/// for `render_menu`; key dispatch routes to `handle_menu_key`. The menu
/// items here are GPUI-specific (a subset of the TUI default that maps to
/// actions the GPUI frontend implements).
struct MenuOverlay {
    state: MenuState,
    menu: Vec<MenuNode>,
    /// Scope tag shown in the overlay header (spec-menu-scopes.md): "MENU"
    /// for the global leader; "DOC"/"EDIT"/"AGENT"/"BROWSE" for the local one.
    header: &'static str,
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

struct SessionSwitcher {
    selected: usize,
}

/// What the workspace picker will do with the chosen target. Drives the
/// header copy and the commit branch so one picker overlay serves both
/// "move pane" (Ctrl-W m) and "also-show pane" (Ctrl-W M).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspacePickerMode {
    /// Relocate the focused leaf into the target workspace (content travels;
    /// works for every pane kind). Spec-workspaces-tagging.md Phase 1.
    Move,
    /// Open a second view onto the focused file-backed pane's file in the
    /// target workspace (file-backed panes only). The original stays put.
    AlsoShow,
}

/// Picker overlay for "move pane to workspace" / "also-show pane in
/// workspace". Lists existing workspaces by display label, plus a trailing
/// "+ new workspace" entry that creates an empty workspace as the target.
/// The currently-active workspace is shown but selecting it is a no-op
/// (you can't move a pane to where it already lives).
struct WorkspacePicker {
    mode: WorkspacePickerMode,
    /// Index into the entry list: `0..tabs.len()` are existing workspaces,
    /// `tabs.len()` is the "+ new workspace" entry.
    selected: usize,
}

/// Single-line input overlay used by both Claude-session rename and
/// tab rename. Pre-filled with the current label; Enter commits, Esc
/// cancels, empty input cancels.
struct RenameOverlay {
    text: String,
    target: RenameTarget,
}

#[derive(Clone, Copy)]
enum RenameTarget {
    /// Claude session — targeted by monotonic `AgentSlot::index` so a
    /// concurrent `claude-close` on another slot doesn't rename the
    /// wrong one.
    AgentSlot { index: usize },
    /// Workspace tab — targeted by current tab position. Tab indices
    /// don't shift during the rename's lifetime since the overlay
    /// captures key dispatch (no structural mutations possible mid-
    /// rename), so positional addressing is safe here.
    Tab { index: usize },
    /// Path-input overlay that, on commit, creates a new agent session
    /// rooted at the typed path. Empty input cancels (spec-agent-cwd.md
    /// §2 — bare `:claude-new` already exists and uses the process cwd).
    AgentNewSessionCwd,
    /// Path-input overlay that, on commit, changes the active slot's
    /// cwd (spec-agent-cwd.md §4). Targeted by monotonic
    /// `AgentSlot::index`, matching `AgentSlot`'s rule.
    AgentChangeCwd { index: usize },
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
#[derive(Default)]
enum ActiveOverlay {
    #[default]
    None,
    Menu(MenuOverlay),
    BufferSwitcher(BufferSwitcher),
    SessionSwitcher(SessionSwitcher),
    WorkspacePicker(WorkspacePicker),
    Rename(RenameOverlay),
    TagInput(TagInputOverlay),
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
    // Pane-scoped entries (edit views, reload, claude session management)
    // live in the `.` local menus; rails live on their global chords
    // (Cmd-B / Cmd-Shift-O / Cmd-Shift-B). The global menu is workspace-
    // scoped only: create panels, arrange windows, manage workspaces,
    // layouts, quit.
    vec![
        MenuNode::submenu(
            "n",
            "new",
            vec![
                MenuNode::entry("f", "file browser pane", "new-browser-pane"),
                MenuNode::entry("b", "buffer list", "buffer-list"),
                MenuNode::entry("c", "claude session pane", "new-agent-pane"),
            ],
        ),
        MenuNode::submenu(
            "w",
            "windows",
            vec![
                MenuNode::label("Split"),
                MenuNode::entry("s", "split horizontal (Ctrl-W s)", "split-h"),
                MenuNode::entry("v", "split vertical (Ctrl-W v)", "split-v"),
                MenuNode::entry("c", "close pane (Cmd-W / Ctrl-W c)", "close-window"),
                MenuNode::entry("o", "only this pane (Ctrl-W o)", "only-window"),
                MenuNode::separator(),
                MenuNode::label("Focus"),
                MenuNode::entry("h", "focus left (Ctrl-W h)", "focus-left"),
                MenuNode::entry("l", "focus right (Ctrl-W l)", "focus-right"),
                MenuNode::entry("k", "focus up (Ctrl-W k)", "focus-up"),
                MenuNode::entry("j", "focus down (Ctrl-W j)", "focus-down"),
                MenuNode::entry("n", "focus next (Ctrl-W w)", "focus-next"),
                MenuNode::entry("p", "focus prev (Ctrl-W W)", "focus-prev"),
                MenuNode::separator(),
                MenuNode::label("Size"),
                MenuNode::entry("-", "shrink (Ctrl-W -)", "resize-shrink"),
                MenuNode::entry("+", "grow (Ctrl-W +)", "resize-grow"),
                MenuNode::entry("=", "equalize (Ctrl-W =)", "equalize"),
            ],
        ),
        MenuNode::submenu(
            "s",
            "workspace",
            vec![
                MenuNode::entry("t", "new workspace (Cmd-T)", "new-tab"),
                MenuNode::entry("x", "close workspace (Cmd-Shift-W)", "close-tab"),
                MenuNode::entry("]", "next workspace (Ctrl-Tab)", "next-tab"),
                MenuNode::entry("[", "prev workspace (Ctrl-Shift-Tab)", "prev-tab"),
                MenuNode::entry("r", "rename workspace (Cmd-Shift-R)", "rename-tab"),
                MenuNode::entry("m", "move pane to workspace (Ctrl-W m)", "move-pane"),
                MenuNode::entry(
                    "M",
                    "also-show pane in workspace (Ctrl-W M)",
                    "also-show-pane",
                ),
            ],
        ),
        MenuNode::submenu(
            "l",
            "layout",
            vec![
                MenuNode::label("Layout"),
                MenuNode::entry("l", "cycle layout mode (Ctrl-W Space)", "cycle-layout"),
                MenuNode::entry("p", "promote to master (Ctrl-W Enter)", "promote-master"),
                MenuNode::entry("i", "increase master count (Ctrl-W i)", "inc-master"),
                MenuNode::entry("d", "decrease master count (Ctrl-W d)", "dec-master"),
                MenuNode::separator(),
                MenuNode::label("Marks"),
                MenuNode::entry("'", "list marks", "list-marks"),
                MenuNode::separator(),
                MenuNode::label("Tags"),
                MenuNode::entry("t", "tag buffer", "tag-add"),
                MenuNode::entry("u", "untag buffer", "tag-remove"),
                MenuNode::entry("v", "view tag", "tag-view"),
                MenuNode::entry("a", "also-tag buffer", "tag-also"),
                MenuNode::entry("x", "tag + view", "tag-send"),
                MenuNode::entry("k", "bind tag shortcut", "tag-bind"),
            ],
        ),
        MenuNode::separator(),
        MenuNode::entry("v", "back to doc", "back-to-doc"),
        MenuNode::separator(),
        MenuNode::entry("q", "quit", "quit"),
    ]
}

// ---- Local menus (spec-menu-scopes.md Behavior 2) --------------------------
//
// One static tree per content kind, opened with the `.` local leader. Same
// overlay machinery as the global menu — only the tree (and header) differ.
// v1 contains only entries with existing GPUI backing; the spec's nav-*
// (links/headings/list-items/code-blocks) and browser-open-* entries are
// deferred until those features exist.

fn doc_local_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("e", "edit (raw markdown)", "enter-edit"),
        MenuNode::entry("w", "edit (word processor)", "enter-wp"),
        MenuNode::entry("r", "reload from disk", "reload-file"),
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
        MenuNode::entry("n", "new session", "claude-new"),
        MenuNode::entry("l", "list sessions", "claude-list"),
        MenuNode::entry("x", "close session", "claude-close"),
        MenuNode::entry("r", "rename session", "claude-rename"),
        MenuNode::separator(),
        MenuNode::entry("w", "toggle worksheet/chatbox", "agent-input-toggle"),
        MenuNode::entry("s", "send buffer", "claude-send"),
        MenuNode::entry("S", "send selection", "claude-send-selection"),
        MenuNode::entry("m", "cycle permission mode", "claude-mode-cycle"),
        MenuNode::entry("d", "detach", "claude-detach"),
        MenuNode::entry("a", "attach", "claude-attach"),
        MenuNode::separator(),
        // Build loop (moved here from the global claude submenu — these act
        // on the focused session, so they're pane-scoped).
        MenuNode::entry("p", "promote (build candidate)", "dev-build-candidate"),
        MenuNode::entry("P", "take over sessions (candidate)", "dev-take-over"),
        MenuNode::entry("g", "rebuild & restart gui", "dev-restart-gui"),
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

struct SketchGpuiView {
    theme: Theme,
    body_font: SharedString,
    code_font: SharedString,
    /// Multiplier applied to document body / heading font sizes (Cmd+= / Cmd+-
    /// / Cmd+0). Chrome (status bar, tabs, file browser) stays fixed. 1.0 is
    /// the unzoomed default; clamped to [MIN_TEXT_SCALE, MAX_TEXT_SCALE] on
    /// every adjustment.
    text_scale: f32,
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
    /// Tabs + n-ary split tree (spec-tabs-and-splits.md). The focused
    /// window's content is the authoritative live state for the workspace.
    workspace: workspace::Workspace<WindowContent>,
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
    /// restarts). Activated by `SKETCH_SESSION_SERVER=1`. When `None`, the
    /// GUI spawns `AcpChannelClient` directly (legacy path).
    session_server: Option<SessionServerClient>,
    /// Where the agent info bar renders: above or below the transcript.
    agent_status_position: AgentStatusPosition,
    /// True when this instance was launched as a build-loop *candidate*
    /// (`SKETCH_CANDIDATE=1`). A candidate attaches to live sessions as a
    /// read-only `Observer` (mirrors the transcript, can't drive), shows a
    /// banner, and refuses to submit prompts until it takes over — which it
    /// can do only once the original owner window closes. See
    /// `build_and_launch_candidate` / `candidate_take_over`.
    is_candidate: bool,
    /// For a candidate: whether the mirrored sessions have been released by
    /// the original holder (i.e. a `LeaseChanged{lease:None}` arrived, or the
    /// lease is now held by our own client_id), meaning take-over will now
    /// succeed. Drives the banner color. Purely a display hint —
    /// `candidate_take_over` re-checks authoritatively.
    candidate_promote_ready: bool,
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
    /// and the agent transcript pane. Loaded once at startup.
    syntect_hl: sketch::highlight::Highlighter,
    /// The lease-heartbeat beater (spec phase 4). A SINGLETON tied to this
    /// view's lifetime: spawned at most once (on the first `start_server_pump`
    /// that finds this `None`) and self-gates per-tick on `slot.is_driver`, so
    /// one beater covers every driven session in this window for the window's
    /// life. Subsequent `start_server_pump` calls (re-opening the Claude
    /// screen) find this `Some` and DON'T spawn another — fixing the leak where
    /// each open spawned a fresh detached beater, so a lease-loss event fanned
    /// out into K redundant Owner re-attaches. Dropped (and thus cancelled)
    /// with the view.
    _lease_heartbeat: Option<Task<()>>,
}

impl SketchGpuiView {
    fn new_doc(
        blocks: Vec<RenderedBlock>,
        theme: Theme,
        file_label: String,
        focus_handle: FocusHandle,
    ) -> Self {
        let label: SharedString = file_label.into();
        let initial = WindowContent::Doc(DocState {
            blocks,
            file_label: label.clone(),
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            source: None,
        });
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            viewport_width_px: 800.0,
            focus_handle,
            active_overlay: ActiveOverlay::None,
            transient_status: None,
            workspace: workspace::Workspace::with_initial(initial),
            doc_selection: None,
            line_layouts: Rc::new(RefCell::new(HashMap::new())),
            session_server: connect_session_server(),
            agent_status_position: AgentStatusPosition::default(),
            is_candidate: is_candidate_launch(),
            candidate_promote_ready: false,
            splash_until: Some(std::time::Instant::now() + Duration::from_millis(1500)),
            syntect_hl: sketch::highlight::Highlighter::new(),
            _lease_heartbeat: None,
            pending_mark_chord: None,
            pending_tag_chord: None,
        }
    }

    fn new_browser(start_dir: PathBuf, theme: Theme, focus_handle: FocusHandle) -> Self {
        let initial = WindowContent::Browser(BrowserWindow::standalone(start_dir));
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            viewport_width_px: 800.0,
            focus_handle,
            active_overlay: ActiveOverlay::None,
            transient_status: None,
            workspace: workspace::Workspace::with_initial(initial),
            doc_selection: None,
            line_layouts: Rc::new(RefCell::new(HashMap::new())),
            session_server: connect_session_server(),
            agent_status_position: AgentStatusPosition::default(),
            is_candidate: is_candidate_launch(),
            candidate_promote_ready: false,
            splash_until: Some(std::time::Instant::now() + Duration::from_millis(1500)),
            syntect_hl: sketch::highlight::Highlighter::new(),
            _lease_heartbeat: None,
            pending_mark_chord: None,
            pending_tag_chord: None,
        }
    }

    /// Replace the focused window's content (old `self.screen = X` writes).
    fn set_screen(&mut self, content: WindowContent) {
        self.workspace.replace_focused_content(content);
    }

    /// Persist the current workspace snapshot for the active cwd. Called
    /// after every structural mutation (tab add/remove, split, close,
    /// focus change, etc.). Best-effort — failures are silent so a
    /// read-only cache_dir or full disk doesn't break the editor.
    fn save_workspace_state(&mut self) {
        // Reap pooled buffers no window references anymore. This is the buffer
        // pool's liveness sweep — called after every structural mutation, so a
        // closed/relocated Edit pane's clean buffer is dropped promptly while
        // dirty ones stay pooled for recovery.
        self.workspace.gc_buffers();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        save_persisted_workspace(&cwd, &self.workspace);
    }

    /// Replace `self.workspace` with one rebuilt from the persisted snapshot
    /// for `cwd`, if any. Doc/Edit windows reload their files; Browser
    /// windows reattach to their saved dir; Claude windows are temporarily
    /// restored as Browser stubs, then replaced with live agent sessions in
    /// a post-pass. Returns `true` if a snapshot was loaded.
    fn restore_workspace_from_disk(&mut self, cx: &mut Context<Self>) -> bool {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let Some(snap) = load_persisted_workspace(&cwd) else {
            return false;
        };
        let mut ws: workspace::Workspace<WindowContent> = workspace::Workspace::new();
        let mut agent_leaf_ids: Vec<workspace::WindowId> = Vec::new();
        for ptab in snap.tabs {
            let (layout, max_id, agents) = restore_layout(&mut ws, &self.theme, ptab.layout);
            ws.next_window_id = ws.next_window_id.max(max_id + 1);
            agent_leaf_ids.extend(agents);
            ws.tabs.push(workspace::Tab {
                auto_name: ptab.auto_name,
                display_name: ptab.display_name,
                focused: ptab.focused_window,
                layout,
                rail: ptab.rail.map(|r| restore_rail(r, ptab.focused_window)),
                layout_mode: ptab.layout_mode,
                saved_manual_layout: None,
                master_ratio: ptab.master_ratio,
                master_count: ptab.master_count,
                tag_view: ptab.tag_view,
            });
            ws.next_tab_index += 1;
        }
        if !ws.tabs.is_empty() {
            ws.active_tab = snap.active_tab.min(ws.tabs.len() - 1);
        }
        if ws.tabs.is_empty() {
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
        // If a tab was saved in automatic layout mode, retile now.
        for t in &mut ws.tabs {
            if t.layout_mode != workspace::LayoutMode::Manual {
                // Retile in-place for non-manual tabs.
                let windows: Vec<workspace::Window<WindowContent>> =
                    workspace::drain_leaves(&mut t.layout);
                if !windows.is_empty() {
                    let focused = t.focused;
                    t.layout = match t.layout_mode {
                        workspace::LayoutMode::MasterStack => {
                            workspace::build_master_stack(windows, t.master_count, t.master_ratio)
                        }
                        workspace::LayoutMode::Monocle => workspace::build_monocle(windows),
                        workspace::LayoutMode::Columns => workspace::build_columns(windows),
                        workspace::LayoutMode::Manual => unreachable!(),
                    };
                    // Restore focus
                    if t.layout.find_leaf(focused).is_some() {
                        t.focused = focused;
                    }
                }
            }
        }
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
    fn restore_agent_leaves(&mut self, leaf_ids: &[workspace::WindowId], cx: &mut Context<Self>) {
        let proc_cwd = process_cwd();

        for &leaf_id in leaf_ids {
            let mut ring = AgentRing::new(None);

            if self.session_server.is_some() {
                // Session-server path: placeholder + async attach.
                let placeholder =
                    AgentState::new_server_managed(Some("connecting to session server…".into()));
                let open_token = alloc_open_token();
                ring.push("claude-1".into(), placeholder, None, proc_cwd.clone(), None);
                let server_pump = self.start_server_pump(cx);
                if let Some(slot) = ring.slots.first_mut() {
                    slot.state._pump = Some(server_pump);
                    slot.pending_open_token = Some(open_token);
                }
                // Install the ring, then kick off async attach.
                for tab in &mut self.workspace.tabs {
                    if let Some(win) = tab.layout.find_leaf_mut(leaf_id) {
                        win.content = WindowContent::Agent(ring);
                        break;
                    }
                }
                self.spawn_open_agent_server(open_token, proc_cwd.clone(), cx);
            } else {
                // Legacy direct-spawn path.
                let persisted = load_persisted_acp_sessions(&proc_cwd);
                if persisted.is_empty() {
                    let slot_cwd = proc_cwd.clone();
                    let session_index = ring.next_index;
                    let state =
                        self.create_agent_session(None, slot_cwd.clone(), session_index, cx);
                    ring.push("claude-1".into(), state, None, slot_cwd, None);
                } else {
                    let active_pos = persisted.iter().position(|s| s.active).unwrap_or(0);
                    for slot in persisted {
                        let slot_cwd = slot.cwd.clone().unwrap_or_else(|| proc_cwd.clone());
                        let session_index = ring.next_index;
                        let mut state = self.create_agent_session(
                            Some(slot.id.clone()),
                            slot_cwd.clone(),
                            session_index,
                            cx,
                        );
                        if slot.mode == InputModeKind::Worksheet {
                            state.input_surface = InputSurface::Worksheet;
                        }
                        state.tasklist_open = slot.tasklist_open;
                        state.subagents_open = slot.subagents_open;
                        ring.push(slot.label, state, Some(slot.id), slot_cwd, None);
                    }
                    ring.active = active_pos.min(ring.slots.len().saturating_sub(1));
                }
                for tab in &mut self.workspace.tabs {
                    if let Some(win) = tab.layout.find_leaf_mut(leaf_id) {
                        win.content = WindowContent::Agent(ring);
                        break;
                    }
                }
            }
        }
        cx.notify();
    }

    /// `Some(doc)` if currently viewing a document, else `None`.
    fn doc_mut(&mut self) -> Option<&mut DocState> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            WindowContent::Doc(d) => Some(d),
            _ => None,
        }
    }

    fn browser_mut(&mut self) -> Option<&mut BrowserWindow> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            WindowContent::Browser(b) => Some(b),
            _ => None,
        }
    }

    fn agent_mut(&mut self) -> Option<&mut AgentState> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            WindowContent::Agent(ring) if !ring.is_empty() => Some(&mut ring.active_mut().state),
            _ => None,
        }
    }

    /// Return the active slot's server session id (cloned), or `None`.
    fn active_server_session_id(&self) -> Option<String> {
        self.agent_ring()
            .and_then(|r| r.slots.get(r.active))
            .and_then(|s| s.server_session_id.clone())
    }

    fn agent_ring(&self) -> Option<&AgentRing> {
        match self.workspace.focused_content().expect("no focused window") {
            WindowContent::Agent(ring) => Some(ring),
            _ => None,
        }
    }

    fn agent_ring_mut(&mut self) -> Option<&mut AgentRing> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            WindowContent::Agent(ring) => Some(ring),
            _ => None,
        }
    }

    /// Open `path` as a doc. If it's already in a tab, switch to that tab.
    /// Otherwise push a new tab containing the doc. Returns false on read error.
    /// Build a Doc `WindowContent` for `path`, bound to the shared buffer
    /// pool (5c: dedup by canonical path so Edit views of the same file
    /// share the exact same rope + undo and edits show live in this Doc).
    /// `None` if the file can't be read.
    fn make_doc_content(&mut self, path: &std::path::Path) -> Option<WindowContent> {
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
        Some(WindowContent::Doc(DocState {
            blocks,
            file_label: canon.into(),
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            source: Some(DocSource::new(buf_id, core)),
        }))
    }

    fn open_file(&mut self, path: PathBuf) -> bool {
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();

        // Already open? Switch to that tab.
        if let Some(idx) = self.find_tab_by_doc_label(&canon) {
            if idx != self.workspace.active_tab {
                self.workspace.active_tab = idx;
            }
            return true;
        }

        let Some(new_content) = self.make_doc_content(&path) else {
            return false;
        };

        // If the current tab is a transient Browser, replace its content
        // (matches today's "browser disappears when you pick a file"). For
        // Doc/Edit/Claude, push a new tab so the existing work isn't lost.
        let replace_in_place = matches!(
            self.workspace.focused_content(),
            Some(WindowContent::Browser(_))
        );
        if replace_in_place {
            self.set_screen(new_content);
        } else {
            self.workspace.push_initial_tab(new_content);
        }
        self.save_workspace_state();
        true
    }

    /// Find a tab whose focused content is a Doc/Edit with the given file
    /// label. Returns the tab index, or None.
    fn find_tab_by_doc_label(&self, label: &str) -> Option<usize> {
        for (i, tab) in self.workspace.tabs.iter().enumerate() {
            if let workspace::Layout::Leaf(w) = &tab.layout {
                match &w.content {
                    WindowContent::Doc(d) if d.file_label.as_ref() == label => return Some(i),
                    WindowContent::Edit(e) if e.file_label.as_ref() == label => return Some(i),
                    _ => {}
                }
            }
        }
        None
    }

    /// Switch the workspace to the tab at `idx`. Used by the buffer-list
    /// picker. No-op if idx is out of range.
    fn switch_to_buffer(&mut self, idx: usize) {
        if idx >= self.workspace.tabs.len() || idx == self.workspace.active_tab {
            return;
        }
        self.workspace.active_tab = idx;
    }

    /// Close the tab at `idx`. Returns false if the tab's content has unsaved
    /// modifications (refusing to close). If it's the last tab, quits.
    fn close_buffer_at(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        if idx >= self.workspace.tabs.len() {
            return true;
        }
        // Check if the tab's focused content is modified.
        let is_modified = match &self.workspace.tabs[idx].layout {
            workspace::Layout::Leaf(w) => screen_is_modified(&w.content),
            _ => false,
        };
        if is_modified {
            return false;
        }
        if self.workspace.tabs.len() <= 1 {
            cx.quit();
            return true;
        }
        self.workspace.close_tab(idx);
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

    fn open_browser_inner(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.workspace.focused_content().expect("no focused window"),
            WindowContent::Browser(_)
        ) {
            return;
        }
        // Open the browser IN the focused pane, stashing the prior content
        // on `BrowserWindow.underlying` so Esc/q restores it. Picking a file
        // discards the underlying and replaces the browser with the picked
        // file in this same pane (see `open_file`'s `replace_in_place`
        // branch). This keeps the browser pane-scoped instead of tab-
        // scoped so splits/tabs aren't disrupted by file picking.
        let placeholder = WindowContent::Browser(BrowserWindow::standalone(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ));
        let prior = self
            .workspace
            .replace_focused_content(placeholder)
            .expect("workspace has no focused window");
        self.set_screen(WindowContent::Browser(BrowserWindow {
            fb: FileBrowser::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            underlying: Some(Box::new(prior)),
        }));
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
                if let Some(c) = self.agent_mut() {
                    c.status = Some(format!("restart failed: {e}").into());
                }
            }
        }
    }

    /// Set the focused agent slot's status line (build-loop feedback). No-op
    /// if the focused window isn't an agent screen, but always logs so the
    /// message isn't lost when triggered from a doc/edit view.
    fn set_agent_status(&mut self, msg: &str, cx: &mut Context<Self>) {
        eprintln!("[sketch-gpui] {msg}");
        if let Some(c) = self.agent_mut() {
            c.status = Some(msg.to_string().into());
        }
        cx.notify();
    }

    /// Build-loop step 1 (the `promote` command). Compile `sketch-gpui`, and
    /// on success spawn the freshly built binary as a read-only *candidate*
    /// (`SKETCH_CANDIDATE=1`) **without quitting** this instance. Both
    /// processes share the running session server, so the candidate mirrors
    /// every live ACP session. Verify the candidate, then close this window
    /// to hand off ownership; the candidate takes over with full transcripts
    /// intact. The session server binary is intentionally left untouched —
    /// only the GUI is hot-swapped, so agents never restart.
    fn build_and_launch_candidate(&mut self, cx: &mut Context<Self>) {
        if self.is_candidate {
            self.set_agent_status("already running as a candidate", cx);
            return;
        }
        if self.session_server.is_none() {
            self.set_agent_status(
                "session server not active — relaunch with SKETCH_SESSION_SERVER=1",
                cx,
            );
            return;
        }
        self.set_agent_status("building candidate: cargo build --bin sketch-gpui…", cx);

        let manifest_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let exe = std::env::current_exe().ok();
        let args: Vec<String> = std::env::args().skip(1).collect();

        cx.spawn(async move |this, cx| {
            // Run the (slow, blocking) build on a background thread.
            let built = cx
                .background_executor()
                .spawn(async move {
                    std::process::Command::new("cargo")
                        .args(["build", "--bin", "sketch-gpui"])
                        .current_dir(&manifest_dir)
                        .output()
                })
                .await;

            let _ = this.update(cx, |this, cx| match built {
                Ok(out) if out.status.success() => match exe {
                    Some(exe) => {
                        let mut cmd = std::process::Command::new(exe);
                        cmd.args(&args);
                        cmd.env("SKETCH_CANDIDATE", "1");
                        // Phase-4 blue-green: give the candidate a DISTINCT
                        // per-launch client_id so it is a different lease client
                        // from the original. It lands as Observer while the
                        // original's lease is live, then Promotes under this
                        // fresh id once the original cleanly exits. (If it read
                        // the on-disk id it would impersonate the original and
                        // steal the lease.)
                        cmd.env("SKETCH_CLIENT_ID", uuid::Uuid::new_v4().to_string());
                        cmd.stdin(std::process::Stdio::null());
                        cmd.stdout(std::process::Stdio::null());
                        cmd.stderr(std::process::Stdio::inherit());
                        match cmd.spawn() {
                            Ok(_) => this.set_agent_status(
                                "candidate launched — verify it, then close this window to hand off",
                                cx,
                            ),
                            Err(e) => {
                                this.set_agent_status(&format!("candidate spawn failed: {e}"), cx)
                            }
                        }
                    }
                    None => this.set_agent_status("cannot locate current executable", cx),
                },
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

    /// Dev hot-restart: rebuild `sketch-gpui` and replace THIS instance with a
    /// fresh NORMAL one. Distinct from `build_and_launch_candidate`: that spawns
    /// a read-only candidate (SKETCH_CANDIDATE=1) that co-attaches and waits for
    /// the user to hand off; this is a full self-restart — build, spawn a fresh
    /// normal instance (no SKETCH_CANDIDATE), then quit so the new process
    /// reclaims ownership of every server-managed session via the deterministic
    /// same-`client_id` `attach_for_role` path (the new instance presents the
    /// SAME stable client_id, so the server resumes its lease on the first
    /// attach). The session server (and its live agent sessions) is left
    /// running, so agents survive the bounce.
    ///
    /// NEEDS-RUNTIME: GPUI can't be driven headlessly, so this is compile-
    /// verified only — the actual rebuild/relaunch/owner-reclaim must be
    /// checked by a human.
    fn dev_rebuild_restart_gui(&mut self, cx: &mut Context<Self>) {
        self.set_agent_status("rebuilding gui: cargo build --bin sketch-gpui…", cx);

        let manifest_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let exe = std::env::current_exe().ok();
        let args: Vec<String> = std::env::args().skip(1).collect();

        cx.spawn(async move |this, cx| {
            // Run the (slow, blocking) build on a background thread.
            let built = cx
                .background_executor()
                .spawn(async move {
                    std::process::Command::new("cargo")
                        .args(["build", "--bin", "sketch-gpui"])
                        .current_dir(&manifest_dir)
                        .output()
                })
                .await;

            let _ = this.update(cx, |this, cx| match built {
                Ok(out) if out.status.success() => match exe {
                    Some(exe) => {
                        // Fresh NORMAL instance: same args, NO SKETCH_CANDIDATE —
                        // this is an ordinary relaunch, not a read-only candidate.
                        // Command inherits the full parent env, so we must
                        // explicitly REMOVE the candidate marker (if this very
                        // process was itself launched as a candidate) — otherwise
                        // the "fresh normal instance" would inherit and come up
                        // read-only.
                        let mut cmd = std::process::Command::new(exe);
                        cmd.args(&args);
                        cmd.env_remove("SKETCH_CANDIDATE");
                        cmd.stdin(std::process::Stdio::null());
                        cmd.stdout(std::process::Stdio::null());
                        // stderr inherited so post-restart logs reach the dev
                        // terminal (unlike reboot_into_claude's fully-detached
                        // stdio).
                        cmd.stderr(std::process::Stdio::inherit());
                        match cmd.spawn() {
                            Ok(_) => {
                                this.set_agent_status(
                                    "rebuilt — relaunching gui, this window will close",
                                    cx,
                                );
                                // Quit promptly: the new instance's deterministic
                                // same-client_id attach_for_role reclaim handles
                                // the teardown race, so we don't need to linger.
                                cx.quit();
                            }
                            Err(e) => {
                                this.set_agent_status(&format!("relaunch spawn failed: {e}"), cx)
                            }
                        }
                    }
                    None => this.set_agent_status("cannot locate current executable", cx),
                },
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

    /// Build-loop step 2 (the candidate's "take over"). Claim ownership of
    /// every session this candidate is mirroring. Succeeds only once the
    /// original owner window has closed (the server reports the sessions as
    /// ownerless); otherwise reports which sessions are still held so the
    /// user knows to close the original first. On success the candidate
    /// sheds its read-only state and becomes the live driver.
    fn candidate_take_over(&mut self, cx: &mut Context<Self>) {
        if self.session_server.is_none() {
            self.set_agent_status("session server not active", cx);
            return;
        }
        let sids: Vec<String> = match self.agent_ring() {
            Some(r) => r
                .slots
                .iter()
                .filter_map(|s| s.server_session_id.clone())
                .collect(),
            None => Vec::new(),
        };
        if sids.is_empty() {
            self.set_agent_status("no mirrored sessions to take over", cx);
            return;
        }

        let mut failures = Vec::new();
        if let Some(server) = self.session_server.as_ref() {
            for sid in &sids {
                if let Err(e) = server.promote(sid) {
                    failures.push(format!("{}: {e}", &sid[..8.min(sid.len())]));
                }
            }
        }

        if failures.is_empty() {
            self.is_candidate = false;
            self.candidate_promote_ready = false;
            // We now hold the lease on every promoted session. Mark each slot a
            // driver so the (already-running, unconditionally-spawned) lease
            // heartbeat begins beating them on its next tick — without this the
            // freshly-promoted owner would hold the lease but never heartbeat
            // it, and the server would sweep it ~15s later (owner-gap bug).
            for sid in &sids {
                self.for_each_server_session_slot(sid, |slot| {
                    slot.is_driver = true;
                });
            }
            self.set_agent_status(
                &format!("took over {} session(s) — you now own them", sids.len()),
                cx,
            );
        } else {
            self.set_agent_status(
                &format!(
                    "close the original window first — still owned: {}",
                    failures.join(", ")
                ),
                cx,
            );
        }
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
            cx.notify();
        }
    }

    /// Snapshot the persistable UI settings (theme, agent info-bar placement,
    /// text zoom) and write them in ONE place. Each settings mutation just calls
    /// this instead of re-listing every field at its own `save_preferences(...)`
    /// site — the structural cause of "added a setting, forgot to persist it at
    /// one of N sites" drift. Fonts are not yet user-settable, so not persisted.
    fn save_settings(&self) {
        save_preferences(&Preferences {
            theme: Some(self.theme.name.as_kebab().to_string()),
            agent_status_position: Some(self.agent_status_position.as_str().to_string()),
            text_scale: Some(self.text_scale),
        });
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
    fn set_theme(&mut self, name: ThemeName, cx: &mut Context<Self>) {
        if self.theme.name == name {
            return;
        }
        self.theme = Theme::from_name(name);
        for tab in self.workspace.tabs.iter_mut() {
            re_render_layout_docs(&mut tab.layout, &self.theme);
        }
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
        _cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.doc_selection else {
            return;
        };
        let Some(text) = self.collect_doc_selection_text(&sel) else {
            return;
        };
        if !text.is_empty() {
            Self::yank_to_clipboard(&text);
        }
    }

    fn collect_doc_selection_text(&self, sel: &DocSelection) -> Option<String> {
        let (start, end) = sel.normalized();
        let content = self.workspace.focused_content()?;
        let blocks = match content {
            WindowContent::Doc(d) => &d.blocks,
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
        let Some(text) = Self::read_from_clipboard() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        // Find the active editor + mode. Chatbox takes priority in chatbox mode.
        let pasted = match self.workspace.focused_content_mut() {
            Some(WindowContent::Edit(e)) => {
                if e.mode == EditMode::Insert {
                    for ch in text.chars() {
                        e.editor.insert_char(ch);
                    }
                    true
                } else {
                    false
                }
            }
            Some(WindowContent::Agent(ring)) => {
                let c = &mut ring.active_mut().state;
                if c.input_surface.is_chatbox() {
                    if let Some(cb) = c.input_surface.chatbox_mut() {
                        if cb.mode == EditMode::Insert {
                            for ch in text.chars() {
                                cb.editor.insert_char(ch);
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else if c.mode == EditMode::Insert {
                    for ch in text.chars() {
                        c.editor.insert_char(ch);
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
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
            Self::yank_to_clipboard(&text);
            return;
        }
        // Edit / Agent views: copy editor selection.
        let text = match self.workspace.focused_content() {
            Some(WindowContent::Edit(e)) => e.editor.selection_text(),
            Some(WindowContent::Agent(ring)) => {
                let c = &ring.active().state;
                if c.input_surface.is_chatbox() {
                    c.input_surface
                        .chatbox()
                        .and_then(|cb| cb.editor.selection_text())
                } else {
                    c.editor.selection_text()
                }
            }
            _ => None,
        };
        if let Some(t) = text
            && !t.is_empty()
        {
            Self::yank_to_clipboard(&t);
        }
        let _ = cx;
    }

    /// Drop every live `AcpChannelClient` we hold so its `Drop` impl can
    /// run the explicit teardown (signal worker, join thread, kill child)
    /// before the rest of the GPUI shutdown clears windows. Without this,
    /// the join races with `App::shutdown` clearing entities; the worker
    /// usually finishes in time but the order is non-deterministic and
    /// lingering child agents have been observed at exit. Called from
    /// `on_app_quit` in `main`.
    fn shutdown_acp(&mut self) {
        // Walk every Claude window in every tab and drop its channel so the
        // worker thread shuts down its child agent before GPUI's window
        // teardown races with us.
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    for slot in &mut ring.slots {
                        let _dropped = slot.state.channel.take();
                    }
                }
            });
        }
    }

    fn next_buffer(&mut self, _: &NextBuffer, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() > 1 {
            let next = (self.workspace.active_tab + 1) % self.workspace.tabs.len();
            self.switch_to_buffer(next);
            cx.notify();
        }
    }

    fn prev_buffer(&mut self, _: &PrevBuffer, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() > 1 {
            let prev = if self.workspace.active_tab == 0 {
                self.workspace.tabs.len() - 1
            } else {
                self.workspace.active_tab - 1
            };
            self.switch_to_buffer(prev);
            cx.notify();
        }
    }

    fn next_tab(&mut self, _: &NextTab, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() > 1 {
            self.workspace.next_tab();
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Activate the tab at `idx`. Mouse-click entry point from the tab
    /// strip — no-ops if the index is out of range or already active.
    fn select_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.tabs.len() || idx == self.workspace.active_tab {
            return;
        }
        self.workspace.active_tab = idx;
        self.save_workspace_state();
        cx.notify();
    }

    fn prev_tab(&mut self, _: &PrevTab, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() > 1 {
            self.workspace.prev_tab();
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Open a new tab containing a Browser rooted at cwd. Spec Behavior 3:
    /// no-arg `:tabnew` / `Cmd-T` creates a browser tab so the user can pick
    /// what to load.
    fn new_tab(&mut self, _: &NewTab, _w: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .push_initial_tab(WindowContent::Browser(BrowserWindow::standalone(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            )));
        self.save_workspace_state();
        cx.notify();
    }

    /// Close the active tab. Spec Behavior 5: ClaudeWindows drop their ACP
    /// channels (subprocess killed via kill_on_drop). When the last tab is
    /// closed, quit the app for now (placeholder-tab Behavior 2 is a
    /// follow-up).
    fn close_tab(&mut self, _: &CloseTab, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() <= 1 {
            cx.quit();
            return;
        }
        let idx = self.workspace.active_tab;
        self.workspace.close_tab(idx);
        self.save_workspace_state();
        cx.notify();
    }

    fn rename_tab(&mut self, _: &RenameTab, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_rename_active_tab_overlay(cx);
    }

    /// `Ctrl-W m` — open the workspace picker to MOVE the focused pane.
    fn move_pane(&mut self, _: &MovePane, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_picker(WorkspacePickerMode::Move, cx);
    }

    /// `Ctrl-W M` — open the workspace picker to ALSO-SHOW the focused pane.
    fn also_show_pane(&mut self, _: &AlsoShowPane, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_picker(WorkspacePickerMode::AlsoShow, cx);
    }

    /// `Ctrl-W s` — horizontal split: new pane below the focused one.
    fn split_h(&mut self, _: &SplitH, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::H);
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W v` — vertical split: new pane to the right of the focused one.
    fn split_v(&mut self, _: &SplitV, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::V);
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    /// Shared helper. The new pane mirrors the focused content kind:
    ///
    /// - Doc → new Doc over the same file (independent scroll/cursor).
    /// - Edit → new Edit over the same file path; the new editor reads
    ///   from disk so unsaved changes in the source pane don't carry over
    ///   (a shared buffer pool would fix that — separate stage).
    /// - Browser → new Browser at cwd.
    /// - Claude → new Browser at cwd (Claude is exclusive per spec).
    ///
    /// Browser is the universal fallback when the focused content has no
    /// natural file pane to clone (Claude) or when reading the source
    /// file fails.
    fn split_focused_with_browser(&mut self, dir: workspace::SplitDir) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let content = self.clone_focused_for_split(&cwd);
        let _ = self.workspace.split_focused(dir, content);
    }

    fn clone_focused_for_split(&mut self, cwd: &std::path::Path) -> WindowContent {
        let (label, is_edit) = match self.workspace.focused_content() {
            Some(WindowContent::Doc(d)) => (Some(d.file_label.clone()), false),
            Some(WindowContent::Edit(e)) => (Some(e.file_label.clone()), true),
            _ => (None, false),
        };
        let browser_fallback =
            || WindowContent::Browser(BrowserWindow::standalone(cwd.to_path_buf()));
        let Some(label) = label else {
            return browser_fallback();
        };
        let path = PathBuf::from(label.as_ref());
        if is_edit {
            // Bind the new pane to the SAME pooled core as the source pane:
            // open_and_retain returns the existing buffer id, so unsaved text
            // + undo are shared. Only cursor/scroll/selection are independent.
            match self.workspace.open_and_retain(&path) {
                Ok((id, core)) => WindowContent::Edit(EditState::new(
                    SharedEditor::new(id, core),
                    label,
                    EditView::Code,
                )),
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
                    WindowContent::Doc(DocState {
                        blocks,
                        file_label: label,
                        cursor_block: 0,
                        list_state: DocState::new_list_state(0),
                        list_item_count: std::cell::Cell::new(0),
                        blocks_seq: 0,
                        blocks_snapshot: RefCell::new(None),
                        last_cursor_block: std::cell::Cell::new(None),
                        source: Some(DocSource::new(id, core)),
                    })
                }
                Err(_) => browser_fallback(),
            }
        }
    }

    /// `Ctrl-W c` — close the focused window. If it was the only window in
    /// the tab, close the tab instead.
    fn close_window(&mut self, _: &CloseWindow, _w: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.close_focused() {
            Ok(Some(_new_focus)) => {
                self.workspace.retile_active();
                self.save_workspace_state();
                cx.notify();
            }
            Ok(None) => {
                // Focused leaf is the only one in its tab. Close the tab
                // if there are other tabs; otherwise no-op — closing the
                // absolute last pane would leave the app with nothing to
                // render. Cmd-Q is the only quit path now.
                if self.workspace.tabs.len() <= 1 {
                    return;
                }
                let idx = self.workspace.active_tab;
                self.workspace.close_tab(idx);
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

    /// `Ctrl-W <` / `Ctrl-W -` — shrink the focused pane by 5% (gives the
    /// space to its next sibling within the parent split).
    fn resize_shrink(&mut self, _: &ResizeShrink, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.resize_focused(-0.05);
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W >` / `Ctrl-W +` — grow the focused pane by 5%.
    fn resize_grow(&mut self, _: &ResizeGrow, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.resize_focused(0.05);
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W =` — even out all sibling weights in the focused pane's
    /// parent split.
    fn equalize(&mut self, _: &Equalize, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.equalize_focused();
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

    /// Jump focus to a specific window (cross-tab). Updates `prev_jump`.
    fn jump_to_window(&mut self, target_id: workspace::WindowId) {
        let Some(tab_idx) = self.workspace.tab_containing(target_id) else {
            // Stale mark — GC
            let live = self.workspace.all_window_ids();
            self.workspace.marks.gc(&live);
            self.transient_status = Some("mark target no longer exists".into());
            return;
        };

        let current_wid = self.workspace.focused_window_id();
        let cross_tab = tab_idx != self.workspace.active_tab;

        if cross_tab {
            if let Some(wid) = current_wid {
                self.workspace.marks.prev_jump = Some(wid);
            }
            self.workspace.active_tab = tab_idx;
        }

        if let Some(tab) = self.workspace.active_tab_mut() {
            tab.focused = target_id;
        }
    }

    // Phase 2: automatic layouts

    fn cycle_layout_mode(&mut self, _: &CycleLayoutMode, _w: &mut Window, cx: &mut Context<Self>) {
        let new_mode = self
            .workspace
            .active_tab()
            .map(|t| t.layout_mode.cycle())
            .unwrap_or(workspace::LayoutMode::Manual);
        self.workspace.set_layout_mode(new_mode);
        let sigil = new_mode.sigil();
        self.transient_status = Some(format!("layout: {sigil}").into());
        self.save_workspace_state();
        cx.notify();
    }

    fn promote_to_master(&mut self, _: &PromoteToMaster, _w: &mut Window, cx: &mut Context<Self>) {
        let is_master_stack = self
            .workspace
            .active_tab()
            .map(|t| t.layout_mode == workspace::LayoutMode::MasterStack)
            .unwrap_or(false);
        if !is_master_stack {
            return;
        }
        // Swap focused window with master (first in tree order).
        let tab = match self.workspace.active_tab_mut() {
            Some(t) => t,
            None => return,
        };
        let ids = tab.layout.leaf_ids();
        if ids.len() < 2 {
            return;
        }
        let focused = tab.focused;
        let master_id = ids[0];
        if focused == master_id {
            return;
        }
        // Swap the content of the two leaves in place.
        tab.layout.swap_leaf_contents(focused, master_id);
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    fn increase_master_count(
        &mut self,
        _: &IncreaseMasterCount,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.workspace.active_tab_mut() {
            if tab.layout_mode != workspace::LayoutMode::MasterStack {
                return;
            }
            let max = tab.layout.leaf_count().saturating_sub(1).max(1);
            tab.master_count = (tab.master_count + 1).min(max);
        }
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
    }

    fn decrease_master_count(
        &mut self,
        _: &DecreaseMasterCount,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.workspace.active_tab_mut() {
            if tab.layout_mode != workspace::LayoutMode::MasterStack {
                return;
            }
            tab.master_count = tab.master_count.saturating_sub(1).max(1);
        }
        self.workspace.retile_active();
        self.save_workspace_state();
        cx.notify();
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
        if let Some(tab) = self.workspace.active_tab_mut() {
            tab.tag_view.clear();
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
                if let Some(tab) = self.workspace.active_tab_mut() {
                    tab.tag_view.clear();
                    tab.tag_view.insert(tag_name.clone());
                }
                self.adjust_focus_for_tag_view();
                self.transient_status = Some(format!("viewing tag: {tag_name}").into());
            }
            'T' => {
                // Toggle tag in view
                if let Some(tab) = self.workspace.active_tab_mut() {
                    if tab.tag_view.contains(&tag_name) {
                        tab.tag_view.remove(&tag_name);
                    } else {
                        tab.tag_view.insert(tag_name.clone());
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
            WindowContent::Doc(d) => d.source.as_ref().map(|s| s.buffer_id),
            WindowContent::Edit(e) => Some(e.editor.buffer_id),
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

    /// Check if a window should be visible given the active tab's tag_view.
    fn window_visible_for_tag_view(
        content: &WindowContent,
        tag_view: &workspace::TagSet,
        file_buffers: &HashMap<workspace::FileBufferId, workspace::FileBuffer>,
    ) -> bool {
        if tag_view.is_empty() {
            return true;
        }
        let buf_id = match content {
            WindowContent::Doc(d) => d.source.as_ref().map(|s| s.buffer_id),
            WindowContent::Edit(e) => Some(e.editor.buffer_id),
            _ => return false, // Agent/Browser hidden when filter active
        };
        let Some(id) = buf_id else { return false };
        let Some(buf) = file_buffers.get(&id) else {
            return false;
        };
        buf.tags.iter().any(|t| tag_view.contains(t))
    }

    /// Check if a layout subtree has any visible leaves for the given tag view.
    fn subtree_has_visible_leaf(
        layout: &workspace::Layout<WindowContent>,
        tag_view: &workspace::TagSet,
        file_buffers: &HashMap<workspace::FileBufferId, workspace::FileBuffer>,
    ) -> bool {
        match layout {
            workspace::Layout::Empty => false,
            workspace::Layout::Leaf(w) => {
                Self::window_visible_for_tag_view(&w.content, tag_view, file_buffers)
            }
            workspace::Layout::Split { children, .. } => children
                .iter()
                .any(|(_, c)| Self::subtree_has_visible_leaf(c, tag_view, file_buffers)),
        }
    }

    /// If the focused window is hidden by the tag filter, move focus to the
    /// first visible window.
    fn adjust_focus_for_tag_view(&mut self) {
        let tag_view = match self.workspace.active_tab() {
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
            .active_tab()
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
            .active_tab()
            .map(|t| t.layout.leaf_ids())
            .unwrap_or_default();

        for id in ids {
            let visible = self
                .workspace
                .active_tab()
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
                if let Some(tab) = self.workspace.active_tab_mut() {
                    tab.focused = id;
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
    fn overlay_is_session(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::SessionSwitcher(_))
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
    fn session_ref(&self) -> Option<&SessionSwitcher> {
        if let ActiveOverlay::SessionSwitcher(s) = &self.active_overlay {
            Some(s)
        } else {
            None
        }
    }
    fn session_mut(&mut self) -> Option<&mut SessionSwitcher> {
        if let ActiveOverlay::SessionSwitcher(s) = &mut self.active_overlay {
            Some(s)
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
            opened_from,
            disabled: self.global_menu_disabled(),
        }));
        cx.notify();
    }

    /// Behavior 10: global-menu entries inapplicable to the focused content
    /// kind are disabled (dimmed, non-dispatching) rather than hidden, so the
    /// menu layout stays spatially stable.
    fn global_menu_disabled(&self) -> HashSet<String> {
        let mut d = HashSet::new();
        // `back-to-doc` only makes sense from an Edit pane. (The other
        // pane-scoped entries moved to the `.` local menus — Phase 2.)
        if !matches!(
            self.workspace.focused_content(),
            Some(WindowContent::Edit(_))
        ) {
            d.insert("back-to-doc".to_string());
        }
        d
    }

    /// `.` — open the content-kind-specific local menu (spec-menu-scopes.md
    /// Behavior 2). Same overlay machinery as the global menu; only the tree
    /// and header differ.
    fn open_local_menu_inner(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_menu() {
            return;
        }
        let Some(opened_from) = self.workspace.focused_window_id() else {
            return;
        };
        let (menu, header) = match self.workspace.focused_content() {
            Some(WindowContent::Doc(_)) => (doc_local_menu(), "DOC"),
            Some(WindowContent::Edit(_)) => (edit_local_menu(), "EDIT"),
            Some(WindowContent::Agent(_)) => (agent_local_menu(), "AGENT"),
            Some(WindowContent::Browser(_)) => (browser_local_menu(), "BROWSE"),
            None => return,
        };
        self.transient_status = None;
        let mut state = MenuState::new();
        state.open();
        self.open_overlay(ActiveOverlay::Menu(MenuOverlay {
            state,
            menu,
            header,
            opened_from,
            disabled: HashSet::new(),
        }));
        cx.notify();
    }

    fn open_local_menu(&mut self, _: &OpenLocalMenu, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_local_menu_inner(cx);
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
    fn dispatch_menu_command(&mut self, name: &str, cx: &mut Context<Self>) {
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
                    WindowContent::Agent(_)
                ) {
                    // Mode-aware submit: Worksheet sweep (§12) or Chatbox
                    // submit (§18) depending on `AgentState::input_mode`.
                    self.submit_agent(cx);
                }
            }
            "claude-new" => self.new_agent_session(None, cx),
            "claude-list" => self.open_session_switcher(cx),
            "claude-close" => self.close_active_agent_session(cx),
            "claude-next" => self.switch_agent_session(1, cx),
            "claude-prev" => self.switch_agent_session(-1, cx),
            "claude-reboot" => self.reboot_into_claude(cx),
            "claude-mode-cycle" => self.cycle_claude_permission_mode(cx),
            "claude-clear" => self.clear_agent_session(cx),
            "claude-detach" => self.detach_active_agent_session(cx),
            "claude-attach" => self.attach_active_agent_session(cx),
            "claude-rename" => self.open_rename_overlay(cx),
            "claude-new-here" => self.open_new_agent_session_cwd_overlay(cx),
            "claude-cd" => self.open_change_agent_cwd_overlay(cx),
            "dev-build-candidate" => self.build_and_launch_candidate(cx),
            "dev-restart-gui" => self.dev_rebuild_restart_gui(cx),
            "dev-take-over" => self.candidate_take_over(cx),
            "rail-files" => self.toggle_file_browser_rail_impl(cx),
            "rail-outline" => self.toggle_outline_rail_impl(cx),
            "rail-flip" => self.flip_rail_side_impl(cx),
            "compose-toggle" | "agent-input-toggle" => {
                if matches!(
                    self.workspace.focused_content().expect("no focused window"),
                    WindowContent::Agent(_)
                ) {
                    self.toggle_agent_input_mode(cx);
                }
            }
            "back-to-doc" => self.back_to_doc(cx),
            "reload-file" => self.reload_focused_from_disk(cx),
            "rename-tab" => self.open_rename_active_tab_overlay(cx),
            "theme-dracula" => self.set_theme(ThemeName::Dracula, cx),
            "theme-nightfox" => self.set_theme(ThemeName::Nightfox, cx),
            "theme-solarized-light" => self.set_theme(ThemeName::SolarizedLight, cx),
            "theme-solarized-dark" => self.set_theme(ThemeName::SolarizedDark, cx),
            "theme-gruvbox-dark" => self.set_theme(ThemeName::GruvboxDark, cx),
            "theme-financial-times" => self.set_theme(ThemeName::FinancialTimes, cx),
            "theme-financial-times-dark" => self.set_theme(ThemeName::FinancialTimesDark, cx),
            "theme-folio" => self.set_theme(ThemeName::Folio, cx),
            "claude-status-bar" => {
                self.agent_status_position = self.agent_status_position.toggle();
                self.save_settings();
                cx.notify();
            }
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
                        if self.workspace.tabs.len() <= 1 {
                            return;
                        }
                        let idx = self.workspace.active_tab;
                        self.workspace.close_tab(idx);
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
            "resize-shrink" => {
                let _ = self.workspace.resize_focused(-0.05);
                self.save_workspace_state();
                cx.notify();
            }
            "resize-grow" => {
                let _ = self.workspace.resize_focused(0.05);
                self.save_workspace_state();
                cx.notify();
            }
            "equalize" => {
                let _ = self.workspace.equalize_focused();
                self.save_workspace_state();
                cx.notify();
            }
            "new-tab" => {
                self.workspace
                    .push_initial_tab(WindowContent::Browser(BrowserWindow::standalone(
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    )));
                self.save_workspace_state();
                cx.notify();
            }
            "close-tab" => {
                if self.workspace.tabs.len() <= 1 {
                    cx.quit();
                    return;
                }
                let idx = self.workspace.active_tab;
                self.workspace.close_tab(idx);
                self.save_workspace_state();
                cx.notify();
            }
            "next-tab" => {
                if self.workspace.tabs.len() > 1 {
                    self.workspace.next_tab();
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "prev-tab" => {
                if self.workspace.tabs.len() > 1 {
                    self.workspace.prev_tab();
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "move-pane" => self.open_workspace_picker(WorkspacePickerMode::Move, cx),
            "also-show-pane" => self.open_workspace_picker(WorkspacePickerMode::AlsoShow, cx),
            // Layout patterns
            "cycle-layout" => {
                self.workspace.set_layout_mode(
                    self.workspace
                        .active_tab()
                        .map(|t| t.layout_mode.cycle())
                        .unwrap_or_default(),
                );
                let sigil = self
                    .workspace
                    .active_tab()
                    .map(|t| t.layout_mode.sigil())
                    .unwrap_or("");
                self.transient_status = Some(format!("layout: {sigil}").into());
                self.save_workspace_state();
                cx.notify();
            }
            "promote-master" => {
                let is_ms = self
                    .workspace
                    .active_tab()
                    .map(|t| t.layout_mode == workspace::LayoutMode::MasterStack)
                    .unwrap_or(false);
                if is_ms {
                    if let Some(tab) = self.workspace.active_tab_mut() {
                        let ids = tab.layout.leaf_ids();
                        if ids.len() >= 2 && tab.focused != ids[0] {
                            tab.layout.swap_leaf_contents(tab.focused, ids[0]);
                        }
                    }
                    self.workspace.retile_active();
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            "inc-master" => {
                if let Some(tab) = self.workspace.active_tab_mut()
                    && tab.layout_mode == workspace::LayoutMode::MasterStack
                {
                    let max = tab.layout.leaf_count().saturating_sub(1).max(1);
                    tab.master_count = (tab.master_count + 1).min(max);
                }
                self.workspace.retile_active();
                self.save_workspace_state();
                cx.notify();
            }
            "dec-master" => {
                if let Some(tab) = self.workspace.active_tab_mut()
                    && tab.layout_mode == workspace::LayoutMode::MasterStack
                {
                    tab.master_count = tab.master_count.saturating_sub(1).max(1);
                }
                self.workspace.retile_active();
                self.save_workspace_state();
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
            // New-panel commands (global `n` submenu): create a NEW pane
            // (vertical split of the focused one) instead of replacing
            // the focused pane's content.
            "new-browser-pane" => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let _ = self.workspace.split_focused(
                    workspace::SplitDir::V,
                    WindowContent::Browser(BrowserWindow::standalone(cwd)),
                );
                self.workspace.retile_active();
                self.save_workspace_state();
                cx.notify();
            }
            "new-agent-pane" => {
                // Split with a placeholder browser (focus lands on the new
                // pane), then let `open_agent_inner` swap the focused pane
                // for the agent ring — reusing all the session machinery.
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if self
                    .workspace
                    .split_focused(
                        workspace::SplitDir::V,
                        WindowContent::Browser(BrowserWindow::standalone(cwd)),
                    )
                    .is_some()
                {
                    self.workspace.retile_active();
                    self.open_agent_inner(cx);
                    self.save_workspace_state();
                    cx.notify();
                }
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
                if matches!(
                    self.workspace.focused_content(),
                    Some(WindowContent::Agent(_))
                ) {
                    self.send_agent_selection(cx);
                }
            }
            "browser-open-workspace" => {
                let sel = self
                    .browser_mut()
                    .and_then(|b| b.fb.selected_entry().map(|e| (e.path.clone(), e.is_dir)));
                match sel {
                    Some((path, false)) => {
                        if let Some(content) = self.make_doc_content(&path) {
                            self.workspace.push_initial_tab(content);
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
            _ => {
                // Unknown command — keep the menu closed but no-op so the
                // user gets visual feedback that the entry "did something".
                // Future: surface a status hint somewhere.
            }
        }
    }

    // ---- Buffer switcher ---------------------------------------------------

    fn open_buffer_switcher(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_buffer() || self.workspace.tabs.is_empty() {
            return;
        }
        self.open_overlay(ActiveOverlay::BufferSwitcher(BufferSwitcher {
            selected: self.workspace.active_tab,
            filter_mode: false,
            filter_text: String::new(),
        }));
        cx.notify();
    }

    fn close_buffer_switcher(&mut self) {
        self.clear_overlay();
    }

    // ---- Workspace picker (move / also-show pane) -------------------------

    /// Count how many distinct **workspaces** show a view of `label` (the
    /// file path backing a Doc/Edit pane). File path is the canonical buffer-
    /// pool key (`Workspace::canonical_key`), so counting by path is the exact
    /// equivalent of counting by `FileBufferId` for pooled Edit panes — and it
    /// additionally captures Doc panes, which render disk snapshots and aren't
    /// pooled. Hence path, not id, is the right unifying membership key
    /// (spec-workspaces-tagging.md C-derived).
    ///
    /// Counts a workspace once even if it holds several views of the same
    /// file, so the multi-home dot means "also lives in another desktop",
    /// not "appears N times here".
    fn workspaces_showing_file(&self, label: &str) -> usize {
        self.workspace
            .tabs
            .iter()
            .filter(|tab| {
                let mut found = false;
                tab.layout.for_each_leaf(&mut |w| {
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

    /// True when the focused pane is file-backed (Doc or Edit) and so can be
    /// "also-shown" in another workspace (Agent/Browser are single-home).
    fn focused_is_file_backed(&self) -> bool {
        matches!(
            self.workspace.focused_content(),
            Some(WindowContent::Doc(_)) | Some(WindowContent::Edit(_))
        )
    }

    /// Open the workspace picker overlay. For `AlsoShow`, reject non-file
    /// panes up front with a footer message (the picker never opens).
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
        // move/also-show into the workspace the pane already lives in); fall
        // back to the "+ new workspace" entry when there's only one.
        let active = self.workspace.active_tab;
        let selected = (0..self.workspace.tabs.len())
            .find(|&i| i != active)
            .unwrap_or(self.workspace.tabs.len());
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
        self.workspace.tabs.len() + 1
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
    /// list (`tabs.len()` means "+ new workspace").
    fn commit_workspace_picker(&mut self, entry: usize, cx: &mut Context<Self>) {
        let mode = match self.workspace_picker_ref() {
            Some(p) => p.mode,
            None => return,
        };
        let n_tabs = self.workspace.tabs.len();
        let active = self.workspace.active_tab;

        // Resolve the target tab index, creating a new workspace if "+ new"
        // was chosen. A new workspace starts Empty; the relocated/also-shown
        // leaf becomes its first pane.
        let make_new = entry >= n_tabs;
        let target = if make_new {
            self.push_empty_workspace();
            self.workspace.tabs.len() - 1
        } else {
            entry
        };

        // Selecting the active workspace is a no-op (the pane is already here).
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

    /// Append a new empty workspace (today's `Tab`) with an auto-name and an
    /// `Empty` layout. Does NOT change the active workspace — the caller picks
    /// what to do next (relocate a leaf into it, etc.).
    fn push_empty_workspace(&mut self) {
        let name = workspace::auto_tab_name(self.workspace.next_tab_index);
        self.workspace.next_tab_index += 1;
        self.workspace.tabs.push(workspace::Tab {
            auto_name: name,
            display_name: None,
            layout: workspace::Layout::Empty,
            focused: 0,
            rail: None,
            layout_mode: workspace::LayoutMode::Manual,
            saved_manual_layout: None,
            master_ratio: 0.6,
            master_count: 1,
            tag_view: std::collections::BTreeSet::new(),
        });
    }

    /// MOVE: relocate the focused leaf out of the active workspace into
    /// `target`. If the source workspace is left empty, remove it (unless it's
    /// the only workspace, which we leave empty). Focus follows the pane to
    /// the target workspace.
    fn move_focused_to_workspace(&mut self, target: usize) {
        let (window, source_empty) = match self.workspace.detach_focused() {
            Ok(v) => v,
            Err(()) => return,
        };
        // `detach_focused` may shift nothing, but if it removed the active
        // tab's only pane the target index could still be valid (target was
        // resolved before detach and detach never removes tabs). Insert first,
        // then prune the empty source so indices stay stable during insert.
        let _ = self.workspace.insert_leaf_into_tab(target, window);

        let source = self.workspace.active_tab;
        if source_empty {
            if self.workspace.tabs.len() > 1 {
                // Removing the source shifts indices; recompute the target's
                // position so we can land focus there.
                let target_after = if target > source { target - 1 } else { target };
                self.workspace.close_tab(source);
                self.workspace.active_tab = target_after.min(self.workspace.tabs.len() - 1);
            } else {
                // Only workspace: leave it empty and stay on it (matches the
                // existing single-tab close behavior — we don't quit here).
                self.workspace.active_tab = target.min(self.workspace.tabs.len() - 1);
            }
        } else {
            // Source still has panes; follow the moved pane to the target.
            self.workspace.active_tab = target.min(self.workspace.tabs.len() - 1);
        }
    }

    /// ALSO-SHOW: open a second view onto the focused file-backed pane's file
    /// in `target`, leaving the original in place. The new view reads the
    /// file from disk (independent cursor/scroll), mirroring how splits clone
    /// a file pane today. Switches to the target workspace so the user sees
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
        let _ = self.workspace.insert_leaf_into_tab(target, window);
        self.workspace.active_tab = target.min(self.workspace.tabs.len() - 1);
    }

    // ---- Session switcher overlay -----------------------------------------

    fn open_session_switcher(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_session() {
            return;
        }
        // Must be on the agent screen with at least one session.
        let ring = match self.agent_ring() {
            Some(r) if !r.is_empty() => r,
            _ => {
                // Not on Claude screen — open Claude first, then show the list.
                self.open_agent_inner(cx);
                if let Some(r) = self.agent_ring() {
                    if r.is_empty() {
                        return;
                    }
                } else {
                    return;
                }
                // Fall through — ring is now valid.
                self.agent_ring().unwrap()
            }
        };
        // Hoist `ring.active` to an owned local so the `ring` (&self) borrow
        // ends before `open_overlay` takes `&mut self`.
        let selected = ring.active;
        self.open_overlay(ActiveOverlay::SessionSwitcher(SessionSwitcher { selected }));
        cx.notify();
    }

    fn close_session_switcher(&mut self) {
        self.clear_overlay();
    }

    fn handle_session_switcher_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let selected = match self.session_ref() {
            Some(ss) => ss.selected,
            None => return,
        };

        match press.key {
            Key::Esc | Key::Char('q') => {
                self.close_session_switcher();
            }
            Key::Char('j') | Key::Down => {
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if let Some(ss) = self.session_mut()
                    && count > 0
                {
                    ss.selected = (ss.selected + 1) % count;
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if let Some(ss) = self.session_mut()
                    && count > 0
                {
                    ss.selected = if ss.selected == 0 {
                        count - 1
                    } else {
                        ss.selected - 1
                    };
                }
            }
            Key::Char('g') => {
                if let Some(ss) = self.session_mut() {
                    ss.selected = 0;
                }
            }
            Key::Char('G') => {
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if let Some(ss) = self.session_mut()
                    && count > 0
                {
                    ss.selected = count - 1;
                }
            }
            Key::Enter | Key::Char('l') => {
                // Switch to the selected session.
                if let Some(ring) = self.agent_ring_mut() {
                    ring.active = selected;
                }
                self.close_session_switcher();
                self.save_agent_ring();
            }
            Key::Char('x') => {
                // Close the selected session (without switching to it first).
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if count > 0 {
                    // Optimistic close (same as `close_active_agent_session`,
                    // S4): drop the slot locally and fire the server close
                    // off-thread so a stalled server can't freeze the switcher.
                    let server_sid = self
                        .agent_ring()
                        .and_then(|r| r.slots.get(selected))
                        .and_then(|s| s.server_session_id.clone());
                    if let Some(sid) = server_sid {
                        self.spawn_close_session(sid, cx);
                    }
                    if let Some(ring) = self.agent_ring_mut() {
                        ring.close_at(selected);
                    }
                    let new_count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                    if new_count == 0 {
                        self.close_session_switcher();
                        self.back_to_doc(cx);
                        self.save_agent_ring();
                        cx.notify();
                        return;
                    }
                    if let Some(ss) = self.session_mut()
                        && ss.selected >= new_count
                    {
                        ss.selected = new_count - 1;
                    }
                    self.save_agent_ring();
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn render_session_switcher(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let ss = match self.session_ref() {
            Some(ss) => ss,
            None => unreachable!(),
        };
        let ring = self.agent_ring().unwrap();

        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let label_fg: Hsla = nc(ov.label);
        let active_fg: Hsla = nc(ov.accent);
        let selected_bg: Hsla = nc(ov.selected_bg);
        let normal_fg: Hsla = nc(ov.fg);
        let popup_border: Hsla = nc(ov.border);
        let busy_fg: Hsla = nc(ov.modified);

        let header_text = format!("SESSIONS ({})", ring.len());
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

        let mut entries_col = div()
            .flex()
            .flex_col()
            .px_4()
            .py_2()
            .text_size(px(14.0))
            .font_family(self.code_font.clone());

        for (i, slot) in ring.slots.iter().enumerate() {
            let is_selected = i == ss.selected;
            let is_active = i == ring.active;
            let is_busy = slot.state.turn_phase.is_awaiting();

            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let active_dot = if is_active { "\u{25cf} " } else { "  " };
            let busy_mark = if is_busy { " \u{2026}" } else { "" };
            let cwd_display = shorten_cwd_for_display(&slot.cwd);
            let label_text = format!("{}{}", slot.label, busy_mark);

            let name_color = if is_active {
                active_fg
            } else if is_busy {
                busy_fg
            } else {
                normal_fg
            };

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
                        .child(SharedString::from(label_text)),
                )
                .child(
                    div()
                        .text_color(label_fg)
                        .child(SharedString::from(format!("  {cwd_display}"))),
                );

            entries_col = entries_col.child(row);
        }

        let hints_row = div()
            .px_4()
            .py_1()
            .text_size(px(11.0))
            .text_color(label_fg)
            .child("j/k move · enter select · x close · q/esc cancel");

        div()
            .id("session-switcher")
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
            WorkspacePickerMode::Move => "MOVE PANE TO WORKSPACE",
            WorkspacePickerMode::AlsoShow => "ALSO-SHOW PANE IN WORKSPACE",
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

        let active = self.workspace.active_tab;
        let n_tabs = self.workspace.tabs.len();
        for (i, tab) in self.workspace.tabs.iter().enumerate() {
            let is_selected = i == picker.selected;
            let is_active = i == active;
            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let here = if is_active { " (here)" } else { "" };
            let label_text = format!("{}{}", tab_strip_label(tab), here);
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
            let is_selected = picker.selected == n_tabs;
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
    fn open_rename_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let Some(ring) = self.agent_ring() else {
            return;
        };
        let slot = &ring.slots[ring.active];
        // Own the reads so the `ring`/`slot` (&self) borrow ends before
        // `open_overlay` takes `&mut self`.
        let text = slot.label.clone();
        let index = slot.index;
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::AgentSlot { index },
        }));
        cx.notify();
    }

    /// Open a path-input overlay; on commit, spawn a new agent session
    /// rooted at the typed path (spec-agent-cwd.md §2). Empty input
    /// cancels — the bare `claude-new` already exists for the
    /// "process cwd" case.
    fn open_new_agent_session_cwd_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        // Don't gate by "claude is focused" — this command can transition
        // the user into the agent screen at the chosen cwd in one step.
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text: String::new(),
            target: RenameTarget::AgentNewSessionCwd,
        }));
        cx.notify();
    }

    /// Open a path-input overlay pre-filled with the active slot's
    /// current cwd; on commit, respawn the slot at the new path
    /// (spec-agent-cwd.md §4).
    fn open_change_agent_cwd_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let Some(ring) = self.agent_ring() else {
            return;
        };
        let slot = &ring.slots[ring.active];
        let text = slot.cwd.display().to_string();
        let index = slot.index;
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::AgentChangeCwd { index },
        }));
        cx.notify();
    }

    /// Open the rename overlay targeting the active workspace tab. The
    /// input pre-fills with the tab's current display label (display_name
    /// if set, else auto_name).
    fn open_rename_active_tab_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay_is_rename() {
            return;
        }
        let idx = self.workspace.active_tab;
        let Some(tab) = self.workspace.tabs.get(idx) else {
            return;
        };
        let text = tab.display_label().to_string();
        self.open_overlay(ActiveOverlay::Rename(RenameOverlay {
            text,
            target: RenameTarget::Tab { index: idx },
        }));
        cx.notify();
    }

    fn close_rename_overlay(&mut self) {
        self.clear_overlay();
    }

    /// Apply the overlay's text to the targeted slot/tab, then close.
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
            RenameTarget::AgentSlot { index } => {
                // Update the label in the local ring and, if this session
                // is managed by the session server, push the new label
                // there so it persists across GUI restarts.
                let server_sid = self.agent_ring_mut().and_then(|ring| {
                    let slot = ring.slot_by_index_mut(index)?;
                    slot.label = new_label.clone();
                    slot.server_session_id.clone()
                });
                if let (Some(server), Some(sid)) = (&self.session_server, server_sid) {
                    let _ = server.rename_session(&sid, &new_label);
                }
                self.close_rename_overlay();
                self.save_agent_ring();
                cx.notify();
            }
            RenameTarget::Tab { index } => {
                if let Some(tab) = self.workspace.tabs.get_mut(index) {
                    tab.display_name = Some(new_label);
                }
                self.close_rename_overlay();
                self.save_workspace_state();
                cx.notify();
            }
            RenameTarget::AgentNewSessionCwd => {
                // Resolve per spec-agent-cwd.md §2 (tilde, canonicalize,
                // validate). Failure surfaces via the active agent's
                // footer hint and leaves the overlay closed.
                match resolve_agent_cwd_arg(&new_label) {
                    Ok(resolved) => {
                        self.close_rename_overlay();
                        self.new_agent_session(Some(resolved), cx);
                    }
                    Err(msg) => {
                        self.close_rename_overlay();
                        if let Some(c) = self.agent_mut() {
                            c.status = Some(msg.into());
                        }
                        cx.notify();
                    }
                }
            }
            RenameTarget::AgentChangeCwd { index } => match resolve_agent_cwd_arg(&new_label) {
                Ok(resolved) => {
                    self.close_rename_overlay();
                    self.change_agent_cwd(index, resolved, cx);
                }
                Err(msg) => {
                    self.close_rename_overlay();
                    if let Some(c) = self.agent_mut() {
                        c.status = Some(msg.into());
                    }
                    cx.notify();
                }
            },
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
                    self.transient_status = Some("cannot tag this pane".into());
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
                if let Some(tab) = self.workspace.active_tab_mut() {
                    if tag.is_empty() {
                        tab.tag_view.clear();
                        self.transient_status = Some("tag filter cleared".into());
                    } else {
                        tab.tag_view.clear();
                        tab.tag_view.insert(tag.clone());
                        self.transient_status = Some(format!("viewing tag '{tag}'").into());
                    }
                }
                self.adjust_focus_for_tag_view();
            }
            TagInputMode::SendTag => {
                self.tag_focused(tag.clone());
                if let Some(tab) = self.workspace.active_tab_mut() {
                    tab.tag_view.clear();
                    tab.tag_view.insert(tag.clone());
                }
                self.adjust_focus_for_tag_view();
                self.transient_status = Some(format!("tagged + viewing '{tag}'").into());
            }
            TagInputMode::AlsoTag => {
                if self.tag_focused(tag.clone()) {
                    self.transient_status = Some(format!("also-tagged '{tag}'").into());
                } else {
                    self.transient_status = Some("cannot tag this pane".into());
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
            None => return (0..self.workspace.tabs.len()).collect(),
        };
        if bs.filter_text.is_empty() {
            return (0..self.workspace.tabs.len()).collect();
        }
        let query = bs.filter_text.to_lowercase();
        (0..self.workspace.tabs.len())
            .filter(|&i| {
                let label = tab_doc_label(&self.workspace.tabs[i])
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

    fn render_menu_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let m = match self.menu_ref() {
            Some(m) => m,
            None => unreachable!(),
        };

        let ov = &self.theme.overlay;
        let menu_bg: Hsla = nc(ov.bg);
        let label_fg: Hsla = nc(ov.label);
        let key_fg: Hsla = nc(ov.key);
        let label_text_fg: Hsla = nc(ov.fg);
        let submenu_fg: Hsla = nc(ov.accent);
        let popup_border: Hsla = nc(ov.border);

        let nodes = m.state.current_nodes(&m.menu);
        let breadcrumb = m
            .state
            .current_label(&m.menu)
            .unwrap_or_else(|| "Commands".to_string());

        let header_row = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .child(format!("{} — {}", m.header, breadcrumb.to_uppercase()));

        // ---- Multi-column layout ----
        //
        // Partition the level into sections (separator-delimited groups,
        // each usually starting with a Label), then distribute whole
        // sections across 1–3 columns so a large menu fits on screen
        // without scrolling. Sections never split mid-group.
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
        let n_cols = if total_rows <= 8 {
            1
        } else if total_rows <= 18 {
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

        let render_node = |node: &MenuNode| -> AnyElement {
            match node.kind() {
                MenuNodeKind::Separator => unreachable!("separators delimit sections"),
                MenuNodeKind::Label => div()
                    .py_0p5()
                    .text_color(label_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(node.label.clone())
                    .into_any_element(),
                MenuNodeKind::Command | MenuNodeKind::Submenu => {
                    let key_display = format_menu_key(&node.key);
                    let trailing = if node.kind() == MenuNodeKind::Submenu {
                        format!(" {} \u{25b8}", node.label)
                    } else {
                        format!(" {}", node.label)
                    };
                    // Behavior 10: disabled entries render dimmed (key and
                    // label both in the label color) and don't dispatch.
                    let is_disabled = matches!(&node.action,
                        MenuAction::Command(name) if m.disabled.contains(name));
                    let label_color = if is_disabled {
                        label_fg
                    } else if node.kind() == MenuNodeKind::Submenu {
                        submenu_fg
                    } else {
                        label_text_fg
                    };
                    let entry_key_fg = if is_disabled { label_fg } else { key_fg };
                    div()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .py_0p5()
                        .child(
                            div()
                                .min_w(px(48.0))
                                .text_color(entry_key_fg)
                                .font_weight(FontWeight::BOLD)
                                .child(key_display),
                        )
                        .child(div().text_color(label_color).child(trailing))
                        .into_any_element()
                }
            }
        };

        let mut entries_col = div()
            .flex()
            .flex_row()
            .items_start()
            .gap_8()
            .px_4()
            .py_2()
            .text_color(label_text_fg)
            .text_size(px(14.0))
            .font_family(self.body_font.clone());
        for col_sections in columns {
            let mut col_div = div().flex().flex_col().min_w(px(220.0));
            let mut first = true;
            for sec in col_sections {
                if !first {
                    // Inter-section gap inside a column (replaces the old
                    // full-width separator rule).
                    col_div = col_div.child(
                        div()
                            .h(px(8.0))
                            .border_b_1()
                            .border_color(popup_border)
                            .my_1(),
                    );
                }
                first = false;
                for node in sec {
                    col_div = col_div.child(render_node(node));
                }
            }
            entries_col = entries_col.child(col_div);
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(22.0))
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static("press a key · Esc back / close"));

        // Absolute-positioned popup at the top of the window. Width spans
        // full window width; height is content-sized. Opaque bg + bottom
        // border so the underlying screen is visible *below* the popup but
        // hidden behind it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .bg(menu_bg)
            .border_b_2()
            .border_color(popup_border)
            .flex()
            .flex_col()
            .child(header_row)
            .child(entries_col)
            .child(footer)
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
        let total = self.workspace.tabs.len();
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
            let tab = &self.workspace.tabs[buf_idx];
            let is_selected = vis_idx == bs.selected;
            let is_active = buf_idx == self.workspace.active_tab;
            let is_modified = match &tab.layout {
                workspace::Layout::Leaf(w) => screen_is_modified(&w.content),
                _ => false,
            };

            let marker = if is_selected { "\u{25b8} " } else { "  " };
            let active_dot = if is_active { "\u{25cf} " } else { "  " };
            let modified_mark = if is_modified { " [+]" } else { "" };

            // Shorten the path for display
            let label_owned = tab_doc_label(tab).unwrap_or_else(|| tab.display_label().to_string());
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
    /// a full-screen panel. Pre-filled with the current label; trailing
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
            RenameTarget::AgentSlot { .. } => "RENAME SESSION",
            RenameTarget::Tab { .. } => "RENAME WORKSPACE",
            RenameTarget::AgentNewSessionCwd => "NEW SESSION AT…",
            RenameTarget::AgentChangeCwd { .. } => "CHANGE SESSION CWD",
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

impl SketchGpuiView {
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
                            .child("sketch"),
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

impl Focusable for SketchGpuiView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SketchGpuiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport_width_px = f32::from(_window.viewport_size().width);

        // Auto-clear expired splash.
        if let Some(deadline) = self.splash_until
            && std::time::Instant::now() >= deadline
        {
            self.splash_until = None;
        }
        if self.splash_until.is_some() {
            return self.render_splash(cx);
        }

        // 5c: re-derive each Doc pane's blocks from its shared core when the
        // rope has advanced (e.g. an Edit pane of the same file took a
        // keystroke). `refresh_blocks` is O(1) per Doc when the core is
        // unchanged and read-only on the core, so this is cheap and panic-safe.
        {
            let theme = &self.theme;
            for tab in self.workspace.tabs.iter_mut() {
                tab.layout.for_each_leaf_content_mut(&mut |content| {
                    if let WindowContent::Doc(d) = content {
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
        // to the wrapper so the screen's `SketchView`/`BrowserView` action
        // bindings don't match (they would otherwise fire BEFORE our key
        // listener — for example, `k` in Doc context is bound to
        // `ScrollUp` and `k` in Browser context is bound to `BrowserUp`,
        // both of which intercept the keystroke before any `on_key_down`
        // handler runs and stop propagation as the default action behavior).
        // When no overlay is open, the focused leaf inside `render_layout`
        // attaches `track_focus(&self.focus_handle)` — that way the focus
        // handle sits INSIDE the SketchView/EditView/etc. key context, so
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
        // claim it, and the leaf's context-scoped bindings (SketchView, …)
        // would shadow the rail's (spec §6, two-state model §5).
        let rail_focused = !has_overlay && self.rail_is_focused();
        let leaf_attach_focus = !has_overlay && !rail_focused;
        // The rail is now injected beside the focused leaf *inside*
        // `render_focused_window` (so it's local to the focused pane, not the
        // whole window). It's focusable only when no overlay owns focus (§4).
        let screen_view: AnyElement =
            self.render_focused_window(screen_root, leaf_attach_focus, !has_overlay, cx);

        // When there's more than one tab, stack the tab strip above the
        // screen view. Single-tab workspaces render no strip — matches the
        // spec for "always show strip when >= 1 tab" but conservatively
        // suppresses it for the most common case (one-tab session) while
        // tab-creation commands are still landing.
        let screen_view = self.wrap_with_tab_strip(screen_view, cx);

        // Tag bar: thin strip above content showing tag labels when any
        // buffers have tags. Active-view tags get accent background.
        let screen_view = self.wrap_with_tag_bar(screen_view);

        // Overlay a one-shot transient status toast (e.g. an also-show
        // rejection for a non-file pane) in the bottom-right.
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

        // Session switcher takes priority over menu.
        if self.overlay_is_session() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("SessionSwitcherView")
                .size_full()
                .bg(editor_bg)
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_session_switcher_key(ev, w, cx);
                    cx.stop_propagation();
                }))
                .child(screen_view)
                .child(self.render_session_switcher(cx))
                .into_any_element();
        }

        // Workspace picker (move / also-show pane).
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

/// Short display label for the tab strip. Doc/Edit tabs show the file's
/// basename (`E ` prefix for Edit); Browser/Claude show their kind.
fn tab_strip_label(tab: &workspace::Tab<WindowContent>) -> String {
    if let workspace::Layout::Leaf(w) = &tab.layout {
        match &w.content {
            WindowContent::Doc(d) => basename_or_full(d.file_label.as_ref()),
            WindowContent::Edit(e) => format!("E {}", basename_or_full(e.file_label.as_ref())),
            WindowContent::Browser(_) => format!("Browser ({})", tab.display_label()),
            WindowContent::Agent(_) => format!("Claude ({})", tab.display_label()),
        }
    } else {
        tab.display_label().to_string()
    }
}

fn basename_or_full(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Extract the file label of a tab's focused window, if Doc or Edit.
/// Returns `None` for Browser/Claude tabs or non-leaf layouts.
fn tab_doc_label(tab: &workspace::Tab<WindowContent>) -> Option<String> {
    if let workspace::Layout::Leaf(w) = &tab.layout {
        match &w.content {
            WindowContent::Doc(d) => Some(d.file_label.to_string()),
            WindowContent::Edit(e) => Some(e.file_label.to_string()),
            _ => None,
        }
    } else {
        None
    }
}

/// Extract the file label from a screen, if it's a Doc or Edit screen.
fn screen_file_label(screen: &WindowContent) -> Option<SharedString> {
    match screen {
        WindowContent::Doc(d) => Some(d.file_label.clone()),
        WindowContent::Edit(e) => Some(e.file_label.clone()),
        _ => None,
    }
}

/// Check whether the screen's underlying editor has unsaved modifications.
fn screen_is_modified(screen: &WindowContent) -> bool {
    match screen {
        WindowContent::Edit(e) => e.editor.is_modified(),
        WindowContent::Doc(d) => d.source.as_ref().is_some_and(|s| s.is_modified()),
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
fn register_keymap(app: &mut App) {
    // Document-view bindings.
    app.bind_keys([
        KeyBinding::new("j", ScrollDown, Some("SketchView")),
        KeyBinding::new("down", ScrollDown, Some("SketchView")),
        KeyBinding::new("ctrl-n", ScrollDown, Some("SketchView")),
        KeyBinding::new("k", ScrollUp, Some("SketchView")),
        KeyBinding::new("up", ScrollUp, Some("SketchView")),
        KeyBinding::new("ctrl-p", ScrollUp, Some("SketchView")),
        KeyBinding::new("ctrl-d", ScrollPageDown, Some("SketchView")),
        KeyBinding::new("pagedown", ScrollPageDown, Some("SketchView")),
        KeyBinding::new("ctrl-u", ScrollPageUp, Some("SketchView")),
        KeyBinding::new("pageup", ScrollPageUp, Some("SketchView")),
        KeyBinding::new("l", CursorNextBlock, Some("SketchView")),
        KeyBinding::new("right", CursorNextBlock, Some("SketchView")),
        KeyBinding::new("h", CursorPrevBlock, Some("SketchView")),
        KeyBinding::new("left", CursorPrevBlock, Some("SketchView")),
        KeyBinding::new("g", CursorTop, Some("SketchView")),
        KeyBinding::new("shift-g", CursorBottom, Some("SketchView")),
        KeyBinding::new("ctrl-o", OpenBrowser, Some("SketchView")),
        KeyBinding::new("ctrl-e", EnterEdit, Some("SketchView")),
        // Ctrl-W is the split chord prefix (see global bindings below).
        // Word-processor entry rebinds to Ctrl-Shift-E.
        KeyBinding::new("ctrl-shift-e", EnterWp, Some("SketchView")),
        KeyBinding::new("ctrl-k", OpenAgent, Some("SketchView")),
        KeyBinding::new("space", OpenMenu, Some("SketchView")),
        // Local leader (spec-menu-scopes.md): `.` opens the content-kind
        // local menu. Edit/Agent views intercept `.` in their key handlers
        // instead (insert mode must keep `.` as a text character).
        KeyBinding::new(".", OpenLocalMenu, Some("SketchView")),
        // Doc-view Esc and bare `q` used to dispatch `Quit` — that
        // made it too easy to lose the app by mashing keys. Quit now
        // lives only on Cmd-Q (the macOS-standard chord). Esc in the
        // doc view is a no-op so users in normal-mode just stay where
        // they are; the menu still dismisses on Esc via its own
        // capture-phase handler.
        KeyBinding::new("tab", NextBuffer, Some("SketchView")),
        KeyBinding::new("shift-tab", PrevBuffer, Some("SketchView")),
    ]);

    // Global Cmd-shortcut bindings — work in every key context, so the
    // macOS menu-bar items (and the user's muscle memory) reach the
    // right action regardless of which screen is focused. `None`
    // context = matches anywhere, identical to how Zed wires its
    // application-wide commands.
    app.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-ctrl-r", Restart, None),
        KeyBinding::new("cmd-o", OpenBrowser, None),
        KeyBinding::new("cmd-k", OpenAgent, None),
        // Agent-window sidepane toggles (§32). Scoped to AgentView
        // so Cmd-1/Cmd-2 don't shadow anything in other screens.
        KeyBinding::new("cmd-1", ToggleTasklist, Some("AgentView")),
        KeyBinding::new("cmd-2", ToggleSubagents, Some("AgentView")),
        KeyBinding::new("ctrl-alt-enter", ToggleAgentInputMode, Some("AgentView")),
        KeyBinding::new("cmd-.", StopAgent, Some("AgentView")),
        // Workspace-level tab switching — app-global so the strip is
        // reachable from every screen and overlay (per spec Interfaces
        // table; bind also `Ctrl-Tab`/`Ctrl-Shift-Tab` for keyboard-only
        // users without Cmd).
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-shift-w", CloseTab, None),
        // Vim-style split chord prefix (spec-tabs-and-splits.md §12–§14).
        // GPUI parses "ctrl-w s" as a two-keystroke chord; pressing
        // Ctrl-W alone never resolves (it's a pure prefix here).
        KeyBinding::new("ctrl-w s", SplitH, None),
        KeyBinding::new("ctrl-w v", SplitV, None),
        KeyBinding::new("ctrl-w c", CloseWindow, None),
        // Mac-standard close shortcut. Closes the focused pane; falls
        // through to closing the tab if the pane was the only one in
        // its tab (unless it's also the only tab — then no-op rather
        // than quit, per the "no surprise quits" rule).
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("ctrl-w o", OnlyWindow, None),
        // Move / also-show the focused pane in another workspace
        // (spec-workspaces-tagging.md Phase 1). `m` moves (pane leaves
        // here), `M` (shift) also-shows a second view of a file pane.
        KeyBinding::new("ctrl-w m", MovePane, None),
        KeyBinding::new("ctrl-w shift-m", AlsoShowPane, None),
        // Vim-style focus motion across split panes.
        KeyBinding::new("ctrl-w h", FocusLeft, None),
        KeyBinding::new("ctrl-w l", FocusRight, None),
        KeyBinding::new("ctrl-w k", FocusUp, None),
        KeyBinding::new("ctrl-w j", FocusDown, None),
        KeyBinding::new("ctrl-w w", FocusNext, None),
        KeyBinding::new("ctrl-w shift-w", FocusPrev, None),
        // Resize the focused pane vs. its next sibling.
        KeyBinding::new("ctrl-w <", ResizeShrink, None),
        KeyBinding::new("ctrl-w -", ResizeShrink, None),
        KeyBinding::new("ctrl-w >", ResizeGrow, None),
        KeyBinding::new("ctrl-w +", ResizeGrow, None),
        KeyBinding::new("ctrl-w =", Equalize, None),
        // Layout patterns (spec-layout-patterns.md)
        // Phase 2: automatic layouts
        KeyBinding::new("ctrl-w space", CycleLayoutMode, None),
        KeyBinding::new("ctrl-w enter", PromoteToMaster, None),
        KeyBinding::new("ctrl-w i", IncreaseMasterCount, None),
        KeyBinding::new("ctrl-w d", DecreaseMasterCount, None),
        // Phase 3: tags
        KeyBinding::new("ctrl-w t", TagViewChord, None),
        KeyBinding::new("ctrl-w ctrl-t", TagToggleChord, None),
        KeyBinding::new("ctrl-w shift-t", ClearTagView, None),
        // Document text zoom — same chord set every Mac app uses for
        // browser/editor zoom (Cmd-=, Cmd-+, Cmd--, Cmd-0). Scales the
        // doc/edit body + heading sizes; chrome stays fixed.
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ZoomReset, None),
        // Copy the view-mode mouse selection. Scoped to SketchView so it
        // doesn't shadow edit-mode yank or other surfaces' copy paths.
        KeyBinding::new("cmd-c", CopyDocSelection, Some("SketchView")),
        // Copy active selection to system clipboard (all screens).
        KeyBinding::new("cmd-c", CopySelection, None),
        // Paste from system clipboard into active editor.
        KeyBinding::new("cmd-v", PasteFromClipboard, None),
        // Rename the active tab. Global so it works from any screen
        // (and the menu's "rename tab" entry uses the same path).
        KeyBinding::new("cmd-shift-r", RenameTab, None),
        // Rail toggles (spec-rail.md §1, §10). Global so they work from
        // any screen and from inside the rail itself.
        KeyBinding::new("cmd-b", ToggleFileBrowserRail, None),
        KeyBinding::new("cmd-shift-o", ToggleOutlineRail, None),
        KeyBinding::new("cmd-shift-b", FlipRailSide, None),
    ]);

    // Browser-view bindings.
    app.bind_keys([
        KeyBinding::new("j", BrowserDown, Some("BrowserView")),
        KeyBinding::new("down", BrowserDown, Some("BrowserView")),
        KeyBinding::new("ctrl-n", BrowserDown, Some("BrowserView")),
        KeyBinding::new("k", BrowserUp, Some("BrowserView")),
        KeyBinding::new("up", BrowserUp, Some("BrowserView")),
        KeyBinding::new("ctrl-p", BrowserUp, Some("BrowserView")),
        KeyBinding::new("enter", BrowserEnter, Some("BrowserView")),
        KeyBinding::new("l", BrowserEnter, Some("BrowserView")),
        KeyBinding::new("right", BrowserEnter, Some("BrowserView")),
        KeyBinding::new("h", BrowserParent, Some("BrowserView")),
        KeyBinding::new("left", BrowserParent, Some("BrowserView")),
        KeyBinding::new("-", BrowserParent, Some("BrowserView")),
        // `.` was BrowserToggleHidden; it's now the local leader
        // (spec-menu-scopes.md) — toggle-hidden lives in the local menu (`. .`).
        KeyBinding::new(".", OpenLocalMenu, Some("BrowserView")),
        KeyBinding::new("s", BrowserCycleSort, Some("BrowserView")),
        KeyBinding::new("space", OpenMenu, Some("BrowserView")),
        KeyBinding::new("q", BrowserClose, Some("BrowserView")),
        KeyBinding::new("escape", BrowserClose, Some("BrowserView")),
        KeyBinding::new("w", BrowserWorktrees, Some("BrowserView")),
        KeyBinding::new("/", BrowserFilter, Some("BrowserView")),
    ]);

    // Rail-view bindings (spec-rail.md §6). Active only while the rail
    // holds focus (its root attaches `track_focus` inside this context).
    app.bind_keys([
        KeyBinding::new("j", RailDown, Some("RailView")),
        KeyBinding::new("down", RailDown, Some("RailView")),
        KeyBinding::new("ctrl-n", RailDown, Some("RailView")),
        KeyBinding::new("k", RailUp, Some("RailView")),
        KeyBinding::new("up", RailUp, Some("RailView")),
        KeyBinding::new("ctrl-p", RailUp, Some("RailView")),
        KeyBinding::new("enter", RailSelect, Some("RailView")),
        KeyBinding::new("escape", RailClose, Some("RailView")),
        KeyBinding::new("-", RailParent, Some("RailView")),
        KeyBinding::new(".", RailToggleHidden, Some("RailView")),
        KeyBinding::new("s", RailCycleSort, Some("RailView")),
        KeyBinding::new("w", RailWorktrees, Some("RailView")),
        KeyBinding::new("/", RailFilter, Some("RailView")),
    ]);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Relocate state written by older builds under <cache_dir>/sketch into the
    // durable `~/.sketch` home (ADR-0018), BEFORE any persisted state (prefs,
    // workspace, client_id, acp_sessions) is read. One-time, idempotent.
    sketch::paths::migrate_legacy_cache_dir();
    let config = sketch::config::Config::load().unwrap_or_default();
    // App-managed preferences override config.kdl's theme — that's where
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
            println!("sketch-gpui: loaded {} ({} blocks)", canon, blocks.len());
            Some((blocks, canon, path))
        }
        None => {
            println!("sketch-gpui: no file given, opening browser");
            None
        }
    };

    Application::new().run(move |app: &mut App| {
        register_keymap(app);

        // Quit when the last window closes. macOS apps typically stay
        // alive in the menu bar after every window is dismissed, but
        // sketch has no menu-bar-only mode — without this hook, closing
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
                        title: Some("sketch".into()),
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
                                let mut v = SketchGpuiView::new_doc(
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
                                    && let Some(WindowContent::Doc(d)) =
                                        v.workspace.focused_content_mut()
                                {
                                    d.source = Some(DocSource::new(id, core));
                                }
                                v
                            }
                            None => SketchGpuiView::new_browser(
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                                theme.clone(),
                                focus_handle,
                            ),
                        };
                        // Agent info bar placement from preferences.
                        if let Some(pos) = prefs.agent_status_position.as_deref() {
                            view.agent_status_position = AgentStatusPosition::parse(pos);
                        }
                        // Restore the saved text zoom (clamped so a hand-edited
                        // preferences file can't push the body off-screen).
                        if let Some(scale) = prefs.text_scale {
                            view.text_scale = scale.clamp(MIN_TEXT_SCALE, MAX_TEXT_SCALE);
                        }
                        // If we were launched with no explicit file arg, try to
                        // restore the saved workspace for this cwd. With an
                        // explicit arg the user wants that file, so the saved
                        // snapshot stays on disk for the next no-arg launch.
                        if initial_doc.is_none() {
                            view.restore_workspace_from_disk(cx);
                        }
                        // Reboot handoff: the previous sketch process set this
                        // env var via `reboot_into_claude` to mean "boot
                        // straight into the claude screen and resume every
                        // saved session." The downstream `open_agent_inner`
                        // consults `load_persisted_acp_sessions`, so
                        // session/load fires once per persisted slot.
                        if std::env::var("SKETCH_OPEN_CLAUDE").is_ok() {
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
        // *before* `App::shutdown` clears windows and races view Drop
        // against worker-thread joins. The hook gives us a 100ms budget
        // (`SHUTDOWN_TIMEOUT`) — comfortably enough for the worker to
        // signal its child, since the agent process has `kill_on_drop`
        // and exits as soon as the runtime drops. Returning a no-op
        // future satisfies the async signature; the real work is sync.
        app.on_app_quit(move |cx| {
            let _ = window_handle.update(cx, |view, _w, _ctx| {
                view.shutdown_acp();
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
                name: "sketch".into(),
                items: vec![MenuItem::action("Quit sketch", Quit)],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Open File Browser", OpenBrowser),
                    MenuItem::action("Open Claude Session", OpenAgent),
                ],
            },
        ]);

        // Bring sketch to the foreground on launch. Without this the
        // process opens a window but stays behind whatever app the user
        // had focused (terminal, editor, etc.) — particularly noticeable
        // on a `cargo run` or a `reboot_into_claude` re-launch. `true`
        // = ignore other apps' "don't yield focus" hints, which is the
        // right behaviour for a user-initiated launch.
        app.activate(true);
    });
}

#[cfg(test)]
mod tests;
