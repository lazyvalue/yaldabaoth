use sketch::tree::TreeState;

#[test]
fn test_parse_markdown() {
    let md = "# Hello\n\nA paragraph.\n\n```rust\ncode\n```\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    assert!(ts.tree().is_some());
}

#[test]
fn test_block_boundaries() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    let blocks = ts.block_boundaries();
    // Should find: heading, paragraph, thematic_break
    assert!(blocks.len() >= 3);
}

#[test]
fn test_active_block_index() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    // Byte offset 0 should be in the heading block
    let idx = ts.active_block_at_byte(0);
    assert_eq!(idx, Some(0));
    // Byte offset in the paragraph
    let idx = ts.active_block_at_byte(10);
    assert!(idx.is_some());
}

#[test]
fn test_incremental_reparse() {
    let md = "# Hello\n\nWorld\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes());
    let _blocks_before = ts.block_boundaries().len();

    // Simulate editing "World" to "World!"
    let new_md = "# Hello\n\nWorld!\n";
    ts.edit(9, 14, 15); // start_byte, old_end_byte, new_end_byte
    ts.parse(new_md.as_bytes());
    assert!(ts.tree().is_some());
}
