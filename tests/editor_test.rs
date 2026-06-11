use std::path::PathBuf;
use yalda::editor::Editor;

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
    assert!(
        ed.selection_range().is_some(),
        "extend mode + char motion creates selection"
    );
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
fn undo_restores_frozen_lines_state() {
    // Press `i` on a middle frozen line (open_line_above + insert), then
    // undo. After undo the frozen range MUST be back to its original shape
    // so the previously-frozen content is still classified as frozen.
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    let frozen_before: Vec<(usize, usize)> = ed.frozen_lines().to_vec();
    let lockable_before = ed.lockable_through_line();
    let text_before = ed.document().full_text();

    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.open_line_above();
    ed.insert_char('X');
    // open_line_above started an undo group; close it as end_insert would.
    ed.end_insert();

    // After the edit: range was split.
    assert_ne!(ed.frozen_lines(), frozen_before.as_slice());
    assert_ne!(ed.document().full_text(), text_before);

    // Undo should fully restore both the document AND the frozen state.
    ed.undo();
    assert_eq!(ed.document().full_text(), text_before, "text restored");
    assert_eq!(
        ed.frozen_lines(),
        frozen_before.as_slice(),
        "frozen ranges restored"
    );
    assert_eq!(ed.lockable_through_line(), lockable_before);
    // Bar must be frozen again.
    assert!(ed.is_frozen_line(1), "Bar must be re-frozen after undo");
}

#[test]
fn redo_restores_frozen_lines_state_post_split() {
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.open_line_above();
    ed.insert_char('X');
    ed.end_insert();

    let after_split_frozen: Vec<(usize, usize)> = ed.frozen_lines().to_vec();
    let after_split_text = ed.document().full_text();

    ed.undo();
    ed.redo();

    assert_eq!(ed.document().full_text(), after_split_text);
    assert_eq!(ed.frozen_lines(), after_split_frozen.as_slice());
}

#[test]
fn enter_on_middle_line_of_multi_line_frozen_range_splits_it() {
    // Three frozen lines: Foo, Bar, Baz. User puts cursor at start of Bar
    // and presses Enter — should produce an empty editable line BETWEEN
    // Foo and Bar, splitting the frozen range.
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('\n');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Foo\n");
    assert_eq!(ed.document().line_text(1), "\n");
    assert_eq!(ed.document().line_text(2), "Bar\n");
    assert_eq!(ed.document().line_text(3), "Baz\n");
    // Range splits: Foo stays frozen at 0; Bar+Baz frozen at 2..4.
    assert_eq!(ed.frozen_lines(), &[(0, 1), (2, 4)]);
}

#[test]
fn enter_on_last_line_of_multi_line_frozen_range_splits_it() {
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 2; // Baz
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('\n');
    ed.end_insert();
    assert_eq!(ed.document().line_text(0), "Foo\n");
    assert_eq!(ed.document().line_text(1), "Bar\n");
    assert_eq!(ed.document().line_text(2), "\n");
    assert_eq!(ed.document().line_text(3), "Baz\n");
    assert_eq!(ed.frozen_lines(), &[(0, 2), (3, 4)]);
}

#[test]
fn open_line_above_on_middle_frozen_line_creates_editable_line_user_can_type_into() {
    // The exact user scenario: Claude wrote 3 lines, user wants to insert
    // their text between Foo and Bar via `i` (which auto-calls
    // open_line_above on a frozen line).
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 1; // on Bar
    ed.cursor_mut().col = 0;

    ed.open_line_above();
    // After: cursor on the new empty line at index 1; Bar moved to 2.
    assert_eq!(ed.document().line_text(0), "Foo\n");
    assert_eq!(ed.document().line_text(1), "\n");
    assert_eq!(ed.document().line_text(2), "Bar\n");
    assert_eq!(ed.cursor().line, 1);
    assert_eq!(ed.cursor().col, 0);
    // Range split — line 1 must NOT be frozen.
    assert!(!ed.is_frozen_line(1), "new empty line must be editable");
    assert!(ed.is_frozen_line(0), "Foo stays frozen");
    assert!(ed.is_frozen_line(2), "Bar stays frozen");
    assert!(ed.is_frozen_line(3), "Baz stays frozen");

    // Now type — characters MUST appear on line 1.
    ed.insert_char('X');
    ed.insert_char('Y');
    assert_eq!(ed.document().line_text(1), "XY\n");
    assert_eq!(ed.cursor().line, 1);
    assert_eq!(ed.cursor().col, 2);
}

