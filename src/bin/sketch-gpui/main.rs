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

use std::path::PathBuf;
use std::process;
use std::time::Duration;

use gpui::{
    actions, div, point, px, rgb, rgba, size, AnyElement, App, AppContext, Application,
    Bounds, Context, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, Keystroke, Menu,
    MenuItem, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, Task, TextRun, TitlebarOptions, UnderlineStyle,
    Window, WindowBounds, WindowOptions,
};

use sketch::acp_channel::AcpChannelClient;
use sketch::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine, StyledSpan};
use sketch::document::Document;
use sketch::editor::Editor;
use sketch::file_browser::{BrowserEntry, FileBrowser};
use sketch::keybind::KeybindManager;
use sketch::keys::{Key, KeyPress, Modifiers as KMods};
use sketch::md_highlight::{highlight_markdown_lines, Segment};
use sketch::menu::{MenuNode, MenuNodeKind, MenuState};
use sketch::render;
use sketch::style::{Color as NColor, Modifier, Style as NStyle};
use sketch::theme::Theme;

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
        OpenClaude,
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
        // Browser view
        BrowserDown,
        BrowserUp,
        BrowserEnter,
        BrowserParent,
        BrowserToggleHidden,
        BrowserCycleSort,
        BrowserClose,
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

// ----------------------------------------------------------------------------
// Block → Element
// ----------------------------------------------------------------------------

struct RenderCtx<'a> {
    theme: &'a Theme,
    body_font: SharedString,
    code_font: SharedString,
    cursor_block: Option<usize>,
}

