//! Typography tokens — the ONE type scale for the markdown reading/writing
//! surfaces (graph 4f1 typography). The rendered Doc view
//! (`render_blocks.rs::block_inner` / `block_element`) and the Word-Processor
//! edit view (`edit_view.rs::build_wp`) both read their heading sizes, heading
//! line height and heading space-above from [`TYPE_SCALE`], so an `# H1` is the
//! same size whether you read it or write it. The agent transcript shares
//! `block_inner`, so its parsed markdown blocks inherit the same scale.
//!
//! Every value is at 1× zoom, in px (or a ratio / a count of `ch`); callers
//! multiply by the document `text_scale` (UXI-TextZoom-1 — chrome stays fixed,
//! so none of these apply to chrome). The inter-block paragraph gap stays the
//! named `PARAGRAPH_GAP_PX` constant (`render_blocks.rs`, UXI-ParagraphSpacing-1);
//! [`TypeScale::block_gap_px`] is the base gap it is added to.

use crate::*;

/// The shared typographic tokens. One instance: [`TYPE_SCALE`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TypeScale {
    /// Body prose size.
    pub(crate) body_px: f32,
    /// Code (fenced blocks, WP code lines) size.
    pub(crate) code_px: f32,
    /// Small de-emphasized labels inside the reading column (a code block's
    /// language label / copy button).
    pub(crate) label_px: f32,
    /// Heading sizes, h1..h6 (index `level - 1`).
    pub(crate) heading_px: [f32; 6],
    /// Line height of heading text as a multiple of its size — tighter than
    /// body leading so a wrapped heading reads as one unit.
    pub(crate) heading_line_height: f32,
    /// Extra space ABOVE a heading (h1..h6), on top of the normal block gap —
    /// headings bind to the text below them, not the text above (vertical
    /// rhythm: more space above than below).
    pub(crate) heading_space_above_px: [f32; 6],
    /// Base gap below every block (the paragraph gap is added to it).
    pub(crate) block_gap_px: f32,
    /// The reading measure: the Doc view's text column is at most this many
    /// `ch` (advance of `0`) of the body font at the current zoom, centered in
    /// wider tiles.
    pub(crate) measure_ch: f32,
    /// Minimum width of a list's marker gutter, in ems of the body size.
    pub(crate) list_gutter_min_em: f32,
    /// Space between a list marker's gutter and the item text, in ems.
    pub(crate) list_marker_gap_em: f32,
}

/// The type scale. Tune here — every consumer follows.
pub(crate) const TYPE_SCALE: TypeScale = TypeScale {
    body_px: 14.0,
    code_px: 13.0,
    label_px: 11.0,
    heading_px: [28.0, 24.0, 20.0, 18.0, 16.0, 15.0],
    heading_line_height: 1.3,
    heading_space_above_px: [18.0, 14.0, 10.0, 8.0, 6.0, 6.0],
    block_gap_px: 8.0,
    measure_ch: 72.0,
    list_gutter_min_em: 1.75,
    list_marker_gap_em: 0.5,
};

impl TypeScale {
    /// Heading size for `level` (1..=6, clamped) at 1× zoom.
    pub(crate) fn heading(&self, level: u8) -> f32 {
        self.heading_px[(level as usize).clamp(1, 6) - 1]
    }

    /// Extra space above a heading of `level` at 1× zoom.
    pub(crate) fn heading_space_above(&self, level: u8) -> f32 {
        self.heading_space_above_px[(level as usize).clamp(1, 6) - 1]
    }

    /// Heading leading, RELATIVE to the heading's own text size — so the line
    /// box follows the size (a wrong size shows as a wrong line box).
    pub(crate) fn heading_leading(&self) -> gpui::DefiniteLength {
        gpui::relative(self.heading_line_height)
    }

    /// Width of a list's marker gutter at `text_scale`, wide enough for the
    /// widest marker (`marker_chars` characters, e.g. 3 for `10.`) so every
    /// item's text starts at the same x (hanging markers).
    pub(crate) fn list_gutter(&self, marker_chars: usize, text_scale: f32) -> Pixels {
        // Proportional digits/punctuation run ≈0.6em; a half-em of slack.
        let ems = (marker_chars as f32 * 0.6 + 0.5).max(self.list_gutter_min_em);
        px(ems * self.body_px * text_scale)
    }

    /// Space between the marker gutter and the item text at `text_scale`.
    pub(crate) fn list_marker_gap(&self, text_scale: f32) -> Pixels {
        px(self.list_marker_gap_em * self.body_px * text_scale)
    }

    /// The reading measure in px, given the body font's `ch` advance at the
    /// current zoom.
    pub(crate) fn measure(&self, ch_advance: Pixels) -> Pixels {
        ch_advance * self.measure_ch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_descend_and_space_above_descends() {
        let t = TYPE_SCALE;
        for l in 1..6u8 {
            assert!(t.heading(l) > t.heading(l + 1), "h{l} larger than h{}", l + 1);
            assert!(t.heading_space_above(l) >= t.heading_space_above(l + 1));
        }
        assert!(t.heading(6) > t.body_px, "the smallest heading still outranks body");
        assert_eq!(t.heading(0), t.heading(1), "clamped");
        assert_eq!(t.heading(9), t.heading(6), "clamped");
    }

    #[test]
    fn list_gutter_fits_the_widest_marker_and_scales() {
        let t = TYPE_SCALE;
        assert!(t.list_gutter(3, 1.0) > t.list_gutter(2, 1.0));
        assert_eq!(t.list_gutter(1, 1.0), px(t.list_gutter_min_em * t.body_px));
        assert_eq!(t.list_gutter(3, 2.0), t.list_gutter(3, 1.0) * 2.0);
    }
}
