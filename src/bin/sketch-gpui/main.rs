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

mod workspace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use gpui::{
    actions, div, point, px, rgb, rgba, size, AnyElement, App, AppContext, Application,
    Bounds, Context, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, Keystroke, Menu,
    MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, StrikethroughStyle,
    Styled, StyledText, Task, TextLayout, TextRun, TitlebarOptions, UnderlineStyle, Window,
    WindowBounds, WindowOptions,
};

use sketch::acp_channel::AcpChannelClient;
use sketch::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine, StyledSpan};
use sketch::document::Document;
use sketch::editor::{Editor, LineAnchor};
use sketch::file_browser::{BrowserEntry, FileBrowser};
use sketch::keybind::KeybindManager;
use sketch::keys::{Key, KeyPress, Modifiers as KMods};
use sketch::md_highlight::{highlight_markdown_lines, Segment};
use sketch::menu::{MenuNode, MenuNodeKind, MenuState};
use sketch::render;
use sketch::style::{Color as NColor, Modifier, Style as NStyle};
use sketch::theme::{Theme, ThemeName};

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
        // Buffer cycling
        NextBuffer,
        PrevBuffer,
        // Tab cycling (workspace-level — independent of buffer list)
        NextTab,
        PrevTab,
        NewTab,
        CloseTab,
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
        // Document text zoom (scales body + headings; chrome stays fixed)
        ZoomIn,
        ZoomOut,
        ZoomReset,
        // View-mode mouse text selection
        CopyDocSelection,
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
const CURSOR_BAR_COLOR: u32 = 0xff5555;
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
    let (block_idx, sink) = match (ctx.current_block, ctx.line_layouts) {
        (Some(b), Some(s)) => (b, s),
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
            runs = apply_selection_bg_to_runs(runs, s_byte, e_byte, ncolor_to_hsla(SELECTION_BG, BG));
        }
    }

    let styled = StyledText::new(text).with_runs(runs);
    // Clone the TextLayout handle into the side channel so mouse handlers
    // on the doc body can later hit-test this line.
    sink.borrow_mut().insert((block_idx, line_idx), styled.layout().clone());

    // Plain text path — no wiki links on this line.
    if wiki_link_ranges.is_empty() {
        return styled.into_any_element();
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
    gpui::InteractiveText::new(element_id, styled)
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
        .into_any_element()
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
    line_layouts: Option<&'a RefCell<HashMap<(usize, usize), TextLayout>>>,
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

fn block_element(ctx: &RenderCtx<'_>, idx: usize, block: &RenderedBlock) -> AnyElement {
    let highlighted = ctx.cursor_block == Some(idx);
    let inner_ctx = RenderCtx {
        theme: ctx.theme,
        body_font: ctx.body_font.clone(),
        code_font: ctx.code_font.clone(),
        text_scale: ctx.text_scale,
        cursor_block: ctx.cursor_block,
        doc_selection: ctx.doc_selection,
        line_layouts: ctx.line_layouts,
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
        RenderedBlock::CodeBlock { language, lines } => {
            let bg = ctx.theme.code_block_bg;
            let mut col = div()
                .flex()
                .flex_col()
                .p_2()
                .rounded_md()
                .bg(bg_or(bg, BG))
                .font_family(ctx.code_font.clone())
                .text_color(rgb(DEFAULT_FG));
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

/// Render markdown to blocks + post-process `[[name]]` / `[[name|display]]`
/// patterns into link-bearing spans. pulldown-cmark doesn't understand
/// wiki links, so they arrive as plain text and we rewrite them after
/// rendering. Reuses the existing `StyledSpan.link` channel so click
/// handling stays uniform with regular markdown links.
fn render_with_wiki(text: &str, theme: &Theme) -> Vec<RenderedBlock> {
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
                let doc = Document::from_text(text, path);
                d.blocks = render_with_wiki(&doc.full_text(), theme);
            }
            // Browser's underlying-stashed content is also restyled if it
            // happens to be a Doc — otherwise reverting via Esc lands on
            // stale-themed blocks.
            if let WindowContent::Browser(b) = &mut win.content {
                if let Some(under) = b.underlying.as_deref_mut() {
                    if let WindowContent::Doc(d) = under {
                        let path = PathBuf::from(d.file_label.as_ref());
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        let doc = Document::from_text(text, path);
                        d.blocks = render_with_wiki(&doc.full_text(), theme);
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

    let caret = match mode {
        EditMode::Insert => div()
            .w(px(2.0))
            .h(px(18.0))
            .bg(cursor_color)
            .into_any_element(),
        EditMode::Normal => div()
            .w(px(8.0))
            .h(px(18.0))
            .bg(cursor_color)
            .text_color(rgb(BG))
            .child(at_char.to_string())
            .into_any_element(),
    };

    // For insert mode the beam is zero-width, so the at_char gets folded
    // into the after-stream (with its original style).
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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Preferences {
    /// Kebab-case theme identifier — `ThemeName::as_kebab()` /
    /// `ThemeName::parse()`. `None` means "no app-managed override; use
    /// the value from config.kdl (or the built-in default)."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedTab {
    auto_name: String,
    display_name: Option<String>,
    focused_window: workspace::WindowId,
    layout: PersistedLayout,
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
        map.insert(cwd.display().to_string(), v);
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
    let entry = map.get(&cwd.display().to_string())?;
    serde_json::from_value(entry.clone()).ok()
}

/// One restored session slot. Order in the returned `Vec` matches the
/// saved ring order; reboot rebuilds the ring in this same order.
/// `mode`, `tasklist_open`, and `subagents_open` are spec §35 additions;
/// older files (without these keys) deserialize with defaults
/// (Chatbox, false, false). Older sketch binaries reading newer files
/// silently drop the unknown keys (downgrade contract, §35).
#[derive(Debug, Clone)]
struct PersistedSlot {
    id: String,
    label: String,
    active: bool,
    mode: InputMode,
    tasklist_open: bool,
    subagents_open: bool,
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
    let key = cwd.to_string_lossy();
    let Some(entry) = json.get(key.as_ref()) else {
        return Vec::new();
    };
    // Legacy single-string shape: synthesize a one-slot list with the
    // spec-§35 defaults for the missing fields.
    if let Some(id) = entry.as_str() {
        return vec![PersistedSlot {
            id: id.to_string(),
            label: "claude-1".into(),
            active: true,
            mode: InputMode::Chatbox,
            tasklist_open: false,
            subagents_open: false,
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
                    "worksheet" => InputMode::Worksheet,
                    _ => InputMode::Chatbox,
                })
                .unwrap_or(InputMode::Chatbox);
            let tasklist_open = obj
                .get("tasklist_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let subagents_open = obj
                .get("subagents_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            Some(PersistedSlot {
                id,
                label,
                active,
                mode,
                tasklist_open,
                subagents_open,
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
            let mode_str = match slot.state.input_mode {
                InputMode::Worksheet => "worksheet",
                InputMode::Chatbox => "chatbox",
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
            obj.remove(cwd.to_string_lossy().as_ref());
        } else {
            obj.insert(
                cwd.to_string_lossy().into_owned(),
                serde_json::Value::Array(entries),
            );
        }
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
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
        ids: Vec<String>,
    },
    /// A structurally-rendered block (table or fenced code block) that
    /// replaces a range of frozen lines with proper layout.
    Block(RenderedBlock),
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
) -> AnyElement {
    use sketch::acp_channel::ToolCallStatus;
    let (status_glyph, status_color): (&str, Hsla) = match tc.status {
        ToolCallStatus::Pending => ("○", rgb(0x6272a4).into()),
        ToolCallStatus::InProgress => ("◐", rgb(0xf1fa8c).into()),
        ToolCallStatus::Completed => ("●", rgb(0x50fa7b).into()),
        ToolCallStatus::Failed => ("✗", rgb(0xff5555).into()),
        _ => ("·", rgb(0x6272a4).into()),
    };
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
        .py_1()
        .child(div().text_color(rgb(0x6272a4)).child(arrow))
        .child(div().text_color(status_color).child(status_glyph))
        .child(
            div()
                .text_color(rgb(0xbfbfbf))
                .text_size(px(12.0))
                .child(format!("[{:?}]", tc.kind).to_lowercase()),
        )
        .child(div().flex_1().text_color(rgb(DEFAULT_FG)).child(title));

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
        .pl_4()
        .border_l_2()
        .border_color(rgb(0x44475a))
        .child(summary_row);

    if expanded && has_body {
        let max_lines = match policy {
            ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
            _ => None,
        };
        if let Some(input) = &tc.raw_input {
            let pretty =
                serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
            block = block.child(tool_body_pane_free(
                "input",
                &pretty,
                None,
                rgb(0x1e1f29),
                code_font,
            ));
        }
        let content_text = render_tool_content_blocks(&tc.content);
        if !content_text.trim().is_empty() {
            block = block.child(tool_body_pane_free(
                "content",
                &content_text,
                max_lines,
                rgb(0x1e1f29),
                code_font,
            ));
        }
        if let Some(output) = &tc.raw_output {
            let pretty =
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
            block = block.child(tool_body_pane_free(
                "output",
                &pretty,
                max_lines,
                rgb(0x282a36),
                code_font,
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
    bg: gpui::Rgba,
    code_font: &SharedString,
) -> gpui::Div {
    let display = match max_lines {
        Some(n) => truncate_lines(body, n),
        None => body.to_string(),
    };
    div()
        .mt_1()
        .px_2()
        .py_1()
        .bg(bg)
        .text_size(px(11.0))
        .text_color(rgb(0xbfbfbf))
        .font_family(code_font.clone())
        .child(format!("{}:\n{}", label, display))
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
    let doc_text = editor.document().full_text();
    if !doc_text.is_empty() && !doc_text.ends_with('\n') {
        let len = editor.document().rope().len_chars();
        editor.programmatic_insert(len, "\n");
    }
    let line_count = editor.document().line_count();
    // After ensuring trailing `\n`, the last actual content line is
    // line_count - 2 (line_count - 1 is the empty trailing line).
    // saturating_sub guards an empty doc, where there's no content to
    // anchor to and we just put the tool block at the top.
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
    doc_line - lines_collapsed + anchors_before.len()
}

/// Detect line ranges in `lines` that should be rendered as structured
/// blocks (tables and fenced code blocks) rather than line-by-line.
/// Only considers frozen (agent-written) lines.
///
/// Returns `Vec<(start, end)>` where `start..end` covers the full block
/// including delimiters. Ranges are non-overlapping and sorted.
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
            // Find closing fence
            while i < lines.len() {
                if lines[i].trim().starts_with("```") && lines[i].trim().len() <= trimmed.len() + 20 {
                    i += 1; // include the closing fence
                    break;
                }
                i += 1;
            }
            // Only emit if we found a closing fence (i advanced past start+1)
            if i > start + 1 {
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
fn parse_block_range(
    lines: &[String],
    start: usize,
    end: usize,
    theme: &Theme,
) -> Option<RenderedBlock> {
    let slice: String = lines[start..end].join("\n");
    let blocks = render_with_wiki(&slice, theme);
    // Take the first Table or CodeBlock produced.
    for b in blocks {
        match &b {
            RenderedBlock::Table { .. } | RenderedBlock::CodeBlock { .. } => return Some(b),
            _ => {}
        }
    }
    None
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
/// `flex_none()` is essential — without it the caret can be shrunk to 0px
/// inside the flex_wrap row when other items want more space, making the
/// cursor appear to vanish. The bar is also a few pixels wider than a
/// typical text caret because, on a wrapped row of monospace text, a 1-2px
/// strip is easy to miss between adjacent glyphs.
fn make_caret(mode: EditMode, cursor_char: char, cursor_color: Hsla) -> AnyElement {
    match mode {
        EditMode::Insert => div()
            .flex_none()
            .w(px(3.0))
            .h(px(18.0))
            .bg(cursor_color)
            .into_any_element(),
        EditMode::Normal => div()
            .flex_none()
            .w(px(8.0))
            .h(px(18.0))
            .bg(cursor_color)
            .text_color(rgb(BG))
            .child(cursor_char.to_string())
            .into_any_element(),
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
}

// ----------------------------------------------------------------------------
// Root view
// ----------------------------------------------------------------------------

/// State held while the user is viewing a rendered markdown document.
struct DocState {
    blocks: Vec<RenderedBlock>,
    file_label: SharedString,
    cursor_block: usize,
    /// Native scroll handle for the body. Renders all blocks into one
    /// overflow-y-scroll container — j/k/ctrl-d/u/g/G drive
    /// `scroll_handle.scroll_to_item(idx)` instead of slicing the block
    /// list, which lets users actually reach the bottom of long files
    /// (the prior block-skip approach couldn't reveal content below the
    /// last block when that block alone overflowed the viewport).
    scroll_handle: ScrollHandle,
    /// Stashed editor from a prior Edit-mode session in the same file,
    /// preserved across Ctrl-V round-trips so unsaved edits aren't lost
    /// when previewing the rendered view. `None` for files that have
    /// only been viewed (never edited) or that came in fresh from disk.
    edit_cache: Option<EditState>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    /// User input is interleaved with LLM output in the transcript editor.
    /// Frozen lines are immutable; editable lines accumulate until a Submit
    /// sweeps and freezes them all (§9–§15).
    Worksheet,
    /// User input goes into a separate `Chatbox` editor pinned to the
    /// bottom of the window. The transcript is read-only while in this
    /// mode (§16–§20).
    Chatbox,
}

/// Tool names that the v1 sub-agent classifier treats as sub-agents.
/// Centralised here so swapping in a structured ACP sub-agent type — or
/// supporting a renamed vendor tool — is a one-slice change (§25).
const SUBAGENT_TOOL_NAMES: &[&str] = &["Task", "Subagent", "Spawn"];

/// Sketch-side classification of a `ToolCall` that represents a sub-agent
/// transcript (§26). Produced by the heuristic in `classify_subagent`; the
/// `Subagents` sidepane lists these, and `focused_subagent` indexes into
/// `AgentState.subagents` to swap the main transcript view.
#[derive(Clone)]
struct SubAgent {
    /// Originating tool-call id. The tool call itself stays in
    /// `tool_calls`; the sub-agent entry is an extra view over the same
    /// content.
    tool_call_id: String,
    /// Best-effort display label: the tool call's `title` if set,
    /// otherwise its `name`, with `subagent-N` as the ultimate fallback.
    label: String,
    /// Mirrors the underlying tool call's status.
    status: sketch::acp_channel::ToolCallStatus,
    /// Accumulated content blocks. Sketch caps these to the same per-
    /// payload budget as main-transcript tool calls (§26).
    transcript: Vec<sketch::acp_channel::ToolCallContent>,
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
        tool_call_id: tc.tool_call_id.0.to_string(),
        label,
        status: tc.status,
        transcript: tc.content.clone(),
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

/// State held while the user is editing a buffer in the GPUI frontend.
/// Raw buffer + cursor + Insert/Normal toggle + vim-style normal-mode
/// actions routed through the shared `KeybindManager`/`Action` vocabulary.
/// Source lines are syntax-highlighted via `md_highlight`.
/// Deferred: IME.
struct EditState {
    editor: Editor,
    file_label: SharedString,
    mode: EditMode,
    keybinds: KeybindManager,
    scroll_handle: ScrollHandle,
    /// Transient footer message — last save outcome ("saved", "save failed: …").
    /// Cleared on the next keystroke that mutates the buffer; persists across
    /// pure motion so the user sees the result for at least one render.
    last_save_msg: Option<SharedString>,
    /// Code (raw monospace + syntax highlight) or WordProcessor (live-preview
    /// proportional + typographic styling). Toggled by `Ctrl-W`.
    view: EditView,
}

impl EditState {
    fn new(editor: Editor, file_label: SharedString, view: EditView) -> Self {
        Self {
            editor,
            file_label,
            mode: EditMode::Normal,
            keybinds: KeybindManager::default(),
            scroll_handle: ScrollHandle::new(),
            last_save_msg: None,
            view,
        }
    }
}

enum WindowContent {
    Doc(DocState),
    Edit(EditState),
    Agent(AgentRing),
    Browser(BrowserWindow),
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
    /// chat-style auto-pin: new items at the end stay anchored to the
    /// bottom of view, but a user scrolling up to read history isn't
    /// snapped back when more chunks land.
    list_state: gpui::ListState,
    /// Total number of items currently registered in `list_state`. We
    /// track it separately so we can splice in new items as the
    /// buffer grows without paying for a full reset.
    list_item_count: usize,
    /// Footer status line — attach result, send result, error. Cleared on
    /// the next non-Ctrl keystroke so it persists for at least one frame.
    status: Option<SharedString>,
    /// Set true when the user sends; cleared once the agent's prompt
    /// response lands (turn count increments). Drives the "…" indicator in
    /// the footer.
    awaiting_reply: bool,
    /// When the current ACP turn started. Set on successful send,
    /// cleared on finalize or channel drop. Drives the elapsed timer
    /// in the header.
    turn_started: Option<std::time::Instant>,
    /// Last-seen turn count. The pump compares this against the live counter
    /// each tick — when it ticks up, the in-flight turn just ended, which is
    /// our cue to finalize the buffer (ensure an editable line below the
    /// frozen content) and clear `awaiting_reply`.
    last_seen_turns: usize,
    /// Live tool calls keyed by `tool_call_id`. Updated in place as the
    /// agent emits `ToolCallUpdate` notifications (status → in_progress →
    /// completed/failed, content arriving incrementally, etc.).
    tool_calls: std::collections::HashMap<String, sketch::acp_channel::ToolCall>,
    /// Display order — `tool_call_id`s in the chronological order they
    /// were first announced. Drives both rendering order and "render
    /// after which buffer line" via [`tool_call_anchor_line`].
    tool_call_order: Vec<String>,
    /// Anchors a tool call to the buffer line that was the last frozen
    /// line at the moment it was announced. The renderer slots the tool
    /// block in just after that line, so tool blocks land between the
    /// chunks that bracketed them in time.
    tool_call_anchor_line: std::collections::HashMap<String, LineAnchor>,
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
    /// Which input surface the user is currently using (§4). Per the
    /// spec, new sessions start at `Chatbox` to match today's compose-box-
    /// first feel; the user toggles with `Ctrl-Alt-Enter` (§5).
    input_mode: InputMode,
    /// Standalone draft editor. `Some` iff `input_mode == Chatbox`; in
    /// Worksheet mode this is `None` and key dispatch routes to the
    /// transcript editor instead.
    chatbox: Option<Chatbox>,
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
    /// Classified sub-agents — `ToolCall`s the heuristic flagged as
    /// representing a sub-agent transcript (§25–§26). Ordered by
    /// first-seen. Each carries the originating tool-call id, label,
    /// status mirror, and accumulated content.
    subagents: Vec<SubAgent>,
    /// Index into `subagents` of the currently focused sub-agent. When
    /// `Some`, the main transcript area swaps to show that sub-agent's
    /// content instead of the root agent's (§27).
    focused_subagent: Option<usize>,
    /// Whether the Tasklist sidepane is open (§24).
    tasklist_open: bool,
    /// Whether the Subagents sidepane is open (§28).
    subagents_open: bool,
    /// Background polling task that drains the ACP channel into the editor
    /// every ~50ms. Held only so that dropping `AgentState` (e.g. on
    /// `back_to_doc`) cancels the task. The leading `_` mutes unused-field
    /// warnings — the field IS used (its Drop runs on screen exit), but
    /// no method reads it.
    _pump: Option<Task<()>>,
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

    fn push(&mut self, label: String, state: AgentState, resume_id: Option<String>) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.slots.push(AgentSlot {
            label,
            index,
            state,
            has_unseen_activity: false,
            resume_id,
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
    fn close_active(&mut self) -> Option<AgentState> {
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
        Some(removed.state)
    }

    fn len(&self) -> usize {
        self.slots.len()
    }

    fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

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
            "claude (acp)",
            vec![
                MenuNode::entry("o", "open chat", "open-claude"),
                MenuNode::entry("s", "send draft", "claude-send"),
                MenuNode::entry("n", "new session", "claude-new"),
                MenuNode::entry("x", "close session", "claude-close"),
                MenuNode::entry("]", "next session", "claude-next"),
                MenuNode::entry("[", "prev session", "claude-prev"),
                MenuNode::entry("m", "cycle permission mode", "claude-mode-cycle"),
                MenuNode::entry("c", "clear → fresh session", "claude-clear"),
                MenuNode::entry("r", "reboot → resume claude", "claude-reboot"),
                MenuNode::entry("d", "detach session", "claude-detach"),
                MenuNode::entry("a", "attach session", "claude-attach"),
                MenuNode::entry("R", "rename session", "claude-rename"),
                MenuNode::entry("t", "compose", "compose-toggle"),
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
            ],
        ),
        MenuNode::separator(),
        MenuNode::submenu(
            "W",
            "window (splits/tabs)",
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
                MenuNode::label("Tabs"),
                MenuNode::entry("t", "new tab (Cmd-T)", "new-tab"),
                MenuNode::entry("x", "close tab (Cmd-Shift-W)", "close-tab"),
                MenuNode::entry("]", "next tab (Ctrl-Tab)", "next-tab"),
                MenuNode::entry("[", "prev tab (Ctrl-Shift-Tab)", "prev-tab"),
                MenuNode::entry("r", "rename tab (Cmd-Shift-R)", "rename-tab"),
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
    focus_handle: FocusHandle,
    /// Active TUI-style menu overlay. `Some` while the picker is open;
    /// flipped to `None` on Esc-from-root or after a command is dispatched.
    menu: Option<MenuOverlay>,
    /// Buffer-list picker overlay — open while `Some`.
    buffer_switcher: Option<BufferSwitcher>,
    /// Single-line rename input overlay for the active claude session.
    /// `Some` while the input box is open; cleared on Enter (commit) or
    /// Esc (cancel).
    rename_overlay: Option<RenameOverlay>,
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
    line_layouts: RefCell<HashMap<(usize, usize), TextLayout>>,
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
            scroll_handle: ScrollHandle::new(),
            edit_cache: None,
        });
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            focus_handle,
            menu: None,
            buffer_switcher: None,
            rename_overlay: None,
            workspace: workspace::Workspace::with_initial(initial),
            doc_selection: None,
            line_layouts: RefCell::new(HashMap::new()),
        }
    }

    fn new_browser(start_dir: PathBuf, theme: Theme, focus_handle: FocusHandle) -> Self {
        let initial = WindowContent::Browser(BrowserWindow::standalone(start_dir));
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("SF Mono"),
            text_scale: 1.0,
            focus_handle,
            menu: None,
            buffer_switcher: None,
            rename_overlay: None,
            workspace: workspace::Workspace::with_initial(initial),
            doc_selection: None,
            line_layouts: RefCell::new(HashMap::new()),
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
    fn save_workspace_state(&self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        save_persisted_workspace(&cwd, &self.workspace);
    }

    /// Replace `self.workspace` with one rebuilt from the persisted snapshot
    /// for `cwd`, if any. Doc/Edit windows reload their files; Browser
    /// windows reattach to their saved dir; Claude windows are replaced
    /// with a Browser at cwd (full ACP restore is a follow-up). Returns
    /// `true` if a snapshot was loaded.
    fn restore_workspace_from_disk(&mut self) -> bool {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let Some(snap) = load_persisted_workspace(&cwd) else {
            return false;
        };
        let mut ws: workspace::Workspace<WindowContent> = workspace::Workspace::new();
        for ptab in snap.tabs {
            let (layout, max_id) = self.restore_layout(ptab.layout);
            ws.next_window_id = ws.next_window_id.max(max_id + 1);
            ws.tabs.push(workspace::Tab {
                auto_name: ptab.auto_name,
                display_name: ptab.display_name,
                focused: ptab.focused_window,
                layout,
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
        true
    }

    fn restore_layout(
        &self,
        layout: PersistedLayout,
    ) -> (workspace::Layout<WindowContent>, workspace::WindowId) {
        match layout {
            PersistedLayout::Leaf(leaf) => {
                let id = leaf.id;
                let content = self.restore_content(leaf.kind);
                (
                    workspace::Layout::Leaf(workspace::Window { id, content }),
                    id,
                )
            }
            PersistedLayout::Split { dir, children } => {
                let mut max_id: workspace::WindowId = 0;
                let mut restored_children = Vec::with_capacity(children.len());
                for (w, child) in children {
                    let (sub, sub_max) = self.restore_layout(child);
                    if sub_max > max_id {
                        max_id = sub_max;
                    }
                    restored_children.push((w, sub));
                }
                (
                    workspace::Layout::Split {
                        dir,
                        children: restored_children,
                    },
                    max_id,
                )
            }
        }
    }

    fn restore_content(&self, kind: PersistedKind) -> WindowContent {
        match kind {
            PersistedKind::Doc { path } => {
                let label: SharedString = path.display().to_string().into();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let doc = Document::from_text(text, path.clone());
                let blocks = render_with_wiki(&doc.full_text(), &self.theme);
                WindowContent::Doc(DocState {
                    blocks,
                    file_label: label,
                    cursor_block: 0,
                    scroll_handle: ScrollHandle::new(),
                    edit_cache: None,
                })
            }
            PersistedKind::Edit { path } => {
                let label: SharedString = path.display().to_string().into();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let editor = Editor::new(text, path);
                WindowContent::Edit(EditState::new(editor, label, EditView::Code))
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
        let blocks = render_with_wiki(&doc.full_text(), &self.theme);
        let new_content = WindowContent::Doc(DocState {
            blocks,
            file_label: label,
            cursor_block: 0,
            scroll_handle: ScrollHandle::new(),
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
                d.scroll_handle.scroll_to_item(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn scroll_up(&mut self, _: &ScrollUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if d.cursor_block > 0 {
                d.cursor_block -= 1;
                d.scroll_handle.scroll_to_item(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn page_down(&mut self, _: &ScrollPageDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = (d.cursor_block + 8).min(d.blocks.len().saturating_sub(1));
            d.scroll_handle.scroll_to_top_of_item(d.cursor_block);
            cx.notify();
        }
    }
    fn page_up(&mut self, _: &ScrollPageUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = d.cursor_block.saturating_sub(8);
            d.scroll_handle.scroll_to_top_of_item(d.cursor_block);
            cx.notify();
        }
    }
    fn cursor_next(&mut self, _: &CursorNextBlock, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if d.cursor_block + 1 < d.blocks.len() {
                d.cursor_block += 1;
                d.scroll_handle.scroll_to_item(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn cursor_prev(&mut self, _: &CursorPrevBlock, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if d.cursor_block > 0 {
                d.cursor_block -= 1;
                d.scroll_handle.scroll_to_item(d.cursor_block);
                cx.notify();
            }
        }
    }
    fn cursor_top(&mut self, _: &CursorTop, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.cursor_block = 0;
            d.scroll_handle.scroll_to_top_of_item(0);
            cx.notify();
        }
    }
    fn cursor_bottom(&mut self, _: &CursorBottom, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            if !d.blocks.is_empty() {
                d.cursor_block = d.blocks.len() - 1;
                d.scroll_handle.scroll_to_item(d.cursor_block);
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
            cx.notify();
        }
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
        save_preferences(&Preferences {
            theme: Some(name.as_kebab().to_string()),
        });
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

    fn clone_focused_for_split(&self, cwd: &std::path::Path) -> WindowContent {
        let label = match self.workspace.focused_content() {
            Some(WindowContent::Doc(d)) => Some(d.file_label.clone()),
            Some(WindowContent::Edit(e)) => Some(e.file_label.clone()),
            _ => None,
        };
        let is_edit = matches!(
            self.workspace.focused_content(),
            Some(WindowContent::Edit(_))
        );
        let browser_fallback = || {
            WindowContent::Browser(BrowserWindow::standalone(cwd.to_path_buf()))
        };
        let Some(label) = label else {
            return browser_fallback();
        };
        let path = PathBuf::from(label.as_ref());
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return browser_fallback(),
        };
        if is_edit {
            let editor = Editor::new(text, path);
            WindowContent::Edit(EditState::new(editor, label, EditView::Code))
        } else {
            let doc = Document::from_text(text, path);
            let blocks = render_with_wiki(&doc.full_text(), &self.theme);
            WindowContent::Doc(DocState {
                blocks,
                file_label: label,
                cursor_block: 0,
                scroll_handle: ScrollHandle::new(),
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
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_right(&mut self, _: &FocusRight, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Right);
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_up(&mut self, _: &FocusUp, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Up);
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_down(&mut self, _: &FocusDown, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Down);
        self.save_workspace_state();
        cx.notify();
    }

    /// `Ctrl-W w` / `Ctrl-W W` — cycle focus through leaves in tree order.
    fn focus_next(&mut self, _: &FocusNext, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_next();
        self.save_workspace_state();
        cx.notify();
    }
    fn focus_prev(&mut self, _: &FocusPrev, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_prev();
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
            b.fb.move_down();
            cx.notify();
        }
    }
    fn browser_up(&mut self, _: &BrowserUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            b.fb.move_up();
            cx.notify();
        }
    }
    fn browser_enter(&mut self, _: &BrowserEnter, _w: &mut Window, cx: &mut Context<Self>) {
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
            b.fb.go_parent();
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

    // ---- Edit mode ---------------------------------------------------------

    /// `Some(edit)` if currently editing, else `None`.
    fn edit_mut(&mut self) -> Option<&mut EditState> {
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Edit(e) => Some(e),
            _ => None,
        }
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
        let mut edit_state = match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Doc(d) => match d.edit_cache.take() {
                Some(cached) => cached,
                None => {
                    let path: PathBuf = d.file_label.to_string().into();
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    let editor = Editor::new(text, path);
                    EditState::new(editor, d.file_label.clone(), view)
                }
            },
            _ => return,
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
                scroll_handle: ScrollHandle::new(),
                edit_cache: None,
            }),
        )
            .expect("workspace has no focused window");
        match prev {
            WindowContent::Edit(edit) => {
                let blocks =
                    render_with_wiki(&edit.editor.document().full_text(), &self.theme);
                let file_label = edit.file_label.clone();
                self.set_screen(WindowContent::Doc(DocState {
                    blocks,
                    file_label,
                    cursor_block: 0,
                    scroll_handle: ScrollHandle::new(),
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
        let doc = Document::from_text(text, path);
        let blocks = render_with_wiki(&doc.full_text(), &self.theme);
        self.set_screen(WindowContent::Doc(DocState {
            blocks,
            file_label: label,
            cursor_block: 0,
            scroll_handle: ScrollHandle::new(),
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
        // Two passes: extract the path from the focused window without
        // holding a mutable borrow across the file I/O + workspace mutation.
        // Preserves the Edit sub-view (Code vs. WordProcessor) so reload
        // doesn't yank the user out of WP mode.
        enum Kind {
            Doc,
            Edit(EditView),
        }
        let (path, label, kind) = match self.workspace.focused_content() {
            Some(WindowContent::Doc(d)) => (
                PathBuf::from(d.file_label.as_ref()),
                d.file_label.clone(),
                Kind::Doc,
            ),
            Some(WindowContent::Edit(e)) => (
                PathBuf::from(e.file_label.as_ref()),
                e.file_label.clone(),
                Kind::Edit(e.view),
            ),
            _ => return,
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("reload: cannot read {}: {}", path.display(), err);
                return;
            }
        };
        let new_content = match kind {
            Kind::Edit(view) => {
                let editor = Editor::new(text, path);
                let mut es = EditState::new(editor, label, EditView::Code);
                es.view = view;
                WindowContent::Edit(es)
            }
            Kind::Doc => {
                let doc = Document::from_text(text, path);
                let blocks = render_with_wiki(&doc.full_text(), &self.theme);
                WindowContent::Doc(DocState {
                    blocks,
                    file_label: label,
                    cursor_block: 0,
                    scroll_handle: ScrollHandle::new(),
                    edit_cache: None,
                })
            }
        };
        self.set_screen(new_content);
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
    fn dispatch_insert_core(editor: &mut Editor, mode: &mut EditMode, press: KeyPress) {
        match press.key {
            Key::Esc => {
                editor.end_insert();
                *mode = EditMode::Normal;
                // Vim convention: cursor steps back one column on leaving insert.
                if editor.cursor().col > 0 {
                    editor.cursor_mut().move_left();
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
    fn dispatch_normal_core(
        editor: &mut Editor,
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
                editor.cursor_mut().move_up();
                editor.clamp_cursor_col(false);
            }
            "move-left" => {
                editor.pre_move(false);
                editor.cursor_mut().move_left();
            }
            "move-right" => {
                editor.pre_move(false);
                editor.move_right_clamped(false);
            }
            "move-line-start" => {
                editor.pre_move(false);
                editor.cursor_mut().move_line_start();
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
                editor.cursor_mut().jump_top();
            }
            "goto-bottom" => {
                editor.pre_move(false);
                editor.jump_cursor_bottom();
            }
            // ---- Mode switches ----
            "insert-mode" => {
                if let Some(((sl, sc), _)) = editor.selection_range() {
                    editor.cursor_mut().line = sl;
                    editor.cursor_mut().col = sc;
                    editor.clear_selection();
                }
                editor.set_extend_mode(false);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "insert-after" => {
                if let Some((_, (el, ec))) = editor.selection_range() {
                    editor.cursor_mut().line = el;
                    editor.cursor_mut().col = ec;
                    let line_len = editor.document().line_len_chars(el);
                    if editor.cursor().col < line_len {
                        editor.cursor_mut().col += 1;
                    }
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
                        .document()
                        .line_text(editor.cursor().line)
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

    fn open_menu_inner(&mut self, cx: &mut Context<Self>) {
        // No-op if already open (defensive — the action shouldn't fire then).
        if self.menu.is_some() {
            return;
        }
        let mut state = MenuState::new();
        state.open();
        self.menu = Some(MenuOverlay {
            state,
            menu: gpui_menu(),
        });
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
            if let Some(m) = &mut self.menu {
                m.state.handle_escape();
                if !m.state.is_active() {
                    self.menu = None;
                }
            }
            cx.notify();
            return;
        }

        let cmd = match &mut self.menu {
            Some(m) => m.state.process_key(press, &m.menu),
            None => return,
        };
        if let Some(name) = cmd {
            self.menu = None;
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
            "claude-new" => self.new_agent_session(cx),
            "claude-close" => self.close_active_agent_session(cx),
            "claude-next" => self.switch_agent_session(1, cx),
            "claude-prev" => self.switch_agent_session(-1, cx),
            "claude-reboot" => self.reboot_into_claude(cx),
            "claude-mode-cycle" => self.cycle_claude_permission_mode(cx),
            "claude-clear" => self.clear_agent_session(cx),
            "claude-detach" => self.detach_active_agent_session(cx),
            "claude-attach" => self.attach_active_agent_session(cx),
            "claude-rename" => self.open_rename_overlay(cx),
            // Pre-rename "compose-toggle" entry retained semantically as
            // the input-mode toggle (spec §34 — menu chord `t` rewires
            // to the new toggle, same chord, new behavior).
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
        if self.buffer_switcher.is_some() || self.workspace.tabs.is_empty() {
            return;
        }
        self.buffer_switcher = Some(BufferSwitcher {
            selected: self.workspace.active_tab,
            filter_mode: false,
            filter_text: String::new(),
        });
        cx.notify();
    }

    fn close_buffer_switcher(&mut self) {
        self.buffer_switcher = None;
    }

    // ---- Session rename overlay -------------------------------------------

    /// Open the rename input overlay for the active claude session. No-op
    /// if claude isn't focused (the command is gated by the menu but a
    /// stray dispatch shouldn't crash) or if an overlay is already open.
    fn open_rename_overlay(&mut self, cx: &mut Context<Self>) {
        if self.rename_overlay.is_some() {
            return;
        }
        let Some(ring) = self.agent_ring() else {
            return;
        };
        let slot = &ring.slots[ring.active];
        self.rename_overlay = Some(RenameOverlay {
            text: slot.label.clone(),
            target: RenameTarget::AgentSlot { index: slot.index },
        });
        cx.notify();
    }

    /// Open the rename overlay targeting the active workspace tab. The
    /// input pre-fills with the tab's current display label (display_name
    /// if set, else auto_name).
    fn open_rename_active_tab_overlay(&mut self, cx: &mut Context<Self>) {
        if self.rename_overlay.is_some() {
            return;
        }
        let idx = self.workspace.active_tab;
        let Some(tab) = self.workspace.tabs.get(idx) else {
            return;
        };
        self.rename_overlay = Some(RenameOverlay {
            text: tab.display_label().to_string(),
            target: RenameTarget::Tab { index: idx },
        });
        cx.notify();
    }

    fn close_rename_overlay(&mut self) {
        self.rename_overlay = None;
    }

    /// Apply the overlay's text to the targeted slot/tab, then close.
    /// Trims whitespace; an all-whitespace input cancels (acts like Esc) so
    /// the user can't accidentally erase the label by hammering Enter.
    fn commit_rename_overlay(&mut self, cx: &mut Context<Self>) {
        let (target, new_label) = match &self.rename_overlay {
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
                if let Some(ring) = self.agent_ring_mut() {
                    if let Some(slot) = ring.slot_by_index_mut(index) {
                        slot.label = new_label;
                    }
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
                if let Some(o) = &mut self.rename_overlay {
                    o.text.pop();
                }
                cx.notify();
            }
            Key::Char(c) => {
                if let Some(o) = &mut self.rename_overlay {
                    o.text.push(c);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Return the indices of buffers matching the current filter query.
    fn filtered_buffer_indices(&self) -> Vec<usize> {
        let bs = match &self.buffer_switcher {
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

        let (filter_mode, selected) = match &self.buffer_switcher {
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
                        if let Some(bs) = &mut self.buffer_switcher {
                            bs.filter_mode = false;
                        }
                    }
                    cx.notify();
                    return;
                }
                Key::Backspace => {
                    if let Some(bs) = &mut self.buffer_switcher {
                        bs.filter_text.pop();
                        bs.selected = 0;
                    }
                }
                Key::Char(c) => {
                    if let Some(bs) = &mut self.buffer_switcher {
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
                if let Some(bs) = &mut self.buffer_switcher {
                    if count > 0 {
                        bs.selected = (bs.selected + 1) % count;
                    }
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = &mut self.buffer_switcher {
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
                if let Some(bs) = &mut self.buffer_switcher {
                    bs.selected = 0;
                }
            }
            Key::Char('G') => {
                let count = self.filtered_buffer_indices().len();
                if let Some(bs) = &mut self.buffer_switcher {
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
                    if let Some(bs) = &mut self.buffer_switcher {
                        if bs.selected >= count && count > 0 {
                            bs.selected = count - 1;
                        }
                    }
                }
            }
            Key::Char('/') => {
                if let Some(bs) = &mut self.buffer_switcher {
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_idx = self.workspace.active_tab;
        let focused_id = self.workspace.tabs[tab_idx].focused;
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
        self.render_layout(root, layout, focused_id, attach_focus, cx)
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
                // Add a thin focus indicator around the focused leaf when
                // there's more than one leaf in the tab. (A border on a
                // single-leaf tab is just visual noise — the whole window
                // *is* the focus.)
                if is_focused && self.active_tab_leaf_count() > 1 {
                    let accent: Hsla = rgb(STATUS_FG).into();
                    div()
                        .size_full()
                        .border_1()
                        .border_color(accent)
                        .child(painted)
                        .into_any_element()
                } else {
                    painted
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
                        self.render_layout(child_root, child, focused_id, attach_focus, cx);
                    let mut slot = div().min_w_0().min_h_0();
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

    fn render_menu_overlay(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let m = match &self.menu {
            Some(m) => m,
            None => unreachable!(),
        };

        let menu_bg: Hsla = rgb(0x1e1e3a).into();
        let label_fg: Hsla = rgb(0x6272a4).into();
        let key_fg: Hsla = rgb(0xbd93f9).into();
        let label_text_fg: Hsla = rgb(0xcccccc).into();
        let submenu_fg: Hsla = rgb(0x8be9fd).into();
        let popup_border: Hsla = rgb(0x383a4f).into();

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
                    .border_color(rgb(0x383a4f))
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
        let bs = match &self.buffer_switcher {
            Some(bs) => bs,
            None => unreachable!(),
        };

        let menu_bg: Hsla = rgb(0x1e1e3a).into();
        let label_fg: Hsla = rgb(0x6272a4).into();
        let active_fg: Hsla = rgb(0x8be9fd).into();
        let selected_bg: Hsla = rgb(0x383a4f).into();
        let normal_fg: Hsla = rgb(0xcccccc).into();
        let modified_fg: Hsla = rgb(0xffb86c).into();
        let popup_border: Hsla = rgb(0x383a4f).into();
        let filter_fg: Hsla = rgb(0xf1fa8c).into();

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
        let o = match &self.rename_overlay {
            Some(o) => o,
            None => unreachable!(),
        };
        let menu_bg: Hsla = rgb(0x1e1e3a).into();
        let popup_border: Hsla = rgb(0x383a4f).into();
        let label_fg: Hsla = rgb(0x6272a4).into();
        let input_fg: Hsla = rgb(0xf1fa8c).into();

        let header_label = match o.target {
            RenameTarget::AgentSlot { .. } => "RENAME SESSION",
            RenameTarget::Tab { .. } => "RENAME TAB",
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
            self.new_agent_session(cx);
            return;
        }

        // Stash the current screen so back_to_doc can restore it.
        let prior = self
            .workspace
            .replace_focused_content(WindowContent::Doc(DocState {
                blocks: Vec::new(),
                file_label: SharedString::new_static(""),
                cursor_block: 0,
                scroll_handle: ScrollHandle::new(),
                edit_cache: None,
            }))
            .expect("workspace has no focused window");

        let mut ring = AgentRing::new(Some(Box::new(prior)));
        let cwd_opt = std::env::current_dir().ok();
        let persisted = cwd_opt
            .as_deref()
            .map(load_persisted_acp_sessions)
            .unwrap_or_default();

        if persisted.is_empty() {
            // No saved state → fresh single slot, as before.
            let state = self.create_agent_session(None, cx);
            ring.push("claude-1".into(), state, None);
        } else {
            // Restore every saved slot in order. Each slot's
            // AcpChannelClient spawns on its own background thread, so
            // the N attaches happen concurrently (~100MB RSS per slot
            // during the restore window).
            let active_pos = persisted
                .iter()
                .position(|s| s.active)
                .unwrap_or(0);
            for slot in persisted {
                let mut state = self.create_agent_session(Some(slot.id.clone()), cx);
                // Spec §35 fields. Mode chatbox stays as default; if the
                // slot was saved in Worksheet, drop the freshly-created
                // empty chatbox so the rendered view starts in Worksheet
                // immediately. Per §36, the chatbox's unsent text is
                // intentionally NOT persisted.
                state.input_mode = slot.mode;
                if slot.mode == InputMode::Worksheet {
                    state.chatbox = None;
                }
                state.tasklist_open = slot.tasklist_open;
                state.subagents_open = slot.subagents_open;
                ring.push(slot.label, state, Some(slot.id));
            }
            ring.active = active_pos.min(ring.slots.len().saturating_sub(1));
        }

        self.set_screen(WindowContent::Agent(ring));

        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Create a new session and add it to the existing ring.
    fn new_agent_session(&mut self, cx: &mut Context<Self>) {
        let ring = match self.agent_ring_mut() {
            Some(r) => r,
            None => return,
        };
        let n = ring.next_index + 1;
        let label = format!("claude-{n}");
        // New sessions don't resume — they're fresh.
        let state = self.create_agent_session(None, cx);
        let ring = self.agent_ring_mut().unwrap();
        ring.push(label, state, None);
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
        let is_empty = {
            let ring = match self.agent_ring_mut() {
                Some(r) => r,
                None => return,
            };
            let _dropped = ring.close_active(); // AgentState drops → pump task cancelled
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

    /// Snapshot the current ring to disk. Called after every ring mutation
    /// (new/close/switch) and from the pump after a slot's attach resolves.
    /// Best-effort: any failure to write is silently ignored.
    fn save_agent_ring(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let Some(ring) = self.agent_ring() else {
            return;
        };
        save_persisted_acp_sessions(&cwd, ring);
    }

    /// Build a `AgentState` with ACP attach thread and pump task.
    /// The returned state is ready to be pushed into a `AgentRing`.
    fn create_agent_session(
        &mut self,
        resume_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> AgentState {
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let cwd = std::env::current_dir().ok();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    cwd,
                    resume_id,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        let editor = Editor::new(String::new(), PathBuf::from("*claude*"));

        // We'll assign the session index after push — use a sentinel
        // that the pump will look up dynamically. The pump captures
        // a stable session_index once the ring assigns one.
        // For now, peek the next_index from the ring.
        let session_index = match self.agent_ring() {
            Some(ring) => ring.next_index,
            None => 0, // first session — ring doesn't exist yet
        };

        let pump = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;
            let idle_delay = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let min_cycle = Duration::from_millis(16);
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
                    let more = match this.update(cx, |this, cx| {
                        this.pump_session(session_index, cx)
                    }) {
                        Ok(more) => more,
                        Err(_) => return,
                    };
                    if !more {
                        break;
                    }
                    cx.background_executor().timer(yield_delay).await;
                }
                let elapsed = cycle_start.elapsed();
                if elapsed < min_cycle {
                    cx.background_executor()
                        .timer(min_cycle - elapsed)
                        .await;
                }
            }
        });

        AgentState {
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
            awaiting_reply: false,
            turn_started: None,
            last_seen_turns: 0,
            tool_calls: std::collections::HashMap::new(),
            tool_call_order: Vec::new(),
            tool_call_anchor_line: std::collections::HashMap::new(),
            expanded_tool_calls: std::collections::HashSet::new(),
            block_ranges: Vec::new(),
            block_cache: std::collections::HashMap::new(),
            block_cache_frozen_count: 0,
            input_mode: InputMode::Chatbox,
            chatbox: Some(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            usage: None,
            subagents: Vec::new(),
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            _pump: Some(pump),
        }
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
        let (has_events, more_pending, attached_with_id, is_active) = {
            let ring = match self.agent_ring_mut() {
                Some(r) => r,
                None => return false,
            };
            let is_active = ring.slots.get(ring.active)
                .map(|s| s.index == session_index)
                .unwrap_or(false);
            let slot = match ring.slot_by_index_mut(session_index) {
                Some(s) => s,
                None => return false,
            };
            let claude = &mut slot.state;

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
                        claude.status = Some(format!("attached: {label}").into());
                        attach_resolved = true;
                    }
                    Ok(Err(e)) => {
                        claude.channel = None;
                        claude.status = Some(
                            format!("attach failed: {e} (set SKETCH_ACP_AGENT=...?)").into(),
                        );
                        attach_resolved = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        claude.status =
                            Some("attach worker died before reporting result".into());
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
                claude.turn_started = None;
                claude.status = Some("agent disconnected".into());
                cx.notify();
                return false;
            }

            // 3) Drain up to PUMP_EVENT_BUDGET reply events.
            let mut events: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
            let mut current_turns = claude.last_seen_turns;
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
            let turn_ended = !more_pending && current_turns > claude.last_seen_turns;
            let has_events = !events.is_empty() || turn_ended;
            if has_events {
                Self::apply_reply_events(claude, events);
                if turn_ended {
                    let mut tail: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
                    if let Some(client) = &claude.channel {
                        while let Some(ev) = client.try_recv() {
                            tail.push(ev);
                        }
                    }
                    Self::apply_reply_events(claude, tail);
                    finalize_agent_turn(&mut claude.editor);
                    claude.last_seen_turns = current_turns;
                    claude.awaiting_reply = false;
                    claude.turn_started = None;
                }
                // Spec §19 auto-scroll. In Chatbox mode the user's
                // cursor isn't in the transcript so chunks land with
                // sticky-bottom behavior. In Worksheet mode the
                // viewport stays anchored to the cursor; the one
                // exception is the cursor-at-EOF case (the user is
                // typing at the tail and wants to keep seeing the
                // freshly streamed output).
                let line_count = claude.editor.document().line_count();
                let cursor_at_eof = claude.editor.cursor().line + 1 >= line_count;
                let follow_chunks = match claude.input_mode {
                    InputMode::Chatbox => true,
                    InputMode::Worksheet => cursor_at_eof,
                };
                if follow_chunks && claude.list_item_count > 0 {
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

        // Mark inactive sessions with unseen activity.
        if has_events && !is_active {
            if let Some(ring) = self.agent_ring_mut() {
                if let Some(slot) = ring.slot_by_index_mut(session_index) {
                    slot.has_unseen_activity = true;
                }
            }
        }

        if has_events {
            cx.notify();
        }
        more_pending
    }

    /// Apply a batch of reply events to the AgentState. Text chunks are
    /// spliced into the buffer; tool calls land in `tool_calls` and are
    /// anchored to whatever buffer line is the current end-of-frozen so
    /// the renderer can slot the tool block in between text on either
    /// side. Updates merge into existing tool calls via `ToolCall::update`.
    fn apply_reply_events(
        claude: &mut AgentState,
        events: Vec<sketch::acp_channel::ReplyEvent>,
    ) {
        use sketch::acp_channel::ReplyEvent;
        // In-progress turn for tagging streamed content. `last_seen_turns`
        // only ticks up when the agent's prompt response resolves; while
        // chunks are streaming for the turn in flight, that turn is k =
        // last_seen_turns + 1 (spec §11, §E3).
        let current_turn = claude.last_seen_turns + 1;
        for ev in events {
            match ev {
                ReplyEvent::Chunk(text) => {
                    // Spec §E3: append at the end of the last frozen line
                    // tagged with this turn (mid-line for in-progress
                    // continuation, EOF for a new turn). Editable user
                    // lines anywhere else in the document stay put.
                    claude
                        .editor
                        .append_llm_chunk(TurnId::Llm(current_turn), text.as_str());
                }
                ReplyEvent::ToolCallStarted(mut tc) => {
                    cap_tool_call_payloads(&mut tc);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor);
                    let id = tc.tool_call_id.0.to_string();
                    claude.tool_call_anchor_line.insert(id.clone(), anchor);
                    // Tag the anchor with `Tool(k)` so the gutter shows
                    // `Tk` on tool-group anchor lines (§11).
                    claude
                        .editor
                        .metadata_mut::<TurnId>()
                        .insert(anchor, TurnId::Tool(current_turn));
                    // Sub-agent classification (§25). Flat — nested tool
                    // calls also classify here and become top-level
                    // sub-agent entries.
                    if let Some(sa) = classify_subagent(&tc) {
                        claude.subagents.push(sa);
                    }
                    if !claude.tool_calls.contains_key(&id) {
                        claude.tool_call_order.push(id.clone());
                    }
                    claude.tool_calls.insert(id, tc);
                }
                ReplyEvent::ToolCallUpdated(upd) => {
                    let id = upd.tool_call_id.0.to_string();
                    if let Some(existing) = claude.tool_calls.get_mut(&id) {
                        existing.update(upd.fields);
                        cap_tool_call_payloads(existing);
                        // Keep sub-agent mirror up to date: status +
                        // accumulated content. Done after the mutation
                        // so the latest state lands in the sidepane.
                        if let Some(sa) = claude
                            .subagents
                            .iter_mut()
                            .find(|s| s.tool_call_id == id)
                        {
                            sa.status = existing.status;
                            sa.transcript = existing.content.clone();
                        }
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
                        if let Some(sa) = classify_subagent(&tc) {
                            claude.subagents.push(sa);
                        }
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
            }
        }
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
                claude.status = Some(format!("permission mode → {}", m.short_label()).into());
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
        claude.awaiting_reply = false;
        claude.turn_started = None;
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

        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let cwd = std::env::current_dir().ok();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    cwd,
                    None,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        if let Some(ring) = self.agent_ring_mut() {
            ring.active_mut().resume_id = None;
            let claude = &mut ring.active_mut().state;
            claude.attach_pending = Some(attach_rx);
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

    /// Set focused sub-agent index (§27). The main transcript swap is
    /// purely a render-time decision; this just flips the field.
    fn focus_subagent(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            if idx < c.subagents.len() {
                c.focused_subagent = Some(idx);
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
        match claude.input_mode {
            InputMode::Chatbox => {
                let text = claude
                    .chatbox
                    .as_ref()
                    .map(|cb| cb.text())
                    .unwrap_or_default();
                claude.chatbox = None;
                claude.input_mode = InputMode::Worksheet;
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
            InputMode::Worksheet => {
                claude.input_mode = InputMode::Chatbox;
                claude.chatbox = Some(Chatbox::new());
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
        let mode = match self.agent_mut() {
            Some(c) => c.input_mode,
            None => return,
        };
        match mode {
            InputMode::Worksheet => self.submit_worksheet(cx),
            InputMode::Chatbox => self.submit_chatbox(cx),
        }
    }

    /// Worksheet submit per §12. Sweep every editable line in document
    /// order, build the prompt body from those with non-whitespace content
    /// (`\n`-joined), freeze every collected line — including blank
    /// spacers — and tag each with `TurnId::User(k)` so the gutter shows
    /// `Uk`. If the body is empty, no-op with a footer hint.
    fn submit_worksheet(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        if claude.channel.is_none() {
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

        // Determine the new turn number. `last_seen_turns` counts
        // completed turns; the next submit becomes turn k = last_seen + 1.
        let turn_k = claude.last_seen_turns + 1;

        // Freeze every collected line (blanks included) as part of turn k.
        // Freeze line by line so the line range stays accurate even if
        // editable lines are non-contiguous.
        for (l, _) in &collected {
            claude.editor.add_frozen_lines(*l, *l + 1);
            let anchor = claude.editor.anchor_for_line(*l);
            claude
                .editor
                .metadata_mut::<TurnId>()
                .insert(anchor, TurnId::User(turn_k));
        }

        // Send and update bookkeeping. `last_seen_turns` only ticks up
        // when the agent's prompt response resolves, so we don't bump it
        // here — `awaiting_reply` flips on instead.
        if let Some(channel) = claude.channel.as_mut() {
            let _ = channel.send(&prompt_body);
            claude.awaiting_reply = true;
            claude.turn_started = Some(std::time::Instant::now());
        }
        claude.editor.clear_selection();
        cx.notify();
    }

    /// Chatbox submit per §18. Take the full chatbox text, append it at
    /// EOF of the transcript as new lines, immediately freeze them with
    /// `TurnId::User(k)`, send via the channel, clear the chatbox. Mode
    /// stays `Chatbox`.
    fn submit_chatbox(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        let text = match &claude.chatbox {
            Some(cb) => cb.text(),
            None => return,
        };
        if text.trim().is_empty() {
            claude.status = Some("nothing to send".into());
            cx.notify();
            return;
        }
        if claude.channel.is_none() {
            claude.status = Some("no channel attached".into());
            cx.notify();
            return;
        }

        // Ensure the transcript ends with a newline so the appended draft
        // starts on its own line.
        if !claude.editor.document().full_text().ends_with('\n')
            && !claude.editor.document().full_text().is_empty()
        {
            let eof = claude.editor.document().rope().len_chars();
            claude.editor.programmatic_insert(eof, "\n");
        }
        let start_line = claude.editor.document().line_count().saturating_sub(1);
        let to_append = text.strip_suffix('\n').unwrap_or(&text).to_string();
        let eof = claude.editor.document().rope().len_chars();
        claude.editor.programmatic_insert(eof, &to_append);
        // Ensure terminating newline so the next chunk starts cleanly.
        if !claude.editor.document().full_text().ends_with('\n') {
            let eof2 = claude.editor.document().rope().len_chars();
            claude.editor.programmatic_insert(eof2, "\n");
        }

        let end_line = claude.editor.document().line_count();
        let turn_k = claude.last_seen_turns + 1;
        // Freeze + tag each newly appended line.
        claude.editor.add_frozen_lines(start_line, end_line);
        for l in start_line..end_line {
            let anchor = claude.editor.anchor_for_line(l);
            claude
                .editor
                .metadata_mut::<TurnId>()
                .insert(anchor, TurnId::User(turn_k));
        }

        // Send.
        let prompt_body = text.trim_end_matches('\n').to_string();
        if let Some(channel) = claude.channel.as_mut() {
            let _ = channel.send(&prompt_body);
            claude.awaiting_reply = true;
            claude.turn_started = Some(std::time::Instant::now());
        }

        // Reset the chatbox to empty; cursor stays inside.
        claude.chatbox = Some(Chatbox::new());
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
                c.chatbox = None;
                c.input_mode = InputMode::Worksheet;
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
            .map(|c| c.input_mode == InputMode::Chatbox && c.chatbox.is_some())
            .unwrap_or(false);
        if in_chatbox {
            let outcome = {
                let claude = match self.agent_mut() {
                    Some(c) => c,
                    None => return,
                };
                claude.status = None;
                let cb = claude.chatbox.as_mut().unwrap();
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
            let target = cursor_visible_child_index(c, cursor_line, &ranges);
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

impl Focusable for SketchGpuiView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SketchGpuiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_overlay = self.menu.is_some()
            || self.buffer_switcher.is_some()
            || self.rename_overlay.is_some();

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

        let screen_view: AnyElement = self.render_focused_window(screen_root, !has_overlay, cx);

        // When there's more than one tab, stack the tab strip above the
        // screen view. Single-tab workspaces render no strip — matches the
        // spec for "always show strip when >= 1 tab" but conservatively
        // suppresses it for the most common case (one-tab session) while
        // tab-creation commands are still landing.
        let screen_view = self.wrap_with_tab_strip(screen_view, cx);

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
        if self.rename_overlay.is_some() {
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
        if self.buffer_switcher.is_some() {
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
        let ctx = RenderCtx {
            theme: &self.theme,
            body_font: self.body_font.clone(),
            code_font: self.code_font.clone(),
            text_scale: self.text_scale,
            cursor_block: Some(d.cursor_block),
            doc_selection: self.doc_selection,
            line_layouts: Some(&self.line_layouts),
            current_block: None,
            weak_view: Some(cx.entity().downgrade()),
            doc_dir,
        };

        // Render every block. The body div is overflow-y-scroll and tracks
        // a ScrollHandle, so trackpad/mouse scrolling works natively and
        // j/k/g/G drive `scroll_to_item(cursor_block)` programmatically.
        let mut body = div()
            .id("doc-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .overflow_y_scroll()
            .track_scroll(&d.scroll_handle)
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(self.editor_fg());
        for (i, b) in d.blocks.iter().enumerate() {
            body = body.child(block_element(&ctx, i, b));
        }
        // View-mode mouse selection: anchor on left MouseDown, update head on
        // every MouseMove while a button is held, release on MouseUp. The
        // doc body is the listener for all three; hit-testing falls through
        // to the registered per-line TextLayouts in `self.line_layouts`.
        body = body
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
            .child(format!("sketch-gpui — {}", d.file_label));

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
            .on_action(cx.listener(Self::rename_tab))
            .child(header)
            .child(body)
            .child(footer)
    }

    fn render_edit(
        &self,
        root: gpui::Div,
        e: &EditState,
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
            .child(format!("sketch-gpui [{}] — {}", header_view_label, e.file_label));

        let dirty_mark = if e.editor.document().is_modified() { "•" } else { " " };
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
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::close_window))
            .child(header)
            .child(body)
            .child(footer)
    }

    /// Code (raw markdown) view: monospace, gutter with line numbers,
    /// per-line `md_highlight` source colors. Cursor splice via the shared
    /// `build_line_content` helper.
    fn build_edit_body_code(&self, e: &EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let line_count = e.editor.document().line_count();
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let dim_fg: Hsla = rgb(0x6272a4).into();

        let lines: Vec<String> = (0..line_count.max(1))
            .map(|i| {
                e.editor
                    .document()
                    .line_text(i)
                    .trim_end_matches('\n')
                    .replace('\t', "    ")
            })
            .collect();
        let highlighted = highlight_markdown_lines(&lines, &self.theme);

        let mut body = div()
            .id("edit-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_2()
            .overflow_y_scroll()
            .track_scroll(&e.scroll_handle)
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.code_font.clone())
            .text_color(self.editor_fg());

        let base_style = self.theme.paragraph;
        let sel = e.editor.selection_range();
        for (line_idx, line_str) in lines.iter().enumerate() {
            let mut segs = highlighted
                .get(line_idx)
                .cloned()
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
                .text_color(dim_fg)
                .child(format!("{:>3} ", line_idx + 1));

            let content = build_line_content(
                &segs,
                line_str,
                line_idx == cursor_line,
                cursor_col,
                e.mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                &self.code_font,
                &self.code_font,
            );

            body = body.child(div().flex().flex_row().child(gutter).child(content));
        }

        body
    }

    /// Word-Processor view: proportional body font + per-line typographic
    /// styling driven by `classify_wp_line`. Headings get larger sizes and
    /// bold weight; lists/blockquote/code get block-level decorations.
    /// `md_highlight`'s segments still carry inline `**bold**`/`*italic*`
    /// modifiers, which `font_for` maps to FontWeight/FontStyle on render.
    /// No gutter — word processors don't show line numbers.
    fn build_edit_body_wp(&self, e: &EditState) -> impl IntoElement {
        let cursor = e.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let line_count = e.editor.document().line_count();
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();

        let lines: Vec<String> = (0..line_count.max(1))
            .map(|i| {
                e.editor
                    .document()
                    .line_text(i)
                    .trim_end_matches('\n')
                    .replace('\t', "    ")
            })
            .collect();
        let highlighted = highlight_markdown_lines(&lines, &self.theme);

        let mut body = div()
            .id("edit-body-wp")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .py_4()
            .overflow_y_scroll()
            .track_scroll(&e.scroll_handle)
            .text_size(px(14.0 * self.text_scale))
            .font_family(self.body_font.clone())
            .text_color(self.editor_fg());

        let base_style = self.theme.paragraph;
        let sel = e.editor.selection_range();
        let mut in_fence = false;

        for (line_idx, line_str) in lines.iter().enumerate() {
            let kind = classify_wp_line(line_str, in_fence);
            if matches!(kind, WpLineKind::CodeFence) {
                in_fence = !in_fence;
            }

            let mut segs = highlighted
                .get(line_idx)
                .cloned()
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
            let text_size_px = raw_size_px * self.text_scale;
            let line_font = match kind {
                WpLineKind::CodeFence | WpLineKind::CodeContent | WpLineKind::TableRow => {
                    &self.code_font
                }
                _ => &self.body_font,
            };

            let content = build_line_content(
                &segs,
                line_str,
                line_idx == cursor_line,
                cursor_col,
                e.mode,
                cursor_color,
                base_style,
                DEFAULT_FG,
                line_font,
                &self.code_font,
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

            body = body.child(line_div);
        }

        body
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
        let session_count = ring.len();
        let active_session_idx = ring.active;

        // Sidebar data snapshot (collected before we borrow the active slot).
        let sidebar_entries: Vec<(usize, String, bool, bool, bool)> = ring.iter().enumerate().map(|(i, slot)| {
            let is_active = i == active_session_idx;
            let has_channel = slot.state.channel.is_some();
            (slot.index, slot.label.clone(), is_active, slot.has_unseen_activity, has_channel)
        }).collect();

        let active_slot_label = ring.active().label.clone();
        let c = &mut ring.active_mut().state;

        let cursor = c.editor.cursor();
        let cursor_line = cursor.line;
        let cursor_col = cursor.col;
        let line_count = c.editor.document().line_count();
        let cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
        let dim_fg: Hsla = rgb(0x6272a4).into();
        // Frozen Claude prose vs user-authored content get distinct bars so
        // the read/write boundary reads at a glance — same idiom as the
        // rendered-mode focused-block bar.
        let frozen_bar: Hsla = rgb(0x8be9fd).into();
        let user_bar: Hsla = rgb(0x50fa7b).into();

        let lines: Vec<String> = (0..line_count.max(1))
            .map(|i| {
                c.editor
                    .document()
                    .line_text(i)
                    .trim_end_matches('\n')
                    .replace('\t', "    ")
            })
            .collect();
        // Run md_highlight so Claude's markdown (`**bold**`, `` `code` ``,
        // headings, lists, blockquotes) renders styled instead of as raw
        // asterisks/backticks. The agent commonly returns markdown for
        // formatted text; without this pass it all reads as prose with
        // syntax noise.
        let highlighted = highlight_markdown_lines(&lines, &self.theme);
        let base_style = self.theme.paragraph;

        // Per-line gutter tag, sourced from the editor's `TurnId` metadata
        // keyed by `LineAnchor` (spec §11, §E2). Lines without a tag yet
        // (currently-editable, not yet swept by Submit) render as a blank
        // gutter. Lines whose anchor hasn't been allocated count as
        // untagged — happens for editable lines the user just typed.
        let gutter_tag_per_line: Vec<Option<TurnId>> = (0..lines.len())
            .map(|i| {
                c.editor
                    .anchor_for_line_opt(i)
                    .and_then(|a| c.editor.metadata::<TurnId>().get(a).copied())
            })
            .collect();

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
        let mut tools_at_line: std::collections::HashMap<usize, Vec<String>> =
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
        let frozen_ranges: Vec<(usize, usize)> = c.editor.frozen_lines().to_vec();
        let frozen_line_count: usize = frozen_ranges.iter().map(|(s, e)| e - s).sum();

        if frozen_line_count != c.block_cache_frozen_count {
            let block_ranges = detect_block_ranges(&lines, &frozen_ranges);
            let mut new_cache: std::collections::HashMap<(usize, usize), RenderedBlock> =
                std::collections::HashMap::new();
            for &(start, end) in &block_ranges {
                // Reuse existing cache entry if the range is unchanged.
                if let Some(cached) = c.block_cache.get(&(start, end)) {
                    new_cache.insert((start, end), cached.clone());
                } else if let Some(block) = parse_block_range(&lines, start, end, &self.theme) {
                    new_cache.insert((start, end), block);
                }
            }
            c.block_ranges = block_ranges;
            c.block_cache = new_cache;
            c.block_cache_frozen_count = frozen_line_count;
        }

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

        // Flat ordering: line_0, tool_group_at[0], line_1, …
        // Lines inside a detected block range are replaced by one
        // FlatItem::Block at the range start; interior lines are skipped.
        let mut flat_items: Vec<FlatItem> = Vec::with_capacity(lines.len() * 2);
        for line_idx in 0..lines.len() {
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

        // Splice ListState to match new item count. When block ranges
        // are active, line count can shrink unpredictably, so always
        // reset. Otherwise use incremental splice for height cache.
        let new_count = flat_items.len();
        let old_count = c.list_item_count;
        if new_count != old_count {
            if !block_ranges.is_empty() || new_count < old_count {
                c.list_state.reset(new_count);
            } else {
                c.list_state.splice(old_count..old_count, new_count - old_count);
            }
            c.list_item_count = new_count;
        }

        // Snapshot data for the render closure. Cloned once per
        // render_agent call; the closure is then called only for
        // visible items.
        let lines_snap = lines.clone();
        let highlighted_snap = highlighted.clone();
        let gutter_tag_snap = gutter_tag_per_line.clone();
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
        let weak_self = cx.entity().downgrade();
        let flat_items_arc: std::rc::Rc<Vec<FlatItem>> =
            std::rc::Rc::new(flat_items);

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

                        // md_highlight segments + author tint.
                        let mut segs: Vec<Segment> = highlighted_snap
                            .get(line_idx)
                            .cloned()
                            .unwrap_or_else(|| vec![(line_str.clone(), base_style)]);
                        let author_tint: NColor = if is_frozen {
                            NColor::Rgb(0xa9, 0xd0, 0xe0)
                        } else {
                            NColor::Rgb(0xb8, 0xe0, 0x9a)
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
                                        &segs, s, e_col, SELECTION_BG,
                                    );
                                }
                            }
                        }

                        let content = build_wrapped_line(
                            &segs,
                            &line_str,
                            line_idx == cursor_line,
                            cursor_col,
                            mode_snap,
                            cursor_color,
                            base_style,
                            DEFAULT_FG,
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
                            rgb(0xb6c4d6)
                        } else {
                            rgb(DEFAULT_FG)
                        };

                        // Gutter tag from the editor's per-line `TurnId`
                        // metadata (spec §11): `N` for LLM lines, `Un`
                        // for user lines, `Tn` for tool-call anchor
                        // lines, blank for currently-editable
                        // (unsubmitted) lines.
                        let tag = gutter_tag_snap.get(line_idx).copied().flatten();
                        let (label_text, label_color): (SharedString, Hsla) = match tag {
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
                                rgb(0xf1fa8c).into(),
                            ),
                            None => ("   ".into(), rgb(0x6272a4).into()),
                        };
                        let row_bg: Hsla = if line_idx == cursor_line {
                            rgba(0x44475a55).into()
                        } else {
                            rgba(0x00000000).into()
                        };

                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
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
                            ("✗", rgb(0xff5555).into())
                        } else if has_in_progress {
                            ("◐", rgb(0xf1fa8c).into())
                        } else if all_completed {
                            ("●", rgb(0x50fa7b).into())
                        } else {
                            ("○", rgb(0x6272a4).into())
                        };

                        let header_title: String = if count == 1 {
                            let tc = calls[0];
                            if tc.title.is_empty() { "(tool)".into() } else { tc.title.clone() }
                        } else {
                            format!("Ran {} tool calls", count)
                        };

                        let arrow = if count > 1 {
                            if group_expanded { "▼" } else { "▶" }
                        } else {
                            let tc = calls[0];
                            let policy = tool_render_policy(tc);
                            if matches!(policy, ToolRenderPolicy::HeaderOnly) {
                                " "
                            } else if group_expanded { "▼" } else { "▶" }
                        };

                        let anchor_str = anchor.to_string();
                        let weak = weak_self.clone();
                        let click_id = anchor_str.clone();
                        let header_row = div()
                            .id(SharedString::from(format!("tool-group-{}", anchor)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .pl_4()
                            .cursor_pointer()
                            .child(div().text_color(rgb(0x6272a4)).child(arrow))
                            .child(div().text_color(group_color).child(group_glyph))
                            .child(div().flex_1().text_color(rgb(DEFAULT_FG)).text_size(px(12.0)).child(header_title))
                            .on_click(
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

                        let mut block = div()
                            .flex()
                            .flex_col()
                            .my_1()
                            .border_l_2()
                            .border_color(rgb(0x44475a))
                            .child(header_row);

                        // Expanded: show individual tool calls.
                        if group_expanded {
                            for tc in &calls {
                                let expanded_detail = expanded_snap.contains(&tc.tool_call_id.0.to_string());
                                block = block.child(
                                    build_tool_block_with_weak(
                                        tc,
                                        expanded_detail,
                                        &code_font_snap,
                                        weak_self.clone(),
                                    ),
                                );
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
                            .py_1()
                            .child(inner)
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
            .px_4()
            .py_2()
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
        let strip_dim: Hsla = rgb(0x6272a4).into();
        let strip_warm: Hsla = rgb(0xf1fa8c).into();
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

        // Sub-agent breadcrumb (only when focused).
        if let Some(idx) = c.focused_subagent {
            if let Some(sa) = c.subagents.get(idx) {
                let crumb = format!(" ⏵ {} ◂", sa.label);
                strip = strip.child(
                    div()
                        .pr_2()
                        .text_color(strip_warm)
                        .child(SharedString::from(crumb)),
                );
            }
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
        let display_turn = if c.awaiting_reply {
            completed_turns + 1
        } else {
            completed_turns
        };
        if display_turn > 0 || c.turn_started.is_some() {
            let elapsed_str = if let Some(t) = c.turn_started {
                let s = t.elapsed().as_secs();
                format!("{}:{:02}", s / 60, s % 60)
            } else {
                String::new()
            };
            let turn_color = if c.turn_started.is_some() {
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

        let in_chatbox = c.input_mode == InputMode::Chatbox && c.chatbox.is_some();
        let mode_label = if in_chatbox {
            match c.chatbox.as_ref().unwrap().mode {
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
        if c.awaiting_reply {
            left_status.push_str(" · …awaiting reply");
        }
        if let Some(msg) = &c.status {
            left_status.push_str("  [");
            left_status.push_str(msg);
            left_status.push(']');
        }
        let _ = dim_fg; // (reserved for future per-line dim styling)

        let hints = if in_chatbox {
            "Ctrl-Enter send · Ctrl-Alt-Enter worksheet · esc normal"
        } else {
            "Ctrl-Enter send · Ctrl-Alt-Enter chatbox · Ctrl-V back · i insert · esc normal"
        };

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
            .child(SharedString::from(hints));

        // Chatbox panel — rendered between body and footer when active.
        let compose_panel = if let Some(tb) = &c.chatbox {
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
            let sep_color: Hsla = rgb(0x6272a4).into();
            let compose_bg: Hsla = rgb(0x1e1e2e).into();
            let compose_cursor_color: Hsla = rgb(CURSOR_BAR_COLOR).into();
            let compose_base_style = self.theme.paragraph;
            let compose_code_font = self.code_font.clone();

            let sep_label = if compose_mode == EditMode::Insert {
                "── compose (insert) ──"
            } else {
                "── compose ──"
            };

            let separator = div()
                .px_4()
                .py(px(2.0))
                .text_color(sep_color)
                .text_size(px(11.0))
                .font_family(compose_code_font.clone())
                .child(SharedString::from(sep_label));

            let max_visible_h = 8.0 * 18.0f32; // ~8 lines at 13px text

            let mut compose_inner = div()
                .px_4()
                .py(px(4.0))
                .bg(compose_bg)
                .font_family(compose_code_font.clone())
                .text_size(px(13.0))
                .text_color(rgb(DEFAULT_FG));

            for (i, line_text) in compose_lines.iter().enumerate() {
                // Build segments with selection highlighting (same path as
                // the main chat body).
                let mut segs: Vec<Segment> =
                    vec![(line_text.clone(), compose_base_style)];
                if let Some(sel) = compose_sel {
                    let line_chars = line_text.chars().count();
                    if let Some((s, e_col)) =
                        line_selection_range(sel, i, line_chars)
                    {
                        if e_col > s {
                            segs =
                                apply_selection_bg(&segs, s, e_col, SELECTION_BG);
                        }
                    }
                }

                compose_inner = compose_inner.child(build_wrapped_line(
                    &segs,
                    line_text,
                    i == compose_cursor_line,
                    compose_cursor_col,
                    compose_mode,
                    compose_cursor_color,
                    compose_base_style,
                    DEFAULT_FG,
                    &compose_code_font,
                ));
            }

            // Wrap in a scroll container with max height so compose
            // doesn't consume the entire screen. GPUI's native scroll
            // handles wrapped lines correctly (fixed pixel height +
            // manual line windowing can't account for flex-wrap).
            let compose_scroll = tb.scroll_handle.clone();
            compose_scroll.scroll_to_item(compose_cursor_line);
            let compose_body = div()
                .id("compose-scroll")
                .max_h(px(max_visible_h))
                .overflow_y_scroll()
                .track_scroll(&compose_scroll)
                .child(compose_inner);

            Some(div().child(separator).child(compose_body))
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
        let pane_border: Hsla = rgb(0x44475a).into();
        let pane_header_fg: Hsla = rgb(0x8be9fd).into();
        let pane_dim_fg: Hsla = rgb(0x6272a4).into();
        let pane_bg: Hsla = rgb(0x21222c).into();

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
            if c.subagents.is_empty() {
                pane = pane.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(pane_dim_fg)
                        .child(SharedString::new_static("(no subagents)")),
                );
            } else {
                use sketch::acp_channel::ToolCallStatus;
                let focused_idx = c.focused_subagent;
                for (i, sa) in c.subagents.iter().enumerate() {
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
                    let is_focused = focused_idx == Some(i);
                    let row_fg: Hsla = if is_focused {
                        rgb(0xf1fa8c).into()
                    } else {
                        rgb(DEFAULT_FG).into()
                    };
                    let row_bg: Hsla = if is_focused {
                        rgba(0x44475a55).into()
                    } else {
                        rgba(0x00000000).into()
                    };
                    let weak = cx.entity().downgrade();
                    let row = div()
                        .id(SharedString::from(format!("subagent-row-{}", i)))
                        .px_2()
                        .py(px(1.0))
                        .cursor_pointer()
                        .text_color(row_fg)
                        .bg(row_bg)
                        .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                            let _ = weak.update(app, |this, cx| {
                                this.focus_subagent(i, cx);
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

        // Session sidebar — visible when more than one session exists.
        let editor_bg = self.editor_bg();
        let editor_fg = self.editor_fg();
        let content_area: gpui::AnyElement = if session_count > 1 {
            let weak_self = cx.entity().downgrade();
            let mut sidebar = div()
                .id("session-sidebar")
                .flex()
                .flex_col()
                .w(px(160.0))
                .min_w(px(160.0))
                .border_r_1()
                .border_color(rgb(0x44475a))
                .bg(editor_bg)
                .py_1()
                .overflow_y_scroll();

            for (slot_index, label, is_active, has_unseen, has_channel) in &sidebar_entries {
                let slot_index = *slot_index;
                let truncated: String = if label.len() > 16 {
                    format!("{}…", &label[..15])
                } else {
                    label.clone()
                };
                let prefix = if *is_active {
                    "● "
                } else if *has_unseen {
                    "• "
                } else {
                    "  "
                };
                let suffix = if !has_channel { " [d]" } else { "" };
                let display = format!("{prefix}{truncated}{suffix}");

                let weak = weak_self.clone();
                let mut row = div()
                    .id(SharedString::from(format!("session-{slot_index}")))
                    .px_2()
                    .py(px(2.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                        let _ = weak.update(app, |this, cx| {
                            if let Some(ring) = this.agent_ring_mut() {
                                if let Some(pos) = ring.slot_by_index(slot_index) {
                                    ring.active = pos;
                                    ring.slots[pos].has_unseen_activity = false;
                                }
                            }
                            cx.notify();
                        });
                    });

                if *is_active {
                    row = row
                        .bg(rgb(0x44475a))
                        .font_weight(FontWeight::BOLD)
                        .text_color(editor_fg);
                } else if *has_unseen {
                    row = row.text_color(rgb(0xf1fa8c));
                } else {
                    row = row.text_color(rgb(0x6272a4));
                }

                row = row.child(SharedString::from(display));
                sidebar = sidebar.child(row);
            }

            // [+] button at the bottom
            let weak_new = cx.entity().downgrade();
            sidebar = sidebar.child(
                div()
                    .id("session-new-btn")
                    .px_2()
                    .py(px(2.0))
                    .text_size(px(12.0))
                    .text_color(rgb(0x6272a4))
                    .cursor_pointer()
                    .child("  [+] new")
                    .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut App| {
                        let _ = weak_new.update(app, |this, cx| {
                            this.new_agent_session(cx);
                        });
                    }),
            );

            // Transcript row: body | tasklist | subagents (panes only
            // appear when open per §1–§2). Below the row, the chatbox
            // panel spans full width.
            let mut transcript_row = div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(div().flex_1().min_w_0().child(body));
            if let Some(p) = tasklist_pane {
                transcript_row = transcript_row.child(p);
            }
            if let Some(p) = subagents_pane {
                transcript_row = transcript_row.child(p);
            }

            let mut right_col = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(transcript_row);
            if let Some(panel) = compose_panel {
                right_col = right_col.child(panel);
            }

            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(sidebar)
                .child(right_col)
                .into_any_element()
        } else {
            // No session sidebar. Same transcript-row + chatbox stack
            // as the multi-session branch, without the left column.
            let mut transcript_row = div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(div().flex_1().min_w_0().child(body));
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
                .min_h_0()
                .child(transcript_row);
            if let Some(panel) = compose_panel {
                col = col.child(panel);
            }
            col.into_any_element()
        };

        root
            .key_context("AgentView")
            .on_key_down(cx.listener(Self::handle_claude_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(|this, _: &ToggleTasklist, _w, cx| {
                this.toggle_tasklist(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSubagents, _w, cx| {
                this.toggle_subagents(cx);
            }))
            .child(header)
            .child(content_area)
            .child(footer)
    }

    fn render_browser(
        &self,
        root: gpui::Div,
        b: &BrowserWindow,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let entries: Vec<&BrowserEntry> = b.fb.visible_entries();
        let selected = b.fb.selected();
        let dir_str = b.fb.current_dir().display().to_string();

        // ---- Header (breadcrumb path) ----
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(rgb(0x282a36))
            .text_color(rgb(0x8be9fd))
            .font_weight(FontWeight::BOLD)
            .child(format!("▸ {}", dir_str));

        // ---- Entry list ----
        let mut list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .text_size(px(13.0))
            .font_family(self.body_font.clone());

        if entries.is_empty() {
            list = list.child(
                div()
                    .px_4()
                    .py_2()
                    .text_color(rgb(0x666666))
                    .child(SharedString::new_static("  (empty)")),
            );
        } else {
            // Simple per-row layout: marker · name · size · mtime
            // Keep the visible window around the selected row so j/k scroll.
            let visible_rows = 28usize; // approx; height-dependent layout is overkill here
            let scroll = scroll_to_keep_visible(selected, visible_rows, entries.len());
            for (i, entry) in entries.iter().enumerate().skip(scroll).take(visible_rows) {
                list = list.child(browser_row(entry, i == selected, &self.code_font));
            }
        }

        // ---- Hint bar ----
        let hint = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_1()
            .h(px(22.0))
            .bg(rgb(0x191928))
            .text_color(rgb(0x6272a4))
            .text_size(px(11.0))
            .child(format!(
                "enter:open · -:parent · .:hidden · s:sort({}) · q:close",
                b.fb.sort_order.label()
            ));

        root.key_context("BrowserView")
            .on_action(cx.listener(Self::browser_down))
            .on_action(cx.listener(Self::browser_up))
            .on_action(cx.listener(Self::browser_enter))
            .on_action(cx.listener(Self::browser_parent))
            .on_action(cx.listener(Self::browser_toggle_hidden))
            .on_action(cx.listener(Self::browser_cycle_sort))
            .on_action(cx.listener(Self::open_menu))
            .on_action(cx.listener(Self::browser_close))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::close_window))
            .child(header)
            .child(list)
            .child(hint)
    }
}

/// Choose `scroll_offset` so `selected` sits inside `[scroll, scroll+rows)`.
/// Mirrors the TUI's behavior of keeping a short margin around the cursor.
fn scroll_to_keep_visible(selected: usize, rows: usize, total: usize) -> usize {
    if total <= rows {
        return 0;
    }
    let margin = (rows / 4).max(2);
    if selected >= rows.saturating_sub(margin) {
        selected.saturating_sub(rows.saturating_sub(margin) - 1)
    } else {
        0
    }
    .min(total.saturating_sub(rows))
}

/// One row in the file-browser list.
fn browser_row(entry: &BrowserEntry, selected: bool, code_font: &SharedString) -> AnyElement {
    let row_bg = if selected {
        rgb(0x32334a)
    } else {
        rgb(0x1e1e2e)
    };
    let marker_color = rgb(0xbd93f9);
    let name_color = if entry.is_dir {
        rgb(0x8be9fd)
    } else {
        rgb(0xcccccc)
    };
    let meta_color = rgb(0x6272a4);

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
        WindowContent::Edit(e) => e.editor.document().is_modified(),
        WindowContent::Doc(d) => d
            .edit_cache
            .as_ref()
            .map_or(false, |ec| ec.editor.document().is_modified()),
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
            let blocks = render_with_wiki(&doc.full_text(), &theme);
            println!("sketch-gpui: loaded {} ({} blocks)", canon, blocks.len());
            Some((blocks, canon))
        }
        None => {
            println!("sketch-gpui: no file given, opening browser");
            None
        }
    };

    Application::new().run(move |app: &mut App| {
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
            KeyBinding::new("cmd-o", OpenBrowser, None),
            KeyBinding::new("cmd-k", OpenAgent, None),
            // Agent-window sidepane toggles (§32). Scoped to AgentView
            // so Cmd-1/Cmd-2 don't shadow anything in other screens.
            KeyBinding::new("cmd-1", ToggleTasklist, Some("AgentView")),
            KeyBinding::new("cmd-2", ToggleSubagents, Some("AgentView")),
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
            // Rename the active tab. Global so it works from any screen
            // (and the menu's "rename tab" entry uses the same path).
            KeyBinding::new("cmd-shift-r", RenameTab, None),
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
        ]);

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
                    // If we were launched with no explicit file arg, try to
                    // restore the saved workspace for this cwd. With an
                    // explicit arg the user wants that file, so the saved
                    // snapshot stays on disk for the next no-arg launch.
                    if initial_doc.is_none() {
                        view.restore_workspace_from_disk();
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

    fn s(text: &str) -> Segment {
        (text.to_string(), NStyle::default())
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
            "open-claude",
            "claude-send",
            "claude-reboot",
            "claude-mode-cycle",
            "claude-clear",
            "enter-edit",
            "enter-wp",
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
    fn menu_c_o_resolves_to_open_agent() {
        // `k` opens the claude submenu; `o` then resolves to open-claude.
        // Regression check that the Label node preceding `k` doesn't shadow
        // it (process_key must skip Labels).
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        let after_c = state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
        assert_eq!(after_c, None, "c alone should open the claude submenu");
        assert!(state.is_active(), "submenu open keeps menu state active");
        let cmd = state.process_key(KeyPress::new(Key::Char('o'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("open-claude".to_string()));
    }

    #[test]
    fn menu_c_s_resolves_to_claude_send() {
        // The reason this submenu exists: `<space> c s` is the muscle-memory
        // shortcut for "send the current draft to claude".
        let mut state = MenuState::new();
        state.open();
        let menu = gpui_menu();
        state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
        let cmd = state.process_key(KeyPress::new(Key::Char('s'), KMods::NONE), &menu);
        assert_eq!(cmd, Some("claude-send".to_string()));
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