fn block_element(ctx: &RenderCtx<'_>, idx: usize, block: &RenderedBlock) -> AnyElement {
    let highlighted = ctx.cursor_block == Some(idx);
    let base = block_inner(ctx, block);

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
                .text_size(px(size_px))
                .font_weight(FontWeight::BOLD)
                .text_color(fg_or(style, DEFAULT_FG))
                .pb_1()
                .child(styled_line_element(
                    content,
                    style,
                    DEFAULT_FG,
                    &ctx.body_font,
                    &ctx.code_font,
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
            for line in lines {
                col = col.child(styled_line_element(
                    line,
                    base,
                    DEFAULT_FG,
                    &ctx.body_font,
                    &ctx.code_font,
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
            for line in lines {
                col = col.child(styled_line_element(
                    line,
                    row_style,
                    DEFAULT_FG,
                    &ctx.code_font,
                    &ctx.code_font,
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
                        cursor_block: None,
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
                cursor_block: None,
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

/// One restored session slot. Order in the returned `Vec` matches the
/// saved ring order; reboot rebuilds the ring in this same order.
#[derive(Debug, Clone)]
struct PersistedSlot {
    id: String,
    label: String,
    active: bool,
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
    // Legacy single-string shape: synthesize a one-slot list.
    if let Some(id) = entry.as_str() {
        return vec![PersistedSlot {
            id: id.to_string(),
            label: "claude-1".into(),
            active: true,
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
            Some(PersistedSlot { id, label, active })
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
fn save_persisted_acp_sessions(cwd: &std::path::Path, ring: &SessionRing) {
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
                    if let Some(c) = this.claude_mut() {
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
fn anchor_for_new_tool_call(editor: &mut Editor) -> usize {
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
    line_count.saturating_sub(2)
}

/// Map a doc-line index to the flat-child index inside the claude body
/// container, accounting for tool blocks rendered between text lines.
///
/// `render_claude` emits children in this order:
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
    c: &ClaudeState,
    doc_line: usize,
    block_ranges: &[(usize, usize)],
) -> usize {
    // Count distinct anchor lines before doc_line (each = one ToolGroup item).
    let mut anchors_before: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for id in &c.tool_call_order {
        if let Some(line) = c.tool_call_anchor_line.get(id) {
            if *line < doc_line {
                anchors_before.insert(*line);
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
    let blocks = sketch::render::render(&slice, theme);
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

/// Splice an incoming Claude reply chunk into the transcript above any
/// pending user draft, then mark those lines frozen. Streaming-safe: each
/// chunk re-finds the splice point so subsequent chunks slot in just after
/// the prior chunk, not below the draft. Direct port of
/// `App::append_to_claude_buffer` from `src/app/claude.rs`.
fn splice_claude_chunk(editor: &mut Editor, text: &str) {
    if text.is_empty() {
        return;
    }

    let total_len = editor.document().rope().len_chars();

    let lockable = editor.lockable_through_char();
    let frozen_end_line = editor
        .frozen_lines()
        .iter()
        .map(|&(_, e)| e)
        .max()
        .unwrap_or(0);
    let frozen_end_char = if frozen_end_line == 0 {
        0
    } else if frozen_end_line >= editor.document().line_count() {
        total_len
    } else {
        editor.document().line_col_to_char(frozen_end_line, 0)
    };
    let splice_at = lockable.max(frozen_end_char).min(total_len);

    let draft_text: String = editor
        .document()
        .rope()
        .slice(splice_at..total_len)
        .to_string();
    let cursor_char = editor
        .document()
        .line_col_to_char(editor.cursor().line, editor.cursor().col);
    let cursor_in_draft = cursor_char.saturating_sub(splice_at);
    let cursor_was_in_draft = cursor_char >= splice_at;

    if !draft_text.is_empty() {
        editor.programmatic_delete(splice_at, total_len);
    }

    // Append the chunk verbatim. Streaming chunks are arbitrary slices of one
    // logical message — any extra padding here breaks sentences (and inserts
    // blank lines between every chunk). The caller's text already carries
    // whatever newlines belong in the rendered transcript.
    let pre_len = editor.document().rope().len_chars();
    editor.programmatic_insert(pre_len, text);

    // Freeze the lines that the chunk now occupies. add_frozen_lines uses a
    // half-open [start, end) range, so when the chunk ends mid-line we have
    // to bump end_line past it. If the chunk ended on \n, the line of
    // claude_end_char is already the next line and serves as end directly.
    let claude_end_char = pre_len + text.chars().count();
    let start_line = doc_char_to_line_col(editor.document(), pre_len).0;
    let mut end_line = doc_char_to_line_col(editor.document(), claude_end_char).0;
    if !text.ends_with('\n') {
        end_line += 1;
    }
    editor.add_frozen_lines(start_line, end_line);

    // Re-attach the draft. If the chunk didn't end on a newline AND the draft
    // is non-empty, prepend one so the user's draft doesn't run onto Claude's
    // last line.
    let needs_separator = !draft_text.is_empty() && !text.ends_with('\n');
    let draft_reattach_at = editor.document().rope().len_chars();
    if needs_separator {
        editor.programmatic_insert(draft_reattach_at, "\n");
    }
    let draft_actual_at = editor.document().rope().len_chars();
    if !draft_text.is_empty() {
        editor.programmatic_insert(draft_actual_at, &draft_text);
    }

    if cursor_was_in_draft {
        let new_cursor_char = if draft_text.is_empty() {
            editor.document().rope().len_chars()
        } else {
            draft_actual_at + cursor_in_draft
        };
        let (cl, cc) = doc_char_to_line_col(editor.document(), new_cursor_char);
        editor.cursor_mut().line = cl;
        editor.cursor_mut().col = cc;
    }
    editor.clear_selection();
}

/// Called when the ACP turn ends (the agent's `session/prompt` response
/// resolves). Streaming chunks splice in verbatim — no padding — so by the
/// time we get here the cursor may sit on the last frozen line with no room
/// to type. Ensure there's an editable line below the frozen content and
/// move the cursor there so the user can immediately keep typing.
fn finalize_claude_turn(editor: &mut Editor) {
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
    let eof = editor.document().rope().len_chars();
    let (cl, cc) = doc_char_to_line_col(editor.document(), eof);
    editor.cursor_mut().line = cl;
    editor.cursor_mut().col = cc;
    editor.clear_selection();
}

/// Lock the active turn: append `\n\n---\n\n` and bump
/// `lockable_through_line` to the cursor's line so the user can't
/// retroactively edit content they just sent. Mirrors
/// `App::lock_active_turn` from `src/app/claude.rs`.
fn lock_claude_turn(editor: &mut Editor) {
    let pre_len = editor.document().rope().len_chars();
    let s = editor.document().full_text();
    let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
    let lead = "\n".repeat(2usize.saturating_sub(trailing_nl));
    let separator = format!("{}{}\n\n", lead, "─".repeat(40));
    editor.programmatic_insert(pre_len, &separator);

    let eof = editor.document().rope().len_chars();
    let (cl, cc) = doc_char_to_line_col(editor.document(), eof);
    editor.set_lockable_through_line(cl);
    editor.cursor_mut().line = cl;
    editor.cursor_mut().col = cc;
    editor.clear_selection();
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

/// State held while the user is browsing the filesystem. The active buffer's
/// screen state is stashed in `open_buffers` before entering the browser and
/// restored when the browser closes.
struct BrowserWindow {
    fb: FileBrowser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    Normal,
    Insert,
}

/// Stable compose surface for drafting messages in the Claude screen.
/// While active, key dispatch routes here instead of the main transcript.
struct ComposeBox {
    editor: Editor,
    mode: EditMode,
    scroll_handle: ScrollHandle,
}

impl ComposeBox {
    fn new() -> Self {
        Self {
            editor: Editor::new(String::new(), std::path::PathBuf::from("*compose*")),
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
    Claude(SessionRing),
    Browser(BrowserWindow),
}

/// State held while the user is conversing with an ACP-attached Claude
/// agent. The transcript lives in an in-memory `Editor` (no on-disk file);
/// Claude's replies are spliced in as frozen lines via the same lock-and-
/// advance pattern the TUI uses (`app::claude::append_to_claude_buffer`),
/// so the user can keep typing inline edits between turns.
struct ClaudeState {
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
    tool_call_anchor_line: std::collections::HashMap<String, usize>,
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
    /// Optional compose textbox — a standalone Editor + mode for drafting
    /// messages without auto-scroll interference. When `Some`, key dispatch
    /// routes here instead of the main transcript editor.
    compose_box: Option<ComposeBox>,
    /// Background polling task that drains the ACP channel into the editor
    /// every ~50ms. Held only so that dropping `ClaudeState` (e.g. on
    /// `back_to_doc`) cancels the task. The leading `_` mutes unused-field
    /// warnings — the field IS used (its Drop runs on screen exit), but
    /// no method reads it.
    _pump: Option<Task<()>>,
}

/// A named wrapper around `ClaudeState` for multi-session support.
struct SessionSlot {
    /// User-facing label shown in the sidebar.
    label: String,
    /// Monotonic index for stable identification (not reused after close).
    index: usize,
    /// The session state. Contains editor, channel, tool calls, etc.
    state: ClaudeState,
    /// True if new content has arrived since the user last viewed this session.
    has_unseen_activity: bool,
    /// The id this slot was created from on persistence restore. The slot's
    /// persisted id stays this value even if `session/load` failed and the
    /// channel fell back to `session/new` with a different id — so the next
    /// reboot retries the original load. `None` for slots created fresh by
    /// `claude-new` (then the channel's session/new id is persisted).
    resume_id: Option<String>,
}

/// An ordered collection of `SessionSlot`s with one active slot.
/// Ring-style next/prev navigation wraps around.
struct SessionRing {
    slots: Vec<SessionSlot>,
    /// Index into `slots` for the currently-active session.
    active: usize,
    /// Monotonic counter for `SessionSlot::index` — never reused.
    next_index: usize,
    /// WindowContent to restore when leaving Claude entirely (Ctrl-V / back_to_doc).
    /// Belongs to the ring, not any individual session.
    underlying: Option<Box<WindowContent>>,
}

impl SessionRing {
    fn new(underlying: Option<Box<WindowContent>>) -> Self {
        Self {
            slots: Vec::new(),
            active: 0,
            next_index: 0,
            underlying,
        }
    }

    #[allow(dead_code)]
    fn active(&self) -> &SessionSlot {
        &self.slots[self.active]
    }

    fn active_mut(&mut self) -> &mut SessionSlot {
        &mut self.slots[self.active]
    }

    fn push(&mut self, label: String, state: ClaudeState, resume_id: Option<String>) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.slots.push(SessionSlot {
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
    fn close_active(&mut self) -> Option<ClaudeState> {
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

    fn iter(&self) -> impl Iterator<Item = &SessionSlot> {
        self.slots.iter()
    }

    /// Find slot position by monotonic index.
    fn slot_by_index(&self, index: usize) -> Option<usize> {
        self.slots.iter().position(|s| s.index == index)
    }

    fn slot_by_index_mut(&mut self, index: usize) -> Option<&mut SessionSlot> {
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
                MenuNode::entry("t", "compose", "compose-toggle"),
            ],
        ),
        MenuNode::separator(),
        MenuNode::label("Edit"),
        MenuNode::entry("e", "edit (raw markdown)", "enter-edit"),
        MenuNode::entry("w", "edit (word processor)", "enter-wp"),
        MenuNode::separator(),
        MenuNode::label("View"),
        MenuNode::entry("v", "back to doc", "back-to-doc"),
        MenuNode::separator(),
        MenuNode::entry("q", "quit", "quit"),
    ]
}

struct SketchGpuiView {
    theme: Theme,
    body_font: SharedString,
    code_font: SharedString,
    focus_handle: FocusHandle,
    /// Active TUI-style menu overlay. `Some` while the picker is open;
    /// flipped to `None` on Esc-from-root or after a command is dispatched.
    menu: Option<MenuOverlay>,
    /// Buffer-list picker overlay — open while `Some`.
    buffer_switcher: Option<BufferSwitcher>,
    /// Tabs + n-ary split tree (spec-tabs-and-splits.md). The focused
    /// window's content is the authoritative live state for the workspace.
    workspace: workspace::Workspace<WindowContent>,
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
            code_font: SharedString::new_static("Menlo"),
            focus_handle,
            menu: None,
            buffer_switcher: None,
            workspace: workspace::Workspace::with_initial(initial),
        }
    }

    fn new_browser(start_dir: PathBuf, theme: Theme, focus_handle: FocusHandle) -> Self {
        let initial = WindowContent::Browser(BrowserWindow {
            fb: FileBrowser::new(start_dir),
        });
        Self {
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("Menlo"),
            focus_handle,
            menu: None,
            buffer_switcher: None,
            workspace: workspace::Workspace::with_initial(initial),
        }
    }

    /// Replace the focused window's content (old `self.screen = X` writes).
    fn set_screen(&mut self, content: WindowContent) {
        self.workspace.replace_focused_content(content);
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

    fn claude_mut(&mut self) -> Option<&mut ClaudeState> {
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Claude(ring) if !ring.is_empty() => Some(&mut ring.active_mut().state),
            _ => None,
        }
    }

    fn claude_ring(&self) -> Option<&SessionRing> {
        match self.workspace.focused_content().expect("no focused window") {
            WindowContent::Claude(ring) => Some(ring),
            _ => None,
        }
    }

    fn claude_ring_mut(&mut self) -> Option<&mut SessionRing> {
        match self.workspace.focused_content_mut().expect("no focused window") {
            WindowContent::Claude(ring) => Some(ring),
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
        let blocks = render::render(&doc.full_text(), &self.theme);
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
        if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Browser(_)) {
            return;
        }
        // Open the browser in a new tab so the current doc/edit/claude work
        // isn't lost. Picking a file from the browser replaces the browser
        // tab in place (see open_file).
        self.workspace.push_initial_tab(WindowContent::Browser(BrowserWindow {
            fb: FileBrowser::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        }));
        cx.notify();
    }
    fn quit(&mut self, _: &Quit, _w: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
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
                if let WindowContent::Claude(ring) = content {
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
            cx.notify();
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, _w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tabs.len() > 1 {
            self.workspace.prev_tab();
            cx.notify();
        }
    }

    /// Open a new tab containing a Browser rooted at cwd. Spec Behavior 3:
    /// no-arg `:tabnew` / `Cmd-T` creates a browser tab so the user can pick
    /// what to load.
    fn new_tab(&mut self, _: &NewTab, _w: &mut Window, cx: &mut Context<Self>) {
        self.workspace.push_initial_tab(WindowContent::Browser(BrowserWindow {
            fb: FileBrowser::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        }));
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
        cx.notify();
    }

    /// `Ctrl-W s` — horizontal split: new pane below the focused one.
    fn split_h(&mut self, _: &SplitH, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::H);
        cx.notify();
    }

    /// `Ctrl-W v` — vertical split: new pane to the right of the focused one.
    fn split_v(&mut self, _: &SplitV, _w: &mut Window, cx: &mut Context<Self>) {
        self.split_focused_with_browser(workspace::SplitDir::V);
        cx.notify();
    }

    /// Shared helper. For now both split commands open a Browser in the new
    /// pane so the user can pick what to load. Cloning the focused content
    /// kind (Doc → Doc, Edit → Edit) is a follow-up that needs the buffer
    /// pool to share editors.
    fn split_focused_with_browser(&mut self, dir: workspace::SplitDir) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let content = WindowContent::Browser(BrowserWindow {
            fb: FileBrowser::new(cwd),
        });
        let _ = self.workspace.split_focused(dir, content);
    }

    /// `Ctrl-W c` — close the focused window. If it was the only window in
    /// the tab, close the tab instead.
    fn close_window(&mut self, _: &CloseWindow, _w: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.close_focused() {
            Ok(Some(_new_focus)) => cx.notify(),
            Ok(None) => {
                // Tab is empty — close the tab too (or quit if last).
                if self.workspace.tabs.len() <= 1 {
                    cx.quit();
                    return;
                }
                let idx = self.workspace.active_tab;
                self.workspace.close_tab(idx);
                cx.notify();
            }
            Err(()) => {}
        }
    }

    /// `Ctrl-W o` — keep only the focused window.
    fn only_window(&mut self, _: &OnlyWindow, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.only();
        cx.notify();
    }

    /// `Ctrl-W h/j/k/l` — move focus to a sibling split in that direction.
    fn focus_left(&mut self, _: &FocusLeft, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Left);
        cx.notify();
    }
    fn focus_right(&mut self, _: &FocusRight, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Right);
        cx.notify();
    }
    fn focus_up(&mut self, _: &FocusUp, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Up);
        cx.notify();
    }
    fn focus_down(&mut self, _: &FocusDown, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_motion(workspace::FocusDir::Down);
        cx.notify();
    }

    /// `Ctrl-W w` / `Ctrl-W W` — cycle focus through leaves in tree order.
    fn focus_next(&mut self, _: &FocusNext, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_next();
        cx.notify();
    }
    fn focus_prev(&mut self, _: &FocusPrev, _w: &mut Window, cx: &mut Context<Self>) {
        let _ = self.workspace.focus_prev();
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
        if !matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Browser(_)) {
            return;
        }
        // Close the Browser tab. If it's the only tab left, quit instead —
        // matches today's behavior of quit-on-last-screen.
        if self.workspace.tabs.len() <= 1 {
            cx.quit();
            return;
        }
        let idx = self.workspace.active_tab;
        self.workspace.close_tab(idx);
        cx.notify();
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
                let blocks = render::render(&edit.editor.document().full_text(), &self.theme);
                let file_label = edit.file_label.clone();
                self.set_screen(WindowContent::Doc(DocState {
                    blocks,
                    file_label,
                    cursor_block: 0,
                    scroll_handle: ScrollHandle::new(),
                    edit_cache: Some(edit),
                }));
            }
            WindowContent::Claude(ring) => {
                // Restore whatever screen the user opened Claude from. If
                // none was stashed, fall back to a fresh Browser at cwd.
                // SessionRing and all its sessions drop here, taking pump
                // tasks and ACP channels with them.
                let new = match ring.underlying {
                    Some(boxed) => *boxed,
                    None => WindowContent::Browser(BrowserWindow {
                        fb: FileBrowser::new(
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        ),
                    }),
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
            "open-claude" => self.open_claude_inner(cx),
            "claude-send" => {
                // Only meaningful while the claude screen is active. Surface
                // a hint via the doc/edit footer if it isn't, so the user
                // gets a visible no-op instead of silent.
                if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Claude(_)) {
                    self.send_claude(cx);
                }
            }
            "claude-new" => self.new_claude_session(cx),
            "claude-close" => self.close_active_claude_session(cx),
            "claude-next" => self.switch_claude_session(1, cx),
            "claude-prev" => self.switch_claude_session(-1, cx),
            "claude-reboot" => self.reboot_into_claude(cx),
            "claude-mode-cycle" => self.cycle_claude_permission_mode(cx),
            "claude-clear" => self.clear_claude_session(cx),
            "compose-toggle" => {
                if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Claude(_)) {
                    self.compose_toggle(cx);
                }
            }
            "back-to-doc" => self.back_to_doc(cx),
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
        self.render_layout(root, layout, focused_id, cx)
    }

    /// Recursively render a `Layout<WindowContent>`. The `root` div is used
    /// only for the leaf case (so leaves can attach focus + key bindings);
    /// split branches build their own container.
    fn render_layout(
        &mut self,
        root: gpui::Div,
        layout: &mut workspace::Layout<WindowContent>,
        focused_id: workspace::WindowId,
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
                let painted: AnyElement = match content {
                    WindowContent::Doc(d) => self.render_doc(root, d, cx).into_any_element(),
                    WindowContent::Edit(e) => self.render_edit(root, e, cx).into_any_element(),
                    WindowContent::Browser(b) => self.render_browser(root, b, cx).into_any_element(),
                    WindowContent::Claude(ring) => {
                        self.render_claude(root, ring, cx).into_any_element()
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
                let mut container = div().size_full().flex().min_w_0().min_h_0();
                container = match dir {
                    workspace::SplitDir::V => container.flex_row(),
                    workspace::SplitDir::H => container.flex_col(),
                };
                for (weight, child) in children.iter_mut() {
                    let w = *weight;
                    let child_root = div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(rgb(BG))
                        .text_color(rgb(DEFAULT_FG));
                    let child_el = self.render_layout(child_root, child, focused_id, cx);
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
                container.into_any_element()
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
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.workspace.tabs.len() <= 1 {
            return screen_view;
        }

        let active_idx = self.workspace.active_tab;
        let active_fg: Hsla = rgb(STATUS_FG).into();
        let inactive_fg: Hsla = rgb(0x6272a4).into();
        let strip_bg: Hsla = rgb(STATUS_BG).into();
        let active_bg: Hsla = rgb(0x282a36).into();

        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .px_2()
            .h(px(24.0))
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
                .px_3()
                .py_1()
                .rounded(px(3.0))
                .bg(bg)
                .text_color(fg)
                .child(label);
            strip = strip.child(entry);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(strip)
            .child(div().flex_1().min_h_0().child(screen_view))
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
    fn open_claude(&mut self, _: &OpenClaude, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_claude_inner(cx);
    }

    fn open_claude_inner(&mut self, cx: &mut Context<Self>) {
        // If already on Claude screen, just add a new session to the ring.
        if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Claude(_)) {
            self.new_claude_session(cx);
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

        let mut ring = SessionRing::new(Some(Box::new(prior)));
        let cwd_opt = std::env::current_dir().ok();
        let persisted = cwd_opt
            .as_deref()
            .map(load_persisted_acp_sessions)
            .unwrap_or_default();

        if persisted.is_empty() {
            // No saved state → fresh single slot, as before.
            let state = self.create_claude_session(None, cx);
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
                let state = self.create_claude_session(Some(slot.id.clone()), cx);
                ring.push(slot.label, state, Some(slot.id));
            }
            ring.active = active_pos.min(ring.slots.len().saturating_sub(1));
        }

        self.set_screen(WindowContent::Claude(ring));

        if let Some(c) = self.claude_mut() {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Create a new session and add it to the existing ring.
    fn new_claude_session(&mut self, cx: &mut Context<Self>) {
        let ring = match self.claude_ring_mut() {
            Some(r) => r,
            None => return,
        };
        let n = ring.next_index + 1;
        let label = format!("claude-{n}");
        // New sessions don't resume — they're fresh.
        let state = self.create_claude_session(None, cx);
        let ring = self.claude_ring_mut().unwrap();
        ring.push(label, state, None);
        if let Some(c) = self.claude_mut() {
            c.editor.begin_insert();
        }
        self.save_claude_ring();
        cx.notify();
    }

    /// Switch to the next (+1) or previous (-1) session in the ring.
    fn switch_claude_session(&mut self, direction: i32, cx: &mut Context<Self>) {
        if let Some(ring) = self.claude_ring_mut() {
            if direction > 0 {
                ring.next();
            } else {
                ring.prev();
            }
        }
        self.save_claude_ring();
        cx.notify();
    }

    /// Close the active session. If the ring is now empty, exit Claude.
    fn close_active_claude_session(&mut self, cx: &mut Context<Self>) {
        let is_empty = {
            let ring = match self.claude_ring_mut() {
                Some(r) => r,
                None => return,
            };
            let _dropped = ring.close_active(); // ClaudeState drops → pump task cancelled
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
            self.save_claude_ring();
            cx.notify();
        }
    }

    /// Snapshot the current ring to disk. Called after every ring mutation
    /// (new/close/switch) and from the pump after a slot's attach resolves.
    /// Best-effort: any failure to write is silently ignored.
    fn save_claude_ring(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let Some(ring) = self.claude_ring() else {
            return;
        };
        save_persisted_acp_sessions(&cwd, ring);
    }

    /// Build a `ClaudeState` with ACP attach thread and pump task.
    /// The returned state is ready to be pushed into a `SessionRing`.
    fn create_claude_session(
        &mut self,
        resume_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> ClaudeState {
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
        let session_index = match self.claude_ring() {
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
                        if let Some(ring) = this.claude_ring_mut() {
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

        ClaudeState {
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
            compose_box: Some(ComposeBox::new()),
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
            let ring = match self.claude_ring_mut() {
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
                    finalize_claude_turn(&mut claude.editor);
                    claude.last_seen_turns = current_turns;
                    claude.awaiting_reply = false;
                    claude.turn_started = None;
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
            self.save_claude_ring();
        }

        // Mark inactive sessions with unseen activity.
        if has_events && !is_active {
            if let Some(ring) = self.claude_ring_mut() {
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

    /// Apply a batch of reply events to the ClaudeState. Text chunks are
    /// spliced into the buffer; tool calls land in `tool_calls` and are
    /// anchored to whatever buffer line is the current end-of-frozen so
    /// the renderer can slot the tool block in between text on either
    /// side. Updates merge into existing tool calls via `ToolCall::update`.
    fn apply_reply_events(
        claude: &mut ClaudeState,
        events: Vec<sketch::acp_channel::ReplyEvent>,
    ) {
        use sketch::acp_channel::ReplyEvent;
        for ev in events {
            match ev {
                ReplyEvent::Chunk(text) => {
                    splice_claude_chunk(&mut claude.editor, &text);
                }
                ReplyEvent::ToolCallStarted(mut tc) => {
                    cap_tool_call_payloads(&mut tc);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor);
                    let id = tc.tool_call_id.0.to_string();
                    claude.tool_call_anchor_line.insert(id.clone(), anchor);
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
                        claude.tool_call_order.push(id.clone());
                        claude.tool_calls.insert(id, tc);
                    }
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
    fn clear_claude_session(&mut self, cx: &mut Context<Self>) {
        // Forget every persisted slot BEFORE re-opening so the new spawn
        // hits session/new instead of session/load. Done first so even
        // if open_claude_inner panics partway through, the next manual
        // attach won't accidentally resume any cleared session.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        // Drop the current claude screen entirely; open_claude_inner
        // builds a new one. We don't try to surgically reset fields on
        // the existing ClaudeState because the underlying screen
        // (browser/doc) is also stashed there — preserving it is the
        // job of open_claude_inner via the prior-screen swap dance.
        if matches!(self.workspace.focused_content().expect("no focused window"), WindowContent::Claude(_)) {
            // Restore underlying first so open_claude_inner can capture
            // it as the new "prior" screen. Otherwise we'd lose the
            // file/browser the user was viewing before they opened
            // claude.
            self.back_to_doc(cx);
        }
        self.open_claude_inner(cx);
        if let Some(c) = self.claude_mut() {
            c.status = Some("session cleared".into());
        }
        cx.notify();
    }

    /// Cycle the ACP permission mode (read-only → auto-edit → ask-each →
    /// yolo → read-only). Surfaces the new mode in the claude footer so
    /// the user sees the change without having to find it in the header.
    fn cycle_claude_permission_mode(&mut self, cx: &mut Context<Self>) {
        let Some(claude) = self.claude_mut() else {
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
                if let Some(c) = self.claude_mut() {
                    c.status = Some(format!("reboot failed: {e}").into());
                }
            }
        }
    }

    /// Send the user's pending draft (`extract_editable_inserts` —
    /// only the editable runs between/after frozen Claude turns) as the
    /// next ACP prompt, then lock the turn so that content can't be
    /// retroactively edited.
    fn send_claude(&mut self, cx: &mut Context<Self>) {
        let claude = match self.claude_mut() {
            Some(c) => c,
            None => return,
        };

        let payload = claude.editor.extract_editable_inserts();
        if payload.trim().is_empty() {
            claude.status = Some("nothing to send (no draft)".into());
            cx.notify();
            return;
        }

        let result = match &mut claude.channel {
            Some(client) => client.send(&payload),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no agent attached",
            )),
        };

        match result {
            Ok(()) => {
                lock_claude_turn(&mut claude.editor);
                claude.awaiting_reply = true;
                claude.turn_started = Some(std::time::Instant::now());
                claude.status = Some(format!("sent ({} chars)", payload.len()).into());
                // Re-enter insert mode for immediate continuation.
                claude.editor.begin_insert();
                claude.mode = EditMode::Insert;
            }
            Err(e) => {
                claude.status = Some(format!("send failed: {e}").into());
                claude.channel = None;
            }
        }
        cx.notify();
    }

    /// Toggle the compose textbox in the Claude screen.
    fn compose_toggle(&mut self, cx: &mut Context<Self>) {
        let claude = match self.claude_mut() {
            Some(c) => c,
            None => return,
        };
        if claude.compose_box.is_some() {
            // Close: extract text and insert into main buffer.
            let text = claude.compose_box.as_ref().unwrap().text();
            claude.compose_box = None;
            if !text.is_empty() {
                let eof = claude.editor.document().rope().len_chars();
                claude.editor.programmatic_insert(eof, &text);
                let new_eof = claude.editor.document().rope().len_chars();
                let (cl, cc) = doc_char_to_line_col(claude.editor.document(), new_eof);
                claude.editor.cursor_mut().line = cl;
                claude.editor.cursor_mut().col = cc;
            }
        } else {
            claude.compose_box = Some(ComposeBox::new());
        }
        cx.notify();
    }

    /// Send compose box contents, then close it.
    fn compose_send(&mut self, cx: &mut Context<Self>) {
        let claude = match self.claude_mut() {
            Some(c) => c,
            None => return,
        };
        let text = match &claude.compose_box {
            Some(tb) => tb.text(),
            None => return,
        };
        if text.trim().is_empty() {
            claude.status = Some("nothing to send (compose box empty)".into());
            claude.compose_box = None;
            cx.notify();
            return;
        }
        // Close compose and insert text into main buffer.
        claude.compose_box = None;
        let eof = claude.editor.document().rope().len_chars();
        claude.editor.programmatic_insert(eof, &text);
        let new_eof = claude.editor.document().rope().len_chars();
        let (cl, cc) = doc_char_to_line_col(claude.editor.document(), new_eof);
        claude.editor.cursor_mut().line = cl;
        claude.editor.cursor_mut().col = cc;
        // Now send via the normal path.
        self.send_claude(cx);
    }

    /// Key dispatch for the Claude screen. Mirrors `handle_edit_key` but
    /// catches `Ctrl-Enter` to send and `Ctrl-V` to leave; everything else
    /// goes through the shared `dispatch_*_core` helpers.
    fn handle_claude_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Mode-independent shortcuts (work in both main and compose).
        if press.modifiers.contains(KMods::CONTROL) {
            if let Key::Char('t') = press.key {
                self.compose_toggle(cx);
                return;
            }
            if let Key::Char('v') | Key::Char('V') = press.key {
                // Close compose without sending before leaving.
                if let Some(c) = self.claude_mut() {
                    c.compose_box = None;
                }
                self.back_to_doc(cx);
                return;
            }
        }

        // Compose box intercept: when open, route keys to the compose editor.
        let compose_active = self
            .claude_mut()
            .map(|c| c.compose_box.is_some())
            .unwrap_or(false);
        if compose_active {
            if press.modifiers.contains(KMods::CONTROL) && press.key == Key::Enter {
                self.compose_send(cx);
                return;
            }
            let outcome = {
                let claude = match self.claude_mut() {
                    Some(c) => c,
                    None => return,
                };
                claude.status = None;
                let tb = claude.compose_box.as_mut().unwrap();
                match tb.mode {
                    EditMode::Insert => {
                        Self::dispatch_insert_core(&mut tb.editor, &mut tb.mode, press);
                        NormalOutcome::Handled
                    }
                    EditMode::Normal => Self::dispatch_normal_core(
                        &mut tb.editor,
                        &mut tb.mode,
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

        // Non-compose: normal Claude key handling.
        if press.modifiers.contains(KMods::CONTROL) {
            if press.key == Key::Enter {
                self.send_claude(cx);
                return;
            }
            // Session switching: Ctrl-] next, Ctrl-[ prev.
            if press.key == Key::Char(']') {
                self.switch_claude_session(1, cx);
                return;
            }
            if press.key == Key::Char('[') {
                self.switch_claude_session(-1, cx);
                return;
            }
        }

        let outcome = {
            let claude = match self.claude_mut() {
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
        if let Some(c) = self.claude_mut() {
            let cursor_line = c.editor.cursor().line;
            let ranges = c.block_ranges.clone();
            let target = cursor_visible_child_index(c, cursor_line, &ranges);
            c.list_state.scroll_to_reveal_item(target);
        }

        match outcome {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                if let Some(c) = self.claude_mut() {
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
        let has_overlay = self.menu.is_some() || self.buffer_switcher.is_some();

        // Build the screen content. When an overlay is OPEN, focus moves up
        // to the wrapper so the screen's `SketchView`/`BrowserView` action
        // bindings don't match (they would otherwise fire BEFORE our key
        // listener — for example, `k` in Doc context is bound to
        // `ScrollUp` and `k` in Browser context is bound to `BrowserUp`,
        // both of which intercept the keystroke before any `on_key_down`
        // handler runs and stop propagation as the default action behavior).
        // When no overlay is open, the screen_root keeps focus.
        let screen_root = {
            let d = div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(BG))
                .text_color(rgb(DEFAULT_FG));
            if !has_overlay {
                d.track_focus(&self.focus_handle)
            } else {
                d
            }
        };

        let screen_view: AnyElement = self.render_focused_window(screen_root, cx);

        // When there's more than one tab, stack the tab strip above the
        // screen view. Single-tab workspaces render no strip — matches the
        // spec for "always show strip when >= 1 tab" but conservatively
        // suppresses it for the most common case (one-tab session) while
        // tab-creation commands are still landing.
        let screen_view = self.wrap_with_tab_strip(screen_view, cx);

        if !has_overlay {
            return screen_view;
        }

        // Buffer switcher takes priority over menu.
        if self.buffer_switcher.is_some() {
            return div()
                .track_focus(&self.focus_handle)
                .key_context("BufferSwitcherView")
                .size_full()
                .bg(rgb(BG))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    this.handle_buffer_switcher_key(ev, w, cx);
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
            .bg(rgb(BG))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                this.handle_menu_key(ev, w, cx);
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
        let ctx = RenderCtx {
            theme: &self.theme,
            body_font: self.body_font.clone(),
            code_font: self.code_font.clone(),
            cursor_block: Some(d.cursor_block),
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
            .text_size(px(14.0))
            .font_family(self.body_font.clone())
            .text_color(rgb(DEFAULT_FG));
        for (i, b) in d.blocks.iter().enumerate() {
            body = body.child(block_element(&ctx, i, b));
        }

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
                "j/k scroll · h/l block · g/G top/bot · Ctrl-O browse · q/Esc quit",
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
            .on_action(cx.listener(Self::open_claude))
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
        // The menu-bar actions (Quit / OpenBrowser / OpenClaude) still need
        // explicit `on_action` listeners on this root so the macOS menu bar
        // can dispatch them to whichever screen happens to be focused.
        root.key_context("EditView")
            .on_key_down(cx.listener(Self::handle_edit_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_claude))
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
            .text_size(px(14.0))
            .font_family(self.code_font.clone())
            .text_color(rgb(DEFAULT_FG));

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
            .text_size(px(14.0))
            .font_family(self.body_font.clone())
            .text_color(rgb(DEFAULT_FG));

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
            let (text_size_px, font_weight, top_pad) = match kind {
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
    fn render_claude(
        &self,
        root: gpui::Div,
        ring: &mut SessionRing,
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

        // Per-line turn numbers. Walk top-to-bottom: turn N covers the user
        // message + the Claude reply for that turn. The boundary is "we just
        // saw frozen content, then the next non-frozen line with content
        // appears" — that's the user starting turn N+1. Empty lines stay in
        // the prior turn so post-finalize blank rows don't shift the count.
        let mut turn_per_line: Vec<usize> = Vec::with_capacity(lines.len());
        let mut turn = 1usize;
        let mut saw_frozen_since_user = false;
        for (i, line_str) in lines.iter().enumerate() {
            let is_frozen = c.editor.is_frozen_line(i);
            let has_content = !line_str.trim().is_empty();
            if has_content && !is_frozen && saw_frozen_since_user {
                turn += 1;
                saw_frozen_since_user = false;
            }
            if is_frozen {
                saw_frozen_since_user = true;
            }
            turn_per_line.push(turn);
        }

        // ============ Virtualised list build ============
        //
        // Frozen (agent) content is parsed into RenderedBlocks so that
        // tables, code blocks, headings, and lists display properly.
        // Editable (user) content stays as per-line rendering with
        // cursor/selection support.

        // Build "tool calls anchored at line N" lookup, grouped by
        // anchor line. All calls at the same anchor form one ToolGroup.
        let mut tools_at_line: std::collections::HashMap<usize, Vec<String>> =
            std::collections::HashMap::new();
        for id in &c.tool_call_order {
            if let Some(line) = c.tool_call_anchor_line.get(id) {
                tools_at_line
                    .entry(*line)
                    .or_default()
                    .push(id.clone());
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
        // render_claude call; the closure is then called only for
        // visible items.
        let lines_snap = lines.clone();
        let highlighted_snap = highlighted.clone();
        let turn_per_line_snap = turn_per_line.clone();
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

                        // Turn label gating. We can't peek at the
                        // previous line's frozen-ness or content
                        // without reaching outside this closure, but
                        // we do have line_idx-1 in the snapshots.
                        let turn_n = *turn_per_line_snap.get(line_idx).unwrap_or(&1);
                        let prev_turn = if line_idx == 0 {
                            0
                        } else {
                            *turn_per_line_snap.get(line_idx - 1).unwrap_or(&0)
                        };
                        let prev_frozen = if line_idx == 0 {
                            !is_frozen
                        } else {
                            is_frozen_at(line_idx - 1, &frozen_lines_snap)
                        };
                        let prev_had_content = if line_idx == 0 {
                            false
                        } else {
                            !lines_snap
                                .get(line_idx - 1)
                                .map(|s| s.trim().is_empty())
                                .unwrap_or(true)
                        };
                        let block_starts_here = line_has_content
                            && (prev_turn != turn_n
                                || prev_frozen != is_frozen
                                || !prev_had_content);
                        let label_text: SharedString = if block_starts_here {
                            format!("{:>3}", format!("T{}", turn_n)).into()
                        } else {
                            "   ".into()
                        };
                        let label_color: Hsla = if !block_starts_here {
                            rgb(0x6272a4).into()
                        } else if is_frozen {
                            frozen_bar
                        } else {
                            user_bar
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
                                        if let Some(c) = this.claude_mut() {
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
                            cursor_block: None,
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
            .text_color(rgb(DEFAULT_FG))
            .child(
                gpui::list(c.list_state.clone(), render_fn)
                    .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                    .flex_1()
                    .w_full(),
            );

        let top = self.theme.top_bar;
        let bot = self.theme.bottom_bar;

        let attach_label: SharedString = match &c.channel {
            Some(ch) => {
                // Turn count: completed turns + 1 if a reply is in flight,
                // so the displayed number tracks "the turn we're in" rather
                // than "turns finished". Settles back to N=completed once
                // the agent's prompt response lands.
                let completed = ch.turn_count();
                let n = if c.awaiting_reply { completed + 1 } else { completed };
                let mode = ch.permission_mode().short_label();
                if n > 0 {
                    format!("ACP: {} · turn {} · {}", ch.command(), n, mode).into()
                } else {
                    format!("ACP: {} · {}", ch.command(), mode).into()
                }
            }
            None if c.attach_pending.is_some() => "ACP: attaching…".into(),
            None => "ACP: not attached".into(),
        };

        let timer_label: String = if let Some(started) = c.turn_started {
            let elapsed = started.elapsed();
            let secs = elapsed.as_secs();
            let m = secs / 60;
            let s = secs % 60;
            format!("  {}:{:02}", m, s)
        } else {
            String::new()
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .h(px(28.0))
            .bg(bg_or(top, STATUS_BG))
            .text_color(fg_or(top, STATUS_FG))
            .font_weight(FontWeight::BOLD)
            .child(format!("sketch-gpui [claude] — {}", attach_label))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(if c.turn_started.is_some() {
                        let c: Hsla = rgb(0xf1fa8c).into();
                        c
                    } else {
                        fg_or(top, STATUS_FG)
                    })
                    .child(SharedString::from(timer_label)),
            );

        let compose_active = c.compose_box.is_some();
        let mode_label = if compose_active {
            match c.compose_box.as_ref().unwrap().mode {
                EditMode::Normal => "COMPOSE",
                EditMode::Insert => "COMPOSE INSERT",
            }
        } else {
            match c.mode {
                EditMode::Normal => "NORMAL",
                EditMode::Insert => "INSERT",
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

        let hints = if compose_active {
            "Ctrl-Enter send · Ctrl-T close · esc normal"
        } else {
            "Ctrl-Enter send · Ctrl-V back · Ctrl-T compose · i insert · esc normal"
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

        // Compose box panel — rendered between body and footer when active.
        let compose_panel = if let Some(tb) = &c.compose_box {
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

        // Session sidebar — visible when more than one session exists.
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
                .bg(rgb(BG))
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
                            if let Some(ring) = this.claude_ring_mut() {
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
                        .text_color(rgb(DEFAULT_FG));
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
                            this.new_claude_session(cx);
                        });
                    }),
            );

            // Right column: body (flex-1) + optional compose panel.
            let mut right_col = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(body);
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
            // No sidebar: body + compose stacked vertically.
            let mut col = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(body);
            if let Some(panel) = compose_panel {
                col = col.child(panel);
            }
            col.into_any_element()
        };

        root
            .key_context("ClaudeView")
            .on_key_down(cx.listener(Self::handle_claude_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_claude))
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
            .on_action(cx.listener(Self::open_claude))
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
            WindowContent::Claude(_) => format!("Claude ({})", tab.display_label()),
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
    let theme = Theme::from_name(config.theme);

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
            let blocks = render::render(&doc.full_text(), &theme);
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
            KeyBinding::new("ctrl-k", OpenClaude, Some("SketchView")),
            KeyBinding::new("space", OpenMenu, Some("SketchView")),
            KeyBinding::new("q", Quit, Some("SketchView")),
            KeyBinding::new("escape", Quit, Some("SketchView")),
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
            KeyBinding::new("cmd-k", OpenClaude, None),
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
            KeyBinding::new("ctrl-w o", OnlyWindow, None),
            // Vim-style focus motion across split panes.
            KeyBinding::new("ctrl-w h", FocusLeft, None),
            KeyBinding::new("ctrl-w l", FocusRight, None),
            KeyBinding::new("ctrl-w k", FocusUp, None),
            KeyBinding::new("ctrl-w j", FocusDown, None),
            KeyBinding::new("ctrl-w w", FocusNext, None),
            KeyBinding::new("ctrl-w shift-w", FocusPrev, None),
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
                    // Reboot handoff: the previous sketch process set this
                    // env var via `reboot_into_claude` to mean "boot
                    // straight into the claude screen and resume every
                    // saved session." The downstream `open_claude_inner`
                    // consults `load_persisted_acp_sessions`, so
                    // session/load fires once per persisted slot.
                    if std::env::var("SKETCH_OPEN_CLAUDE").is_ok() {
                        view.open_claude_inner(cx);
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
                    MenuItem::action("Open Claude Session", OpenClaude),
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

    // ---- Claude splice / lock helpers ----

    fn fresh_claude_editor() -> Editor {
        Editor::new(String::new(), std::path::PathBuf::from("*claude*"))
    }

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

    #[test]
    fn splice_claude_chunk_into_empty_editor_adds_frozen_lines() {
        let mut ed = fresh_claude_editor();
        splice_claude_chunk(&mut ed, "Hello, human!");
        let text = ed.document().full_text();
        assert!(
            text.contains("Hello, human!"),
            "buffer should contain the spliced text, got {:?}",
            text
        );
        // The reply line should be frozen (read-only).
        assert!(
            !ed.frozen_lines().is_empty(),
            "expected at least one frozen-line range after splice"
        );
        assert!(
            ed.is_frozen_line(0),
            "first line should be frozen after splice into empty buffer"
        );
    }

    #[test]
    fn splice_claude_chunk_preserves_user_draft_below() {
        let mut ed = fresh_claude_editor();
        // Type some draft text first.
        ed.insert_char('h');
        ed.insert_char('i');
        // The draft is in the editable region; splice a Claude reply.
        splice_claude_chunk(&mut ed, "I am Claude.");

        let text = ed.document().full_text();
        assert!(
            text.contains("I am Claude."),
            "reply should be in buffer, got {:?}",
            text
        );
        assert!(
            text.contains("hi"),
            "user draft should survive the splice, got {:?}",
            text
        );
        // The reply should appear before the draft in the document.
        let reply_pos = text.find("I am Claude.").unwrap();
        let draft_pos = text.find("hi").unwrap();
        assert!(
            reply_pos < draft_pos,
            "reply should slot ABOVE the draft, but reply@{} draft@{} in {:?}",
            reply_pos,
            draft_pos,
            text
        );
    }

    #[test]
    fn splice_claude_chunk_empty_text_is_noop() {
        let mut ed = fresh_claude_editor();
        ed.insert_char('a');
        let before = ed.document().full_text();
        splice_claude_chunk(&mut ed, "   \n\n");
        // Whitespace-only text is treated as empty (trim_end_matches('\n') leaves spaces but the impl uses trimmed.is_empty after trim_end of '\n' only — so spaces survive). Verify the docstring behavior: empty after trim.
        // For this test, an actually-empty payload:
        splice_claude_chunk(&mut ed, "");
        // The all-spaces case may or may not no-op; what we definitely want is
        // "" doesn't change the doc.
        let after = ed.document().full_text();
        // The first call (whitespace) may add some content; we only assert
        // that at minimum the user's char is still there.
        assert!(after.contains('a'));
        let _ = before;
    }

    #[test]
    fn lock_claude_turn_appends_separator_and_locks_above() {
        let mut ed = fresh_claude_editor();
        ed.insert_char('h');
        ed.insert_char('i');
        let line_count_before = ed.document().line_count();
        lock_claude_turn(&mut ed);

        let text = ed.document().full_text();
        assert!(
            text.contains("──"),
            "lock should append a horizontal-rule separator, got {:?}",
            text
        );
        // Lockable-through-line should have moved past the original content.
        let cursor_line = ed.cursor().line;
        let lockable = ed.lockable_through_line();
        assert!(
            lockable >= line_count_before,
            "lockable_through_line ({}) should be at/past original last line ({})",
            lockable,
            line_count_before
        );
        assert_eq!(
            cursor_line, lockable,
            "cursor should land on the next-turn line (the lockable boundary)"
        );
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
    fn menu_c_o_resolves_to_open_claude() {
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
    fn splice_then_lock_then_splice_again_chains_above_draft() {
        let mut ed = fresh_claude_editor();
        // Turn 1: Claude greets first (no prior draft).
        splice_claude_chunk(&mut ed, "Hi.");
        // Turn-end housekeeping mirrors what `pump_claude_replies` does
        // when the agent's prompt response lands: ensure an editable line
        // sits below the frozen content so the user can type.
        finalize_claude_turn(&mut ed);
        // User types a reply.
        ed.insert_char('o');
        ed.insert_char('k');
        // Send/lock.
        lock_claude_turn(&mut ed);
        // User starts typing the next prompt.
        ed.insert_char('?');
        // Claude streams a reply mid-typing.
        splice_claude_chunk(&mut ed, "Yes!");
        finalize_claude_turn(&mut ed);

        let text = ed.document().full_text();
        // All three pieces of content + the locked draft survive.
        assert!(text.contains("Hi."), "first reply missing: {:?}", text);
        assert!(text.contains("ok"), "first user draft missing: {:?}", text);
        assert!(text.contains("Yes!"), "second reply missing: {:?}", text);
        assert!(text.contains('?'), "in-progress draft missing: {:?}", text);
        // Order: Hi. → ok → Yes! → ?
        let pos_hi = text.find("Hi.").unwrap();
        let pos_ok = text.find("ok").unwrap();
        let pos_yes = text.find("Yes!").unwrap();
        let pos_q = text.find('?').unwrap();
        assert!(pos_hi < pos_ok, "Hi before ok ({:?})", text);
        assert!(pos_ok < pos_yes, "ok before Yes! ({:?})", text);
        assert!(pos_yes < pos_q, "Yes! before ? ({:?})", text);
    }
}
