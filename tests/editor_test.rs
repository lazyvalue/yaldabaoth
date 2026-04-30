use sketch::editor::Editor;
use std::path::PathBuf;

fn editor(text: &str) -> Editor {
    Editor::new(text.to_string(), PathBuf::from("test.md"))
}

#[test]
fn test_insert_char() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 5;
    ed.begin_insert();
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Hello!");
}

#[test]
fn test_insert_mode_undo() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 5;
    ed.begin_insert();
    ed.insert_char('!');
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Hello!!");
    ed.undo();
    assert_eq!(ed.document().line_text(0), "Hello");
}

#[test]
fn test_delete_char() {
    let mut ed = editor("Hello");
    ed.cursor_mut().col = 4; // on 'o'
    ed.delete_char_at_cursor();
    assert_eq!(ed.document().line_text(0), "Hell");
}

#[test]
fn test_delete_line() {
    let mut ed = editor("Line1\nLine2\nLine3");
    ed.cursor_mut().line = 1;
    ed.delete_current_line();
    assert_eq!(ed.document().line_count(), 2);
    assert_eq!(ed.document().line_text(1), "Line3");
}

#[test]
fn test_delete_line_undo() {
    let mut ed = editor("Line1\nLine2\nLine3");
    ed.cursor_mut().line = 1;
    ed.delete_current_line();
    ed.undo();
    assert_eq!(ed.document().line_count(), 3);
    assert_eq!(ed.document().line_text(1), "Line2\n");
}

#[test]
fn test_open_line_below() {
    let mut ed = editor("Line1\nLine2");
    ed.open_line_below();
    assert_eq!(ed.document().line_count(), 3);
    assert_eq!(ed.cursor().line, 1);
    assert_eq!(ed.document().line_text(1), "\n");
}

#[test]
fn test_active_block_index() {
    let mut ed = editor("# Heading\n\nParagraph\n");
    ed.cursor_mut().line = 2; // on "Paragraph"
    let idx = ed.active_block_index();
    assert!(idx.is_some());
}

#[test]
fn test_block_text() {
    let ed = editor("# Heading\n\nParagraph\n");
    let blocks = ed.block_boundaries();
    assert!(blocks.len() >= 2);
    let first_block_text = ed.block_text(0);
    assert!(first_block_text.contains("Heading"));
}

// --- Helix-style selection tests ---

#[test]
fn test_selection_starts_empty() {
    let ed = editor("Hello");
    assert_eq!(ed.selection_range(), None);
    assert_eq!(ed.selection_anchor(), None);
}

#[test]
fn test_word_motion_creates_selection() {
    let mut ed = editor("Hello world");
    ed.cursor_mut().col = 0;
    ed.pre_move(true);
    ed.move_cursor_word_forward();
    let sel = ed.selection_range();
    assert!(sel.is_some(), "word motion should create a selection");
    let ((sl, sc), (el, ec)) = sel.unwrap();
    assert_eq!((sl, sc), (0, 0));
    assert!(ec > sc, "selection end should be past start");
    assert_eq!(el, 0);
}

#[test]
fn test_char_motion_collapses_selection() {
    let mut ed = editor("Hello world");
    // Establish a selection
    ed.cursor_mut().col = 0;
    ed.pre_move(true);
    ed.move_cursor_word_forward();
    assert!(ed.selection_range().is_some());
    // Now a char motion should collapse it
    ed.pre_move(false);
    ed.cursor_mut().move_left();
    assert_eq!(ed.selection_range(), None);
}

#[test]
fn test_extend_mode_preserves_selection() {
    let mut ed = editor("Hello world foo bar");
    ed.cursor_mut().col = 0;
    ed.set_extend_mode(true);
    ed.pre_move(false);
    ed.move_right_clamped(false);
    assert!(ed.selection_range().is_some(), "extend mode + char motion creates selection");
    // Subsequent motion should keep the same anchor
    let anchor_before = ed.selection_anchor().unwrap();
    ed.pre_move(false);
    ed.move_right_clamped(false);
    let anchor_after = ed.selection_anchor().unwrap();
    assert_eq!(anchor_before.col, anchor_after.col);
}

#[test]
fn test_select_all() {
    let mut ed = editor("line1\nline2\nline3");
    ed.select_all();
    let sel = ed.selection_range().unwrap();
    assert_eq!(sel.0, (0, 0));
    assert_eq!(sel.1, (2, 5));
}

#[test]
fn test_flip_selection() {
    let mut ed = editor("Hello world");
    ed.cursor_mut().col = 5;
    ed.anchor_at_cursor();
    ed.cursor_mut().col = 10;
    let before = ed.selection_range().unwrap();
    ed.flip_selection();
    let after = ed.selection_range().unwrap();
    // Range should be the same; just anchor and head swap
    assert_eq!(before, after);
    // But cursor should now be at the start
    assert_eq!(ed.cursor().col, 5);
}

