use yalda::document::Document;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_new_document_from_string() {
    let doc = Document::from_text("Hello\nWorld".to_string(), PathBuf::from("test.md"));
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(0), "Hello\n");
    assert_eq!(doc.line_text(1), "World");
    assert!(!doc.is_modified());
}

#[test]
fn test_insert_char() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.insert_char(0, 5, 'X'); // line 0, char 5
    assert_eq!(doc.line_text(0), "HelloX");
    assert!(doc.is_modified());
}

#[test]
fn test_insert_newline() {
    let mut doc = Document::from_text("Hello World".to_string(), PathBuf::from("test.md"));
    doc.insert_char(0, 5, '\n');
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(0), "Hello\n");
    assert_eq!(doc.line_text(1), " World");
}

#[test]
fn test_delete_char() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.delete_char(0, 4); // delete 'o'
    assert_eq!(doc.line_text(0), "Hell");
    assert!(doc.is_modified());
}

#[test]
fn test_delete_line() {
    let mut doc = Document::from_text("Line1\nLine2\nLine3".to_string(), PathBuf::from("test.md"));
    doc.delete_line(1);
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_text(1), "Line3");
}

#[test]
fn test_undo_insert() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.begin_undo_group(0, 5, &[], 0);
    doc.insert_char(0, 5, 'X');
    doc.end_undo_group(0, 6);
    doc.undo(&[], 0);
    assert_eq!(doc.line_text(0), "Hello");
    assert!(!doc.is_modified());
}

#[test]
fn test_undo_redo() {
    let mut doc = Document::from_text("Hello".to_string(), PathBuf::from("test.md"));
    doc.begin_undo_group(0, 5, &[], 0);
    doc.insert_char(0, 5, 'X');
    doc.end_undo_group(0, 6);
    doc.undo(&[], 0);
    assert_eq!(doc.line_text(0), "Hello");
    doc.redo(&[], 0);
    assert_eq!(doc.line_text(0), "HelloX");
}

#[test]
fn test_save() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.md");
    std::fs::write(&path, "Original").unwrap();
    let mut doc = Document::from_text("Modified".to_string(), path.clone());
    doc.insert_char(0, 8, '!');
    assert!(doc.is_modified());
    doc.save().unwrap();
    assert!(!doc.is_modified());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Modified!");
}

#[test]
fn test_full_text() {
    let doc = Document::from_text("Hello\nWorld".to_string(), PathBuf::from("test.md"));
    assert_eq!(doc.full_text(), "Hello\nWorld");
}
