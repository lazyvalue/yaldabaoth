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
