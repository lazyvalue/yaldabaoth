use insta::assert_debug_snapshot;
use sketch::render::render;
use sketch::theme::Theme;

#[test]
fn snapshot_heading_levels() {
    let md = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
    let blocks = render(md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}

#[test]
fn snapshot_complex_document() {
    let md = std::fs::read_to_string("tests/fixtures/showcase.md").unwrap();
    let blocks = render(&md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}

#[test]
fn snapshot_code_block_rust() {
    let md = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```";
    let blocks = render(md, &Theme::dark());
    assert_debug_snapshot!(blocks);
}
