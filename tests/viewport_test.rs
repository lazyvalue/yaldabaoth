use yalda::blocks::*;
use yalda::viewport::Viewport;

fn make_heading(level: u8) -> RenderedBlock {
    RenderedBlock::Heading {
        level,
        content: StyledLine::plain(format!("Heading {}", level)),
    }
}

fn make_paragraph(text: &str) -> RenderedBlock {
    RenderedBlock::Paragraph {
        lines: vec![StyledLine::plain(text)],
    }
}

#[test]
fn test_content_width_respects_max() {
    let vp = Viewport::new(80);
    assert_eq!(vp.content_width(120), 80);
    assert_eq!(vp.content_width(60), 60);
}

#[test]
fn test_content_offset_left_pads() {
    let vp = Viewport::new(80);
    assert_eq!(vp.content_offset(120), 4);
    assert_eq!(vp.content_offset(80), 4);
    assert_eq!(vp.content_offset(60), 4);
    assert_eq!(vp.content_offset(2), 0);
}

#[test]
fn test_scroll_down_clamps() {
    let blocks = vec![make_heading(1), make_paragraph("text")];
    let mut vp = Viewport::new(80);
    vp.calculate_total_lines(&blocks, 80);
    vp.scroll_down(1000, 10);
    assert!(vp.scroll_offset <= vp.total_lines);
}

#[test]
fn test_scroll_up_clamps_to_zero() {
    let mut vp = Viewport::new(80);
    vp.scroll_offset = 5;
    vp.scroll_up(100);
    assert_eq!(vp.scroll_offset, 0);
}

#[test]
fn test_visible_blocks_returns_correct_blocks() {
    let blocks = vec![
        make_heading(1),
        make_paragraph("first"),
        make_paragraph("second"),
        make_paragraph("third"),
    ];
    let vp = Viewport::new(80);
    let visible = vp.visible_blocks(&blocks, 80, 5);
    assert!(!visible.is_empty());
    assert!(matches!(visible[0].block, RenderedBlock::Heading { .. }));
}

#[test]
fn test_jump_top_and_bottom() {
    let blocks = vec![make_heading(1), make_paragraph("a"), make_paragraph("b")];
    let mut vp = Viewport::new(80);
    vp.calculate_total_lines(&blocks, 80);
    vp.jump_bottom(5);
    assert!(vp.scroll_offset > 0);
    vp.jump_top();
    assert_eq!(vp.scroll_offset, 0);
}

#[test]
fn ensure_cursor_visible_keeps_cursor_within_scrolloff_at_bottom() {
    use yalda::viewport::SCROLLOFF;
    let mut vp = Viewport::new(80);
    vp.total_lines = 1000;
    let viewport_height = 30;

    // Cursor at visual row 100; viewport empty (scroll_offset = 0). Should
    // scroll so cursor sits at viewport_height - SCROLLOFF - 1.
    vp.ensure_cursor_visible(100, viewport_height);
    let cursor_visual_row = 100;
    let cursor_pos_in_viewport = cursor_visual_row - vp.scroll_offset;
    assert!(
        cursor_pos_in_viewport < viewport_height,
        "cursor must be within viewport (got pos {} of {})",
        cursor_pos_in_viewport,
        viewport_height
    );
    let bottom_margin = viewport_height - cursor_pos_in_viewport - 1;
    assert!(
        bottom_margin >= SCROLLOFF,
        "cursor must have at least SCROLLOFF rows of bottom margin (got {})",
        bottom_margin
    );
}

#[test]
fn ensure_cursor_visible_keeps_cursor_within_scrolloff_at_top() {
    use yalda::viewport::SCROLLOFF;
    let mut vp = Viewport::new(80);
    vp.total_lines = 1000;
    vp.scroll_offset = 50;
    let viewport_height = 30;

    // Cursor at visual row 51 — within the SCROLLOFF margin at the top.
    vp.ensure_cursor_visible(51, viewport_height);
    let top_margin = 51 - vp.scroll_offset;
    assert!(
        top_margin >= SCROLLOFF || vp.scroll_offset == 0,
        "cursor must have at least SCROLLOFF rows of top margin (got {})",
        top_margin
    );
}
