//! Shared element builders for rendered markdown + raw lines: color/font
//! helpers, styled-line/Block/table/list elements, wiki-link expansion,
//! doc selection mapping, and the WP line classifier. Extracted verbatim
//! from main.rs (split-gpui-main); free functions only — no view state.

use super::*;

/// Dracula-derived background pulled from the neutral theme; GPUI doesn't
/// have a `Reset` color, so we pick concrete defaults.
pub(crate) const BG: u32 = 0x282a36;
pub(crate) const DEFAULT_FG: u32 = 0xf8f8f2;
pub(crate) const CURSOR_BAR_COLOR: u32 = 0xff3030;
pub(crate) const STATUS_BG: u32 = 0x16213e;
pub(crate) const STATUS_FG: u32 = 0x8be9fd;
/// Selection background (matches TUI's `view::apply_selection_bg`). Dracula's
/// "current line" gray reads as a contiguous swath against the editor bg
/// without overpowering syntax-highlighted spans.
pub(crate) const SELECTION_BG: NColor = NColor::Rgb(68, 71, 90);
/// Background tint for the focused line in source-file doc view. Slightly
/// lighter than the editor bg so the cursor line stands out without clashing
/// with syntax highlighting. Dracula "current line" = 0x44475a.
pub(crate) const CURSOR_LINE_BG: u32 = 0x44475a;

/// Multiplicative step per Cmd+= / Cmd+- press. 1.1 is the same ratio
/// Chromium uses for browser zoom — small enough that hitting the key twice
/// is meaningful, large enough to feel responsive.
pub(crate) const TEXT_SCALE_STEP: f32 = 1.1;
pub(crate) const MIN_TEXT_SCALE: f32 = 0.5;
pub(crate) const MAX_TEXT_SCALE: f32 = 3.0;

/// UXI-ParagraphSpacing-1. Extra vertical space inserted BETWEEN stacked blocks /
/// paragraphs / list items on the reading surfaces (Doc view, agent transcript,
/// WP edit) so paragraphs read as distinct chunks — the "break up paragraphs"
/// readability gap. It is added on top of each surface's existing base gap and
/// multiplied by `text_scale` (see `paragraph_gap`) so it scales with zoom.
/// Within-paragraph wrapped lines and all chrome are unaffected. Tune here.
pub(crate) const PARAGRAPH_GAP_PX: f32 = 6.0;

/// The readability paragraph gap in pixels at the given document zoom.
pub(crate) fn paragraph_gap(text_scale: f32) -> gpui::Pixels {
    px(PARAGRAPH_GAP_PX * text_scale)
}

/// Convert a `NColor` to `Hsla`, using a hardcoded white fallback for
/// `Reset` / `Indexed` variants. Suitable for agent theme colors which
/// are always `Color::Rgb` and never need a real fallback.
pub(crate) fn nc(c: NColor) -> Hsla {
    ncolor_to_hsla(c, DEFAULT_FG)
}

