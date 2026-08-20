//! Generic detail-panel render primitives — no domain coupling. A read-only
//! "detail view" (a Linear issue, a metadata panel, anything that's labels +
//! prose + sections) is the same handful of shapes: multi-line text, key/value
//! rows, section headings, author/date note blocks, and an ISO-date formatter.
//! These live here (not `render_blocks.rs`, which is already large) so any
//! surface can reuse them by passing a [`DetailStyle`].

use crate::*;

/// Resolved colors/fonts/size for a detail panel, snapshotted once per render.
pub(crate) struct DetailStyle {
    pub(crate) fg: Hsla,
    pub(crate) dim: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) err: Hsla,
    pub(crate) mono: SharedString,
    pub(crate) prose: SharedString,
    /// Body text size in px (already zoom-scaled).
    pub(crate) base: Pixels,
    /// Same value as a raw f32, for deriving heading / sub-text sizes.
    pub(crate) pt: f32,
}

/// Render possibly-multiline text, one child per line so `\n` becomes a real
/// break (a bare `SharedString` renders newlines as spaces). Empty → "—".
pub(crate) fn multiline_text(
    text: &str,
    color: Hsla,
    font: &SharedString,
    base: Pixels,
) -> gpui::Div {
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .text_color(color)
        .font_family(font.clone())
        .text_size(base);
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return col.child(SharedString::from("—"));
    }
    for line in trimmed.split('\n') {
        if line.trim().is_empty() {
            col = col.child(div().h(base));
        } else {
            col = col.child(div().w_full().child(SharedString::from(line.to_string())));
        }
    }
    col
}

/// A fixed-label / value row.
pub(crate) fn kv_row(label: &str, value: String, st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .items_start()
        .w_full()
        .text_size(st.base)
        .font_family(st.mono.clone())
        .child(
            div()
                .w(px(96.0))
                .flex_none()
                .text_color(st.dim)
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(st.fg)
                .child(SharedString::from(value)),
        )
}

/// An underlined section heading.
pub(crate) fn section_heading(text: &str, st: &DetailStyle) -> gpui::Div {
    div()
        .w_full()
        .pt_3()
        .pb_1()
        .border_b_1()
        .border_color(st.dim)
        .text_color(st.accent)
        .font_family(st.mono.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(st.pt * 0.95))
        .child(SharedString::from(text.to_uppercase()))
}

/// A compact tab/button for narrow chrome surfaces. The caller owns the tab
/// group layout and attaches the click listener; this primitive owns the shared
/// selected/hover typography and geometry. An optional indicator remains inside
/// the equal-width target. Selection is carried by the caller's background
/// color; labels stay at normal foreground contrast.
pub(crate) fn compact_tab(
    id: impl Into<ElementId>,
    label: &str,
    indicator: Option<gpui::AnyElement>,
    selected: bool,
    selected_bg: Hsla,
    st: &DetailStyle,
) -> gpui::Stateful<gpui::Div> {
    let transparent: Hsla = rgba(0x00000000).into();
    let mut tab = div()
        .id(id)
        .flex_1()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .py(px(3.0))
        .rounded_sm()
        .cursor_pointer()
        .font_family(st.mono.clone())
        .font_weight(if selected {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        })
        .text_size(px(st.pt * 0.82))
        .text_color(st.fg)
        .bg(if selected { selected_bg } else { transparent })
        .hover(|s| s.bg(selected_bg))
        .child(SharedString::from(label.to_string()));
    if let Some(indicator) = indicator {
        tab = tab.child(indicator);
    }
    tab
}