#[test]
fn test_collapse_selection() {
    let mut ed = editor("Hello world");
    ed.cursor_mut().col = 0;
    ed.anchor_at_cursor();
    ed.cursor_mut().col = 5;
    assert!(ed.selection_range().is_some());
    ed.collapse_selection();
    assert!(ed.selection_range().is_none());
}

#[test]
fn test_delete_selection() {
    let mut ed = editor("Hello world");
    // Select "Hello"
    ed.cursor_mut().col = 0;
    ed.anchor_at_cursor();
    ed.cursor_mut().col = 5;
    let deleted = ed.delete_selection();
    assert!(deleted);
    assert_eq!(ed.document().line_text(0), " world");
}

#[test]
fn test_yank_selection_text() {
    let mut ed = editor("Hello world");
    ed.cursor_mut().col = 0;
    ed.anchor_at_cursor();
    ed.cursor_mut().col = 5;
    let text = ed.yank_selection().unwrap();
    assert_eq!(text, "Hello");
}

#[test]
fn test_extend_by_line_initial() {
    let mut ed = editor("first\nsecond\nthird");
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 2;
    ed.extend_by_line();
    let sel = ed.selection_range().unwrap();
    assert_eq!(sel.0, (0, 0));
    assert_eq!(sel.1, (0, 5)); // "first" length 5
}

#[test]
fn test_extend_by_line_extends_down() {
    let mut ed = editor("first\nsecond\nthird");
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 2;
    ed.extend_by_line();
    // Now selection is line 0; extend again should grow to line 1
    ed.extend_by_line();
    let sel = ed.selection_range().unwrap();
    assert_eq!(sel.0, (0, 0));
    assert_eq!(sel.1.0, 1);
}

#[test]
fn test_toggle_extend_mode() {
    let mut ed = editor("Hello");
    assert!(!ed.extend_mode());
    ed.toggle_extend_mode();
    assert!(ed.extend_mode());
    ed.toggle_extend_mode();
    assert!(!ed.extend_mode());
}

// --- Frozen-range tests (the *claude* buffer model) ---

#[test]
fn frozen_range_blocks_deletion_of_claude_text() {
    // Buffer is "ABC", all three chars belong to a frozen range.
    let mut ed = editor("ABC");
    ed.add_frozen_range(0, 3);
    // Cursor at position 1, try delete-char ('x'). Should be a no-op because
    // the char at index 1 is frozen.
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 1;
    ed.delete_char_at_cursor();
    assert_eq!(ed.document().line_text(0), "ABC");
}

#[test]
fn insert_inside_frozen_line_is_rejected() {
    // Line-aligned model: mid-frozen-line inserts are no-ops. The frozen line
    // stays intact.
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 1;
    ed.begin_insert();
    ed.insert_char('X');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
}

#[test]
fn enter_at_col_zero_of_frozen_line_inserts_blank_line_above() {
    // Pressing Enter at the start of a frozen line creates an empty editable
    // line above it; the frozen range shifts down by one.
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('\n');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "\n");
    assert_eq!(ed.document().line_text(1), "ABC\n");
    assert_eq!(ed.frozen_lines(), &[(1, 2)]);
}

#[test]
fn enter_at_end_of_frozen_line_inserts_blank_line_below() {
    // Pressing Enter at end-of-line of a frozen line opens an editable line
    // after it; the frozen range stays put.
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 3; // end of "ABC"
    ed.begin_insert();
    ed.insert_char('\n');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
    assert_eq!(ed.document().line_text(1), "\n");
    assert_eq!(ed.frozen_lines(), &[(0, 1)]);
}

#[test]
fn non_newline_at_frozen_line_boundary_is_rejected() {
    // Even at col 0 or end-of-line, only '\n' may be inserted at a frozen
    // line — anything else is a no-op.
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('X');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
}

#[test]
fn insert_on_editable_line_below_frozen_works() {
    // The realistic shape: a frozen line followed by an editable line. The
    // user types freely on the editable line.
    let mut ed = editor("ABC\nuser\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 4;
    ed.begin_insert();
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
    assert_eq!(ed.document().line_text(1), "user!\n");
    assert_eq!(ed.frozen_lines(), &[(0, 1)]);
}

#[test]
fn backspace_into_frozen_line_is_rejected() {
    // Buffer: frozen "ABC" then editable empty line. Backspace at col 0 of
    // the editable line would delete the trailing '\n' of the frozen line and
    // join them — must be rejected.
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.backspace();
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
    assert_eq!(ed.document().line_count(), 2);
}

#[test]
fn backspace_on_editable_line_works() {
    // Editable line below a frozen line. Backspace removes user-typed chars.
    let mut ed = editor("ABC\nuserX\n");
    ed.add_frozen_lines(0, 1);
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 5; // after 'X'
    ed.begin_insert();
    ed.backspace();
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "ABC\n");
    assert_eq!(ed.document().line_text(1), "user\n");
    assert_eq!(ed.frozen_lines(), &[(0, 1)]);
}

