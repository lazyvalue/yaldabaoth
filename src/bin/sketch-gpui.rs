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
//!   Ctrl-O               open file browser
//!   q / Esc              quit
//!
//! File-browser view keys:
//!   j / Down / Ctrl-N    next entry
//!   k / Up / Ctrl-P      previous entry
//!   Enter / l            open entry (descend into dir, or open file)
//!   - / h                go to parent directory
//!   .                    toggle hidden files
//!   s                    cycle sort order (name / date↓ / date↑)
//!   q / Esc              close browser (returns to doc, or quits)

use std::path::PathBuf;
use std::process;

use gpui::{
    actions, div, point, prelude::*, px, rgb, rgba, size, AnyElement, App, AppContext,
    Application, Bounds, Context, FocusHandle, Focusable, Font, FontFeatures, FontStyle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
    ScrollHandle, SharedString, StrikethroughStyle, Styled, StyledText, TextRun,
    UnderlineStyle, Window, WindowBounds, WindowOptions,
};

use sketch::blocks::{ColumnAlignment, ListItem, RenderedBlock, StyledLine};
use sketch::document::Document;
use sketch::file_browser::{BrowserEntry, FileBrowser};
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
        Quit,
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
// Theme palette
// ----------------------------------------------------------------------------

/// Dracula-derived background pulled from the neutral theme; GPUI doesn't
/// have a `Reset` color, so we pick concrete defaults.
const BG: u32 = 0x282a36;
const DEFAULT_FG: u32 = 0xf8f8f2;
const CURSOR_BAR_COLOR: u32 = 0xff5555;
const STATUS_BG: u32 = 0x16213e;
const STATUS_FG: u32 = 0x8be9fd;

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
}

/// State held while the user is browsing the filesystem. `underlying` keeps
/// the document state we'll restore when the user closes the browser without
/// opening a new file (or `None` when the app started directly in the
/// browser, in which case close = quit).
struct BrowserScreen {
    fb: FileBrowser,
    underlying: Option<DocState>,
}

enum Screen {
    Doc(DocState),
    Browser(BrowserScreen),
}

struct SketchGpuiView {
    screen: Screen,
    theme: Theme,
    body_font: SharedString,
    code_font: SharedString,
    focus_handle: FocusHandle,
}

impl SketchGpuiView {
    fn new_doc(
        blocks: Vec<RenderedBlock>,
        theme: Theme,
        file_label: String,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            screen: Screen::Doc(DocState {
                blocks,
                file_label: file_label.into(),
                cursor_block: 0,
                scroll_handle: ScrollHandle::new(),
            }),
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("Menlo"),
            focus_handle,
        }
    }

    fn new_browser(start_dir: PathBuf, theme: Theme, focus_handle: FocusHandle) -> Self {
        Self {
            screen: Screen::Browser(BrowserScreen {
                fb: FileBrowser::new(start_dir),
                underlying: None,
            }),
            theme,
            body_font: SharedString::new_static(".SystemUIFont"),
            code_font: SharedString::new_static("Menlo"),
            focus_handle,
        }
    }

    /// `Some(doc)` if currently viewing a document, else `None`.
    fn doc_mut(&mut self) -> Option<&mut DocState> {
        match &mut self.screen {
            Screen::Doc(d) => Some(d),
            _ => None,
        }
    }

    fn browser_mut(&mut self) -> Option<&mut BrowserScreen> {
        match &mut self.screen {
            Screen::Browser(b) => Some(b),
            _ => None,
        }
    }

    /// Open `path` as a doc, replacing the current screen. Returns false if
    /// the file couldn't be read; on false, `screen` is unchanged.
    fn open_file(&mut self, path: PathBuf) -> bool {
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path.display(), e);
                return false;
            }
        };
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();
        let doc = Document::from_text(text, path.clone());
        let blocks = render::render(&doc.full_text(), &self.theme);
        self.screen = Screen::Doc(DocState {
            blocks,
            file_label: canon.into(),
            cursor_block: 0,
            scroll_handle: ScrollHandle::new(),
        });
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
        // Only meaningful from a doc screen — captures current state for
        // restore on close.
        let doc = match std::mem::replace(
            &mut self.screen,
            Screen::Browser(BrowserScreen {
                fb: FileBrowser::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
                underlying: None,
            }),
        ) {
            Screen::Doc(d) => Some(d),
            other => {
                // Already in browser; restore.
                self.screen = other;
                return;
            }
        };
        // Re-attach the captured doc to the browser so close returns to it.
        if let Screen::Browser(b) = &mut self.screen {
            b.underlying = doc;
        }
        cx.notify();
    }
    fn quit(&mut self, _: &Quit, _w: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
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
        // If the browser was opened from a doc, return to it. Otherwise quit.
        let underlying = match &mut self.screen {
            Screen::Browser(b) => b.underlying.take(),
            _ => return,
        };
        match underlying {
            Some(d) => {
                self.screen = Screen::Doc(d);
                cx.notify();
            }
            None => cx.quit(),
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
        let root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(DEFAULT_FG));

        match &self.screen {
            Screen::Doc(_) => self.render_doc(root, cx).into_any_element(),
            Screen::Browser(_) => self.render_browser(root, cx).into_any_element(),
        }
    }
}

impl SketchGpuiView {
    fn render_doc(
        &self,
        root: gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let d = match &self.screen {
            Screen::Doc(d) => d,
            _ => unreachable!(),
        };

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
            .on_action(cx.listener(Self::quit))
            .child(header)
            .child(body)
            .child(footer)
    }

    fn render_browser(
        &self,
        root: gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let b = match &self.screen {
            Screen::Browser(b) => b,
            _ => unreachable!(),
        };

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
            .on_action(cx.listener(Self::browser_close))
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
// Main
// ----------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let theme = Theme::dark();

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
            KeyBinding::new("q", Quit, Some("SketchView")),
            KeyBinding::new("escape", Quit, Some("SketchView")),
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
            KeyBinding::new("q", BrowserClose, Some("BrowserView")),
            KeyBinding::new("escape", BrowserClose, Some("BrowserView")),
        ]);

        let bounds = Bounds::new(point(px(120.0), px(80.0)), size(px(900.0), px(700.0)));
        app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                ..Default::default()
            },
            move |window, app| {
                app.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);
                    match initial_doc.clone() {
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
                    }
                })
            },
        )
        .unwrap();
    });
}