/// A small always-visible numeric indicator for compact chrome. Its tint is
/// semantic (for example ready-green or working-orange), while the tab that
/// contains it remains responsible for selected/hover treatment.
pub(crate) fn compact_count_indicator(
    id: impl Into<ElementId>,
    count: usize,
    tint: Hsla,
    st: &DetailStyle,
) -> gpui::Stateful<gpui::Div> {
    let mut wash = tint;
    wash.a *= 0.14;
    div()
        .id(id)
        .min_w(px(16.0))
        .h(px(16.0))
        .px(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .bg(wash)
        .text_color(tint)
        .font_family(st.mono.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(st.pt * 0.7))
        .child(SharedString::from(count.to_string()))
}

/// A compact heading inside a dense list. The caller supplies the semantic
/// glyph and tint; this primitive owns the shared uppercase label, count,
/// trailing hairline, and spacing so repeated list groups read as one system.
pub(crate) fn compact_list_group_heading(
    id: impl Into<ElementId>,
    glyph: &str,
    label: &str,
    count: usize,
    tint: Hsla,
    st: &DetailStyle,
) -> gpui::Stateful<gpui::Div> {
    let mut quiet_tint = tint;
    quiet_tint.a *= 0.72;
    let mut hairline = tint;
    hairline.a *= 0.24;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap_2()
        .px_3()
        .pt(px(8.0))
        .pb(px(3.0))
        .font_family(st.mono.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(st.pt * 0.74))
        .text_color(tint)
        .child(
            div()
                .w(px(16.0))
                .flex_none()
                .text_center()
                .child(SharedString::from(glyph.to_string())),
        )
        .child(SharedString::from(label.to_uppercase()))
        .child(
            div()
                .flex_1()
                .h(px(1.0))
                .bg(hairline),
        )
        .child(
            div()
                .text_color(quiet_tint)
                .child(SharedString::from(count.to_string())),
        )
}

/// One row in a small cursor-anchored context menu. Callers own the popup shell
/// and attach the domain action; this primitive keeps glyph alignment,
/// typography, spacing, and hover treatment consistent across menus.
pub(crate) fn context_menu_item(
    id: impl Into<ElementId>,
    glyph: &str,
    glyph_color: Hsla,
    label: &str,
    label_color: Hsla,
    hover_bg: Hsla,
    mono: &SharedString,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
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
        .child(
            div()
                .flex_1()
                .text_color(label_color)
                .child(SharedString::from(label.to_string())),
        )
}

/// One option in a compact picker card. The caller supplies the domain label,
/// optional trailing badge, and interaction handler; this primitive owns the
/// accent rail, selected/hover treatment, typography, and alignment shared by
/// destination choosers and other keyboard-first option lists.
#[allow(clippy::too_many_arguments)]
pub(crate) fn picker_option_row(
    id: impl Into<ElementId>,
    glyph: &str,
    label: &str,
    badge: Option<(&str, Hsla)>,
    selected: bool,
    accent: Hsla,
    label_color: Hsla,
    selected_bg: Hsla,
    body_font: &SharedString,
    mono_font: &SharedString,
) -> gpui::Stateful<gpui::Div> {
    let transparent: Hsla = rgba(0x00000000).into();
    let mut hover_bg = selected_bg;
    hover_bg.a *= 0.62;
    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(42.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .bg(if selected { selected_bg } else { transparent })
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .w(px(2.0))
                .h(px(24.0))
                .flex_none()
                .rounded(px(1.0))
                .bg(if selected { accent } else { transparent }),
        )
        .child(
            div()
                .w(px(18.0))
                .flex_none()
                .text_center()
                .font_family(mono_font.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.0))
                .text_color(accent)
                .child(SharedString::from(glyph.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .font_family(body_font.clone())
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_size(px(13.0))
                .text_color(label_color)
                .child(SharedString::from(label.to_string())),
        );
    if let Some((badge, badge_color)) = badge {
        let mut badge_bg = badge_color;
        badge_bg.a *= 0.10;
        row = row.child(
            div()
                .flex_none()
                .px(px(7.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded(px(10.0))
                .bg(badge_bg)
                .font_family(mono_font.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(10.0))
                .text_color(badge_color)
                .child(SharedString::from(badge.to_string())),
        );
    }
    row
}

/// An author · timestamp header over a multiline body (comments, updates).
pub(crate) fn note_block(author: String, when: String, body: &str, st: &DetailStyle) -> gpui::Div {
    let hdr = if when.is_empty() {
        author
    } else {
        format!("{author}  ·  {when}")
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap_1()
        .pb_2()
        .child(
            div()
                .text_color(st.accent)
                .font_family(st.mono.clone())
                .text_size(px(st.pt * 0.9))
                .child(SharedString::from(hdr)),
        )
        .child(multiline_text(body, st.fg, &st.prose, st.base))
}

/// Trim an ISO-8601 timestamp to `YYYY-MM-DD HH:MM`.
pub(crate) fn fmt_iso_datetime(s: &Option<String>) -> String {
    match s {
        Some(d) if d.len() >= 16 => d[..16].replace('T', " "),
        Some(d) => d.clone(),
        None => String::new(),
    }
}