pub(crate) fn ncolor_to_hsla(c: NColor, fallback: u32) -> Hsla {
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
pub(crate) fn ncolor_to_u32(c: NColor, fallback: u32) -> u32 {
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

pub(crate) fn fg_or(s: NStyle, fallback: u32) -> Hsla {
    match s.fg {
        Some(c) => ncolor_to_hsla(c, fallback),
        None => rgb(fallback).into(),
    }
}

pub(crate) fn bg_or(s: NStyle, fallback: u32) -> Hsla {
    match s.bg {
        Some(c) => ncolor_to_hsla(c, fallback),
        None => rgb(fallback).into(),
    }
}

/// Tint a background color by blending in a hue at `saturation` and
/// shifting lightness by `lightness_delta`. Used to derive subtle per-turn
/// card backgrounds from the theme's editor_bg.
pub(crate) fn tint_bg(base: Hsla, hue: f32, saturation: f32, lightness_delta: f32) -> Hsla {
    Hsla {
        h: hue,
        s: saturation,
        l: (base.l + lightness_delta).clamp(0.0, 1.0),
        a: base.a,
    }
}

pub(crate) fn font_for(s: NStyle, family: &SharedString) -> Font {
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

/// Decide whether a span renders in the monospace CODE font. Inline code is
/// detected by its distinctive bg (the `code_inline` proxy) or the Dracula code
/// fg. The **selection-highlight bg is explicitly excluded** — any span can
/// carry it, and treating it as code would flip selected proportional prose to
/// monospace (the "highlight becomes monospaced" bug).
pub(crate) fn span_uses_code_font(
    bg: Option<NColor>,
    fg: Option<NColor>,
    selection_bg: Option<NColor>,
) -> bool {
    let is_code_bg = bg.is_some() && bg != selection_bg;
    is_code_bg || fg == Some(NColor::Rgb(241, 250, 140))
}

/// Semantic wrapper around the legacy color proxies. Selection styling can
/// replace both colors, so it marks spans that were code with `MONOSPACE`
/// before applying the universal selection palette.
pub(crate) fn style_uses_code_font(style: NStyle, selection_bg: Option<NColor>) -> bool {
    style.modifier.contains(Modifier::MONOSPACE)
        || span_uses_code_font(style.bg, style.fg, selection_bg)
}

pub(crate) fn styled_line_element(
    line: &StyledLine,
    base_style: NStyle,
    base_fg: u32,
    body_font: &SharedString,
    code_font: &SharedString,
    // The active selection-highlight background, if any. A selected span carries
    // this as its `bg`, which must NOT be mistaken for the inline-code bg proxy
    // below (else selecting proportional prose flips it to monospace). `None`
    // for callers that never paint a selection into the span style.
    selection_bg: Option<NColor>,
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

            let font = if style_uses_code_font(combined, selection_bg) {
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
pub(crate) fn doc_styled_line_element(
    ctx: &RenderCtx<'_>,
    line: &StyledLine,
    base_style: NStyle,
    base_fg: u32,
    body_font: &SharedString,
    code_font: &SharedString,
    line_idx: usize,
) -> AnyElement {
    // Two selection-capable paths share this builder:
    //   - doc view: `current_block` + `line_layouts` set → paints selection via
    //     `doc_selection` and registers a `TextLayout` for `doc_pos_at`.
    //   - transcript code block (bug-0017): `block_hits` set → paints selection
    //     via the raw-line `selection` and registers a REAL-bounds `TokenHit`.
    // With neither, fall back to the plain, selection-less element.
    let doc_ctx: Option<(usize, std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>)> =
        match (ctx.current_block, ctx.line_layouts.as_ref()) {
            (Some(b), Some(s)) => Some((b, s.clone())),
            _ => None,
        };
    let bh = ctx.block_hits.as_ref();
    if doc_ctx.is_none() && bh.is_none() {
        return styled_line_element(line, base_style, base_fg, body_font, code_font, None);
    }

    // Build text + runs, identical to `styled_line_element`. We need the
    // intermediate form so we can patch background_color before sealing the
    // StyledText.
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    // Byte range + raw target for every link on this line (wiki OR external
    // URL — bug-0018). Populated as we walk spans below; used to wrap the
    // StyledText in InteractiveText with on_click handlers that route through
    // `classify_link`.
    let mut link_ranges: Vec<(std::ops::Range<usize>, String)> = Vec::new();

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
            // Collect EVERY linked span (wiki note OR external URL), keeping the
            // raw link string; `classify_link` at click time decides the action
            // (bug-0018). Previously only `wiki:`-prefixed links were collected,
            // so URL links got no click handler and were inert.
            if let Some(link) = span.link.as_deref() {
                link_ranges.push((span_start..span_start + len, link.to_string()));
            }
            let font = if combined.bg.is_some() || combined.fg == Some(NColor::Rgb(241, 250, 140)) {
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
    if let Some((block_idx, _)) = &doc_ctx {
        if let Some(sel) = ctx.doc_selection {
            let line_chars = styled_line_char_count(line);
            if let Some((s_char, e_char)) =
                doc_selection_for_line(&sel, *block_idx, line_idx, line_chars)
            {
                let s_byte = char_offset_to_byte_offset(&text, s_char);
                let e_byte = char_offset_to_byte_offset(&text, e_char);
                #[cfg(test)]
                DOC_RENDER_TAP.with(|t| {
                    t.borrow_mut()
                        .selection
                        .push((*block_idx, line_idx, s_byte, e_byte))
                });
                runs = apply_selection_bg_to_runs(
                    runs,
                    s_byte,
                    e_byte,
                    ncolor_to_hsla(SELECTION_BG, BG),
                );
            }
        }
    } else if let Some(bh) = bh {
        // Transcript code block (bug-0017): selection is keyed by RAW document
        // line so the highlight lands on the glyphs, independent of the block's
        // padding / `[lang]` header. `raw_base` already skips the opening fence.
        let raw_line = bh.raw_base + line_idx;
        let line_chars = styled_line_char_count(line);
        if let Some((s_char, e_char)) = bh
            .selection
            .and_then(|sel| line_selection_range(sel, raw_line, line_chars))
            .filter(|(s, e)| e > s)
        {
            let s_byte = char_offset_to_byte_offset(&text, s_char);
            let e_byte = char_offset_to_byte_offset(&text, e_char);
            #[cfg(test)]
            DOC_RENDER_TAP
                .with(|t| t.borrow_mut().block_selection.push((raw_line, s_char, e_char)));
            runs = apply_selection_bg_to_runs(runs, s_byte, e_byte, bh.sel_bg);
        }
    }

    let styled = StyledText::new(text).with_runs(runs);

    // Transcript code block: register a hit token from this line's OWN painted
    // bounds (not an even split of the padded/headered outer block) so a click
    // maps to the right raw line. Code content has no wiki links, so return here.
    if let Some(bh) = bh {
        let raw_line = bh.raw_base + line_idx;
        let line_chars = styled_line_char_count(line);
        return register_token_on_paint(
            styled.into_any_element(),
            bh.sink.clone(),
            raw_line,
            0,
            line_chars,
        );
    }

    let (block_idx, sink) =
        doc_ctx.expect("doc_ctx is Some when block_hits is None (guarded above)");
    // Capture the line's TextLayout handle. It is registered into the hit-test
    // sink at PAINT time (via RegisterOnPaint), NOT here at build time: the
    // virtualized `gpui::list` builds/measures lines it never prepaints, and
    // `doc_pos_at` calling `.bounds()` on an un-prepainted layout panics across
    // the platform input callback. Registering on paint guarantees every sink
    // entry has bounds set.
    let layout = styled.layout().clone();
    let key = (block_idx, line_idx);

    // Plain text path — no links on this line.
    if link_ranges.is_empty() {
        return register_line_on_paint(styled.into_any_element(), sink, key, layout);
    }

    // Wrap in InteractiveText so we can attach an on_click handler. A click
    // routes through `classify_link`: an external URL opens in the default
    // browser (`open_external_link`), while a note/local Markdown link opens
    // the file in a new buffer tile (`open_wiki_link`). View reached via the
    // weak handle captured in RenderCtx.
    let weak = match &ctx.weak_view {
        Some(w) => w.clone(),
        None => return styled.into_any_element(),
    };
    let doc_dir = ctx.doc_dir.clone();
    let ranges: Vec<std::ops::Range<usize>> =
        link_ranges.iter().map(|(r, _)| r.clone()).collect();
    let targets: Vec<String> = link_ranges.into_iter().map(|(_, t)| t).collect();
    let element_id = gpui::ElementId::Name(SharedString::from(format!(
        "link-line-{block_idx}-{line_idx}"
    )));
    let el = gpui::InteractiveText::new(element_id, styled)
        .on_click(ranges, move |idx, _w, app| {
            let Some(target) = targets.get(idx) else {
                return;
            };
            let target = target.clone();
            let doc_dir = doc_dir.clone();
            let _ = weak.update(app, |view, cx| {
                view.open_link_target(&target, doc_dir.as_deref(), cx);
            });
        })
        .into_any_element();
    #[cfg(test)]
    let el = probe_bounds_dyn(format!("doc-link-{block_idx}-{line_idx}"), el);
    register_line_on_paint(el, sink, key, layout)
}

/// Convert a char-index into its byte offset within `s`. Saturates at
/// `s.len()` if `char_offset` is past the end (the selection projection
/// already clamps to `line_char_count`, but `text` here may include the
/// defensive trailing space we add for empty lines).
pub(crate) fn char_offset_to_byte_offset(s: &str, char_offset: usize) -> usize {
    for (chars_seen, (byte_idx, _)) in s.char_indices().enumerate() {
        if chars_seen == char_offset {
            return byte_idx;
        }
    }
    s.len()
}

/// Split runs at `[s_byte, e_byte)` and patch the in-range runs'
/// `background_color`. Runs are sequential and their `len` sums to the
/// total text byte length; we walk byte by byte (run-major) and split
/// at each boundary.
pub(crate) fn apply_selection_bg_to_runs(
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

pub(crate) struct RenderCtx<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) body_font: SharedString,
    pub(crate) code_font: SharedString,
    /// Cmd-zoom multiplier on body text. 1.0 = unzoomed. Set to 1.0 in
    /// contexts where zoom shouldn't apply (e.g. the Claude session block
    /// renderer).
    pub(crate) text_scale: f32,
    pub(crate) cursor_block: Option<usize>,
    /// Active doc-view mouse selection, used to paint background on
    /// participating lines. `None` outside the view-mode render path.
    pub(crate) doc_selection: Option<DocSelection>,
    /// Side channel for line-layout registration. Lines store their cloned
    /// `TextLayout` here keyed by `(block_idx, line_idx)` so mouse handlers
    /// on the doc body can hit-test against bounds and map pixels → bytes.
    /// `None` outside the view-mode render path (e.g. edit-mode rendering
    /// and nested ctxes inside blockquotes/lists where v1 doesn't yet
    /// support selection).
    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    pub(crate) line_layouts: Option<std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>>,
    /// The top-level block index currently being rendered. Set by
    /// `block_element` and cleared (set to `None`) when `block_inner`
    /// recurses into nested blocks (blockquote/list content), so the v1
    /// "top-level only" selection scope is enforced naturally.
    pub(crate) current_block: Option<usize>,
    /// Weak handle on the view, captured so click handlers built inside
    /// free render functions (`doc_styled_line_element`, etc.) can call
    /// back into the view for wiki-link navigation. `None` outside the
    /// view-mode render path.
    pub(crate) weak_view: Option<gpui::WeakEntity<YaldaGpuiView>>,
    /// Directory of the currently focused Doc, used to resolve wiki link
    /// targets (`[[notes]]` → `<doc_dir>/notes.md`). `None` outside the
    /// view-mode render path or when the doc has no parent dir.
    pub(crate) doc_dir: Option<PathBuf>,
    /// Total top-level block count of the doc being rendered. For source
    /// files (one block per line) this equals the file's line count, which
    /// fixes the line-number gutter width so it doesn't jump at digit
    /// boundaries while scrolling. `0` where unknown (agent chat, nested
    /// contexts) — the gutter falls back to per-block math.
    pub(crate) block_count: usize,
    /// When true, headings render with their literal markdown markers (`## `,
    /// `### `) prepended to the styled text. Used only by the agent transcript
    /// (toggle: agent menu "heading markers"); the doc/edit views always pass
    /// `false`. Nested ctxes propagate it so headings inside blockquotes/lists
    /// stay consistent.
    pub(crate) show_heading_markers: bool,
    /// bug-0017: transcript-only. When set, the lines of a parsed code Block
    /// self-register REAL painted bounds into the token-hit sink AND paint the
    /// selection background — the two things a `FlatItem::Block` otherwise never
    /// did (the even-split outer band was offset by the block's padding/`[lang]`
    /// header + the fence-inclusive raw range, and NOTHING painted a highlight,
    /// so code-block selection was invisible and grabbed the wrong lines). The
    /// doc/edit views leave this `None` (they use `doc_selection`/`line_layouts`).
    pub(crate) block_hits: Option<BlockHits>,
    /// Read handle on the root view's rendered-diagram cache (`UXI-Diagram-1`).
    /// The `RenderedBlock::Diagram` arm looks up `hash(source + theme + width)`
    /// and paints the ready image, or the raw-source placeholder/fallback.
    /// Read-only here; populated at the two top-level render sites (doc view +
    /// transcript), `None` in nested/edit contexts (they inherit the parent's).
    pub(crate) diagrams: Option<std::rc::Rc<RefCell<DiagramCache>>>,
}

/// bug-0017: side channel that makes a transcript code Block's content lines
/// selectable *and visible*. Content line `li` maps to raw document line
/// `raw_base + li` (skipping the opening ``` fence), so the hit band and the
/// selection highlight land on the real glyphs regardless of the block's
/// padding / language header.
#[derive(Clone)]
pub(crate) struct BlockHits {
    /// The transcript token-hit sink; each content line pushes one `TokenHit`
    /// with its REAL painted bounds (not an even split of the outer block).
    pub(crate) sink: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    /// Raw document line of this block's first rendered content line.
    pub(crate) raw_base: usize,
    /// Active transcript selection in raw `(line, col)` doc coords (focused only).
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    /// Selection background color.
    pub(crate) sel_bg: Hsla,
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
/// Wraps an element and records its painted bounds `(x, y, w, h)` into a
/// shared cell. The desktop canvas uses it so mouse listeners (which receive
/// WINDOW coordinates) can convert into desktop coordinates, and so the
/// render pass knows the real viewport for culling / pan clamping / the
/// drop-time effective width. Same idiom as [`RegisterOnPaint`].
pub(crate) struct CaptureBounds {
    pub(crate) inner: AnyElement,
    pub(crate) sink: std::rc::Rc<std::cell::Cell<(f32, f32, f32, f32)>>,
}

impl IntoElement for CaptureBounds {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for CaptureBounds {
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        self.sink.set((
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        ));
        self.inner.paint(window, cx);
    }
}

// ─────────────────── Transcript token hit-testing (select-to-clipboard) ──────
//
// The agent transcript renders each doc line as a `flex_wrap` row of MANY
// tokenized `styled_line_element` children (monospace word-wrap), NOT one
// hittable `StyledText` — so the doc view's per-line `TextLayout` hit-test
// (`line_layouts` / `doc_pos_at`) doesn't apply. Instead each token registers
// its PAINTED bounds plus the (doc line, starting char) it covers, and mouse
// hit-testing maps a window point → token → char via the token's monospace
// width (`width / char_count`). Registration is at PAINT time (bounds set,
// virtualized rows never painted are absent) — same idiom as `RegisterOnPaint`.

/// One painted transcript token: the doc line + char range it covers and where
/// it landed on screen. `bounds` are WINDOW-space (same space mouse events use).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TokenHit {
    pub(crate) line_idx: usize,
    pub(crate) start_char: usize,
    pub(crate) char_count: usize,
    pub(crate) bounds: Bounds<Pixels>,
}

/// Map a window point to the nearest painted transcript `(line, char_offset)`.
/// Picks the token minimizing (vertical distance, horizontal distance) so a
/// click in a gap / past a line end / above-or-below all content snaps to the
/// sensible edge; the column within the chosen token is derived from its
/// monospace width. Returns `None` only when no tokens were painted.
pub(crate) fn hit_test_tokens(
    pt: gpui::Point<Pixels>,
    tokens: &[TokenHit],
) -> Option<(usize, usize)> {
    let px_dist = |lo: Pixels, hi: Pixels, v: Pixels| -> f32 {
        if v < lo {
            f32::from(lo - v)
        } else if v > hi {
            f32::from(v - hi)
        } else {
            0.0
        }
    };
    let mut best: Option<(f32, f32, &TokenHit)> = None;
    for t in tokens {
        let b = t.bounds;
        let vd = px_dist(b.top(), b.bottom(), pt.y);
        let hd = px_dist(b.left(), b.right(), pt.x);
        let better = match best {
            None => true,
            Some((bvd, bhd, _)) => (vd, hd) < (bvd, bhd),
        };
        if better {
            best = Some((vd, hd, t));
        }
    }
    let (_, _, t) = best?;
    let b = t.bounds;
    let col = if t.char_count == 0 {
        t.start_char
    } else {
        let w = f32::from(b.size.width);
        let frac = if w > 0.0 {
            ((f32::from(pt.x - b.left())) / w * t.char_count as f32).round()
        } else {
            0.0
        };
        let within = (frac.max(0.0) as usize).min(t.char_count);
        t.start_char + within
    };
    Some((t.line_idx, col))
}

/// Wrap a transcript token element so its painted bounds + covered char range
/// register into `sink` at paint time (for `hit_test_tokens`). No-op-ish in
/// production beyond one push per painted token.
pub(crate) fn register_token_on_paint(
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    line_idx: usize,
    start_char: usize,
    char_count: usize,
) -> AnyElement {
    RegisterTokenOnPaint {
        inner,
        sink,
        line_idx,
        start_char,
        char_count,
    }
    .into_any_element()
}

struct RegisterTokenOnPaint {
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    line_idx: usize,
    start_char: usize,
    char_count: usize,
}

impl IntoElement for RegisterTokenOnPaint {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RegisterTokenOnPaint {
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        self.sink.borrow_mut().push(TokenHit {
            line_idx: self.line_idx,
            start_char: self.start_char,
            char_count: self.char_count,
            bounds,
        });
        self.inner.paint(window, cx);
    }
}

/// Is a raw markdown table line the SEPARATOR row (`| --- | :--: |`)? — its content
/// is only pipes, dashes, colons and spaces. The separator has no rendered row, so it
/// must be skipped when aligning hit bands to painted rows.
pub(crate) fn is_table_separator_line(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty()
        && t.contains('-')
        && t.chars().all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

/// Char ranges `(start_char, count)` of each cell's TRIMMED content in a raw markdown
/// table row: `| ab | c |` → `[(2, 2), (7, 1)]`. Cells are the spans between pipes.
pub(crate) fn parse_table_cell_ranges(raw: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = raw.chars().collect();
    let pipes: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '|')
        .map(|(i, _)| i)
        .collect();
    let mut cells = Vec::new();
    for w in pipes.windows(2) {
        let (mut s, b) = (w[0] + 1, w[1]);
        while s < b && chars[s] == ' ' {
            s += 1;
        }
        let mut e = b;
        while e > s && chars[e - 1] == ' ' {
            e -= 1;
        }
        cells.push((s, e - s));
    }
    cells
}

/// Wrap a rendered BLOCK (`FlatItem::Block` — table / bullet list / code / heading)
/// so it registers hit-test bands into `sink` at paint time. Parsed blocks otherwise
/// register NO `TokenHit`s at all, so the mouse has nothing to grab and their content
/// is unselectable (bug-0008). `rows` is one entry per PAINTED row, each
/// `(raw_line, cells)` where `cells` are `(start_char, count)` spans within that raw
/// line. The block's painted height is split into `rows.len()` equal horizontal
/// bands; each band's width is split into `cells.len()` equal columns (table cells
/// render `flex_1` = equal width, so this lands on the real cells), and each column
/// registers a hit mapping to its cell's raw `(line, start_char, count)`. Non-table
/// blocks pass one full-width cell per line.
pub(crate) fn register_block_hits_on_paint(
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    rows: Vec<(usize, Vec<(usize, usize)>)>,
    selection: Option<((usize, usize), (usize, usize))>,
    sel_bg: Hsla,
) -> AnyElement {
    RegisterBlockHitsOnPaint {
        inner,
        sink,
        rows,
        selection,
        sel_bg,
    }
    .into_any_element()
}

struct RegisterBlockHitsOnPaint {
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    rows: Vec<(usize, Vec<(usize, usize)>)>,
    /// bug-0030: active transcript selection in raw `(line, col)` doc coords
    /// (focused only) so this even-split band path can PAINT the highlight — not
    /// just register hits. `None` when nothing is selected / transcript unfocused.
    selection: Option<((usize, usize), (usize, usize))>,
    /// Selection background color for the painted quad.
    sel_bg: Hsla,
}

impl IntoElement for RegisterBlockHitsOnPaint {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RegisterBlockHitsOnPaint {
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        let nrows = self.rows.len().max(1);
        let h = f32::from(bounds.size.height);
        let full_w = f32::from(bounds.size.width);
        let x0 = f32::from(bounds.origin.x);
        let y0 = f32::from(bounds.origin.y);
        let band_h = h / nrows as f32;
        {
            let mut sink = self.sink.borrow_mut();
            for (ri, (raw_line, cells)) in self.rows.iter().enumerate() {
                let top = y0 + band_h * ri as f32;
                let ncols = cells.len().max(1);
                let col_w = full_w / ncols as f32;
                for (ci, (start_char, count)) in cells.iter().enumerate() {
                    let left = x0 + col_w * ci as f32;
                    sink.push(TokenHit {
                        line_idx: *raw_line,
                        start_char: *start_char,
                        char_count: *count,
                        bounds: Bounds {
                            origin: gpui::point(px(left), px(top)),
                            size: gpui::size(px(col_w), px(band_h)),
                        },
                    });
                    // bug-0030: paint the selection highlight over the selected
                    // sub-span of THIS cell using the SAME uniform monospace-width
                    // model `hit_test_tokens` uses (`x = left + within/count * col_w`),
                    // so the highlight lands exactly where a click/drag selects.
                    // Without this a bullet list / table cell registered hits (copy
                    // worked) but painted NO highlight — the user saw nothing.
                    if let Some(((sl, sc), (el, ec))) = self.selection
                        && *count > 0
                        && *raw_line >= sl
                        && *raw_line <= el
                    {
                        let sel_s = if *raw_line == sl { sc } else { 0 };
                        let sel_e = if *raw_line == el { ec } else { usize::MAX };
                        let cs = sel_s.max(*start_char);
                        let ce = sel_e.min(start_char + count);
                        if ce > cs {
                            let x_s = left + (cs - start_char) as f32 / *count as f32 * col_w;
                            let x_e = left + (ce - start_char) as f32 / *count as f32 * col_w;
                            window.paint_quad(gpui::fill(
                                Bounds {
                                    origin: gpui::point(px(x_s), px(top)),
                                    size: gpui::size(px(x_e - x_s), px(band_h)),
                                },
                                self.sel_bg,
                            ));
                            #[cfg(test)]
                            DOC_RENDER_TAP.with(|t| {
                                t.borrow_mut().band_selection.push((*raw_line, cs, ce))
                            });
                        }
                    }
                }
            }
        }
        self.inner.paint(window, cx);
    }
}

// ───────────────────────── Layout probe (headless harness #3.2) ─────────────
//
// The verification-harness gap that let the caret-visibility class keep
// regressing: `run_until_parked` runs a real layout/paint pass, but tests only
// asserted STATE — never what was painted. This probe records the PAINTED bounds
// of tagged elements so a `#[gpui::test]` can prove geometry for real (e.g. "the
// compose cursor row is inside the compose box"). Same paint-time capture idiom
// as `CaptureBounds`, but keyed by a static label a test reads back — so it can
// reach a free render fn (the caret lives deep in `build_chatbox_line`) without
// threading a sink down every call. Inactive (`None`) in production: a branch +
// early return, no allocation.

thread_local! {
    static LAYOUT_PROBE: RefCell<Option<HashMap<&'static str, (f32, f32, f32, f32)>>> =
        const { RefCell::new(None) };
    // Dynamic-label sibling: per-tile probe tags (`plane-card-{id}`,
    // `plane-tile-content-{id}`) can't be `&'static str`, so they record here
    // under an owned `String`. Shares the same begin/end lifecycle as
    // `LAYOUT_PROBE`; `layout_probe_get` checks BOTH maps.
    static LAYOUT_PROBE_DYN: RefCell<Option<HashMap<String, (f32, f32, f32, f32)>>> =
        const { RefCell::new(None) };
}

/// Start recording painted bounds for [`probe_bounds`]-tagged elements. Call in a
/// test before `run_until_parked`. Until called, the probe is a no-op.
#[cfg(test)]
pub(crate) fn layout_probe_begin() {
    LAYOUT_PROBE.with(|p| *p.borrow_mut() = Some(HashMap::new()));
    LAYOUT_PROBE_DYN.with(|p| *p.borrow_mut() = Some(HashMap::new()));
}

/// The last painted bounds `(x, y, w, h)` of the element tagged `label`, or
/// `None` if it was never painted this pass — itself a signal (a virtualized row
/// scrolled BELOW the fold is never painted, so a missing `compose-cursor-row`
/// means the caret is off-screen).
#[cfg(test)]
pub(crate) fn layout_probe_get(label: &str) -> Option<(f32, f32, f32, f32)> {
    LAYOUT_PROBE
        .with(|p| p.borrow().as_ref().and_then(|m| m.get(label).copied()))
        .or_else(|| {
            LAYOUT_PROBE_DYN.with(|p| p.borrow().as_ref().and_then(|m| m.get(label).copied()))
        })
}

/// Stop recording and clear (test teardown).
#[cfg(test)]
pub(crate) fn layout_probe_end() {
    LAYOUT_PROBE.with(|p| *p.borrow_mut() = None);
    LAYOUT_PROBE_DYN.with(|p| *p.borrow_mut() = None);
}

fn layout_probe_record(label: &'static str, b: (f32, f32, f32, f32)) {
    LAYOUT_PROBE.with(|p| {
        if let Some(m) = p.borrow_mut().as_mut() {
            m.insert(label, b);
        }
    });
}

fn layout_probe_record_dyn(label: &str, b: (f32, f32, f32, f32)) {
    LAYOUT_PROBE_DYN.with(|p| {
        if let Some(m) = p.borrow_mut().as_mut() {
            m.insert(label.to_string(), b);
        }
    });
}

/// Wrap `inner` so its PAINTED bounds are recorded under `label` when the layout
/// probe is active (no-op otherwise). The headless geometry-assertion primitive.
pub(crate) fn probe_bounds(label: &'static str, inner: AnyElement) -> AnyElement {
    ProbeBounds { label, inner }.into_any_element()
}

/// Dynamic-label sibling of [`probe_bounds`] for per-instance tags whose label
/// isn't `&'static` (e.g. `plane-card-{id}`). Recorded into `LAYOUT_PROBE_DYN`;
/// no-op unless the probe is active (production never activates it). The
/// `String` is retained by the element and only allocates once per frame per
/// tagged tile — negligible next to building the tile itself.
pub(crate) fn probe_bounds_dyn(label: String, inner: AnyElement) -> AnyElement {
    ProbeBoundsDyn { label, inner }.into_any_element()
}

struct ProbeBoundsDyn {
    label: String,
    inner: AnyElement,
}

impl IntoElement for ProbeBoundsDyn {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ProbeBoundsDyn {
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        layout_probe_record_dyn(
            &self.label,
            (
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ),
        );
        self.inner.paint(window, cx);
    }
}

struct ProbeBounds {
    label: &'static str,
    inner: AnyElement,
}

impl IntoElement for ProbeBounds {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ProbeBounds {
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        layout_probe_record(
            self.label,
            (
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ),
        );
        self.inner.paint(window, cx);
    }
}

pub(crate) struct RegisterOnPaint {
    pub(crate) inner: AnyElement,
    pub(crate) sink: std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>,
    pub(crate) key: (usize, usize),
    pub(crate) layout: TextLayout,
}

pub(crate) fn register_line_on_paint(
    inner: AnyElement,
    sink: std::rc::Rc<RefCell<HashMap<(usize, usize), TextLayout>>>,
    key: (usize, usize),
    layout: TextLayout,
) -> AnyElement {
    RegisterOnPaint {
        inner,
        sink,
        key,
        layout,
    }
    .into_any_element()
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
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
        cx: &mut GpuiApp,
    ) {
        // prepaint has run → the layout's bounds are set. Registering here means
        // `doc_pos_at` only ever sees prepainted (bounds-Some) layouts.
        self.sink.borrow_mut().insert(self.key, self.layout.clone());
        #[cfg(test)]
        DOC_RENDER_TAP.with(|t| t.borrow_mut().painted.push(self.key));
        self.inner.paint(window, cx);
    }
}

pub(crate) fn block_element(ctx: &RenderCtx<'_>, idx: usize, block: &RenderedBlock) -> AnyElement {
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
        block_count: ctx.block_count,
        show_heading_markers: ctx.show_heading_markers,
        // Nested/doc-view blocks don't use the transcript code-block hit path.
        block_hits: None,
        diagrams: ctx.diagrams.clone(),
    };
    let base = block_inner(&inner_ctx, block);

    // Source-file lines are one block each — no inter-block margin, or the
    // file gets an 8px gap between every line of code.
    let is_source_line = matches!(
        block,
        RenderedBlock::CodeBlock {
            source_file: true,
            ..
        }
    );

    // Wrap with a left "cursor bar" indicator when this is the focused block.
    let mut row = div().flex().flex_row().items_start().w_full();
    if !is_source_line {
        // UXI-ParagraphSpacing-1: readability gap below each block so paragraphs
        // read as distinct chunks, scaled with zoom. PADDING, not margin: the doc
        // stacks blocks in a `gpui::list`, which IGNORES item margins (the old
        // `mb_2` produced no visible gap) — padding is part of the box the list
        // measures and stacks by. Source-file lines are exempt (one block/line).
        row = row.pb(px(8.0 * ctx.text_scale) + paragraph_gap(ctx.text_scale));
    }
    // Source-file lines: add a full-row background tint so the focused line
    // is unmistakable (the 3px bar alone is too subtle in a wall of code).
    if is_source_line && highlighted {
        row = row.bg(rgb(CURSOR_LINE_BG));
    }
    let bar = div().w(px(3.0)).flex_none().h_full().bg(if highlighted {
        rgb(CURSOR_BAR_COLOR)
    } else {
        rgba(0x00000000)
    });
    // Content column. Its painted bounds EXCLUDE the row's bottom margin, so a test
    // can recover the applied paragraph gap as (row-slot height − content height).
    let content_col = div()
        .pl_3()
        .flex_1()
        .min_w_0()
        .child(base)
        .into_any_element();
    #[cfg(test)]
    let content_col = probe_bounds_dyn(format!("doc-block-inner-{idx}"), content_col);
    row.child(bar).child(content_col).into_any_element()
}

/// Prepend the literal markdown heading markers (`## ` for h2, `### ` for h3,
/// …) to a heading's already-parsed styled line. pulldown strips the markers
/// during parse, so this re-inserts them as a leading span carrying the heading
/// style. Pure; the agent transcript calls it when its heading-marker toggle is
/// on (the doc/edit views never do). Level is clamped to 1..=6.
pub(crate) fn heading_line_with_markers(
    level: u8,
    content: &StyledLine,
    style: yalda::style::Style,
) -> StyledLine {
    let marker = format!("{} ", "#".repeat((level as usize).clamp(1, 6)));
    let mut spans = Vec::with_capacity(content.spans.len() + 1);
    spans.push(StyledSpan::new(marker, style));
    spans.extend(content.spans.iter().cloned());
    StyledLine::new(spans)
}

pub(crate) fn block_inner(ctx: &RenderCtx<'_>, block: &RenderedBlock) -> AnyElement {
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
            // Optionally prepend the literal markdown markers (`## `, `### `)
            // ahead of the rendered heading text — pulldown strips them during
            // parse, so we re-insert a span carrying the same heading style.
            // Agent-transcript-only (doc/edit pass `show_heading_markers: false`).
            let with_markers;
            let content: &StyledLine = if ctx.show_heading_markers {
                with_markers = heading_line_with_markers(*level, content, style);
                &with_markers
            } else {
                content
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
            let mut col = div().flex().flex_col().text_color(fg_or(base, DEFAULT_FG));
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
        RenderedBlock::CodeBlock {
            language,
            lines,
            source_file,
            start_line,
        } => {
            // Un-highlighted code (no language / unknown language) has no
            // per-span fg, so the fallback must come from the theme — a
            // hardcoded white is unreadable on light themes' code-block tint.
            let code_fg = ncolor_to_u32(ctx.theme.editor_fg, DEFAULT_FG);
            let mut col = div()
                .flex()
                .flex_col()
                .font_family(ctx.code_font.clone())
                .text_color(rgb(code_fg));
            if *source_file {
                // Source file: no container chrome — code IS the document.
            } else {
                // Fenced code block inside markdown: tinted background + padding.
                let bg = ctx.theme.code_block_bg;
                col = col.p_2().rounded_md().bg(bg_or(bg, BG));
            }
            if !*source_file
                && let Some(lang) = language
                && !lang.is_empty()
            {
                col = col.child(
                    div()
                        .text_color(rgb(0x6272a4))
                        .text_size(px(11.0))
                        .pb_1()
                        .child(format!("[{}]", lang)),
                );
            }
            let row_style = NStyle::default();
            if *source_file {
                // Source file: line-number gutter, same as the edit view's
                // Code gutter. Width derives from the doc's total block
                // count (== file line count under the one-block-per-line
                // split), so it stays stable across the whole file instead
                // of jumping at digit boundaries.
                let digits = ctx
                    .block_count
                    .max(start_line + lines.len())
                    .max(1)
                    .to_string()
                    .len();
                let num_fg = fg_or(ctx.theme.line_number, 0x6272a4);
                for (li, line) in lines.iter().enumerate() {
                    let num = format!("{:>width$}", start_line + li + 1, width = digits);
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .child(div().flex_none().pr_3().text_color(num_fg).child(num))
                            .child(doc_styled_line_element(
                                ctx,
                                line,
                                row_style,
                                code_fg,
                                &ctx.code_font,
                                &ctx.code_font,
                                li,
                            )),
                    );
                }
            } else {
                for (li, line) in lines.iter().enumerate() {
                    col = col.child(doc_styled_line_element(
                        ctx,
                        line,
                        row_style,
                        code_fg,
                        &ctx.code_font,
                        &ctx.code_font,
                        li,
                    ));
                }
            }
            col.into_any_element()
        }
        RenderedBlock::Diagram { source, lines } => {
            // UXI-Diagram-1. Paint the rendered mermaid PNG when it is ready;
            // otherwise paint the raw highlighted source — as a placeholder
            // while the async render runs, or as the fallback (plus a note)
            // when `mmdc` is absent or errored. READ-ONLY: this path never
            // requests a render or notifies (that happens in the reconcile
            // pass; see `dispatch-notify` / `request_diagram`). The rendered
            // image opts out of the per-line code hit-testing that
            // `CodeBlock` participates in — there is no text to select in a
            // bitmap; the raw source stays selectable in the Editing view.
            let ready = ctx.diagrams.as_ref().and_then(|cache| {
                let key = diagram_key(source, MermaidTheme::from_theme_name(ctx.theme.name));
                match cache.borrow().get(key) {
                    Some(DiagramRender::Ready(image, width)) => Some(Ok((image.clone(), *width))),
                    Some(DiagramRender::Failed(msg)) => Some(Err(msg.clone())),
                    // Pending or not-yet-requested: placeholder.
                    _ => None,
                }
            });
            match ready {
                // Explicit display WIDTH (height stays `Auto` → gpui derives it
                // from the image aspect); `max_w_full` clamps on a narrow pane.
                // Without a set width gpui paints the img at its intrinsic PIXEL
                // size (2× the render scale) — enormous. See UXI-Diagram-1.
                Some(Ok((image, width))) => div()
                    .child(
                        img(image)
                            .w(px(width))
                            .max_w_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element(),
                other => {
                    let note = match other {
                        Some(Err(msg)) => Some(msg),
                        _ => None,
                    };
                    diagram_fallback_column(ctx, lines, note)
                }
            }
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
                        block_count: 0,
                        show_heading_markers: ctx.show_heading_markers,
                        // Nested blocks don't use the transcript code-block hit path.
                        block_hits: None,
                        diagrams: ctx.diagrams.clone(),
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
            // UXI-ParagraphSpacing-1: air between bullet / ordered items (scaled
            // with zoom) so a list reads item-by-item. Within an item, wrapped
            // lines stay tight (one `build_wrapped_line` per line). Shared by the
            // Doc view and the transcript (both reach this via `block_inner`).
            let mut col = div()
                .flex()
                .flex_col()
                .gap(paragraph_gap(ctx.text_scale));
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
        // Frontmatter (bug-0014): metadata ABOUT the document, so it reads
        // de-emphasized — dimmed, monospace, one row per source line, behind a
        // left rule — and never at heading scale. Before the metadata-block
        // extension this whole thing rendered as a giant setext `<h2>`.
        RenderedBlock::Metadata { lines } => {
            let s = ctx.theme.paragraph;
            let mut col = div()
                .flex()
                .flex_col()
                .pl_2()
                .my_1()
                .border_l_2()
                .border_color(rgb(0x6272a4))
                .text_color(fg_or(s, 0x6272a4))
                .opacity(0.7)
                .font_family(ctx.code_font.clone())
                .text_size(px(12.0 * ctx.text_scale));
            for line in lines.iter() {
                col = col.child(line.text_content());
            }
            col.into_any_element()
        }
    }
}

pub(crate) fn list_item_element(
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
                block_count: 0,
                show_heading_markers: ctx.show_heading_markers,
                // Nested blocks don't use the transcript code-block hit path.
                block_hits: None,
                diagrams: ctx.diagrams.clone(),
            },
            b,
        ));
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .w_full()
        .gap_2()
        .child(
            div()
                .min_w(px(24.0))
                .flex_none()
                .text_color(fg_or(marker_style, 0x50fa7b))
                .font_weight(FontWeight::BOLD)
                .child(marker),
        )
        .child(content_col)
        .into_any_element()
}

pub(crate) fn table_element(
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
            None,
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
                None,
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
pub(crate) fn segments_to_styled_line(segs: &[Segment]) -> StyledLine {
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
pub(crate) fn split_segments_at_col(
    segs: &[Segment],
    col: usize,
) -> (Vec<Segment>, (char, NStyle), Vec<Segment>) {
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
pub(crate) struct DocSelection {
    pub(crate) anchor: DocPos,
    pub(crate) head: DocPos,
    pub(crate) dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DocPos {
    pub(crate) block_idx: usize,
    pub(crate) line_idx: usize,
    pub(crate) char_offset: usize,
}

impl DocSelection {
    pub(crate) fn normalized(&self) -> (DocPos, DocPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Char count of a StyledLine (sum of `chars().count()` over all spans).
pub(crate) fn styled_line_char_count(line: &StyledLine) -> usize {
    line.spans.iter().map(|s| s.text.chars().count()).sum()
}

/// Prefix on `StyledSpan.link` that marks a wiki-style link target
/// (`[[note]]` in markdown). The doc-view click handler treats spans with
/// this prefix as file references; regular markdown links keep their raw
/// `dest_url` and are classified by [`classify_link`].
pub(crate) const WIKI_LINK_PREFIX: &str = "wiki:";

/// What a clicked link should do (bug-0018). Every linked span in a rendered
/// doc is routed through here so URL links open the browser and note links open
/// the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// An internal note / local-file reference — resolved relative to the doc
    /// dir and opened in a new in-app buffer tile by `open_wiki_link`. Covers
    /// `wiki:`-prefixed `[[note]]` links AND scheme-less / relative markdown
    /// links (`./a.md`).
    Wiki(String),
    /// An external URL — opened in the OS default handler by
    /// `open_external_link`. Restricted to `http`/`https`/`mailto` so `open`
    /// never launches an arbitrary local handler.
    External(String),
}

/// Classify a raw link string (a `StyledSpan.link`) into the action it should
/// trigger (bug-0018). `wiki:`-prefixed → `Wiki` (prefix stripped);
/// `http://` / `https://` / `mailto:` (case-insensitive) → `External`;
/// everything else (a relative or scheme-less link) → `Wiki` as a local-file
/// reference. Pure so the routing decision — the actual fix — is unit-testable.
pub(crate) fn classify_link(raw: &str) -> LinkTarget {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix(WIKI_LINK_PREFIX) {
        return LinkTarget::Wiki(rest.to_string());
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
    {
        return LinkTarget::External(t.to_string());
    }
    LinkTarget::Wiki(t.to_string())
}

/// Turn a local Markdown destination into a filesystem spelling:
/// - `file:///tmp/a.md` → `/tmp/a.md`
/// - `%20` and other percent escapes are decoded
/// - `#heading` / `?query` and Codex-style `:line[:column]` suffixes are
///   removed because they are navigation metadata, not part of the path.
pub(crate) fn normalize_local_link_target(raw: &str) -> String {
    let mut target = raw.trim();
    if target.starts_with('<') && target.ends_with('>') && target.len() >= 2 {
        target = &target[1..target.len() - 1];
    }
    if let Some(path) = target.strip_prefix("file://") {
        target = path.strip_prefix("localhost").unwrap_or(path);
    }
    if let Some(end) = target.find(['#', '?']) {
        target = &target[..end];
    }

    let mut decoded = percent_decode_path(target);
    if !std::path::Path::new(&decoded).is_file() {
        decoded = strip_source_position_suffix(&decoded).to_string();
    }
    decoded
}

fn percent_decode_path(raw: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(&hi), Some(&lo)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(hi), Some(lo)) = (hex(hi), hex(lo))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_source_position_suffix(path: &str) -> &str {
    let Some((before_last, last)) = path.rsplit_once(':') else {
        return path;
    };
    if !last.chars().all(|ch| ch.is_ascii_digit()) {
        return path;
    }
    if let Some((before_line, line)) = before_last.rsplit_once(':')
        && line.chars().all(|ch| ch.is_ascii_digit())
    {
        return before_line;
    }
    before_last
}

/// Map a file extension to a syntect language token. Returns `None` for
/// markdown and unknown extensions — those are rendered as prose.
pub(crate) fn lang_for_path(path: &std::path::Path) -> Option<&'static str> {
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

/// Render pre-parsed markdown `blocks` into a read-only column element, reusing
/// the doc block renderer via `block_inner` (no cursor / selection / wiki-nav
/// plumbing — a self-contained read-only surface). For embedded markdown: tool
/// output, subagent reports, prompts. Prose in the body font, fenced code in the
/// code font, headings/lists/tables/blockquotes styled by the theme. `max_blocks`
/// caps huge payloads (a dim "+N more blocks" footer is appended when clipped);
/// `None` renders all. Per-block vertical gap follows `ParagraphSpacing`
/// (`paragraph_gap`). Blocks are pre-parsed so the caller can cache the parse.
/// (UXI-AgentTile-25.)
pub(crate) fn render_markdown_column(
    blocks: &[RenderedBlock],
    max_blocks: Option<usize>,
    theme: &Theme,
    body_font: &SharedString,
    code_font: &SharedString,
    text_scale: f32,
) -> AnyElement {
    let ctx = RenderCtx {
        theme,
        body_font: body_font.clone(),
        code_font: code_font.clone(),
        text_scale,
        cursor_block: None,
        doc_selection: None,
        line_layouts: None,
        current_block: None,
        weak_view: None,
        doc_dir: None,
        block_count: blocks.len(),
        show_heading_markers: false,
        block_hits: None,
        diagrams: None,
    };
    let cap = max_blocks.unwrap_or(blocks.len());
    let gap = paragraph_gap(text_scale);
    let mut col = div().flex().flex_col().w_full().min_w_0();
    for (i, b) in blocks.iter().enumerate().take(cap) {
        // Each block gets a DEFINITE full-pane width — the same `w_full` +
        // `flex_1().min_w_0()` content-column shape `block_element` gives blocks
        // in the doc view. Without it a bullet-list item's inner `flex_1` column
        // (which carries `min_w_0`, so its min-content floor is 0) has no width
        // to distribute against and collapses to ~0, so a long unbroken file
        // path wraps ONE GLYPH PER LINE (UXI-AgentTile-26). Paragraphs happened
        // to survive on shrink-to-fit; lists did not.
        let inner = div()
            .flex_1()
            .min_w_0()
            .child(block_inner(&ctx, b))
            .into_any_element();
        #[cfg(test)]
        let inner = probe_bounds_dyn(format!("md-block-{i}"), inner);
        #[cfg(not(test))]
        let _ = i;
        col = col.child(div().flex().flex_row().w_full().pt(gap).child(inner));
    }
    if blocks.len() > cap {
        col = col.child(
            div()
                .pt(gap)
                .text_size(px(11.0 * text_scale))
                .text_color(nc(theme.agent.dim))
                .child(SharedString::from(format!("… +{} more blocks", blocks.len() - cap))),
        );
    }
    col.into_any_element()
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
pub(crate) fn render_with_wiki(
    text: &str,
    theme: &Theme,
    path: Option<&std::path::Path>,
) -> Vec<RenderedBlock> {
    if let Some(lang) = path.and_then(lang_for_path) {
        let hl = yalda::highlight::Highlighter::with_syntect_theme(theme.name.syntect_theme());
        // Use a transparent base style — source files render against the
        // normal document background, not the code-block tint.
        let base = yalda::style::Style::default();
        let mut lines = hl.highlight(lang, text, base).unwrap_or_else(|| {
            // Fallback: plain text with default style.
            text.lines()
                .map(|l| StyledLine::new(vec![StyledSpan::new(l, theme.paragraph)]))
                .collect()
        });
        if lines.is_empty() {
            // Empty file: keep one block so cursor/reveal logic has a target.
            lines.push(StyledLine::new(vec![]));
        }
        // One block PER LINE: the doc view scrolls and focuses by block
        // (j/k move `cursor_block`), and `gpui::list` virtualizes by item —
        // a whole file as one giant block can neither scroll nor virtualize.
        // Highlighting ran over the full text above, so cross-line state
        // (block comments, raw strings) is already correct in the split.
        return lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| RenderedBlock::CodeBlock {
                language: Some(lang.to_string()),
                lines: vec![line],
                source_file: true,
                start_line: i,
            })
            .collect();
    }
    let mut blocks = render::render(text, theme);
    expand_wiki_links_in_blocks(&mut blocks, theme);
    blocks
}

pub(crate) fn expand_wiki_links_in_blocks(blocks: &mut [RenderedBlock], theme: &Theme) {
    for b in blocks.iter_mut() {
        expand_wiki_links_in_block(b, theme);
    }
}

pub(crate) fn expand_wiki_links_in_block(block: &mut RenderedBlock, theme: &Theme) {
    match block {
        RenderedBlock::Heading { content, .. } => expand_wiki_links_in_line(content, theme),
        RenderedBlock::Paragraph { lines } => {
            for line in lines.iter_mut() {
                expand_wiki_links_in_line(line, theme);
            }
        }
        // Code blocks: deliberately untouched. `[[foo]]` inside a fenced
        // block is code text, not a link. A mermaid Diagram is the same — its
        // source is code, not prose (UXI-Diagram-1).
        RenderedBlock::CodeBlock { .. } | RenderedBlock::Diagram { .. } => {}
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
        // Frontmatter is metadata, not prose: no wiki-link expansion (bug-0014).
        RenderedBlock::HorizontalRule
        | RenderedBlock::Image { .. }
        | RenderedBlock::Metadata { .. } => {}
    }
}

/// Paint a mermaid `Diagram` block's raw highlighted source (`UXI-Diagram-1`):
/// the placeholder while the async render runs, and the fallback when `mmdc` is
/// absent or errors. Mirrors the fenced-`CodeBlock` column (tinted background +
/// per-line highlight), tagged `[mermaid]`, with an optional trailing note line
/// carrying the render error. Never blank.
fn diagram_fallback_column(
    ctx: &RenderCtx<'_>,
    lines: &[StyledLine],
    note: Option<String>,
) -> AnyElement {
    let code_fg = ncolor_to_u32(ctx.theme.editor_fg, DEFAULT_FG);
    let bg = ctx.theme.code_block_bg;
    let mut col = div()
        .flex()
        .flex_col()
        .font_family(ctx.code_font.clone())
        .text_color(rgb(code_fg))
        .p_2()
        .rounded_md()
        .bg(bg_or(bg, BG))
        .child(
            div()
                .text_color(rgb(0x6272a4))
                .text_size(px(11.0))
                .pb_1()
                .child("[mermaid]"),
        );
    let row_style = NStyle::default();
    for (li, line) in lines.iter().enumerate() {
        col = col.child(doc_styled_line_element(
            ctx,
            line,
            row_style,
            code_fg,
            &ctx.code_font,
            &ctx.code_font,
            li,
        ));
    }
    if let Some(msg) = note {
        col = col.child(
            div()
                .text_color(rgb(0x6272a4))
                .text_size(px(11.0))
                .pt_1()
                .child(format!("mermaid: {msg}")),
        );
    }
    col.into_any_element()
}

/// Does this block contain any link (hyperlink span or expanded wiki link)?
/// Used by the Doc local menu's `navigate → next link` (spec-menu-scopes.md).
pub(crate) fn block_contains_link(block: &RenderedBlock) -> bool {
    fn line_has_link(line: &StyledLine) -> bool {
        line.spans.iter().any(|s| s.link.is_some())
    }
    match block {
        RenderedBlock::Heading { content, .. } => line_has_link(content),
        RenderedBlock::Paragraph { lines } | RenderedBlock::CodeBlock { lines, .. } => {
            lines.iter().any(line_has_link)
        }
        RenderedBlock::BlockQuote { blocks } => blocks.iter().any(block_contains_link),
        RenderedBlock::List { items, .. } => items
            .iter()
            .any(|item| item.content.iter().any(block_contains_link)),
        RenderedBlock::Table { headers, rows, .. } => {
            headers.iter().any(line_has_link)
                || rows.iter().any(|row| row.iter().any(line_has_link))
        }
        RenderedBlock::HorizontalRule => false,
        // Frontmatter carries no expanded links (bug-0014).
        RenderedBlock::Metadata { .. } => false,
        // A mermaid Diagram has no links: its source is code, and the rendered
        // image opts out of hit-testing (UXI-Diagram-1).
        RenderedBlock::Diagram { .. } => false,
        // An image is itself a navigable target (it carries a URL).
        RenderedBlock::Image { .. } => true,
    }
}

pub(crate) fn expand_wiki_links_in_line(line: &mut StyledLine, theme: &Theme) {
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

pub(crate) fn split_wiki_links(span: &StyledSpan, theme: &Theme) -> Vec<StyledSpan> {
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
pub(crate) fn re_render_layout_docs(layout: &mut workspace::Layout<App>, theme: &Theme) {
    match layout {
        workspace::Layout::Empty => {}
        workspace::Layout::Leaf(win) => {
            if let App::Buffer(BufferApp::Viewing(d)) = &mut win.content {
                re_render_one_doc(d, theme);
            }
            // The picker's underlying-stashed BufferApp is also restyled if it
            // happens to be a Viewing Doc — otherwise reverting via Esc lands on
            // stale-themed blocks.
            if let App::Buffer(BufferApp::Picking(b)) = &mut win.content
                && let Some(BufferApp::Viewing(d)) = b.underlying.as_deref_mut()
            {
                re_render_one_doc(d, theme);
            }
        }
        workspace::Layout::Split { children, .. } => {
            for (_, child) in children.iter_mut() {
                re_render_layout_docs(child, theme);
            }
        }
    }
}

/// Re-render one Doc's blocks under a new theme. For a pool-bound Doc (5c) the
/// authority is the *live shared core*, not the file on disk — reading disk
/// here would silently revert unsaved edits made through a sibling Edit view
/// (and, because `rendered_seq` wouldn't advance, the per-frame `refresh_blocks`
/// would not correct it). So source from the live core when present, stamping
/// `rendered_seq` so the live path stays coherent. Only string-backed Docs
/// (`source == None`: help/welcome/legacy) fall back to a disk read.
pub(crate) fn re_render_one_doc(d: &mut DocState, theme: &Theme) {
    let path = PathBuf::from(d.file_label.as_ref());
    match d.source.as_ref() {
        Some(src) => {
            let seq = src.edit_seq();
            let text = src.full_text();
            d.set_blocks(render_with_wiki(&text, theme, Some(&path)));
            if let Some(src) = d.source.as_mut() {
                src.rendered_seq = seq;
            }
        }
        None => {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            d.set_blocks(render_with_wiki(&text, theme, Some(&path)));
        }
    }
}

/// Return the `StyledLine`s of a block that v1 view-mode selection treats
/// as selectable, in the same order line_idx is assigned during render.
/// Blocks not covered (BlockQuote, List, Table, HorizontalRule, Image)
/// produce an empty slice — they remain unselectable for now.
pub(crate) fn block_selectable_lines(block: &RenderedBlock) -> &[StyledLine] {
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
pub(crate) fn doc_selection_for_line(
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
    if s < e { Some((s, e)) } else { None }
}

/// Project a document-level selection range onto a single line. Returns
/// `[start_col, end_col)` clamped to the line's character count, or `None`
/// if the line is fully outside the selection. Mirrors view.rs's projection
/// (lines fully inside multi-line selections get `(0, line_char_count)`;
/// the first/last lines get the partial range).
pub(crate) fn line_selection_range(
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
pub(crate) enum WpLineKind {
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
pub(crate) fn classify_wp_line(text: &str, in_fence: bool) -> WpLineKind {
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
    if let Some(c) = chars.next()
        && matches!(c, '-' | '*' | '+')
        && chars.next() == Some(' ')
    {
        return WpLineKind::BulletItem;
    }

    // Ordered list: digits + (`.` | `)`) + space.
    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count > 0 {
        let after = &trimmed[digit_count..];
        let mut after_chars = after.chars();
        if matches!(after_chars.next(), Some('.') | Some(')')) && after_chars.next() == Some(' ') {
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

/// Apply the visual-selection background to one line's segments, handling the
/// blank / whitespace-only case the raw [`apply_selection_bg`] can't: the
/// syntax highlighter yields no segments for a whitespace-only line, and a
/// blank line that sits *inside* a multi-line selection projects to a
/// zero-width range — so `apply_selection_bg` would paint nothing and the line
/// reads as an un-highlighted gap. Here we emit an explicit highlighted
/// placeholder whenever the line is blank and either part of it or its trailing
/// newline (the selection continuing onto a later line) is selected. Returns
/// the segments unchanged when the line is outside the selection.
pub(crate) fn apply_line_selection(
    segs: &[Segment],
    line_str: &str,
    sel: ((usize, usize), (usize, usize)),
    line_idx: usize,
    base_style: NStyle,
    selection_bg: NColor,
) -> Vec<Segment> {
    let line_chars = line_str.chars().count();
    let Some((s, e_col)) = line_selection_range(sel, line_idx, line_chars) else {
        return segs.to_vec();
    };
    if line_str.trim().is_empty() {
        // Blank / whitespace-only line. Highlight it when any column is
        // selected (`e_col > s`) or when its newline is — i.e. the selection
        // continues past this line (`line_idx < end_line`). A blank line that
        // is merely the zero-width *end* of a selection stays un-highlighted,
        // matching vim.
        let newline_selected = line_idx < sel.1.0;
        if e_col > s || newline_selected {
            return vec![(" ".to_string(), base_style.bg(selection_bg))];
        }
        return segs.to_vec();
    }
    if e_col > s {
        apply_selection_bg(segs, s, e_col, selection_bg)
    } else {
        segs.to_vec()
    }
}

/// Walk segments char by char, applying `bg` to chars whose column falls in
/// `[start_col, end_col)`. Output may have more segments than input (a single
/// styled run can split across the selection boundary). Direct port of
/// `view::apply_selection_bg` so visual behavior matches the TUI.
pub(crate) fn apply_selection_bg(
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

/// Apply the transcript's universal selection palette to a character range.
/// Unlike [`apply_selection_bg`], this replaces syntax foregrounds too: a list
/// marker, heading marker, link, or emphasis span reads exactly like selected
/// prose. Inline code records its semantic monospace role before its identifying
/// colors are replaced, so selection changes color without changing typography.
pub(crate) fn apply_selection_style(
    segs: &[Segment],
    start_col: usize,
    end_col: usize,
    bg: NColor,
    fg: NColor,
) -> Vec<Segment> {
    let mut result: Vec<Segment> = Vec::new();
    let mut col = 0usize;
    for (text, style) in segs {
        let mut current_text = String::new();
        let mut current_style = *style;
        let mut started = false;
        for ch in text.chars() {
            let is_selected = col >= start_col && col < end_col;
            let new_style = if is_selected {
                let was_code = style_uses_code_font(*style, None);
                let mut selected = style.bg(bg).fg(fg);
                if was_code {
                    selected = selected.add_modifier(Modifier::MONOSPACE);
                }
                selected
            } else {
                *style
            };
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