#[test]
fn lockable_prefix_blocks_all_edits_below() {
    let mut ed = editor("locked\neditable");
    // Lock the first line and its newline (chars 0..7 of "locked\n").
    ed.set_lockable_through_char(7);
    // Cursor on line 0 (locked). Try inserting.
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 3;
    ed.begin_insert();
    ed.insert_char('X');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "locked\n");

    // Cursor on line 1 (editable). Insert succeeds.
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 8;
    ed.begin_insert();
    ed.insert_char('!');
    ed.end_insert();
    assert_eq!(ed.document().line_text(1).trim_end_matches('\n'), "editable!");
}

#[test]
fn delete_selection_rejects_frozen_overlap() {
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.select_all();
    let deleted = ed.delete_selection();
    assert!(!deleted, "should refuse to delete a selection that overlaps frozen text");
    assert_eq!(ed.document().line_text(0), "ABC\n");
}

#[test]
fn programmatic_insert_bypasses_lockable_guard() {
    let mut ed = editor("");
    ed.set_lockable_through_line(0);
    ed.programmatic_insert(0, "Claude says hi\n");
    ed.add_frozen_lines(0, 1);
    assert_eq!(ed.document().line_text(0), "Claude says hi\n");
    assert_eq!(ed.frozen_lines(), &[(0, 1)]);
}

#[test]
fn extract_editable_inserts_returns_only_user_text() {
    // Realistic shape: a frozen Claude line, then an editable user line, then
    // another frozen line, then another editable line. extract joins the
    // editable lines with a blank-line separator.
    let mut ed = editor("Claude one\nX\nClaude two\nYZ\n");
    ed.add_frozen_lines(0, 1);
    ed.add_frozen_lines(2, 3);
    let payload = ed.extract_editable_inserts();
    assert_eq!(payload, "X\n\nYZ");
}

#[test]
fn extract_editable_inserts_skips_locked_prefix() {
    // Lock through every line; nothing left in the active region.
    let mut ed = editor("old\nnew\n");
    ed.set_lockable_through_line(2); // line_count == 2 → all locked
    let payload = ed.extract_editable_inserts();
    assert_eq!(payload, "");
}

#[test]
fn extract_editable_inserts_handles_empty_active_region() {
    let ed = editor("");
    assert_eq!(ed.extract_editable_inserts(), "");
}

#[test]
fn extract_editable_inserts_with_no_frozen_returns_active_text() {
    // Active region exists, but no frozen ranges → entire active region is
    // editable user text. extract returns it as one run.
    let mut ed = editor("hello world");
    ed.set_lockable_through_char(0); // nothing locked
    // No frozen ranges at all.
    let payload = ed.extract_editable_inserts();
    assert_eq!(payload, "hello world");
}

// --- f / F / t / T find-char motions ---

#[test]
fn find_char_forward_moves_to_match() {
    let mut ed = editor("hello world");
    ed.cursor_mut().col = 0;
    assert!(ed.find_char_forward('w'));
    assert_eq!(ed.cursor().col, 6);
}

#[test]
fn find_char_forward_skips_current_position() {
    let mut ed = editor("aaa");
    ed.cursor_mut().col = 0;
    assert!(ed.find_char_forward('a'));
    assert_eq!(ed.cursor().col, 1);
}

#[test]
fn find_char_forward_no_match_keeps_cursor() {
    let mut ed = editor("hello");
    ed.cursor_mut().col = 0;
    assert!(!ed.find_char_forward('z'));
    assert_eq!(ed.cursor().col, 0);
}

#[test]
fn find_char_forward_stops_at_end_of_line() {
    let mut ed = editor("foo\nbar");
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 0;
    assert!(!ed.find_char_forward('b'));
    assert_eq!(ed.cursor().line, 0);
    assert_eq!(ed.cursor().col, 0);
}

#[test]
fn find_char_backward_moves_to_match() {
    let mut ed = editor("hello world");
    ed.cursor_mut().col = 10;
    assert!(ed.find_char_backward('h'));
    assert_eq!(ed.cursor().col, 0);
}

#[test]
fn find_char_backward_no_match_at_col_zero() {
    let mut ed = editor("hello");
    ed.cursor_mut().col = 0;
    assert!(!ed.find_char_backward('h'));
    assert_eq!(ed.cursor().col, 0);
}

#[test]
fn till_char_forward_lands_one_before() {
    let mut ed = editor("hello world");
    ed.cursor_mut().col = 0;
    assert!(ed.till_char_forward('w'));
    assert_eq!(ed.cursor().col, 5);
}

#[test]
fn till_char_backward_lands_one_after() {
    let mut ed = editor("hello world");
    ed.cursor_mut().col = 10;
    assert!(ed.till_char_backward('h'));
    assert_eq!(ed.cursor().col, 1);
}