#[test]
fn open_line_below_on_middle_frozen_line_creates_editable_line_user_can_type_into() {
    // Same scenario via `a` (which auto-calls open_line_below).
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 1; // on Bar
    ed.cursor_mut().col = 0;

    ed.open_line_below();
    // After: cursor on the new empty line at index 2; Baz moved to 3.
    assert_eq!(ed.document().line_text(0), "Foo\n");
    assert_eq!(ed.document().line_text(1), "Bar\n");
    assert_eq!(ed.document().line_text(2), "\n");
    assert_eq!(ed.document().line_text(3), "Baz\n");
    assert_eq!(ed.cursor().line, 2);
    assert_eq!(ed.cursor().col, 0);
    assert!(!ed.is_frozen_line(2), "new empty line must be editable");
    assert!(ed.is_frozen_line(0));
    assert!(ed.is_frozen_line(1));
    assert!(ed.is_frozen_line(3));

    ed.insert_char('Z');
    assert_eq!(ed.document().line_text(2), "Z\n");
}

#[test]
fn open_line_above_on_first_line_of_frozen_range_shifts_range_down() {
    // Cursor on the first line of a multi-line frozen range — Enter should
    // shift the entire range down and put cursor on a fresh editable line.
    let mut ed = editor("Foo\nBar\nBaz\n");
    ed.add_frozen_lines(0, 3);
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 0;

    ed.open_line_above();
    assert_eq!(ed.document().line_text(0), "\n");
    assert_eq!(ed.document().line_text(1), "Foo\n");
    assert_eq!(ed.document().line_text(2), "Bar\n");
    assert_eq!(ed.document().line_text(3), "Baz\n");
    assert_eq!(ed.cursor().line, 0);
    assert_eq!(ed.cursor().col, 0);
    assert!(
        !ed.is_frozen_line(0),
        "new empty line at top must be editable"
    );
    assert_eq!(ed.frozen_lines(), &[(1, 4)]);

    ed.insert_char('Q');
    assert_eq!(ed.document().line_text(0), "Q\n");
}

