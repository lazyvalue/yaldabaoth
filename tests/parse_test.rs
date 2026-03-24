use pulldown_cmark::{Event, Tag, TagEnd};

#[test]
fn test_parse_heading() {
    let md = "# Hello World";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(matches!(events[0], Event::Start(Tag::Heading { level: pulldown_cmark::HeadingLevel::H1, .. })));
    assert!(matches!(events[1], Event::Text(_)));
    assert!(matches!(events[2], Event::End(TagEnd::Heading(pulldown_cmark::HeadingLevel::H1))));
}

#[test]
fn test_parse_code_block() {
    let md = "```rust\nfn main() {}\n```";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(matches!(events[0], Event::Start(Tag::CodeBlock(_))));
}

#[test]
fn test_parse_task_list() {
    let md = "- [x] done\n- [ ] todo";
    let events: Vec<_> = sketch::parse::parse(md).collect();

    assert!(events.iter().any(|e| matches!(e, Event::TaskListMarker(true))));
    assert!(events.iter().any(|e| matches!(e, Event::TaskListMarker(false))));
}
