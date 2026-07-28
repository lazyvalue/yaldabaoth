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
/// selected/hover typography and geometry. Selection is carried by the caller's
/// background color; labels stay at normal foreground contrast.
pub(crate) fn compact_tab(
    id: impl Into<ElementId>,
    label: &str,
    selected: bool,
    selected_bg: Hsla,
    st: &DetailStyle,
) -> gpui::Stateful<gpui::Div> {
    let transparent: Hsla = rgba(0x00000000).into();
    div()
        .id(id)
        .flex_1()
        .py(px(3.0))
        .rounded_sm()
        .cursor_pointer()
        .text_center()
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
        .child(SharedString::from(label.to_string()))
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
