use std::path::PathBuf;
use yalda::cursor::CursorPos;
use yalda::document::Document;

fn doc(text: &str) -> Document {
    Document::from_text(text.to_string(), PathBuf::from("test.md"))
}

#[test]
fn test_move_right() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_right(&d, false);
    assert_eq!(c.col, 1);
}

#[test]
fn test_move_right_clamps_normal() {
    let d = doc("Hi");
    let mut c = CursorPos::new();
    c.col = 1; // on 'i', last char
    c.move_right(&d, false); // normal mode: can't go past last char
    assert_eq!(c.col, 1);
}

#[test]
fn test_move_right_insert_mode() {
    let d = doc("Hi");
    let mut c = CursorPos::new();
    c.col = 1;
    c.move_right(&d, true); // insert mode: can go one past
    assert_eq!(c.col, 2);
}

#[test]
fn test_move_left() {
    let _d = doc("Hello");
    let mut c = CursorPos::new();
    c.col = 3;
    c.move_left();
    assert_eq!(c.col, 2);
}

#[test]
fn test_move_left_clamps() {
    let _d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_left();
    assert_eq!(c.col, 0);
}

#[test]
fn test_move_down() {
    let d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.move_down(&d, false);
    assert_eq!(c.line, 1);
}

#[test]
fn test_move_down_clamps() {
    let d = doc("Only");
    let mut c = CursorPos::new();
    c.move_down(&d, false);
    assert_eq!(c.line, 0);
}

#[test]
fn test_move_up() {
    let _d = doc("Line1\nLine2");
    let mut c = CursorPos::new();
    c.line = 1;
    c.move_up();
    assert_eq!(c.line, 0);
}

#[test]
fn test_sticky_column() {
    let d = doc("LongLine\nHi\nLongLine");
    let mut c = CursorPos::new();
    c.col = 7; // end of "LongLine"
    c.move_down(&d, false); // "Hi" — clamps to col 1
    assert_eq!(c.col, 1);
    c.move_down(&d, false); // "LongLine" — restores to col 7
    assert_eq!(c.col, 7);
}

#[test]
fn test_move_line_start() {
    let _d = doc("Hello");
    let mut c = CursorPos::new();
    c.col = 3;
    c.move_line_start();
    assert_eq!(c.col, 0);
}

#[test]
fn test_move_line_end() {
    let d = doc("Hello");
    let mut c = CursorPos::new();
    c.move_line_end(&d, false);
    assert_eq!(c.col, 4); // on 'o', the last char
}

#[test]
fn test_move_word_forward() {
    let d = doc("hello world");
    let mut c = CursorPos::new();
    c.move_word_forward(&d);
    assert_eq!(c.col, 6); // start of "world"
}

#[test]
fn test_move_word_backward() {
    let d = doc("hello world");
    let mut c = CursorPos::new();
    c.col = 8;
    c.move_word_backward(&d);
    assert_eq!(c.col, 6); // start of "world"
}

#[test]
fn test_jump_top() {
    let _d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.line = 2;
    c.col = 3;
    c.jump_top();
    assert_eq!(c.line, 0);
    assert_eq!(c.col, 0);
}

#[test]
fn test_jump_bottom() {
    let d = doc("Line1\nLine2\nLine3");
    let mut c = CursorPos::new();
    c.jump_bottom(&d);
    assert_eq!(c.line, 2);
}