#[test]
fn realistic_claude_buffer_flow_user_can_insert_between_claude_lines() {
    // Mirrors the actual app flow:
    //  1. user types "hi" + send
    //  2. lock_active_turn appends "\n\n---\n\n" and bumps lockable_through_line
    //     to the cursor's destination line (the new empty editable line)
    //  3. append_to_claude_buffer appends "Foo\nBar\nBaz\n" and registers the
    //     three frozen lines, parking the cursor on the new trailing empty line
    //  4. user navigates up to the middle frozen line and presses `i` (which
    //     in app.rs becomes open_line_above when on a frozen line)
    //  5. user types "X" and "Y"
    //
    // Expected final result: Bar's frozen line is preserved, an editable line
    // containing "XY" sits between Foo and Bar, the user can keep typing.
    let mut ed = editor("");

    // Step 1: type "hi"
    ed.cursor_mut().line = 0;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('h');
    ed.insert_char('i');
    ed.end_insert();
    assert_eq!(ed.document().full_text(), "hi");

    // Step 2: lock_active_turn equivalent
    {
        let pre_len = ed.document().rope().len_chars();
        let s = ed.document().full_text();
        let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
        let lead = "\n".repeat(2usize.saturating_sub(trailing_nl));
        let separator = format!("{}---\n\n", lead);
        ed.programmatic_insert(pre_len, &separator);
        let eof = ed.document().rope().len_chars();
        // emulate char_to_line_col(eof)
        let (cl, cc) = {
            let rope = ed.document().rope();
            let len = rope.len_chars();
            let i = eof.min(len);
            let line = rope.char_to_line(i);
            let line_start = rope.line_to_char(line);
            (line, i - line_start)
        };
        ed.set_lockable_through_line(cl);
        ed.cursor_mut().line = cl;
        ed.cursor_mut().col = cc;
    }
    // Buffer should now be "hi\n\n---\n\n"; cursor on the trailing empty line.
    assert_eq!(ed.document().full_text(), "hi\n\n---\n\n");
    let lock_line = ed.lockable_through_line();
    assert_eq!(ed.cursor().line, lock_line);

    // Step 3: append Claude reply "Foo\nBar\nBaz"
    {
        let pre_len = ed.document().rope().len_chars();
        let trimmed = "Foo\nBar\nBaz";
        let s = ed.document().full_text();
        let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
        let pad = "\n".repeat(2usize.saturating_sub(trailing_nl));
        let trailing_pad = if trimmed.ends_with('\n') { "" } else { "\n" };
        let payload = format!("{}{}{}", pad, trimmed, trailing_pad);
        ed.programmatic_insert(pre_len, &payload);
        let claude_start_char = pre_len + pad.chars().count();
        let claude_end_char =
            claude_start_char + trimmed.chars().count() + trailing_pad.chars().count();
        let line_of = |idx: usize| -> usize {
            let rope = ed.document().rope();
            let i = idx.min(rope.len_chars());
            rope.char_to_line(i)
        };
        let start_line = line_of(claude_start_char);
        let end_line = line_of(claude_end_char);
        ed.add_frozen_lines(start_line, end_line);
        let post = ed.document().rope().len_chars();
        let (cl, cc) = {
            let rope = ed.document().rope();
            let line = rope.char_to_line(post);
            let line_start = rope.line_to_char(line);
            (line, post - line_start)
        };
        ed.cursor_mut().line = cl;
        ed.cursor_mut().col = cc;
    }
    // Document now ends with the Claude reply on its own three lines.
    assert!(ed.document().full_text().ends_with("Foo\nBar\nBaz\n"));
    // Find Bar's line index.
    let bar_line = (0..ed.document().line_count())
        .find(|&l| ed.document().line_text(l) == "Bar\n")
        .expect("Bar must be present");
    assert!(ed.is_frozen_line(bar_line), "Bar must be frozen");

    // Step 4: navigate to Bar and press `i` (== open_line_above on frozen).
    ed.cursor_mut().line = bar_line;
    ed.cursor_mut().col = 0;
    ed.open_line_above();

    // The new empty line replaces Bar's old index; Bar shifts down by 1.
    let new_empty_line = bar_line;
    let new_bar_line = bar_line + 1;
    assert_eq!(ed.document().line_text(new_empty_line), "\n");
    assert_eq!(ed.document().line_text(new_bar_line), "Bar\n");
    assert!(
        !ed.is_frozen_line(new_empty_line),
        "new empty line must NOT be frozen (got is_frozen=true)"
    );
    assert!(
        ed.is_frozen_line(new_bar_line),
        "Bar must remain frozen after the split"
    );
    assert_eq!(ed.cursor().line, new_empty_line);
    assert_eq!(ed.cursor().col, 0);

    // Step 5: type "XY" — must land on the new empty line.
    ed.insert_char('X');
    ed.insert_char('Y');
    assert_eq!(ed.document().line_text(new_empty_line), "XY\n");
    assert_eq!(ed.cursor().line, new_empty_line);
    assert_eq!(ed.cursor().col, 2);
}

#[test]
fn lockable_through_line_unchanged_when_inserting_at_first_editable_line() {
    // lockable_through_line points to the first editable line. Inserting
    // empty lines at that position pushes the original first-editable
    // content down BUT the new empty lines are themselves editable, so
    // lockable_through_line must NOT shift.
    let mut ed = editor("locked\neditable\n");
    ed.set_lockable_through_line(1);
    assert!(!ed.is_frozen_line(1));
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('\n');
    ed.end_insert();
    assert_eq!(ed.lockable_through_line(), 1, "lockable must stay at 1");
    // Line 1 (the new empty) must be editable.
    ed.cursor_mut().line = 1;
    ed.cursor_mut().col = 0;
    ed.begin_insert();
    ed.insert_char('A');
    ed.end_insert();
    assert_eq!(ed.document().line_text(1), "A\n");
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
    assert_eq!(
        ed.document().line_text(1).trim_end_matches('\n'),
        "editable!"
    );
}

#[test]
fn delete_selection_rejects_frozen_overlap() {
    let mut ed = editor("ABC\n");
    ed.add_frozen_lines(0, 1);
    ed.select_all();
    let deleted = ed.delete_selection();
    assert!(
        !deleted,
        "should refuse to delete a selection that overlaps frozen text"
    );
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
