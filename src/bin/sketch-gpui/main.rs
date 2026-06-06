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
//!   .                    toggle hidden files
//!   s                    cycle sort order (name / date↓ / date↑)
//!   q / Esc              close browser (returns to doc, or quits)

mod highlight_cache;
mod workspace;
#[cfg(test)]
mod verify_harness;

use highlight_cache::{HighlightCache, LineHl};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::process;
use std::time::Duration;

use gpui::{
    actions, div, point, px, rgb, rgba, size, AnyElement, App, AppContext, Application,
    Bounds, Context, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, Keystroke, Menu,
    ListScrollEvent, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, StrikethroughStyle,
    Styled, StyledText, Task, TextLayout, TextRun, TitlebarOptions, UnderlineStyle, Window,
    WindowBounds, WindowOptions, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels,
};

use sketch::acp_channel::AcpChannelClient;
use sketch::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine, StyledSpan};
use sketch::cursor::CursorPos;
use sketch::document::Document;
use sketch::editor::{Editor, EditorCore, EditorView, LineAnchor};
use sketch::file_browser::{BrowserEntry, FileBrowser};
use sketch::worktree;
use sketch::keybind::KeybindManager;
use sketch::keys::{Key, KeyPress, Modifiers as KMods};
use sketch::md_highlight::{
    highlight_markdown_lines_syn, highlight_markdown_lines_stripped_syn,
    Segment,
};
use sketch::menu::{MenuNode, MenuNodeKind, MenuState};
use sketch::render;
use sketch::session_client::SessionServerClient;
use sketch::session_proto::AttachMode;
use sketch::session_proto::Notification as ServerNotification;
use sketch::session_proto::SessionInfo;
use sketch::style::{Color as NColor, Modifier, Style as NStyle};
use sketch::theme::{OverlayTheme, Theme, ThemeName};

// ----------------------------------------------------------------------------
// Render performance knobs (env-gated, read once)
// ----------------------------------------------------------------------------

/// `true` when `SKETCH_PERF` is set to anything other than `0`/empty. Enables
/// per-frame timing breakdowns from the agent pane render path (extract /
/// highlight / snapshot / total), printed to stderr. Read once and cached.
fn perf_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        matches!(std::env::var("SKETCH_PERF"), Ok(v) if v != "0" && !v.is_empty())
    })
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
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => {
            Key::F(s[1..].parse().unwrap())
        }
        s => {
            // Prefer key_char when present (handles shifted chars properly:
            // shift-g → key="g", key_char=Some("G")).
            let ch_str = ks.key_char.as_deref().unwrap_or(s);
            ch_str
                .chars()
                .next()
                .map(Key::Char)
                .unwrap_or(Key::Other)
        }
    };
    KeyPress::new(key, mods)
}

// ----------------------------------------------------------------------------
// Theme palette
// ----------------------------------------------------------------------------

/// Dracula-derived background pulled from the neutral theme; GPUI doesn't
/// have a `Reset` color, so we pick concrete defaults.
const BG: u32 = 0x282a36;
const DEFAULT_FG: u32 = 0xf8f8f2;
const CURSOR_BAR_COLOR: u32 = 0xff3030;
const STATUS_BG: u32 = 0x16213e;
const STATUS_FG: u32 = 0x8be9fd;
/// Selection background (matches TUI's `view::apply_selection_bg`). Dracula's
/// "current line" gray reads as a contiguous swath against the editor bg
/// without overpowering syntax-highlighted spans.
const SELECTION_BG: NColor = NColor::Rgb(68, 71, 90);

/// Multiplicative step per Cmd+= / Cmd+- press. 1.1 is the same ratio
/// Chromium uses for browser zoom — small enough that hitting the key twice
/// is meaningful, large enough to feel responsive.
const TEXT_SCALE_STEP: f32 = 1.1;
const MIN_TEXT_SCALE: f32 = 0.5;
const MAX_TEXT_SCALE: f32 = 3.0;

/// Convert a `NColor` to `Hsla`, using a hardcoded white fallback for
/// `Reset` / `Indexed` variants. Suitable for agent theme colors which
/// are always `Color::Rgb` and never need a real fallback.
fn nc(c: NColor) -> Hsla {
    ncolor_to_hsla(c, DEFAULT_FG)
}

fn ncolor_to_hsla(c: NColor, fallback: u32) -> Hsla {
    match c {
        NColor::Reset => rgb(fallback).into(),
        NColor::Black => rgb(0x000000).into(),
        NColor::Red => rgb(0xff5555).into(),
        NColor::Green => rgb(0x50fa7b).into(),
        NColor::Yellow => rgb(0xf1fa8c).into(),
        NColor::Blue => rgb(0x6272a4).into(),
        NColor::Magenta => rgb(0xff79c6).into(),
        NColor::Cyan => rgb(0x8be9fd).into(),
        NColor::Gray => rgb(0xbfbfbf).into(),
        NColor::DarkGray => rgb(0x6272a4).into(),
        NColor::LightRed => rgb(0xff6e6e).into(),
        NColor::LightGreen => rgb(0x69ff94).into(),
        NColor::LightYellow => rgb(0xffffa5).into(),
        NColor::LightBlue => rgb(0xd6acff).into(),
        NColor::LightMagenta => rgb(0xff92df).into(),
        NColor::LightCyan => rgb(0xa4ffff).into(),
        NColor::White => rgb(0xffffff).into(),
        NColor::Indexed(_) => rgb(fallback).into(),
        NColor::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | (b as u32)).into(),
    }
}

/// RGB-packed `u32` for an `NColor`, mirroring [`ncolor_to_hsla`]'s palette.
/// Used where a downstream API wants a `u32` base color (e.g.
/// `styled_line_element`) rather than an `Hsla` — so the theme's foreground
/// reaches per-span text runs instead of the hardcoded `DEFAULT_FG`.
fn ncolor_to_u32(c: NColor, fallback: u32) -> u32 {
    match c {
        NColor::Reset | NColor::Indexed(_) => fallback,
        NColor::Black => 0x000000,
        NColor::Red => 0xff5555,
        NColor::Green => 0x50fa7b,
        NColor::Yellow => 0xf1fa8c,
        NColor::Blue => 0x6272a4,
        NColor::Magenta => 0xff79c6,
        NColor::Cyan => 0x8be9fd,
        NColor::Gray => 0xbfbfbf,
        NColor::DarkGray => 0x6272a4,
        NColor::LightRed => 0xff6e6e,
        NColor::LightGreen => 0x69ff94,
        NColor::LightYellow => 0xffffa5,
        NColor::LightBlue => 0xd6acff,
        NColor::LightMagenta => 0xff92df,
        NColor::LightCyan => 0xa4ffff,
        NColor::White => 0xffffff,
        NColor::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
    }
}

fn fg_or(s: NStyle, fallback: u32) -> Hsla {
    match s.fg {
        Some(c) => ncolor_to_hsla(c, fallback),
        None => rgb(fallback).into(),
    }
}

fn bg_or(s: NStyle, fallback: u32) -> Hsla {
    match s.bg {
        Some(c) => ncolor_to_hsla(c, fallback),
        None => rgb(fallback).into(),
    }
}

/// Tint a background color by blending in a hue at `saturation` and
/// shifting lightness by `lightness_delta`. Used to derive subtle per-turn
/// card backgrounds from the theme's editor_bg.
fn tint_bg(base: Hsla, hue: f32, saturation: f32, lightness_delta: f32) -> Hsla {
    Hsla {
        h: hue,
        s: saturation,
        l: (base.l + lightness_delta).clamp(0.0, 1.0),
        a: base.a,
    }
}

fn font_for(s: NStyle, family: &SharedString) -> Font {
    let weight = if s.modifier.contains(Modifier::BOLD) {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    let style = if s.modifier.contains(Modifier::ITALIC) {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    Font {
        family: family.clone(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style,
    }
}

// ----------------------------------------------------------------------------
// Convert a StyledLine to a GPUI StyledText with TextRuns
// ----------------------------------------------------------------------------

fn styled_line_element(
    line: &StyledLine,
    base_style: NStyle,
    base_fg: u32,
    body_font: &SharedString,
    code_font: &SharedString,
) -> AnyElement {
    // Build the concatenated text and a parallel run list.
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();

    if line.spans.is_empty() {
        // Empty line — render a single zero-width run so layout still allocates a row.
        text.push(' ');
        runs.push(TextRun {
            len: 1,
            font: font_for(base_style, body_font),
            color: rgb(base_fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    } else {
        for span in &line.spans {
            if span.text.is_empty() {
                continue;
            }
            // Combine base + per-span style, with per-span overriding.
            let combined = base_style.patch(span.style);
            let len = span.text.len();
            text.push_str(&span.text);

            // Pick code font for spans whose bg matches code_inline (yellow on dark);
            // simpler proxy: any span with explicit bg uses code font.
            let font = if combined.bg.is_some() || combined.fg == Some(NColor::Rgb(241, 250, 140))
            {
                font_for(combined, code_font)
            } else {
                font_for(combined, body_font)
            };

            let underline = if combined.modifier.contains(Modifier::UNDERLINED) {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(fg_or(combined, base_fg)),
                    wavy: false,
                })
            } else {
                None
            };

            let strikethrough = if combined.modifier.contains(Modifier::CROSSED_OUT) {
                Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(fg_or(combined, base_fg)),
                })
            } else {
                None
            };

            let bg = combined.bg.map(|c| ncolor_to_hsla(c, BG));

            runs.push(TextRun {
                len,
                font,
                color: fg_or(combined, base_fg),
                background_color: bg,
                underline,
                strikethrough,
            });
        }
    }

    if runs.is_empty() {
        // Defensive: every span had empty text.
        text.push(' ');
        runs.push(TextRun {
            len: 1,
            font: font_for(base_style, body_font),
            color: rgb(base_fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    StyledText::new(text).with_runs(runs).into_any_element()
}

/// Doc-view counterpart to `styled_line_element` — same run construction,
/// plus optional selection-background painting (driven by `ctx.doc_selection`
/// projected onto this `(block_idx, line_idx)`) and registration of the
/// resulting `TextLayout` in `ctx.line_layouts` for mouse hit-testing.
///
/// Used only on the view-mode render path; edit-mode and Claude rendering
/// continue to call `styled_line_element` directly.
fn doc_styled_line_element(
    ctx: &RenderCtx<'_>,
    line: &StyledLine,
    base_style: NStyle,
    base_fg: u32,
    body_font: &SharedString,
    code_font: &SharedString,
    line_idx: usize,
) -> AnyElement {
    // Reuse the plain element when this ctx isn't set up for doc-view selection.
    let (block_idx, sink) = match (ctx.current_block, ctx.line_layouts.as_ref()) {
        (Some(b), Some(s)) => (b, s.clone()),
        _ => return styled_line_element(line, base_style, base_fg, body_font, code_font),
    };

    // Build text + runs, identical to `styled_line_element`. We need the
    // intermediate form so we can patch background_color before sealing the
    // StyledText.
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    // Byte range + target for every wiki link on this line. Populated as
    // we walk spans below; used to wrap the StyledText in InteractiveText
    // with on_click handlers.
    let mut wiki_link_ranges: Vec<(std::ops::Range<usize>, String)> = Vec::new();

    if line.spans.is_empty() {
        text.push(' ');
        runs.push(TextRun {
            len: 1,
            font: font_for(base_style, body_font),
            color: rgb(base_fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    } else {
        for span in &line.spans {
            if span.text.is_empty() {
                continue;
            }
            let combined = base_style.patch(span.style);
            let len = span.text.len();
            let span_start = text.len();
            text.push_str(&span.text);
            if let Some(link) = span
                .link
                .as_deref()
                .and_then(|l| l.strip_prefix(WIKI_LINK_PREFIX))
            {
                wiki_link_ranges.push((span_start..span_start + len, link.to_string()));
            }
            let font = if combined.bg.is_some() || combined.fg == Some(NColor::Rgb(241, 250, 140))
            {
                font_for(combined, code_font)
            } else {
                font_for(combined, body_font)
            };
            let underline = if combined.modifier.contains(Modifier::UNDERLINED) {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(fg_or(combined, base_fg)),
                    wavy: false,
                })
            } else {
                None
            };
            let strikethrough = if combined.modifier.contains(Modifier::CROSSED_OUT) {
                Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(fg_or(combined, base_fg)),
                })
            } else {
                None
            };
            let bg = combined.bg.map(|c| ncolor_to_hsla(c, BG));
            runs.push(TextRun {
                len,
                font,
                color: fg_or(combined, base_fg),
                background_color: bg,
                underline,
                strikethrough,
            });
        }
    }
    if runs.is_empty() {
        text.push(' ');
        runs.push(TextRun {
            len: 1,
            font: font_for(base_style, body_font),
            color: rgb(base_fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    // Patch selection background on the runs that overlap the projected
    // char range. Convert char range → byte range against `text`, then
    // walk runs and split any that straddle a boundary.
    if let Some(sel) = ctx.doc_selection {
        let line_chars = styled_line_char_count(line);
        if let Some((s_char, e_char)) = doc_selection_for_line(&sel, block_idx, line_idx, line_chars) {
            let s_byte = char_offset_to_byte_offset(&text, s_char);
            let e_byte = char_offset_to_byte_offset(&text, e_char);
            #[cfg(test)]
            DOC_RENDER_TAP.with(|t| {
                t.borrow_mut().selection.push((block_idx, line_idx, s_byte, e_byte))
            });
            runs = apply_selection_bg_to_runs(runs, s_byte, e_byte, ncolor_to_hsla(SELECTION_BG, BG));
        }
    }

    let styled = StyledText::new(text).with_runs(runs);
    // Capture the line's TextLayout handle. It is registered into the hit-test
    // sink at PAINT time (via RegisterOnPaint), NOT here at build time: the
    // virtualized `gpui::list` builds/measures lines it never prepaints, and
    // `doc_pos_at` calling `.bounds()` on an un-prepainted layout panics across
    // the platform input callback. Registering on paint guarantees every sink
    // entry has bounds set.
    let layout = styled.layout().clone();
    let key = (block_idx, line_idx);

    // Plain text path — no wiki links on this line.
    if wiki_link_ranges.is_empty() {
        return register_line_on_paint(styled.into_any_element(), sink, key, layout);
    }

    // Wrap in InteractiveText so we can attach an on_click handler. Wiki
    // link clicks navigate the focused pane to the target file via
    // `open_wiki_link` on the view (resolved through the weak handle
    // captured in RenderCtx).
    let weak = match &ctx.weak_view {
        Some(w) => w.clone(),
        None => return styled.into_any_element(),
    };
    let doc_dir = ctx.doc_dir.clone();
    let ranges: Vec<std::ops::Range<usize>> =
        wiki_link_ranges.iter().map(|(r, _)| r.clone()).collect();
    let targets: Vec<String> = wiki_link_ranges.into_iter().map(|(_, t)| t).collect();
    let element_id = gpui::ElementId::Name(SharedString::from(format!(
        "wiki-line-{block_idx}-{line_idx}"
    )));
    let el = gpui::InteractiveText::new(element_id, styled)
        .on_click(ranges, move |idx, _w, app| {
            let Some(target) = targets.get(idx) else {
                return;
            };
            let target = target.clone();
            let doc_dir = doc_dir.clone();
            let _ = weak.update(app, |view, cx| {
                view.open_wiki_link(&target, doc_dir.as_deref(), cx);
            });
        })
        .into_any_element();
    register_line_on_paint(el, sink, key, layout)
}

/// Convert a char-index into its byte offset within `s`. Saturates at
/// `s.len()` if `char_offset` is past the end (the selection projection
/// already clamps to `line_char_count`, but `text` here may include the
/// defensive trailing space we add for empty lines).
fn char_offset_to_byte_offset(s: &str, char_offset: usize) -> usize {
    let mut chars_seen = 0;
    for (byte_idx, _) in s.char_indices() {
        if chars_seen == char_offset {
            return byte_idx;
        }
        chars_seen += 1;
    }
    s.len()
}

/// Split runs at `[s_byte, e_byte)` and patch the in-range runs'
/// `background_color`. Runs are sequential and their `len` sums to the
/// total text byte length; we walk byte by byte (run-major) and split
/// at each boundary.
fn apply_selection_bg_to_runs(
    runs: Vec<TextRun>,
    s_byte: usize,
    e_byte: usize,
    bg: Hsla,
) -> Vec<TextRun> {
    if s_byte >= e_byte {
        return runs;
    }
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len() + 2);
    let mut cursor = 0usize;
    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.len;
        cursor = run_end;
        if run_end <= s_byte || run_start >= e_byte {
            out.push(run);
            continue;
        }
        // Split into pre / mid / post relative to [s_byte, e_byte).
        let mid_s = s_byte.max(run_start);
        let mid_e = e_byte.min(run_end);
        if run_start < mid_s {
            let mut pre = run.clone();
            pre.len = mid_s - run_start;
            out.push(pre);
        }
        if mid_s < mid_e {
            let mut mid = run.clone();
            mid.len = mid_e - mid_s;
            mid.background_color = Some(bg);
            out.push(mid);
        }
        if mid_e < run_end {
            let mut post = run.clone();
            post.len = run_end - mid_e;
            out.push(post);
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Block → Element
// ----------------------------------------------------------------------------

struct RenderCtx<'a> {
    theme: &'a Theme,
    body_font: SharedString,
    code_font: SharedString,
    /// Cmd-zoom multiplier on body text. 1.0 = unzoomed. Set to 1.0 in
    /// contexts where zoom shouldn't apply (e.g. the Claude session block
    /// renderer).
    text_scale: f32,
    cursor_block: Option<usize>,
    /// Active doc-view mouse selection, used to paint background on
    /// participating lines. `None` outside the view-mode render path.
    doc_selection: Option<DocSelection>,
    /// Side channel for line-layout registration. Lines store their cloned
    /// `TextLayout` here keyed by `(block_idx, line_idx)` so mouse handlers
    /// on the doc body can hit-test against bounds and map pixels → bytes.
    /// `None` outside the view-mode render path (e.g. edit-mode rendering
    /// and nested ctxes inside blockquotes/lists where v1 doesn't yet
    /// support selection).
    line_layouts: Option<std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>>,
    /// The top-level block index currently being rendered. Set by
    /// `block_element` and cleared (set to `None`) when `block_inner`
    /// recurses into nested blocks (blockquote/list content), so the v1
    /// "top-level only" selection scope is enforced naturally.
    current_block: Option<usize>,
    /// Weak handle on the view, captured so click handlers built inside
    /// free render functions (`doc_styled_line_element`, etc.) can call
    /// back into the view for wiki-link navigation. `None` outside the
    /// view-mode render path.
    weak_view: Option<gpui::WeakEntity<SketchGpuiView>>,
    /// Directory of the currently focused Doc, used to resolve wiki link
    /// targets (`[[notes]]` → `<doc_dir>/notes.md`). `None` outside the
    /// view-mode render path or when the doc has no parent dir.
    doc_dir: Option<PathBuf>,
}

/// A transparent element wrapper that registers a doc line's `TextLayout` into
/// the hit-test sink (`line_layouts`) **only when the line is actually
/// painted** — i.e. in `paint`, after `prepaint` has set the layout's bounds.
///
/// Why this exists: under the virtualized doc `gpui::list`, lines get built (and
/// sometimes measured) that are never prepainted, so registering at build time
/// put `bounds == None` layouts in the sink. `doc_pos_at` then iterates the sink
/// and calls `.bounds()` (which `expect`s prepaint) → panic across the platform
/// input callback ("cannot unwind" → abort). Registering on paint guarantees
/// every sink entry has bounds.
struct RegisterOnPaint {
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>,
    key: (usize, usize),
    layout: TextLayout,
}

fn register_line_on_paint(
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>,
    key: (usize, usize),
    layout: TextLayout,
) -> AnyElement {
    RegisterOnPaint { inner, sink, key, layout }.into_any_element()
}

impl IntoElement for RegisterOnPaint {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RegisterOnPaint {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.inner.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // prepaint has run → the layout's bounds are set. Registering here means
        // `doc_pos_at` only ever sees prepainted (bounds-Some) layouts.
        self.sink.borrow_mut().insert(self.key, self.layout.clone());
        #[cfg(test)]
        DOC_RENDER_TAP.with(|t| t.borrow_mut().painted.push(self.key));
        self.inner.paint(window, cx);
    }
}

fn block_element(ctx: &RenderCtx<'_>, idx: usize, block: &RenderedBlock) -> AnyElement {
    let highlighted = ctx.cursor_block == Some(idx);
    #[cfg(test)]
    if highlighted {
        DOC_RENDER_TAP.with(|t| t.borrow_mut().cursor_bar_block = Some(idx));
    }
    let inner_ctx = RenderCtx {
        theme: ctx.theme,
        body_font: ctx.body_font.clone(),
        code_font: ctx.code_font.clone(),
        text_scale: ctx.text_scale,
        cursor_block: ctx.cursor_block,
        doc_selection: ctx.doc_selection,
        line_layouts: ctx.line_layouts.clone(),
        current_block: Some(idx),
        weak_view: ctx.weak_view.clone(),
        doc_dir: ctx.doc_dir.clone(),
    };
    let base = block_inner(&inner_ctx, block);

    // Wrap with a left "cursor bar" indicator when this is the focused block.
    div()
        .flex()
        .flex_row()
        .items_start()
        .w_full()
        .mb_2()
        .child(
            div()
                .w(px(3.0))
                .flex_none()
                .h_full()
                .bg(if highlighted {
                    rgb(CURSOR_BAR_COLOR)
                } else {
                    rgba(0x00000000)
                }),
        )
        .child(div().pl_3().flex_1().min_w_0().child(base))
        .into_any_element()
}

fn block_inner(ctx: &RenderCtx<'_>, block: &RenderedBlock) -> AnyElement {
    match block {
        RenderedBlock::Heading { level, content } => {
            let lvl = (*level as usize).clamp(1, 6) - 1;
            let style = ctx.theme.heading[lvl];
            let size_px = match level {
                1 => 28.0,
                2 => 24.0,
                3 => 20.0,
                4 => 18.0,
                5 => 16.0,
                _ => 15.0,
            };
            div()
                .text_size(px(size_px * ctx.text_scale))
                .font_weight(FontWeight::BOLD)
                .text_color(fg_or(style, DEFAULT_FG))
                .pb_1()
                .child(doc_styled_line_element(
                    ctx,
                    content,
                    style,
                    DEFAULT_FG,
                    &ctx.body_font,
                    &ctx.code_font,
                    0,
                ))
                .into_any_element()
        }
        RenderedBlock::Paragraph { lines } => {
            // GPUI flex wraps long text inside a fixed-width container — but
            // StyledText itself doesn't word-wrap, so we render each input
            // line as its own row and rely on per-line spans. This matches
            // the TUI behaviour where pulldown emits separate StyledLines.
            let base = ctx.theme.paragraph;
            let mut col = div()
                .flex()
                .flex_col()
                .text_color(fg_or(base, DEFAULT_FG));
            for (li, line) in lines.iter().enumerate() {
                col = col.child(doc_styled_line_element(
                    ctx,
                    line,
                    base,
                    DEFAULT_FG,
                    &ctx.body_font,
                    &ctx.code_font,
                    li,
                ));
            }
            col.into_any_element()
        }
        RenderedBlock::CodeBlock { language, lines, source_file } => {
            let mut col = div()
                .flex()
                .flex_col()
                .font_family(ctx.code_font.clone())
                .text_color(rgb(DEFAULT_FG));
            if *source_file {
                // Source file: no container chrome — code IS the document.
            } else {
                // Fenced code block inside markdown: tinted background + padding.
                let bg = ctx.theme.code_block_bg;
                col = col
                    .p_2()
                    .rounded_md()
                    .bg(bg_or(bg, BG));
            }
            if !*source_file {
                if let Some(lang) = language {
                    if !lang.is_empty() {
                        col = col.child(
                            div()
                                .text_color(rgb(0x6272a4))
                                .text_size(px(11.0))
                                .pb_1()
                                .child(format!("[{}]", lang)),
                        );
                    }
                }
            }
            let row_style = NStyle::default();
            for (li, line) in lines.iter().enumerate() {
                col = col.child(doc_styled_line_element(
                    ctx,
                    line,
                    row_style,
                    DEFAULT_FG,
                    &ctx.code_font,
                    &ctx.code_font,
                    li,
                ));
            }
            col.into_any_element()
        }
        RenderedBlock::BlockQuote { blocks } => {
            let bar = ctx.theme.blockquote_bar;
            let txt = ctx.theme.blockquote_text;
            let mut content = div()
                .flex()
                .flex_col()
                .pl_3()
                .text_color(fg_or(txt, DEFAULT_FG))
                .italic();
            for (i, b) in blocks.iter().enumerate() {
                content = content.child(block_inner(
                    &RenderCtx {
                        theme: ctx.theme,
                        body_font: ctx.body_font.clone(),
                        code_font: ctx.code_font.clone(),
                        text_scale: ctx.text_scale,
                        cursor_block: None,
                        doc_selection: None,
                        line_layouts: None,
                        current_block: None,
                        // Wiki links should still be clickable inside
                        // nested blocks (blockquotes, list items) — only
                        // selection is scoped top-level.
                        weak_view: ctx.weak_view.clone(),
                        doc_dir: ctx.doc_dir.clone(),
                    },
                    b,
                ));
                let _ = i;
            }
            div()
                .flex()
                .flex_row()
                .child(div().w(px(3.0)).h_full().bg(fg_or(bar, 0xffb86c)))
                .child(content)
                .into_any_element()
        }
        RenderedBlock::List {
            ordered,
            start,
            items,
        } => {
            let marker_style = ctx.theme.list_marker;
            let mut col = div().flex().flex_col();
            let mut counter = start.unwrap_or(1);
            for item in items {
                col = col.child(list_item_element(
                    ctx,
                    item,
                    *ordered,
                    counter,
                    marker_style,
                ));
                counter += 1;
            }
            col.into_any_element()
        }
        RenderedBlock::Table {
            headers,
            rows,
            alignments,
        } => table_element(ctx, headers, rows, alignments),
        RenderedBlock::HorizontalRule => {
            let s = ctx.theme.horizontal_rule;
            div()
                .h(px(1.0))
                .my_2()
                .bg(fg_or(s, 0x6272a4))
                .into_any_element()
        }
        RenderedBlock::Image { alt, url } => {
            let s = ctx.theme.image_label;
            div()
                .text_color(fg_or(s, 0xffb86c))
                .italic()
                .child(format!("[image: {} <{}>]", alt, url))
                .into_any_element()
        }
    }
}

fn list_item_element(
    ctx: &RenderCtx<'_>,
    item: &ListItem,
    ordered: bool,
    counter: u64,
    marker_style: NStyle,
) -> AnyElement {
    let marker = if !item.marker.is_empty() {
        item.marker.clone()
    } else if let Some(checked) = item.checked {
        if checked { "[x]".into() } else { "[ ]".into() }
    } else if ordered {
        format!("{}.", counter)
    } else {
        "•".into()
    };

    let mut content_col = div().flex().flex_col().flex_1().min_w_0();
    for b in &item.content {
        content_col = content_col.child(block_inner(
            &RenderCtx {
                theme: ctx.theme,
                body_font: ctx.body_font.clone(),
                code_font: ctx.code_font.clone(),
                text_scale: ctx.text_scale,
                cursor_block: None,
                doc_selection: None,
                line_layouts: None,
                current_block: None,
                weak_view: ctx.weak_view.clone(),
                doc_dir: ctx.doc_dir.clone(),
            },
            b,
        ));
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .child(
            div()
                .min_w(px(24.0))
                .text_color(fg_or(marker_style, 0x50fa7b))
                .font_weight(FontWeight::BOLD)
                .child(marker),
        )
        .child(content_col)
        .into_any_element()
}

fn table_element(
    ctx: &RenderCtx<'_>,
    headers: &[StyledLine],
    rows: &[Vec<StyledLine>],
    _alignments: &[ColumnAlignment],
) -> AnyElement {
    let border = ctx.theme.table_border;
    let header_style = ctx.theme.table_header;
    let body_style = ctx.theme.paragraph;
    let border_color = fg_or(border, 0x6272a4);

    let mut table = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(border_color)
        .rounded_md();

    // Header row
    let mut header_row = div().flex().flex_row().bg(rgba(0x44475a40));
    for (i, h) in headers.iter().enumerate() {
        let mut cell = div()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .text_color(fg_or(header_style, DEFAULT_FG))
            .font_weight(FontWeight::BOLD);
        if i + 1 < headers.len() {
            cell = cell.border_r_1().border_color(border_color);
        }
        header_row = header_row.child(cell.child(styled_line_element(
            h,
            header_style,
            DEFAULT_FG,
            &ctx.body_font,
            &ctx.code_font,
        )));
    }
    table = table.child(header_row);

    // Body rows
    for (ri, row) in rows.iter().enumerate() {
        let mut row_div = div()
            .flex()
            .flex_row()
            .border_t_1()
            .border_color(border_color);
        let _ = ri;
        for (i, c) in row.iter().enumerate() {
            let mut cell = div()
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .text_color(fg_or(body_style, DEFAULT_FG));
            if i + 1 < row.len() {
                cell = cell.border_r_1().border_color(border_color);
            }
            row_div = row_div.child(cell.child(styled_line_element(
                c,
                body_style,
                DEFAULT_FG,
                &ctx.body_font,
                &ctx.code_font,
            )));
        }
        table = table.child(row_div);
    }

    table.into_any_element()
}

// ----------------------------------------------------------------------------
// Edit-mode helpers (md_highlight segments → renderable pieces)
// ----------------------------------------------------------------------------

/// Convert a flat segment list to a `StyledLine` so we can reuse
/// `styled_line_element` for the rendering. md_highlight produces
/// `(String, Style)` per chunk; `StyledLine` wraps that as `StyledSpan`s.
fn segments_to_styled_line(segs: &[Segment]) -> StyledLine {
    StyledLine {
        spans: segs
            .iter()
            .map(|(text, style)| StyledSpan {
                text: text.clone(),
                style: *style,
                link: None,
            })
            .collect(),
    }
}

/// Split styled segments at character column `col` into
/// `(before_segs, (at_char, at_style), after_segs)`. If `col` is past the
/// last character, `at_char` is a virtual space (cursor at end of line).
/// Used by the cursor-line render path: before/after each go through
/// `segments_to_styled_line` while the at_char drives the caret cell.
fn split_segments_at_col(segs: &[Segment], col: usize) -> (Vec<Segment>, (char, NStyle), Vec<Segment>) {
    let mut before: Vec<Segment> = Vec::new();
    let mut at: Option<(char, NStyle)> = None;
    let mut after: Vec<Segment> = Vec::new();
    let mut cumulative = 0usize;
    let mut last_style = NStyle::default();

    for (text, style) in segs {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        if len > 0 {
            last_style = *style;
        }
        if at.is_some() {
            after.push((text.clone(), *style));
            cumulative += len;
            continue;
        }
        if cumulative + len <= col {
            before.push((text.clone(), *style));
            cumulative += len;
        } else {
            let local = col - cumulative;
            let pre: String = chars[..local].iter().collect();
            let ch = chars[local];
            let post: String = chars[local + 1..].iter().collect();
            if !pre.is_empty() {
                before.push((pre, *style));
            }
            at = Some((ch, *style));
            if !post.is_empty() {
                after.push((post, *style));
            }
            cumulative += len;
        }
    }

    let at = at.unwrap_or((' ', last_style));
    (before, at, after)
}

/// Doc-view mouse selection. Coordinates are (block_idx, line_idx within
/// the block's rendered lines, char_offset within that line). `dragging`
/// is true between MouseDown and MouseUp on the doc body — during that
/// window every MouseMove updates `head`. Once `dragging` is false the
/// range is frozen and Cmd-C reads from it.
#[derive(Clone, Copy, Debug)]
struct DocSelection {
    anchor: DocPos,
    head: DocPos,
    dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DocPos {
    block_idx: usize,
    line_idx: usize,
    char_offset: usize,
}

impl DocSelection {
    fn normalized(&self) -> (DocPos, DocPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Char count of a StyledLine (sum of `chars().count()` over all spans).
fn styled_line_char_count(line: &StyledLine) -> usize {
    line.spans.iter().map(|s| s.text.chars().count()).sum()
}

/// Prefix on `StyledSpan.link` that marks a wiki-style link target
/// (`[[note]]` in markdown). The doc-view click handler treats spans with
/// this prefix as file references — anything else is a regular markdown
/// link and is left alone for now.
const WIKI_LINK_PREFIX: &str = "wiki:";

/// Map a file extension to a syntect language token. Returns `None` for
/// markdown and unknown extensions — those are rendered as prose.
fn lang_for_path(path: &std::path::Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Some("rust"),
        Some("py") | Some("pyi") => Some("python"),
        Some("js") | Some("mjs") | Some("cjs") => Some("javascript"),
        Some("ts") | Some("mts") | Some("cts") => Some("typescript"),
        Some("tsx") => Some("tsx"),
        Some("jsx") => Some("jsx"),
        Some("c") | Some("h") => Some("c"),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") | Some("hxx") | Some("hh") => {
            Some("cpp")
        }
        Some("go") => Some("go"),
        Some("java") => Some("java"),
        Some("rb") => Some("ruby"),
        Some("sh") | Some("bash") | Some("zsh") => Some("bash"),
        Some("css") => Some("css"),
        Some("html") | Some("htm") => Some("html"),
        Some("json") => Some("json"),
        Some("xml") => Some("xml"),
        Some("yaml") | Some("yml") => Some("yaml"),
        Some("toml") => Some("toml"),
        Some("sql") => Some("sql"),
        Some("swift") => Some("swift"),
        Some("kt") | Some("kts") => Some("kotlin"),
        Some("lua") => Some("lua"),
        Some("r") | Some("R") => Some("r"),
        Some("zig") => Some("zig"),
        Some("hs") => Some("haskell"),
        Some("ex") | Some("exs") => Some("elixir"),
        Some("erl") => Some("erlang"),
        Some("ml") | Some("mli") => Some("ocaml"),
        Some("scala") | Some("sc") => Some("scala"),
        Some("cs") => Some("c#"),
        Some("fs") | Some("fsx") => Some("f#"),
        Some("pl") | Some("pm") => Some("perl"),
        Some("php") => Some("php"),
        Some("dart") => Some("dart"),
        Some("m") => Some("objective-c"),
        Some("clj") | Some("cljs") => Some("clojure"),
        Some("el") => Some("lisp"),
        Some("vim") => Some("viml"),
        Some("cmake") => Some("cmake"),
        Some("dockerfile") | Some("Dockerfile") => Some("dockerfile"),
        Some("tf") | Some("hcl") => Some("hcl"),
        Some("proto") => Some("protobuf"),
        Some("graphql") | Some("gql") => Some("graphql"),
        _ => None,
    }
}

/// Render markdown to blocks + post-process `[[name]]` / `[[name|display]]`
/// patterns into link-bearing spans. pulldown-cmark doesn't understand
/// wiki links, so they arrive as plain text and we rewrite them after
/// rendering. Reuses the existing `StyledSpan.link` channel so click
/// handling stays uniform with regular markdown links.
///
/// When `path` points to a recognised source file (`.rs`, `.py`, etc.),
/// the text is highlighted with syntect and returned as a single
/// `CodeBlock` with `source_file: true` so the renderer skips container
/// chrome (background tint, padding, rounded corners).
fn render_with_wiki(text: &str, theme: &Theme, path: Option<&std::path::Path>) -> Vec<RenderedBlock> {
    if let Some(lang) = path.and_then(lang_for_path) {
        let hl = sketch::highlight::Highlighter::with_syntect_theme(theme.name.syntect_theme());
        // Use a transparent base style — source files render against the
        // normal document background, not the code-block tint.
        let base = sketch::style::Style::default();
        let lines = hl
            .highlight(lang, text, base)
            .unwrap_or_else(|| {
                // Fallback: plain text with default style.
                text.lines()
                    .map(|l| StyledLine::new(vec![StyledSpan::new(l, theme.paragraph)]))
                    .collect()
            });
        return vec![RenderedBlock::CodeBlock {
            language: Some(lang.to_string()),
            lines,
            source_file: true,
        }];
    }
    let mut blocks = render::render(text, theme);
    expand_wiki_links_in_blocks(&mut blocks, theme);
    blocks
}

fn expand_wiki_links_in_blocks(blocks: &mut Vec<RenderedBlock>, theme: &Theme) {
    for b in blocks.iter_mut() {
        expand_wiki_links_in_block(b, theme);
    }
}

fn expand_wiki_links_in_block(block: &mut RenderedBlock, theme: &Theme) {
    match block {
        RenderedBlock::Heading { content, .. } => expand_wiki_links_in_line(content, theme),
        RenderedBlock::Paragraph { lines } => {
            for line in lines.iter_mut() {
                expand_wiki_links_in_line(line, theme);
            }
        }
        // Code blocks: deliberately untouched. `[[foo]]` inside a fenced
        // block is code text, not a link.
        RenderedBlock::CodeBlock { .. } => {}
        RenderedBlock::BlockQuote { blocks } => expand_wiki_links_in_blocks(blocks, theme),
        RenderedBlock::List { items, .. } => {
            for item in items.iter_mut() {
                expand_wiki_links_in_blocks(&mut item.content, theme);
            }
        }
        RenderedBlock::Table { headers, rows, .. } => {
            for h in headers.iter_mut() {
                expand_wiki_links_in_line(h, theme);
            }
            for row in rows.iter_mut() {
                for cell in row.iter_mut() {
                    expand_wiki_links_in_line(cell, theme);
                }
            }
        }
        RenderedBlock::HorizontalRule | RenderedBlock::Image { .. } => {}
    }
}

fn expand_wiki_links_in_line(line: &mut StyledLine, theme: &Theme) {
    let mut new_spans: Vec<StyledSpan> = Vec::with_capacity(line.spans.len());
    for span in line.spans.drain(..) {
        // Skip spans that already carry a link (real markdown links) —
        // wiki rewriting only applies to plain text.
        if span.link.is_some() {
            new_spans.push(span);
            continue;
        }
        new_spans.extend(split_wiki_links(&span, theme));
    }
    line.spans = new_spans;
}

fn split_wiki_links(span: &StyledSpan, theme: &Theme) -> Vec<StyledSpan> {
    let text = &span.text;
    if !text.contains("[[") {
        return vec![span.clone()];
    }
    let mut out: Vec<StyledSpan> = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        let Some(rel_start) = text[cursor..].find("[[") else {
            out.push(StyledSpan {
                text: text[cursor..].to_string(),
                style: span.style,
                link: None,
            });
            break;
        };
        let abs_start = cursor + rel_start;
        let inner_start = abs_start + 2;
        let Some(rel_end) = text[inner_start..].find("]]") else {
            // No closing — emit the rest verbatim and stop.
            out.push(StyledSpan {
                text: text[cursor..].to_string(),
                style: span.style,
                link: None,
            });
            break;
        };
        let abs_close = inner_start + rel_end;
        // Emit the chunk before the `[[`.
        if abs_start > cursor {
            out.push(StyledSpan {
                text: text[cursor..abs_start].to_string(),
                style: span.style,
                link: None,
            });
        }
        let inner = &text[inner_start..abs_close];
        let (target, display) = match inner.find('|') {
            Some(pipe) => (inner[..pipe].trim(), inner[pipe + 1..].trim()),
            None => (inner.trim(), inner.trim()),
        };
        // Empty `[[]]` — keep raw text, don't make it a link.
        if target.is_empty() {
            out.push(StyledSpan {
                text: text[abs_start..abs_close + 2].to_string(),
                style: span.style,
                link: None,
            });
        } else {
            out.push(StyledSpan {
                text: display.to_string(),
                style: theme.link,
                link: Some(format!("{}{}", WIKI_LINK_PREFIX, target)),
            });
        }
        cursor = abs_close + 2;
    }
    out
}

/// Walk a `Layout` tree and re-render every Doc window's `blocks` against
/// `theme`. Called on theme switch. Edit / Browser / Claude windows don't
/// cache theme-styled output, so they pick up the new palette on next
/// paint without needing intervention here.
fn re_render_layout_docs(layout: &mut workspace::Layout<WindowContent>, theme: &Theme) {
    match layout {
        workspace::Layout::Empty => {}
        workspace::Layout::Leaf(win) => {
            if let WindowContent::Doc(d) = &mut win.content {
                let path = PathBuf::from(d.file_label.as_ref());
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let doc = Document::from_text(text, path.clone());
                d.set_blocks(render_with_wiki(&doc.full_text(), theme, Some(&path)));
            }
            // Browser's underlying-stashed content is also restyled if it
            // happens to be a Doc — otherwise reverting via Esc lands on
            // stale-themed blocks.
            if let WindowContent::Browser(b) = &mut win.content {
                if let Some(under) = b.underlying.as_deref_mut() {
                    if let WindowContent::Doc(d) = under {
                        let path = PathBuf::from(d.file_label.as_ref());
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        let doc = Document::from_text(text, path.clone());
                        d.set_blocks(render_with_wiki(&doc.full_text(), theme, Some(&path)));
                    }
                }
            }
        }
        workspace::Layout::Split { children, .. } => {
            for (_, child) in children.iter_mut() {
                re_render_layout_docs(child, theme);
            }
        }
    }
}

/// Return the `StyledLine`s of a block that v1 view-mode selection treats
/// as selectable, in the same order line_idx is assigned during render.
/// Blocks not covered (BlockQuote, List, Table, HorizontalRule, Image)
/// produce an empty slice — they remain unselectable for now.
fn block_selectable_lines(block: &RenderedBlock) -> &[StyledLine] {
    match block {
        RenderedBlock::Heading { content, .. } => std::slice::from_ref(content),
        RenderedBlock::Paragraph { lines } => lines.as_slice(),
        RenderedBlock::CodeBlock { lines, .. } => lines.as_slice(),
        _ => &[],
    }
}

/// Project a `DocSelection` onto a single rendered line. Returns
/// `[start_col, end_col)` in characters or `None` if the line is outside
/// the selection.
fn doc_selection_for_line(
    sel: &DocSelection,
    block_idx: usize,
    line_idx: usize,
    line_char_count: usize,
) -> Option<(usize, usize)> {
    let (start, end) = sel.normalized();
    if (block_idx, line_idx) < (start.block_idx, start.line_idx) {
        return None;
    }
    if (block_idx, line_idx) > (end.block_idx, end.line_idx) {
        return None;
    }
    let s = if (block_idx, line_idx) == (start.block_idx, start.line_idx) {
        start.char_offset.min(line_char_count)
    } else {
        0
    };
    let e = if (block_idx, line_idx) == (end.block_idx, end.line_idx) {
        end.char_offset.min(line_char_count)
    } else {
        line_char_count
    };
    if s < e {
        Some((s, e))
    } else {
        None
    }
}

/// Project a document-level selection range onto a single line. Returns
/// `[start_col, end_col)` clamped to the line's character count, or `None`
/// if the line is fully outside the selection. Mirrors view.rs's projection
/// (lines fully inside multi-line selections get `(0, line_char_count)`;
/// the first/last lines get the partial range).
fn line_selection_range(
    sel: ((usize, usize), (usize, usize)),
    line_idx: usize,
    line_char_count: usize,
) -> Option<(usize, usize)> {
    let ((sl, sc), (el, ec)) = sel;
    if line_idx < sl || line_idx > el {
        return None;
    }
    let start = if line_idx == sl { sc } else { 0 };
    let end = if line_idx == el {
        ec.min(line_char_count)
    } else {
        line_char_count
    };
    if start <= end {
        Some((start, end))
    } else {
        None
    }
}

/// Per-line classification used by the Word-Processor renderer to pick
/// typography (font size, weight, family, decoration). Drives ONLY the
/// rendering choices — the source line text is unchanged and md_highlight's
/// segments still carry the inline styling for `**bold**` / `*italic*` /
/// `` `code` ``.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WpLineKind {
    Empty,
    /// Atx heading; `level` is 1..=6 (number of leading `#`s).
    Heading(u8),
    /// Unordered list item — `-`, `*`, or `+` followed by space.
    BulletItem,
    /// Ordered list item — digits + `.` or `)` followed by space.
    OrderedItem,
    /// Blockquote line — starts with `>`.
    Blockquote,
    /// ` ``` ` or `~~~` delimiter line (toggles fence state).
    CodeFence,
    /// Line inside an open fence — rendered monospace + bg.
    CodeContent,
    /// Heuristic: line containing two or more `|` characters. For MVP we
    /// render these as monospace pre-formatted text rather than as a styled
    /// table block (which would require detecting a contiguous run of
    /// `|`-rows + a delimiter row + extracting cells).
    TableRow,
    /// Anything else — a normal paragraph line.
    Paragraph,
}

/// Classify a single source line. `in_fence` is the fence state on entry to
/// this line (true if the previous line opened a fence and we haven't seen a
/// closer yet). Caller is responsible for toggling its tracking flag when
/// the result is `CodeFence`.
fn classify_wp_line(text: &str, in_fence: bool) -> WpLineKind {
    let trimmed = text.trim_start();

    if in_fence {
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            return WpLineKind::CodeFence;
        }
        return WpLineKind::CodeContent;
    }

    if trimmed.is_empty() {
        return WpLineKind::Empty;
    }

    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return WpLineKind::CodeFence;
    }

    // Heading: 1-6 leading `#`s, then space (or EOL). Mirrors md_highlight's
    // `try_heading` rule so classification agrees with the segment styling.
    let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hash_count) {
        let after = &trimmed[hash_count..];
        if after.is_empty() || after.starts_with(' ') {
            return WpLineKind::Heading(hash_count as u8);
        }
    }

    // Unordered list: -, *, + followed by a space.
    let mut chars = trimmed.chars();
    if let Some(c) = chars.next() {
        if matches!(c, '-' | '*' | '+') && chars.next() == Some(' ') {
            return WpLineKind::BulletItem;
        }
    }

    // Ordered list: digits + (`.` | `)`) + space.
    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count > 0 {
        let after = &trimmed[digit_count..];
        let mut after_chars = after.chars();
        if matches!(after_chars.next(), Some('.') | Some(')'))
            && after_chars.next() == Some(' ')
        {
            return WpLineKind::OrderedItem;
        }
    }

    if trimmed.starts_with('>') {
        return WpLineKind::Blockquote;
    }

    if text.matches('|').count() >= 2 {
        return WpLineKind::TableRow;
    }

    WpLineKind::Paragraph
}

/// Walk segments char by char, applying `bg` to chars whose column falls in
/// `[start_col, end_col)`. Output may have more segments than input (a single
/// styled run can split across the selection boundary). Direct port of
/// `view::apply_selection_bg` so visual behavior matches the TUI.
fn apply_selection_bg(
    segs: &[Segment],
    start_col: usize,
    end_col: usize,
    bg: NColor,
) -> Vec<Segment> {
    let mut result: Vec<Segment> = Vec::new();
    let mut col = 0usize;
    for (text, style) in segs {
        let mut current_text = String::new();
        let mut current_style = *style;
        let mut started = false;
        for ch in text.chars() {
            let is_selected = col >= start_col && col < end_col;
            let new_style = if is_selected { style.bg(bg) } else { *style };
            if started && new_style != current_style {
                result.push((std::mem::take(&mut current_text), current_style));
                current_style = new_style;
            } else if !started {
                current_style = new_style;
                started = true;
            }
            current_text.push(ch);
            col += 1;
        }
        if !current_text.is_empty() {
            result.push((current_text, current_style));
        }
    }
    result
}

/// Render a single line's content (the part *after* any gutter / list marker
/// decoration). On the cursor's line this splices a caret div between the
/// before/after styled-text halves; on other lines it just renders the
/// segments. Shared by both Code and Word-Processor body builders.
///
/// `line_font` is the typography font for non-code spans (monospace in Code
/// view, proportional in WP view); `code_font` is always the monospace font
/// `styled_line_element` falls back to for spans with a code background.
#[allow(clippy::too_many_arguments)]
fn build_line_content(
    segs: &[Segment],
    line_str: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    base_style: NStyle,
    base_fg: u32,
    line_font: &SharedString,
    code_font: &SharedString,
) -> AnyElement {
    if !is_cursor_line {
        let line = if segs.is_empty() {
            // Empty source line — emit a placeholder row so the line height
            // doesn't collapse to zero.
            segments_to_styled_line(&[(" ".to_string(), base_style)])
        } else {
            segments_to_styled_line(segs)
        };
        return styled_line_element(&line, base_style, base_fg, line_font, code_font);
    }

    let total_chars = line_str.chars().count();
    let col = cursor_col.min(total_chars);
    let (before, (at_char, _at_style), after) = split_segments_at_col(segs, col);

    // Block cursor in both modes — character under the cursor is shown
    // inside the block. In insert mode the block sits *before* the at_char
    // (the character shifts right); in normal mode the block *replaces* it.
    let caret = div()
        .flex_none()
        .w(px(8.0))
        .h(px(18.0))
        .bg(cursor_color)
        .text_color(rgb(BG))
        .child(if mode == EditMode::Normal { at_char.to_string() } else { " ".into() })
        .into_any_element();

    // In insert mode the at_char isn't consumed by the block so it appears
    // in the after-stream with its original style.
    let after_segs = match mode {
        EditMode::Normal => after,
        EditMode::Insert => {
            let mut s = vec![(at_char.to_string(), base_style)];
            s.extend(after);
            s
        }
    };

    let mut row = div().flex().flex_row();
    if !before.is_empty() {
        row = row.child(styled_line_element(
            &segments_to_styled_line(&before),
            base_style,
            base_fg,
            line_font,
            code_font,
        ));
    }
    row = row.child(caret);
    if !after_segs.is_empty() {
        row = row.child(styled_line_element(
            &segments_to_styled_line(&after_segs),
            base_style,
            base_fg,
            line_font,
            code_font,
        ));
    }
    row.into_any_element()
}

/// Path to the JSON file that maps cwd → list of ACP session slots. Lives
/// next to `debug.log` so all sketch-managed transient state stays in one
/// place.
fn acp_session_persist_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("sketch").join("acp_sessions.json"))
}

/// Sketch's process cwd, with a safe fallback. Used both as the default
/// per-session cwd for new agent slots (spec-agent-cwd.md §1) and as the
/// top-level key in `acp_sessions.json` / `workspace.json`.
fn process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Canonicalize a path for resume-matching, falling back to the path verbatim
/// when it can't be resolved (e.g. it no longer exists). Both the stored
/// session cwd and the current cwd go through this before comparison, so a
/// symlinked / non-normalized launch directory still matches its saved session
/// instead of silently falling into the "create a fresh session" branch (which
/// is what made a resumed session look like it was "replaced" by a new one).
/// Comparing raw-vs-raw on a canonicalize failure preserves the old exact-match
/// behavior with no regression.
fn cwd_match_key(p: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// THE canonical on-disk key for a cwd (ADR-0010 / D5). Every cwd-keyed map in
/// our persistence files (`workspace.json`, the ACP-session file) keys through
/// this, so a symlinked / non-normalized / `/tmp`-vs-`/private/tmp` launch dir
/// resolves to the SAME string the entry was saved under — instead of silently
/// missing and resurrecting the workspace/session as empty. Mirrors
/// [`cwd_match_key`] (the resume *filter* key); this is its on-disk twin.
/// Falls back to the raw spelling when `canonicalize` fails (deleted path), so
/// a transient stat failure never regresses to never-matching.
fn persist_cwd_key(cwd: &std::path::Path) -> String {
    cwd_match_key(cwd).to_string_lossy().into_owned()
}

/// Attach to a server session, retrying an Owner attach briefly when the
/// server still reports a previous owner.
///
/// The session server is persistent and outlives the GUI, so on app reboot the
/// pre-reboot connection's `detach` — which releases ownership when its socket
/// closes — can be processed by the server *after* the freshly-launched process
/// issues its attach. A one-shot Owner attach then loses that race and returns
/// "another GUI already owns this session"; the old code swallowed the error
/// and bound the slot anyway, leaving a session that received no events and
/// whose prompts the server silently rejected (`prompt` is fire-and-forget, so
/// the rejection never surfaced) — the "resume never responds" bug.
///
/// Retrying for a bounded window lets the stale owner clear. If the window
/// expires still-owned (a genuinely live peer, or a wedged connection), fall
/// back to an Observer attach so the replay/live stream is at least received,
/// and report `Ok(false)` so the caller can surface "not the owner". Errors
/// other than ownership contention are returned immediately.
///
/// Runs on the background executor (never the paint thread), so the bounded
/// `sleep` between tries is safe.
fn attach_with_owner_retry(
    handle: &sketch::session_client::SessionServerHandle,
    sid: &str,
    want_owner: bool,
) -> Result<bool, String> {
    if !want_owner {
        return handle
            .attach(sid, AttachMode::Observer)
            .map(|_| false)
            .map_err(|e| e.to_string());
    }
    let mut last_err = String::new();
    // ~2.4s total (8 × 300ms) — comfortably longer than the socket-close →
    // detach window on a clean reboot, short enough not to stall the open.
    for _ in 0..8 {
        match handle.attach(sid, AttachMode::Owner) {
            Ok(()) => return Ok(true),
            Err(e) => {
                last_err = e.to_string();
                // Only ownership contention is transient; anything else is fatal.
                if !last_err.contains("already own") {
                    return Err(last_err);
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    // Still owned after the window: subscribe as an observer so the transcript
    // still replays, and tell the caller we are not the owner.
    match handle.attach(sid, AttachMode::Observer) {
        Ok(()) => Ok(false),
        Err(e) => Err(format!("{last_err}; observer fallback failed: {e}")),
    }
}

/// Whether this process was launched as a build-loop candidate.
fn is_candidate_launch() -> bool {
    std::env::var("SKETCH_CANDIDATE").as_deref() == Ok("1")
}

/// Connect to the session server, the default model: a persistent server owns
/// the agent subprocesses so sessions survive GUI restarts/crashes, and the
/// GUI auto-launches a detached one if none is running. Set
/// `SKETCH_SESSION_SERVER=0` to force the legacy in-process direct-spawn path.
/// Returns `None` when disabled, or when the connection/launch fails (falls
/// back to direct spawning so the GUI still starts).
fn connect_session_server() -> Option<SessionServerClient> {
    if std::env::var("SKETCH_SESSION_SERVER").as_deref() == Ok("0") {
        eprintln!("[sketch-gpui] session server disabled (SKETCH_SESSION_SERVER=0); direct spawn");
        return None;
    }
    match SessionServerClient::connect() {
        Ok(client) => {
            eprintln!("[sketch-gpui] connected to session server");
            Some(client)
        }
        Err(e) => {
            eprintln!("[sketch-gpui] session server connect failed: {e}; falling back to direct spawn");
            None
        }
    }
}

/// Resolve a user-typed path argument to an absolute directory, per
/// spec-agent-cwd.md §2: expand a leading `~`, canonicalize when the
/// directory exists, fall back to process-cwd-relative resolution with
/// `.`/`..` collapsed otherwise, then validate that the result names a
/// directory. Returns the absolute path on success, or an error string
/// suitable for a footer hint on failure.
fn resolve_agent_cwd_arg(arg: &str) -> Result<PathBuf, String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Err("missing path argument".into());
    }
    // 1) Tilde expansion. `~` or `~/...` → $HOME/.... `~user/...` is not
    //    supported in v1 — sketch is single-user.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded: PathBuf = if trimmed == "~" {
        match home {
            Some(h) => h,
            None => return Err("$HOME not set, cannot expand ~".into()),
        }
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => return Err("$HOME not set, cannot expand ~".into()),
        }
    } else {
        PathBuf::from(trimmed)
    };

    // 2) Canonicalize when possible, else fall back to cwd-relative with
    //    `.`/`..` collapsed (same pattern as `Workspace::canonical_key`).
    let resolved = match std::fs::canonicalize(&expanded) {
        Ok(c) => c,
        Err(_) => {
            let abs = if expanded.is_absolute() {
                expanded
            } else {
                process_cwd().join(&expanded)
            };
            let mut out = PathBuf::new();
            for comp in abs.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            out
        }
    };

    // 3) Validate.
    if !resolved.is_dir() {
        return Err(format!("not a directory: {}", resolved.display()));
    }
    Ok(resolved)
}

/// Shorten an absolute path for display in the Status Strip
/// (spec-agent-cwd.md §6): replace a `$HOME` prefix with `~`, then if the
/// result is longer than 32 characters elide the middle so the leading and
/// trailing segments survive.
fn shorten_cwd_for_display(cwd: &std::path::Path) -> String {
    let raw = cwd.display().to_string();
    let shortened = if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home).display().to_string();
        if let Some(rest) = raw.strip_prefix(&home) {
            if rest.is_empty() {
                "~".to_string()
            } else if rest.starts_with('/') {
                format!("~{}", rest)
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        raw
    };
    if shortened.chars().count() <= 32 {
        return shortened;
    }
    // Keep leading two and trailing two segments. If we can't get that many
    // segments, fall back to leading-truncation with a `…` prefix.
    let parts: Vec<&str> = shortened.split('/').collect();
    if parts.len() >= 4 {
        let head = parts[..2].join("/");
        let tail = parts[parts.len() - 2..].join("/");
        return format!("{}/…/{}", head, tail);
    }
    // Few segments but very long names: leading-truncate.
    let chars: Vec<char> = shortened.chars().collect();
    let keep_tail = 30;
    if chars.len() > keep_tail + 1 {
        let tail: String = chars[chars.len() - keep_tail..].iter().collect();
        format!("…{}", tail)
    } else {
        shortened
    }
}

/// Path to the JSON file that maps cwd → workspace snapshot (tabs + layout
/// tree). Companion to acp_sessions.json; cleared by clearing cache_dir.
fn workspace_persist_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("sketch").join("workspace.json"))
}

/// Path to the JSON file holding app-managed runtime preferences (theme
/// choice, eventually other "View" menu state). Kept separate from the
/// user-edited `~/.config/sketch/config.kdl` so the menu-driven theme
/// switcher doesn't have to rewrite a hand-curated config file. On launch
/// preferences override the config's theme — if the user picked a theme
/// from the menu, that's what they expect next time, regardless of what
/// the kdl says.
fn preferences_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("sketch").join("preferences.json"))
}

/// Where the agent info bar sits relative to the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AgentStatusPosition {
    Top,
    #[default]
    Bottom,
}

impl AgentStatusPosition {
    fn toggle(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "top" => Self::Top,
            _ => Self::Bottom,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Preferences {
    /// Kebab-case theme identifier — `ThemeName::as_kebab()` /
    /// `ThemeName::parse()`. `None` means "no app-managed override; use
    /// the value from config.kdl (or the built-in default)."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    /// Agent info bar placement: "top" or "bottom".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_status_position: Option<String>,
    /// Document text-zoom factor (`Cmd-=`/`Cmd--`/`Cmd-0`). `None` means "no
    /// saved zoom; start at 1.0." Clamped to `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`
    /// on load so a hand-edited file can't push the body off-screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text_scale: Option<f32>,
}

fn load_preferences() -> Preferences {
    let Some(path) = preferences_path() else {
        return Preferences::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Preferences::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Best-effort write. Silently no-ops on any I/O / serialization failure —
/// preference persistence is a convenience, not a correctness boundary.
fn save_preferences(prefs: &Preferences) {
    let Some(path) = preferences_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Serializable shadow of `WindowContent` for spec-tabs-and-splits.md
/// Behavior 23. Doc/Edit persist their file path; Browser its current_dir;
/// Claude its session_id (or `None` if not yet attached). Window-local view
/// state (scroll, cursor) is intentionally NOT persisted (Constraint §4).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
enum PersistedKind {
    Doc { path: PathBuf },
    Edit { path: PathBuf },
    Browser { dir: PathBuf },
    /// JSON tag stays as "claude" so saved layouts from earlier builds load
    /// without migration; the in-memory variant is `Agent` to match the rest
    /// of the rename pass (spec-agent-window.md).
    #[serde(rename = "claude")]
    Agent { session_id: Option<String> },
}

/// One leaf in a persisted layout. Carries the (stable) window id so
/// `focused_window` references survive restore.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedLeaf {
    id: workspace::WindowId,
    #[serde(flatten)]
    kind: PersistedKind,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedLayout {
    Leaf(PersistedLeaf),
    Split {
        dir: workspace::SplitDir,
        children: Vec<(f32, PersistedLayout)>,
    },
}

/// Persisted rail kind tag (spec-rail.md §14). Outline rails persist only
/// their kind — the heading list re-derives on restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedRailKind {
    FileBrowser,
    Outline,
}

/// Persisted per-tab rail (spec-rail.md §14). Optional on `PersistedTab` so
/// snapshots written before rails existed still load (serde default → `None`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedRail {
    kind: PersistedRailKind,
    #[serde(default)]
    side: workspace::RailSide,
    /// Column width in px. Older/partial entries default to the standard width.
    #[serde(default = "default_rail_width")]
    width: f32,
    /// File-browser rail: directory it was rooted at. Absent for outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    /// Leaf the rail is pinned to. Defaults to 0 for old snapshots (will be
    /// overridden by the tab's focused_window on restore).
    #[serde(default)]
    pinned_to: workspace::WindowId,
}

fn default_rail_width() -> f32 {
    workspace::RAIL_DEFAULT_WIDTH
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedTab {
    auto_name: String,
    display_name: Option<String>,
    focused_window: workspace::WindowId,
    layout: PersistedLayout,
    /// Optional rail (spec-rail.md §14). Absent in old snapshots → no rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rail: Option<PersistedRail>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedWorkspace {
    tabs: Vec<PersistedTab>,
    active_tab: usize,
}

/// Snapshot a live `WindowContent` into its persisted shadow. Returns `None`
/// for content kinds that aren't worth persisting (e.g., an unattached
/// transient state we'd lose nothing by skipping).
fn snapshot_content(content: &WindowContent) -> PersistedKind {
    match content {
        WindowContent::Doc(d) => PersistedKind::Doc {
            path: PathBuf::from(d.file_label.as_ref()),
        },
        WindowContent::Edit(e) => PersistedKind::Edit {
            path: PathBuf::from(e.file_label.as_ref()),
        },
        WindowContent::Browser(b) => PersistedKind::Browser {
            dir: b.fb.current_dir().to_path_buf(),
        },
        WindowContent::Agent(ring) => {
            // Use the active session's id if any. Multi-session restore is
            // handled by the existing ACP persistence path; this is just
            // enough to know "this slot had a Claude session" so on restore
            // we can spawn the ring shell.
            let session_id = ring
                .slots
                .first()
                .and_then(|s| s.state.channel.as_ref())
                .and_then(|c| c.session_id().map(|s| s.to_string()));
            PersistedKind::Agent { session_id }
        }
    }
}

/// Snapshot a live `Layout<WindowContent>` into its persisted shadow.
fn snapshot_layout(layout: &workspace::Layout<WindowContent>) -> PersistedLayout {
    match layout {
        workspace::Layout::Empty => PersistedLayout::Leaf(PersistedLeaf {
            id: 0,
            kind: PersistedKind::Browser {
                dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            },
        }),
        workspace::Layout::Leaf(win) => PersistedLayout::Leaf(PersistedLeaf {
            id: win.id,
            kind: snapshot_content(&win.content),
        }),
        workspace::Layout::Split { dir, children } => PersistedLayout::Split {
            dir: *dir,
            children: children
                .iter()
                .map(|(w, c)| (*w, snapshot_layout(c)))
                .collect(),
        },
    }
}

/// Snapshot a live rail into its persisted shadow (spec-rail.md §14).
fn snapshot_rail(rail: &workspace::RailState) -> PersistedRail {
    match &rail.content {
        workspace::RailContent::FileBrowser(fb) => PersistedRail {
            kind: PersistedRailKind::FileBrowser,
            side: rail.side,
            width: rail.width_px,
            cwd: Some(fb.current_dir().to_path_buf()),
            pinned_to: rail.pinned_to,
        },
        workspace::RailContent::Outline(_) => PersistedRail {
            kind: PersistedRailKind::Outline,
            side: rail.side,
            width: rail.width_px,
            cwd: None,
            pinned_to: rail.pinned_to,
        },
    }
}

/// Reconstruct a live rail from its persisted shadow (spec-rail.md §14). The
/// restored rail is unfocused (focus stays on the content leaf on restore).
/// `fallback_pin` is used when the snapshot predates the `pinned_to` field.
fn restore_rail(p: PersistedRail, fallback_pin: workspace::WindowId) -> workspace::RailState {
    let content = match p.kind {
        PersistedRailKind::FileBrowser => {
            let dir = match p.cwd {
                Some(d) if d.is_dir() => d,
                _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            };
            workspace::RailContent::FileBrowser(FileBrowser::new(dir))
        }
        PersistedRailKind::Outline => {
            workspace::RailContent::Outline(workspace::OutlineState::new())
        }
    };
    let pinned_to = if p.pinned_to != 0 { p.pinned_to } else { fallback_pin };
    workspace::RailState {
        content,
        side: p.side,
        width_px: p.width,
        focused: false,
        pinned_to,
    }
}

/// Reconstruct a persisted layout into live `WindowContent`, opening any
/// file-backed leaves through `ws`'s buffer pool so two restored views of the
/// same file share one core. Returns the live layout plus the max window id
/// seen (so the caller can advance the id allocator past restored ids).
/// Returns (layout, max_window_id, agent_leaf_ids).
fn restore_layout(
    ws: &mut workspace::Workspace<WindowContent>,
    theme: &Theme,
    layout: PersistedLayout,
) -> (workspace::Layout<WindowContent>, workspace::WindowId, Vec<workspace::WindowId>) {
    match layout {
        PersistedLayout::Leaf(leaf) => {
            let id = leaf.id;
            let is_agent = matches!(&leaf.kind, PersistedKind::Agent { .. });
            let content = restore_content(ws, theme, leaf.kind);
            let agents = if is_agent { vec![id] } else { vec![] };
            (
                workspace::Layout::Leaf(workspace::Window { id, content }),
                id,
                agents,
            )
        }
        PersistedLayout::Split { dir, children } => {
            let mut max_id: workspace::WindowId = 0;
            let mut agents = Vec::new();
            let mut restored_children = Vec::with_capacity(children.len());
            for (w, child) in children {
                let (sub, sub_max, sub_agents) = restore_layout(ws, theme, child);
                if sub_max > max_id {
                    max_id = sub_max;
                }
                agents.extend(sub_agents);
                restored_children.push((w, sub));
            }
            (
                workspace::Layout::Split {
                    dir,
                    children: restored_children,
                },
                max_id,
                agents,
            )
        }
    }
}

fn restore_content(
    ws: &mut workspace::Workspace<WindowContent>,
    theme: &Theme,
    kind: PersistedKind,
) -> WindowContent {
    match kind {
        PersistedKind::Doc { path } => {
            let label: SharedString = path.display().to_string().into();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let doc = Document::from_text(text, path.clone());
            let blocks = render_with_wiki(&doc.full_text(), theme, Some(&path));
            WindowContent::Doc(DocState {
                blocks,
                file_label: label,
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                edit_cache: None,
            })
        }
        PersistedKind::Edit { path } => {
            let label: SharedString = path.display().to_string().into();
            // Restore through the pool — a second restored Edit view of the
            // same file binds to the same shared core.
            match ws.open_and_retain(&path) {
                Ok((id, core)) => WindowContent::Edit(EditState::new(
                    SharedEditor::new(id, core),
                    label,
                    EditView::Code,
                )),
                Err(_) => WindowContent::Browser(BrowserWindow::standalone(
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                )),
            }
        }
        PersistedKind::Browser { dir } => {
            let dir = if dir.is_dir() {
                dir
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            };
            WindowContent::Browser(BrowserWindow::standalone(dir))
        }
        PersistedKind::Agent { .. } => {
            // Claude restore is its own subsystem (acp_sessions.json +
            // open_agent_inner). Replace with a Browser stub here so the
            // tab survives; user can re-attach via the existing Claude
            // commands.
            WindowContent::Browser(BrowserWindow::standalone(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ))
        }
    }
}

/// Snapshot a live workspace into a fully serializable shape.
fn snapshot_workspace(ws: &workspace::Workspace<WindowContent>) -> PersistedWorkspace {
    PersistedWorkspace {
        tabs: ws
            .tabs
            .iter()
            .map(|t| PersistedTab {
                auto_name: t.auto_name.clone(),
                display_name: t.display_name.clone(),
                focused_window: t.focused,
                layout: snapshot_layout(&t.layout),
                rail: t.rail.as_ref().map(snapshot_rail),
            })
            .collect(),
        active_tab: ws.active_tab,
    }
}

/// Best-effort write of the workspace snapshot for `cwd`. Silently no-ops
/// on any I/O / serialization failure (Behavior 23: best-effort + silent).
fn save_persisted_workspace(cwd: &std::path::Path, ws: &workspace::Workspace<WindowContent>) {
    let Some(path) = workspace_persist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Read-modify-write so other cwds in the file aren't clobbered (Constraint
    // §11 / multi-session §15: last-writer-wins).
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let snap = snapshot_workspace(ws);
    if let Ok(v) = serde_json::to_value(&snap) {
        // Drop any entry saved under the old raw spelling so the file doesn't
        // accumulate a canonical + raw duplicate for the same dir (ADR-0010:
        // the next save rewrites canonical — this is that rewrite).
        map.remove(&cwd.display().to_string());
        map.insert(persist_cwd_key(cwd), v);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&map) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Read the persisted workspace for `cwd`. Returns `None` if no file, no
/// entry, or unparseable — the caller treats these as "no saved state,
/// bootstrap fresh" (Behavior 24).
fn load_persisted_workspace(cwd: &std::path::Path) -> Option<PersistedWorkspace> {
    let path = workspace_persist_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&bytes).ok()?;
    // Canonical key first; lazy fallback to the old raw spelling for entries
    // saved before D5 (ADR-0010 — adopt on read, next save rewrites canonical).
    let entry = map
        .get(&persist_cwd_key(cwd))
        .or_else(|| map.get(&cwd.display().to_string()))?;
    serde_json::from_value(entry.clone()).ok()
}

/// One restored session slot. Order in the returned `Vec` matches the
/// saved ring order; reboot rebuilds the ring in this same order.
/// `mode`, `tasklist_open`, and `subagents_open` are spec §35 additions;
/// older files (without these keys) deserialize with defaults
/// (Chatbox, false, false). Older sketch binaries reading newer files
/// silently drop the unknown keys (downgrade contract, §35).
/// `cwd` is a spec-agent-cwd.md §5 addition; `None` (absence in JSON)
/// resolves to the process cwd at restore time per §1.
#[derive(Debug, Clone)]
struct PersistedSlot {
    id: String,
    label: String,
    active: bool,
    mode: InputModeKind,
    tasklist_open: bool,
    subagents_open: bool,
    cwd: Option<PathBuf>,
}

/// Load the persisted slot list for `cwd`. Returns an empty vec if no
/// file, no entry, or unparseable input — all of which the caller treats
/// as "no saved state, open a fresh claude-1". Migrates the legacy
/// `{cwd: <string-id>}` shape on the fly to a one-element list labelled
/// `"claude-1"`.
fn load_persisted_acp_sessions(cwd: &std::path::Path) -> Vec<PersistedSlot> {
    let Some(path) = acp_session_persist_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    // Canonical key first; lazy fallback to the old raw spelling (ADR-0010).
    let raw = cwd.to_string_lossy();
    let Some(entry) = json.get(&persist_cwd_key(cwd)).or_else(|| json.get(raw.as_ref())) else {
        return Vec::new();
    };
    // Legacy single-string shape: synthesize a one-slot list with the
    // spec-§35 defaults for the missing fields.
    if let Some(id) = entry.as_str() {
        return vec![PersistedSlot {
            id: id.to_string(),
            label: "claude-1".into(),
            active: true,
            mode: InputModeKind::Chatbox,
            tasklist_open: false,
            subagents_open: false,
            cwd: None,
        }];
    }
    let Some(arr) = entry.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let id = obj.get("id")?.as_str()?.to_string();
            let label = obj
                .get("label")
                .and_then(|s| s.as_str())
                .unwrap_or("claude")
                .to_string();
            let active = obj
                .get("active")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            // Spec §35 additions. Missing keys default per the same
            // table (chatbox, false, false). Unknown mode strings fall
            // back to Chatbox.
            let mode = obj
                .get("mode")
                .and_then(|m| m.as_str())
                .map(|s| match s {
                    "worksheet" => InputModeKind::Worksheet,
                    _ => InputModeKind::Chatbox,
                })
                .unwrap_or(InputModeKind::Chatbox);
            let tasklist_open = obj
                .get("tasklist_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let subagents_open = obj
                .get("subagents_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            // spec-agent-cwd.md §5: optional per-slot cwd. Absent (old
            // file, or pre-spec save) is loaded as None so the restore
            // path can fall back to process cwd per §1.
            let cwd = obj
                .get("cwd")
                .and_then(|c| c.as_str())
                .map(PathBuf::from);
            Some(PersistedSlot {
                id,
                label,
                active,
                mode,
                tasklist_open,
                subagents_open,
                cwd,
            })
        })
        .collect()
}

/// Forget the saved ACP session list for `cwd`. Used by `claude-clear` so
/// the next attach hits `session/new` instead of resuming the cleared
/// sessions.
fn forget_persisted_acp_sessions(cwd: &std::path::Path) {
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if let Some(obj) = json.as_object_mut() {
        // Clear both spellings so a pre-D5 raw entry can't linger (ADR-0010).
        obj.remove(persist_cwd_key(cwd).as_str());
        obj.remove(cwd.to_string_lossy().as_ref());
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Persist the ring's slots for `cwd` so the next sketch run can resume
/// every session in the ring, not just the active one. Best-effort writes
/// — failures (no cache dir, permissions, malformed prior file) silently
/// bail. Per-slot id resolution honors the resume_id stability rule: if a
/// slot was restored with a `resume_id`, that id is what gets persisted
/// (even if `session/load` failed and the slot fell back to a fresh
/// `session/new`). Slots without an id (pending attach or attach failed
/// outright) are skipped.
///
/// Concurrent sketch instances on the same `cwd`: last-writer-wins. Each
/// call does a read-modify-write of the file, replacing only the cwd
/// entry; other cwds are preserved.
fn save_persisted_acp_sessions(cwd: &std::path::Path, ring: &AgentRing) {
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let active_index = ring.active;
    let entries: Vec<serde_json::Value> = ring
        .slots
        .iter()
        .enumerate()
        .filter_map(|(i, slot)| {
            // resume_id wins over channel id: if we were trying to resume,
            // keep retrying the original id even when load fell back.
            let id = slot.resume_id.clone().or_else(|| {
                slot.state.channel.as_ref().and_then(|c| c.session_id())
            })?;
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), serde_json::Value::String(id));
            obj.insert("label".into(), serde_json::Value::String(slot.label.clone()));
            if i == active_index {
                obj.insert("active".into(), serde_json::Value::Bool(true));
            }
            // Spec §35: persist input mode and sidepane state per slot.
            // Older sketch binaries reading this file ignore the unknown
            // keys (serde's standard behavior); no migration needed.
            let mode_str = match slot.state.input_surface.mode() {
                InputModeKind::Worksheet => "worksheet",
                InputModeKind::Chatbox => "chatbox",
            };
            obj.insert(
                "mode".into(),
                serde_json::Value::String(mode_str.to_string()),
            );
            obj.insert(
                "tasklist_open".into(),
                serde_json::Value::Bool(slot.state.tasklist_open),
            );
            obj.insert(
                "subagents_open".into(),
                serde_json::Value::Bool(slot.state.subagents_open),
            );
            // spec-agent-cwd.md §5: persist the slot's working directory.
            // Lossy on non-UTF8 paths (Constraint §11) — same as the
            // top-level `cwd` key in this file. Acceptable on macOS where
            // APFS enforces UTF8-encodable names.
            obj.insert(
                "cwd".into(),
                serde_json::Value::String(slot.cwd.display().to_string()),
            );
            Some(serde_json::Value::Object(obj))
        })
        .collect();

    let mut json = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !json.is_object() {
        json = serde_json::json!({});
    }
    if let Some(obj) = json.as_object_mut() {
        if entries.is_empty() {
            // Don't leave a stale list behind if nothing is persistable
            // (e.g., user closed all sessions but reboot hasn't fired yet).
            obj.remove(persist_cwd_key(cwd).as_str());
            obj.remove(cwd.to_string_lossy().as_ref());
        } else {
            // Clear the old raw spelling, then write under the canonical key
            // (ADR-0010: next save rewrites canonical).
            obj.remove(cwd.to_string_lossy().as_ref());
            obj.insert(
                persist_cwd_key(cwd),
                serde_json::Value::Array(entries),
            );
        }
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Test-only counter: incremented every time `render_agent` rebuilds the
/// memoized view-model (flat_items + gutter). A fingerprint hit must leave
/// this unchanged. Asserted by `view_model_memoization_fast_skip`.
#[cfg(test)]
thread_local! {
    static VIEW_MODEL_REBUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Domain newtype for a tool-call identity (Finding 7, parse-don't-validate).
/// The protocol hands us a typed [`ToolCallId`](sketch::acp_channel::ToolCallId)
/// (`Arc<str>` under the hood); we parse it into this key ONCE at the boundary
/// (`apply_reply_events`) and key every tool map on it — `tool_calls`,
/// `tool_call_order`, `tool_call_anchor_line`, and `FlatItem::ToolGroup.ids`.
///
/// Deliberately NO `Deref` to `String`/`str`: a `ToolCallKey` is not
/// interchangeable with a session id, a label, or an arbitrary string, so a
/// mismatched key is a compile error rather than a silently-missed HashMap
/// lookup. Stringification happens only at the render edge (`as_str` /
/// `to_string`) where a DOM id or display label is needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolCallKey(sketch::acp_channel::ToolCallId);

impl ToolCallKey {
    /// Parse a protocol `ToolCallId` into the domain key. Cheap: clones an
    /// `Arc<str>`, no string allocation.
    fn from_id(id: &sketch::acp_channel::ToolCallId) -> Self {
        ToolCallKey(id.clone())
    }

    /// Borrow the underlying id as a `&str` (render edge only).
    fn as_str(&self) -> &str {
        &self.0 .0
    }
}

impl std::fmt::Display for ToolCallKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row in the virtualised claude transcript list. The render
/// closure handed to `gpui::list` indexes into a `Vec<FlatItem>`
/// snapshot that mirrors the old "line N then any tool blocks
/// anchored at line N" emission order.
#[derive(Debug, Clone)]
enum FlatItem {
    /// Doc line at this index in the editor's document.
    Line(usize),
    /// A group of tool calls sharing the same anchor line. Rendered
    /// as a single "Ran N tool calls" header (collapsed) or header +
    /// individual rows (expanded). The anchor_line is the key for
    /// expand/collapse state.
    ToolGroup {
        anchor_line: usize,
        ids: Vec<ToolCallKey>,
    },
    /// A structurally-rendered block (table or fenced code block) that
    /// replaces a range of frozen lines with proper layout.
    Block(RenderedBlock),
    /// Visual divider at a turn boundary: role label + faint rule.
    TurnHeader {
        role: TurnRole,
    },
    /// Pulsing indicator shown at transcript tail while awaiting reply.
    ThinkingIndicator,
}

/// Role shown in a `TurnHeader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnRole {
    Claude,
    User,
}

/// Free-function variant of `SketchGpuiView::build_tool_block` that
/// works without an active `&Context<Self>`. Used inside `gpui::list`'s
/// per-item render closure (which only gets `&mut Window, &mut App`).
/// Click handlers are wired through a `WeakEntity<SketchGpuiView>`
/// captured at render-build time so toggling `expanded_tool_calls`
/// still goes through the same entity update path.
fn build_tool_block_with_weak(
    tc: &sketch::acp_channel::ToolCall,
    expanded: bool,
    code_font: &SharedString,
    weak_view: gpui::WeakEntity<SketchGpuiView>,
    at: &sketch::theme::AgentTheme,
) -> AnyElement {
    use sketch::acp_channel::ToolCallStatus;
    let (status_glyph, status_color): (&str, Hsla) = match tc.status {
        ToolCallStatus::Pending => ("○", nc(at.tool_pending)),
        ToolCallStatus::InProgress => ("◐", nc(at.tool_in_progress)),
        ToolCallStatus::Completed => ("●", nc(at.tool_completed)),
        ToolCallStatus::Failed => ("✗", nc(at.tool_failed)),
        _ => ("·", nc(at.tool_pending)),
    };
    let dim_color = nc(at.dim);
    let policy = tool_render_policy(tc);
    let title = if tc.title.is_empty() {
        "(tool)".to_string()
    } else {
        tc.title.clone()
    };
    let id_str = tc.tool_call_id.0.to_string();
    let has_body = !matches!(policy, ToolRenderPolicy::HeaderOnly);
    let arrow = if has_body {
        if expanded { "▼" } else { "▶" }
    } else {
        " "
    };

    let mut summary_row = div()
        .id(SharedString::from(format!("tool-summary-{}", id_str)))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .py(px(5.0))
        .px_2()
        .child(div().text_color(dim_color).child(arrow))
        .child(div().text_color(status_color).child(status_glyph))
        .child(
            div()
                .text_color(nc(at.tool_body_fg))
                .text_size(px(12.0))
                .child(format!("[{:?}]", tc.kind).to_lowercase()),
        )
        .child(div().flex_1().text_color(nc(at.frozen_fg)).child(title));

    if has_body {
        let id_for_click = id_str.clone();
        summary_row = summary_row.cursor_pointer().on_click(
            move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                let id = id_for_click.clone();
                let _ = weak_view.update(app, |this, cx| {
                    if let Some(c) = this.agent_mut() {
                        if c.expanded_tool_calls.contains(&id) {
                            c.expanded_tool_calls.remove(&id);
                        } else {
                            c.expanded_tool_calls.insert(id);
                        }
                    }
                    cx.notify();
                });
            },
        );
    }

    let mut block = div()
        .flex()
        .flex_col()
        .my_1()
        .pl_2()
        .ml_2()
        .border_l_2()
        .border_color(nc(at.tool_card_border))
        .child(summary_row);

    if expanded && has_body {
        let max_lines = match policy {
            ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
            _ => None,
        };
        let body_bg = nc(at.tool_body_bg);
        let output_bg = nc(at.tool_output_bg);
        let body_fg = nc(at.tool_body_fg);
        let diff_add = nc(at.diff_add);
        let diff_remove = nc(at.diff_remove);
        let diff_header = nc(at.diff_header);
        if let Some(input) = &tc.raw_input {
            let pretty =
                serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
            block = block.child(tool_body_pane_free(
                "input",
                &pretty,
                None,
                body_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
        let content_text = render_tool_content_blocks(&tc.content);
        if !content_text.trim().is_empty() {
            block = block.child(tool_body_pane_free(
                "content",
                &content_text,
                max_lines,
                body_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
        if let Some(output) = &tc.raw_output {
            let pretty =
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
            block = block.child(tool_body_pane_free(
                "output",
                &pretty,
                max_lines,
                output_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
    }

    block.into_any_element()
}

/// Free-function form of [`SketchGpuiView::tool_body_pane`] for the
/// virtualised render path. Same content layout, accepts a borrowed
/// `code_font` instead of reaching through `&self`.
fn tool_body_pane_free(
    label: &str,
    body: &str,
    max_lines: Option<usize>,
    bg: Hsla,
    fg: Hsla,
    code_font: &SharedString,
    diff_add: Hsla,
    diff_remove: Hsla,
    diff_header: Hsla,
) -> gpui::Div {
    let display = match max_lines {
        Some(n) => truncate_lines(body, n),
        None => body.to_string(),
    };

    // Build diff-highlighted lines: color +/- lines and diff headers.
    let mut container = div()
        .mt_1()
        .mx_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(bg)
        .text_size(px(11.0))
        .text_color(fg)
        .font_family(code_font.clone());

    // Label
    container = container.child(
        div()
            .text_size(px(10.0))
            .pb(px(2.0))
            .child(SharedString::from(format!("{}:", label))),
    );

    // Diff-highlighted body lines.
    for line in display.lines() {
        let color = if line.starts_with("+ ") || line.starts_with("+\t") || line == "+" {
            diff_add
        } else if line.starts_with("- ") || line.starts_with("-\t") || line == "-" {
            diff_remove
        } else if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ") {
            diff_header
        } else {
            fg
        };
        container = container.child(
            div()
                .text_color(color)
                .child(SharedString::from(line.to_string())),
        );
    }

    container
}

/// How much of a tool call's body to render when expanded. Mirrors the
/// per-tool policy baked into Claude Code's TUI (see cli.js's
/// `renderToolResultMessage` table) — Read/Search show no body, Bash
/// shows the first 3 lines, edits show their full diff, etc.
#[derive(Debug, Clone, Copy)]
enum ToolRenderPolicy {
    /// No body even when expanded — the user only needs to know the
    /// action happened. Read, Grep/Glob, TodoWrite, mode switches.
    HeaderOnly,
    /// Show at most this many lines per body pane; cap further with a
    /// "+N lines hidden" footer.
    Truncated { max_lines: usize },
    /// Show everything. For diffs, MCP tool returns, Task subagents.
    Full,
}

/// Pick the render policy for a tool call. We classify on `kind`
/// (mapped from claude-code-acp's `tools.js`) plus a couple of
/// raw_input sniffs for tools the kind alone doesn't disambiguate
/// (TodoWrite is `Think`, same as Task — but the user wants its body
/// hidden, so we detect it by an `input.todos` field).
fn tool_render_policy(tc: &sketch::acp_channel::ToolCall) -> ToolRenderPolicy {
    use sketch::acp_channel::ToolKind;
    // TodoWrite shows up as `kind=Think` (same as the Task subagent),
    // and its body is the running todo list — too noisy to render. Sniff
    // for the distinctive `todos` array on the input to tell them apart.
    let is_todowrite = tc
        .raw_input
        .as_ref()
        .and_then(|v| v.get("todos"))
        .is_some();
    if is_todowrite {
        return ToolRenderPolicy::HeaderOnly;
    }
    match tc.kind {
        ToolKind::Read | ToolKind::Search | ToolKind::SwitchMode => {
            ToolRenderPolicy::HeaderOnly
        }
        ToolKind::Execute => ToolRenderPolicy::Truncated { max_lines: 3 },
        ToolKind::Fetch => ToolRenderPolicy::Truncated { max_lines: 10 },
        ToolKind::Edit
        | ToolKind::Move
        | ToolKind::Delete
        | ToolKind::Think
        | ToolKind::Other
        | _ => ToolRenderPolicy::Full,
    }
}

/// Extract a short detail string from a tool call's input for inline
/// display in the group header. Returns the file path for Read/Edit/Write,
/// truncated command for Execute, query for Search, etc.
fn tool_inline_detail(tc: &sketch::acp_channel::ToolCall) -> Option<String> {
    let input = tc.raw_input.as_ref()?;
    // Try file_path first (Read, Edit, Write, Glob).
    if let Some(fp) = input.get("file_path").and_then(|v| v.as_str()) {
        // Show just the filename or last path component to keep it short.
        let short = std::path::Path::new(fp)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(fp);
        return Some(short.to_string());
    }
    // Execute/Bash: show truncated command.
    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        let first_line = cmd.lines().next().unwrap_or(cmd);
        let truncated = if first_line.len() > 60 {
            format!("{}…", &first_line[..60])
        } else {
            first_line.to_string()
        };
        return Some(truncated);
    }
    // Search (Grep/Glob): show pattern.
    if let Some(pat) = input.get("pattern").and_then(|v| v.as_str()) {
        let truncated = if pat.len() > 40 {
            format!("{}…", &pat[..40])
        } else {
            pat.to_string()
        };
        return Some(truncated);
    }
    None
}

/// Short type label for a tool call used in collapsed group headers
/// (e.g. "grep", "edit", "read"). Prefers the leading word of the title — for
/// claude-code-acp this is the tool name (Grep / Read / Bash / Edit / …) — and
/// falls back to the ACP `kind` when the title isn't a clean single token.
fn tool_type_label(tc: &sketch::acp_channel::ToolCall) -> String {
    tc.title
        .split_whitespace()
        .next()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() <= 12 && w.chars().all(|c| c.is_alphanumeric()))
        .unwrap_or_else(|| tool_kind_label(&tc.kind))
}

/// Fallback label derived from the ACP tool kind when the title isn't usable.
fn tool_kind_label(kind: &sketch::acp_channel::ToolKind) -> String {
    use sketch::acp_channel::ToolKind;
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Search => "search",
        ToolKind::Execute => "run",
        ToolKind::Move => "move",
        ToolKind::Delete => "delete",
        ToolKind::Fetch => "fetch",
        ToolKind::Think => "think",
        ToolKind::SwitchMode => "mode",
        ToolKind::Other => "tool",
        _ => "tool",
    }
    .to_string()
}

/// Append a tool call's body panes directly to a container div.
/// Used for single-tool groups where we skip the nested sub-header.
fn append_tool_body(
    mut block: gpui::Div,
    tc: &sketch::acp_channel::ToolCall,
    policy: ToolRenderPolicy,
    code_font: &SharedString,
    at: &sketch::theme::AgentTheme,
) -> gpui::Div {
    let max_lines = match policy {
        ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
        _ => None,
    };
    let body_bg = nc(at.tool_body_bg);
    let output_bg = nc(at.tool_output_bg);
    let body_fg = nc(at.tool_body_fg);
    let diff_add = nc(at.diff_add);
    let diff_remove = nc(at.diff_remove);
    let diff_header = nc(at.diff_header);
    if let Some(input) = &tc.raw_input {
        let pretty =
            serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        block = block.child(tool_body_pane_free(
            "input",
            &pretty,
            None,
            body_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    let content_text = render_tool_content_blocks(&tc.content);
    if !content_text.trim().is_empty() {
        block = block.child(tool_body_pane_free(
            "content",
            &content_text,
            max_lines,
            body_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    if let Some(output) = &tc.raw_output {
        let pretty =
            serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
        block = block.child(tool_body_pane_free(
            "output",
            &pretty,
            max_lines,
            output_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    block
}

/// Flatten a tool call's `Vec<ToolCallContent>` into a single human-
/// readable string. Splits diffs into a labelled `--- path` header plus
/// old/new bodies; treats terminal embeds as a one-line placeholder.
/// Centralised so policy tweaks (e.g., suppressing the old half of a
/// diff) only need to be made in one spot.
fn render_tool_content_blocks(content: &[sketch::acp_channel::ToolCallContent]) -> String {
    use sketch::acp_channel::ToolCallContent;
    let mut buf = String::new();
    for c in content {
        match c {
            ToolCallContent::Content(content) => {
                if let agent_client_protocol::schema::ContentBlock::Text(t) = &content.content {
                    buf.push_str(&t.text);
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                }
            }
            ToolCallContent::Diff(d) => {
                buf.push_str(&format!("--- {}\n", d.path.display()));
                if let Some(old) = &d.old_text {
                    buf.push_str("- (old)\n");
                    buf.push_str(old);
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                }
                buf.push_str("+ (new)\n");
                buf.push_str(&d.new_text);
                if !buf.ends_with('\n') {
                    buf.push('\n');
                }
            }
            ToolCallContent::Terminal(_) => {
                buf.push_str("[terminal embed — not rendered]\n");
            }
            // ToolCallContent is `#[non_exhaustive]`; future variants
            // render as a placeholder rather than failing to build.
            _ => {
                buf.push_str("[unsupported content variant]\n");
            }
        }
    }
    buf
}

/// Trim `body` to its first `max_lines` lines. If anything was dropped,
/// append a dim "+N lines hidden" footer so the user knows there's more
/// to see — they can re-expand on a wider window or pop the original
/// payload off-screen by collapsing the block.
fn truncate_lines(body: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let mut lines = body.lines();
    let mut head: Vec<&str> = Vec::with_capacity(max_lines);
    for _ in 0..max_lines {
        match lines.next() {
            Some(l) => head.push(l),
            None => break,
        }
    }
    let remaining = lines.count();
    let mut out = head.join("\n");
    if remaining > 0 {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("… +{} lines hidden", remaining));
    }
    out
}

/// Pick the buffer line a freshly-announced tool call should anchor to,
/// and force a section break so subsequent text chunks don't extend the
/// pre-tool line.
///
/// Why the section break matters: text chunks splice verbatim — there's
/// no per-chunk newline in the buffer. If Claude says "Let me check
/// this" → tool fires → "now I see X", all three pieces would land on
/// the same doc line and the tool block (rendered after that line)
/// would visually appear AFTER both halves of text instead of between
/// them. Inserting a `\n` here forces the post-tool text onto a new
/// line below the tool block, restoring chronological order.
///
/// Anchor lands on the last line containing actual text (i.e., the line
/// terminated by the trailing `\n`), so the tool block renders just
/// after the pre-tool content and just before the empty line where the
/// next chunk will splice in.
///
/// Returns a [`LineAnchor`] that survives inserts and deletes elsewhere in
/// the document, per spec-agent-window.md §E1. The renderer resolves it to
/// a line index via `editor.line_for_anchor(a)`; a `None` (line consumed)
/// falls back to EOF rendering.

fn anchor_for_new_tool_call(editor: &mut Editor) -> LineAnchor {
    // Perf (finding 5): O(1) tail probe instead of cloning the whole transcript
    // (`full_text`) just to test emptiness + trailing newline per tool call.
    if !editor.document().is_empty() && editor.document().last_char() != Some('\n') {
        let len = editor.document().rope().len_chars();
        editor.programmatic_insert(len, "\n");
    }
    // Append a dedicated blank line for the tool block to anchor on, rather
    // than reusing the trailing LLM content line. Tagging the anchor line
    // `Tool(k)` (for the gutter) would otherwise steal that line's `Llm(k)`
    // tag, and `find_llm_insertion_point` keys off the last `Llm`-tagged line
    // to place the turn's next chunk — stealing it makes post-tool prose
    // splice into an earlier line (the "ThereLet" / "Found key line" clobber).
    let len = editor.document().rope().len_chars();
    editor.programmatic_insert(len, "\n");
    let line_count = editor.document().line_count();
    // The dedicated blank line we just created is line_count - 2
    // (line_count - 1 is the empty trailing line). saturating_sub guards an
    // empty doc, where the tool block just anchors at the top.
    let line = line_count.saturating_sub(2);
    editor.anchor_for_line(line)
}

/// Map a doc-line index to the flat-child index inside the claude body
/// container, accounting for tool blocks rendered between text lines.
///
/// `render_agent` emits children in this order:
///
/// ```text
/// line 0
/// (any tool blocks anchored at line 0)
/// line 1
/// (any tool blocks anchored at line 1)
/// ...
/// line N
/// ```
///
/// Maps a document line to its position in the flat_items list.
///
/// Accounts for tool groups (each adds one item) and block ranges
/// (each collapses N lines into one FlatItem::Block, removing N-1
/// items).
fn cursor_visible_child_index(
    c: &AgentState,
    doc_line: usize,
    block_ranges: &[(usize, usize)],
    turn_headers_before: usize,
) -> usize {
    // Count distinct anchor lines before doc_line (each = one ToolGroup item).
    // `LineAnchor` is opaque; resolve to a current line index via the editor.
    // Anchors whose line was consumed by a delete (`None`) are treated as
    // EOF — they sort after `doc_line` and don't count.
    let eof_line = c.editor.document().line_count();
    let mut anchors_before: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for id in &c.tool_call_order {
        if let Some(&anchor) = c.tool_call_anchor_line.get(id) {
            let line = c.editor.line_for_anchor(anchor).unwrap_or(eof_line);
            if line < doc_line {
                anchors_before.insert(line);
            }
        }
    }
    // Each block range (start..end) before doc_line replaces `end-start`
    // Line items with 1 Block item, saving `end-start-1` slots.
    let mut lines_collapsed: usize = 0;
    for &(s, e) in block_ranges {
        if s < doc_line {
            lines_collapsed += (e - s) - 1; // N lines → 1 block = N-1 fewer items
        }
    }
    doc_line - lines_collapsed + anchors_before.len() + turn_headers_before
}

/// Count `TurnHeader` items that would be inserted before `before_line`
/// during the flat-items build. Must match the insertion logic in
/// `render_agent`'s flat-items loop.
fn count_turn_headers_before(tags: &[Option<TurnId>], before_line: usize) -> usize {
    let mut count = 0usize;
    let mut prev: Option<TurnId> = None;
    for i in 0..before_line.min(tags.len()) {
        if let Some(tid) = tags[i] {
            let dominated_by = match tid {
                // Mirror the flat-items loop: Tool and System are non-dominant
                // (no header, no turn-run break) — Finding 5, INV-3.
                TurnId::Tool(_) | TurnId::System => None,
                other => Some(other),
            };
            if let Some(dt) = dominated_by {
                let changed = match prev {
                    Some(p) => p != dt,
                    None => true,
                };
                if changed {
                    count += 1;
                    prev = Some(dt);
                }
            }
        } else if prev.is_some() {
            count += 1;
            prev = None;
        }
    }
    count
}

/// Detect line ranges in `lines` that should be rendered as structured
/// blocks (tables and fenced code blocks) rather than line-by-line.
/// Only considers frozen (agent-written) lines.
///
/// Returns `Vec<(start, end)>` where `start..end` covers the full block
/// including delimiters. Ranges are non-overlapping and sorted.
/// Pure follow-tail policy (F4, INV-13), factored out of `AgentState` so it
/// can be unit-tested without a GPUI editor/list. In Chatbox mode the user's
/// cursor is outside the transcript, so following is purely the sticky-bottom
/// `follow_output` flag; in Worksheet mode the viewport tracks the cursor and
/// follows only when the cursor is at EOF.
fn should_follow_tail(input_mode: InputModeKind, follow_output: bool, cursor_at_eof: bool) -> bool {
    match input_mode {
        InputModeKind::Chatbox => follow_output,
        InputModeKind::Worksheet => cursor_at_eof,
    }
}

fn detect_block_ranges(
    lines: &[String],
    frozen_ranges: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let is_frozen = |i: usize| -> bool {
        frozen_ranges.iter().any(|&(s, e)| i >= s && i < e)
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_frozen(i) {
            i += 1;
            continue;
        }
        let trimmed = lines[i].trim();

        // Fenced code block: starts with ``` (optionally with language)
        if trimmed.starts_with("```") {
            let start = i;
            i += 1;
            // Find closing fence. Track whether we actually matched one —
            // exhausting the buffer is NOT a close (INV-11). A streaming,
            // still-open fence must render its arrived lines as plain Lines
            // until the closing delimiter freezes, so each new line stays
            // its own FlatItem (keeping the count-keyed scroll path live)
            // and we avoid an O(block) re-parse-to-EOF every chunk (F12).
            let mut closed = false;
            while i < lines.len() {
                if lines[i].trim().starts_with("```") && lines[i].trim().len() <= trimmed.len() + 20 {
                    i += 1; // include the closing fence
                    closed = true;
                    break;
                }
                i += 1;
            }
            // Only emit a block range once the closing fence is present
            // (symmetric to the >=3-row table rule below). Without a match
            // the loop ran to EOF, so leave these lines unblocked.
            if closed && i > start + 1 {
                ranges.push((start, i));
            }
            continue;
        }

        // Table: consecutive lines starting with `|` (need at least 2 rows
        // for a header + separator).
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let start = i;
            while i < lines.len() && is_frozen(i) {
                let t = lines[i].trim();
                if t.starts_with('|') && t.ends_with('|') {
                    i += 1;
                } else {
                    break;
                }
            }
            if i - start >= 3 {
                // 3+ rows: header, separator, at least one data row
                ranges.push((start, i));
            }
            continue;
        }

        i += 1;
    }
    ranges
}

/// Parse a contiguous line range into a single RenderedBlock (table or code
/// block). Returns `None` if the parser doesn't produce a usable block.
/// Outcome of trying to render a detected range as a single structured block.
/// Total over the partition (Finding 10, INV-10): a detected range either
/// becomes one `Parsed` block or explicitly `FallBackToLines`, so the flat
/// build emits either the Block or the constituent Lines for every range —
/// "detected but not emitted" is unrepresentable rather than an `Option::None`
/// a later reader might forget to expand back into lines.
enum BlockParse {
    Parsed(RenderedBlock),
    FallBackToLines,
}

fn parse_block_range(
    lines: &[String],
    start: usize,
    end: usize,
    theme: &Theme,
) -> BlockParse {
    let slice: String = lines[start..end].join("\n");
    let blocks = render_with_wiki(&slice, theme, None);
    // Take the first Table or CodeBlock produced.
    for b in blocks {
        match &b {
            RenderedBlock::Table { .. } | RenderedBlock::CodeBlock { .. } => {
                return BlockParse::Parsed(b)
            }
            _ => {}
        }
    }
    BlockParse::FallBackToLines
}

/// Storage-side ceiling for any single text payload we keep on a tool
/// call (one diff body, one content text block, one raw_input/raw_output
/// JSON blob). Tool results from the agent — especially Bash captures
/// or large grep outputs — can run into the megabytes; that's fine to
/// hand back to the model, but storing it verbatim makes our render
/// pass tokenize and lay out a wall of text every time the user
/// expands a tool block. 64K chars per payload is generous enough to
/// keep typical traces intact while bounding the worst case.
const TOOL_PAYLOAD_MAX_CHARS: usize = 65_536;

/// Trim oversized strings on a tool call's content/raw_input/raw_output
/// to [`TOOL_PAYLOAD_MAX_CHARS`]. Idempotent: re-running on a tool that
/// got further updated only re-trims new growth.
fn cap_tool_call_payloads(tc: &mut sketch::acp_channel::ToolCall) {
    use sketch::acp_channel::ToolCallContent;
    for c in tc.content.iter_mut() {
        match c {
            ToolCallContent::Content(content) => {
                if let agent_client_protocol::schema::ContentBlock::Text(t) =
                    &mut content.content
                {
                    if t.text.chars().count() > TOOL_PAYLOAD_MAX_CHARS {
                        t.text = cap_string_chars(&t.text, TOOL_PAYLOAD_MAX_CHARS);
                    }
                }
            }
            ToolCallContent::Diff(d) => {
                if d.new_text.chars().count() > TOOL_PAYLOAD_MAX_CHARS {
                    d.new_text = cap_string_chars(&d.new_text, TOOL_PAYLOAD_MAX_CHARS);
                }
                if let Some(old) = &mut d.old_text {
                    if old.chars().count() > TOOL_PAYLOAD_MAX_CHARS {
                        *old = cap_string_chars(old, TOOL_PAYLOAD_MAX_CHARS);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(input) = &mut tc.raw_input {
        cap_json_value_strings(input, TOOL_PAYLOAD_MAX_CHARS);
    }
    if let Some(output) = &mut tc.raw_output {
        cap_json_value_strings(output, TOOL_PAYLOAD_MAX_CHARS);
    }
}

/// Walk a `serde_json::Value` and trim any string leaf longer than
/// `max_chars`. Used on tool-call raw_input/raw_output so a single
/// massive `stdout` field can't bloat the cached payload.
fn cap_json_value_strings(v: &mut serde_json::Value, max_chars: usize) {
    match v {
        serde_json::Value::String(s) => {
            if s.chars().count() > max_chars {
                *s = cap_string_chars(s, max_chars);
            }
        }
        serde_json::Value::Array(arr) => {
            for x in arr {
                cap_json_value_strings(x, max_chars);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, x) in map {
                cap_json_value_strings(x, max_chars);
            }
        }
        _ => {}
    }
}

/// Cap a string at `max_chars` UTF-8 chars, replacing the dropped tail
/// with a marker. Used at storage time on tool-call content/output so
/// the renderer never has to chew through multi-MB payloads even when
/// the user expands a tool block. Operates on chars (not bytes) to
/// avoid splitting multi-byte sequences.
fn cap_string_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    let dropped = s.chars().count() - max_chars;
    format!("{head}\n… (+{dropped} chars truncated at storage)")
}

/// Tokenize a line's segments into per-word + per-whitespace-run children
/// inside a `flex_wrap` row, so the GPUI flex layout breaks at word
/// boundaries when the row exceeds container width. StyledText itself
/// doesn't word-wrap, so we have to feed flex many small children for it
/// to have somewhere to break.
///
/// Cursor handling is fused in: for the cursor line, the caret is emitted
/// inline as its own flex child between the before/after halves of the
/// containing token. This keeps wrap behaviour consistent across cursor
/// and non-cursor lines.
#[allow(clippy::too_many_arguments)]
fn build_wrapped_line(
    segs: &[Segment],
    line_str: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    base_style: NStyle,
    base_fg: u32,
    code_font: &SharedString,
) -> AnyElement {
    let mut row = div().flex().flex_row().flex_wrap().flex_1().min_w_0();

    // Tokenize each segment into runs of whitespace vs non-whitespace,
    // preserving the segment's style on each token.
    let mut tokens: Vec<Segment> = Vec::new();
    for (text, style) in segs {
        if text.is_empty() {
            continue;
        }
        let mut current = String::new();
        let mut current_ws = false;
        for ch in text.chars() {
            let is_ws = ch == ' ' || ch == '\t';
            if current.is_empty() {
                current_ws = is_ws;
                current.push(ch);
            } else if current_ws == is_ws {
                current.push(ch);
            } else {
                tokens.push((std::mem::take(&mut current), *style));
                current_ws = is_ws;
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push((current, *style));
        }
    }

    // Empty-line placeholder so the row still occupies a visual line.
    if tokens.is_empty() {
        let line = segments_to_styled_line(&[(" ".to_string(), base_style)]);
        row = row.child(styled_line_element(
            &line, base_style, base_fg, code_font, code_font,
        ));
        if is_cursor_line {
            row = row.child(make_caret(mode, ' ', cursor_color));
        }
        return row.into_any_element();
    }

    if !is_cursor_line {
        for (text, style) in &tokens {
            let line = segments_to_styled_line(&[(text.clone(), *style)]);
            row = row.child(styled_line_element(
                &line, base_style, base_fg, code_font, code_font,
            ));
        }
        return row.into_any_element();
    }

    // Cursor line: walk tokens by visual column and inject the caret at the
    // cursor's column boundary, splitting the containing token if needed.
    let line_chars = line_str.chars().count();
    let cursor_col = cursor_col.min(line_chars);
    let mut col_so_far = 0usize;
    let mut caret_emitted = false;

    for (text, style) in &tokens {
        let token_chars = text.chars().count();
        let token_end_col = col_so_far + token_chars;
        let caret_in_token = !caret_emitted
            && cursor_col >= col_so_far
            && cursor_col <= token_end_col;

        if caret_in_token {
            let split_point = cursor_col - col_so_far;
            let chars: Vec<char> = text.chars().collect();
            let before: String = chars[..split_point].iter().collect();
            if !before.is_empty() {
                let line = segments_to_styled_line(&[(before, *style)]);
                row = row.child(styled_line_element(
                    &line, base_style, base_fg, code_font, code_font,
                ));
            }
            let cursor_char = chars.get(split_point).copied().unwrap_or(' ');
            row = row.child(make_caret(mode, cursor_char, cursor_color));
            caret_emitted = true;
            // After-the-caret: in Normal mode the cursor cell consumed the
            // char at split_point; in Insert mode it's a zero-width beam so
            // the char at split_point still belongs to the after-stream.
            let after_start = match mode {
                EditMode::Normal => split_point + 1,
                EditMode::Insert => split_point,
            };
            if after_start < chars.len() {
                let after: String = chars[after_start..].iter().collect();
                let line = segments_to_styled_line(&[(after, *style)]);
                row = row.child(styled_line_element(
                    &line, base_style, base_fg, code_font, code_font,
                ));
            }
        } else {
            let line = segments_to_styled_line(&[(text.clone(), *style)]);
            row = row.child(styled_line_element(
                &line, base_style, base_fg, code_font, code_font,
            ));
        }
        col_so_far = token_end_col;
    }

    // Cursor sits past the last char (e.g., end-of-line in Insert mode).
    if !caret_emitted {
        row = row.child(make_caret(mode, ' ', cursor_color));
    }

    row.into_any_element()
}

/// Build the cursor caret element. Pulled out so the empty-line, mid-line,
/// and end-of-line code paths all produce identical-looking carets.
///
/// Render a single chatbox logical line as a wrapping row.
///
/// Long lines wrap at whitespace boundaries (flex_wrap), so the cursor stays
/// visible without horizontal scrolling. The caret is emitted inline as its
/// own flex child between the before/after halves of the containing token,
/// so wrap behaviour stays consistent across cursor and non-cursor lines.
fn build_chatbox_line(
    full_text: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    sel: Option<((usize, usize), (usize, usize))>,
    line_idx: usize,
    total_line_chars: usize,
    code_font: &SharedString,
    text_color: Hsla,
) -> AnyElement {
    let line_h = px(18.0);
    let fg: Hsla = text_color;
    let sel_bg: Hsla = ncolor_to_hsla(SELECTION_BG, BG);

    let chars: Vec<char> = full_text.chars().collect();
    let char_count = chars.len();

    // Selection range projected onto this line.
    let line_sel = sel
        .and_then(|s| line_selection_range(s, line_idx, total_line_chars))
        .and_then(|(s, e)| {
            if e > s {
                Some((s.min(char_count), e.min(char_count)))
            } else {
                None
            }
        });

    // Tokenize into whitespace vs non-whitespace runs so flex_wrap can break
    // at token boundaries. Each token becomes its own flex child.
    let mut tokens: Vec<String> = Vec::new();
    {
        let mut current = String::new();
        let mut current_ws = false;
        for ch in chars.iter().copied() {
            let is_ws = ch == ' ' || ch == '\t';
            if current.is_empty() {
                current_ws = is_ws;
                current.push(ch);
            } else if current_ws == is_ws {
                current.push(ch);
            } else {
                tokens.push(std::mem::take(&mut current));
                current_ws = is_ws;
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }

    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .min_w_0()
        .w_full()
        .min_h(line_h)
        .font_family(code_font.clone())
        .text_size(px(13.0))
        .text_color(fg);

    // Emit a chunk of text with the on-line selection highlight painted
    // through any overlapping range. `chunk_start_col` is the column at
    // which `text` begins on the logical line.
    let emit_chunk = |row: gpui::Div, text: String, chunk_start_col: usize| -> gpui::Div {
        if text.is_empty() {
            return row;
        }
        let chunk_chars: Vec<char> = text.chars().collect();
        let chunk_len = chunk_chars.len();
        let chunk_end_col = chunk_start_col + chunk_len;
        if let Some((ss, se)) = line_sel {
            if se > chunk_start_col && ss < chunk_end_col {
                let local_ss = ss.saturating_sub(chunk_start_col).min(chunk_len);
                let local_se = se.saturating_sub(chunk_start_col).min(chunk_len);
                let mut r = row;
                if local_ss > 0 {
                    let pre: String = chunk_chars[..local_ss].iter().collect();
                    r = r.child(pre);
                }
                if local_se > local_ss {
                    let in_sel: String = chunk_chars[local_ss..local_se].iter().collect();
                    r = r.child(div().bg(sel_bg).child(in_sel));
                }
                if local_se < chunk_len {
                    let post: String = chunk_chars[local_se..].iter().collect();
                    r = r.child(post);
                }
                return r;
            }
        }
        row.child(text)
    };

    // Empty line: just emit a placeholder space + (cursor if needed) so the
    // row still occupies a visual line.
    if tokens.is_empty() {
        if is_cursor_line {
            row = row.child(make_caret(mode, ' ', cursor_color));
        } else {
            row = row.child(" ");
        }
        return row.into_any_element();
    }

    if !is_cursor_line {
        let mut col_so_far = 0usize;
        for token in &tokens {
            let token_len = token.chars().count();
            row = emit_chunk(row, token.clone(), col_so_far);
            col_so_far += token_len;
        }
        return row.into_any_element();
    }

    // Cursor line: walk tokens by column and inject the caret at the cursor's
    // column boundary, splitting the containing token if needed.
    let cursor_col = cursor_col.min(char_count);
    let mut col_so_far = 0usize;
    let mut caret_emitted = false;
    for token in &tokens {
        let token_chars: Vec<char> = token.chars().collect();
        let token_len = token_chars.len();
        let token_end_col = col_so_far + token_len;
        let caret_in_token =
            !caret_emitted && cursor_col >= col_so_far && cursor_col <= token_end_col;

        if caret_in_token {
            let split_point = cursor_col - col_so_far;
            let before: String = token_chars[..split_point].iter().collect();
            if !before.is_empty() {
                row = emit_chunk(row, before, col_so_far);
            }
            let cursor_char = token_chars.get(split_point).copied().unwrap_or(' ');
            row = row.child(make_caret(mode, cursor_char, cursor_color));
            caret_emitted = true;
            // In Normal mode the cursor cell consumed the char at split_point;
            // in Insert mode the caret is a zero-width beam so the char at
            // split_point still belongs to the after-stream.
            let after_start = match mode {
                EditMode::Normal => split_point + 1,
                EditMode::Insert => split_point,
            };
            if after_start < token_len {
                let after: String = token_chars[after_start..].iter().collect();
                row = emit_chunk(row, after, col_so_far + after_start);
            }
        } else {
            row = emit_chunk(row, token.clone(), col_so_far);
        }
        col_so_far = token_end_col;
    }

    // Cursor sits past the last char (e.g., end-of-line in Insert mode).
    if !caret_emitted {
        row = row.child(make_caret(mode, ' ', cursor_color));
    }

    row.into_any_element()
}

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
        .child(if mode == EditMode::Normal { cursor_char.to_string() } else { " ".into() })
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

/// Wire a `ListState`'s scroll handler to update the shared `follow_output`
/// flag. When the user scrolls up (`is_scrolled == true`), follow is disabled.
/// When they scroll back to the bottom (`is_scrolled == false`), it re-enables.
fn setup_list_follow_handler(
    list_state: &gpui::ListState,
    follow: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    let flag = follow.clone();
    list_state.set_scroll_handler(move |ev: &gpui::ListScrollEvent, _w, _cx| {
        // `is_scrolled` is false when the list is pinned to the bottom
        // (logical_scroll_top == None in GPUI's ListState internals).
        flag.set(!ev.is_scrolled);
    });
}

/// Called when the ACP turn ends (the agent's `session/prompt` response
/// resolves). Ensures the transcript has a trailing newline so the next
/// chunk has a clean starting point. The cursor stays where the user put
/// it (the worksheet is cursor-anchored, not auto-following the agent —
/// spec-agent-window.md §19).
fn finalize_agent_turn(editor: &mut Editor) {
    let total_len = editor.document().rope().len_chars();
    let needs_newline = total_len == 0
        || editor
            .document()
            .full_text()
            .chars()
            .last()
            .map(|c| c != '\n')
            .unwrap_or(true);
    if needs_newline {
        editor.programmatic_insert(total_len, "\n");
    }
    // Perf cache (finding 2): the turn is over; invalidate the LLM-tail hint so
    // the next turn re-anchors from scratch instead of trusting a stale line.
    editor.clear_cached_llm_line();
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
    /// Stashed editor from a prior Edit-mode session in the same file,
    /// preserved across Ctrl-V round-trips so unsaved edits aren't lost
    /// when previewing the rendered view. `None` for files that have
    /// only been viewed (never edited) or that came in fresh from disk.
    edit_cache: Option<EditState>,
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

    /// O(1) pointer clone of the blocks snapshot, rebuilding it (one full
    /// clone) only when `blocks_seq` has advanced past the version the cached
    /// snapshot was built at. Mirrors `EditState.lines_cache` keyed on
    /// `edit_seq`.
    fn blocks_rc(&self) -> Rc<Vec<RenderedBlock>> {
        let mut slot = self.blocks_snapshot.borrow_mut();
        if let Some((seq, rc)) = slot.as_ref() {
            if *seq == self.blocks_seq {
                return rc.clone();
            }
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

/// Which input surface the agent window is currently presenting. Per
/// spec-agent-window.md §4, every `AgentState` carries one of these two
/// values; new sessions start at `Chatbox` to match today's compose-box-
/// first feel. Toggled by `Ctrl-Alt-Enter` (§5).
///
/// `InputModeKind` is the **Copy discriminant** — the two-variant tag with no
/// payload, kept for the persisted `PersistedSlot.mode` string and the
/// `should_follow_tail` policy fn. The live state is [`InputSurface`], which
/// owns the `Chatbox` inside its variant so "a chatbox exists iff we're in
/// Chatbox mode" is enforced by the type rather than two hand-synced fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputModeKind {
    /// User input is interleaved with LLM output in the transcript editor.
    /// Frozen lines are immutable; editable lines accumulate until a Submit
    /// sweeps and freezes them all (§9–§15).
    Worksheet,
    /// User input goes into a separate `Chatbox` editor pinned to the
    /// bottom of the window. The transcript is read-only while in this
    /// mode (§16–§20).
    Chatbox,
}

/// The single live input surface of an agent window (§4–§5). Replaces the old
/// `input_mode: InputMode` + `chatbox: Option<Chatbox>` pair: the Chatbox data
/// lives INSIDE the `Chatbox` variant, so the illegal states (Chatbox-mode with
/// no box, or Worksheet with a stranded box) are unrepresentable. New sessions
/// start at `Chatbox` (compose-box-first); `Ctrl-Alt-Enter` toggles. NOT `Copy`
/// (it owns a `Chatbox`).
enum InputSurface {
    Worksheet,
    Chatbox(Chatbox),
}

impl InputSurface {
    fn is_chatbox(&self) -> bool {
        matches!(self, InputSurface::Chatbox(_))
    }
    fn chatbox(&self) -> Option<&Chatbox> {
        match self {
            InputSurface::Chatbox(cb) => Some(cb),
            InputSurface::Worksheet => None,
        }
    }
    fn chatbox_mut(&mut self) -> Option<&mut Chatbox> {
        match self {
            InputSurface::Chatbox(cb) => Some(cb),
            InputSurface::Worksheet => None,
        }
    }
    /// The Copy discriminant, for the persisted mode string and
    /// `should_follow_tail` (which must not borrow the owned `Chatbox`).
    fn mode(&self) -> InputModeKind {
        match self {
            InputSurface::Worksheet => InputModeKind::Worksheet,
            InputSurface::Chatbox(_) => InputModeKind::Chatbox,
        }
    }
}

/// Tool names that the v1 sub-agent classifier treats as sub-agents.
/// Centralised here so swapping in a structured ACP sub-agent type — or
/// supporting a renamed vendor tool — is a one-slice change (§25).
const SUBAGENT_TOOL_NAMES: &[&str] = &["Task", "Subagent", "Spawn"];

/// Sketch-side classification of a `ToolCall` that represents a sub-agent
/// transcript (§26). Produced by the heuristic in `classify_subagent`; the
/// `Subagents` sidepane lists these, and `focused_subagent` keys into the
/// derived list (by `tool_call_id`) to swap the main transcript view.
///
/// Not stored: `AgentState::subagents()` derives this list on demand by
/// folding over `tool_call_order` + `tool_calls`, so it can never drift
/// from the underlying tool-call state (ADR-0006 quick win #1).
#[derive(Clone)]
struct SubAgent {
    /// Originating tool-call id. The tool call itself stays in
    /// `tool_calls`; the sub-agent entry is an extra view over the same
    /// content.
    tool_call_id: ToolCallKey,
    /// Best-effort display label: the tool call's `title` if set,
    /// otherwise its `name`, with `subagent-N` as the ultimate fallback.
    label: String,
    /// Mirrors the underlying tool call's status.
    status: sketch::acp_channel::ToolCallStatus,
}

/// Heuristic classifier (§25). v1: anything with `kind == ToolKind::Other`
/// AND a `title` prefix in [`SUBAGENT_TOOL_NAMES`] is treated as a sub-
/// agent. (The spec calls the matching field "name"; ACP names it
/// `title` — same meaning, the user-facing label for the tool call.)
/// Returns the freshly-constructed `SubAgent`, or `None` if the tool call
/// doesn't match.
fn classify_subagent(tc: &sketch::acp_channel::ToolCall) -> Option<SubAgent> {
    use sketch::acp_channel::ToolKind;
    if tc.kind != ToolKind::Other {
        return None;
    }
    let title = tc.title.as_str();
    if !SUBAGENT_TOOL_NAMES
        .iter()
        .any(|prefix| title.starts_with(prefix))
    {
        return None;
    }
    let label = if title.is_empty() {
        "subagent".to_string()
    } else {
        title.to_string()
    };
    Some(SubAgent {
        tool_call_id: ToolCallKey::from_id(&tc.tool_call_id),
        label,
        status: tc.status,
    })
}

/// Per-line metadata that the Worksheet gutter reads to label each line.
/// Stored in `editor.metadata::<TurnId>()` keyed by `LineAnchor`, so the
/// tag follows the line through inserts, deletes, and inter-block
/// annotations (spec-agent-window.md §11, §E2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnId {
    /// LLM output, turn N. Gutter prints `N` in a dim accent color.
    Llm(usize),
    /// User input frozen as part of turn n's prompt. Gutter prints `Un`.
    User(usize),
    /// Tool-call block originating from turn N. Gutter prints `Tn`.
    /// Lives on the anchor line of a `ToolGroup` flat-item.
    Tool(usize),
    /// Sketch-local lifecycle notice (attach/detach/disconnect/permission/
    /// force-restart, retry `Notice`s). NOT agent-authored: it carries no
    /// turn number, never emits a Claude `TurnHeader`, renders with a blank
    /// gutter, and is excluded from agent-turn numbering and the live≡replay
    /// parity contract (which is defined over `{User, Llm, Tool}` only —
    /// Finding 5, INV-3 / Constraint 5). Kept out of `append_llm_chunk`'s
    /// `Llm(k)` lane so a notice can never seed or mis-attribute a turn.
    System,
}

/// The role a header-owning turn maps to. A header-owning turn is exactly
/// `{Llm, User}`; `Tool` and `System` turns anchor ToolGroups / lifecycle
/// notices and never emit a `TurnHeader`. Encoding this as a returned
/// `Option<HeaderRole>` (rather than an `unreachable!()` arm) makes "Tool
/// has no header" a compiler-checked total mapping (Finding 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderRole {
    Claude,
    User,
}

impl HeaderRole {
    /// Total mapping from a turn id to the header it owns (if any).
    /// `Tool`/`System` -> `None`; `Llm` -> `Claude`; `User` -> `User`.
    fn from_turn(tid: TurnId) -> Option<HeaderRole> {
        match tid {
            TurnId::Llm(_) => Some(HeaderRole::Claude),
            TurnId::User(_) => Some(HeaderRole::User),
            TurnId::Tool(_) | TurnId::System => None,
        }
    }

    fn into_turn_role(self) -> TurnRole {
        match self {
            HeaderRole::Claude => TurnRole::Claude,
            HeaderRole::User => TurnRole::User,
        }
    }
}

/// Standalone input editor used when `InputMode == Chatbox`. Has its own
/// document, cursor, undo stack, and modal state (§16). The chatbox is
/// dropped on a `Chatbox → Worksheet` toggle (§6) and re-constructed empty
/// on a `Worksheet → Chatbox` toggle (§7) — undo history doesn't survive
/// the round trip; the previous draft is recoverable as transcript
/// content if the user already submitted.
struct Chatbox {
    editor: Editor,
    mode: EditMode,
    scroll_handle: ScrollHandle,
}

impl Chatbox {
    fn new() -> Self {
        Self {
            editor: Editor::new(String::new(), std::path::PathBuf::from("*chatbox*")),
            mode: EditMode::Insert,
            scroll_handle: ScrollHandle::new(),
        }
    }

    fn text(&self) -> String {
        self.editor.document().full_text()
    }
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
        self.view.move_right_clamped(&self.core.borrow(), insert_mode);
    }
    fn clamp_cursor_col(&mut self, insert_mode: bool) {
        self.view.clamp_cursor_col(&self.core.borrow(), insert_mode);
    }
    fn move_cursor_line_end(&mut self, insert_mode: bool) {
        self.view.move_cursor_line_end(&self.core.borrow(), insert_mode);
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

/// Build the trimmed, tab-expanded per-line text for an Edit pane's body,
/// reading the pooled core's rope once. Mirrors the prior per-line
/// `document().line_text(i)` loop but takes a single `RefCell` borrow.

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

enum WindowContent {
    Doc(DocState),
    Edit(EditState),
    Agent(AgentRing),
    Browser(BrowserWindow),
}

/// The agent turn lifecycle as one explicit state (Finding 9). Replaces the
/// loose `(awaiting_reply, turn_started, last_event_at, stop_requested_at)`
/// quadruple whose valid combinations were unwritten convention — e.g.
/// `awaiting_reply=false` with `stop_requested_at=Some` was structurally
/// reachable but meaningless. Each transition site (submit, on-event,
/// finalize, reset_for_replay, the Stop handler) is now a total function over
/// this enum, and the thinking indicator / Stop-escalation read the variant
/// rather than probing flag combinations.
///
/// Invariants made unrepresentable: a `since`/`escalated` stop marker can only
/// exist while the turn is in flight (it lives *inside* `StopRequested`), and
/// the elapsed/quiet timers (`started`/`last_event`) only exist while awaiting.
#[derive(Clone, Copy, Debug)]
enum TurnPhase {
    /// No turn in flight. The footer shows no spinner; Stop is a no-op.
    Idle,
    /// A prompt was sent and we're streaming the reply. `started` drives the
    /// elapsed timer; `last_event` drives the "quiet for M:SS" stall reading.
    Awaiting {
        started: std::time::Instant,
        last_event: std::time::Instant,
    },
    /// The user pressed Stop once; a graceful `session/cancel` is pending but
    /// the turn is still in flight (timers keep running). A second Stop while
    /// in this state escalates to a hard kill + resume — captured by setting
    /// `escalated` on the way into `force_restart_agent`. Carries the same
    /// `started`/`last_event` so the indicator keeps reading correctly.
    StopRequested {
        started: std::time::Instant,
        last_event: std::time::Instant,
        since: std::time::Instant,
        escalated: bool,
    },
}

impl TurnPhase {
    /// True while a reply is in flight (Awaiting or StopRequested). Drives the
    /// thinking indicator, the Stop button's visibility, and `any_agent_awaiting`.
    fn is_awaiting(&self) -> bool {
        !matches!(self, TurnPhase::Idle)
    }

    /// When the in-flight turn started (elapsed-timer source), or `None` when idle.
    fn turn_started(&self) -> Option<std::time::Instant> {
        match self {
            TurnPhase::Idle => None,
            TurnPhase::Awaiting { started, .. }
            | TurnPhase::StopRequested { started, .. } => Some(*started),
        }
    }

    /// Last inbound reply activity (quiet-clock source), or `None` when idle.
    fn last_event_at(&self) -> Option<std::time::Instant> {
        match self {
            TurnPhase::Idle => None,
            TurnPhase::Awaiting { last_event, .. }
            | TurnPhase::StopRequested { last_event, .. } => Some(*last_event),
        }
    }

    /// True once the user pressed Stop for the in-flight turn (a graceful
    /// cancel is pending). A second Stop in this state escalates.
    fn stop_requested(&self) -> bool {
        matches!(self, TurnPhase::StopRequested { .. })
    }

    /// Refresh the quiet-clock for the in-flight turn (any inbound event). A
    /// no-op when idle. Preserves a pending Stop request.
    fn note_event(&mut self, now: std::time::Instant) {
        match self {
            TurnPhase::Idle => {}
            TurnPhase::Awaiting { last_event, .. }
            | TurnPhase::StopRequested { last_event, .. } => *last_event = now,
        }
    }

    /// Enter the awaiting state on a successful submit. Clears any prior Stop.
    fn begin(now: std::time::Instant) -> Self {
        TurnPhase::Awaiting {
            started: now,
            last_event: now,
        }
    }

    /// Record the user's first Stop (graceful cancel pending). No-op if not
    /// awaiting, or already stop-requested (idempotent on repeat from a stale
    /// call path — escalation is decided by the handler before this runs).
    fn request_stop(&mut self, now: std::time::Instant) {
        if let TurnPhase::Awaiting { started, last_event } = *self {
            *self = TurnPhase::StopRequested {
                started,
                last_event,
                since: now,
                escalated: false,
            };
        }
    }

    /// Mark the pending Stop as escalated (a second Stop → hard kill + resume).
    /// Only meaningful while `StopRequested`; a no-op otherwise. The caller
    /// then drives `force_restart_agent`, which returns the phase to `Idle`.
    fn escalate(&mut self) {
        if let TurnPhase::StopRequested { escalated, .. } = self {
            *escalated = true;
        }
    }

    /// Whether a pending Stop has been escalated to a hard kill (second Stop).
    fn is_escalated(&self) -> bool {
        matches!(self, TurnPhase::StopRequested { escalated: true, .. })
    }
}

/// State held while the user is conversing with an ACP-attached Claude
/// agent. The transcript lives in an in-memory `Editor` (no on-disk file);
/// Claude's replies are spliced in as frozen lines via the same lock-and-
/// advance pattern the TUI uses (`app::claude::append_to_claude_buffer`),
/// so the user can keep typing inline edits between turns.
struct AgentState {
    /// Editor over the chat transcript. `frozen_lines` mark Claude's turns;
    /// the editable region below `lockable_through_line` is the user's
    /// pending draft.
    editor: Editor,
    /// Live ACP connection. `None` while attaching, after a worker crash,
    /// or when the user pre-emptively detached.
    channel: Option<AcpChannelClient>,
    /// Receiver for an in-flight ACP attach. The attach runs on a
    /// background `std::thread` because `AcpChannelClient::spawn` blocks
    /// on the worker's initialize handshake — running it on the GPUI
    /// foreground executor would freeze the UI. The pump task polls this
    /// each tick; when it resolves, the result moves to `channel` and
    /// `attach_pending` clears.
    attach_pending: Option<std::sync::mpsc::Receiver<std::io::Result<AcpChannelClient>>>,
    mode: EditMode,
    keybinds: KeybindManager,
    /// Virtualized list state for the claude transcript. We render
    /// every doc-line + tool-block as an item in a `gpui::list` —
    /// non-uniform-height list that only paints visible rows. Without
    /// this, render scaled with total transcript length and made input
    /// laggy on long sessions because every cx.notify re-tokenized
    /// every line for word-wrap. `ListAlignment::Bottom` gives the
    /// chat-style initial pin. The `follow_output` flag (maintained by
    /// the scroll handler) gates pump-driven auto-scroll so the user
    /// can scroll up to read history without being yanked to the bottom.
    list_state: gpui::ListState,
    /// Total number of items currently registered in `list_state`. We
    /// track it separately so we can splice in new items as the
    /// buffer grows without paying for a full reset.
    list_item_count: usize,
    /// Footer status line — attach result, send result, error. Cleared on
    /// the next non-Ctrl keystroke so it persists for at least one frame.
    status: Option<SharedString>,
    /// The turn lifecycle as one explicit state (Finding 9). `Idle` between
    /// turns; `Awaiting` while streaming a reply (carrying the elapsed-timer
    /// `started` and the quiet-clock `last_event`); `StopRequested` once the
    /// user pressed Stop (a graceful cancel pending, a second Stop escalates).
    /// Replaces the prior `(awaiting_reply, turn_started, last_event_at,
    /// stop_requested_at)` quadruple — see `TurnPhase`.
    turn_phase: TurnPhase,
    /// The turn-number state machine (Findings 3 & 13, INV-3/INV-4) — the
    /// **single owner** of `k`. Holds `last_seen` (settled live turns; the pump
    /// compares the live counter against it each tick, and when it ticks up the
    /// in-flight turn just ended → finalize the buffer + return the phase to
    /// `Idle`) and `replay_turn` (the replay cursor). On `session/load` the
    /// agent re-emits the whole prior conversation as one burst of
    /// `UserMessage`/`Chunk` events with no per-turn prompt-response to advance
    /// `last_seen`; without the cursor the replayed history collapses into one
    /// `TurnId::Llm(1)`. Instead each replayed `UserMessage` boundary steps the
    /// cursor so chunks attach to the *next* `Llm(k)` —
    /// `User(1),Llm(1),User(2),Llm(2)` — and `current_turn()` prefers it when
    /// non-zero so live submit and replay share one source of `k`.
    /// `ReplayComplete` folds the cursor back into `last_seen` and zeroes it.
    /// (Was two loose `usize` fields reconstructed into a temporary `ReplayTurns`
    /// on every read and copied back out on every mutation — now owned directly.)
    replay_turns: sketch::acp_channel::ReplayTurns,
    /// `edit_seq` at which the tail was last revealed by the follow-scroll
    /// (F4, INV-13). The render-time re-reveal historically fired only when
    /// the flat-item COUNT changed, so a chunk that grows the last line/block
    /// without adding a row (agent prose before a `\n`, or a streaming code
    /// fence) was skipped and the freshly grown tail fell below the fold.
    /// Tracking the last-scrolled `edit_seq` lets the reveal fire on content
    /// growth regardless of count delta, while still de-duping idle frames
    /// (same `edit_seq` ⇒ no re-scroll). `u64::MAX` = never scrolled.
    last_scrolled_edit_seq: u64,
    /// Live tool calls keyed by `tool_call_id`. Updated in place as the
    /// agent emits `ToolCallUpdate` notifications (status → in_progress →
    /// completed/failed, content arriving incrementally, etc.).
    tool_calls: std::collections::HashMap<ToolCallKey, sketch::acp_channel::ToolCall>,
    /// Display order — `tool_call_id`s in the chronological order they
    /// were first announced. Drives both rendering order and "render
    /// after which buffer line" via [`tool_call_anchor_line`].
    tool_call_order: Vec<ToolCallKey>,
    /// Anchors a tool call to the buffer line that was the last frozen
    /// line at the moment it was announced. The renderer slots the tool
    /// block in just after that line, so tool blocks land between the
    /// chunks that bracketed them in time.
    tool_call_anchor_line: std::collections::HashMap<ToolCallKey, LineAnchor>,
    /// Tool calls the user has expanded inline. Default-collapsed; click
    /// the summary header to expand or recollapse.
    expanded_tool_calls: std::collections::HashSet<String>,
    /// Line ranges `(start, end)` that are rendered as structural blocks
    /// (tables, fenced code blocks) instead of line-by-line. Updated each
    /// render pass; used by `cursor_visible_child_index` for scroll math.
    block_ranges: Vec<(usize, usize)>,
    /// Cache of parsed RenderedBlocks keyed by `(start, end)` range.
    /// Invalidated when frozen line count changes.
    block_cache: std::collections::HashMap<(usize, usize), RenderedBlock>,
    /// Frozen line count when `block_cache` was last populated.
    block_cache_frozen_count: usize,
    /// Cached per-line transcript text (trimmed + tab-expanded) used by
    /// `render_agent`. Perf: building this `Vec<String>` allocates a String
    /// per transcript line on every `cx.notify()` (cursor blink, cross-pane
    /// wakeups, every streamed chunk), an O(L) cost regardless of how few
    /// lines changed. Cache it keyed on `edit_seq` so unchanged frames reuse
    /// the prior vec instead of re-extracting + re-allocating the whole doc.
    lines_cache: std::rc::Rc<Vec<String>>,
    /// `edit_seq` the `lines_cache` was built at; `u64::MAX` = never built.
    lines_cache_seq: u64,
    /// Memoized `render_agent` view-model. The flat-items list and per-line
    /// gutter tags depend only on the structural inputs captured in
    /// `view_model_fp` (edit_seq, frozen line count, tool-call order,
    /// expanded set, turn_phase.is_awaiting()) — NOT on cursor/selection/theme, which
    /// the render closure reads later. On a fingerprint hit `render_agent`
    /// reuses these `Rc`s verbatim and skips the whole rebuild (gutter scan,
    /// tool-anchor resolution, flat build, blank-collapse). Perf: those run
    /// every `cx.notify()` today (cursor blink, ~1Hz thinking tick), each an
    /// O(n) pass over the transcript.
    flat_items_cache: std::rc::Rc<Vec<FlatItem>>,
    gutter_cache: std::rc::Rc<Vec<Option<TurnId>>>,
    /// Fingerprint of the structural inputs the cached view-model was built
    /// from. `None` = never built (forces a rebuild on first render).
    view_model_fp: Option<u64>,
    /// Bumped on every view-model rebuild. Lets tests assert a fingerprint
    /// hit reused the cache (seq unchanged) vs. forced a rebuild.
    view_model_seq: u64,
    /// Incremental highlight cache for the transcript. Re-highlights only the
    /// lines that changed between renders instead of the whole buffer every
    /// `cx.notify()`. Bypassed when `SKETCH_HL_CACHE=0`.
    highlight_cache: HighlightCache,
    /// The active input surface (§4). The `Chatbox` draft editor lives INSIDE
    /// the `Chatbox` variant — make-illegal-states-unrepresentable, so the old
    /// "`chatbox` is `Some` iff `input_mode == Chatbox`" invariant (two
    /// hand-synced fields) is now enforced by the type. New sessions start at
    /// `Chatbox`; `Ctrl-Alt-Enter` toggles (§5).
    input_surface: InputSurface,
    /// Last-seen full snapshot of the agent's plan. Updated on every ACP
    /// `Plan` notification (which carries a complete plan, not a delta —
    /// see spec-agent-window.md §21). Consumed by the Tasklist sidepane.
    current_plan: Option<sketch::acp_channel::Plan>,
    /// Last-seen session mode id from the agent (Claude Code's `default` /
    /// `plan` / `learn`, etc.). Distinct from the permission mode on
    /// `AcpChannelClient`. Surfaced by the Status Strip.
    agent_mode: Option<sketch::acp_channel::SessionModeId>,
    /// Last-seen usage snapshot (tokens used/total, cost). Populated only
    /// when the upstream `unstable_session_usage` feature is on; otherwise
    /// stays `None` and the Status Strip omits these fields per §30.
    usage: Option<sketch::acp_channel::UsageSnapshot>,
    /// `tool_call_id` of the currently focused sub-agent. When `Some`, the
    /// main transcript area swaps to show that sub-agent's content instead
    /// of the root agent's (§27). Keyed by a stable `ToolCallKey` rather
    /// than a positional index so it survives any reordering of the derived
    /// `subagents()` list (ADR-0006 quick win #1).
    ///
    /// The sub-agent list itself is NOT stored — see `subagents()`, which
    /// derives it from `tool_call_order` + `tool_calls`.
    focused_subagent: Option<ToolCallKey>,
    /// Whether auto-scroll should follow new output. Defaults to `true`
    /// (pinned to bottom). Set to `false` when the user scrolls up in the
    /// transcript, re-enabled when they scroll back to the bottom or send
    /// a new message. Shared with the ListState scroll handler via Rc.
    follow_output: std::rc::Rc<std::cell::Cell<bool>>,
    /// Whether the Tasklist sidepane is open (§24).
    tasklist_open: bool,
    /// Whether the Subagents sidepane is open (§28).
    subagents_open: bool,
    /// True when this session is managed by the session server (client/server
    /// mode). False when the GUI owns the ACP subprocess directly (legacy).
    /// Checked by the status strip and anywhere that needs to distinguish
    /// the two paths from within `AgentState` alone.
    server_managed: bool,
    /// Order-independent reconciler for user-turn insertions — the single
    /// authority that de-dupes the three sites a user turn can be announced
    /// from (optimistic submit, server `UserPrompt`, agent `UserMessage`).
    /// Replaces the position-dependent `document_trimmed_end_ends_with`
    /// heuristic that double-rendered input whenever content streamed in
    /// between the optimistic echo and its stream copy. See `agent_transcript`.
    reconciler: sketch::agent_transcript::UserTurnReconciler,
    /// User-turn `k`s inserted since the last `reset_for_replay` generation.
    /// The M3 runtime tripwire asserts a `k` is never inserted twice — a
    /// double-render reuses a `k`, so this turns a silent visual regression
    /// into a loud, located failure. Scoped per generation (cleared on
    /// transcript wipe) so a reconnect's `k`-restart is not a false positive.
    user_turn_ks: std::collections::HashSet<usize>,
    /// Background polling task that drains the ACP channel into the editor
    /// every ~50ms. Held only so that dropping `AgentState` (e.g. on
    /// `back_to_doc`) cancels the task. The leading `_` mutes unused-field
    /// warnings — the field IS used (its Drop runs on screen exit), but
    /// no method reads it.
    _pump: Option<Task<()>>,
}

impl AgentState {
    /// Derived list of classified sub-agents (§25–§26), folded over
    /// `tool_call_order` + `tool_calls` in first-seen order. This is a pure
    /// projection of the tool-call state — there is no stored mirror to keep
    /// in sync, so it can never drift (ADR-0006 quick win #1). Each entry
    /// carries the originating tool-call id, label, and status, all read
    /// live from the underlying `ToolCall`.
    fn subagents(&self) -> Vec<SubAgent> {
        self.tool_call_order
            .iter()
            .filter_map(|id| self.tool_calls.get(id))
            .filter_map(classify_subagent)
            .collect()
    }

    /// Fingerprint of the structural inputs to the `render_agent`
    /// view-model (flat_items + gutter). Two renders with an equal
    /// fingerprint produce byte-identical flat_items/gutter, so the
    /// cached `Rc`s can be reused. Deliberately EXCLUDES cursor,
    /// selection, theme, and tool-call *content* — none of those affect
    /// the flat build (they're read later, inside the render closure).
    /// See the call site in `render_agent` (S1) for the trap analysis.
    fn view_model_fingerprint(&self, edit_seq: u64, frozen_line_count: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        edit_seq.hash(&mut h);
        frozen_line_count.hash(&mut h);
        self.tool_call_order.len().hash(&mut h);
        self.tool_call_order.last().hash(&mut h);
        // Resolved tool anchor lines (Finding 11, INV-8). The flat build
        // resolves each tool's `LineAnchor` to a current line via
        // `line_for_anchor` and groups tools by that line, so the resolved
        // line is a genuine input to the build. Folding it in (in
        // `tool_call_order` order, the same order the build reads) makes the
        // memo key name this dependency directly instead of relying on the
        // unstated invariant "anything that moves an anchor also bumps
        // `edit_seq`". Cheap: one `line_for_anchor` per live tool call.
        for id in &self.tool_call_order {
            let resolved = self
                .tool_call_anchor_line
                .get(id)
                .and_then(|&anchor| self.editor.line_for_anchor(anchor));
            resolved.hash(&mut h);
        }
        // Expanded set: hash len + sorted contents (order-independent).
        self.expanded_tool_calls.len().hash(&mut h);
        {
            let mut ids: Vec<&String> = self.expanded_tool_calls.iter().collect();
            ids.sort_unstable();
            for id in ids {
                id.hash(&mut h);
            }
        }
        self.turn_phase.is_awaiting().hash(&mut h);
        h.finish()
    }

    /// Return the memoized view-model (flat_items + gutter), reusing the
    /// cached `Rc`s when `fp` matches the fingerprint the cache was built
    /// at. On a miss, runs `rebuild`, stores the result, stamps the
    /// fingerprint, and bumps `view_model_seq`. The single source of truth
    /// for the S1 cache decision — exercised directly by
    /// `view_model_memoization_fast_skip`.
    fn memoize_view_model(
        &mut self,
        fp: u64,
        rebuild: impl FnOnce(&mut Self) -> (Vec<FlatItem>, Vec<Option<TurnId>>),
    ) -> (
        std::rc::Rc<Vec<FlatItem>>,
        std::rc::Rc<Vec<Option<TurnId>>>,
    ) {
        if self.view_model_fp == Some(fp) {
            // Fast skip: structural inputs unchanged — reuse the cache.
            return (self.flat_items_cache.clone(), self.gutter_cache.clone());
        }
        #[cfg(test)]
        {
            VIEW_MODEL_REBUILDS.with(|n| n.set(n.get() + 1));
        }
        let (flat_items, gutter) = rebuild(self);
        let flat_rc = std::rc::Rc::new(flat_items);
        let gutter_rc = std::rc::Rc::new(gutter);
        self.flat_items_cache = flat_rc.clone();
        self.gutter_cache = gutter_rc.clone();
        self.view_model_fp = Some(fp);
        self.view_model_seq = self.view_model_seq.wrapping_add(1);
        (flat_rc, gutter_rc)
    }

    /// Minimal `AgentState` for unit tests. Only the fields the S1
    /// memoization touches need realistic values; the rest are empty/default.
    #[cfg(test)]
    fn new_for_test() -> Self {
        AgentState {
            editor: Editor::new(String::new(), PathBuf::from("*claude*")),
            channel: None,
            attach_pending: None,
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(
                0,
                gpui::ListAlignment::Bottom,
                gpui::px(256.0),
            ),
            list_item_count: 0,
            status: None,
            turn_phase: TurnPhase::Idle,
            replay_turns: sketch::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tool_calls: std::collections::HashMap::new(),
            tool_call_order: Vec::new(),
            tool_call_anchor_line: std::collections::HashMap::new(),
            expanded_tool_calls: std::collections::HashSet::new(),
            block_ranges: Vec::new(),
            block_cache: std::collections::HashMap::new(),
            block_cache_frozen_count: 0,
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            flat_items_cache: std::rc::Rc::new(Vec::new()),
            gutter_cache: std::rc::Rc::new(Vec::new()),
            view_model_fp: None,
            view_model_seq: 0,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: true,
            reconciler: sketch::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: None,
        }
    }

    /// Build a fresh server-managed `AgentState` in the empty baseline, with
    /// `status` shown in the footer. Used for both the "connecting…"
    /// placeholder a panel renders the instant it opens (before the
    /// `list_sessions` / `create_session` round-trip lands) and for the
    /// reconnected/created slots once a `server_session_id` is known. Replaces
    /// the several copies of this giant struct literal that previously lived
    /// inline in `open_agent_inner` / `create_agent_session_via_server`. The
    /// follow handler is wired up before returning.
    fn new_server_managed(status: Option<SharedString>) -> Self {
        let state = AgentState {
            editor: Editor::new(String::new(), PathBuf::from("*claude*")),
            channel: None,
            attach_pending: None,
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            status,
            turn_phase: TurnPhase::Idle,
            replay_turns: sketch::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tool_calls: std::collections::HashMap::new(),
            tool_call_order: Vec::new(),
            tool_call_anchor_line: std::collections::HashMap::new(),
            expanded_tool_calls: std::collections::HashSet::new(),
            block_ranges: Vec::new(),
            block_cache: std::collections::HashMap::new(),
            block_cache_frozen_count: 0,
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            flat_items_cache: std::rc::Rc::new(Vec::new()),
            gutter_cache: std::rc::Rc::new(Vec::new()),
            view_model_fp: None,
            view_model_seq: 0,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: true,
            reconciler: sketch::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: None,
        };
        setup_list_follow_handler(&state.list_state, &state.follow_output);
        state
    }

    /// Single source of the in-flight turn number `k` (Finding 3, INV-3),
    /// used by both live submit and replay so gutter tags and TurnHeaders
    /// are identical in both regimes. Delegates to the owned `ReplayTurns`.
    fn current_turn(&self) -> usize {
        self.replay_turns.current_turn()
    }

    /// Advance the replay cursor at a replayed user-message boundary and
    /// return the new turn `k` (Finding 3, INV-3).
    fn advance_replay_user_boundary(&mut self) -> usize {
        self.replay_turns.advance_user_boundary()
    }

    /// The lowest turn number not yet issued to a user turn this generation.
    /// `current_turn()` (`= last_seen + 1`) only advances when a turn's boundary
    /// (`TurnEnded`) settles `last_seen`, so a NEW local submit made while the
    /// previous turn is still in flight would otherwise reuse the in-flight
    /// turn's `k`. Taking `max(current_turn(), next_unused_user_turn())` for a
    /// local submit gives pipelined submits distinct, monotonic turn numbers
    /// instead of colliding (and tripping the M3 double-insert tripwire). It is
    /// a no-op for the common non-pipelined case (`last_seen + 1` is already the
    /// next unused number). `user_turn_ks` holds every user `k` issued this
    /// generation and is wiped per replay generation, so this never drifts.
    fn next_unused_user_turn(&self) -> usize {
        self.user_turn_ks.iter().max().map_or(1, |m| m + 1)
    }

    /// THE single chokepoint for user-turn **dedup + turn-number attribution**.
    /// All four announcement sites — the chatbox optimistic submit, the
    /// worksheet submit, the server `UserPrompt` notification, and the agent's
    /// `UserMessage` echo — route their reconcile through here so suppression
    /// and `k`-derivation have exactly one home instead of drifting copies (the
    /// structural cause of the double-render regressions). Returns `Some(k)` —
    /// the canonical turn number the caller must COMMIT (freeze) however its
    /// surface lays the turn out — or `None` to skip (the reconciler suppressed
    /// an echo, or the M3 tripwire fired). The two commit shapes are
    /// [`insert_user_turn`] (append at EOF) and [`commit_worksheet_turn`]
    /// (freeze authored lines in place); both share this core so a worksheet
    /// turn can never drift from a chatbox turn in numbering or dedup.
    ///
    /// `advance_boundary` must be `true` only for the direct-channel replay
    /// path (`!server_managed`), where there is no replayed `TurnEnded` to bump
    /// the live counter and the [`ReplayTurns`] cursor must be stepped per user
    /// boundary. It is `false` for every live insertion and for the
    /// server-managed path (whose boundaries arrive as replayed `TurnEnded`),
    /// so a live or server turn can never wrongly drive the machine into replay
    /// mode. A *skipped* echo never advances the boundary — suppression and
    /// attribution stay decoupled.
    fn register_user_turn(
        &mut self,
        text: &str,
        origin: sketch::agent_transcript::UserTurnOrigin,
        advance_boundary: bool,
    ) -> Option<usize> {
        use sketch::agent_transcript::UserTurnAction;
        match self.reconciler.reconcile(origin, text, advance_boundary) {
            UserTurnAction::Skip => None,
            UserTurnAction::Insert { advance_boundary } => {
                let k = if advance_boundary {
                    self.advance_replay_user_boundary()
                } else {
                    // Every NON-replay insert mints a fresh turn (a local submit,
                    // or a live/server echo that wasn't suppressed — dual-source
                    // echoes for an existing turn already returned `Skip`). It
                    // should attribute to `current_turn()` (`= last_seen + 1`),
                    // BUT if that `k` is already taken because the previous turn's
                    // boundary hasn't advanced `last_seen` yet (a pipelined submit,
                    // or a content-mismatched echo), take the next unused number
                    // instead — otherwise two distinct turns collide on one `k`
                    // and trip the M3 tripwire (the live crash this guards). A
                    // no-op in the common, non-pipelined case.
                    self.current_turn().max(self.next_unused_user_turn())
                };
                // M3 runtime tripwire: a `k` inserted twice within one
                // generation means the dedup failed and we are about to
                // double-render. Panic in dev (located at the exact mutation);
                // log + drop the duplicate in release rather than ship a double.
                if !self.user_turn_ks.insert(k) {
                    debug_assert!(
                        false,
                        "double user turn: TurnId::User({k}) inserted twice \
                         (text={text:?}) — reconciler dedup regression"
                    );
                    eprintln!(
                        "[sketch-gpui] INVARIANT: TurnId::User({k}) already present; \
                         dropping duplicate user turn (text={text:?})"
                    );
                    return None;
                }
                Some(k)
            }
        }
    }

    /// Insert a user turn into the transcript by APPENDING it at EOF — the
    /// chatbox optimistic submit, the server `UserPrompt` notification, and the
    /// agent's `UserMessage` echo all route here. Delegates dedup + attribution
    /// to [`register_user_turn`] and commits an accepted turn via
    /// `freeze_as_user_turn`. (Worksheet submits share the same core but freeze
    /// in place — see [`commit_worksheet_turn`].)
    fn insert_user_turn(
        &mut self,
        text: &str,
        origin: sketch::agent_transcript::UserTurnOrigin,
        advance_boundary: bool,
    ) {
        if let Some(k) = self.register_user_turn(text, origin, advance_boundary) {
            self.editor.freeze_as_user_turn(text, TurnId::User(k));
        }
    }

    /// Commit a Worksheet-mode submit: derive the canonical turn `k` through the
    /// shared reconciler core ([`register_user_turn`]) — so the server/agent
    /// echo of this prompt is content-matched and **suppressed** instead of
    /// double-rendered — then freeze every collected line IN PLACE under
    /// `TurnId::User(k)`. Worksheet freezes pre-existing, possibly
    /// non-contiguous authored lines (blank spacers included) in document order,
    /// so it does its own per-line freeze rather than the EOF-append
    /// `freeze_as_user_turn`: the chokepoint supplies the *number*, the
    /// worksheet supplies the *placement*.
    ///
    /// `prompt_body` MUST be the joined body actually sent (not the raw
    /// per-line text): registering that is what lets `normalize_user_text` match
    /// the single multi-line echo. Worksheet is a LOCAL submit exactly like the
    /// chatbox, so `advance_boundary` is `false` and `k = current_turn()` (the
    /// single source for the in-flight turn number, INV-3) — replacing the old
    /// hand-rolled `last_seen_turns + 1`, which silently diverged from the
    /// chokepoint and never armed dedup. Returns the committed `k`, or `None` if
    /// the M3 tripwire fired (no lines frozen; the caller still clears/notifies).
    fn commit_worksheet_turn(
        &mut self,
        collected: &[(usize, String)],
        prompt_body: &str,
    ) -> Option<usize> {
        let k = self.register_user_turn(
            prompt_body,
            sketch::agent_transcript::UserTurnOrigin::LocalSubmit,
            false,
        )?;
        for (l, _) in collected {
            self.editor.add_frozen_lines(*l, *l + 1);
            let anchor = self.editor.anchor_for_line(*l);
            self.editor
                .metadata_mut::<TurnId>()
                .insert(anchor, TurnId::User(k));
        }
        Some(k)
    }

    /// Auto-scroll follow decision (F4, INV-13). In Chatbox mode the user's
    /// cursor isn't in the transcript so output streams with sticky-bottom
    /// behavior gated by `follow_output`; in Worksheet mode the viewport
    /// stays anchored to the cursor, following only when the cursor sits at
    /// EOF (the user is typing at the tail and wants to keep seeing fresh
    /// output). This is the single authority the pump (×2) and render-time
    /// re-reveal all consult, replacing the byte-identical copy that used to
    /// live at each site (and drift independently).
    fn follow_tail(&self) -> bool {
        let line_count = self.editor.document().line_count();
        let cursor_at_eof = self.editor.cursor().line + 1 >= line_count;
        should_follow_tail(self.input_surface.mode(), self.follow_output.get(), cursor_at_eof)
    }

    /// Reveal the tail item if we're following AND content has actually grown
    /// since the last reveal (F4, INV-13). The trigger keys on `edit_seq`
    /// (true content growth), NOT on flat-item COUNT, so a chunk that extends
    /// the last line/block without adding a row still re-pins the viewport.
    /// Idempotent within a frame: a repeat call at the same `edit_seq` is a
    /// no-op, so idle ticks don't fight a user who scrolled up. Returns whether
    /// a reveal was actually requested (exercised by the unit test).
    fn reveal_tail_if_following(&mut self, count: usize) -> bool {
        let edit_seq = self.editor.document().edit_seq();
        if count == 0 || edit_seq == self.last_scrolled_edit_seq || !self.follow_tail() {
            return false;
        }
        self.last_scrolled_edit_seq = edit_seq;
        self.list_state.scroll_to_reveal_item(count - 1);
        true
    }

    /// Reconcile the `(list_state, list_item_count)` pair to a new flat-item
    /// count, updating BOTH atomically so the GPUI `ListState` GPUI paints and
    /// the scalar we splice against can never drift (Finding 8, INV-12). When
    /// block ranges are active the item count can shrink unpredictably, so we
    /// reset rather than splice; an incremental splice preserves the height
    /// cache on pure growth. Returns whether the list grew (so callers / the
    /// follow path can key on growth without re-deriving it). This is the only
    /// mutator that touches `list_item_count`, so parity is a property of the
    /// method rather than discipline at each render surface.
    fn reconcile_list(&mut self, new_count: usize) -> bool {
        let old_count = self.list_item_count;
        if new_count != old_count {
            if !self.block_ranges.is_empty() || new_count < old_count {
                self.list_state.reset(new_count);
            } else {
                self.list_state
                    .splice(old_count..old_count, new_count - old_count);
            }
            self.list_item_count = new_count;
        }
        new_count > old_count
    }

    /// Fold the replay cursor back into the live counter at end-of-replay
    /// (Finding 13, INV-4).
    fn finish_replay(&mut self) {
        self.replay_turns.finish_replay();
    }

    /// Reset all transcript-derived state to the empty baseline so that a
    /// server re-attach — which replays the session's full `event_log` — can
    /// rebuild the transcript from scratch without duplicating what's already
    /// on screen. Used by the reconnect path. Preserves the live channel /
    /// attach handle, input mode, follow-output preference, and pump handle;
    /// only the rendered transcript and its derived caches are cleared.
    fn reset_for_replay(&mut self) {
        self.editor = Editor::new(String::new(), PathBuf::from("*claude*"));
        self.list_state =
            gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0));
        setup_list_follow_handler(&self.list_state, &self.follow_output);
        self.list_item_count = 0;
        // Fresh editor restarts `edit_seq` at 0; clear the follow-scroll
        // watermark so the first replayed chunk re-reveals the tail (F4).
        self.last_scrolled_edit_seq = u64::MAX;
        self.turn_phase = TurnPhase::Idle;
        self.replay_turns = sketch::acp_channel::ReplayTurns::default();
        // The transcript is being rebuilt from the authoritative event_log:
        // nothing is "pending local" any more, and this starts a fresh
        // tripwire generation (the replay re-numbers `k` from 1). This clear
        // MUST happen-before any replayed echo is processed — guaranteed since
        // reset runs inside the reconnect update before re-attach.
        self.reconciler.reset();
        self.user_turn_ks.clear();
        self.tool_calls.clear();
        self.tool_call_order.clear();
        self.tool_call_anchor_line.clear();
        self.expanded_tool_calls.clear();
        self.block_ranges.clear();
        self.block_cache.clear();
        self.block_cache_frozen_count = 0;
        self.lines_cache = std::rc::Rc::new(Vec::new());
        self.lines_cache_seq = u64::MAX;
        self.flat_items_cache = std::rc::Rc::new(Vec::new());
        self.gutter_cache = std::rc::Rc::new(Vec::new());
        self.view_model_fp = None;
        self.view_model_seq = 0;
        self.highlight_cache = HighlightCache::new();
        self.current_plan = None;
        self.focused_subagent = None;
        self.usage = None;
    }
}

/// A re-attachable session resolved by the background half of `open_agent`
/// (S4). Carries everything the main thread needs to fill or push a slot —
/// the attach round-trip has already been issued off-thread.
struct AttachedSlot {
    label: String,
    sid: String,
    /// The ACP session id, used as the slot's `resume_id`.
    acp_id: Option<String>,
    /// Footer status string ("reconnected …").
    status: String,
}

/// Outcome of the background session-server round-trips kicked off by
/// `spawn_open_agent_server`. Applied on the paint thread by
/// `apply_open_agent_resolution`.
enum OpenResolution {
    /// Existing cwd sessions were found and re-attached.
    Attached(Vec<AttachedSlot>),
    /// No existing session — a fresh one was created.
    Created {
        sid: String,
        acp_id: Option<String>,
    },
    /// The list or create round-trip failed; surface it on the placeholder.
    Failed(String),
}

/// Process-wide monotonic allocator for `AgentSlot::pending_open_token`.
/// Tokens are never reused, so an in-flight async server open always binds
/// back to exactly the placeholder that started it — even across rings whose
/// per-ring `index` counters both start at 0 (the collision that dropped a
/// session's events on restore: `pump: no slot for server session`).
static NEXT_OPEN_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn alloc_open_token() -> u64 {
    NEXT_OPEN_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A named wrapper around `AgentState` for multi-session support.
struct AgentSlot {
    /// User-facing label shown in the sidebar.
    label: String,
    /// Monotonic index for stable identification (not reused after close).
    index: usize,
    /// The session state. Contains editor, channel, tool calls, etc.
    state: AgentState,
    /// True if new content has arrived since the user last viewed this session.
    has_unseen_activity: bool,
    /// The id this slot was created from on persistence restore. The slot's
    /// persisted id stays this value even if `session/load` failed and the
    /// channel fell back to `session/new` with a different id — so the next
    /// reboot retries the original load. `None` for slots created fresh by
    /// `claude-new` (then the channel's session/new id is persisted).
    resume_id: Option<String>,
    /// Absolute working directory the agent subprocess runs in and the
    /// directory its tool calls resolve relative to (spec-agent-cwd.md §1).
    /// Defaults to `std::env::current_dir()` at slot creation; set
    /// explicitly via `:claude-new <path>` or `:claude-cd <path>`.
    cwd: PathBuf,
    /// When using the session server, this is the server-assigned session id.
    /// `None` when using direct AcpChannelClient spawning.
    server_session_id: Option<String>,
    /// Set while an async server open/create round-trip for this slot is in
    /// flight; the resolution binds back to this slot by matching the token
    /// across the whole workspace. Globally unique (see `alloc_open_token`) so
    /// it disambiguates two placeholders that share a per-ring `index` of 0.
    /// Cleared once the round-trip resolves. `None` for settled slots.
    pending_open_token: Option<u64>,
}

/// An ordered collection of `AgentSlot`s with one active slot.
/// Ring-style next/prev navigation wraps around.
struct AgentRing {
    slots: Vec<AgentSlot>,
    /// Index into `slots` for the currently-active session.
    active: usize,
    /// Monotonic counter for `AgentSlot::index` — never reused.
    next_index: usize,
    /// WindowContent to restore when leaving Claude entirely (Ctrl-V / back_to_doc).
    /// Belongs to the ring, not any individual session.
    underlying: Option<Box<WindowContent>>,
}

impl AgentRing {
    fn new(underlying: Option<Box<WindowContent>>) -> Self {
        Self {
            slots: Vec::new(),
            active: 0,
            next_index: 0,
            underlying,
        }
    }

    #[allow(dead_code)]
    fn active(&self) -> &AgentSlot {
        &self.slots[self.active]
    }

    fn active_mut(&mut self) -> &mut AgentSlot {
        &mut self.slots[self.active]
    }

    fn push(
        &mut self,
        label: String,
        state: AgentState,
        resume_id: Option<String>,
        cwd: PathBuf,
        server_session_id: Option<String>,
    ) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.slots.push(AgentSlot {
            label,
            index,
            state,
            has_unseen_activity: false,
            resume_id,
            cwd,
            server_session_id,
            pending_open_token: None,
        });
        self.active = self.slots.len() - 1;
        index
    }

    fn next(&mut self) {
        if self.slots.len() <= 1 {
            return;
        }
        self.active = (self.active + 1) % self.slots.len();
        self.slots[self.active].has_unseen_activity = false;
    }

    fn prev(&mut self) {
        if self.slots.len() <= 1 {
            return;
        }
        self.active = if self.active == 0 {
            self.slots.len() - 1
        } else {
            self.active - 1
        };
        self.slots[self.active].has_unseen_activity = false;
    }

    /// Remove the active slot and return its state. Advances to the next
    /// slot (or previous if at the end). Returns `None` if the ring is
    /// now empty.
    fn close_active(&mut self) -> Option<AgentSlot> {
        if self.slots.is_empty() {
            return None;
        }
        let removed = self.slots.remove(self.active);
        if self.slots.is_empty() {
            self.active = 0;
        } else if self.active >= self.slots.len() {
            self.active = self.slots.len() - 1;
        }
        if !self.slots.is_empty() {
            self.slots[self.active].has_unseen_activity = false;
        }
        Some(removed)
    }

    fn close_at(&mut self, idx: usize) -> Option<AgentSlot> {
        if idx >= self.slots.len() {
            return None;
        }
        let removed = self.slots.remove(idx);
        if self.slots.is_empty() {
            self.active = 0;
        } else if self.active >= self.slots.len() {
            self.active = self.slots.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
        if !self.slots.is_empty() {
            self.slots[self.active].has_unseen_activity = false;
        }
        Some(removed)
    }

    fn len(&self) -> usize {
        self.slots.len()
    }

    fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    #[allow(dead_code)]
    fn iter(&self) -> impl Iterator<Item = &AgentSlot> {
        self.slots.iter()
    }

    /// Find slot position by monotonic index.
    fn slot_by_index(&self, index: usize) -> Option<usize> {
        self.slots.iter().position(|s| s.index == index)
    }

    fn slot_by_index_mut(&mut self, index: usize) -> Option<&mut AgentSlot> {
        self.slots.iter_mut().find(|s| s.index == index)
    }

    /// Find a slot by its server-assigned session id.
    fn slot_by_server_session_id_mut(&mut self, sid: &str) -> Option<&mut AgentSlot> {
        self.slots.iter_mut().find(|s| {
            s.server_session_id.as_deref() == Some(sid)
        })
    }

    /// Position of the slot for `sid`, if any. Used to remove it via
    /// [`close_at`] when the server broadcasts that the session closed.
    fn position_by_server_session_id(&self, sid: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.server_session_id.as_deref() == Some(sid))
    }
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
enum ActiveOverlay {
    None,
    Menu(MenuOverlay),
    BufferSwitcher(BufferSwitcher),
    SessionSwitcher(SessionSwitcher),
    WorkspacePicker(WorkspacePicker),
    Rename(RenameOverlay),
}

impl Default for ActiveOverlay {
    fn default() -> Self {
        ActiveOverlay::None
    }
}

/// GPUI menu tree. Mirrors the TUI's `default_menu` for the navigation
/// commands that exist in the GPUI frontend; omits TUI-only entries
/// (search, claude-attach via socket, save-quit, …) that have no GPUI
/// counterpart yet.
fn gpui_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::label("Open"),
        MenuNode::entry("f", "file browser", "open-browser"),
        MenuNode::entry("b", "buffer list", "buffer-list"),
        MenuNode::submenu(
            "c",
            "claude",
            vec![
                MenuNode::entry("n", "new session", "claude-new"),
                MenuNode::entry("l", "list sessions", "claude-list"),
                MenuNode::entry("x", "close session", "claude-close"),
                MenuNode::entry("r", "rename session", "claude-rename"),
                MenuNode::entry("w", "toggle worksheet/chatbox (Ctrl-Alt-Enter)", "agent-input-toggle"),
                MenuNode::separator(),
                MenuNode::label("Build loop"),
                MenuNode::entry("p", "promote: build & launch candidate", "dev-build-candidate"),
                MenuNode::entry("P", "take over sessions (candidate)", "dev-take-over"),
            ],
        ),
        MenuNode::separator(),
        MenuNode::label("Edit"),
        MenuNode::entry("e", "edit (raw markdown)", "enter-edit"),
        MenuNode::entry("w", "edit (word processor)", "enter-wp"),
        MenuNode::entry("r", "reload from disk (discards unsaved)", "reload-file"),
        MenuNode::separator(),
        MenuNode::label("View"),
        MenuNode::entry("v", "back to doc", "back-to-doc"),
        MenuNode::entry("s", "toggle agent status bar position", "claude-status-bar"),
        MenuNode::submenu(
            "t",
            "theme",
            vec![
                MenuNode::entry("d", "Dracula (dark)", "theme-dracula"),
                MenuNode::entry("n", "Nightfox (dark)", "theme-nightfox"),
                MenuNode::entry("g", "Gruvbox (dark)", "theme-gruvbox-dark"),
                MenuNode::entry("l", "Solarized Light", "theme-solarized-light"),
                MenuNode::entry("L", "Solarized Dark", "theme-solarized-dark"),
                MenuNode::entry("f", "Financial Times", "theme-financial-times"),
                MenuNode::entry("F", "Financial Times Dark", "theme-financial-times-dark"),
                MenuNode::entry("o", "Folio", "theme-folio"),
            ],
        ),
        MenuNode::separator(),
        MenuNode::label("Rail"),
        MenuNode::entry("B", "file browser rail (Cmd-B)", "rail-files"),
        MenuNode::entry("O", "outline rail (Cmd-Shift-O)", "rail-outline"),
        MenuNode::entry("S", "flip rail side (Cmd-Shift-B)", "rail-flip"),
        MenuNode::separator(),
        MenuNode::submenu(
            "W",
            "window (splits/workspaces)",
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
                MenuNode::separator(),
                MenuNode::label("Workspaces"),
                MenuNode::entry("t", "new workspace (Cmd-T)", "new-tab"),
                MenuNode::entry("x", "close workspace (Cmd-Shift-W)", "close-tab"),
                MenuNode::entry("]", "next workspace (Ctrl-Tab)", "next-tab"),
                MenuNode::entry("[", "prev workspace (Ctrl-Shift-Tab)", "prev-tab"),
                MenuNode::entry("r", "rename workspace (Cmd-Shift-R)", "rename-tab"),
                MenuNode::entry("m", "move pane to workspace (Ctrl-W m)", "move-pane"),
                MenuNode::entry("M", "also-show pane in workspace (Ctrl-W M)", "also-show-pane"),
            ],
        ),
        MenuNode::separator(),
        MenuNode::entry("q", "quit", "quit"),
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
    /// the original owner (i.e. an `OwnerChanged{has_owner:false}` arrived),
    /// meaning take-over will now succeed. Drives the banner color. Purely a
    /// display hint — `candidate_take_over` re-checks authoritatively.
    candidate_promote_ready: bool,
    /// Splash screen shown at startup. `Some(deadline)` while visible;
    /// `None` after dismissal (auto-timeout or keypress).
    splash_until: Option<std::time::Instant>,
    /// Shared syntect highlighter for code block syntax coloring in Edit Mode
    /// and the agent transcript pane. Loaded once at startup.
    syntect_hl: sketch::highlight::Highlighter,
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
            edit_cache: None,
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
            });
            ws.next_tab_index += 1;
        }
        if !ws.tabs.is_empty() {
            ws.active_tab = snap.active_tab.min(ws.tabs.len() - 1);
        }
        if ws.tabs.is_empty() {
            return false;
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
    fn restore_agent_leaves(
        &mut self,
        leaf_ids: &[workspace::WindowId],
        cx: &mut Context<Self>,
    ) {
        let proc_cwd = process_cwd();

        for &leaf_id in leaf_ids {
            let mut ring = AgentRing::new(None);

            if self.session_server.is_some() {
                // Session-server path: placeholder + async attach.
                let placeholder = AgentState::new_server_managed(Some(
                    "connecting to session server…".into(),
                ));
                let open_token = alloc_open_token();
                ring.push(
                    "claude-1".into(),
                    placeholder,
                    None,
                    proc_cwd.clone(),
                    None,
                );
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
                    let state = self.create_agent_session(
                        None,
                        slot_cwd.clone(),
                        session_index,
                        cx,
                    );
                    ring.push("claude-1".into(), state, None, slot_cwd, None);
                } else {
                    let active_pos = persisted
                        .iter()
                        .position(|s| s.active)
                        .unwrap_or(0);
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
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Doc(d) => Some(d),
            _ => None,
        }
    }

    fn browser_mut(&mut self) -> Option<&mut BrowserWindow> {
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Browser(b) => Some(b),
            _ => None,
        }
    }

    fn agent_mut(&mut self) -> Option<&mut AgentState> {
        match self.workspace.focused_content_mut().expect("no focused window") {
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
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Agent(ring) => Some(ring),
            _ => None,
        }
    }

    /// Open `path` as a doc. If it's already in a tab, switch to that tab.
    /// Otherwise push a new tab containing the doc. Returns false on read error.
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

        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path.display(), e);
                return false;
            }
        };
        let label: SharedString = canon.into();
        let doc = Document::from_text(text, path.clone());
        let blocks = render_with_wiki(&doc.full_text(), &self.theme, Some(&path));
        let new_content = WindowContent::Doc(DocState {
            blocks,
            file_label: label,
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            edit_cache: None,
        });

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
        if let Some(d) = self.doc_mut() {
            if d.cursor_block + 1 < d.blocks.len() {
                d.cursor_block += 1;
                d.reveal_block(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn scroll_up(&mut self, _: &ScrollUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if d.cursor_block > 0 {
                d.cursor_block -= 1;
                d.reveal_block(d.cursor_block);
                cx.notify();
            }
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
        if let Some(d) = self.doc_mut() {
            if d.cursor_block + 1 < d.blocks.len() {
                d.cursor_block += 1;
                d.reveal_block(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn cursor_prev(&mut self, _: &CursorPrevBlock, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if d.cursor_block > 0 {
                d.cursor_block -= 1;
                d.reveal_block(d.cursor_block);
                cx.notify();
            }
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
        if let Some(d) = self.doc_mut() {
            if !d.blocks.is_empty() {
                d.cursor_block = d.blocks.len() - 1;
                d.reveal_block(d.cursor_block);
                cx.notify();
            }
        }
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
        if let Some(sel) = self.doc_selection.as_mut() {
            if sel.head != pos {
                sel.head = pos;
                cx.notify();
            }
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
            let l_start = if bi == start.block_idx { start.line_idx } else { 0 };
            let l_end = if bi == end.block_idx {
                end.line_idx
            } else {
                lines.len().saturating_sub(1)
            };
            for li in l_start..=l_end {
                let Some(line) = lines.get(li) else { continue };
                let line_text: String =
                    line.spans.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("");
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
    fn copy_selection(
        &mut self,
        _: &CopySelection,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Doc view: delegate to existing mouse-selection copy.
        if let Some(sel) = self.doc_selection {
            if let Some(text) = self.collect_doc_selection_text(&sel) {
                if !text.is_empty() {
                    Self::yank_to_clipboard(&text);
                    return;
                }
            }
        }
        // Edit / Agent views: copy editor selection.
        let text = match self.workspace.focused_content() {
            Some(WindowContent::Edit(e)) => e.editor.selection_text(),
            Some(WindowContent::Agent(ring)) => {
                let c = &ring.active().state;
                if c.input_surface.is_chatbox() {
                    c.input_surface.chatbox().and_then(|cb| cb.editor.selection_text())
                } else {
                    c.editor.selection_text()
                }
            }
            _ => None,
        };
        if let Some(t) = text {
            if !t.is_empty() {
                Self::yank_to_clipboard(&t);
            }
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
        self.workspace.push_initial_tab(WindowContent::Browser(BrowserWindow::standalone(
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
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W v` — vertical split: new pane to the right of the focused one.
    fn split_v(&mut self, _: &SplitV, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::V);
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
        let browser_fallback = || {
            WindowContent::Browser(BrowserWindow::standalone(cwd.to_path_buf()))
        };
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
            // Doc panes render a disk snapshot (read-only view); no shared
            // editor state to pool.
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => return browser_fallback(),
            };
            let doc = Document::from_text(text, path.clone());
            let blocks = render_with_wiki(&doc.full_text(), &self.theme, Some(&path));
            WindowContent::Doc(DocState {
                blocks,
                file_label: label,
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                edit_cache: None,
            })
        }
    }

    /// `Ctrl-W c` — close the focused window. If it was the only window in
    /// the tab, close the tab instead.
    fn close_window(&mut self, _: &CloseWindow, _w: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.close_focused() {
            Ok(Some(_new_focus)) => {
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

    // ---- Browser actions ----------------------------------------------------

    fn browser_down(&mut self, _: &BrowserDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if let Some(wm) = &mut b.fb.worktree_mode {
                wm.move_down();
            } else {
                b.fb.move_down();
            }
            cx.notify();
        }
    }
    fn browser_up(&mut self, _: &BrowserUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if let Some(wm) = &mut b.fb.worktree_mode {
                wm.move_up();
            } else {
                b.fb.move_up();
            }
            cx.notify();
        }
    }
    fn browser_enter(&mut self, _: &BrowserEnter, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                b.fb.select_worktree();
                cx.notify();
                return;
            }
        }
        let to_open = match self.browser_mut() {
            Some(b) => b.fb.enter_selected(),
            None => return,
        };
        if let Some(path) = to_open {
            self.open_file(path);
        }
        cx.notify();
    }
    fn browser_parent(&mut self, _: &BrowserParent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                return; // no-op in worktree mode
            }
            b.fb.go_parent();
            cx.notify();
        }
    }
    fn browser_worktrees(
        &mut self,
        _: &BrowserWorktrees,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                b.fb.exit_worktree_mode();
            } else {
                b.fb.enter_worktree_mode();
            }
            cx.notify();
        }
    }
    fn browser_toggle_hidden(
        &mut self,
        _: &BrowserToggleHidden,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            b.fb.toggle_hidden();
            cx.notify();
        }
    }
    fn browser_cycle_sort(
        &mut self,
        _: &BrowserCycleSort,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            b.fb.cycle_sort();
            cx.notify();
        }
    }
    fn browser_close(&mut self, _: &BrowserClose, _w: &mut Window, cx: &mut Context<Self>) {
        // If in worktree mode, Esc exits that overlay instead of closing.
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                b.fb.exit_worktree_mode();
                cx.notify();
                return;
            }
        }
        let underlying = match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Browser(b) => b.underlying.take(),
            _ => return,
        };
        // If the browser was opened over an existing pane (Cmd-O from a
        // Doc/Edit/Claude window), restore that prior content in place —
        // user pressed Esc/q to cancel the file pick.
        if let Some(boxed) = underlying {
            self.set_screen(*boxed);
            self.save_workspace_state();
            cx.notify();
            return;
        }
        // Standalone browser (new-tab open, persisted browser tab, split
        // fallback). Try to dismiss the pane:
        //   - one pane of a split → close just that pane.
        //   - sole pane in tab, multiple tabs → close the tab.
        //   - sole pane in sole tab → no-op. Esc/q is intentionally NOT a
        //     quit shortcut — too easy to lose the app by mashing keys.
        //     Quit lives on Cmd-Q.
        match self.workspace.close_focused() {
            Ok(Some(_)) => {
                self.save_workspace_state();
                cx.notify();
            }
            Ok(None) => {
                if self.workspace.tabs.len() > 1 {
                    let idx = self.workspace.active_tab;
                    self.workspace.close_tab(idx);
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            Err(()) => {}
        }
    }

    fn browser_filter(&mut self, _: &BrowserFilter, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if b.fb.filter_mode {
                b.fb.clear_filter();
            } else {
                b.fb.filter_mode = true;
                b.fb.set_filter("");
            }
            cx.notify();
        }
    }

    /// Key-down handler for browser filter text input.
    fn handle_browser_filter_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let Some(b) = self.browser_mut() else { return };
        if !b.fb.filter_mode {
            return;
        }
        match press.key {
            Key::Esc => {
                b.fb.clear_filter();
                cx.notify();
                cx.stop_propagation();
            }
            Key::Enter => {
                // Open the selected result and exit filter.
                let entries: Vec<_> = b.fb.visible_entries().iter().map(|e| e.path.clone()).collect();
                let selected = b.fb.selected();
                if let Some(path) = entries.get(selected).cloned() {
                    let is_dir = path.is_dir();
                    b.fb.clear_filter();
                    if is_dir {
                        b.fb.navigate_to(path);
                        cx.notify();
                    } else {
                        self.open_file(path);
                        cx.notify();
                    }
                } else {
                    b.fb.clear_filter();
                    cx.notify();
                }
                cx.stop_propagation();
            }
            Key::Backspace => {
                let mut text = b.fb.filter_text().to_string();
                if text.pop().is_some() {
                    b.fb.set_filter(&text);
                } else {
                    b.fb.clear_filter();
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Char(c) => {
                let mut text = b.fb.filter_text().to_string();
                text.push(c);
                b.fb.set_filter(&text);
                cx.notify();
                cx.stop_propagation();
            }
            Key::Down => {
                let count = b.fb.visible_entries().len();
                if count > 0 {
                    let sel = (b.fb.selected() + 1) % count;
                    b.fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Up => {
                let count = b.fb.visible_entries().len();
                if count > 0 {
                    let sel = if b.fb.selected() == 0 { count - 1 } else { b.fb.selected() - 1 };
                    b.fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    // ---- Rail (persistent side column, spec-rail.md) -----------------------

    /// `&mut` to the active tab's rail state, if a rail is open.
    fn rail_mut(&mut self) -> Option<&mut workspace::RailState> {
        self.workspace.active_tab_mut()?.rail.as_mut()
    }

    /// True when the active tab has a rail open AND it currently holds focus.
    fn rail_is_focused(&self) -> bool {
        self.workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .map(|r| r.focused)
            .unwrap_or(false)
    }

    /// Sync `rail.focused` after a focus-motion: the rail holds focus only
    /// when the newly focused leaf is the one the rail is pinned to.
    fn sync_rail_focus_after_motion(&mut self) {
        let Some(tab) = self.workspace.active_tab_mut() else { return };
        let Some(rail) = tab.rail.as_mut() else { return };
        rail.focused = tab.focused == rail.pinned_to;
    }

    /// Toggle the file-browser rail (Cmd-B). Two-state model (spec §5):
    /// - closed / different kind  → open-and-focus a file browser at cwd.
    /// - file-browser already open → close it, return focus to content.
    fn toggle_file_browser_rail(
        &mut self,
        _: &ToggleFileBrowserRail,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_file_browser_rail_impl(cx);
    }

    /// Toggle-logic for the file-browser rail, shared by the keybinding action
    /// and the command menu (`rail-files`).
    fn toggle_file_browser_rail_impl(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.active_tab_mut() else {
            return;
        };
        match &tab.rail {
            Some(r) if r.content.is_file_browser() => {
                tab.rail = None;
            }
            existing => {
                let side = existing.as_ref().map(|r| r.side).unwrap_or_default();
                let pinned_to = tab.focused;
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let content = workspace::RailContent::FileBrowser(FileBrowser::new(cwd));
                tab.rail = Some(workspace::RailState::new(content, side, pinned_to));
            }
        }
        self.save_workspace_state();
        cx.notify();
    }

    /// Toggle the outline rail (Cmd-Shift-O). Two-state model (spec §5). The
    /// heading list is derived lazily on render from the focused window.
    fn toggle_outline_rail(
        &mut self,
        _: &ToggleOutlineRail,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_outline_rail_impl(cx);
    }

    /// Toggle-logic for the outline rail, shared by the keybinding action and
    /// the command menu (`rail-outline`).
    fn toggle_outline_rail_impl(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.active_tab_mut() else {
            return;
        };
        match &tab.rail {
            Some(r) if r.content.is_outline() => {
                tab.rail = None;
            }
            existing => {
                let side = existing.as_ref().map(|r| r.side).unwrap_or_default();
                let pinned_to = tab.focused;
                let content = workspace::RailContent::Outline(workspace::OutlineState::new());
                tab.rail = Some(workspace::RailState::new(content, side, pinned_to));
            }
        }
        self.save_workspace_state();
        cx.notify();
    }

    /// Flip which edge the rail anchors to (Cmd-Shift-B). No-op when no rail
    /// is open. Persisted in the workspace snapshot.
    fn flip_rail_side(&mut self, _: &FlipRailSide, _w: &mut Window, cx: &mut Context<Self>) {
        self.flip_rail_side_impl(cx);
    }

    /// Flip-logic shared by the keybinding action and the command menu
    /// (`rail-flip`).
    fn flip_rail_side_impl(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            r.side = match r.side {
                workspace::RailSide::Left => workspace::RailSide::Right,
                workspace::RailSide::Right => workspace::RailSide::Left,
            };
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Close the rail and return focus to the previously-focused split-tree
    /// leaf (spec §7 — `tab.focused` is the single source of truth).
    fn rail_close(&mut self, _: &RailClose, _w: &mut Window, cx: &mut Context<Self>) {
        // If in worktree mode, Esc exits that overlay instead of closing the rail.
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                if fb.worktree_mode.is_some() {
                    fb.exit_worktree_mode();
                    cx.notify();
                    return;
                }
            }
        }
        if let Some(tab) = self.workspace.active_tab_mut() {
            if tab.rail.is_some() {
                tab.rail = None;
                self.save_workspace_state();
                cx.notify();
            }
        }
    }

    fn rail_down(&mut self, _: &RailDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => {
                    if let Some(wm) = &mut fb.worktree_mode {
                        wm.move_down();
                    } else {
                        fb.move_down();
                    }
                }
                workspace::RailContent::Outline(o) => {
                    if !o.entries.is_empty() {
                        o.selected = (o.selected + 1) % o.entries.len();
                    }
                }
            }
            cx.notify();
        }
    }

    fn rail_up(&mut self, _: &RailUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => {
                    if let Some(wm) = &mut fb.worktree_mode {
                        wm.move_up();
                    } else {
                        fb.move_up();
                    }
                }
                workspace::RailContent::Outline(o) => {
                    if !o.entries.is_empty() {
                        o.selected = if o.selected == 0 {
                            o.entries.len() - 1
                        } else {
                            o.selected - 1
                        };
                    }
                }
            }
            cx.notify();
        }
    }

    /// Enter the selected rail entry. File browser: open a file (rail stays
    /// open) or navigate into a directory. Outline: scroll the focused window
    /// to the heading's block/line.
    fn rail_select(&mut self, _: &RailSelect, _w: &mut Window, cx: &mut Context<Self>) {
        // Worktree mode: select worktree and navigate.
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                if fb.worktree_mode.is_some() {
                    fb.select_worktree();
                    cx.notify();
                    return;
                }
            }
        }
        // File browser: collect the action without holding the rail borrow.
        let to_open = match self.rail_mut() {
            Some(r) => match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => fb.enter_selected(),
                workspace::RailContent::Outline(_) => None,
            },
            None => return,
        };
        if let Some(path) = to_open {
            // Selecting a file opens it in the focused leaf; the rail stays
            // open but yields focus back to the content (spec §7, §12).
            // `open_file` replaces a transient Browser pane in place or
            // pushes a new tab otherwise.
            self.open_file(path);
            if let Some(r) = self.rail_mut() {
                r.focused = false;
            }
            cx.notify();
            return;
        }

        // Outline: jump the focused window to the selected heading.
        let target = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .and_then(|r| match &r.content {
                workspace::RailContent::Outline(o) => o.entries.get(o.selected).map(|(_, _, idx)| *idx),
                _ => None,
            });
        if let Some(idx) = target {
            match self.workspace.focused_content_mut() {
                Some(WindowContent::Doc(d)) => {
                    d.cursor_block = idx.min(d.blocks.len().saturating_sub(1));
                    d.reveal_block(d.cursor_block);
                }
                Some(WindowContent::Edit(e)) => {
                    let lines = e.editor.line_count();
                    let line = idx.min(lines.saturating_sub(1));
                    e.editor.set_cursor(line, 0);
                    // The Edit body is now a virtualized `gpui::list`; reveal
                    // through the ListState (the old ScrollHandle drove the
                    // pre-virtualization overflow container).
                    if line < e.list_item_count {
                        e.list_state.scroll_to_reveal_item(line);
                    }
                }
                _ => {}
            }
            cx.notify();
        }
    }

    fn rail_parent(&mut self, _: &RailParent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                if fb.worktree_mode.is_some() {
                    return; // no-op in worktree mode
                }
                fb.go_parent();
                cx.notify();
            }
        }
    }

    fn rail_worktrees(&mut self, _: &RailWorktrees, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                if fb.worktree_mode.is_some() {
                    fb.exit_worktree_mode();
                } else {
                    fb.enter_worktree_mode();
                }
                cx.notify();
            }
        }
    }

    fn rail_toggle_hidden(
        &mut self,
        _: &RailToggleHidden,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                fb.toggle_hidden();
                cx.notify();
            }
        }
    }

    fn rail_cycle_sort(&mut self, _: &RailCycleSort, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                fb.cycle_sort();
                cx.notify();
            }
        }
    }

    fn rail_filter(&mut self, _: &RailFilter, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                if fb.filter_mode {
                    fb.clear_filter();
                } else {
                    fb.filter_mode = true;
                    fb.set_filter("");
                }
                cx.notify();
            }
        }
    }

    /// Key-down handler for rail filter text input.
    fn handle_rail_filter_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let Some(r) = self.rail_mut() else { return };
        let fb = match &mut r.content {
            workspace::RailContent::FileBrowser(fb) => fb,
            _ => return,
        };
        if !fb.filter_mode {
            return;
        }
        match press.key {
            Key::Esc => {
                fb.clear_filter();
                cx.notify();
                cx.stop_propagation();
            }
            Key::Enter => {
                let entries: Vec<_> = fb.visible_entries().iter().map(|e| e.path.clone()).collect();
                let selected = fb.selected();
                if let Some(path) = entries.get(selected).cloned() {
                    let is_dir = path.is_dir();
                    fb.clear_filter();
                    if is_dir {
                        fb.navigate_to(path);
                        cx.notify();
                    } else {
                        self.open_file(path);
                        cx.notify();
                    }
                } else {
                    let Some(r) = self.rail_mut() else { return };
                    if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                        fb.clear_filter();
                    }
                    cx.notify();
                }
                cx.stop_propagation();
            }
            Key::Backspace => {
                let mut text = fb.filter_text().to_string();
                if text.pop().is_some() {
                    fb.set_filter(&text);
                } else {
                    fb.clear_filter();
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Char(c) => {
                let mut text = fb.filter_text().to_string();
                text.push(c);
                fb.set_filter(&text);
                cx.notify();
                cx.stop_propagation();
            }
            Key::Down => {
                let count = fb.visible_entries().len();
                if count > 0 {
                    let sel = (fb.selected() + 1) % count;
                    fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Up => {
                let count = fb.visible_entries().len();
                if count > 0 {
                    let sel = if fb.selected() == 0 { count - 1 } else { fb.selected() - 1 };
                    fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    // ---- Edit mode ---------------------------------------------------------

    /// `Some(edit)` if currently editing, else `None`.
    fn edit_mut(&mut self) -> Option<&mut EditState> {
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Edit(e) => Some(e),
            _ => None,
        }
    }

    /// Test-only: install a fresh Edit screen over `text` (Code view, Insert
    /// mode) so the headless harness can drive keystrokes through the real
    /// `build_edit_body_code` highlight path.
    #[cfg(test)]
    fn test_open_edit(&mut self, text: &str) {
        let core: workspace::SharedCore = std::rc::Rc::new(std::cell::RefCell::new(
            sketch::editor::EditorCore::new(text.to_string(), PathBuf::from("/tmp/harness.md")),
        ));
        let mut e = EditState::new(SharedEditor::new(1, core), "harness.md".into(), EditView::Code);
        e.mode = EditMode::Insert;
        // Skip the boot splash so render() builds the real Edit body, not the
        // splash screen — the harness needs the highlight path to actually run.
        self.splash_until = None;
        self.set_screen(WindowContent::Edit(e));
    }

    /// Test-only: `(last_recomputed, last_was_skip)` of the focused Edit view's
    /// incremental highlight cache — the O(changed) latency-gate observable.
    #[cfg(test)]
    fn test_edit_cache_stats(&mut self) -> (usize, bool) {
        let e = self.edit_mut().expect("focused window is not an Edit view");
        (e.highlight_cache.last_recomputed, e.highlight_cache.last_was_skip)
    }

    /// Test-only: install a fresh Doc screen rendering `blocks` so the headless
    /// harness can drive the real virtualized doc body. Skips the boot splash
    /// (otherwise `render()` builds the splash screen, not the doc list) and
    /// resets the per-frame block-build counter so the latency gate measures
    /// from a clean slate.
    #[cfg(test)]
    fn test_open_doc(&mut self, markdown: &str) {
        let blocks = render_with_wiki(markdown, &self.theme, None);
        self.set_screen(WindowContent::Doc(DocState {
            blocks,
            file_label: SharedString::new_static("harness.md"),
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            edit_cache: None,
        }));
        // The real doc body only renders once the splash deadline passes; clear
        // it so the harness exercises the list path immediately.
        self.splash_until = None;
        Self::test_reset_doc_block_builds();
    }

    /// Test-only: zero the virtualized-doc block-build counter.
    #[cfg(test)]
    fn test_reset_doc_block_builds() {
        DOC_BLOCK_BUILDS.with(|c| c.set(0));
    }

    /// Test-only: how many `block_element`s the doc list built since the last
    /// reset — the O(visible) latency-gate observable.
    #[cfg(test)]
    fn test_doc_block_builds() -> usize {
        DOC_BLOCK_BUILDS.with(|c| c.get())
    }

    /// Test-only: clear the doc render-decision tap (call before the frame to
    /// measure).
    #[cfg(test)]
    fn test_reset_doc_render_tap() {
        DOC_RENDER_TAP.with(|t| *t.borrow_mut() = DocRenderTap::default());
    }

    /// Test-only: snapshot the doc render-decision tap — what the last frame(s)
    /// since reset decided to paint / select / cursor-bar.
    #[cfg(test)]
    fn test_doc_render_tap() -> DocRenderTap {
        DOC_RENDER_TAP.with(|t| t.borrow().clone())
    }

    /// Swap from Doc view into Edit screen with the Code (raw markdown) view.
    fn enter_edit(&mut self, _: &EnterEdit, _w: &mut Window, cx: &mut Context<Self>) {
        self.enter_edit_with(EditView::Code, cx);
    }

    /// Swap from Doc view into Edit screen with the Word-Processor (live
    /// preview) view. Bound to `Ctrl-W` in the SketchView key context.
    fn enter_wp(&mut self, _: &EnterWp, _w: &mut Window, cx: &mut Context<Self>) {
        self.enter_edit_with(EditView::WordProcessor, cx);
    }

    /// Common entry point: restore the cached EditState if one exists (so
    /// unsaved edits survive the round-trip) or build a fresh editor from
    /// disk. The chosen `view` is applied either way — switching from Code
    /// → WP without losing cursor/buffer state is just `cached.view = view`.
    fn enter_edit_with(&mut self, view: EditView, cx: &mut Context<Self>) {
        // Take the cached EditState (preserving unsaved edits + cursor) without
        // holding a mutable borrow across the pool mutation below.
        let (cached, label): (Option<EditState>, SharedString) =
            match self.workspace.focused_content_mut() {
                Some(WindowContent::Doc(d)) => (d.edit_cache.take(), d.file_label.clone()),
                _ => return,
            };
        let mut edit_state = match cached {
            Some(cached) => cached,
            None => {
                // Bind a fresh view to the file's pooled core (shared text +
                // undo with any other window on the same file).
                let path: PathBuf = label.to_string().into();
                let (id, core) = match self.workspace.open_and_retain(&path) {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                EditState::new(SharedEditor::new(id, core), label, view)
            }
        };
        edit_state.view = view;
        self.set_screen(WindowContent::Edit(edit_state));
        cx.notify();
    }

    /// Edit → Doc round trip. Re-renders the buffer's *current* text (not the
    /// on-disk version), so unsaved edits show up in the rendered preview.
    /// Stashes the EditState on the new DocState so re-entering edit picks
    /// up exactly where the user left off (cursor, mode, scroll, undo).
    fn back_to_doc(&mut self, cx: &mut Context<Self>) {
        let prev = self
            .workspace
            .replace_focused_content(
            // Placeholder; overwritten in every match arm below.
            WindowContent::Doc(DocState {
                blocks: Vec::new(),
                file_label: SharedString::new_static(""),
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                edit_cache: None,
            }),
        )
            .expect("workspace has no focused window");
        match prev {
            WindowContent::Edit(edit) => {
                let edit_path = PathBuf::from(edit.file_label.as_ref());
                let blocks =
                    render_with_wiki(&edit.editor.full_text(), &self.theme, Some(&edit_path));
                let file_label = edit.file_label.clone();
                self.set_screen(WindowContent::Doc(DocState {
                    blocks,
                    file_label,
                    cursor_block: 0,
                    list_state: DocState::new_list_state(0),
                    list_item_count: std::cell::Cell::new(0),
                    blocks_seq: 0,
                    blocks_snapshot: RefCell::new(None),
                    last_cursor_block: std::cell::Cell::new(None),
                    edit_cache: Some(edit),
                }));
            }
            WindowContent::Agent(ring) => {
                // Restore whatever screen the user opened Claude from. If
                // none was stashed, fall back to a fresh Browser at cwd.
                // AgentRing and all its sessions drop here, taking pump
                // tasks and ACP channels with them.
                let new = match ring.underlying {
                    Some(boxed) => *boxed,
                    None => WindowContent::Browser(BrowserWindow::standalone(
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    )),
                };
                self.set_screen(new);
            }
            other => {
                self.set_screen(other);
                return;
            }
        }
        cx.notify();
    }

    /// Resolve a wiki link target (e.g. `notes`, `subdir/topic`) against
    /// the source doc's directory and replace the focused pane with the
    /// resulting Doc. Lookup order:
    ///   1. `<doc_dir>/<target>.md` — markdown convention; matches what
    ///      Obsidian / Foam / most wiki-aware editors do.
    ///   2. `<doc_dir>/<target>` — literal path, in case the user included
    ///      the extension already (or wants a non-md file).
    /// If neither exists, log to stderr and no-op (the pane stays put;
    /// nothing to navigate to).
    fn open_wiki_link(
        &mut self,
        target: &str,
        doc_dir: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) {
        let target = target.trim();
        if target.is_empty() {
            return;
        }
        let bases: Vec<PathBuf> = match doc_dir {
            Some(d) => vec![d.to_path_buf()],
            None => vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))],
        };
        let mut resolved: Option<PathBuf> = None;
        for base in &bases {
            let with_md = base.join(format!("{target}.md"));
            if with_md.is_file() {
                resolved = Some(with_md);
                break;
            }
            let bare = base.join(target);
            if bare.is_file() {
                resolved = Some(bare);
                break;
            }
        }
        let Some(path) = resolved else {
            eprintln!("wiki link: no file found for [[{}]]", target);
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("wiki link: cannot read {}: {}", path.display(), err);
                return;
            }
        };
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();
        let label: SharedString = canon.into();
        let doc = Document::from_text(text, path.clone());
        let blocks = render_with_wiki(&doc.full_text(), &self.theme, Some(&path));
        self.set_screen(WindowContent::Doc(DocState {
            blocks,
            file_label: label,
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            edit_cache: None,
        }));
        self.doc_selection = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Re-read the focused window's file from disk and rebuild its content,
    /// discarding any unsaved buffer state. Doc view: re-renders blocks and
    /// resets scroll/cursor (file may have shifted out from under the user).
    /// Edit view: replaces the Editor with a fresh one over the same path.
    /// Browser / Claude windows: no-op — there's no on-disk file to revert
    /// to. Read failures log to stderr (consistent with the existing open
    /// path) and leave the buffer untouched.
    fn reload_focused_from_disk(&mut self, cx: &mut Context<Self>) {
        // Extract the path (and, for Edit, the shared core handle) from the
        // focused window without holding a mutable borrow across file I/O +
        // workspace mutation.
        enum FocusKind {
            Doc(PathBuf, SharedString),
            Edit(workspace::SharedCore, PathBuf),
        }
        let focus_kind = match self.workspace.focused_content() {
            Some(WindowContent::Doc(d)) => FocusKind::Doc(
                PathBuf::from(d.file_label.as_ref()),
                d.file_label.clone(),
            ),
            Some(WindowContent::Edit(e)) => FocusKind::Edit(
                std::rc::Rc::clone(&e.editor.core),
                PathBuf::from(e.file_label.as_ref()),
            ),
            _ => return,
        };
        match focus_kind {
            // Edit reload resets the SHARED core in place, so every view of
            // the file (splits, also-shown panes) sees the disk version — not
            // a fresh, un-shared buffer. The pane keeps its own cursor/scroll
            // and Code/WP sub-view (we never replace the EditState itself).
            FocusKind::Edit(core, path) => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(err) => {
                        eprintln!("reload: cannot read {}: {}", path.display(), err);
                        return;
                    }
                };
                *core.borrow_mut() = EditorCore::new(text, path);
                // The text may have shrunk; reset the focused view's cursor to
                // the top so it can't dangle past the new end (matches the old
                // reload-replaces-editor behavior). Other shared views keep
                // their own cursors.
                if let Some(WindowContent::Edit(e)) = self.workspace.focused_content_mut() {
                    e.editor.set_cursor(0, 0);
                    e.editor.clear_selection();
                }
            }
            // Doc reload re-renders a fresh disk snapshot.
            FocusKind::Doc(path, label) => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(err) => {
                        eprintln!("reload: cannot read {}: {}", path.display(), err);
                        return;
                    }
                };
                let doc = Document::from_text(text, path.clone());
                let blocks = render_with_wiki(&doc.full_text(), &self.theme, Some(&path));
                self.set_screen(WindowContent::Doc(DocState {
                    blocks,
                    file_label: label,
                    cursor_block: 0,
                    list_state: DocState::new_list_state(0),
                    list_item_count: std::cell::Cell::new(0),
                    blocks_seq: 0,
                    blocks_snapshot: RefCell::new(None),
                    last_cursor_block: std::cell::Cell::new(None),
                    edit_cache: None,
                }));
            }
        }
        self.doc_selection = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Dispatch a key in Edit mode. Insert mode handles raw text input;
    /// Normal mode routes through the shared `KeybindManager` to map the
    /// keystroke to an action name, then this method dispatches a small
    /// subset of actions against the editor. `Ctrl-S` (save) and `Ctrl-V`
    /// (back to Doc view) are caught here before mode dispatch so they
    /// behave identically in both Insert and Normal.
    fn handle_edit_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Mode-independent shortcuts.
        if press.modifiers.contains(KMods::CONTROL) {
            if let Key::Char(c) = press.key {
                match c {
                    's' | 'S' => {
                        self.save_buffer(cx);
                        return;
                    }
                    'v' | 'V' => {
                        self.back_to_doc(cx);
                        return;
                    }
                    'w' | 'W' => {
                        self.toggle_edit_view(cx);
                        return;
                    }
                    _ => {}
                }
            }
        }

        let mode = match self.edit_mut() {
            Some(e) => e.mode,
            None => return,
        };

        // Tab/Shift-Tab in normal mode cycle buffers.
        if mode == EditMode::Normal {
            match press.key {
                Key::Tab => {
                    if self.workspace.tabs.len() > 1 {
                        let next = (self.workspace.active_tab + 1) % self.workspace.tabs.len();
                        self.switch_to_buffer(next);
                        cx.notify();
                    }
                    return;
                }
                Key::BackTab => {
                    if self.workspace.tabs.len() > 1 {
                        let prev = if self.workspace.active_tab == 0 {
                            self.workspace.tabs.len() - 1
                        } else {
                            self.workspace.active_tab - 1
                        };
                        self.switch_to_buffer(prev);
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        match mode {
            EditMode::Insert => self.dispatch_insert(press, cx),
            EditMode::Normal => self.dispatch_normal(press, cx),
        }
    }

    /// Flip between Code and WordProcessor views without touching buffer
    /// state. Bound to `Ctrl-W`.
    fn toggle_edit_view(&mut self, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        edit.view = match edit.view {
            EditView::Code => EditView::WordProcessor,
            EditView::WordProcessor => EditView::Code,
        };
        cx.notify();
    }

    /// Save the current edit buffer; record the outcome on `last_save_msg`
    /// so the footer can surface it. No-op if the screen isn't Edit.
    fn save_buffer(&mut self, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        let msg: SharedString = match edit.editor.save() {
            Ok(()) => "saved".into(),
            Err(e) => format!("save failed: {}", e).into(),
        };
        edit.last_save_msg = Some(msg);
        cx.notify();
    }

    fn dispatch_insert(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        // Any non-save key invalidates the transient save message.
        edit.last_save_msg = None;
        Self::dispatch_insert_core(&mut edit.editor, &mut edit.mode, press);
        cx.notify();
    }

    /// Insert-mode dispatch on raw `(editor, mode)` references — shared by
    /// the Edit screen and the Claude (ACP) screen so both have the same
    /// typing semantics. Unlike the wrapper above, this does not call
    /// `cx.notify()` — the caller must.
    fn dispatch_insert_core<E: EditOps>(editor: &mut E, mode: &mut EditMode, press: KeyPress) {
        match press.key {
            Key::Esc => {
                editor.end_insert();
                *mode = EditMode::Normal;
                // Vim convention: cursor steps back one column on leaving insert.
                if editor.cursor().col > 0 {
                    editor.cursor_move_left();
                }
            }
            Key::Enter => {
                editor.insert_char('\n');
            }
            Key::Backspace => {
                editor.backspace();
            }
            Key::Tab => {
                editor.insert_char(' ');
                editor.insert_char(' ');
            }
            Key::Char(c) => {
                if press.modifiers.contains(KMods::CONTROL) {
                    // Ignore ctrl-chords in insert mode for the MVP; only
                    // bare typed chars produce text.
                    return;
                }
                editor.insert_char(c);
            }
            _ => {}
        }
    }

    fn dispatch_normal(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        edit.last_save_msg = None;
        match Self::dispatch_normal_core(
            &mut edit.editor,
            &mut edit.mode,
            &mut edit.keybinds,
            press,
        ) {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                edit.last_save_msg = Some("yanked".into());
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
        }
    }

    /// Normal-mode dispatch on raw `(editor, mode, keybinds)` references —
    /// shared by the Edit screen and the Claude (ACP) screen. Caller is
    /// responsible for `cx.notify()` and any post-action status messaging
    /// based on the returned `NormalOutcome`.
    fn dispatch_normal_core<E: EditOps>(
        editor: &mut E,
        mode: &mut EditMode,
        keybinds: &mut KeybindManager,
        press: KeyPress,
    ) -> NormalOutcome {
        // Esc clears any active selection and exits extend mode.
        if press.key == Key::Esc {
            editor.set_extend_mode(false);
            editor.clear_selection();
            return NormalOutcome::Handled;
        }

        let action_name = match keybinds.process_key(press) {
            Some(name) => name,
            None => return NormalOutcome::Skipped,
        };

        match action_name.as_str() {
            // ---- Pure motions: collapse selection (or extend in extend mode) ----
            "move-down" => {
                editor.pre_move(false);
                editor.move_down(false);
            }
            "move-up" => {
                editor.pre_move(false);
                editor.cursor_move_up();
                editor.clamp_cursor_col(false);
            }
            "move-left" => {
                editor.pre_move(false);
                editor.cursor_move_left();
            }
            "move-right" => {
                editor.pre_move(false);
                editor.move_right_clamped(false);
            }
            "move-line-start" => {
                editor.pre_move(false);
                editor.cursor_move_line_start();
            }
            "move-line-end" => {
                editor.pre_move(false);
                editor.move_cursor_line_end(false);
            }
            // ---- Word motions: create a fresh selection from cursor → motion target ----
            "move-word-forward" => {
                editor.pre_move(true);
                editor.move_cursor_word_forward();
            }
            "move-word-backward" => {
                editor.pre_move(true);
                editor.move_cursor_word_backward();
            }
            "move-word-end" => {
                editor.pre_move(true);
                editor.move_cursor_word_end();
            }
            // ---- Doc-level jumps ----
            "goto-top" => {
                editor.pre_move(false);
                editor.cursor_jump_top();
            }
            "goto-bottom" => {
                editor.pre_move(false);
                editor.jump_cursor_bottom();
            }
            // ---- Mode switches ----
            "insert-mode" => {
                if let Some(((sl, sc), _)) = editor.selection_range() {
                    editor.cursor_set(sl, sc);
                    editor.clear_selection();
                }
                editor.set_extend_mode(false);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "insert-after" => {
                if let Some((_, (el, ec))) = editor.selection_range() {
                    let line_len = editor.line_len_chars(el);
                    let new_col = if ec < line_len { ec + 1 } else { ec };
                    editor.cursor_set(el, new_col);
                    editor.clear_selection();
                } else {
                    editor.move_right_clamped(true);
                }
                editor.set_extend_mode(false);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "open-line-below" => {
                editor.open_line_below();
                *mode = EditMode::Insert;
            }
            "open-line-above" => {
                editor.open_line_above();
                *mode = EditMode::Insert;
            }
            // ---- Helix selection actions ----
            "delete-selection" => {
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
            }
            "change-selection" => {
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "yank-selection" => {
                let text = match editor.yank_selection() {
                    Some(t) if !t.is_empty() => t,
                    _ => editor
                        .line_text_at_cursor()
                        .trim_end_matches('\n')
                        .to_string(),
                };
                Self::yank_to_clipboard(&text);
                return NormalOutcome::Yanked;
            }
            "collapse-selection" => editor.collapse_selection(),
            "flip-selection" => editor.flip_selection(),
            "select-all" => editor.select_all(),
            "extend-line" => editor.extend_by_line(),
            "toggle-extend-mode" => {
                editor.toggle_extend_mode();
                if editor.extend_mode() && editor.selection_anchor().is_none() {
                    editor.anchor_at_cursor();
                }
            }
            // ---- Direct-edit actions (still callable via custom config) ----
            "delete-char" => {
                editor.delete_char_at_cursor();
            }
            "delete-line" => {
                editor.delete_current_line();
            }
            "undo" => {
                editor.undo();
            }
            "redo" => {
                editor.redo();
            }
            "quit" | "force-quit" => return NormalOutcome::Quit,
            "open-menu" => return NormalOutcome::OpenMenu,
            _ => return NormalOutcome::Skipped,
        }
        NormalOutcome::Handled
    }

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

    fn menu_ref(&self) -> Option<&MenuOverlay> {
        if let ActiveOverlay::Menu(m) = &self.active_overlay { Some(m) } else { None }
    }
    /// Hands back the WHOLE `&mut MenuOverlay` (never a per-field accessor) so
    /// `m.state.process_key(press, &m.menu)`'s disjoint two-field split-borrow
    /// keeps type-checking.
    fn menu_mut(&mut self) -> Option<&mut MenuOverlay> {
        if let ActiveOverlay::Menu(m) = &mut self.active_overlay { Some(m) } else { None }
    }
    fn buffer_ref(&self) -> Option<&BufferSwitcher> {
        if let ActiveOverlay::BufferSwitcher(b) = &self.active_overlay { Some(b) } else { None }
    }
    fn buffer_mut(&mut self) -> Option<&mut BufferSwitcher> {
        if let ActiveOverlay::BufferSwitcher(b) = &mut self.active_overlay { Some(b) } else { None }
    }
    fn session_ref(&self) -> Option<&SessionSwitcher> {
        if let ActiveOverlay::SessionSwitcher(s) = &self.active_overlay { Some(s) } else { None }
    }
    fn session_mut(&mut self) -> Option<&mut SessionSwitcher> {
        if let ActiveOverlay::SessionSwitcher(s) = &mut self.active_overlay { Some(s) } else { None }
    }
    fn workspace_picker_ref(&self) -> Option<&WorkspacePicker> {
        if let ActiveOverlay::WorkspacePicker(p) = &self.active_overlay { Some(p) } else { None }
    }
    fn workspace_picker_mut(&mut self) -> Option<&mut WorkspacePicker> {
        if let ActiveOverlay::WorkspacePicker(p) = &mut self.active_overlay { Some(p) } else { None }
    }
    fn rename_ref(&self) -> Option<&RenameOverlay> {
        if let ActiveOverlay::Rename(o) = &self.active_overlay { Some(o) } else { None }
    }
    fn rename_mut(&mut self) -> Option<&mut RenameOverlay> {
        if let ActiveOverlay::Rename(o) = &mut self.active_overlay { Some(o) } else { None }
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
        // Opening the menu dismisses any lingering toast.
        self.transient_status = None;
        let mut state = MenuState::new();
        state.open();
        self.open_overlay(ActiveOverlay::Menu(MenuOverlay {
            state,
            menu: gpui_menu(),
        }));
        cx.notify();
    }

    /// Menu's key handler. Esc pops a level (or closes from root). Any
    /// other key is offered to `MenuState::process_key`; if it resolves
    /// to a command, the menu closes and `dispatch_menu_command` runs it.
    fn handle_menu_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            Some(m) => m.state.process_key(press, &m.menu),
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
                if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Agent(_)) {
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
            "dev-take-over" => self.candidate_take_over(cx),
            "rail-files" => self.toggle_file_browser_rail_impl(cx),
            "rail-outline" => self.toggle_outline_rail_impl(cx),
            "rail-flip" => self.flip_rail_side_impl(cx),
            "compose-toggle" | "agent-input-toggle" => {
                if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Agent(_)) {
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
                self.workspace.push_initial_tab(WindowContent::Browser(
                    BrowserWindow::standalone(
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    ),
                ));
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
                    if let Some(l) = screen_file_label(&w.content) {
                        if l.as_ref() == label {
                            found = true;
                        }
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
    fn open_workspace_picker(
        &mut self,
        mode: WorkspacePickerMode,
        cx: &mut Context<Self>,
    ) {
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
        self.open_overlay(ActiveOverlay::WorkspacePicker(WorkspacePicker { mode, selected }));
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
                if let Some(p) = self.workspace_picker_mut() {
                    if count > 0 {
                        p.selected = (p.selected + 1) % count;
                    }
                }
            }
            Key::Char('k') | Key::Up => {
                if let Some(p) = self.workspace_picker_mut() {
                    if count > 0 {
                        p.selected = if p.selected == 0 { count - 1 } else { p.selected - 1 };
                    }
                }
            }
            Key::Char('g') => {
                if let Some(p) = self.workspace_picker_mut() {
                    p.selected = 0;
                }
            }
            Key::Char('G') => {
                if let Some(p) = self.workspace_picker_mut() {
                    if count > 0 {
                        p.selected = count - 1;
                    }
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
                    if r.is_empty() { return; }
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
                if let Some(ss) = self.session_mut() {
                    if count > 0 {
                        ss.selected = (ss.selected + 1) % count;
                    }
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if let Some(ss) = self.session_mut() {
                    if count > 0 {
                        ss.selected = if ss.selected == 0 {
                            count - 1
                        } else {
                            ss.selected - 1
                        };
                    }
                }
            }
            Key::Char('g') => {
                if let Some(ss) = self.session_mut() {
                    ss.selected = 0;
                }
            }
            Key::Char('G') => {
                let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
                if let Some(ss) = self.session_mut() {
                    if count > 0 {
                        ss.selected = count - 1;
                    }
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
                    if let Some(ss) = self.session_mut() {
                        if ss.selected >= new_count {
                            ss.selected = new_count - 1;
                        }
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

            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_0p5();

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

            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_0p5();
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
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_0p5();
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
                let server_sid = self
                    .agent_ring_mut()
                    .and_then(|ring| {
                        let slot = ring.slot_by_index_mut(index)?;
                        slot.label = new_label.clone();
                        slot.server_session_id.clone()
                    });
                if let (Some(server), Some(sid)) =
                    (&self.session_server, server_sid)
                {
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
            RenameTarget::AgentChangeCwd { index } => {
                match resolve_agent_cwd_arg(&new_label) {
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
                }
            }
        }
    }

    fn handle_rename_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    } else if !filtered.is_empty() {
                        if let Some(bs) = self.buffer_mut() {
                            bs.filter_mode = false;
                        }
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
                if let Some(bs) = self.buffer_mut() {
                    if count > 0 {
                        bs.selected = (bs.selected + 1) % count;
                    }
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = self.buffer_mut() {
                    if count > 0 {
                        bs.selected = if bs.selected == 0 {
                            count - 1
                        } else {
                            bs.selected - 1
                        };
                    }
                }
            }
            Key::Char('g') => {
                if let Some(bs) = self.buffer_mut() {
                    bs.selected = 0;
                }
            }
            Key::Char('G') => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = self.buffer_mut() {
                    if count > 0 {
                        bs.selected = count - 1;
                    }
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
                    if let Some(bs) = self.buffer_mut() {
                        if bs.selected >= count && count > 0 {
                            bs.selected = count - 1;
                        }
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

    /// Build the menu popup as an absolutely-positioned overlay anchored
    /// to the top of the window. Renders header (breadcrumb), entry list,
    /// and a footer hint. Has *no* key handlers — the wrapper in
    /// `Render::render` handles input via `capture_key_down` so the
    /// underlying screen never sees keystrokes while the menu is open.
    /// Render the active tab's layout tree. Leaves dispatch to per-kind
    /// render methods; splits become flex containers (row for V splits,
    /// col for H splits) with weighted children.
    fn render_focused_window(
        &mut self,
        root: gpui::Div,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_idx = self.workspace.active_tab;
        let focused_id = self.workspace.tabs[tab_idx].focused;
        // Re-derive the outline rail (if any) once before rendering the tree,
        // so the focused leaf can render it inline without a second pass.
        self.refresh_outline_rail();
        let layout_ptr: *mut workspace::Layout<WindowContent> =
            &mut self.workspace.tabs[tab_idx].layout as *mut _;
        // SAFETY: `layout_ptr` is valid for as long as the active tab's
        // `layout` field isn't structurally mutated (no splits/closes/etc.).
        // The render pipeline only reads self's other fields (theme/fonts)
        // and the layout subtree via this pointer; structural mutations
        // happen in action handlers, never inside render. This sidesteps a
        // Rust borrowck limitation where the compiler can't prove that
        // &mut Layout<WindowContent> (a field inside self.workspace.tabs)
        // is disjoint from &self.render_X's other field accesses.
        let layout = unsafe { &mut *layout_ptr };
        self.render_layout(root, layout, focused_id, attach_focus, rail_focusable, cx)
    }

    /// Recursively render a `Layout<WindowContent>`. The `root` div is used
    /// only for the leaf case (so leaves can attach focus + key bindings);
    /// split branches build their own container.
    ///
    /// `attach_focus` is true when no overlay is open — in that case the
    /// focused leaf attaches `track_focus(&self.focus_handle)` so the focus
    /// handle sits inside that leaf's key context. When an overlay is open,
    /// focus belongs on the overlay wrapper and no leaf attaches it.
    fn render_layout(
        &mut self,
        root: gpui::Div,
        layout: &mut workspace::Layout<WindowContent>,
        focused_id: workspace::WindowId,
        attach_focus: bool,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            workspace::Layout::Empty => div().size_full().into_any_element(),
            workspace::Layout::Leaf(window) => {
                let is_focused = window.id == focused_id;
                let content_ptr: *mut WindowContent =
                    &mut window.content as *mut _;
                // SAFETY: same as in render_focused_window — the leaf's
                // content sits inside a layout tree we won't structurally
                // mutate during this render call.
                let content = unsafe { &mut *content_ptr };
                let leaf_root = if is_focused && attach_focus {
                    root.track_focus(&self.focus_handle)
                } else {
                    root
                };
                let painted: AnyElement = match content {
                    WindowContent::Doc(d) => self.render_doc(leaf_root, d, cx).into_any_element(),
                    WindowContent::Edit(e) => self.render_edit(leaf_root, e, cx).into_any_element(),
                    WindowContent::Browser(b) => self.render_browser(leaf_root, b, cx).into_any_element(),
                    WindowContent::Agent(ring) => {
                        self.render_agent(leaf_root, ring, cx).into_any_element()
                    }
                };
                // Pin the rail to the leaf it was opened from, not whichever
                // leaf currently has focus. Falls back to the focused leaf
                // when no pinned_to is set (single-pane case).
                let is_rail_pinned = self
                    .workspace
                    .active_tab()
                    .and_then(|t| t.rail.as_ref())
                    .map(|r| r.pinned_to == window.id)
                    .unwrap_or(false);
                let with_rail = if is_rail_pinned {
                    self.wrap_leaf_with_rail(painted, rail_focusable, cx)
                } else {
                    painted
                };
                // Focus indicator: thick border around the whole pane+rail
                // group when there's more than one leaf, plus a small "focused"
                // tag in the upper-right corner.
                if is_focused && self.active_tab_leaf_count() > 1 {
                    let accent: Hsla = rgb(STATUS_FG).into();
                    let tag = div()
                        .absolute()
                        .top_1()
                        .right_1()
                        .px_1p5()
                        .py_0p5()
                        .bg(accent)
                        .text_color(rgb(BG))
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .rounded_sm()
                        .child("focused");
                    div()
                        .size_full()
                        .relative()
                        .border_2()
                        .border_color(accent)
                        .child(with_rail)
                        .child(tag)
                        .into_any_element()
                } else {
                    with_rail
                }
            }
            workspace::Layout::Split { dir, children } => {
                // The `root` div carries `track_focus(&self.focus_handle)`
                // when no overlay is open, so we must include it in the
                // tree. Without it the focus handle isn't attached to any
                // rendered element and global key bindings (e.g. Space →
                // OpenMenu) have nowhere to dispatch. Wrap the split's
                // flex container inside `root` rather than discarding it.
                let mut container = div().size_full().flex().min_w_0().min_h_0();
                container = match dir {
                    workspace::SplitDir::V => container.flex_row(),
                    workspace::SplitDir::H => container.flex_col(),
                };
                let editor_bg = self.editor_bg();
                let editor_fg = self.editor_fg();
                for (weight, child) in children.iter_mut() {
                    let w = *weight;
                    let child_root = div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(editor_bg)
                        .text_color(editor_fg);
                    let child_el =
                        self.render_layout(child_root, child, focused_id, attach_focus, rail_focusable, cx);
                    let mut slot = div().min_w_0().min_h_0().overflow_hidden();
                    {
                        let style = slot.style();
                        style.flex_grow = Some(w);
                        style.flex_shrink = Some(1.0);
                        style.flex_basis = Some(gpui::relative(0.0).into());
                    }
                    slot = slot.child(child_el);
                    container = container.child(slot);
                }
                root.child(container).into_any_element()
            }
        }
    }

    /// How many leaves does the active tab's layout contain?
    fn active_tab_leaf_count(&self) -> usize {
        self.workspace
            .active_tab()
            .map(|t| t.layout.leaf_count())
            .unwrap_or(0)
    }

    /// If the workspace has more than one tab, stack a thin horizontal tab
    /// strip above the screen view. Single-tab workspaces render the screen
    /// alone (no strip).
    fn wrap_with_tab_strip(
        &self,
        screen_view: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.workspace.tabs.len() <= 1 {
            return screen_view;
        }

        let active_idx = self.workspace.active_tab;
        // Pull chrome colors from the active theme so the strip matches the
        // light/dark palette. Active tab inverts to editor_bg (the doc body
        // colour) so the focused tab visually connects to the work area.
        let top_bar = self.theme.top_bar;
        let active_fg: Hsla = fg_or(top_bar, STATUS_FG);
        let inactive_fg: Hsla = rgb(0x6272a4).into();
        let strip_bg: Hsla = bg_or(top_bar, STATUS_BG);
        let active_bg: Hsla = self.editor_bg();

        // Vertical sidebar on the left, fixed-width. Flex default for
        // align-items is stretch, which is what we want — entries fill the
        // strip width and truncate via overflow_hidden.
        let mut strip = div()
            .flex()
            .flex_col()
            .px_1()
            .py_2()
            .w(px(160.0))
            .min_w(px(160.0))
            .bg(strip_bg)
            .text_size(px(12.0))
            .font_family(self.body_font.clone())
            .gap_1();

        for (i, tab) in self.workspace.tabs.iter().enumerate() {
            let label = tab_strip_label(tab);
            let is_active = i == active_idx;
            let fg = if is_active { active_fg } else { inactive_fg };
            let bg = if is_active { active_bg } else { strip_bg };

            let entry = div()
                .id(("tab-strip-entry", i))
                .w_full()
                .px_2()
                .py_1()
                .rounded(px(3.0))
                .bg(bg)
                .text_color(fg)
                .overflow_hidden()
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, ev: &MouseDownEvent, _w, cx| {
                        // Double-click on a tab entry opens the rename
                        // overlay for that tab. Single-click just
                        // switches to it.
                        if ev.click_count >= 2 {
                            view.workspace.active_tab = i;
                            view.open_rename_active_tab_overlay(cx);
                        } else {
                            view.select_tab(i, cx);
                        }
                    }),
                );
            strip = strip.child(entry);
        }

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(strip)
            .child(div().flex_1().min_w_0().min_h_0().child(screen_view))
            .into_any_element()
    }

    /// Inject the active tab's rail beside the **focused leaf's** content
    /// (spec-rail.md §8, adjusted: the rail is chrome local to the focused
    /// pane, not the whole window — so in a split it sits against the focused
    /// content, not at the window edge). `content_el` is the already-rendered
    /// focused-leaf element. No-op passthrough when no rail is open.
    /// `rail_focusable` is false when an overlay owns focus — the rail still
    /// renders as background but is not focusable (constraint §4).
    ///
    /// The outline entries are re-derived once per frame in
    /// `render_focused_window` before this runs (spec §13).
    fn wrap_leaf_with_rail(
        &mut self,
        content_el: AnyElement,
        rail_focusable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .workspace
            .active_tab()
            .map(|t| t.rail.is_none())
            .unwrap_or(true)
        {
            return content_el;
        }

        let (side, focused) = {
            let r = self
                .workspace
                .active_tab()
                .and_then(|t| t.rail.as_ref())
                .expect("rail present");
            (r.side, r.focused && rail_focusable)
        };

        let rail = self.render_rail(focused, cx);

        let content_slot = div().flex_1().min_w_0().min_h_0().child(content_el);

        let row = div().size_full().flex().flex_row().min_w_0().min_h_0();
        let row = match side {
            workspace::RailSide::Left => row.child(rail).child(content_slot),
            workspace::RailSide::Right => row.child(content_slot).child(rail),
        };
        row.into_any_element()
    }

    /// Re-derive the outline rail's heading entries from the focused window
    /// (spec §13). No-op when the rail is closed or showing the file browser.
    /// Change-key for the outline: focused window id + that window's content
    /// version. Re-deriving the outline is O(document) (an Edit pane allocates
    /// the whole rope via `full_text()` and scans every line), and the render
    /// loop runs every frame — including every keystroke. Keying on this lets
    /// `refresh_outline_rail` skip the work when nothing relevant changed.
    fn outline_change_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(tab) = self.workspace.active_tab() {
            tab.focused.hash(&mut h); // focus change → re-derive
        }
        match self.workspace.focused_content() {
            // Edit: edit_seq is the exact monotonic content version.
            Some(WindowContent::Edit(e)) => e.editor.edit_seq().hash(&mut h),
            // Doc: blocks only change on load/reload/edit-flush; block count is
            // a cheap proxy (outline is cosmetic, so a same-count content change
            // leaving it briefly stale is acceptable).
            Some(WindowContent::Doc(d)) => d.blocks.len().hash(&mut h),
            // Agent/Browser have no outline; constant so it derives once (empty).
            _ => 0u64.hash(&mut h),
        }
        h.finish()
    }

    fn refresh_outline_rail(&mut self) {
        let is_outline = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .map(|r| r.content.is_outline())
            .unwrap_or(false);
        if !is_outline {
            return;
        }
        // Skip the O(document) re-derivation when neither the focused window nor
        // its content changed since the last derive (the common case — cursor
        // blink, scroll, cross-pane notify, and unrelated keystrokes).
        let key = self.outline_change_key();
        let unchanged = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .and_then(|r| match &r.content {
                workspace::RailContent::Outline(o) => o.last_key,
                _ => None,
            })
            == Some(key);
        if unchanged {
            return;
        }
        let entries = self.derive_outline();
        if let Some(r) = self.rail_mut() {
            if let workspace::RailContent::Outline(o) = &mut r.content {
                o.entries = entries;
                o.last_key = Some(key);
                if o.selected >= o.entries.len() {
                    o.selected = o.entries.len().saturating_sub(1);
                }
            }
        }
    }

    /// Build `(depth, text, block_index_or_line)` heading entries from the
    /// focused window's content (spec §13).
    fn derive_outline(&self) -> Vec<(u8, String, usize)> {
        match self.workspace.focused_content() {
            Some(WindowContent::Doc(d)) => {
                let mut out = Vec::new();
                for (idx, block) in d.blocks.iter().enumerate() {
                    if let RenderedBlock::Heading { level, content } = block {
                        out.push((*level, styled_line_plain(content), idx));
                    }
                }
                out
            }
            Some(WindowContent::Edit(e)) => {
                let text = e.editor.full_text();
                let mut out = Vec::new();
                for (line_no, line) in text.lines().enumerate() {
                    if let Some((level, heading)) = atx_heading(line) {
                        out.push((level, heading, line_no));
                    }
                }
                out
            }
            // Agent / Browser have no outline.
            _ => Vec::new(),
        }
    }

    /// Render the rail column for the active tab (spec §9, §11–§13). Chrome
    /// styling — text is fixed at 12px and does NOT scale with `text_scale`.
    fn render_rail(&self, focused: bool, cx: &mut Context<Self>) -> gpui::Div {
        let rail = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .expect("rail present");

        let top_bar = self.theme.top_bar;
        let rail_bg: Hsla = bg_or(top_bar, STATUS_BG);
        // Unselected entry text: use the brighter overlay *foreground* rather
        // than the dim `overlay.label` token — the label color reads too
        // low-contrast against the rail background. `overlay.fg` is the same
        // high-contrast body color the command menu uses for its entries.
        let label_fg: Hsla = nc(self.theme.overlay.fg);
        // Placeholder text ("(empty)", "(no outline)") stays intentionally dim.
        let muted_fg: Hsla = nc(self.theme.overlay.label);
        let accent_fg: Hsla = nc(self.theme.overlay.accent);
        let selected_bg: Hsla = self.editor_bg();
        let selected_fg: Hsla = rgb(STATUS_FG).into();
        let border_color: Hsla = rgb(0x6272a4).into();
        let side = rail.side;
        let width = rail.width_px;

        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_w(px(width))
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(rail_bg)
            .text_size(px(12.0))
            .font_family(self.body_font.clone());

        // Content-facing border (right when Left, left when Right).
        col = match side {
            workspace::RailSide::Left => col.border_r_1().border_color(border_color),
            workspace::RailSide::Right => col.border_l_1().border_color(border_color),
        };

        // When focused, attach the focus handle inside the RailView key
        // context so its context-scoped bindings (j/k/enter/…) match.
        let mut col = col.key_context("RailView");
        if focused {
            col = col.track_focus(&self.focus_handle);
        }
        col = col
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_rail_filter_key(ev, w, cx);
            }))
            .on_action(cx.listener(Self::rail_down))
            .on_action(cx.listener(Self::rail_up))
            .on_action(cx.listener(Self::rail_select))
            .on_action(cx.listener(Self::rail_close))
            .on_action(cx.listener(Self::rail_parent))
            .on_action(cx.listener(Self::rail_toggle_hidden))
            .on_action(cx.listener(Self::rail_cycle_sort))
            .on_action(cx.listener(Self::rail_worktrees))
            .on_action(cx.listener(Self::rail_filter))
            // Global actions forwarded so they keep working while the rail is
            // focused (same pattern as every other screen root).
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            // Pane focus motion — without these the ctrl-w h/j/k/l chords
            // are swallowed when the rail holds `track_focus`.
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev));

        match &rail.content {
            workspace::RailContent::FileBrowser(fb) => {
                if let Some(wm) = &fb.worktree_mode {
                    // ── Worktree picker overlay ──────────────────────
                    let header = div()
                        .px_2()
                        .py_1()
                        .flex_none()
                        .text_color(accent_fg)
                        .font_weight(FontWeight::BOLD)
                        .overflow_hidden()
                        .child(SharedString::new_static("WORKTREES"));

                    let mut list = div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden();

                    if wm.worktrees.is_empty() {
                        list = list.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(muted_fg)
                                .child(SharedString::new_static("  (no worktrees)")),
                        );
                    } else {
                        let visible_rows = 40usize;
                        let scroll = scroll_to_keep_visible(
                            wm.selected,
                            visible_rows,
                            wm.worktrees.len(),
                        );
                        for (i, wt) in wm
                            .worktrees
                            .iter()
                            .enumerate()
                            .skip(scroll)
                            .take(visible_rows)
                        {
                            let is_sel = i == wm.selected;
                            let marker = if wt.is_current {
                                "* "
                            } else if is_sel {
                                "▸ "
                            } else {
                                "  "
                            };
                            let label = format!("{}{}", marker, wt.label);
                            let (rbg, rfg) = if is_sel {
                                (selected_bg, selected_fg)
                            } else {
                                (rail_bg, accent_fg)
                            };
                            list = list.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .py_0p5()
                                    .bg(rbg)
                                    .text_color(rfg)
                                    .overflow_hidden()
                                    .child(SharedString::from(label)),
                            );
                        }
                    }
                    col.child(header).child(list)
                } else {
                    // ── Normal file browser ──────────────────────────
                    let dir_str = fb.current_dir().display().to_string();
                    let header_text = if fb.filter_mode {
                        format!("/{}", fb.filter_text())
                    } else {
                        format!("▸ {}", dir_str)
                    };
                    let header = div()
                        .px_2()
                        .py_1()
                        .flex_none()
                        .text_color(accent_fg)
                        .font_weight(FontWeight::BOLD)
                        .overflow_hidden()
                        .child(SharedString::from(header_text));

                    let mut list = div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden();

                    let entries = fb.visible_entries();
                    let selected = fb.selected();
                    if entries.is_empty() {
                        let msg = if fb.filter_mode {
                            "  (no matches)"
                        } else {
                            "  (empty)"
                        };
                        list = list.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_color(muted_fg)
                                .child(SharedString::new_static(msg)),
                        );
                    } else {
                        let visible_rows = 40usize;
                        let scroll =
                            scroll_to_keep_visible(selected, visible_rows, entries.len());
                        for (i, entry) in
                            entries.iter().enumerate().skip(scroll).take(visible_rows)
                        {
                            let is_sel = i == selected;
                            let suffix = if entry.is_dir { "/" } else { "" };
                            let name = format!("{}{}", entry.name, suffix);
                            let (rbg, rfg) = if is_sel {
                                (selected_bg, selected_fg)
                            } else if entry.is_dir {
                                (rail_bg, accent_fg)
                            } else {
                                (rail_bg, label_fg)
                            };
                            list = list.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .py_0p5()
                                    .bg(rbg)
                                    .text_color(rfg)
                                    .overflow_hidden()
                                    .child(SharedString::from(name)),
                            );
                        }
                    }
                    col.child(header).child(list)
                }
            }
            workspace::RailContent::Outline(o) => {
                let header = div()
                    .px_2()
                    .py_1()
                    .flex_none()
                    .text_color(accent_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("OUTLINE"));

                let mut list = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden();

                if o.entries.is_empty() {
                    list = list.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(muted_fg)
                            .child(SharedString::new_static("(no outline)")),
                    );
                } else {
                    let visible_rows = 40usize;
                    let scroll =
                        scroll_to_keep_visible(o.selected, visible_rows, o.entries.len());
                    for (i, (level, text, _)) in
                        o.entries.iter().enumerate().skip(scroll).take(visible_rows)
                    {
                        let is_sel = i == o.selected;
                        // Indent by heading depth; depth-1 headings are
                        // section headers (accent + bold).
                        let indent = "  ".repeat((*level as usize).saturating_sub(1));
                        let label_text = format!("{}{}", indent, text);
                        let mut row = div()
                            .w_full()
                            .px_2()
                            .py_0p5()
                            .overflow_hidden();
                        if is_sel {
                            row = row.bg(selected_bg).text_color(selected_fg);
                        } else if *level == 1 {
                            row = row.text_color(accent_fg).font_weight(FontWeight::BOLD);
                        } else {
                            row = row.text_color(label_fg);
                        }
                        list = list.child(row.child(SharedString::from(label_text)));
                    }
                }
                col.child(header).child(list)
            }
        }
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
            .child(format!("MENU — {}", breadcrumb.to_uppercase()));

        let mut entries_col = div()
            .flex()
            .flex_col()
            .px_4()
            .py_2()
            .text_color(label_text_fg)
            .text_size(px(14.0))
            .font_family(self.body_font.clone());

        for node in nodes {
            let row: AnyElement = match node.kind() {
                MenuNodeKind::Separator => div()
                    .h(px(8.0))
                    .border_b_1()
                    .border_color(popup_border)
                    .my_1()
                    .into_any_element(),
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
                    let label_color = if node.kind() == MenuNodeKind::Submenu {
                        submenu_fg
                    } else {
                        label_text_fg
                    };
                    div()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .py_0p5()
                        .child(
                            div()
                                .min_w(px(48.0))
                                .text_color(key_fg)
                                .font_weight(FontWeight::BOLD)
                                .child(key_display),
                        )
                        .child(div().text_color(label_color).child(trailing))
                        .into_any_element()
                }
            };
            entries_col = entries_col.child(row);
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
            .child(SharedString::new_static(
                "press a key · Esc back / close",
            ));

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

            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_0p5();

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
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
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

    /// Open the Claude screen and attempt to attach to an ACP agent. Bound
    /// to `Ctrl-K` in the Doc and Edit views. Stashes the prior screen so
    /// `Ctrl-V` from Claude returns to it.
    ///
    /// Attach uses `SKETCH_ACP_AGENT` if set, else the
    /// `claude-agent-acp` default (`AcpChannelClient::DEFAULT_AGENT_COMMAND`).
    fn open_agent(&mut self, _: &OpenAgent, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_agent_inner(cx);
    }

    fn open_agent_inner(&mut self, cx: &mut Context<Self>) {
        // If already on Claude screen, just add a new session to the ring.
        if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Agent(_)) {
            self.new_agent_session(None, cx);
            return;
        }

        // Stash the current screen so back_to_doc can restore it.
        let prior = self
            .workspace
            .replace_focused_content(WindowContent::Doc(DocState {
                blocks: Vec::new(),
                file_label: SharedString::new_static(""),
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                edit_cache: None,
            }))
            .expect("workspace has no focused window");

        let mut ring = AgentRing::new(Some(Box::new(prior)));
        let proc_cwd = process_cwd();

        if self.session_server.is_some() {
            // ── Session-server path (S4: non-blocking) ───────────────
            // Render IMMEDIATELY in a "connecting…" placeholder, then do the
            // (potentially slow) list_sessions / attach / create round-trips
            // on a background thread. The server pump replays each session's
            // full event_log on attach, so the transcript lands through the
            // pump — we never have to block the paint thread on an Ack. The
            // worst case the old synchronous path could hit was a ~30s freeze
            // (request `recv_timeout`) when the server stalled.
            let placeholder = AgentState::new_server_managed(Some(
                "connecting to session server…".into(),
            ));
            let open_token = alloc_open_token();
            ring.push(
                "claude-1".into(),
                placeholder,
                None,
                proc_cwd.clone(),
                None,
            );
            // Start the unified server pump (one per view, routes by
            // session_id) and stash it on the placeholder so it lives as long
            // as the ring does — events for the soon-to-be-attached sessions
            // need it running before the attach Ack returns.
            let server_pump = self.start_server_pump(cx);
            if let Some(slot) = ring.slots.first_mut() {
                slot.state._pump = Some(server_pump);
                slot.pending_open_token = Some(open_token);
            }

            self.set_screen(WindowContent::Agent(ring));
            if let Some(c) = self.agent_mut() {
                c.editor.begin_insert();
            }
            cx.notify();

            self.spawn_open_agent_server(open_token, proc_cwd, cx);
            return;
        } else {
            // ── Direct-spawn path (legacy) ───────────────────────────
            let persisted = load_persisted_acp_sessions(&proc_cwd);

            if persisted.is_empty() {
                let slot_cwd = proc_cwd.clone();
                let session_index = ring.next_index;
                let state = self.create_agent_session(
                    None,
                    slot_cwd.clone(),
                    session_index,
                    cx,
                );
                ring.push("claude-1".into(), state, None, slot_cwd, None);
            } else {
                let active_pos = persisted
                    .iter()
                    .position(|s| s.active)
                    .unwrap_or(0);
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
        }

        self.set_screen(WindowContent::Agent(ring));

        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Background half of `open_agent_inner`'s session-server path (S4). Runs
    /// `list_sessions` and the resulting `attach`/`create_session` round-trips
    /// off the paint thread, then splices the real slot(s) into the
    /// placeholder ring via `this.update`. `placeholder_index` identifies the
    /// "connecting…" slot to fill in place (it owns the pump task, so we
    /// mutate it rather than replace it). If the window/ring is gone by the
    /// time the result lands (weak entity dropped, screen switched), every
    /// `this.update` is a no-op and the work is harmlessly discarded.
    fn spawn_open_agent_server(
        &self,
        open_token: u64,
        proc_cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        // Snapshot the server sids already open in any panel so the background
        // thread can dedup without touching `self`. Taken now, while we're
        // still on the (single-threaded) UI thread, so it can't race a
        // concurrent ring mutation. (Attach — and thus the Owner/Observer mode
        // choice — is deferred to `spawn_attach_sessions` after the bind.)
        let mut open_sids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tab in self.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |w| {
                if let WindowContent::Agent(ring) = &w.content {
                    for slot in ring.slots.iter() {
                        if let Some(sid) = &slot.server_session_id {
                            open_sids.insert(sid.clone());
                        }
                    }
                }
            });
        }

        cx.spawn(async move |this, cx| {
            let cwd = proc_cwd.clone();
            let resolution = cx
                .background_executor()
                .spawn(async move {
                    // 1. Discover existing sessions for this cwd that aren't
                    //    already shown elsewhere.
                    let existing = match handle.list_sessions() {
                        Ok(v) => v,
                        Err(e) => return OpenResolution::Failed(format!("list failed: {e}")),
                    };
                    let cwd_key = cwd_match_key(&cwd);
                    let matching: Vec<SessionInfo> = existing
                        .into_iter()
                        .filter(|s| cwd_match_key(&s.cwd) == cwd_key)
                        .filter(|s| !open_sids.contains(&s.session_id))
                        .collect();

                    if matching.is_empty() {
                        // 2a. None — create a fresh session. The server
                        //     registers it and returns the sid immediately
                        //     (ACP subprocess spawns server-side). NOTE: we do
                        //     NOT attach here. Attaching starts the server's
                        //     event replay, and the slot's `server_session_id`
                        //     isn't bound until `apply_open_agent_resolution`
                        //     runs on the foreground — attaching first races
                        //     that bind and the pump drops the replay (the
                        //     "resumed session is wonky/empty" bug). Attach is
                        //     deferred to after the bind; see `spawn_attach_sessions`.
                        match handle.create_session(cwd, "claude-1".to_string(), None) {
                            Ok(info) => OpenResolution::Created {
                                sid: info.session_id,
                                acp_id: info.acp_session_id,
                            },
                            Err(e) => OpenResolution::Failed(format!("create failed: {e}")),
                        }
                    } else {
                        // 2b. Resume each matching session — bind first, attach
                        //     later. Same rationale as 2a: deferring the attach
                        //     until the slot is bound closes the replay-drop
                        //     race. Owner reclaim + status come from the
                        //     deferred `spawn_attach_sessions`.
                        let attached: Vec<AttachedSlot> = matching
                            .iter()
                            .enumerate()
                            .map(|(i, info)| AttachedSlot {
                                label: if matching.len() == 1 {
                                    "claude-1".to_string()
                                } else {
                                    format!("claude-{}", i + 1)
                                },
                                sid: info.session_id.clone(),
                                acp_id: info.acp_session_id.clone(),
                                status: if info.connected {
                                    "reconnecting…".to_string()
                                } else {
                                    "reconnecting (agent spawning…)".to_string()
                                },
                            })
                            .collect();
                        OpenResolution::Attached(attached)
                    }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.apply_open_agent_resolution(open_token, resolution, cx);
            });
        })
        .detach();
    }

    /// Apply the result of the background `open_agent` round-trips: fill the
    /// "connecting…" placeholder slot (preserving its pump) and, for the
    /// re-attach case, push any additional slots. A no-op if the placeholder
    /// is gone (screen switched / slot closed before the result returned).
    fn apply_open_agent_resolution(
        &mut self,
        open_token: u64,
        resolution: OpenResolution,
        cx: &mut Context<Self>,
    ) {
        // Bind back to the exact placeholder that started this open, searching
        // the WHOLE workspace (not just the focused ring) and matching the
        // globally-unique `open_token` (not the per-ring `index`, which
        // collides at 0 across rings — the cause of `pump: no slot for server
        // session`). If the placeholder is gone (screen closed before the
        // round-trip returned), this is a harmless no-op.
        // Sids whose slot we actually bound in this pass. Collected inside the
        // ring closure (which only runs if the placeholder still exists) so we
        // attach EXACTLY the sessions now routable — attaching a sid whose slot
        // is gone would resurrect the replay-drop race we are fixing.
        let bound_sids: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
        let bound_sids_c = bound_sids.clone();
        self.with_open_token_ring(open_token, move |ring| {
            let Some(pos) = ring
                .slots
                .iter()
                .position(|s| s.pending_open_token == Some(open_token))
            else {
                return;
            };
            let proc_cwd = ring.slots[pos].cwd.clone();
            // Consume the token regardless of outcome so a late duplicate
            // resolution can't re-bind this slot.
            ring.slots[pos].pending_open_token = None;

            match resolution {
                OpenResolution::Failed(msg) => {
                    let m = format!("session server error — {msg}");
                    Self::append_system_notice(&mut ring.slots[pos].state, &m);
                    ring.slots[pos].state.status = Some(m.into());
                }
                OpenResolution::Created { sid, acp_id } => {
                    let slot = &mut ring.slots[pos];
                    slot.server_session_id = Some(sid.clone());
                    slot.resume_id = acp_id;
                    slot.state.status =
                        Some("attaching to ACP agent via session server…".into());
                    bound_sids_c.borrow_mut().push(sid);
                }
                OpenResolution::Attached(attached) => {
                    let mut iter = attached.into_iter();
                    // First attached session fills the placeholder in place.
                    if let Some(first) = iter.next() {
                        let slot = &mut ring.slots[pos];
                        slot.label = first.label;
                        slot.server_session_id = Some(first.sid.clone());
                        slot.resume_id = first.acp_id;
                        slot.state.status = Some(first.status.into());
                        bound_sids_c.borrow_mut().push(first.sid);
                    }
                    // Remaining sessions get their own slots in the same ring.
                    for a in iter {
                        let state = AgentState::new_server_managed(Some(a.status.into()));
                        ring.push(a.label, state, a.acp_id, proc_cwd.clone(), Some(a.sid.clone()));
                        bound_sids_c.borrow_mut().push(a.sid);
                    }
                    // Land the user on the placeholder slot, not the last push.
                    ring.active = pos;
                }
            }
        });
        self.save_agent_ring();
        cx.notify();

        // Now that the slots carry their `server_session_id`, attach (which
        // starts the server's event replay). Routing can no longer drop the
        // replay because every target is already bound. Deferred off the paint
        // thread; surfaces ownership/attach failures into the slot status.
        let targets = std::rc::Rc::try_unwrap(bound_sids)
            .map(|c| c.into_inner())
            .unwrap_or_default();
        if !targets.is_empty() {
            self.spawn_attach_sessions(targets, cx);
        }
    }

    /// Attach (with Owner-reclaim retry) to sessions whose slots were just
    /// bound by `apply_open_agent_resolution`, off the paint thread. Attaching
    /// here — AFTER the bind — is what closes the replay-drop race: the pump
    /// can route every replayed notification because its slot already exists.
    /// Per-session ownership outcome is reconciled back into the slot status so
    /// a read-only / failed attach is visible instead of a silently-dead session.
    fn spawn_attach_sessions(&self, sids: Vec<String>, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        let want_owner = !self.is_candidate;
        cx.spawn(async move |this, cx| {
            let results: Vec<(String, Result<bool, String>)> = cx
                .background_executor()
                .spawn(async move {
                    sids.into_iter()
                        .map(|sid| {
                            let r = attach_with_owner_retry(&handle, &sid, want_owner);
                            (sid, r)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (sid, r) in results {
                    let status: Option<SharedString> = match r {
                        // Owner (or observer-by-design): leave the optimistic
                        // "reconnecting…"/"attaching…" status to be overwritten
                        // by the first real event / SessionAttached notice.
                        Ok(true) => None,
                        Ok(false) if want_owner => {
                            Some("read-only — another window owns this session".into())
                        }
                        Ok(false) => None,
                        Err(e) => {
                            eprintln!(
                                "[sketch-gpui] attach failed for {}: {e}",
                                &sid[..sid.len().min(8)]
                            );
                            Some("attach failed — session may be unavailable".into())
                        }
                    };
                    if let Some(s) = status {
                        this.for_each_server_session_slot(&sid, |slot| {
                            slot.state.status = Some(s.clone());
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run `f` on the agent ring holding the placeholder slot stamped with
    /// `token` (see `AgentSlot::pending_open_token`), searching every tab and
    /// pane. Returns whether a match was found. Lets an async server
    /// open/create bind back to its originating slot regardless of which
    /// window happens to be focused when the round-trip returns.
    fn with_open_token_ring(&mut self, token: u64, f: impl FnOnce(&mut AgentRing)) -> bool {
        let mut f = Some(f);
        for tab in self.workspace.tabs.iter_mut() {
            let found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    if ring.slots.iter().any(|s| s.pending_open_token == Some(token)) {
                        if let Some(f) = f.take() {
                            f(ring);
                        }
                        return Some(());
                    }
                }
                None
            });
            if found.is_some() {
                return true;
            }
        }
        false
    }

    /// Create a new session and add it to the existing ring. With `cwd =
    /// None`, the new slot inherits the process cwd (today's behavior). With
    /// `cwd = Some(path)`, that already-resolved absolute path becomes the
    /// new slot's cwd — the caller (typically the `:claude-new <path>`
    /// command handler) is responsible for running the input through
    /// `resolve_agent_cwd_arg` first.
    fn new_agent_session(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        let (label, session_index) = match self.agent_ring() {
            Some(r) => (format!("claude-{}", r.next_index + 1), r.next_index),
            None => {
                // Not on the Agent screen yet — bootstrap it (which creates
                // a fresh session as part of setup), then we're done.
                self.open_agent_inner(cx);
                return;
            }
        };
        let slot_cwd = cwd.unwrap_or_else(process_cwd);

        if self.session_server.is_some() {
            // Session-server path (S4: non-blocking). Push a "connecting…"
            // placeholder immediately and create the session off-thread; the
            // sid is spliced in when the round-trip returns.
            let placeholder =
                AgentState::new_server_managed(Some("connecting to session server…".into()));
            let open_token = alloc_open_token();
            let ring = self.agent_ring_mut().unwrap();
            ring.push(label.clone(), placeholder, None, slot_cwd.clone(), None);
            if let Some(slot) = ring.slots.last_mut() {
                slot.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(open_token, label, slot_cwd, cx);
        } else {
            // Direct-spawn path.
            let state = self.create_agent_session(
                None,
                slot_cwd.clone(),
                session_index,
                cx,
            );
            let ring = self.agent_ring_mut().unwrap();
            ring.push(label, state, None, slot_cwd, None);
        }
        // §18 soft cap: at 6+ slots, surface a one-shot footer warning so
        // the user notices the per-slot ~100MB subprocess cost. Advisory
        // only — no enforcement.
        let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
            if count >= 6 {
                c.status = Some(
                    format!("{count} sessions active — each uses ~100MB").into(),
                );
            }
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Background half of `new_agent_session`'s session-server path (S4).
    /// Issues the `create_session` + `attach` round-trips off the paint thread
    /// and fills the "connecting…" placeholder (by `placeholder_index`) when
    /// they return. No-op if the placeholder is gone by then.
    fn spawn_create_agent_session(
        &self,
        open_token: u64,
        label: String,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let resolution = cx
                .background_executor()
                .spawn(async move {
                    // Create only — attach is deferred to
                    // `apply_open_agent_resolution` (after the slot binds its
                    // `server_session_id`) so the bind-before-attach ordering
                    // is uniform across the open and new-session paths.
                    match handle.create_session(cwd, label, None) {
                        Ok(info) => OpenResolution::Created {
                            sid: info.session_id,
                            acp_id: info.acp_session_id,
                        },
                        Err(e) => OpenResolution::Failed(format!("create failed: {e}")),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_open_agent_resolution(open_token, resolution, cx);
            });
        })
        .detach();
    }

    /// Respawn the slot identified by `slot_index` (monotonic
    /// `AgentSlot::index`) at a new working directory. Implements
    /// spec-agent-cwd.md §4 step-by-step: drop the current channel
    /// (kills subprocess), null out attach/awaiting state, swap the
    /// slot's `cwd`, drop `resume_id`, append a session-divider line
    /// to the transcript, and spawn a fresh channel. The transcript
    /// is otherwise preserved so the user can scroll back through
    /// the prior session's history above the divider.
    fn change_agent_cwd(
        &mut self,
        slot_index: usize,
        new_cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        // Resolve slot position once; the index is monotonic so it
        // doesn't shift unless the slot was closed.
        let pos = match self.agent_ring().and_then(|r| r.slot_by_index(slot_index)) {
            Some(p) => p,
            None => return,
        };

        // Phase 1: tear down the existing channel + attach state. The
        // borrow ends before we cross-call create_agent_session.
        let prev_cwd = {
            let ring = self.agent_ring_mut().unwrap();
            let slot = &mut ring.slots[pos];
            let prev = slot.cwd.clone();
            // Dropping `channel` kills the subprocess via kill_on_drop.
            slot.state.channel = None;
            slot.state.attach_pending = None;
            slot.state.turn_phase = TurnPhase::Idle;
            let msg = format!(
                "changing cwd to {}…",
                shorten_cwd_for_display(&new_cwd),
            );
            Self::append_system_notice(&mut slot.state, &msg);
            slot.state.status = Some(msg.into());
            slot.cwd = new_cwd.clone();
            // The agent-side session was bound to the old cwd; a fresh
            // session/new is the right resume strategy.
            slot.resume_id = None;
            prev
        };

        // Phase 2: build a fresh agent session at the new cwd.
        if self.session_server.is_some() {
            // Server path (S4: non-blocking): take the old sid, fire its close
            // off-thread, mark the slot "connecting…", and create the new
            // session off-thread. `spawn_create_agent_session` splices the new
            // sid into this slot (by its monotonic `slot_index`) when ready.
            let old_sid = {
                if let Some(ring) = self.agent_ring_mut() {
                    ring.slots.get_mut(pos).and_then(|s| s.server_session_id.take())
                } else {
                    None
                }
            };
            if let Some(old_sid) = old_sid {
                self.spawn_close_session(old_sid, cx);
            }
            let open_token = alloc_open_token();
            if let Some(ring) = self.agent_ring_mut() {
                if let Some(slot) = ring.slots.get_mut(pos) {
                    slot.state.attach_pending = None;
                    slot.state.channel = None;
                    slot.pending_open_token = Some(open_token);
                    let msg = format!(
                        "cwd → {}, connecting to fresh session…",
                        shorten_cwd_for_display(&new_cwd),
                    );
                    Self::append_system_notice(&mut slot.state, &msg);
                    slot.state.status = Some(msg.into());
                }
            }
            self.spawn_create_agent_session(
                open_token,
                "respawned".to_string(),
                new_cwd.clone(),
                cx,
            );
        } else {
            // Direct-spawn path: graft a throwaway AgentState's
            // channel + pump into the existing slot.
            let fresh = self.create_agent_session(
                None,
                new_cwd.clone(),
                slot_index,
                cx,
            );
            if let Some(ring) = self.agent_ring_mut() {
                if let Some(slot) = ring.slots.get_mut(pos) {
                    slot.state.attach_pending = fresh.attach_pending;
                    slot.state._pump = fresh._pump;
                    let msg = format!(
                        "cwd → {}, fresh session",
                        shorten_cwd_for_display(&new_cwd),
                    );
                    Self::append_system_notice(&mut slot.state, &msg);
                    slot.state.status = Some(msg.into());
                }
            }
        }

        let _ = prev_cwd;
        self.save_agent_ring();
        cx.notify();
    }

    /// Switch to the next (+1) or previous (-1) session in the ring.
    fn switch_agent_session(&mut self, direction: i32, cx: &mut Context<Self>) {
        if let Some(ring) = self.agent_ring_mut() {
            if direction > 0 {
                ring.next();
            } else {
                ring.prev();
            }
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Close the active session. If the ring is now empty, exit Claude.
    fn close_active_agent_session(&mut self, cx: &mut Context<Self>) {
        // For server sessions: drop the slot locally NOW (optimistic) and fire
        // the close round-trip off the paint thread (S4). `close_session`
        // parks on a 30s `recv_timeout`, so doing it synchronously froze the
        // window when the server stalled. The server broadcasts `SessionClosed`
        // on success, which `reconcile_session_closed` already folds into every
        // panel — so the worst case of an off-thread close that ends up not
        // landing is a stale entry that the next open's dedup/reconnect path
        // cleans up, not a frozen UI.
        let server_sid = self
            .agent_ring()
            .filter(|r| !r.is_empty())
            .and_then(|r| r.active().server_session_id.clone());

        if let Some(sid) = server_sid {
            self.spawn_close_session(sid, cx);
        }

        let is_empty = {
            let ring = match self.agent_ring_mut() {
                Some(r) => r,
                None => return,
            };
            let _dropped = ring.close_active(); // AgentSlot drops → pump task cancelled
            ring.is_empty()
        };
        if is_empty {
            // Last slot closed: wipe the cwd entry so reboot doesn't
            // resurrect anything, then drop the Claude screen.
            if let Ok(cwd) = std::env::current_dir() {
                forget_persisted_acp_sessions(&cwd);
            }
            self.back_to_doc(cx);
        } else {
            self.save_agent_ring();
            cx.notify();
        }
    }

    /// Fire a `close_session` off the paint thread (S4). The local slot has
    /// already been dropped (optimistic close); this just tells the server to
    /// tear down its session. On a logical error (we're an observer, or it's
    /// already gone) it best-effort detaches. Errors are logged, not surfaced —
    /// the slot is gone and the `SessionClosed` broadcast reconciles the rest.
    fn spawn_close_session(&self, sid: String, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                match handle.close_session(&sid) {
                    Ok(()) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        eprintln!(
                            "[sketch-gpui] close_session({}) failed (connection): {e}",
                            &sid[..sid.len().min(8)],
                        );
                    }
                    Err(_) => {
                        // Logical error — detach best-effort so the server drops
                        // our subscription even though the session lives on.
                        let _ = handle.detach(&sid);
                    }
                }
            })
            .detach();
    }

    /// Snapshot the current ring to disk. Called after every ring mutation
    /// (new/close/switch) and from the pump after a slot's attach resolves.
    /// Best-effort: any failure to write is silently ignored.
    fn save_agent_ring(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        // Save agent rings from ALL panes, not just the focused one.
        if let Some(tab) = self.workspace.active_tab() {
            tab.layout.for_each_leaf(&mut |window| {
                if let WindowContent::Agent(ring) = &window.content {
                    save_persisted_acp_sessions(&cwd, ring);
                }
            });
        }
    }

    /// Build a `AgentState` with ACP attach thread and pump task. The
    /// returned state is ready to be pushed into a `AgentRing`. `cwd` is
    /// the per-session working directory (spec-agent-cwd.md §3) — both the
    /// `NewSessionRequest` payload and the OS-level subprocess cwd come
    /// from this single argument. `session_index` is the monotonic
    /// `AgentSlot::index` the pump task will use to find this slot every
    /// tick; callers MUST pass the value that `AgentRing::push` will (or
    /// did) assign to this slot. Passing the wrong value silently strands
    /// the slot's attach (the pump drains some other slot's
    /// `attach_pending` and this slot's channel stays `None` forever).
    fn create_agent_session(
        &mut self,
        resume_id: Option<String>,
        cwd: PathBuf,
        session_index: usize,
        cx: &mut Context<Self>,
    ) -> AgentState {
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let spawn_cwd = Some(cwd);
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    spawn_cwd,
                    resume_id,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        let editor = Editor::new(String::new(), PathBuf::from("*claude*"));

        let pump = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;
            let idle_delay = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let min_cycle = Duration::from_millis(16);
            // Local throttle for the thinking-indicator animation: while a
            // turn is in flight we re-render at ~8fps even without events so
            // the elapsed/quiet timers stay live through a stall. Kept local
            // so the idle path doesn't grab the model lock every 16ms.
            let anim_period = Duration::from_millis(120);
            let mut last_anim = std::time::Instant::now();
            let mut wake_rx: Option<futures::channel::mpsc::UnboundedReceiver<()>> =
                None;
            loop {
                let cycle_start = std::time::Instant::now();
                if wake_rx.is_some() {
                    let mut rx = wake_rx.take().unwrap();
                    let timer = cx.background_executor().timer(idle_delay);
                    futures::select_biased! {
                        _ = rx.next().fuse() => {}
                        _ = timer.fuse() => {}
                    }
                    while rx.next().now_or_never().flatten().is_some() {}
                    wake_rx = Some(rx);
                } else {
                    cx.background_executor().timer(idle_delay).await;
                    let _ = this.update(cx, |this, _cx| {
                        if let Some(ring) = this.agent_ring_mut() {
                            if let Some(slot) = ring.slot_by_index_mut(session_index) {
                                if let Some(ch) = &slot.state.channel {
                                    wake_rx = ch.take_wake_receiver();
                                }
                            }
                        }
                    });
                }
                loop {
                    let t_apply = perf_enabled().then(std::time::Instant::now);
                    let more = match this.update(cx, |this, cx| {
                        this.pump_session(session_index, cx)
                    }) {
                        Ok(more) => more,
                        Err(_) => return,
                    };
                    if let Some(t) = t_apply {
                        eprintln!(
                            "[perf] acp-pump drain+apply lock_held={:.2}ms more={more}",
                            t.elapsed().as_secs_f64() * 1e3,
                        );
                    }
                    if !more {
                        break;
                    }
                    cx.background_executor().timer(yield_delay).await;
                }
                // Animation heartbeat: keep the thinking timer ticking even
                // when no events arrived this cycle.
                if last_anim.elapsed() >= anim_period {
                    last_anim = std::time::Instant::now();
                    let _ = this.update(cx, |this, cx| {
                        if this.any_agent_awaiting() {
                            cx.notify();
                        }
                    });
                }
                let elapsed = cycle_start.elapsed();
                if elapsed < min_cycle {
                    cx.background_executor()
                        .timer(min_cycle - elapsed)
                        .await;
                }
            }
        });

        let state = AgentState {
            editor,
            channel: None,
            attach_pending: Some(attach_rx),
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(
                0,
                gpui::ListAlignment::Bottom,
                gpui::px(256.0),
            ),
            list_item_count: 0,
            status: Some("attaching to ACP agent…".into()),
            turn_phase: TurnPhase::Idle,
            replay_turns: sketch::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tool_calls: std::collections::HashMap::new(),
            tool_call_order: Vec::new(),
            tool_call_anchor_line: std::collections::HashMap::new(),
            expanded_tool_calls: std::collections::HashSet::new(),
            block_ranges: Vec::new(),
            block_cache: std::collections::HashMap::new(),
            block_cache_frozen_count: 0,
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            flat_items_cache: std::rc::Rc::new(Vec::new()),
            gutter_cache: std::rc::Rc::new(Vec::new()),
            view_model_fp: None,
            view_model_seq: 0,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: false,
            reconciler: sketch::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: Some(pump),
        };
        setup_list_follow_handler(&state.list_state, &state.follow_output);
        state
    }


    /// Re-establish the session-server connection after a drop, then
    /// re-subscribe every live slot. Returns the fresh notification + wake
    /// receivers so the pump can splice them in and keep running; returns
    /// `None` when the reconnect itself failed (server still down — the pump
    /// retries on its backoff).
    ///
    /// Each slot's transcript is reset before re-attach: the server replays
    /// the full `event_log` on attach, so resetting lets that replay rebuild
    /// the on-screen transcript cleanly instead of duplicating it.
    fn reconnect_session_server(
        &mut self,
    ) -> Option<(
        std::sync::mpsc::Receiver<ServerNotification>,
        futures::channel::mpsc::UnboundedReceiver<()>,
    )> {
        let (note_rx, wake_rx) = match self.session_server.as_mut()?.reconnect() {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("[sketch-gpui] session-server reconnect failed: {e}");
                return None;
            }
        };

        // Reset every server-backed slot's transcript and collect the sids to
        // re-attach. (Borrow of `session_server` above has ended.)
        let mut sids: Vec<String> = Vec::new();
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    for slot in ring.slots.iter_mut() {
                        if let Some(sid) = slot.server_session_id.clone() {
                            slot.state.reset_for_replay();
                            Self::append_system_notice(&mut slot.state, "reconnecting…");
                            slot.state.status = Some("reconnecting…".into());
                            sids.push(sid);
                        }
                    }
                }
            });
        }

        let mode = if self.is_candidate {
            AttachMode::Observer
        } else {
            AttachMode::Owner
        };
        if let Some(server) = self.session_server.as_ref() {
            for sid in &sids {
                if let Err(e) = server.attach(sid, mode) {
                    eprintln!(
                        "[sketch-gpui] re-attach after reconnect failed for {}: {e}",
                        &sid[..sid.len().min(8)],
                    );
                }
            }
        }
        eprintln!(
            "[sketch-gpui] session-server reconnected; re-attached {} session(s)",
            sids.len(),
        );
        Some((note_rx, wake_rx))
    }

    /// Unified pump task for the session server path. Drains all
    /// notifications from `SessionServerClient::try_recv()` and routes
    /// them to the correct `AgentSlot` by `server_session_id`. Runs as a
    /// single GPUI background task per view (not per-slot).
    fn start_server_pump(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            use futures::stream::StreamExt;
            use futures::FutureExt;

            // Take exclusive ownership of the notification + wake receivers
            // once (Phase 2 of spec-pump-fix-synthesis.md). Channel reads
            // need no `&mut SketchGpuiView`, so the old pattern of grabbing
            // the model lock just to call `try_recv` was pure contention with
            // keystrokes and render. After this, the loop only takes the lock
            // to *apply* a pre-drained batch.
            let (mut note_rx, mut wake_rx) = match this.update(cx, |this, _cx| {
                this.session_server.as_mut().map(|s| {
                    (s.take_notification_receiver(), s.take_wake_receiver())
                })
            }) {
                Ok(Some((Some(rx), wake))) => (rx, wake),
                // No server, or receivers already taken — nothing to pump.
                _ => return,
            };

            // Reconnect backoff: once the wake channel closes (server gone) we
            // retry the connection on this cadence rather than hammering it.
            let reconnect_backoff = Duration::from_millis(1000);
            let mut last_reconnect: Option<std::time::Instant> = None;

            // Per-cycle cap so a runaway producer can't starve other tasks;
            // when we hit it we skip the wait and immediately drain more.
            const DRAIN_CAP: usize = 4096;
            let heartbeat = Duration::from_millis(100);
            let poll_fallback = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let anim_period = Duration::from_millis(120);
            let mut last_anim = std::time::Instant::now();
            // Last thinking-indicator second-fingerprint we repainted for. The
            // probe still runs every `anim_period` (cheap traversal), but we
            // only `cx.notify()` — which forces a full O(transcript) re-render —
            // when the displayed whole-second clock actually changes (~1Hz),
            // not on every 120ms tick.
            let mut last_anim_fp: Option<u64> = None;
            let mut more_pending = false;

            loop {
                // 1. WAIT — event-driven when we have a wake channel, else
                // poll. Skipped entirely when the last cycle hit the cap.
                if !more_pending {
                    let mut wake_closed = false;
                    if let Some(rx) = wake_rx.as_mut() {
                        let timer = cx.background_executor().timer(heartbeat);
                        futures::select_biased! {
                            v = rx.next().fuse() => {
                                if v.is_some() {
                                    // Collapse coalesced wakes; one drain covers them.
                                    while rx.next().now_or_never().flatten().is_some() {}
                                } else {
                                    wake_closed = true;
                                }
                            }
                            _ = timer.fuse() => {}
                        }
                    } else {
                        cx.background_executor().timer(poll_fallback).await;
                    }
                    // Wake channel closed (server reader thread gone): degrade
                    // to polling rather than spinning on an instant `None`.
                    if wake_closed {
                        wake_rx = None;
                    }
                }

                // RECONNECT — when the wake channel is gone the connection
                // dropped. Try to re-establish it (rate-limited) and, on
                // success, splice the fresh receivers back into the loop and
                // re-attach every slot so the durable session resumes.
                if wake_rx.is_none() {
                    let now = std::time::Instant::now();
                    let due = last_reconnect
                        .map_or(true, |t| now.duration_since(t) >= reconnect_backoff);
                    if due {
                        last_reconnect = Some(now);
                        match this.update(cx, |this, _cx| this.reconnect_session_server()) {
                            Ok(Some((new_note, new_wake))) => {
                                note_rx = new_note;
                                wake_rx = Some(new_wake);
                                last_reconnect = None;
                                let _ = this.update(cx, |_t, cx| cx.notify());
                            }
                            Ok(None) => {} // still down — retry after backoff
                            Err(_) => return, // view dropped
                        }
                    }
                }

                // 2. EXTRACT — drain the channel with no model lock held.
                let mut batch: Vec<ServerNotification> = Vec::new();
                while batch.len() < DRAIN_CAP {
                    match note_rx.try_recv() {
                        Ok(note) => batch.push(note),
                        Err(_) => break,
                    }
                }
                more_pending = batch.len() >= DRAIN_CAP;
                if batch.is_empty() {
                    more_pending = false;
                    // No events — but if a turn is in flight, tick the
                    // thinking animation so the elapsed/quiet timers stay
                    // live during a stall.
                    if last_anim.elapsed() >= anim_period {
                        last_anim = std::time::Instant::now();
                        let _ = this.update(cx, |this, cx| {
                            // Only repaint when the whole-second indicator clock
                            // changed; an unchanged fingerprint means the visible
                            // "Thinking… mm:ss" label is identical, so the full
                            // transcript rebuild a notify() triggers would be
                            // wasted (this is the dominant idle-stall cost).
                            let fp = this.awaiting_anim_fingerprint();
                            if fp.is_some() && fp != last_anim_fp {
                                last_anim_fp = fp;
                                cx.notify();
                            } else if fp.is_none() {
                                last_anim_fp = None;
                            }
                        });
                    }
                    continue;
                }

                // 3. APPLY — one model-lock acquisition for the whole cycle.
                // `apply_server_batch` notifies once if anything changed.
                let perf = perf_enabled();
                let batch_len = batch.len();
                let t_apply = perf.then(std::time::Instant::now);
                if this
                    .update(cx, |this, cx| this.apply_server_batch(batch, cx))
                    .is_err()
                {
                    return; // view dropped
                }
                if let Some(t) = t_apply {
                    eprintln!(
                        "[perf] server-pump apply events={batch_len} \
                         lock_held={:.2}ms more_pending={more_pending}",
                        t.elapsed().as_secs_f64() * 1e3,
                    );
                }

                // Yield between mega-batches so GPUI can repaint.
                if more_pending {
                    cx.background_executor().timer(yield_delay).await;
                }
            }
        })
    }

    /// Find an agent slot by its server session id across ALL tabs and panes,
    /// running `f` on the first match. Returns `true` if a slot was found.
    ///
    /// A single shared `SessionServerClient` multiplexes every session's
    /// notifications onto one pump (`start_server_pump`), so routing must
    /// search the whole workspace — not just the active tab — or a session
    /// living in a background tab silently drops its streamed output. The
    /// scan is cheap: a handful of tabs × panes × slots.
    fn with_server_session_slot(
        &mut self,
        sid: &str,
        mut f: impl FnMut(&mut AgentSlot),
    ) -> bool {
        for tab in self.workspace.tabs.iter_mut() {
            let found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    if let Some(slot) = ring.slot_by_server_session_id_mut(sid) {
                        f(slot);
                        return Some(());
                    }
                }
                None
            });
            if found.is_some() {
                return true;
            }
        }
        false
    }

    /// Run `f` on the slot for `sid` in *every* panel that has one (unlike
    /// [`with_server_session_slot`], which stops at the first match). A session
    /// observed in multiple panes must fan its events out to all of them.
    /// Returns the number of slots visited.
    fn for_each_server_session_slot(
        &mut self,
        sid: &str,
        mut f: impl FnMut(&mut AgentSlot),
    ) -> usize {
        let mut count = 0;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    if let Some(slot) = ring.slot_by_server_session_id_mut(sid) {
                        f(slot);
                        count += 1;
                    }
                }
            });
        }
        count
    }

    /// Reconcile a server-side close into the local model: drop the slot for
    /// `sid` from every panel's ring. A ring left empty is replaced in place
    /// with its stashed underlying screen (or a fresh browser) so no panel is
    /// ever left holding an empty `AgentRing`, which would panic on render.
    /// Returns whether anything changed.
    fn reconcile_session_closed(&mut self, sid: &str) -> bool {
        let mut changed = false;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                // Compute the replacement (if the ring empties) *before*
                // reassigning, so the `ring` borrow ends first.
                let restore: Option<Option<WindowContent>> =
                    if let WindowContent::Agent(ring) = content {
                        if let Some(pos) = ring.position_by_server_session_id(sid) {
                            ring.close_at(pos);
                            changed = true;
                            if ring.is_empty() {
                                Some(ring.underlying.take().map(|b| *b))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                if let Some(under) = restore {
                    *content = under.unwrap_or_else(|| {
                        WindowContent::Browser(BrowserWindow::standalone(
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        ))
                    });
                }
            });
        }
        changed
    }

    /// Reconcile a server-side rename: update the label on the matching slot in
    /// every panel. Returns whether anything changed.
    fn reconcile_session_renamed(&mut self, sid: &str, label: &str) -> bool {
        let mut changed = false;
        self.for_each_server_session_slot(sid, |slot| {
            if slot.label != label {
                slot.label = label.to_string();
                changed = true;
            }
        });
        changed
    }

    /// Apply a pre-drained batch of server notifications to the model. Called
    /// inside a single `this.update()` so the model lock is held only for the
    /// state mutation, never for channel I/O. Returns whether anything
    /// changed and emits exactly one `cx.notify()` for the whole batch.
    fn apply_server_batch(
        &mut self,
        batch: Vec<ServerNotification>,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_candidate = self.is_candidate;
        let mut ready_change: Option<bool> = None;

        let warn_unrouted = |routed: bool, sid: &str| {
            if !routed {
                eprintln!(
                    "[sketch-gpui] pump: no slot for server session {}",
                    &sid[..sid.len().min(8)],
                );
            }
        };

        // A single shared `SessionServerClient` multiplexes every session's
        // notifications onto this one pump, so each note is routed by its
        // `session_id` across the *whole* workspace (all tabs and panes), not
        // just the active tab — otherwise a session living in a background
        // tab silently drops its streamed output.
        let did_work = !batch.is_empty();
        // Sessions that received at least one ReplyEvent in this batch. The
        // follow-scroll is hoisted out of the per-event loop and applied once
        // per affected session below: only the *final* scroll position matters,
        // and a batch can hold thousands of chunk events (DRAIN_CAP), so doing
        // the workspace walk + scroll bookkeeping per event was O(events ×
        // workspace) wasted work during fast streaming.
        let mut scrolled_sessions: Vec<String> = Vec::new();
        // Perf: a streaming batch is overwhelmingly a run of ReplyEvent chunks
        // for the SAME session. Previously each chunk re-walked every tab+pane
        // to find the slot (O(events*panes)) and cloned the event String into a
        // throwaway `vec![event.clone()]`. Coalesce consecutive same-session
        // ReplyEvents into one slot lookup + one `apply_reply_events` call,
        // moving the events by value (no per-chunk clone). This keeps ordering
        // relative to other event kinds (we only merge adjacent ReplyEvents)
        // while collapsing routing to O(distinct_runs*panes) and shortening the
        // model-lock hold time.
        let mut batch = batch.into_iter().peekable();
        while let Some(note) = batch.next() {
            match note {
                ServerNotification::ReplyEvent { session_id, event } => {
                    // Drain the contiguous run of ReplyEvents for this session.
                    let mut events = vec![event];
                    while let Some(ServerNotification::ReplyEvent {
                        session_id: next_sid,
                        ..
                    }) = batch.peek()
                    {
                        if *next_sid != session_id {
                            break;
                        }
                        match batch.next() {
                            Some(ServerNotification::ReplyEvent { event, .. }) => {
                                events.push(event)
                            }
                            _ => unreachable!(),
                        }
                    }
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let claude = &mut slot.state;
                        // The server path finalizes on its own `TurnEnded`
                        // notification, but if a `ReplayComplete` marker is
                        // forwarded in the event stream, honor it here too so
                        // the resumed transcript finalizes exactly once
                        // (Finding 13, INV-4).
                        if Self::apply_reply_events(claude, std::mem::take(&mut events)) {
                            finalize_agent_turn(&mut claude.editor);
                            claude.turn_phase = TurnPhase::Idle;
                        }
                    });
                    if routed && !scrolled_sessions.iter().any(|s| s == &session_id) {
                        scrolled_sessions.push(session_id.clone());
                    }
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::TurnEnded { session_id, turn_count } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let claude = &mut slot.state;
                        finalize_agent_turn(&mut claude.editor);
                        // Turn boundary: clear last-inserted so the next turn's
                        // user echo isn't mistaken for a duplicate of this one.
                        claude.reconciler.note_turn_progressed();
                        claude.replay_turns.last_seen = turn_count;
                        claude.turn_phase = TurnPhase::Idle;
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::UserPrompt { session_id, text } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        // Route through the single chokepoint as an `Echo`: the
                        // reconciler suppresses it when it matches our own
                        // optimistic submit (live) or a turn already inserted by
                        // a second source (replay), and inserts it otherwise.
                        // Server-managed slots never advance the replay boundary
                        // here — their boundaries arrive as replayed `TurnEnded`.
                        slot.state.insert_user_turn(
                            &text,
                            sketch::agent_transcript::UserTurnOrigin::Echo,
                            false,
                        );
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionAttached { session_id, acp_session_id } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let label = acp_session_id.as_deref().unwrap_or("connected");
                        let msg = format!("attached: {label}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionDetached { session_id, reason } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let msg = format!("detached: {reason}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::OwnerChanged { session_id, has_owner } => {
                    if is_candidate {
                        ready_change = Some(!has_owner);
                        self.with_server_session_slot(&session_id, |slot| {
                            let msg = if has_owner {
                                "mirroring (original active) — read-only"
                            } else {
                                "original released — menu → claude → take over"
                            };
                            Self::append_system_notice(&mut slot.state, msg);
                            slot.state.status = Some(msg.into());
                        });
                    }
                }
                ServerNotification::SessionCreated { session } => {
                    // List-level signal that some connection created a session.
                    // The primary GUI does not auto-add it to unrelated panels
                    // (a new session belongs to the panel that opened it, which
                    // already has its slot from the create response). Kept as a
                    // no-op hook for a future "available sessions" view / for
                    // mirror GUIs that want to surface every live session.
                    let _ = &session;
                }
                ServerNotification::SessionClosed { session_id } => {
                    // A session closed somewhere (this GUI, another panel, or
                    // another GUI instance). Drop its slot from every panel so
                    // the lists stay consistent.
                    self.reconcile_session_closed(&session_id);
                }
                ServerNotification::SessionRenamed { session_id, label } => {
                    self.reconcile_session_renamed(&session_id, &label);
                }
            }
        }
        // Single follow-scroll per session that streamed this batch, instead of
        // once per chunk event. Uses the stale `list_item_count` exactly as the
        // per-event path did (the authoritative re-scroll with the fresh count
        // happens later in render_agent after the ListState splice); this just
        // keeps unfocused panes that miss render's scroll roughly pinned.
        for sid in &scrolled_sessions {
            self.with_server_session_slot(sid, |slot| {
                let claude = &mut slot.state;
                // Stale-count pre-pin; the authoritative reveal with the fresh
                // post-reconcile count runs in render_agent
                // (`reveal_tail_if_following`). This does NOT stamp
                // `last_scrolled_edit_seq`, so it never suppresses that
                // render-time reveal. Shares the `follow_tail` decision (F4).
                if claude.follow_tail() && claude.list_item_count > 0 {
                    claude
                        .list_state
                        .scroll_to_reveal_item(claude.list_item_count - 1);
                }
            });
        }
        // Deferred apply outside the layout borrow.
        if let Some(ready) = ready_change {
            self.candidate_promote_ready = ready;
        }
        if did_work {
            cx.notify();
        }
        did_work
    }

    /// Pump a specific session by its monotonic index. Returns `true` if
    /// the per-tick budget was hit and more events may be queued. Returns
    /// `false` when the session is gone (pump task should exit) or the
    /// queue is drained.
    fn pump_session(&mut self, session_index: usize, cx: &mut Context<Self>) -> bool {
        const PUMP_EVENT_BUDGET: usize = 64;

        // Scoped borrow: all mutable access to the ring/slot/claude happens
        // inside this block. Returns (has_events, more_pending, attached_with_id,
        // is_active) so post-borrow work (persistence, activity flag) can proceed.
        //
        // Search ALL panes (not just the focused one) so that agent sessions
        // in unfocused split panes keep pumping events.
        let (has_events, more_pending, attached_with_id, is_active) = {
            // Find the slot across every pane in EVERY tab (not just the
            // active tab) so agent sessions in background tabs and unfocused
            // split panes keep pumping events.
            let mut found = None;
            for tab in self.workspace.tabs.iter_mut() {
                found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                    if let WindowContent::Agent(ring) = content {
                        let is_active_in_ring = ring.slots.get(ring.active)
                            .map(|s| s.index == session_index)
                            .unwrap_or(false);
                        if let Some(slot) = ring.slot_by_index_mut(session_index) {
                            // SAFETY: pointer is valid for the scoped-borrow
                            // block below — we don't structurally mutate the
                            // layout.
                            let ptr = &mut slot.state as *mut AgentState;
                            return Some((ptr, is_active_in_ring));
                        }
                    }
                    None
                });
                if found.is_some() {
                    break;
                }
            }
            let (state_ptr, is_active) = match found {
                Some(f) => f,
                None => return false,
            };
            // SAFETY: the layout isn't mutated during this block; the
            // pointer remains valid until the scoped borrow ends.
            let claude = unsafe { &mut *state_ptr };

            // 1) Resolve pending attach.
            let mut attach_resolved = false;
            let mut attached_with_id = false;
            if let Some(rx) = &claude.attach_pending {
                match rx.try_recv() {
                    Ok(Ok(client)) => {
                        let label = client.description();
                        if client.session_id().is_some() {
                            attached_with_id = true;
                        }
                        claude.channel = Some(client);
                        let msg = format!("attached: {label}");
                        Self::append_system_notice(claude, &msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                    Ok(Err(e)) => {
                        claude.channel = None;
                        let msg = format!("attach failed: {e} (set SKETCH_ACP_AGENT=...?)");
                        Self::append_system_notice(claude, &msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        let msg = "attach worker died before reporting result";
                        Self::append_system_notice(claude, msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                }
            }
            if attach_resolved {
                claude.attach_pending = None;
            }

            // 2) Worker dropped (channel closed)?
            let stale = claude
                .channel
                .as_ref()
                .map(|c| !c.is_connected())
                .unwrap_or(false);
            if stale {
                claude.channel = None;
                // Channel gone → no turn can be in flight; drop to Idle so the
                // spinner/timer can't strand (Finding 9). The prior code cleared
                // only `turn_started`, leaving `awaiting_reply` stuck true.
                claude.turn_phase = TurnPhase::Idle;
                Self::append_system_notice(claude, "agent disconnected");
                claude.status = Some("agent disconnected".into());
                cx.notify();
                return false;
            }

            // 3) Drain up to PUMP_EVENT_BUDGET reply events.
            let mut events: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
            let mut current_turns = claude.replay_turns.last_seen;
            let mut more_pending = false;
            if let Some(client) = &claude.channel {
                while events.len() < PUMP_EVENT_BUDGET {
                    match client.try_recv() {
                        Some(ev) => events.push(ev),
                        None => break,
                    }
                }
                if events.len() == PUMP_EVENT_BUDGET {
                    more_pending = client.try_recv().map(|ev| {
                        events.push(ev);
                        true
                    }).unwrap_or(false);
                }
                current_turns = client.turn_count();
            }
            // A live turn ends when the agent's prompt-response settles and
            // bumps the turn counter. Replay (`session/load`) never fires a
            // prompt response — its end is the explicit `ReplayComplete`
            // marker (Finding 13, INV-4) returned by `apply_reply_events`, so
            // a transiently-empty queue between notification bursts can no
            // longer infer turn-end and finalize mid-replay.
            let turn_ended = !more_pending && current_turns > claude.replay_turns.last_seen;
            let has_events = !events.is_empty() || turn_ended;
            if has_events {
                let mut replay_complete = Self::apply_reply_events(claude, events);
                if turn_ended {
                    // Drain any straggler events that queued after the budget
                    // cut so they're applied before we finalize.
                    let mut tail: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
                    if let Some(client) = &claude.channel {
                        while let Some(ev) = client.try_recv() {
                            tail.push(ev);
                        }
                    }
                    replay_complete |= Self::apply_reply_events(claude, tail);
                    claude.replay_turns.last_seen = current_turns;
                }
                if turn_ended || replay_complete {
                    finalize_agent_turn(&mut claude.editor);
                    claude.turn_phase = TurnPhase::Idle;
                }
                // Spec §19 auto-scroll. Shares the `follow_tail` decision (F4).
                // Stale-count pre-pin only; the authoritative reveal with the
                // fresh post-reconcile count runs in render_agent
                // (`reveal_tail_if_following`), so this does NOT stamp
                // `last_scrolled_edit_seq`.
                if claude.follow_tail() && claude.list_item_count > 0 {
                    claude
                        .list_state
                        .scroll_to_reveal_item(claude.list_item_count - 1);
                }
            }

            (has_events, more_pending, attached_with_id, is_active)
        };

        // Post-borrow: persist the whole ring snapshot so the just-attached
        // slot's id (or its preserved resume_id, if load failed) lands on
        // disk. Writing the snapshot (not just the one slot) is what makes
        // a stale pump from a removed slot safe — it contributes nothing
        // if its slot isn't in the ring anymore.
        if attached_with_id {
            self.save_agent_ring();
        }

        // Mark inactive sessions with unseen activity (cross-tab, cross-pane).
        if has_events && !is_active {
            for tab in self.workspace.tabs.iter_mut() {
                tab.layout.for_each_leaf_content_mut(&mut |content| {
                    if let WindowContent::Agent(ring) = content {
                        if let Some(slot) = ring.slot_by_index_mut(session_index) {
                            slot.has_unseen_activity = true;
                        }
                    }
                });
            }
        }

        if has_events {
            cx.notify();
        }
        more_pending
    }

    /// Insert a lifecycle notice into the agent buffer as a frozen line.
    /// The `―` prefix distinguishes system notices from agent prose.
    /// Splice a sketch-local lifecycle notice into the transcript. Tagged
    /// `TurnId::System` — NOT `Llm(k)` — so it never masquerades as an agent
    /// turn: it carries no turn number, emits no Claude `TurnHeader`, renders
    /// a blank gutter, and is excluded from agent-turn numbering. Because the
    /// next agent chunk's `Llm(k)` lookup keys off the last `Llm`-tagged line,
    /// a `System`-tagged notice can't perturb it (Finding 5, INV-3).
    fn append_system_notice(claude: &mut AgentState, msg: &str) {
        // Ensure the transcript ends on a newline so the notice starts on its
        // OWN line. Otherwise the notice's leading `\n` splices onto the prior
        // (possibly in-flight `Llm(k)`) line, and `append_llm_chunk` re-tags
        // that whole line `System` — silently demoting agent prose. Mirrors
        // `freeze_as_user_turn`'s boundary guard (Finding 5, INV-3).
        let doc = claude.editor.document();
        if !doc.is_empty() && doc.last_char() != Some('\n') {
            let eof = doc.rope().len_chars();
            claude.editor.programmatic_insert(eof, "\n");
        }
        let notice_line = format!("― {msg}\n");
        claude
            .editor
            .append_llm_chunk(TurnId::System, &notice_line);
    }

    /// Apply a batch of reply events to the AgentState. Text chunks are
    /// spliced into the buffer; tool calls land in `tool_calls` and are
    /// anchored to whatever buffer line is the current end-of-frozen so
    /// the renderer can slot the tool block in between text on either
    /// side. Updates merge into existing tool calls via `ToolCall::update`.
    /// Apply a batch of events, returning `true` if a `ReplayComplete`
    /// marker was seen (Finding 13, INV-4) — the caller then finalizes the
    /// turn exactly once. Returning the signal (rather than finalizing here)
    /// keeps finalize a pump-side decision colocated with the live
    /// `turn_ended` path.
    fn apply_reply_events(
        claude: &mut AgentState,
        events: Vec<sketch::acp_channel::ReplyEvent>,
    ) -> bool {
        use sketch::acp_channel::ReplyEvent;
        // Any inbound activity refreshes the quiet-clock the thinking
        // indicator reads, so a streaming turn never looks stalled. A no-op
        // when idle (e.g. replay events arriving outside an awaited turn).
        if !events.is_empty() {
            claude.turn_phase.note_event(std::time::Instant::now());
        }
        let mut replay_complete = false;
        for ev in events {
            // In-progress turn for tagging streamed content, resolved per
            // event so a replayed `UserMessage` boundary mid-batch advances
            // the turn for the chunks that follow it (Finding 3, INV-3).
            // `current_turn()` is the single source of `k` (live submit and
            // replay agree): live turns read `replay_turns.last_seen + 1`;
            // during replay the boundary-advanced cursor takes over.
            let current_turn = claude.current_turn();
            match ev {
                ReplyEvent::Chunk(text) => {
                    // Spec §E3: append at the end of the last frozen line
                    // tagged with this turn (mid-line for in-progress
                    // continuation, EOF for a new turn). Editable user
                    // lines anywhere else in the document stay put.
                    if perf_enabled() {
                        eprintln!("[chunklog gui] turn={current_turn} {text:?}");
                    }
                    // Real content means a retry (if any) succeeded — drop
                    // the transient "retrying…" notice.
                    claude.status = None;
                    claude
                        .editor
                        .append_llm_chunk(TurnId::Llm(current_turn), text.as_str());
                }
                ReplyEvent::ToolCallStarted(mut tc) => {
                    cap_tool_call_payloads(&mut tc);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor);
                    // Parse the protocol id into the domain key ONCE here, at
                    // the boundary where a ToolCall enters apply_reply_events
                    // (Finding 7). All tool maps below are keyed on it.
                    let id = ToolCallKey::from_id(&tc.tool_call_id);
                    claude.tool_call_anchor_line.insert(id.clone(), anchor);
                    // Tag the anchor with `Tool(k)` so the gutter shows
                    // `Tk` on tool-group anchor lines (§11).
                    claude
                        .editor
                        .metadata_mut::<TurnId>()
                        .insert(anchor, TurnId::Tool(current_turn));
                    // Sub-agent classification (§25) is derived on demand
                    // from `tool_call_order` + `tool_calls` — see
                    // `AgentState::subagents()`. Nothing to push here.
                    if !claude.tool_calls.contains_key(&id) {
                        claude.tool_call_order.push(id.clone());
                    }
                    claude.tool_calls.insert(id, tc);
                }
                ReplyEvent::ToolCallUpdated(upd) => {
                    let id = ToolCallKey::from_id(&upd.tool_call_id);
                    if let Some(existing) = claude.tool_calls.get_mut(&id) {
                        existing.update(upd.fields);
                        cap_tool_call_payloads(existing);
                        // No sub-agent mirror to update: `subagents()`
                        // derives label + status live from the tool call we
                        // just mutated (ADR-0006 quick win #1).
                    } else {
                        // Update arrived for a tool call we never saw the
                        // start for (rare, but possible if the worker
                        // dropped an early notification). Synthesize an
                        // entry so the user still sees it.
                        let mut tc = sketch::acp_channel::ToolCall::new(
                            upd.tool_call_id.clone(),
                            String::new(),
                        );
                        tc.update(upd.fields);
                        cap_tool_call_payloads(&mut tc);
                        let anchor = anchor_for_new_tool_call(&mut claude.editor);
                        claude.tool_call_anchor_line.insert(id.clone(), anchor);
                        claude
                            .editor
                            .metadata_mut::<TurnId>()
                            .insert(anchor, TurnId::Tool(current_turn));
                        // Sub-agent entry (if any) is derived by
                        // `subagents()` from the maps below.
                        claude.tool_call_order.push(id.clone());
                        claude.tool_calls.insert(id, tc);
                    }
                }
                ReplyEvent::PlanUpdated(plan) => {
                    // Full snapshot replaces the previous plan (§21).
                    claude.current_plan = Some(plan);
                }
                ReplyEvent::ModeChanged(mode_id) => {
                    claude.agent_mode = Some(mode_id);
                }
                ReplyEvent::UsageUpdated(snap) => {
                    claude.usage = Some(snap);
                }
                ReplyEvent::Notice(ref msg) => {
                    // Driver status (retry/failed) — show inline in the
                    // buffer and in the footer status slot.
                    Self::append_system_notice(claude, msg);
                    claude.status = Some(msg.clone().into());
                }
                ReplyEvent::UserMessage(text) => {
                    // A user-authored turn surfaced by the agent's
                    // `UserMessageChunk` (Finding 1 / defect B, INV-1, INV-6).
                    // Emitted unconditionally by the worker — both live (an
                    // echo of the prompt Submit already inserted) and on
                    // `session/load` replay (reconstructing prior prompts). The
                    // single chokepoint's reconciler suppresses the live echo
                    // by content identity (order-independent — the old
                    // suffix check double-rendered whenever a chunk streamed
                    // first) and inserts genuine replayed turns. Only the
                    // direct-channel replay path advances the replay boundary
                    // (`!server_managed`): there is no replayed `TurnEnded` to
                    // bump the live counter, so each user boundary must step
                    // the cursor — User(1),Llm(1),User(2),Llm(2)…. A suppressed
                    // echo never advances, so the live counter is safe.
                    let advance = !claude.server_managed;
                    claude.insert_user_turn(
                        &text,
                        sketch::agent_transcript::UserTurnOrigin::Echo,
                        advance,
                    );
                }
                ReplyEvent::ReplayComplete => {
                    // The agent finished re-emitting the prior conversation
                    // (Finding 13, INV-4). Fold the replay cursor back into
                    // the live counter so the next live turn continues from
                    // the right `k`, then signal the pump to finalize once —
                    // after the last replayed chunk, never mid-replay.
                    claude.finish_replay();
                    claude.reconciler.note_turn_progressed();
                    replay_complete = true;
                }
            }
        }
        replay_complete
    }

    /// Wipe the local claude buffer + tool-call state, drop the saved
    /// session id for the current cwd, and reattach. Equivalent to
    /// `/clear` in the Claude Code TUI: previous turns disappear from
    /// the view *and* the agent gets a fresh `session/new` so it isn't
    /// holding on to context from the cleared conversation. Use this
    /// when the model has gone off-track and you want a clean slate
    /// without restarting sketch.
    fn clear_agent_session(&mut self, cx: &mut Context<Self>) {
        // Forget every persisted slot BEFORE re-opening so the new spawn
        // hits session/new instead of session/load. Done first so even
        // if open_agent_inner panics partway through, the next manual
        // attach won't accidentally resume any cleared session.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        // Drop the current claude screen entirely; open_agent_inner
        // builds a new one. We don't try to surgically reset fields on
        // the existing AgentState because the underlying screen
        // (browser/doc) is also stashed there — preserving it is the
        // job of open_agent_inner via the prior-screen swap dance.
        if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Agent(_)) {
            // Restore underlying first so open_agent_inner can capture
            // it as the new "prior" screen. Otherwise we'd lose the
            // file/browser the user was viewing before they opened
            // claude.
            self.back_to_doc(cx);
        }
        self.open_agent_inner(cx);
        if let Some(c) = self.agent_mut() {
            c.status = Some("session cleared".into());
        }
        cx.notify();
    }

    /// Cycle the ACP permission mode (read-only → auto-edit → ask-each →
    /// yolo → read-only). Surfaces the new mode in the claude footer so
    /// the user sees the change without having to find it in the header.
    fn cycle_claude_permission_mode(&mut self, cx: &mut Context<Self>) {
        let Some(claude) = self.agent_mut() else {
            return;
        };
        let new_mode = match &claude.channel {
            Some(ch) => {
                let next = ch.permission_mode().next();
                ch.set_permission_mode(next);
                Some(next)
            }
            None => None,
        };
        match new_mode {
            Some(m) => {
                let msg = format!("permission mode → {}", m.short_label());
                Self::append_system_notice(claude, &msg);
                claude.status = Some(msg.into());
            }
            None => {
                claude.status = Some("permission mode: no agent attached".into());
            }
        }
        cx.notify();
    }

    /// Drop the active session's `AcpChannelClient` (kills the subprocess
    /// via `kill_on_drop`) but keep the `AgentSlot` and its chat history
    /// intact. The sidebar's `[d]` suffix surfaces the detached state. The
    /// slot's `resume_id` is preserved so the next reboot still tries to
    /// `session/load` the original id (per spec §15 stability rule); fresh
    /// `claude-new` slots without a `resume_id` will silently drop from
    /// persistence on the next save (per spec: "slots without a session id
    /// are not written").
    fn detach_active_agent_session(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        if claude.channel.is_none() && claude.attach_pending.is_none() {
            claude.status = Some("session is already detached".into());
            cx.notify();
            return;
        }
        // Drop runs `kill_on_drop` on the subprocess; cancel any in-flight
        // attach by dropping its receiver (the spawning thread's send will
        // fail silently when the connection drops).
        claude.channel = None;
        claude.attach_pending = None;
        claude.turn_phase = TurnPhase::Idle;
        Self::append_system_notice(claude, "session detached");
        claude.status = Some("session detached".into());
        self.save_agent_ring();
        cx.notify();
    }

    /// Spawn a fresh `AcpChannelClient` for the active session. Per spec §4
    /// re-attach does NOT resume the previous conversation — the agent
    /// subprocess was killed on detach, so the session is gone. Clear
    /// `resume_id` so persistence captures the new channel's id once it
    /// binds (rather than retrying the original-load id forever).
    fn attach_active_agent_session(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            if c.channel.is_some() || c.attach_pending.is_some() {
                c.status = Some("session is already attached".into());
                cx.notify();
                return;
            }
        }

        // Use the active slot's per-session cwd (spec-agent-cwd.md §3)
        // rather than the process cwd, so a slot that lives at /foo
        // re-attaches at /foo and not at sketch's launch directory.
        let slot_cwd = match self.agent_ring() {
            Some(r) => Some(r.active().cwd.clone()),
            None => return,
        };
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    None,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        if let Some(ring) = self.agent_ring_mut() {
            ring.active_mut().resume_id = None;
            let claude = &mut ring.active_mut().state;
            claude.attach_pending = Some(attach_rx);
            Self::append_system_notice(claude, "attaching new session…");
            claude.status = Some("attaching new session…".into());
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Quit-and-relaunch sketch with the auto-open-claude flag set, so the
    /// new process boots straight into the claude screen and restores every
    /// session that was in the ring at quit time via `load_persisted_acp_sessions`
    /// + per-slot `spawn_with_resume`. Designed for "I broke something in
    /// sketch and want to keep iterating with the same Claude context" —
    /// the user's chat history (on the agent side) is preserved through
    /// `session/load`.
    ///
    /// Spawns the child detached from the parent's stdio so the new GUI
    /// stays alive after `cx.quit()` tears down the current window. Args
    /// from the original invocation (e.g. the file path) are forwarded so
    /// the file the user was editing also reappears.
    fn reboot_into_claude(&mut self, cx: &mut Context<Self>) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.env("SKETCH_OPEN_CLAUDE", "1");
        for arg in std::env::args().skip(1) {
            cmd.arg(arg);
        }
        // Detach stdio: the child inherits its own session so the dying
        // parent doesn't drag it down. On macOS/Linux a successful spawn
        // from a GUI process already survives parent exit because launchd
        // / init reparents it, but null-ing the streams is still cheap
        // insurance against any inherited pipe getting closed.
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(_) => cx.quit(),
            Err(e) => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some(format!("reboot failed: {e}").into());
                }
            }
        }
    }

    /// Send the user's pending draft (`extract_editable_inserts` —
    /// only the editable runs between/after frozen Claude turns) as the
    /// next ACP prompt, then lock the turn so that content can't be
    /// retroactively edited.
    /// Toggle the Tasklist sidepane visibility (§24).
    fn toggle_tasklist(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.tasklist_open = !c.tasklist_open;
        }
        cx.notify();
    }

    /// Toggle the Subagents sidepane visibility (§28).
    fn toggle_subagents(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.subagents_open = !c.subagents_open;
        }
        cx.notify();
    }

    /// Set the focused sub-agent by its stable tool-call key (§27). The
    /// main transcript swap is purely a render-time decision; this just
    /// flips the field. Keying by `ToolCallKey` (not a positional index)
    /// keeps focus pinned to the same sub-agent regardless of how the
    /// derived `subagents()` list is ordered (ADR-0006 quick win #1).
    fn focus_subagent(&mut self, key: ToolCallKey, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            if c.tool_calls.contains_key(&key) {
                c.focused_subagent = Some(key);
            }
        }
        cx.notify();
    }

    /// Return focus from a sub-agent transcript to the root agent (§27).
    fn unfocus_subagent(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.focused_subagent = None;
        }
        cx.notify();
    }

    /// Flip the agent window's input mode (§5). Data movement is
    /// asymmetric per §6/§7:
    ///
    /// * Chatbox → Worksheet: take whatever's in the chatbox, append at
    ///   EOF of the transcript as new editable user lines (one transcript
    ///   line per chatbox line), drop the chatbox. If the chatbox was
    ///   empty, nothing is added.
    /// * Worksheet → Chatbox: don't touch the transcript at all; create
    ///   a fresh empty chatbox `Editor` and route input there. Any
    ///   editable lines already in the transcript stay pending and will
    ///   be swept by the next Submit.
    ///
    /// The chatbox's undo history is per-`Editor`; closing the chatbox
    /// drops that history (§7).
    fn toggle_agent_input_mode(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        match &claude.input_surface {
            // Read the draft text out (last use of `cb`) BEFORE reassigning the
            // field, so the match's shared borrow ends and the write is clean.
            InputSurface::Chatbox(cb) => {
                let text = cb.text();
                claude.input_surface = InputSurface::Worksheet;
                if !text.is_empty() {
                    // Ensure the transcript ends with a `\n` so the
                    // appended draft starts on its own line, then drop
                    // the trailing newline of `text` so we don't leave a
                    // dangling blank below the cursor.
                    let needs_nl = !claude
                        .editor
                        .document()
                        .full_text()
                        .ends_with('\n');
                    let eof = claude.editor.document().rope().len_chars();
                    if needs_nl {
                        claude.editor.programmatic_insert(eof, "\n");
                    }
                    let to_append =
                        text.strip_suffix('\n').unwrap_or(&text).to_string();
                    let eof2 = claude.editor.document().rope().len_chars();
                    claude.editor.programmatic_insert(eof2, &to_append);
                    let new_eof = claude.editor.document().rope().len_chars();
                    let (cl, cc) =
                        doc_char_to_line_col(claude.editor.document(), new_eof);
                    claude.editor.cursor_mut().line = cl;
                    claude.editor.cursor_mut().col = cc;
                }
                claude.editor.clear_selection();
            }
            InputSurface::Worksheet => {
                claude.input_surface = InputSurface::Chatbox(Chatbox::new());
            }
        }
        cx.notify();
    }

    /// Submit the user's draft to the agent. Dispatches on `input_mode`:
    /// Worksheet sweep (§12) sweeps every editable line in document order,
    /// freezes them with `TurnId::User(k)`, and sends the non-blank lines.
    /// Chatbox submit (§18) takes the chatbox text, appends + freezes it
    /// at EOF of the transcript, then sends and clears the chatbox.
    fn submit_agent(&mut self, cx: &mut Context<Self>) {
        if self.is_candidate {
            self.set_agent_status(
                "read-only mirror — close the original window, then menu → claude → take over",
                cx,
            );
            return;
        }
        let is_chatbox = match self.agent_mut() {
            Some(c) => {
                // Re-enable auto-scroll when the user sends a message.
                c.follow_output.set(true);
                c.input_surface.is_chatbox()
            }
            None => return,
        };
        if is_chatbox {
            self.submit_chatbox(cx);
        } else {
            self.submit_worksheet(cx);
        }
    }

    /// Whether any agent slot (across all tabs/panes) is mid-turn. Cheap
    /// traversal the pumps use to decide whether an idle animation tick is
    /// worth a re-render.
    fn any_agent_awaiting(&mut self) -> bool {
        let mut awaiting = false;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    if ring.slots.iter().any(|s| s.state.turn_phase.is_awaiting()) {
                        awaiting = true;
                    }
                }
            });
        }
        awaiting
    }

    /// Whole-second fingerprint of the thinking-indicator clock across all
    /// awaiting agents, or `None` if nothing is awaiting. The indicator only
    /// displays `mm:ss`-granular elapsed/quiet timers, so the pump uses this to
    /// notify (and trigger the full transcript re-render) at most ~1Hz instead
    /// of every 120ms — 8x fewer O(transcript) rebuilds during a stall. We fold
    /// elapsed + quiet seconds into one value so a change in either repaints.
    fn awaiting_anim_fingerprint(&mut self) -> Option<u64> {
        let mut any = false;
        let mut fp: u64 = 0;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let WindowContent::Agent(ring) = content {
                    for s in ring.slots.iter() {
                        if s.state.turn_phase.is_awaiting() {
                            any = true;
                            let elapsed = s
                                .state
                                .turn_phase
                                .turn_started()
                                .map(|t| t.elapsed().as_secs())
                                .unwrap_or(0);
                            let quiet = s
                                .state
                                .turn_phase
                                .last_event_at()
                                .map(|t| t.elapsed().as_secs())
                                .unwrap_or(0);
                            // Combine without losing either's transitions.
                            fp = fp.wrapping_add(elapsed).wrapping_mul(1_000_003)
                                ^ quiet.wrapping_add(1);
                        }
                    }
                }
            });
        }
        any.then_some(fp)
    }

    /// Interrupt the in-flight agent turn (ACP `session/cancel`). Routes
    /// through the active path — session server when one owns the slot,
    /// otherwise the direct `AcpChannelClient`. The agent resolves the turn
    /// with `StopReason::Cancelled`, which bumps the turn counter and clears
    /// `awaiting_reply` on the next pump tick. No-op when nothing is in
    /// flight. Bound to `StopAgent` (Cmd-.) and the footer Stop button.
    fn stop_agent(&mut self, _: &StopAgent, _w: &mut Window, cx: &mut Context<Self>) {
        // Read-only mirrors can't drive the session.
        if self.is_candidate {
            return;
        }
        // Only meaningful mid-turn.
        let awaiting = self
            .agent_mut()
            .map(|c| c.turn_phase.is_awaiting())
            .unwrap_or(false);
        if !awaiting {
            if let Some(claude) = self.agent_mut() {
                claude.status = Some("nothing to stop".into());
            }
            cx.notify();
            return;
        }

        // Second Stop while a cancel is already pending escalates to a hard
        // kill + resume — for a turn wedged on a hung upstream request the
        // cooperative `session/cancel` may never land.
        let escalate = self
            .agent_mut()
            .map(|c| c.turn_phase.stop_requested())
            .unwrap_or(false);
        if escalate {
            // Record the escalation on the phase before the hard kill so the
            // transition stays a total function over `TurnPhase` (the marker is
            // transient — `force_restart_agent` drops to Idle immediately after).
            if let Some(claude) = self.agent_mut() {
                claude.turn_phase.escalate();
            }
            self.force_restart_agent(cx);
            return;
        }

        // First Stop → graceful ACP session/cancel.
        let server_sid = self.active_server_session_id();
        let sent = if let Some(sid) = &server_sid {
            self.session_server
                .as_ref()
                .and_then(|s| s.cancel(sid).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            match claude.channel.as_ref() {
                Some(channel) => {
                    channel.cancel();
                    true
                }
                None => false,
            }
        } else {
            false
        };
        if let Some(claude) = self.agent_mut() {
            claude.turn_phase.request_stop(std::time::Instant::now());
            claude.status = Some(if sent {
                "stopping… (⌘. again to force-restart)".into()
            } else {
                "nothing to stop".into()
            });
        }
        cx.notify();
    }

    /// Hard recovery for a wedged turn: kill the agent subprocess and respawn
    /// it, resuming the same ACP session so prior context survives. The
    /// escalation behind a second Stop press. Routes to the session server
    /// (which owns the subprocess) in server mode, otherwise drops and
    /// re-attaches the direct channel.
    fn force_restart_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(sid) = self.active_server_session_id() {
            let ok = self
                .session_server
                .as_ref()
                .and_then(|s| s.restart_session(&sid).ok())
                .is_some();
            if let Some(claude) = self.agent_mut() {
                claude.turn_phase = TurnPhase::Idle;
                claude.status = Some(if ok {
                    "force-restarting agent (resuming session)…".into()
                } else {
                    "force-restart request failed".into()
                });
            }
            cx.notify();
            return;
        }

        // Direct mode: resume the current ACP session id on a fresh
        // subprocess; dropping the old channel kills the wedged one.
        let resume_id = self
            .agent_mut()
            .and_then(|c| c.channel.as_ref().and_then(|ch| ch.session_id()));
        let slot_cwd = self.agent_ring().map(|r| r.active().cwd.clone());
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let resume_for_worker = resume_id.clone();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-force-restart".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    resume_for_worker,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });
        if let Some(ring) = self.agent_ring_mut() {
            ring.active_mut().resume_id = resume_id;
            let claude = &mut ring.active_mut().state;
            claude.channel = None; // Drop → kills the wedged subprocess.
            claude.attach_pending = Some(attach_rx);
            claude.turn_phase = TurnPhase::Idle;
            Self::append_system_notice(claude, "force-restarting agent (resuming session)…");
            claude.status = Some("force-restarting agent (resuming session)…".into());
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Worksheet submit per §12. Sweep every editable line in document
    /// order, build the prompt body from those with non-whitespace content
    /// (`\n`-joined), freeze every collected line — including blank
    /// spacers — and tag each with `TurnId::User(k)` so the gutter shows
    /// `Uk`. If the body is empty, no-op with a footer hint.
    fn submit_worksheet(&mut self, cx: &mut Context<Self>) {
        // Capture server path info before borrowing agent_mut.
        let server_sid = self.active_server_session_id();

        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        // Check sendability: either direct channel or server session.
        if claude.channel.is_none() && server_sid.is_none() {
            claude.status = Some("no channel attached".into());
            cx.notify();
            return;
        }

        // Walk every line, classify editable vs frozen.
        let line_count = claude.editor.document().line_count();
        let mut collected: Vec<(usize, String)> = Vec::new();
        for l in 0..line_count {
            if claude.editor.is_frozen_line(l) {
                continue;
            }
            let line_text = claude.editor.document().line_text(l);
            let stripped = line_text.trim_end_matches('\n').to_string();
            collected.push((l, stripped));
        }

        // Build prompt body from lines with non-whitespace content.
        let prompt_body: String = collected
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if prompt_body.is_empty() {
            claude.status = Some("nothing to send".into());
            cx.notify();
            return;
        }

        // Send FIRST, then freeze the authored lines only on success — mirroring
        // submit_chatbox. The old order computed `last_seen_turns + 1` by hand
        // and froze the lines BEFORE the send check, which (a) bypassed the
        // reconciler chokepoint so the server/agent echo of this prompt
        // re-rendered it (the double-render bug) and (b) left a phantom frozen
        // turn in place when the send failed. `collected`/`prompt_body` are
        // owned, captured above, so they survive the agent re-borrow; the send
        // is fire-and-forget over a socket and never touches the editor, so the
        // captured line indices stay valid for the post-send freeze.
        let sent = if let Some(sid) = &server_sid {
            // Server path: prompt via session server (fire-and-forget; `Ok`
            // means written, not accepted — ownership is reasserted on resume).
            self.session_server.as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            // Direct path: send via AcpChannelClient.
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(claude) = self.agent_mut() {
                // Derive `k` + arm dedup through the shared reconciler core and
                // freeze the authored lines in place. Registering `prompt_body`
                // as a LocalSubmit is what suppresses the echo. `None` means the
                // M3 tripwire fired — leave the lines editable rather than
                // freeze an unattributed turn.
                claude.commit_worksheet_turn(&collected, &prompt_body);
                claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
            }
        } else if let Some(claude) = self.agent_mut() {
            // Send failed: keep the authored lines editable so the user can
            // retry, and surface it rather than dropping the prompt silently.
            claude.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        if let Some(claude) = self.agent_mut() {
            claude.editor.clear_selection();
        }
        cx.notify();
    }

    /// Chatbox submit per §18. Take the full chatbox text, append it at
    /// EOF of the transcript as new lines, immediately freeze them with
    /// `TurnId::User(k)`, send via the channel, clear the chatbox. Mode
    /// stays `Chatbox`.
    fn submit_chatbox(&mut self, cx: &mut Context<Self>) {
        // Capture server path info before borrowing agent_mut.
        let server_sid = self.active_server_session_id();

        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        let text = match claude.input_surface.chatbox() {
            Some(cb) => cb.text(),
            None => return,
        };
        if text.trim().is_empty() {
            claude.status = Some("nothing to send".into());
            cx.notify();
            return;
        }
        if claude.channel.is_none() && server_sid.is_none() {
            claude.status = Some("no channel attached".into());
            cx.notify();
            return;
        }

        // Send FIRST, then freeze the optimistic echo only on success. Freezing
        // before the send could leave a "phantom" user turn in the transcript
        // that was never delivered (the old order did this). The send is to a
        // local socket/pipe, so doing it first costs nothing perceptible.
        let prompt_body = text.trim_end_matches('\n').to_string();
        let sent = if let Some(sid) = &server_sid {
            // NB: server `prompt` is fire-and-forget — `Ok` means the request
            // was written, NOT that the server accepted it (a non-owner
            // rejection is invisible here). Ownership is instead guaranteed on
            // resume by the retrying attach in `spawn_attach_sessions`, so by
            // the time the user can type, this connection owns the session.
            self.session_server.as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(claude) = self.agent_mut() {
                // Optimistic echo + begin the turn. `LocalSubmit` always
                // inserts and records the text so the stream echo that follows
                // (server `UserPrompt` or agent `UserMessage`, in any order
                // relative to streamed content) is suppressed. Never advances
                // the replay boundary on a live submit.
                claude.insert_user_turn(
                    &text,
                    sketch::agent_transcript::UserTurnOrigin::LocalSubmit,
                    false,
                );
                claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
                // Reset the chatbox to empty; cursor stays inside.
                claude.input_surface = InputSurface::Chatbox(Chatbox::new());
            }
        } else if let Some(claude) = self.agent_mut() {
            // Send failed: leave the chatbox text intact so the user can retry,
            // and surface it instead of dropping the message into the void.
            claude.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        cx.notify();
    }

    /// Key dispatch for the agent window. Recognises the agent-window-
    /// scoped shortcuts (`Ctrl-Enter` submit, `Ctrl-Alt-Enter` mode toggle,
    /// `Ctrl-V` leave, session-cycle `Ctrl-]`/`Ctrl-[`) before routing
    /// remaining keys to either the chatbox (in Chatbox mode) or the
    /// transcript editor (in Worksheet mode). See spec-agent-window.md §32.
    fn handle_claude_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Session switcher overlay intercepts all keys when open.
        if self.overlay_is_session() {
            self.handle_session_switcher_key(ev, _w, cx);
            return;
        }

        // Esc with a focused sub-agent: return to the parent transcript
        // (§27). Otherwise Esc falls through — the project rule is
        // "Esc never quits / never closes", so an unfocused-sub-agent
        // Esc keeps the existing per-mode behavior (toggle Normal etc.).
        if press.key == Key::Esc
            && self
                .agent_mut()
                .map(|c| c.focused_subagent.is_some())
                .unwrap_or(false)
        {
            self.unfocus_subagent(cx);
            return;
        }

        // Mode-toggle: Ctrl-Alt-Enter (§5). Checked before Ctrl-Enter so
        // an accidental Alt-press doesn't fire a submit instead.
        if press.modifiers.contains(KMods::CONTROL)
            && press.modifiers.contains(KMods::ALT)
            && press.key == Key::Enter
        {
            self.toggle_agent_input_mode(cx);
            return;
        }

        // Submit: Ctrl-Enter (§8). Bare Enter NEVER sends — it inserts a
        // literal newline (chatbox) or a new editable line (worksheet),
        // gated by the frozen-line invariants.
        if press.modifiers.contains(KMods::CONTROL) && press.key == Key::Enter {
            self.submit_agent(cx);
            return;
        }

        // Leave the agent window with Ctrl-V; the chatbox (if any) is
        // dropped without sending — its content is recoverable by toggling
        // back into Chatbox mode (which creates a fresh chatbox) and
        // re-typing, but we don't try to preserve unsent text across the
        // jump (spec §36).
        if press.modifiers.contains(KMods::CONTROL)
            && matches!(press.key, Key::Char('v') | Key::Char('V'))
        {
            if let Some(c) = self.agent_mut() {
                c.input_surface = InputSurface::Worksheet;
            }
            self.back_to_doc(cx);
            return;
        }

        // Session switching: Ctrl-] next, Ctrl-[ prev.
        if press.modifiers.contains(KMods::CONTROL) {
            if press.key == Key::Char(']') {
                self.switch_agent_session(1, cx);
                return;
            }
            if press.key == Key::Char('[') {
                self.switch_agent_session(-1, cx);
                return;
            }
        }

        // Chatbox-mode intercept: input routes to the chatbox editor when
        // we're in Chatbox mode; the transcript is read-only (§17). In
        // Worksheet mode the transcript IS the editing surface and the
        // chatbox doesn't exist.
        let in_chatbox = self
            .agent_mut()
            .map(|c| c.input_surface.is_chatbox())
            .unwrap_or(false);
        if in_chatbox {
            let outcome = {
                let claude = match self.agent_mut() {
                    Some(c) => c,
                    None => return,
                };
                claude.status = None;
                let cb = claude.input_surface.chatbox_mut().unwrap();
                match cb.mode {
                    EditMode::Insert => {
                        Self::dispatch_insert_core(&mut cb.editor, &mut cb.mode, press);
                        NormalOutcome::Handled
                    }
                    EditMode::Normal => Self::dispatch_normal_core(
                        &mut cb.editor,
                        &mut cb.mode,
                        &mut claude.keybinds,
                        press,
                    ),
                }
            };
            match outcome {
                NormalOutcome::OpenMenu => self.open_menu_inner(cx),
                NormalOutcome::Quit => cx.quit(),
                _ => cx.notify(),
            }
            return;
        }

        let outcome = {
            let claude = match self.agent_mut() {
                Some(c) => c,
                None => return,
            };
            // Any non-shortcut keystroke clears the transient status.
            claude.status = None;

            match claude.mode {
                EditMode::Insert => {
                    Self::dispatch_insert_core(&mut claude.editor, &mut claude.mode, press);
                    NormalOutcome::Handled
                }
                EditMode::Normal => Self::dispatch_normal_core(
                    &mut claude.editor,
                    &mut claude.mode,
                    &mut claude.keybinds,
                    press,
                ),
            }
        };

        // Keep the cursor's doc line in view after every key. Compute
        // the cursor's index in the virtualised list (text lines are
        // interleaved with tool blocks anchored above them) and ask
        // the ListState to scroll just enough to reveal it.
        if let Some(c) = self.agent_mut() {
            let cursor_line = c.editor.cursor().line;
            let ranges = c.block_ranges.clone();
            let line_count = c.editor.document().line_count();
            let gutter_tags: Vec<Option<TurnId>> = (0..line_count)
                .map(|i| {
                    c.editor
                        .anchor_for_line_opt(i)
                        .and_then(|a| c.editor.metadata::<TurnId>().get(a).copied())
                })
                .collect();
            let th_before = count_turn_headers_before(&gutter_tags, cursor_line);
            let target = cursor_visible_child_index(c, cursor_line, &ranges, th_before);
            c.list_state.scroll_to_reveal_item(target);
        }

        match outcome {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some("yanked".into());
                }
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
        }
    }
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
                let modifier_only = ev.keystroke.key.as_str().is_empty()
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
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                this.splash_until = None;
                cx.notify();
            }))
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
        if let Some(deadline) = self.splash_until {
            if std::time::Instant::now() >= deadline {
                self.splash_until = None;
            }
        }
        if self.splash_until.is_some() {
            return self.render_splash(cx);
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

/// Test-only counter of how many `block_element`s the virtualized doc list
/// builds. The latency gate (verify_harness) asserts this stays O(visible) —
/// a few dozen for a 3000-block doc — proving render is no longer O(document).
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

impl SketchGpuiView {
    fn render_doc(
        &self,
        root: gpui::Div,
        d: &DocState,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Clear the per-render layout sink before re-emitting lines. Mouse
        // hit-testing reads this map between renders, so stale entries from
        // a now-removed block would otherwise leak through.
        self.line_layouts.borrow_mut().clear();
        // Resolve the focused doc's directory for wiki link targets.
        // `file_label` is the canonicalized path of the file backing this
        // Doc — its parent dir is where `[[name]]` lookups start. `None`
        // if the doc has no parent (e.g., root-level untitled buffer).
        let doc_dir = {
            let path = PathBuf::from(d.file_label.as_ref());
            path.parent().map(|p| p.to_path_buf())
        };
        // ---- Virtualized doc body (audit #1) ----
        //
        // The body is a `gpui::list`: only the visible block window is built
        // and laid out per frame, not one element per block. This makes a
        // `cx.notify()` (j/k move, scroll, theme/zoom, and especially every
        // mouse-move during a selection drag) O(visible) instead of
        // O(blocks+spans). Audit #2 falls out of this — `line_layouts` only
        // holds the visible lines, so `doc_pos_at`'s scan collapses to
        // O(visible) too.

        // Splice the list to the current block count. `blocks` only changes on
        // load / reload / edit-flush (each builds a fresh `DocState`) or theme
        // switch (`set_blocks` bumps `blocks_seq`), so a count change is the
        // reliable trigger; a plain `reset` is correct
        // and cheap relative to the per-row work it gates. Must run EVERY frame.
        let new_count = d.blocks.len();
        if new_count != d.list_item_count.get() {
            d.list_state.reset(new_count);
            d.list_item_count.set(new_count);
            // Force a re-reveal below (the reset cleared scroll position).
            d.last_cursor_block.set(None);
        }
        // Keep the focused block on-screen when it changed (this also catches
        // nav actions whose `reveal_block` ran against a stale count before the
        // list was first populated).
        if d.last_cursor_block.get() != Some(d.cursor_block) {
            d.last_cursor_block.set(Some(d.cursor_block));
            if d.cursor_block < new_count {
                d.list_state.scroll_to_reveal_item(d.cursor_block);
            }
        }

        // Owned snapshots for the `'static` per-row render closure — all cheap
        // (Theme clone once per frame, Rc pointer clones, SharedString refcount
        // bumps, Copy values). The closure rebuilds a `RenderCtx` borrowing
        // these owned locals for each visible block it constructs.
        let theme = self.theme.clone();
        let body_font = self.body_font.clone();
        let code_font = self.code_font.clone();
        let text_scale = self.text_scale;
        let cursor_block = d.cursor_block;
        let doc_selection = self.doc_selection;
        let line_layouts = self.line_layouts.clone();
        let weak_view = cx.entity().downgrade();
        let blocks_rc = d.blocks_rc();

        let render_fn = move |idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
            let Some(block) = blocks_rc.get(idx) else {
                return div().into_any_element();
            };
            #[cfg(test)]
            DOC_BLOCK_BUILDS.with(|c| c.set(c.get() + 1));
            let ctx = RenderCtx {
                theme: &theme,
                body_font: body_font.clone(),
                code_font: code_font.clone(),
                text_scale,
                cursor_block: Some(cursor_block),
                doc_selection,
                line_layouts: Some(line_layouts.clone()),
                current_block: None,
                weak_view: Some(weak_view.clone()),
                doc_dir: doc_dir.clone(),
            };
            block_element(&ctx, idx, block)
        };

        // View-mode mouse selection: anchor on left MouseDown, update head on
        // every MouseMove while a button is held, release on MouseUp. The
        // wrapping doc body is the listener for all three; hit-testing falls
        // through to the registered per-line TextLayouts in `self.line_layouts`
        // (now populated only for the visible window — audit #2).
        let body = div()
            .id("doc-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(self.editor_fg())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, ev: &MouseDownEvent, _w, cx| {
                    view.doc_mouse_down(ev, cx);
                }),
            )
            .on_mouse_move(cx.listener(|view, ev: &MouseMoveEvent, _w, cx| {
                view.doc_mouse_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, ev: &MouseUpEvent, _w, cx| {
                    view.doc_mouse_up(ev, cx);
                }),
            )
            .child(
                // Default (visible-only) measuring — NOT `Auto`. `Auto` means
                // "measure all items" (gpui list.rs), which builds every line to
                // measure it and registers its `TextLayout` into `line_layouts`,
                // but only the visible lines get prepainted (bounds set). Then
                // `doc_pos_at` iterating all of them calls `.bounds()` on an
                // un-prepainted layout → panic across the input callback. The
                // agent + Edit lists already use the default; the doc body's
                // parent is `flex_1().min_h_0()`, so the list fills the viewport
                // and scrolls without needing to size to content.
                gpui::list(d.list_state.clone(), render_fn)
                    .flex_1()
                    .w_full(),
            );

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(fg_or(top, STATUS_FG))
            .font_weight(FontWeight::BOLD)
            .child(format!("sketch-gpui — {}", d.file_label))
            .child(self.multi_home_dot(d.file_label.as_ref()));

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(bg_or(bot, STATUS_BG))
            .text_color(fg_or(bot, 0x666666))
            .text_size(px(11.0))
            .child(format!(
                "block {} / {}",
                d.cursor_block.saturating_add(1),
                d.blocks.len()
            ))
            .child(SharedString::new_static(
                "j/k scroll · h/l block · g/G top/bot · Ctrl-O browse · Space menu",
            ));

        root.key_context("SketchView")
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::cursor_next))
            .on_action(cx.listener(Self::cursor_prev))
            .on_action(cx.listener(Self::cursor_top))
            .on_action(cx.listener(Self::cursor_bottom))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::enter_edit))
            .on_action(cx.listener(Self::enter_wp))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::next_buffer))
            .on_action(cx.listener(Self::prev_buffer))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::split_h))
            .on_action(cx.listener(Self::split_v))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::only_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::resize_shrink))
            .on_action(cx.listener(Self::resize_grow))
            .on_action(cx.listener(Self::equalize))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_doc_selection))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(body)
            .child(footer)
    }

    fn render_edit(
        &self,
        root: gpui::Div,
        e: &mut EditState,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let mode_label = match e.mode {
            EditMode::Normal => "NORMAL",
            EditMode::Insert => "INSERT",
        };
        let view_label = match e.view {
            EditView::Code => "RAW",
            EditView::WordProcessor => "WP",
        };

        let body: AnyElement = match e.view {
            EditView::Code => self.build_edit_body_code(e).into_any_element(),
            EditView::WordProcessor => self.build_edit_body_wp(e).into_any_element(),
        };

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        let header_view_label = match e.view {
            EditView::Code => "edit",
            EditView::WordProcessor => "wp",
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(fg_or(top, STATUS_FG))
            .font_weight(FontWeight::BOLD)
            .child(format!("sketch-gpui [{}] — {}", header_view_label, e.file_label))
            .child(self.multi_home_dot(e.file_label.as_ref()));

        let dirty_mark = if e.editor.is_modified() { "•" } else { " " };
        let extend_mark = if e.editor.extend_mode() { " EXT" } else { "" };
        let sel_size: Option<usize> = e.editor.selection_range().map(|((sl, sc), (el, ec))| {
            // Cheap size summary: char count for single-line, line count otherwise.
            // Mirrors the kind of one-glance status the user wants in the footer.
            if sl == el { ec.saturating_sub(sc) } else { (el - sl) + 1 }
        });
        let mut left_status = format!(
            "{} {} {}{} · L{}:C{}",
            dirty_mark,
            view_label,
            mode_label,
            extend_mark,
            cursor_line + 1,
            cursor_col + 1,
        );
        if let Some(n) = sel_size {
            let same_line = e
                .editor
                .selection_range()
                .map(|((sl, _), (el, _))| sl == el)
                .unwrap_or(false);
            let unit = if same_line { "ch" } else { "ln" };
            left_status.push_str(&format!(" · sel:{}{}", n, unit));
        }
        if let Some(msg) = &e.last_save_msg {
            left_status.push_str("  [");
            left_status.push_str(msg);
            left_status.push(']');
        }
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(bg_or(bot, STATUS_BG))
            .text_color(fg_or(bot, 0x666666))
            .text_size(px(11.0))
            .child(left_status)
            .child(SharedString::new_static(
                "Ctrl-W toggle wp/raw · Ctrl-S save · Ctrl-V view · v ext · d del · y yank",
            ));

        // No `actions!` wired here — the EditView key context catches all
        // keys via `on_key_down` so the same vocabulary works in both modes.
        // The menu-bar actions (Quit / OpenBrowser / OpenAgent) still need
        // explicit `on_action` listeners on this root so the macOS menu bar
        // can dispatch them to whichever screen happens to be focused.
        root.key_context("EditView")
            .on_key_down(cx.listener(Self::handle_edit_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(body)
            .child(footer)
    }

    /// Code (raw markdown) view: monospace, gutter with line numbers,
    /// per-line `md_highlight` source colors. Cursor splice via the shared
    /// `build_line_content` helper.
    ///
    /// **Virtualized**: rendered through a `gpui::list` so only the visible rows
    /// are built/laid-out per frame, not one element per document line. Combined
    /// with the incremental highlight cache this makes a keystroke O(changed),
    /// not O(document).
    fn build_edit_body_code(&self, e: &mut EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let dim_fg: Hsla = rgb(0x6272a4).into();
        let sel = e.editor.selection_range();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        // Incremental highlight: only changed lines are re-tokenized; unchanged
        // frames recompute zero. `lines_rc`/`hl_snap` are cheap Rc clones.
        let (lines_rc, hl_snap) = e.highlight_snapshot(&self.theme, &self.syntect_hl);
        let line_count = lines_rc.len();

        // Splice the list to the current line count (cheap; preserves the
        // height cache for unchanged rows) and keep the cursor on-screen when
        // the buffer or caret moved.
        let new_count = line_count.max(1);
        if new_count != e.list_item_count {
            // Line count can shrink (delete) or grow (insert/paste); a plain
            // reset is correct and cheap relative to the per-row work it gates.
            e.list_state.reset(new_count);
            e.list_item_count = new_count;
        }
        let anchor = (edit_seq, cursor_line);
        if e.last_cursor_anchor != Some(anchor) {
            e.last_cursor_anchor = Some(anchor);
            if cursor_line < new_count {
                e.list_state.scroll_to_reveal_item(cursor_line);
            }
        }

        // Owned snapshots for the `'static` per-row render closure — all cheap
        // (Rc pointer clones / Copy / SharedString refcount bumps).
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let text_size = px(14.0 * self.text_scale);

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                let line_chars = line_str.chars().count();
                if let Some((s, e_col)) = line_selection_range(sel, line_idx, line_chars) {
                    if e_col > s {
                        segs = apply_selection_bg(&segs, s, e_col, SELECTION_BG);
                    }
                }
            }

            let gutter = div()
                .w(px(40.0))
                .flex_none()
                .text_color(dim_fg)
                .child(format!("{:>3} ", line_idx + 1));

            let content = build_line_content(
                &segs,
                &line_str,
                line_idx == cursor_line,
                cursor_col,
                mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                &code_font,
                &code_font,
            );

            div().flex().flex_row().child(gutter).child(content).into_any_element()
        };

        div()
            .id("edit-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_2()
            .text_size(text_size)
            .font_family(self.code_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list_state.clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }

    /// Word-Processor view: proportional body font + per-line typographic
    /// styling driven by `classify_wp_line`. Headings get larger sizes and
    /// bold weight; lists/blockquote/code get block-level decorations.
    /// `md_highlight`'s segments still carry inline `**bold**`/`*italic*`
    /// modifiers, which `font_for` maps to FontWeight/FontStyle on render.
    /// No gutter — word processors don't show line numbers.
    fn build_edit_body_wp(&self, e: &mut EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let sel = e.editor.selection_range();
        let mode = e.mode;
        let edit_seq = e.editor.edit_seq();

        // Incremental highlight: only changed lines are re-tokenized; unchanged
        // frames recompute zero. `lines_rc`/`hl_snap` are cheap Rc clones.
        let (lines_rc, hl_snap) = e.highlight_snapshot(&self.theme, &self.syntect_hl);
        let line_count = lines_rc.len();

        // Per-line typographic kind. `classify_wp_line` carries a running fence
        // state, so it must be folded over the buffer in order — but it's a
        // cheap byte scan (no highlighting), and precomputing it lets the
        // virtualized render closure index any visible line directly. Cheaper
        // than the per-row *element layout* the list now skips.
        let mut kinds: Vec<WpLineKind> = Vec::with_capacity(line_count);
        let mut in_fence = false;
        for line_str in lines_rc.iter() {
            let kind = classify_wp_line(line_str, in_fence);
            if matches!(kind, WpLineKind::CodeFence) {
                in_fence = !in_fence;
            }
            kinds.push(kind);
        }

        // Splice the list to line count and keep the cursor visible on edits /
        // motion (mirrors the Code view).
        let new_count = line_count.max(1);
        if new_count != e.list_item_count {
            e.list_state.reset(new_count);
            e.list_item_count = new_count;
        }
        let anchor = (edit_seq, cursor_line);
        if e.last_cursor_anchor != Some(anchor) {
            e.last_cursor_anchor = Some(anchor);
            if cursor_line < new_count {
                e.list_state.scroll_to_reveal_item(cursor_line);
            }
        }

        // Owned snapshots for the `'static` per-row closure.
        let base_style = self.theme.paragraph;
        let lines_snap = lines_rc.clone();
        let hl_snap = hl_snap.clone();
        let kinds = std::rc::Rc::new(kinds);
        let body_font = self.body_font.clone();
        let code_font = self.code_font.clone();
        let editor_fg = self.editor_fg();
        let text_scale = self.text_scale;

        let render_fn = move |line_idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
            let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
            let kind = kinds.get(line_idx).copied().unwrap_or(WpLineKind::Paragraph);

            let mut segs = hl_snap
                .get(line_idx)
                .map(|lh| lh.raw.clone())
                .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
            if let Some(sel) = sel {
                let line_chars = line_str.chars().count();
                if let Some((s, e_col)) = line_selection_range(sel, line_idx, line_chars) {
                    if e_col > s {
                        segs = apply_selection_bg(&segs, s, e_col, SELECTION_BG);
                    }
                }
            }

            // Per-kind typography. Headings get scaled sizes + bold; lists
            // and paragraphs use the body font at the default size; code and
            // tables use monospace.
            let (raw_size_px, font_weight, top_pad) = match kind {
                WpLineKind::Heading(1) => (26.0, FontWeight::BOLD, 10.0),
                WpLineKind::Heading(2) => (22.0, FontWeight::BOLD, 8.0),
                WpLineKind::Heading(3) => (18.0, FontWeight::BOLD, 6.0),
                WpLineKind::Heading(4) => (16.0, FontWeight::BOLD, 5.0),
                WpLineKind::Heading(5) => (15.0, FontWeight::BOLD, 4.0),
                WpLineKind::Heading(_) => (14.0, FontWeight::BOLD, 4.0),
                WpLineKind::CodeFence | WpLineKind::CodeContent => {
                    (13.0, FontWeight::NORMAL, 0.0)
                }
                WpLineKind::TableRow => (13.0, FontWeight::NORMAL, 0.0),
                _ => (14.0, FontWeight::NORMAL, 0.0),
            };
            let text_size_px = raw_size_px * text_scale;
            let line_font = match kind {
                WpLineKind::CodeFence | WpLineKind::CodeContent | WpLineKind::TableRow => {
                    &code_font
                }
                _ => &body_font,
            };

            let content = build_line_content(
                &segs,
                &line_str,
                line_idx == cursor_line,
                cursor_col,
                mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                line_font,
                &code_font,
            );

            // Block-level decoration per kind.
            let line_div = match kind {
                WpLineKind::Blockquote => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad))
                    .italic()
                    .text_color(rgb(0xbfbfbf))
                    .child(div().w(px(3.0)).bg(rgb(0xffb86c)).mr_2())
                    .child(content),
                WpLineKind::CodeFence | WpLineKind::CodeContent => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .px_2()
                    .py_0p5()
                    .bg(rgb(0x21222c))
                    .child(content),
                WpLineKind::Empty => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .h(px(18.0))
                    .child(content),
                _ => div()
                    .flex()
                    .flex_row()
                    .text_size(px(text_size_px))
                    .font_weight(font_weight)
                    .pt(px(top_pad))
                    .child(content),
            };

            line_div.into_any_element()
        };

        div()
            .id("edit-body-wp")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(e.list_state.clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            )
    }

    /// Render a single ACP tool call as a collapsible block. The
    /// expanded body honours a per-tool render policy that mirrors the
    /// Claude Code TUI:
    ///
    /// - `Read` / `Search` / `SwitchMode` and `TodoWrite`: header only.
    ///   The model gets the data; the user only needs to know the action
    ///   happened. Click does nothing for these (no body to expand).
    /// - `Execute` (Bash): show the first 3 lines of output + a "+N more"
    ///   marker — same `tb1 = 3` cap the TUI uses (`Tw9` in cli.js).
    /// - `Fetch`: 10 lines (web fetches are usually short HTML excerpts).
    /// - `Edit` / `Move` / `Delete`: full diff/content — the visible
    ///   change is the whole point.
    /// - `Think` (subagents) / `Other` (MCP tools): full content.
    ///
    // The previous `build_tool_block` / `tool_body_pane` lived here as
    // `&self` methods. They've been replaced by the free-function
    // `build_tool_block_with_weak` / `tool_body_pane_free` further up
    // in the file — necessary so the per-item closure handed to
    // `gpui::list` can construct tool blocks without holding a borrow
    // of `self`.

    /// Render the Claude (ACP) screen. Frozen lines (Claude's prior turns)
    /// get a left bar + dim color; the editable region (the user's pending
    /// draft and any inline replies) renders normally with cursor splice.
    /// Header shows attach status; footer shows mode + send hint + send
    /// state ("…" while a reply is in flight).
    fn render_agent(
        &self,
        root: gpui::Div,
        ring: &mut AgentRing,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Legacy multi-session sidebar removed; the workspace tabs/splits
        // model is the surface for running multiple agents. Sessions
        // within a single ring remain reachable via Ctrl-]/Ctrl-[.

        let active_slot_label = ring.active().label.clone();
        // Per-slot cwd (spec-agent-cwd.md §6). Cloned before the
        // active_mut() reborrow so the Status Strip render can compare
        // against the process cwd without holding two borrows on the ring.
        let active_slot_cwd = ring.active().cwd.clone();
        let c = &mut ring.active_mut().state;

        let cursor = c.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let line_count = c.editor.document().line_count();
        let at = &self.theme.agent; // shorthand for agent theme
        let cursor_color: Hsla = nc(at.cursor);
        let dim_fg: Hsla = nc(at.dim);
        // Frozen Claude prose vs user-authored content get distinct bars so
        // the read/write boundary reads at a glance — same idiom as the
        // rendered-mode focused-block bar.
        let frozen_bar: Hsla = nc(at.frozen_bar);
        let user_bar: Hsla = nc(at.user_bar);
        // Theme-derived background tints for turn cards. Blend a faint
        // tint into the editor background so cards work on any theme.
        let base_bg: Hsla = self.editor_bg();
        let claude_turn_bg: Hsla = nc(at.agent_turn_bg);
        let user_turn_bg: Hsla = nc(at.user_turn_bg);
        let _frozen_fg: Hsla = self.editor_fg();
        let compose_panel_bg: Hsla = tint_bg(base_bg, 0.55, 0.1, 0.03);
        // Compose input text uses the theme's editor foreground so it stays
        // legible against `compose_panel_bg` on light themes (folio, FT,
        // solarized-light) — not the hardcoded Dracula light gray, which
        // vanished into the near-white panel.
        let compose_fg: Hsla = self.editor_fg();

        let perf = perf_enabled();
        // Whole-body timer: covers extract + highlight + gutter tags + block
        // parse + flat_items build + element-tree assembly (everything in
        // render_agent up to the return). GPUI's own layout/paint happens
        // after we return and is not captured here.
        let t_render0 = perf.then(std::time::Instant::now);
        let t_extract0 = perf.then(std::time::Instant::now);
        let edit_seq = c.editor.document().edit_seq();
        // Perf: only re-extract the per-line transcript text when the document
        // actually changed. On cursor-blink / cross-pane notifies edit_seq is
        // unchanged, so reuse the cached Rc verbatim instead of re-allocating a
        // String per line (an O(L) cost that previously ran every frame). The
        // Rc clone below is O(1).
        let lines_rc: std::rc::Rc<Vec<String>> = if c.lines_cache_seq == edit_seq {
            c.lines_cache.clone()
        } else {
            let built: Vec<String> = (0..line_count.max(1))
                .map(|i| {
                    c.editor
                        .document()
                        .line_text(i)
                        .trim_end_matches('\n')
                        .replace('\t', "    ")
                })
                .collect();
            let rc = std::rc::Rc::new(built);
            c.lines_cache = rc.clone();
            c.lines_cache_seq = edit_seq;
            rc
        };
        let lines: &Vec<String> = &lines_rc;
        let t_extract = t_extract0.map(|t| t.elapsed());

        // Per-line highlight, raw + stripped. The incremental cache
        // re-highlights only changed lines and hands back a cheap `Rc`
        // snapshot; the bypass path (SKETCH_HL_CACHE=0) recomputes both
        // passes in full every frame, feeding the identical closure shape so
        // the two paths are directly comparable.
        let t_hl0 = perf.then(std::time::Instant::now);
        let hl_snap: std::rc::Rc<Vec<std::rc::Rc<LineHl>>> = if hl_cache_enabled() {
            c.highlight_cache.snapshot_syn(lines, &self.theme, edit_seq, &self.syntect_hl)
        } else {
            let raw = highlight_markdown_lines_syn(lines, &self.theme, &self.syntect_hl);
            let stripped = highlight_markdown_lines_stripped_syn(lines, &self.theme, &self.syntect_hl);
            std::rc::Rc::new(
                raw.into_iter()
                    .zip(stripped)
                    .map(|(raw, stripped)| std::rc::Rc::new(LineHl { raw, stripped }))
                    .collect(),
            )
        };
        // Stash the per-section timings; the consolidated trace prints at the
        // end of render_agent so we can attribute cost across the whole body.
        let perf_hl_ms = t_hl0.map(|t| t.elapsed().as_secs_f64() * 1e3);
        let perf_extract_ms = t_extract.map(|d| d.as_secs_f64() * 1e3);
        let (perf_recomputed, perf_skip) = if hl_cache_enabled() {
            (c.highlight_cache.last_recomputed, c.highlight_cache.last_was_skip)
        } else {
            (lines.len(), false)
        };
        let perf_lines = lines.len();
        let base_style = self.theme.paragraph;

        // Frozen ranges drive both the structural-block cache and the
        // blank-collapse pass below; resolve once here so they're also
        // available for the view-model fingerprint.
        let frozen_ranges: Vec<(usize, usize)> = c.editor.frozen_lines().to_vec();
        let frozen_line_count: usize = frozen_ranges.iter().map(|(s, e)| e - s).sum();

        // ── View-model memoization (S1) ──────────────────────────────
        // `flat_items` + `gutter_tag_per_line` depend ONLY on these
        // structural inputs — NOT on cursor/selection/theme, which the
        // render closure reads afterward. On cursor-blink / cross-pane
        // notify / the ~1Hz thinking tick these inputs are unchanged, so we
        // reuse the cached `Rc`s and skip the gutter scan, tool-anchor
        // resolution, flat build and blank-collapse pass.
        //
        // Trap check: `ToolCallUpdated` mutates tool-call *content* in
        // `c.tool_calls` without touching `tool_call_order` or `edit_seq`.
        // That content is rendered inside the closure from `tool_calls_snap`,
        // never baked into a `FlatItem` (ToolGroup carries only ids), so it
        // is correctly EXCLUDED from this fingerprint.
        let view_model_fp: u64 = c.view_model_fingerprint(edit_seq, frozen_line_count);

        // On a fingerprint hit `memoize_view_model` returns the cached `Rc`s
        // without invoking the rebuild closure; on a miss it runs the closure,
        // stores the result, stamps `view_model_fp`, and bumps `view_model_seq`.
        let theme_ref = &self.theme;
        let (flat_items_arc, gutter_tag_snap) =
            c.memoize_view_model(view_model_fp, |c| {

        // Per-line gutter tag, sourced from the editor's `TurnId` metadata
        // keyed by `LineAnchor` (spec §11, §E2). Lines without a tag yet
        // (currently-editable, not yet swept by Submit) render as a blank
        // gutter. Lines whose anchor hasn't been allocated count as
        // untagged — happens for editable lines the user just typed.
        // Hoist the metadata view out of the per-line loop: `metadata::<TurnId>()`
        // does a HashMap-by-TypeId lookup and builds a fresh view each call, so
        // calling it once per line was O(n) view constructions per frame. Build
        // it once and reuse it across all lines.
        let gutter_tag_per_line: Vec<Option<TurnId>> = {
            let turn_meta = c.editor.metadata::<TurnId>();
            (0..lines.len())
                .map(|i| {
                    c.editor
                        .anchor_for_line_opt(i)
                        .and_then(|a| turn_meta.get(a).copied())
                })
                .collect()
        };

        // ============ Virtualised list build ============
        //
        // Frozen (agent) content is parsed into RenderedBlocks so that
        // tables, code blocks, headings, and lists display properly.
        // Editable (user) content stays as per-line rendering with
        // cursor/selection support.

        // Build "tool calls anchored at line N" lookup, grouped by
        // anchor line. All calls at the same anchor form one ToolGroup.
        // Anchors are opaque `LineAnchor` ids (spec §E1); resolve via the
        // editor each paint. Anchors whose line got consumed by a delete
        // fall back to EOF so the tool block still renders, just at the
        // tail of the transcript.
        let eof_line = c.editor.document().line_count().saturating_sub(1);
        let mut tools_at_line: std::collections::HashMap<usize, Vec<ToolCallKey>> =
            std::collections::HashMap::new();
        for id in &c.tool_call_order {
            if let Some(&anchor) = c.tool_call_anchor_line.get(id) {
                let line = c.editor.line_for_anchor(anchor).unwrap_or(eof_line);
                tools_at_line.entry(line).or_default().push(id.clone());
            }
        }

        // Detect tables and fenced code blocks in frozen content for
        // block-level rendering. Everything else stays line-by-line.
        // Cached: only re-detect/re-parse when frozen line count changes.
        // (`frozen_ranges` / `frozen_line_count` were resolved above for the
        // view-model fingerprint and are reused here.)
        if frozen_line_count != c.block_cache_frozen_count {
            let block_ranges = detect_block_ranges(lines, &frozen_ranges);
            let mut new_cache: std::collections::HashMap<(usize, usize), RenderedBlock> =
                std::collections::HashMap::new();
            for &(start, end) in &block_ranges {
                // Reuse existing cache entry if the range is unchanged. A
                // range that `parse_block_range` rejects (`FallBackToLines`)
                // gets NO cache entry, so the partition below renders its
                // source lines individually (Finding 10, INV-10).
                if let Some(cached) = c.block_cache.get(&(start, end)) {
                    new_cache.insert((start, end), cached.clone());
                } else if let BlockParse::Parsed(block) =
                    parse_block_range(lines, start, end, theme_ref)
                {
                    new_cache.insert((start, end), block);
                }
            }
            c.block_ranges = block_ranges;
            c.block_cache = new_cache;
            c.block_cache_frozen_count = frozen_line_count;
        }

        // Build the block/line partition from ONE source: `block_cache` holds
        // exactly the ranges that became a `Parsed` block. `block_at_start`
        // (the line that emits a `FlatItem::Block`) and `in_block` (the
        // interior lines that the Block subsumes) are derived from the same
        // iteration, so `in_block` cannot disagree with what was emitted — a
        // detected-but-unparsed range contributes neither, and the flat build
        // falls back to a Line per source line (Finding 10, INV-10).
        let block_ranges = c.block_ranges.clone();
        let mut block_at_start: std::collections::HashMap<usize, RenderedBlock> =
            std::collections::HashMap::new();
        let mut in_block: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        for &(start, end) in &block_ranges {
            if let Some(block) = c.block_cache.get(&(start, end)) {
                block_at_start.insert(start, block.clone());
                for li in start..end {
                    in_block.insert(li);
                }
            }
        }

        // Flat ordering: TurnHeader?, line_0, tool_group_at[0], line_1, …
        // Lines inside a detected block range are replaced by one
        // FlatItem::Block at the range start; interior lines are skipped.
        // TurnHeader items are inserted at turn boundaries (role changes).
        let mut flat_items: Vec<FlatItem> = Vec::with_capacity(lines.len() * 2);
        let mut prev_turn: Option<TurnId> = None;
        for line_idx in 0..lines.len() {
            // Insert a TurnHeader whenever the dominant turn changes.
            let cur_turn = gutter_tag_per_line.get(line_idx).copied().flatten();
            // Tools and sketch-local System notices don't get their own header
            // and don't break the current turn run — a notice landing mid-turn
            // must not re-emit a Claude header (Finding 5, INV-3). The
            // total `HeaderRole::from_turn` returns `None` for those, so the
            // header-owning turn set `{Llm, User}` is enforced by the type
            // rather than an `unreachable!()` arm (Finding 6).
            if let Some(tid) = cur_turn {
                if let Some(role) = HeaderRole::from_turn(tid) {
                    let changed = match prev_turn {
                        Some(prev) => prev != tid,
                        None => true,
                    };
                    if changed {
                        flat_items.push(FlatItem::TurnHeader {
                            role: role.into_turn_role(),
                        });
                        prev_turn = Some(tid);
                    }
                }
            } else if prev_turn.is_some() {
                // Editable (untagged) lines after frozen content = user input.
                // Suppress the "You" header when the editable tail is all
                // blank — in Chatbox mode the compose area is separate, so
                // an empty transcript tail is just whitespace, not a turn.
                let remaining_non_empty = (line_idx..lines.len())
                    .any(|j| !lines[j].trim().is_empty());
                if remaining_non_empty {
                    flat_items.push(FlatItem::TurnHeader {
                        role: TurnRole::User,
                    });
                }
                prev_turn = None;
            }

            if let Some(block) = block_at_start.remove(&line_idx) {
                flat_items.push(FlatItem::Block(block));
            } else if !in_block.contains(&line_idx) {
                flat_items.push(FlatItem::Line(line_idx));
            }
            // Tool groups anchored inside a block range still render.
            if let Some(ids) = tools_at_line.get(&line_idx) {
                flat_items.push(FlatItem::ToolGroup {
                    anchor_line: line_idx,
                    ids: ids.clone(),
                });
            }
        }

        // Collapse blank lines: (a) strip blank frozen (Claude) Lines
        // entirely — they're protocol padding with no visual purpose,
        // (b) strip blank Lines adjacent to ToolGroup / TurnHeader /
        // Block items, and (c) collapse runs of consecutive blank
        // user Lines to at most one.
        {
            let is_blank_line = |item: &FlatItem| -> bool {
                matches!(item, FlatItem::Line(idx) if lines.get(*idx).map_or(true, |s| s.trim().is_empty()))
            };
            let is_frozen_line = |item: &FlatItem| -> bool {
                matches!(item, FlatItem::Line(idx) if frozen_ranges.iter().any(|&(s, e)| *idx >= s && *idx < e))
            };
            let is_structural = |item: &FlatItem| -> bool {
                matches!(
                    item,
                    FlatItem::ToolGroup { .. }
                        | FlatItem::TurnHeader { .. }
                        | FlatItem::Block(_)
                )
            };
            let mut keep = vec![true; flat_items.len()];
            for i in 0..flat_items.len() {
                if !is_blank_line(&flat_items[i]) {
                    continue;
                }
                // Blank frozen lines are always stripped — they're just
                // anchor padding inserted by the ACP splice logic.
                if is_frozen_line(&flat_items[i]) {
                    keep[i] = false;
                    continue;
                }
                // Drop blank line if adjacent to a structural item.
                let adj_structural = (i > 0 && is_structural(&flat_items[i - 1]))
                    || (i + 1 < flat_items.len() && is_structural(&flat_items[i + 1]));
                if adj_structural {
                    keep[i] = false;
                    continue;
                }
                // Collapse consecutive blanks to one.
                if i > 0 && is_blank_line(&flat_items[i - 1]) && keep[i - 1] {
                    keep[i] = false;
                }
            }
            let mut j = 0;
            for i in 0..flat_items.len() {
                if keep[i] {
                    flat_items.swap(i, j);
                    j += 1;
                }
            }
            flat_items.truncate(j);
        }

        // Coalesce a contiguous run of tool calls into ONE collapsible group
        // so a long sequence (grep → grep → edit → read → …) doesn't flood the
        // transcript. The blank anchor lines between adjacent tool calls were
        // already stripped by the blank-collapse pass above, so a run shows up
        // as directly-adjacent `ToolGroup`s; merge their ids into the first.
        // Any prose Line, Block, or TurnHeader between two runs breaks the run
        // (those are real content), so tool calls separated by agent text stay
        // in separate groups. The merged group renders as a typed-count header
        // (e.g. "4 grep, 3 edit, 7 read"), collapsed by default.
        {
            let mut merged: Vec<FlatItem> = Vec::with_capacity(flat_items.len());
            for item in flat_items.drain(..) {
                if let FlatItem::ToolGroup { ids, .. } = &item {
                    if let Some(FlatItem::ToolGroup { ids: prev_ids, .. }) = merged.last_mut() {
                        prev_ids.extend(ids.iter().cloned());
                        continue;
                    }
                }
                merged.push(item);
            }
            flat_items = merged;
        }

        // Thinking indicator at the tail while waiting for Claude.
        if c.turn_phase.is_awaiting() {
            flat_items.push(FlatItem::ThinkingIndicator);
        }

            (flat_items, gutter_tag_per_line)
        });

        // Splice ListState to match new item count. When block ranges
        // are active, line count can shrink unpredictably, so always
        // reset. Otherwise use incremental splice for height cache.
        // (Side-effect — must run EVERY frame, so it lives OUTSIDE the
        // memoized boundary above.)
        let new_count = flat_items_arc.len();
        // Reconcile (count parity → splice/reset) stays count-keyed, but the
        // `(list_state, list_item_count)` mutation is funneled through one
        // mutator so the two can't drift (Finding 8, INV-12).
        c.reconcile_list(new_count);
        // INV-12: after reconcile, the registered count equals what we built.
        debug_assert!(
            c.list_item_count == flat_items_arc.len(),
            "list_item_count ({}) out of sync with flat_items ({})",
            c.list_item_count,
            flat_items_arc.len(),
        );

        // Follow-scroll is SEPARATE from reconcile (F4, INV-13). Re-reveal the
        // tail whenever following AND content grew since the last reveal —
        // keyed on `edit_seq`, NOT on the count delta — so an intra-line chunk
        // (agent prose before a `\n`, a streaming code fence) that bumps the
        // last item's height without adding a row still re-pins the viewport.
        // The pump functions also scroll, but they fire before render so their
        // count is stale; this is the authoritative re-reveal with the fresh
        // post-reconcile count, and also catches unfocused panes that missed
        // the pump's scroll.
        c.reveal_tail_if_following(new_count);

        // Snapshot data for the render closure. Cloned once per
        // render_agent call; the closure is then called only for
        // visible items.
        // O(1) pointer clone — the closure shares the cached line vec for the
        // frame instead of deep-copying every transcript line each render.
        let lines_snap: std::rc::Rc<Vec<String>> = lines_rc.clone();
        // O(1) pointer clone of the per-line highlight snapshot; the closure
        // owns it for the frame and indexes `.raw` / `.stripped` per line.
        let hl_snap = hl_snap;
        // `gutter_tag_snap` and `flat_items_arc` come from the memoized
        // view-model tuple above (cached `Rc`s reused across frames when the
        // structural fingerprint is unchanged).
        let tool_calls_snap = c.tool_calls.clone();
        let expanded_snap = c.expanded_tool_calls.clone();
        let frozen_lines_snap: Vec<(usize, usize)> =
            c.editor.frozen_lines().to_vec();
        let lockable_through_snap = c.editor.lockable_through_line();
        let sel_snap = c.editor.selection_range();
        let mode_snap = c.mode;
        let code_font_snap = self.code_font.clone();
        let body_font_snap = self.body_font.clone();
        let theme_snap = self.theme.clone();
        let at_snap = self.theme.agent.clone();
        let self_editor_fg = self.editor_fg();
        // u32 base colors for `styled_line_element`, which falls back to the
        // base for spans without an explicit fg. Theme-derived so plain
        // editable / frozen text stays legible on light themes (folio, FT)
        // instead of using the hardcoded Dracula `DEFAULT_FG`.
        let editor_fg_u32 = ncolor_to_u32(self.theme.editor_fg, DEFAULT_FG);
        let frozen_fg_u32 = ncolor_to_u32(self.theme.agent.frozen_fg, DEFAULT_FG);
        let turn_started_snap = c.turn_phase.turn_started();
        let last_event_at_snap = c.turn_phase.last_event_at();
        let weak_self = cx.entity().downgrade();

        // Helper closures for frozen-line lookup and "block starts
        // here" gating (used to gate the T-label). Inlined inside the
        // render closure.
        let is_frozen_at = move |line_idx: usize, ranges: &[(usize, usize)]| -> bool {
            ranges.iter().any(|&(s, e)| line_idx >= s && line_idx < e)
        };

        let render_fn = {
            let flat_items = flat_items_arc.clone();
            move |idx: usize, _w: &mut Window, _app: &mut App| -> AnyElement {
                let item = &flat_items[idx];
                match item {
                    FlatItem::Line(line_idx) => {
                        let line_idx = *line_idx;
                        let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
                        let is_frozen = is_frozen_at(line_idx, &frozen_lines_snap);
                        let is_locked = line_idx < lockable_through_snap;
                        let _ = is_locked; // kept for future visual cue parity

                        // md_highlight segments + author tint. Frozen (Claude)
                        // lines use stripped highlights (no raw delimiters);
                        // editable (user) lines use raw highlights.
                        let mut segs: Vec<Segment> = match hl_snap.get(line_idx) {
                            Some(hl) if is_frozen => hl.stripped.clone(),
                            Some(hl) => hl.raw.clone(),
                            None => vec![(line_str.clone(), base_style)],
                        };
                        let author_tint: NColor = if is_frozen {
                            at_snap.agent_tint
                        } else {
                            at_snap.user_tint
                        };
                        for (_text, style) in segs.iter_mut() {
                            if *style == base_style {
                                *style = style.fg(author_tint);
                            }
                        }
                        if let Some(sel) = sel_snap {
                            let line_chars = line_str.chars().count();
                            if let Some((s, e_col)) =
                                line_selection_range(sel, line_idx, line_chars)
                            {
                                if e_col > s {
                                    segs = apply_selection_bg(
                                        &segs, s, e_col, at_snap.selection_bg,
                                    );
                                }
                            }
                        }

                        // Per-line rendering uses monospace (code_font)
                        // for all lines — the token-based flex-wrap in
                        // build_wrapped_line doesn't play well with
                        // proportional fonts. Proportional rendering is
                        // handled by the FlatItem::Block path which uses
                        // body_font through block_inner/doc_styled_line_element.
                        let line_base_fg = if is_frozen {
                            frozen_fg_u32
                        } else {
                            editor_fg_u32
                        };
                        let content = build_wrapped_line(
                            &segs,
                            &line_str,
                            line_idx == cursor_line,
                            cursor_col,
                            mode_snap,
                            cursor_color,
                            base_style,
                            line_base_fg,
                            &code_font_snap,
                        );

                        let line_has_content = !line_str.trim().is_empty();
                        let bar_color: Hsla = if is_frozen {
                            frozen_bar
                        } else if line_has_content {
                            user_bar
                        } else {
                            rgba(0x00000000).into()
                        };
                        let line_text_color = if is_frozen {
                            nc(at_snap.frozen_fg)
                        } else {
                            self_editor_fg
                        };

                        // Gutter tag from the editor's per-line `TurnId`
                        // metadata (spec §11): `N` for LLM lines, `Un`
                        // for user lines, `Tn` for tool-call anchor
                        // lines, blank for currently-editable
                        // (unsubmitted) lines. Only show the label on the
                        // first line of each contiguous turn block.
                        let tag = gutter_tag_snap.get(line_idx).copied().flatten();
                        let prev_tag = if line_idx > 0 {
                            gutter_tag_snap.get(line_idx - 1).copied().flatten()
                        } else {
                            None
                        };
                        let is_first_in_turn = tag != prev_tag;
                        let (label_text, label_color): (SharedString, Hsla) = if !is_first_in_turn {
                            ("   ".into(), dim_fg)
                        } else {
                            match tag {
                                Some(TurnId::Llm(n)) => (
                                    format!("{:>3}", n).into(),
                                    frozen_bar,
                                ),
                                Some(TurnId::User(n)) => (
                                    format!("{:>3}", format!("U{}", n)).into(),
                                    user_bar,
                                ),
                                Some(TurnId::Tool(n)) => (
                                    format!("{:>3}", format!("T{}", n)).into(),
                                    nc(at_snap.tool_label),
                                ),
                                // System notices carry no turn number — blank
                                // gutter, like untagged lines (Finding 5).
                                Some(TurnId::System) | None => ("   ".into(), dim_fg),
                            }
                        };
                        let card_bg: Hsla = match tag {
                            Some(TurnId::Llm(_)) => claude_turn_bg,
                            Some(TurnId::User(_)) => user_turn_bg,
                            // Tool-anchor, System-notice, and untagged lines
                            // float on the base editor_bg — no turn tint
                            // (Constraint 6, Finding 5).
                            Some(TurnId::Tool(_)) | Some(TurnId::System) | None => {
                                rgba(0x00000000).into()
                            }
                        };
                        let row_bg: Hsla = if line_idx == cursor_line {
                            // Blend cursor highlight on top of turn bg.
                            let mut h = nc(at_snap.dim);
                            h.a = 0.2;
                            h
                        } else {
                            card_bg
                        };

                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .py(px(2.0))
                            .bg(row_bg)
                            .text_color(line_text_color)
                            .child(
                                div()
                                    .w(px(28.0))
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(label_color)
                                    .font_family(code_font_snap.clone())
                                    .pr_1()
                                    .child(label_text),
                            )
                            .child(
                                div()
                                    .w(px(3.0))
                                    .flex_none()
                                    .bg(bar_color)
                                    .mr_2(),
                            )
                            .child(content)
                            .into_any_element()
                    }
                    FlatItem::ToolGroup { anchor_line, ids } => {
                        let anchor = *anchor_line;
                        // Collect resolved tool calls for this group.
                        let calls: Vec<&sketch::acp_channel::ToolCall> = ids
                            .iter()
                            .filter_map(|id| tool_calls_snap.get(id))
                            .collect();
                        if calls.is_empty() {
                            return div().h(px(0.0)).into_any_element();
                        }
                        let group_expanded = expanded_snap.contains(&anchor.to_string());
                        let count = calls.len();

                        // Aggregate status for the group header glyph.
                        use sketch::acp_channel::ToolCallStatus;
                        let has_failed = calls.iter().any(|tc| tc.status == ToolCallStatus::Failed);
                        let has_in_progress = calls.iter().any(|tc| tc.status == ToolCallStatus::InProgress);
                        let all_completed = calls.iter().all(|tc| tc.status == ToolCallStatus::Completed);
                        let (group_glyph, group_color): (&str, Hsla) = if has_failed {
                            ("✗", nc(at_snap.tool_failed))
                        } else if has_in_progress {
                            ("◐", nc(at_snap.tool_in_progress))
                        } else if all_completed {
                            ("●", nc(at_snap.tool_completed))
                        } else {
                            ("○", nc(at_snap.tool_pending))
                        };

                        let header_title: String = if count == 1 {
                            let tc = calls[0];
                            let base = if tc.title.is_empty() { "(tool)".to_string() } else { tc.title.clone() };
                            // Append a useful detail for single-tool groups so
                            // the user doesn't need to expand to see *what* was
                            // read/edited/executed.
                            if let Some(detail) = tool_inline_detail(tc) {
                                format!("{} {}", base, detail)
                            } else {
                                base
                            }
                        } else {
                            // Typed summary of the run: count each tool label in
                            // first-appearance order → "4 grep, 3 edit, 7 read".
                            let mut order: Vec<String> = Vec::new();
                            let mut counts: std::collections::HashMap<String, usize> =
                                std::collections::HashMap::new();
                            for tc in &calls {
                                let label = tool_type_label(tc);
                                if !counts.contains_key(&label) {
                                    order.push(label.clone());
                                }
                                *counts.entry(label).or_insert(0) += 1;
                            }
                            order
                                .iter()
                                .map(|l| format!("{} {}", counts[l], l))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };

                        // For single-tool groups: determine if the inner tool
                        // has a body worth showing. If HeaderOnly, the header
                        // line is the entire UI — no expand arrow, no nesting.
                        let single_policy = if count == 1 {
                            Some(tool_render_policy(calls[0]))
                        } else {
                            None
                        };
                        let expandable = if count > 1 {
                            true
                        } else {
                            !matches!(single_policy, Some(ToolRenderPolicy::HeaderOnly))
                        };
                        let arrow = if !expandable {
                            " "
                        } else if group_expanded { "▼" } else { "▶" };

                        let anchor_str = anchor.to_string();
                        let weak = weak_self.clone();
                        let click_id = anchor_str.clone();
                        let mut header_row = div()
                            .id(SharedString::from(format!("tool-group-{}", anchor)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py(px(6.0))
                            .px_2()
                            .child(div().text_color(dim_fg).child(arrow))
                            .child(div().text_color(group_color).child(group_glyph))
                            .child(div().flex_1().text_color(self_editor_fg).text_size(px(12.0)).child(header_title));

                        if expandable {
                            header_row = header_row.cursor_pointer().on_click(
                                move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                                    let id = click_id.clone();
                                    let _ = weak.update(app, |this, cx| {
                                        if let Some(c) = this.agent_mut() {
                                            if c.expanded_tool_calls.contains(&id) {
                                                c.expanded_tool_calls.remove(&id);
                                            } else {
                                                c.expanded_tool_calls.insert(id);
                                            }
                                        }
                                        cx.notify();
                                    });
                                },
                            );
                        }

                        let mut block = div()
                            .flex()
                            .flex_col()
                            .mt(px(16.0))
                            .mb(px(8.0))
                            .mx_4()
                            .child(header_row);

                        // Expanded: show contents.
                        if group_expanded && expandable {
                            if count == 1 {
                                // Single-tool group: render body directly
                                // under the header — no nested sub-header.
                                let tc = calls[0];
                                block = append_tool_body(
                                    block,
                                    tc,
                                    single_policy.unwrap_or(ToolRenderPolicy::Full),
                                    &code_font_snap,
                                    &at_snap,
                                );
                            } else {
                                for tc in &calls {
                                    let expanded_detail = expanded_snap.contains(&tc.tool_call_id.0.to_string());
                                    block = block.child(
                                        build_tool_block_with_weak(
                                            tc,
                                            expanded_detail,
                                            &code_font_snap,
                                            weak_self.clone(),
                                            &at_snap,
                                        ),
                                    );
                                }
                            }
                        }

                        block.into_any_element()
                    }
                    FlatItem::Block(rendered_block) => {
                        let ctx = RenderCtx {
                            theme: &theme_snap,
                            body_font: body_font_snap.clone(),
                            code_font: code_font_snap.clone(),
                            // Claude session chat blocks stay at fixed size —
                            // Cmd-zoom is scoped to the document view.
                            text_scale: 1.0,
                            cursor_block: None,
                            doc_selection: None,
                            line_layouts: None,
                            current_block: None,
                            // Wiki link clicks in Claude messages aren't
                            // wired up — they'd need a per-message source
                            // path which we don't track. Skip for v1.
                            weak_view: None,
                            doc_dir: None,
                        };
                        let inner = block_inner(&ctx, rendered_block);
                        div()
                            .mt(px(4.0))
                            .mb(px(4.0))
                            .child(inner)
                            .into_any_element()
                    }
                    FlatItem::TurnHeader { role } => {
                        let (label, accent): (&str, Hsla) = match role {
                            TurnRole::Claude => ("Claude", nc(at_snap.turn_header_agent)),
                            TurnRole::User => ("You", nc(at_snap.turn_header_user)),
                        };
                        let rule_color = nc(at_snap.turn_rule);
                        // TurnHeaders float on editor_bg — no turn tint
                        // (Constraint 6). The neutral gap between tinted
                        // text bands is the visual separator.
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .w_full()
                            .pt(px(32.0))
                            .pb(px(8.0))
                            .px_4()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(accent)
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.0))
                                    .bg(rule_color),
                            )
                            .into_any_element()
                    }
                    FlatItem::ThinkingIndicator => {
                        // Pulsing dot: opacity cycles 0.3–1.0 on a sine wave.
                        let phase = if let Some(t) = turn_started_snap {
                            let ms = t.elapsed().as_millis() as f64;
                            ((ms / 750.0).sin() * 0.5 + 0.5) as f32
                        } else {
                            1.0
                        };
                        let alpha = 0.3 + phase * 0.7;

                        // Live elapsed (since the prompt was sent) and quiet
                        // time (since the last streamed event). A streaming
                        // turn keeps `quiet` near zero; a stall lets it climb,
                        // which is the tell that the API — not sketch — is
                        // wedged. Past STALL_WARN_S we switch to an explicit
                        // warning so the user knows it's abnormal.
                        const STALL_WARN_S: u64 = 30;
                        let elapsed_s =
                            turn_started_snap.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        let quiet_s =
                            last_event_at_snap.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        let fmt_ms = |s: u64| format!("{}:{:02}", s / 60, s % 60);
                        let stalled = quiet_s >= STALL_WARN_S;

                        let dot_color = if stalled {
                            // Amber when stalled, regardless of pulse phase.
                            nc(at_snap.warm_accent)
                        } else {
                            Hsla { h: 0.53, s: 0.9, l: 0.76, a: alpha }
                        };
                        let (label, label_color) = if stalled {
                            (
                                format!(
                                    "No reply for {} (running {}) — the API may be overloaded. ⌘. to stop · ⌘. again to force-restart",
                                    fmt_ms(quiet_s),
                                    fmt_ms(elapsed_s),
                                ),
                                nc(at_snap.warm_accent),
                            )
                        } else {
                            (
                                format!("Thinking\u{2026} {}", fmt_ms(elapsed_s)),
                                Hsla { h: 0.0, s: 0.0, l: 0.6, a: alpha },
                            )
                        };
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .pt_3()
                            .pb_2()
                            .pl_1()
                            .pr_4()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(dot_color)
                                    .child("●"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.0))
                                    .text_color(label_color)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .into_any_element()
                    }
                }
            }
        };

        let body = div()
            .id("claude-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_3()
            .text_size(px(13.0))
            .font_family(self.code_font.clone())
            .text_color(self.editor_fg())
            .child(
                gpui::list(c.list_state.clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            );

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        // ---- Status Strip (spec §30) ----
        // Single-row header showing agent label, sub-agent breadcrumb
        // (when focused), model id, permission mode, context-window
        // usage + cost (when present), and turn / elapsed. Any field
        // whose underlying signal is absent renders nothing — no
        // placeholder, no `?`. The strip is at most as wide as the
        // data it has.
        let strip_dim: Hsla = nc(at.dim);
        let strip_warm: Hsla = nc(at.warm_accent);
        let strip_fg = fg_or(top, STATUS_FG);

        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(strip_fg)
            .font_weight(FontWeight::BOLD)
            .text_size(px(12.0));

        // Agent label (slot label).
        strip = strip.child(
            div()
                .pr_2()
                .child(SharedString::from(active_slot_label.clone())),
        );

        // Session-server indicator.
        if c.server_managed {
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::new_static("server")),
            );
        }

        // Sub-agent breadcrumb (only when focused).
        if let Some(key) = c.focused_subagent.as_ref() {
            if let Some(sa) = c.tool_calls.get(key).and_then(classify_subagent) {
                let crumb = format!(" ⏵ {} ◂", sa.label);
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_warm)
                        .child(SharedString::from(crumb)),
                );
            }
        }

        // Per-slot cwd (spec-agent-cwd.md §6). Hidden when the slot cwd
        // matches the process cwd — surfacing the implicit default on
        // every session is noise. Tooltip with the absolute path is a
        // follow-up (GPUI tooltip support is patchy on this version);
        // for now the shortened display is the only affordance.
        let proc_cwd = process_cwd();
        if active_slot_cwd != proc_cwd {
            let shortened = shorten_cwd_for_display(&active_slot_cwd);
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(shortened)),
            );
        }

        // Model id (best-effort: agent_mode → channel description).
        let model_label: Option<String> = c
            .agent_mode
            .as_ref()
            .map(|m| m.0.to_string())
            .or_else(|| {
                c.channel.as_ref().map(|ch| ch.command().to_string())
            });
        if let Some(m) = model_label {
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(m)),
            );
        }

        // Permission mode.
        if let Some(ch) = &c.channel {
            let mode_str = ch.permission_mode().short_label();
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(mode_str.to_string())),
            );
        }

        // Context-window usage + cost (when the unstable feature is on
        // and the agent has emitted a UsageUpdate).
        if let Some(usage) = &c.usage {
            let used_k = (usage.tokens_used as f64) / 1000.0;
            let total_k = (usage.tokens_total as f64) / 1000.0;
            let pct = if usage.tokens_total > 0 {
                (usage.tokens_used as f64 / usage.tokens_total as f64) * 100.0
            } else {
                0.0
            };
            let usage_text = format!(
                "{:.1}k / {:.0}k ({:.0}%)",
                used_k, total_k, pct
            );
            strip = strip.child(
                div()
                    .pr_2()
                    .text_color(strip_dim)
                    .child(SharedString::from(usage_text)),
            );
            if let Some(cost) = usage.cost_usd {
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_dim)
                        .child(SharedString::from(format!("${:.2}", cost))),
                );
            }
        }

        // Turn / elapsed. Show "turn N · M:SS" when a turn has run; "turn
        // N" alone if no timer is active; nothing if no turns have run.
        let completed_turns = c
            .channel
            .as_ref()
            .map(|ch| ch.turn_count())
            .unwrap_or(0);
        let display_turn = if c.turn_phase.is_awaiting() {
            completed_turns + 1
        } else {
            completed_turns
        };
        let turn_started = c.turn_phase.turn_started();
        if display_turn > 0 || turn_started.is_some() {
            let elapsed_str = if let Some(t) = turn_started {
                let s = t.elapsed().as_secs();
                format!("{}:{:02}", s / 60, s % 60)
            } else {
                String::new()
            };
            let turn_color = if turn_started.is_some() {
                strip_warm
            } else {
                strip_dim
            };
            let label = if elapsed_str.is_empty() {
                format!("turn {}", display_turn)
            } else {
                format!("turn {} · {}", display_turn, elapsed_str)
            };
            strip = strip
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(turn_color)
                        .child(SharedString::from(label)),
                );
        }

        let header = strip;

        // ---- Agent Info Bar ----
        // Dedicated status bar showing context-window size, cwd, and active
        // subagents. Position (top/bottom) is a user preference.
        let info_bar = {
            use sketch::acp_channel::ToolCallStatus;

            // Context window segment.
            let ctx_text: String = if let Some(usage) = &c.usage {
                let used_k = (usage.tokens_used as f64) / 1000.0;
                let total_k = (usage.tokens_total as f64) / 1000.0;
                let pct = if usage.tokens_total > 0 {
                    (usage.tokens_used as f64 / usage.tokens_total as f64) * 100.0
                } else {
                    0.0
                };
                format!("{:.1}k / {:.0}k ({:.0}%)", used_k, total_k, pct)
            } else {
                "\u{2014}".to_string()
            };

            // Cwd segment — always shown.
            let cwd_text = shorten_cwd_for_display(&active_slot_cwd);

            // Subagents segment — show in-progress agents with glyphs.
            let agents_text: String = {
                let active: Vec<String> = c
                    .subagents()
                    .iter()
                    .filter(|sa| {
                        matches!(
                            sa.status,
                            ToolCallStatus::InProgress | ToolCallStatus::Pending
                        )
                    })
                    .map(|sa| {
                        let glyph = match sa.status {
                            ToolCallStatus::InProgress => "\u{25d0}",
                            ToolCallStatus::Pending => "\u{25cb}",
                            _ => "\u{00b7}",
                        };
                        let label: String = if sa.label.chars().count() > 16 {
                            let head: String = sa.label.chars().take(15).collect();
                            format!("{}\u{2026}", head)
                        } else {
                            sa.label.clone()
                        };
                        format!("{}{}", glyph, label)
                    })
                    .collect();
                if active.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    active.join("  ")
                }
            };

            let sep_color: Hsla = nc(at.turn_rule);

            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .py_1()
                .h(px(22.0))
                .bg(bg_or(bot, STATUS_BG))
                .text_color(fg_or(bot, 0x666666))
                .text_size(px(11.0))
                .font_family(self.code_font.clone())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("ctx")),
                        )
                        .child(SharedString::from(ctx_text)),
                )
                .child(
                    div()
                        .text_color(sep_color)
                        .child(SharedString::new_static("\u{00b7}")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("cwd")),
                        )
                        .child(SharedString::from(cwd_text)),
                )
                .child(
                    div()
                        .text_color(sep_color)
                        .child(SharedString::new_static("\u{00b7}")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(strip_dim)
                                .child(SharedString::new_static("agents")),
                        )
                        .child(SharedString::from(agents_text)),
                )
        };

        let in_chatbox = c.input_surface.is_chatbox();
        let mode_label = if in_chatbox {
            match c.input_surface.chatbox().unwrap().mode {
                EditMode::Normal => "CHATBOX",
                EditMode::Insert => "CHATBOX INSERT",
            }
        } else {
            match c.mode {
                EditMode::Normal => "WORKSHEET",
                EditMode::Insert => "WORKSHEET INSERT",
            }
        };
        let dirty_mark = if c.editor.document().is_modified() { "•" } else { " " };
        let extend_mark = if c.editor.extend_mode() { " EXT" } else { "" };
        let mut left_status = format!(
            "{} CLAUDE {}{} · L{}:C{}",
            dirty_mark,
            mode_label,
            extend_mark,
            cursor_line + 1,
            cursor_col + 1,
        );
        if c.turn_phase.is_awaiting() {
            left_status.push_str(" · …awaiting reply");
        }
        if let Some(msg) = &c.status {
            left_status.push_str("  [");
            left_status.push_str(msg);
            left_status.push(']');
        }
        // dim_fg is now used actively via agent theme

        let hints = if in_chatbox {
            "Ctrl-Enter send · Ctrl-Alt-Enter worksheet · esc normal"
        } else {
            "Ctrl-Enter send · Ctrl-Alt-Enter chatbox · Ctrl-V back · i insert · esc normal"
        };

        // Right side of the footer: a Stop button (only while a reply is in
        // flight) followed by the key hints. The button dispatches the same
        // StopAgent path as Cmd-. — ACP session/cancel for the active turn.
        let stop_fg: Hsla = nc(at.tool_failed);
        let mut footer_right = div().flex().flex_row().items_center().gap_2();
        if c.turn_phase.is_awaiting() {
            // After a graceful cancel is already pending, the button (and
            // ⌘.) escalate to a hard kill + resume.
            let escalating = c.turn_phase.stop_requested();
            let stop_label = if escalating {
                "■ Force-restart ⌘."
            } else {
                "■ Stop ⌘."
            };
            let weak_stop = cx.entity().downgrade();
            footer_right = footer_right.child(
                div()
                    .id("agent-stop-btn")
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_2()
                    .py(px(1.0))
                    .rounded_md()
                    .border_1()
                    .border_color(stop_fg)
                    .text_color(stop_fg)
                    .cursor_pointer()
                    .on_click(move |_ev: &gpui::ClickEvent, window: &mut Window, app: &mut App| {
                        let _ = weak_stop.update(app, |this, cx| {
                            this.stop_agent(&StopAgent, window, cx);
                        });
                    })
                    .child(SharedString::from(stop_label)),
            );
        }
        footer_right = footer_right.child(SharedString::from(hints));

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(bg_or(bot, STATUS_BG))
            .text_color(fg_or(bot, 0x666666))
            .text_size(px(11.0))
            .child(left_status)
            .child(footer_right);

        // Chatbox panel — rendered between body and footer when active.
        //
        // Each line is rendered as a non-wrapping row inside a per-line
        // overflow_hidden clip container. The cursor line is shifted left
        // via a negative pixel margin so the caret stays visible. The clip
        // container inherits its width from the flex layout — no need to
        // know the pixel width at render time.
        let compose_panel = if let InputSurface::Chatbox(tb) = &mut c.input_surface {
            let compose_lines: Vec<String> = {
                let doc = tb.editor.document();
                (0..doc.line_count().max(1))
                    .map(|i| {
                        doc.line_text(i)
                            .trim_end_matches('\n')
                            .replace('\t', "    ")
                    })
                    .collect()
            };
            let compose_cursor_line = tb.editor.cursor().line;
            let compose_cursor_col = tb.editor.cursor().col;
            let compose_mode = tb.mode;
            let compose_sel = tb.editor.selection_range();
            let sep_color: Hsla = nc(at.compose_separator);
            let compose_cursor_color: Hsla = nc(at.cursor);
            let compose_code_font = self.code_font.clone();

            let separator = div()
                .w_full()
                .h(px(1.0))
                .bg(dim_fg);

            // Cap height at ~8 logical lines, then vertical scroll kicks in.
            // Wrapped lines may exceed one row visually, so the actual cap
            // can show fewer logical lines when text wraps — that's fine,
            // overflow_y_scroll handles it.
            let max_visible_h = 8.0 * 18.0f32;

            let compose_scroll = tb.scroll_handle.clone();
            // scroll_to_item only sees direct children of the scroll
            // container, so each logical line is added straight to
            // `compose_body` (no intermediate wrapper) — that's what keeps
            // the cursor in view when the user types past the visible area.
            compose_scroll.scroll_to_item(compose_cursor_line);

            let mut compose_body = div()
                .id("compose-scroll")
                .w_full()
                .min_w_0()
                .max_h(px(max_visible_h))
                .overflow_y_scroll()
                .overflow_x_hidden()
                .track_scroll(&compose_scroll)
                .px_4()
                .py(px(8.0))
                .bg(compose_panel_bg)
                .border_1()
                .border_color(dim_fg)
                .rounded_md()
                .mx_2()
                .mb_1()
                .font_family(compose_code_font.clone())
                .text_size(px(13.0))
                .text_color(compose_fg);

            for (i, line_text) in compose_lines.iter().enumerate() {
                let is_cursor_line = i == compose_cursor_line;
                let total_chars = line_text.chars().count();
                let line_el = build_chatbox_line(
                    line_text,
                    is_cursor_line,
                    compose_cursor_col,
                    compose_mode,
                    compose_cursor_color,
                    compose_sel,
                    i,
                    total_chars,
                    &compose_code_font,
                    compose_fg,
                );
                compose_body = compose_body.child(line_el);
            }

            // Top edge: a 1px darker rule creates a subtle
            // visual separation between the scrolling transcript
            // and the fixed compose panel.
            let edge_color = {
                let mut h = sep_color;
                h.a = 0.4;
                h
            };
            Some(
                div()
                    .w_full()
                    .min_w_0()
                    .border_t_1()
                    .border_color(edge_color)
                    .child(separator)
                    .child(compose_body),
            )
        } else {
            None
        };

        // ---- Right-side sidepanes (Tasklist / Subagents) ----
        //
        // Stacked horizontally in fixed order (Tasklist innermost, then
        // Subagents) per spec §2. Each pane is a fixed 28-char column;
        // the transcript area's flex-1 shrinks to make room. Panes only
        // render when their `*_open` flag is true.
        let pane_width = px(28.0 * 7.0); // ~28 monospace cols at 13px = ~196px
        let pane_border: Hsla = nc(at.pane_border);
        let pane_header_fg: Hsla = nc(at.pane_header);
        let pane_dim_fg: Hsla = nc(at.dim);
        let pane_bg: Hsla = nc(at.pane_bg);

        let tasklist_pane = if c.tasklist_open {
            let mut pane = div()
                .id("tasklist-pane")
                .flex()
                .flex_col()
                .w(pane_width)
                .min_w(pane_width)
                .flex_none()
                .bg(pane_bg)
                .border_l_1()
                .border_color(pane_border)
                .py_1()
                .text_size(px(12.0))
                .font_family(self.code_font.clone());
            pane = pane.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(pane_header_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("Plan")),
            );
            match &c.current_plan {
                Some(plan) if !plan.entries.is_empty() => {
                    use sketch::acp_channel::PlanEntryStatus;
                    for entry in &plan.entries {
                        let glyph: &'static str = match entry.status {
                            PlanEntryStatus::Completed => "✓",
                            PlanEntryStatus::InProgress => "●",
                            PlanEntryStatus::Pending => "○",
                            // ACP marks the enum #[non_exhaustive]; a
                            // future "failed" or similar status falls
                            // back to a clear indicator (§22).
                            _ => "✗",
                        };
                        let line_text = if entry.content.chars().count() > 22 {
                            let truncated: String =
                                entry.content.chars().take(21).collect();
                            format!("{}  {}…", glyph, truncated)
                        } else {
                            format!("{}  {}", glyph, entry.content)
                        };
                        pane = pane.child(
                            div()
                                .px_2()
                                .py(px(1.0))
                                .text_color(rgb(DEFAULT_FG))
                                .child(SharedString::from(line_text)),
                        );
                    }
                }
                _ => {
                    pane = pane.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(pane_dim_fg)
                            .child(SharedString::new_static("(no plan)")),
                    );
                }
            }
            Some(pane)
        } else {
            None
        };

        let subagents_pane = if c.subagents_open {
            let mut pane = div()
                .id("subagents-pane")
                .flex()
                .flex_col()
                .w(pane_width)
                .min_w(pane_width)
                .flex_none()
                .bg(pane_bg)
                .border_l_1()
                .border_color(pane_border)
                .py_1()
                .text_size(px(12.0))
                .font_family(self.code_font.clone());
            pane = pane.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(pane_header_fg)
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static("Subagents")),
            );
            let subagents = c.subagents();
            if subagents.is_empty() {
                pane = pane.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(pane_dim_fg)
                        .child(SharedString::new_static("(no subagents)")),
                );
            } else {
                use sketch::acp_channel::ToolCallStatus;
                let focused_key = c.focused_subagent.clone();
                for (i, sa) in subagents.iter().enumerate() {
                    let glyph: &'static str = match sa.status {
                        ToolCallStatus::Completed => "✓",
                        ToolCallStatus::Failed => "✗",
                        ToolCallStatus::InProgress => "●",
                        ToolCallStatus::Pending => "○",
                        _ => "·",
                    };
                    let trunc_label: String = if sa.label.chars().count() > 20 {
                        let head: String = sa.label.chars().take(19).collect();
                        format!("{}…", head)
                    } else {
                        sa.label.clone()
                    };
                    let row_text = format!("▸ {} {}", glyph, trunc_label);
                    let is_focused = focused_key.as_ref() == Some(&sa.tool_call_id);
                    let row_fg: Hsla = if is_focused {
                        nc(at.warm_accent)
                    } else {
                        self.editor_fg()
                    };
                    let row_bg: Hsla = if is_focused {
                        let mut h = nc(at.dim);
                        h.a = 0.2;
                        h
                    } else {
                        rgba(0x00000000).into()
                    };
                    let weak = cx.entity().downgrade();
                    let row_key = sa.tool_call_id.clone();
                    let row = div()
                        .id(SharedString::from(format!("subagent-row-{}", i)))
                        .px_2()
                        .py(px(1.0))
                        .cursor_pointer()
                        .text_color(row_fg)
                        .bg(row_bg)
                        .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                            let key = row_key.clone();
                            let _ = weak.update(app, |this, cx| {
                                this.focus_subagent(key, cx);
                            });
                        })
                        .child(SharedString::from(row_text));
                    pane = pane.child(row);
                }
            }
            Some(pane)
        } else {
            None
        };

        let mut transcript_row = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(body),
            );
        if let Some(p) = tasklist_pane {
            transcript_row = transcript_row.child(p);
        }
        if let Some(p) = subagents_pane {
            transcript_row = transcript_row.child(p);
        }

        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(transcript_row);
        if let Some(panel) = compose_panel {
            col = col.child(panel);
        }
        let content_area: gpui::AnyElement = col.into_any_element();

        // Build-loop candidate banner. Sits above the status strip so a
        // read-only mirror is unmistakable. Amber while the original owner
        // still holds the sessions; green once it has closed and take-over
        // will succeed.
        let candidate_banner = if self.is_candidate {
            let (bar_bg, text): (Hsla, &'static str) = if self.candidate_promote_ready {
                (
                    rgb(0x50fa7b).into(),
                    "✓ CANDIDATE · original closed — menu → claude → take over (P) to go live",
                )
            } else {
                (
                    rgb(0xffb86c).into(),
                    "🔭 CANDIDATE · read-only mirror — close the original window, then menu → claude → take over (P)",
                )
            };
            Some(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px_4()
                    .py_1()
                    .h(px(24.0))
                    .bg(bar_bg)
                    .text_color(rgb(0x1e1e2e))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::new_static(text)),
            )
        } else {
            None
        };

        let mut root = root
            .key_context("AgentView")
            .on_key_down(cx.listener(Self::handle_claude_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(|this, _: &ToggleTasklist, _w, cx| {
                this.toggle_tasklist(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSubagents, _w, cx| {
                this.toggle_subagents(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleAgentInputMode, _w, cx| {
                this.toggle_agent_input_mode(cx);
            }))
            .on_action(cx.listener(Self::stop_agent))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side));
        if let Some(banner) = candidate_banner {
            root = root.child(banner);
        }
        let out = match self.agent_status_position {
            AgentStatusPosition::Top => root
                .child(header)
                .child(info_bar)
                .child(content_area)
                .child(footer),
            AgentStatusPosition::Bottom => root
                .child(header)
                .child(content_area)
                .child(info_bar)
                .child(footer),
        };

        if let Some(t0) = t_render0 {
            let total_ms = t0.elapsed().as_secs_f64() * 1e3;
            let extract_ms = perf_extract_ms.unwrap_or(0.0);
            let hl_ms = perf_hl_ms.unwrap_or(0.0);
            // `rest` = the untimed remainder inside render_agent: gutter tags,
            // block detect/parse, flat_items build, element-tree assembly.
            // If total is large but extract+hl are small, the cost is here
            // (or in GPUI layout after we return — not captured).
            let rest_ms = (total_ms - extract_ms - hl_ms).max(0.0);
            eprintln!(
                "[perf] agent-render lines={perf_lines} total={total_ms:.2}ms \
                 extract={extract_ms:.2}ms hl={hl_ms:.2}ms rest={rest_ms:.2}ms \
                 recomputed={perf_recomputed} skip={perf_skip} cache={}",
                if hl_cache_enabled() { "on" } else { "off" },
            );
        }
        out
    }

    fn render_browser(
        &self,
        root: gpui::Div,
        b: &BrowserWindow,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ov = &self.theme.overlay;

        // ── Worktree-mode overlay ──────────────────────────────────
        let (header, list, hint) = if let Some(wm) = &b.fb.worktree_mode {
            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(28.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.accent))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::new_static("WORKTREES"));

            let mut list = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .text_size(px(13.0))
                .font_family(self.body_font.clone());

            if wm.worktrees.is_empty() {
                list = list.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(nc(ov.label))
                        .child(SharedString::new_static("  (no worktrees)")),
                );
            } else {
                let visible_rows = 28usize;
                let scroll = scroll_to_keep_visible(wm.selected, visible_rows, wm.worktrees.len());
                for (i, wt) in wm.worktrees.iter().enumerate().skip(scroll).take(visible_rows) {
                    list = list.child(worktree_row(wt, i == wm.selected, ov));
                }
            }

            let hint = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(22.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.label))
                .text_size(px(11.0))
                .child(SharedString::new_static(
                    "enter:switch · w:close · esc:cancel",
                ));

            (header, list, hint)
        } else {
            // ── Normal file-browser view ───────────────────────────────
            let entries: Vec<&BrowserEntry> = b.fb.visible_entries();
            let selected = b.fb.selected();
            let dir_str = b.fb.current_dir().display().to_string();

            let header_text = if b.fb.filter_mode {
                format!("▸ {} — /{}", dir_str, b.fb.filter_text())
            } else {
                format!("▸ {}", dir_str)
            };

            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .h(px(28.0))
                .bg(nc(ov.bg))
                .text_color(nc(ov.accent))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(header_text));

            let mut list = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .text_size(px(13.0))
                .font_family(self.body_font.clone());

            if b.fb.filter_mode {
                // Show filter input bar
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py_1()
                        .bg(nc(ov.selected_bg))
                        .text_color(nc(ov.input))
                        .child(SharedString::from(format!("/ {}\u{2588}", b.fb.filter_text()))),
                );
            }

            if entries.is_empty() {
                let msg = if b.fb.filter_mode { "  (no matches)" } else { "  (empty)" };
                list = list.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(nc(ov.label))
                        .child(SharedString::new_static(msg)),
                );
            } else {
                let visible_rows = 80usize;
                let scroll = scroll_to_keep_visible(selected, visible_rows, entries.len());
                for (i, entry) in entries.iter().enumerate().skip(scroll).take(visible_rows) {
                    list = list.child(browser_row(entry, i == selected, &self.code_font, ov));
                }
            }

            let hint = if b.fb.filter_mode {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_1()
                    .h(px(22.0))
                    .bg(nc(ov.bg))
                    .text_color(nc(ov.label))
                    .text_size(px(11.0))
                    .child(SharedString::new_static(
                        "enter:open · esc:cancel · type to filter",
                    ))
            } else {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_1()
                    .h(px(22.0))
                    .bg(nc(ov.bg))
                    .text_color(nc(ov.label))
                    .text_size(px(11.0))
                    .child(format!(
                        "enter:open · -:parent · /:filter · .:hidden · s:sort({}) · w:wt · q:close",
                        b.fb.sort_order.label()
                    ))
            };

            (header, list, hint)
        };

        root.key_context("BrowserView")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_browser_filter_key(ev, w, cx);
            }))
            .on_action(cx.listener(Self::browser_down))
            .on_action(cx.listener(Self::browser_up))
            .on_action(cx.listener(Self::browser_enter))
            .on_action(cx.listener(Self::browser_parent))
            .on_action(cx.listener(Self::browser_toggle_hidden))
            .on_action(cx.listener(Self::browser_cycle_sort))
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::browser_close))
            .on_action(cx.listener(Self::browser_worktrees))
            .on_action(cx.listener(Self::browser_filter))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::move_pane))
            .on_action(cx.listener(Self::also_show_pane))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .child(header)
            .child(list)
            .child(hint)
    }
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
    selected.saturating_sub(half).min(total.saturating_sub(rows))
}

/// One row in the file-browser list.
fn browser_row(entry: &BrowserEntry, selected: bool, code_font: &SharedString, ov: &OverlayTheme) -> AnyElement {
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
    let row_bg = if selected { nc(ov.selected_bg) } else { nc(ov.bg) };
    let marker = if wt.is_current { "* " } else if selected { "▸ " } else { "  " };
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
        WindowContent::Doc(d) => d
            .edit_cache
            .as_ref()
            .map_or(false, |ec| ec.editor.is_modified()),
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
        KeyBinding::new(".", BrowserToggleHidden, Some("BrowserView")),
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
    let initial_doc: Option<(Vec<RenderedBlock>, String)> = match args.get(1) {
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
            Some((blocks, canon))
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
        let window_handle = app.open_window(
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
                        Some((blocks, canon)) => SketchGpuiView::new_doc(
                            blocks,
                            theme.clone(),
                            canon,
                            focus_handle,
                        ),
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
mod tests {
    use super::*;

    /// ADR-0010: the canonical on-disk cwd key resolves a symlinked spelling
    /// and the real path to the SAME string (so a session saved under one is
    /// found when launched under the other), and falls back to the raw spelling
    /// when the path can't be canonicalized (never regresses to never-matching).
    #[test]
    fn persist_cwd_key_canonicalizes_symlinks() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("sketch-cwdkey-{}", std::process::id()));
        let real = base.join("real");
        let link = base.join("link");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        assert_eq!(
            persist_cwd_key(&link),
            persist_cwd_key(&real),
            "symlinked and real cwd must share one on-disk key"
        );
        assert_ne!(
            persist_cwd_key(&link),
            link.to_string_lossy(),
            "the key must be canonicalized, not the raw symlink spelling"
        );

        // Non-existent path: canonicalize fails -> echo raw (no never-match).
        let missing = base.join("does-not-exist");
        assert_eq!(persist_cwd_key(&missing), missing.to_string_lossy());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Settings persistence round-trips (theme, agent bar, text zoom) and a
    /// preferences file written before `text_scale` existed still loads — the
    /// `#[serde(default)]` keeps it forward-compatible (no panic, zoom = None).
    #[test]
    fn preferences_round_trip_with_text_scale() {
        let prefs = Preferences {
            theme: Some("dracula".into()),
            agent_status_position: Some("top".into()),
            text_scale: Some(1.21),
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: Preferences = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme.as_deref(), Some("dracula"));
        assert_eq!(back.agent_status_position.as_deref(), Some("top"));
        assert_eq!(back.text_scale, Some(1.21));

        // Default (no zoom) is omitted from the serialized form.
        let bare = Preferences::default();
        assert!(!serde_json::to_string(&bare).unwrap().contains("text_scale"));

        // An old file lacking the field deserializes with text_scale == None.
        let legacy = r#"{"theme":"folio","agent_status_position":"bottom"}"#;
        let parsed: Preferences = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.text_scale, None);
        assert_eq!(parsed.theme.as_deref(), Some("folio"));
    }

    fn s(text: &str) -> Segment {
        (text.to_string(), NStyle::default())
    }

    /// Finding 9 enforcement hook: the turn lifecycle is a total function over
    /// `TurnPhase`, and the canonical `submit → stop → stop → finalize`
    /// sequence pins the escalation behavior that used to live only in a field
    /// comment. The first Stop moves Awaiting → StopRequested (graceful cancel
    /// pending, not yet escalated); the second Stop, gated on `stop_requested()`,
    /// escalates; `finalize` returns to Idle.
    #[test]
    fn turn_phase_submit_stop_stop_finalize_pins_escalation() {
        use std::time::Instant;

        // submit → Awaiting (in flight, no stop yet).
        let mut phase = TurnPhase::begin(Instant::now());
        assert!(phase.is_awaiting(), "submit must enter awaiting");
        assert!(!phase.stop_requested(), "fresh turn has no pending stop");
        assert!(phase.turn_started().is_some(), "awaiting carries the elapsed timer");
        assert!(phase.last_event_at().is_some(), "awaiting carries the quiet clock");

        // First Stop → StopRequested, graceful (not escalated). The handler
        // gate `stop_requested()` is what decides escalate-vs-graceful.
        let first_stop_escalates = phase.stop_requested();
        assert!(!first_stop_escalates, "the FIRST stop must be graceful, not a hard kill");
        phase.request_stop(Instant::now());
        assert!(phase.is_awaiting(), "a pending stop is still in flight (timers run)");
        assert!(phase.stop_requested(), "first stop records a pending cancel");
        assert!(!phase.is_escalated(), "first stop has not escalated");
        // Timers survive the transition so the indicator keeps reading.
        assert!(phase.turn_started().is_some());
        assert!(phase.last_event_at().is_some());

        // Second Stop → the handler sees `stop_requested()` and escalates.
        let second_stop_escalates = phase.stop_requested();
        assert!(second_stop_escalates, "the SECOND stop while awaiting must escalate");
        phase.escalate();
        assert!(phase.is_escalated(), "second stop marks the phase escalated");

        // finalize (turn end / force-restart) → Idle, all markers cleared.
        phase = TurnPhase::Idle;
        assert!(!phase.is_awaiting(), "finalize returns to idle");
        assert!(!phase.stop_requested(), "idle has no pending stop");
        assert!(!phase.is_escalated(), "idle is not escalated");
        assert!(phase.turn_started().is_none(), "idle has no timer");
        assert!(phase.last_event_at().is_none(), "idle has no quiet clock");
    }

    /// `request_stop`/`escalate`/`note_event` are no-ops when idle, so a stray
    /// Stop or stale event can never strand the phase in a contradictory state.
    #[test]
    fn turn_phase_idle_transitions_are_noops() {
        use std::time::Instant;
        let mut phase = TurnPhase::Idle;
        phase.request_stop(Instant::now());
        assert!(matches!(phase, TurnPhase::Idle), "stop on idle is a no-op");
        phase.escalate();
        assert!(matches!(phase, TurnPhase::Idle), "escalate on idle is a no-op");
        phase.note_event(Instant::now());
        assert!(matches!(phase, TurnPhase::Idle), "event on idle is a no-op");

        // note_event refreshes the quiet clock only while in flight.
        let t0 = Instant::now();
        let mut awaiting = TurnPhase::Awaiting { started: t0, last_event: t0 };
        let later = t0 + std::time::Duration::from_secs(5);
        awaiting.note_event(later);
        assert_eq!(
            awaiting.last_event_at(),
            Some(later),
            "note_event advances the quiet clock while awaiting",
        );
        assert_eq!(
            awaiting.turn_started(),
            Some(t0),
            "note_event must not disturb the elapsed timer",
        );
    }

    #[test]
    fn split_segments_at_col_zero_in_first_segment() {
        let segs = vec![s("hello"), s(" "), s("world")];
        let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
        assert!(before.is_empty());
        assert_eq!(ch, 'h');
        let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(after_text, "ello world");
    }

    #[test]
    fn split_segments_at_col_inside_a_segment() {
        // col 2 of "hello" → 'l', before="he", after="lo world"
        let segs = vec![s("hello"), s(" world")];
        let (before, (ch, _), after) = split_segments_at_col(&segs, 2);
        let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
        let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(before_text, "he");
        assert_eq!(ch, 'l');
        assert_eq!(after_text, "lo world");
    }

    #[test]
    fn split_segments_at_col_on_segment_boundary() {
        // col 5 lands on the first char of the second segment (' ').
        let segs = vec![s("hello"), s(" world")];
        let (before, (ch, _), after) = split_segments_at_col(&segs, 5);
        let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
        let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(before_text, "hello");
        assert_eq!(ch, ' ');
        assert_eq!(after_text, "world");
    }

    #[test]
    fn split_segments_at_col_past_end_is_virtual_space() {
        let segs = vec![s("hi")];
        let (before, (ch, _), after) = split_segments_at_col(&segs, 99);
        let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(before_text, "hi");
        assert_eq!(ch, ' '); // cursor at/past EOL renders as a space caret
        assert!(after.is_empty());
    }

    #[test]
    fn split_segments_at_col_empty_input() {
        let segs: Vec<Segment> = vec![];
        let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
        assert!(before.is_empty());
        assert_eq!(ch, ' ');
        assert!(after.is_empty());
    }

    /// S1 enforcement: an unchanged fingerprint must take the fast-skip
    /// path — reuse the cached `Rc`s (same pointer) and run ZERO rebuilds.
    /// A changed fingerprint must rebuild exactly once and produce fresh
    /// `Rc`s. Models the `highlight_cache` fast-skip tests.
    #[test]
    fn view_model_memoization_fast_skip() {
        VIEW_MODEL_REBUILDS.with(|n| n.set(0));
        let mut st = AgentState::new_for_test();

        // Build a fingerprint over the empty structural state.
        let fp1 = st.view_model_fingerprint(0, 0);

        // First call: cold cache → one rebuild.
        let (flat1, gut1) = st.memoize_view_model(fp1, |_c| {
            (vec![FlatItem::Line(0)], vec![None])
        });
        assert_eq!(
            VIEW_MODEL_REBUILDS.with(|n| n.get()),
            1,
            "cold cache must rebuild once"
        );
        let seq_after_first = st.view_model_seq;
        assert_eq!(seq_after_first, 1, "first rebuild bumps the seq to 1");

        // Second call, SAME fingerprint: must skip the rebuild entirely and
        // hand back the very same `Rc`s (pointer identity), seq unchanged.
        let (flat2, gut2) = st.memoize_view_model(fp1, |_c| {
            panic!("rebuild closure must NOT run on a fingerprint hit");
        });
        assert_eq!(
            VIEW_MODEL_REBUILDS.with(|n| n.get()),
            1,
            "fingerprint hit must not rebuild"
        );
        assert!(
            std::rc::Rc::ptr_eq(&flat1, &flat2),
            "flat_items Rc must be reused on a hit"
        );
        assert!(
            std::rc::Rc::ptr_eq(&gut1, &gut2),
            "gutter Rc must be reused on a hit"
        );
        assert_eq!(
            st.view_model_seq, seq_after_first,
            "seq must not change on a fingerprint hit"
        );

        // Fingerprint sensitivity: a structural change (turn_phase enters
        // awaiting, which the thinking indicator depends on) yields a DIFFERENT
        // fingerprint and forces exactly one rebuild + a fresh Rc.
        st.turn_phase = TurnPhase::begin(std::time::Instant::now());
        let fp2 = st.view_model_fingerprint(0, 0);
        assert_ne!(fp1, fp2, "turn_phase awaiting must change the fingerprint");
        let (flat3, _gut3) = st.memoize_view_model(fp2, |_c| {
            (vec![FlatItem::ThinkingIndicator], vec![None])
        });
        assert_eq!(
            VIEW_MODEL_REBUILDS.with(|n| n.get()),
            2,
            "a fingerprint miss must rebuild"
        );
        assert!(
            !std::rc::Rc::ptr_eq(&flat1, &flat3),
            "a rebuild must produce a fresh Rc"
        );
        assert_eq!(st.view_model_seq, 2, "miss bumps the seq again");
    }

    /// F7 (parse-don't-validate at the trust boundary): a `ToolCallKey` parsed
    /// from a protocol `ToolCallId` is the maps' key type, and two keys built
    /// from the same protocol id are equal + hash-equal, so an insert via one
    /// and a lookup via another (the live-update path) land on the same entry.
    /// The type itself is the enforcement hook (no `Deref` to `String`, so an
    /// arbitrary label can't be substituted for a tool id); this pins the
    /// round-trip the maps rely on.
    #[test]
    fn tool_call_key_round_trips_through_the_maps() {
        use sketch::acp_channel::ToolCallId;

        let id: ToolCallId = "tool-abc".into();
        let key_started = ToolCallKey::from_id(&id);
        // A later `ToolCallUpdated` re-parses the SAME protocol id into a key.
        let key_updated = ToolCallKey::from_id(&id);

        assert_eq!(
            key_started, key_updated,
            "keys parsed from the same protocol id must be equal"
        );
        assert_eq!(
            key_started.as_str(),
            "tool-abc",
            "the render edge can recover the id string"
        );
        assert_eq!(key_started.to_string(), "tool-abc");

        // Insert on the started key, look up on the (separately parsed) updated
        // key — the live ToolCallUpdated path. The lookup must hit.
        let mut map: std::collections::HashMap<ToolCallKey, u32> =
            std::collections::HashMap::new();
        map.insert(key_started, 7);
        assert_eq!(
            map.get(&key_updated),
            Some(&7),
            "a key re-parsed from the same id must resolve the same map entry"
        );

        // A DIFFERENT id is a distinct key — no accidental collision.
        let other = ToolCallKey::from_id(&("tool-xyz".into()));
        assert_eq!(map.get(&other), None, "a different id must miss");
    }

    /// The fingerprint must EXCLUDE tool-call content (the `ToolCallUpdated`
    /// trap): mutating a `ToolCall`'s content without touching
    /// `tool_call_order` / `edit_seq` must leave the fingerprint unchanged,
    /// so the cached flat_items (which only carry tool ids) stay valid.
    #[test]
    fn view_model_fingerprint_ignores_tool_content() {
        let mut st = AgentState::new_for_test();
        st.tool_call_order.push(ToolCallKey::from_id(&"tool-1".into()));
        let before = st.view_model_fingerprint(7, 3);

        // Simulate a ToolCallUpdated: content changes, order/edit_seq don't.
        // (We don't have a ToolCall constructor handy in-test; the point is
        // that the fingerprint reads neither `tool_calls` content nor map
        // size — only `tool_call_order`.) Re-derive with identical structural
        // inputs and assert stability.
        let after = st.view_model_fingerprint(7, 3);
        assert_eq!(before, after, "tool content is not part of the fingerprint");
    }

    /// F6 / INV (header-owning turns are exactly {Llm, User}): `HeaderRole`
    /// is a TOTAL mapping over `TurnId` — `Tool`/`System` -> None (no header),
    /// `Llm` -> Claude, `User` -> User. This replaces the old `unreachable!()`
    /// arm with a compiler-checked `Option`, so a new `TurnId` variant is a
    /// compile error, not a paint-path panic.
    #[test]
    fn header_role_is_total_over_turn_id() {
        assert_eq!(HeaderRole::from_turn(TurnId::Tool(3)), None);
        assert_eq!(HeaderRole::from_turn(TurnId::System), None);
        assert_eq!(
            HeaderRole::from_turn(TurnId::Llm(1)),
            Some(HeaderRole::Claude)
        );
        assert_eq!(
            HeaderRole::from_turn(TurnId::User(2)),
            Some(HeaderRole::User)
        );
        // And the role threads through to the rendered `TurnRole`.
        assert_eq!(HeaderRole::Claude.into_turn_role(), TurnRole::Claude);
        assert_eq!(HeaderRole::User.into_turn_role(), TurnRole::User);
    }

    /// F8 / INV-12 (count parity): `reconcile_list` is the ONLY mutator of
    /// `(list_state, list_item_count)`, updating both together so they can't
    /// drift. It returns whether the list grew. After any reconcile the
    /// registered count equals the requested count.
    #[test]
    fn reconcile_list_keeps_count_in_sync_and_reports_growth() {
        let mut st = AgentState::new_for_test();
        assert_eq!(st.list_item_count, 0);

        // Growth: count rises, reports grew=true, splices.
        assert!(st.reconcile_list(5), "0 -> 5 must report growth");
        assert_eq!(st.list_item_count, 5, "count tracks the requested length");

        // No change: same count, reports grew=false, count unchanged.
        assert!(!st.reconcile_list(5), "5 -> 5 is not growth");
        assert_eq!(st.list_item_count, 5);

        // Shrink: count falls, reports grew=false, resets.
        assert!(!st.reconcile_list(2), "5 -> 2 is not growth");
        assert_eq!(st.list_item_count, 2, "count tracks a shrink too");

        // With block ranges active, even growth resets (height cache can't be
        // spliced) — but parity must still hold.
        st.block_ranges.push((0, 3));
        assert!(st.reconcile_list(9));
        assert_eq!(st.list_item_count, 9);
    }

    /// F10 / INV-10 (block/line partition is total): a range
    /// `detect_block_ranges` emits but `parse_block_range` rejects must
    /// `FallBackToLines`, contribute NO entry to the block cache, and so
    /// leave every one of its source lines to render as a standalone Line.
    /// Mirrors render_agent's cache + `in_block` construction exactly.
    #[test]
    fn unparsed_detected_range_falls_back_to_one_line_per_source_line() {
        // 3 pipe-delimited rows with NO separator row: `detect_block_ranges`
        // accepts it (>=3 rows, all `|...|`), but it is NOT a valid markdown
        // table, so `parse_block_range` rejects it.
        let lines: Vec<String> = vec![
            "| a | b |".to_string(),
            "| c | d |".to_string(),
            "| e | f |".to_string(),
        ];
        let frozen = vec![(0usize, lines.len())];
        let ranges = detect_block_ranges(&lines, &frozen);
        assert_eq!(
            ranges,
            vec![(0, 3)],
            "the 3 pipe rows must be DETECTED as a candidate range"
        );

        let theme = Theme::default();
        assert!(
            matches!(
                parse_block_range(&lines, 0, 3, &theme),
                BlockParse::FallBackToLines
            ),
            "a separator-less pipe block must NOT parse as a table"
        );

        // Replicate the render_agent partition: block_cache holds only Parsed
        // ranges; `in_block` is derived from the cache; any line not in a
        // block is emitted as a Line.
        let mut block_cache: std::collections::HashMap<(usize, usize), RenderedBlock> =
            std::collections::HashMap::new();
        for &(s, e) in &ranges {
            if let BlockParse::Parsed(b) = parse_block_range(&lines, s, e, &theme) {
                block_cache.insert((s, e), b);
            }
        }
        let mut in_block: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        for &(s, e) in &ranges {
            if block_cache.contains_key(&(s, e)) {
                for li in s..e {
                    in_block.insert(li);
                }
            }
        }
        let line_items: Vec<usize> = (0..lines.len())
            .filter(|i| {
                !block_cache.keys().any(|&(s, _)| s == *i) && !in_block.contains(i)
            })
            .collect();
        // Count parity over the range: a Line for EVERY source line, no Block.
        assert!(
            block_cache.is_empty(),
            "rejected range must emit no Block item"
        );
        assert_eq!(
            line_items,
            vec![0, 1, 2],
            "every source line of an unparsed range must render as a Line"
        );
    }

    /// F11 / INV-8 (memo soundness): the fingerprint must change when a
    /// resolved tool anchor line changes, because the flat build groups tool
    /// calls by that resolved line. Holding `edit_seq` FIXED across the two
    /// fingerprint calls isolates the anchor dependency from the `edit_seq`
    /// co-variation the memo previously leaned on implicitly.
    #[test]
    fn fingerprint_tracks_resolved_tool_anchor_line() {
        let mut st = AgentState::new_for_test();
        // Seed a few frozen lines so an anchor can resolve to a real line.
        st.editor
            .programmatic_insert(0, "line0\nline1\nline2\nline3\n");

        // Anchor a tool call to line 2 and register it in the build's inputs.
        let anchor = st.editor.anchor_for_line(2);
        let key = ToolCallKey::from_id(&"tool-1".into());
        st.tool_call_order.push(key.clone());
        st.tool_call_anchor_line.insert(key, anchor);
        assert_eq!(st.editor.line_for_anchor(anchor), Some(2));

        // Fingerprint at a FIXED edit_seq/frozen_count.
        let fp_before = st.view_model_fingerprint(42, 4);

        // Insert a line ABOVE the anchor: its resolved line moves 2 -> 3.
        // We pass the SAME edit_seq (42) again, so any fingerprint change is
        // attributable to the resolved anchor line, not to edit_seq.
        st.editor.programmatic_insert(0, "header\n");
        assert_eq!(
            st.editor.line_for_anchor(anchor),
            Some(3),
            "the anchor must have shifted down by one line"
        );
        let fp_after = st.view_model_fingerprint(42, 4);

        assert_ne!(
            fp_before, fp_after,
            "a moved tool anchor must change the fingerprint even at a fixed edit_seq"
        );
    }

    /// F4 / INV-13 enforcement: the tail re-reveal must fire on CONTENT growth
    /// (`edit_seq` advanced), NOT on a flat-item count delta. A chunk that
    /// grows the last line without adding a row (agent prose before a `\n`)
    /// bumps `edit_seq` but leaves the count unchanged; the old count-keyed
    /// path skipped it. `reveal_tail_if_following` must request the reveal
    /// anyway, and must NOT re-request at the same `edit_seq` (idle ticks).
    #[test]
    fn reveal_tail_keys_on_content_growth_not_count() {
        let mut st = AgentState::new_for_test();
        // new_for_test starts in Chatbox with follow_output = true, so the
        // follow decision is satisfied; we isolate the edit_seq/count behavior.
        assert!(st.follow_tail(), "Chatbox + follow_output should follow");

        let count = 3usize; // simulated post-reconcile flat-item count
        let seq0 = st.editor.document().edit_seq();

        // First reveal at the current edit_seq: requested (watermark was MAX).
        assert!(
            st.reveal_tail_if_following(count),
            "first reveal at a new edit_seq must be requested"
        );
        assert_eq!(
            st.last_scrolled_edit_seq, seq0,
            "reveal stamps the watermark to the current edit_seq"
        );

        // Idle tick — same edit_seq, same count: must NOT re-reveal (so a
        // user who scrolled up isn't yanked back every frame).
        assert!(
            !st.reveal_tail_if_following(count),
            "no content growth ⇒ no re-reveal at the same edit_seq"
        );

        // Append a chunk WITHOUT a trailing newline: grows the last line but
        // adds no row, so the flat-item count is UNCHANGED. This is exactly
        // the case the old `new_count != old_count` trigger missed.
        let char_len = st.editor.document().rope().len_chars();
        st.editor.programmatic_insert(char_len, "more streamed prose");
        let seq1 = st.editor.document().edit_seq();
        assert_ne!(seq1, seq0, "an intra-line insert must advance edit_seq");

        // Count is held constant (no new row) — the reveal must STILL fire,
        // keyed on the advanced edit_seq, not on a count delta.
        assert!(
            st.reveal_tail_if_following(count),
            "intra-line content growth must re-reveal even with unchanged count"
        );
        assert_eq!(st.last_scrolled_edit_seq, seq1);

        // A zero count never reveals (guards the `count - 1` underflow).
        let seq2_before = st.last_scrolled_edit_seq;
        st.editor.programmatic_insert(0, "x");
        assert!(
            !st.reveal_tail_if_following(0),
            "an empty list never reveals regardless of growth"
        );
        assert_eq!(
            st.last_scrolled_edit_seq, seq2_before,
            "a skipped reveal must not advance the watermark"
        );

        // When following is OFF (user scrolled up in Chatbox), growth alone
        // must not yank the viewport back.
        st.follow_output.set(false);
        assert!(!st.follow_tail());
        st.editor.programmatic_insert(0, "y");
        assert!(
            !st.reveal_tail_if_following(count),
            "no reveal while the user has scrolled away from the tail"
        );
    }

    /// F12 / INV-11 enforcement: an UNTERMINATED code fence must yield NO
    /// block range, so its arrived lines render as plain Lines (each its own
    /// FlatItem) until the closing fence freezes. A matched closing fence is
    /// required, symmetric to the >=3-row table rule.
    #[test]
    fn detect_block_ranges_skips_unterminated_fence() {
        // Open fence, two body lines, NO closing ``` — all frozen.
        let lines: Vec<String> = vec![
            "```rust".to_string(),
            "let x = 1;".to_string(),
            "let y = 2;".to_string(),
        ];
        let frozen = vec![(0usize, lines.len())];
        let ranges = detect_block_ranges(&lines, &frozen);
        assert!(
            ranges.is_empty(),
            "an unterminated fence must NOT emit a block range, got {ranges:?}"
        );

        // Sanity: once the closing fence arrives, the range IS emitted so
        // the closed block still renders as one Block.
        let mut closed = lines.clone();
        closed.push("```".to_string());
        let frozen_closed = vec![(0usize, closed.len())];
        let ranges_closed = detect_block_ranges(&closed, &frozen_closed);
        assert_eq!(
            ranges_closed,
            vec![(0usize, closed.len())],
            "a closed fence must emit exactly one block range"
        );
    }

    #[test]
    fn segments_to_styled_line_preserves_text_and_count() {
        let segs = vec![s("foo"), s("bar"), s("")];
        let line = segments_to_styled_line(&segs);
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].text, "foo");
        assert_eq!(line.spans[2].text, "");
    }

    // ---- line_selection_range ----

    #[test]
    fn line_selection_range_outside_returns_none() {
        // Selection lines 1..=3, querying line 0 (above) and line 5 (below).
        let sel = ((1, 0), (3, 0));
        assert_eq!(line_selection_range(sel, 0, 10), None);
        assert_eq!(line_selection_range(sel, 5, 10), None);
    }

    #[test]
    fn line_selection_range_single_line_returns_partial() {
        // Sel from col 2 to col 6 on line 4.
        let sel = ((4, 2), (4, 6));
        assert_eq!(line_selection_range(sel, 4, 20), Some((2, 6)));
    }

    #[test]
    fn line_selection_range_first_line_starts_at_sc() {
        let sel = ((2, 5), (4, 3));
        assert_eq!(line_selection_range(sel, 2, 12), Some((5, 12)));
    }

    #[test]
    fn line_selection_range_last_line_ends_at_ec() {
        let sel = ((2, 5), (4, 3));
        assert_eq!(line_selection_range(sel, 4, 20), Some((0, 3)));
    }

    #[test]
    fn line_selection_range_middle_line_full_width() {
        let sel = ((2, 5), (4, 3));
        assert_eq!(line_selection_range(sel, 3, 8), Some((0, 8)));
    }

    // ---- apply_selection_bg ----

    fn seg_text(segs: &[Segment]) -> String {
        segs.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn apply_selection_bg_no_overlap_preserves_segments() {
        // Selection col 0..2 but apply over a single 3-char segment by passing
        // 99..100 (out of range). Result should equal input with 0 bg applied.
        let segs = vec![s("abc")];
        let out = apply_selection_bg(&segs, 99, 100, NColor::Red);
        assert_eq!(seg_text(&out), "abc");
        assert!(out.iter().all(|(_, st)| st.bg.is_none()));
    }

    #[test]
    fn apply_selection_bg_full_segment_gets_bg() {
        let segs = vec![s("abc")];
        let out = apply_selection_bg(&segs, 0, 3, NColor::Red);
        assert_eq!(seg_text(&out), "abc");
        assert!(out.iter().all(|(_, st)| st.bg == Some(NColor::Red)));
    }

    #[test]
    fn apply_selection_bg_splits_segment_at_boundary() {
        // Selection covers chars 1..2 of a 3-char segment → expect 3 segments:
        // unselected "a", selected "b", unselected "c".
        let segs = vec![s("abc")];
        let out = apply_selection_bg(&segs, 1, 2, NColor::Red);
        assert_eq!(seg_text(&out), "abc");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].0, "a");
        assert_eq!(out[0].1.bg, None);
        assert_eq!(out[1].0, "b");
        assert_eq!(out[1].1.bg, Some(NColor::Red));
        assert_eq!(out[2].0, "c");
        assert_eq!(out[2].1.bg, None);
    }

    #[test]
    fn apply_selection_bg_spans_multiple_input_segments() {
        // Sel chars 2..6 across two segments "hello"+"world".
        let segs = vec![s("hello"), s("world")];
        let out = apply_selection_bg(&segs, 2, 6, NColor::Red);
        // Reconstructed text should be unchanged; "ll" + "o" + "w" should be bg'd.
        assert_eq!(seg_text(&out), "helloworld");
        let bg_text: String = out
            .iter()
            .filter(|(_, st)| st.bg == Some(NColor::Red))
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(bg_text, "llow");
    }

    #[test]
    fn apply_selection_bg_empty_input_returns_empty() {
        let out = apply_selection_bg(&[], 0, 5, NColor::Red);
        assert!(out.is_empty());
    }

    // ---- classify_wp_line ----

    #[test]
    fn classify_wp_line_empty_blank_and_whitespace() {
        assert_eq!(classify_wp_line("", false), WpLineKind::Empty);
        assert_eq!(classify_wp_line("   ", false), WpLineKind::Empty);
        assert_eq!(classify_wp_line("\t  ", false), WpLineKind::Empty);
    }

    #[test]
    fn classify_wp_line_headings_levels_1_through_6() {
        assert_eq!(classify_wp_line("# H1", false), WpLineKind::Heading(1));
        assert_eq!(classify_wp_line("## H2", false), WpLineKind::Heading(2));
        assert_eq!(classify_wp_line("### H3", false), WpLineKind::Heading(3));
        assert_eq!(classify_wp_line("###### H6", false), WpLineKind::Heading(6));
        // 7 hashes = not a valid heading per CommonMark; treat as paragraph.
        assert_eq!(classify_wp_line("####### too many", false), WpLineKind::Paragraph);
    }

    #[test]
    fn classify_wp_line_heading_requires_space_after_hashes() {
        // No space after hashes = not a heading.
        assert_eq!(classify_wp_line("#hashtag", false), WpLineKind::Paragraph);
        // Hashes only on the line is still a heading per CommonMark.
        assert_eq!(classify_wp_line("##", false), WpLineKind::Heading(2));
    }

    #[test]
    fn classify_wp_line_bullet_markers() {
        assert_eq!(classify_wp_line("- item", false), WpLineKind::BulletItem);
        assert_eq!(classify_wp_line("* item", false), WpLineKind::BulletItem);
        assert_eq!(classify_wp_line("+ item", false), WpLineKind::BulletItem);
        assert_eq!(classify_wp_line("  - nested", false), WpLineKind::BulletItem);
        // Dash without trailing space is not a bullet.
        assert_eq!(classify_wp_line("-no-space", false), WpLineKind::Paragraph);
    }

    #[test]
    fn classify_wp_line_ordered_markers() {
        assert_eq!(classify_wp_line("1. item", false), WpLineKind::OrderedItem);
        assert_eq!(classify_wp_line("42. item", false), WpLineKind::OrderedItem);
        assert_eq!(classify_wp_line("3) item", false), WpLineKind::OrderedItem);
        // No space after marker.
        assert_eq!(classify_wp_line("1.no", false), WpLineKind::Paragraph);
        // No marker punctuation.
        assert_eq!(classify_wp_line("1 hello", false), WpLineKind::Paragraph);
    }

    #[test]
    fn classify_wp_line_blockquote() {
        assert_eq!(classify_wp_line("> quote", false), WpLineKind::Blockquote);
        assert_eq!(classify_wp_line(">>nested", false), WpLineKind::Blockquote);
    }

    #[test]
    fn classify_wp_line_code_fences() {
        // Opening fence outside of a fence.
        assert_eq!(classify_wp_line("```", false), WpLineKind::CodeFence);
        assert_eq!(classify_wp_line("```rust", false), WpLineKind::CodeFence);
        assert_eq!(classify_wp_line("~~~", false), WpLineKind::CodeFence);
        // Inside a fence: any line is content unless it's a closer.
        assert_eq!(classify_wp_line("let x = 1;", true), WpLineKind::CodeContent);
        assert_eq!(classify_wp_line("```", true), WpLineKind::CodeFence);
        // A heading inside a fence is still code, not a heading.
        assert_eq!(classify_wp_line("# not a heading", true), WpLineKind::CodeContent);
    }

    #[test]
    fn classify_wp_line_table_row_heuristic() {
        // 2+ pipes → table row.
        assert_eq!(classify_wp_line("| col1 | col2 |", false), WpLineKind::TableRow);
        assert_eq!(classify_wp_line("|---|---|", false), WpLineKind::TableRow);
        // Single pipe falls through to paragraph (heuristic requires 2+).
        assert_eq!(classify_wp_line("a | b", false), WpLineKind::Paragraph);
        // Zero pipes = paragraph.
        assert_eq!(classify_wp_line("just text", false), WpLineKind::Paragraph);
    }

    #[test]
    fn classify_wp_line_paragraph_fallback() {
        assert_eq!(classify_wp_line("hello world", false), WpLineKind::Paragraph);
        assert_eq!(classify_wp_line("**bold** text", false), WpLineKind::Paragraph);
    }

    // ---- doc_char_to_line_col ----

    #[test]
    fn doc_char_to_line_col_basic_mapping() {
        let ed = Editor::new("ab\ncd\nef".into(), std::path::PathBuf::from("/t"));
        assert_eq!(doc_char_to_line_col(ed.document(), 0), (0, 0));
        assert_eq!(doc_char_to_line_col(ed.document(), 1), (0, 1));
        // Char 2 is the '\n' between line 0 and line 1.
        assert_eq!(doc_char_to_line_col(ed.document(), 3), (1, 0));
        assert_eq!(doc_char_to_line_col(ed.document(), 6), (2, 0));
        // Past EOF clamps to len.
        assert_eq!(doc_char_to_line_col(ed.document(), 999), (2, 2));
    }

    // ---- Menu rendering helpers ----

    #[test]
    fn format_menu_key_single_char() {
        let kp = KeyPress::new(Key::Char('f'), KMods::NONE);
        assert_eq!(format_menu_key(&[kp]), "f");
    }

    #[test]
    fn format_menu_key_with_ctrl() {
        let kp = KeyPress::new(Key::Char('k'), KMods::CONTROL);
        assert_eq!(format_menu_key(&[kp]), "Ctrl-k");
    }

    #[test]
    fn format_menu_key_named_keys() {
        assert_eq!(
            format_menu_key(&[KeyPress::new(Key::Enter, KMods::NONE)]),
            "Enter"
        );
        assert_eq!(
            format_menu_key(&[KeyPress::new(Key::Esc, KMods::NONE)]),
            "Esc"
        );
        assert_eq!(
            format_menu_key(&[KeyPress::new(Key::F(2), KMods::NONE)]),
            "F2"
        );
    }

    #[test]
    fn format_menu_key_multi_press_sequence() {
        // `g g` for goto-top, etc.
        let g = KeyPress::new(Key::Char('g'), KMods::NONE);
        assert_eq!(format_menu_key(&[g.clone(), g]), "g g");
    }

    #[test]
    fn gpui_menu_has_required_entries() {
        // Sanity check: the menu builder must include every action that
        // `dispatch_menu_command` knows how to dispatch. If we add a new
        // command name to the menu, this assert points at the missing
        // dispatch arm via the matching test below.
        fn collect_leaves<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a str>) {
            for n in nodes {
                match &n.action {
                    sketch::menu::MenuAction::Command(s) => out.push(s.as_str()),
                    sketch::menu::MenuAction::Submenu(children) => {
                        collect_leaves(children, out);
                    }
                    _ => {}
                }
            }
        }
        let menu = gpui_menu();
        let mut leaf_actions: Vec<&str> = Vec::new();
        collect_leaves(&menu, &mut leaf_actions);
        // The expected leaf actions — change here if gpui_menu changes.
        let expected = [
            "open-browser",
            "buffer-list",
            "claude-new",
            "claude-list",
            "claude-close",
            "claude-rename",
            "agent-input-toggle",
            "claude-status-bar",
            "enter-edit",
            "enter-wp",
            "reload-file",
            "back-to-doc",
            "quit",
        ];
        for e in expected {
            assert!(
                leaf_actions.contains(&e),
                "expected menu to contain leaf {:?}, got {:?}",
                e,
                leaf_actions
            );
        }
    }

    #[test]
    fn menu_state_round_trip_picks_command() {
        // Pressing 'q' at root closes the menu and returns "quit".
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        let cmd = state.process_key(KeyPress::new(Key::Char('q'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("quit".to_string()));
        assert!(!state.is_active(), "menu should close after a leaf select");
    }

    #[test]
    fn menu_c_n_resolves_to_claude_new() {
        // `c` opens the claude submenu; `n` then resolves to claude-new.
        // Regression check that the Label node preceding `c` doesn't shadow
        // it (process_key must skip Labels).
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        let after_c = state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
        assert_eq!(after_c, None, "c alone should open the claude submenu");
        assert!(state.is_active(), "submenu open keeps menu state active");
        let cmd = state.process_key(KeyPress::new(Key::Char('n'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("claude-new".to_string()));
    }

    #[test]
    fn menu_c_l_resolves_to_claude_list() {
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
        let cmd = state.process_key(KeyPress::new(Key::Char('l'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("claude-list".to_string()));
    }

    #[test]
    fn menu_f_resolves_to_open_browser() {
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        let cmd = state.process_key(KeyPress::new(Key::Char('f'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("open-browser".to_string()));
    }

    #[test]
    fn menu_e_and_w_resolve_to_edit_views() {
        let menu = gpui_menu();
        for (ch, expected) in &[('e', "enter-edit"), ('w', "enter-wp"), ('v', "back-to-doc")] {
            let mut state = MenuState::new();
            state.open();
            let cmd = state.process_key(KeyPress::new(Key::Char(*ch), KMods::NONE), &menu);
            assert_eq!(
                cmd,
                Some(expected.to_string()),
                "key {:?} should resolve to {:?}",
                ch,
                expected
            );
        }
    }

    #[test]
    fn menu_state_unknown_key_keeps_menu_open() {
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        // 'z' isn't bound at root.
        let cmd = state.process_key(KeyPress::new(Key::Char('z'), KMods::NONE), &menu);
        assert_eq!(cmd, None);
        assert!(state.is_active(), "menu should stay open on unknown key");
    }

    #[test]
    fn append_llm_chunk_chains_turns_above_draft() {
        // Mirrors the old splice-then-lock-then-splice integration test
        // for the new append-and-tag flow: each turn appends just after
        // the last frozen Llm(n) line; a manually-inserted user draft
        // (simulating worksheet typing) survives the agent's reply
        // arriving for the same turn.
        let mut ed = Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
        // Turn 1: agent greets.
        ed.append_llm_chunk(TurnId::Llm(1), "Hi.");
        finalize_agent_turn(&mut ed);
        // User types a reply on the editable line below the frozen
        // "Hi.". The worksheet cursor lives wherever the user puts it.
        ed.cursor_mut().line = ed.document().line_count().saturating_sub(1);
        ed.cursor_mut().col = 0;
        ed.insert_char('o');
        ed.insert_char('k');
        // Turn 2 starts: agent's first chunk goes at EOF (no Llm(2) lines
        // yet) — i.e. after the user's draft "ok". This matches the
        // worksheet's "agent writes at the far end" model (§19).
        ed.append_llm_chunk(TurnId::Llm(2), "Yes!");
        finalize_agent_turn(&mut ed);

        let text = ed.document().full_text();
        assert!(text.contains("Hi."));
        assert!(text.contains("ok"));
        assert!(text.contains("Yes!"));
        let pos_hi = text.find("Hi.").unwrap();
        let pos_ok = text.find("ok").unwrap();
        let pos_yes = text.find("Yes!").unwrap();
        assert!(pos_hi < pos_ok, "Hi before ok ({:?})", text);
        assert!(pos_ok < pos_yes, "ok before Yes! ({:?})", text);
    }
}
