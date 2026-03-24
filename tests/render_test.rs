use pretty_assertions::assert_eq;
use ratatui::style::Modifier;
use sketch::blocks::RenderedBlock;
use sketch::render::render;
use sketch::theme::Theme;

#[test]
fn test_render_heading() {
    let md = "# Hello World";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        RenderedBlock::Heading { level, content } => {
            assert_eq!(*level, 1);
            assert_eq!(content.text_content(), "Hello World");
            assert_eq!(content.spans[0].style, theme.heading[0]);
        }
        other => panic!("Expected Heading, got {:?}", other),
    }
}

#[test]
fn test_render_paragraph_with_bold() {
    let md = "Hello **world**";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let spans = &lines[0].spans;
            assert_eq!(spans.len(), 2);
            assert_eq!(spans[0].text, "Hello ");
            assert_eq!(spans[1].text, "world");
            assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_paragraph_with_link() {
    let md = "Click [here](https://example.com) now";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let link_span = lines[0].spans.iter().find(|s| s.link.is_some()).unwrap();
            assert_eq!(link_span.text, "here");
            assert_eq!(link_span.link.as_deref(), Some("https://example.com"));
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_inline_code() {
    let md = "Use `foo()` here";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::Paragraph { lines } => {
            let code_span = lines[0].spans.iter().find(|s| s.text == "foo()").unwrap();
            assert_eq!(code_span.style, theme.code_inline);
        }
        other => panic!("Expected Paragraph, got {:?}", other),
    }
}

#[test]
fn test_render_multiple_blocks() {
    let md = "# Title\n\nParagraph text.\n\n## Subtitle";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RenderedBlock::Heading { level: 1, .. }));
    assert!(matches!(blocks[1], RenderedBlock::Paragraph { .. }));
    assert!(matches!(blocks[2], RenderedBlock::Heading { level: 2, .. }));
}
