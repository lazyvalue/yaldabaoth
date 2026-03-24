use sketch::blocks::*;
use sketch::viewport::Viewport;

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
fn test_content_offset_centers() {
    let vp = Viewport::new(80);
    assert_eq!(vp.content_offset(120), 20);
    assert_eq!(vp.content_offset(80), 0);
    assert_eq!(vp.content_offset(60), 0);
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
