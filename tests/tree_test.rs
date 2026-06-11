use yalda::tree::TreeState;

#[test]
fn test_parse_markdown() {
    let md = "# Hello\n\nA paragraph.\n\n```rust\ncode\n```\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes(), None);
    assert!(ts.tree().is_some());
}

#[test]
fn test_block_boundaries() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes(), None);
    let blocks = ts.block_boundaries();
    // Should find: heading, paragraph, thematic_break
    assert!(blocks.len() >= 3);
}

#[test]
fn test_active_block_index() {
    let md = "# Hello\n\nA paragraph.\n\n---\n";
    let mut ts = TreeState::new();
    ts.parse(md.as_bytes(), None);
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
    ts.parse(md.as_bytes(), None);
    let _blocks_before = ts.block_boundaries().len();

    // Simulate editing "World" to "World!" — insert '!' at byte 14
    // (the char after "World", on line 2 col 5). The InputEdit describes
    // that single splice so tree-sitter can reparse incrementally.
    let new_md = "# Hello\n\nWorld!\n";
    let edit = tree_sitter::InputEdit {
        start_byte: 14,
        old_end_byte: 14,
        new_end_byte: 15,
        start_position: tree_sitter::Point { row: 2, column: 5 },
        old_end_position: tree_sitter::Point { row: 2, column: 5 },
        new_end_position: tree_sitter::Point { row: 2, column: 6 },
    };
    ts.parse(new_md.as_bytes(), Some(edit));
    assert!(ts.tree().is_some());
}
