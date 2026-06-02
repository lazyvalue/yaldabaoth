use pretty_assertions::assert_eq;
use sketch::blocks::RenderedBlock;
use sketch::render::render;
use sketch::style::Modifier;
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
            assert!(spans[1].style.modifier.contains(Modifier::BOLD));
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

#[test]
fn test_render_unordered_list() {
    let md = "- Alpha\n- Beta";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::List { ordered, items, .. } => {
            assert!(!ordered);
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].marker, "•");
            assert!(items[0].checked.is_none());
        }
        other => panic!("Expected List, got {:?}", other),
    }
}

#[test]
fn test_render_task_list() {
    let md = "- [x] Done\n- [ ] Todo";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::List { items, .. } => {
            assert_eq!(items[0].checked, Some(true));
            assert_eq!(items[1].checked, Some(false));
        }
        other => panic!("Expected List, got {:?}", other),
    }
}

#[test]
fn test_render_blockquote() {
    let md = "> Quoted text";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::BlockQuote { blocks: inner } => {
            assert_eq!(inner.len(), 1);
            assert!(matches!(inner[0], RenderedBlock::Paragraph { .. }));
        }
        other => panic!("Expected BlockQuote, got {:?}", other),
    }
}

#[test]
fn test_render_code_block() {
    let md = "```rust\nfn main() {}\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::CodeBlock { language, lines, .. } => {
            assert_eq!(language.as_deref(), Some("rust"));
            assert!(!lines.is_empty());
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}

#[test]
fn test_render_horizontal_rule() {
    let md = "---";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    assert!(matches!(blocks[0], RenderedBlock::HorizontalRule));
}

#[test]
fn test_render_image() {
    let md = "![Alt text](image.png)";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::Image { alt, url } => {
            assert_eq!(alt, "Alt text");
            assert_eq!(url, "image.png");
        }
        RenderedBlock::Paragraph { .. } => {
            // pulldown-cmark may wrap images in paragraphs — acceptable
        }
        other => panic!("Expected Image, got {:?}", other),
    }
}

#[test]
fn test_render_table() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::Table { headers, rows, .. } => {
            assert_eq!(headers.len(), 2);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].len(), 2);
        }
        other => panic!("Expected Table, got {:?}", other),
    }
}

#[test]
fn test_code_block_has_multiple_styled_spans() {
    let md = "```rust\nlet x = 42;\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::CodeBlock { lines, .. } => {
            let first_line = &lines[0];
            assert!(
                first_line.spans.len() > 1,
                "Expected multiple styled spans from syntax highlighting, got {}",
                first_line.spans.len()
            );
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}

#[test]
fn test_code_block_unknown_language_falls_back() {
    let md = "```unknownlang\nhello world\n```";
    let theme = Theme::dark();
    let blocks = render(md, &theme);
    match &blocks[0] {
        RenderedBlock::CodeBlock { lines, .. } => {
            assert_eq!(lines[0].spans.len(), 1);
            assert_eq!(lines[0].text_content(), "hello world");
        }
        other => panic!("Expected CodeBlock, got {:?}", other),
    }
}
