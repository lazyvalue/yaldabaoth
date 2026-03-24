use sketch::file_browser::FileBrowser;
use std::fs;
use tempfile::TempDir;

fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("README.md"), "hello").unwrap();
    fs::write(dir.path().join("Cargo.toml"), "cargo").unwrap();
    fs::write(dir.path().join(".hidden"), "secret").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::create_dir(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("src").join("main.rs"), "fn main(){}").unwrap();
    dir
}

#[test]
fn test_lists_directory() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    assert!(!browser.entries().is_empty());
}

#[test]
fn test_directories_sorted_first() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    let entries = browser.entries();
    // Directories should come before files
    let first_file_idx = entries.iter().position(|e| !e.is_dir);
    let last_dir_idx = entries.iter().rposition(|e| e.is_dir);
    if let (Some(first_file), Some(last_dir)) = (first_file_idx, last_dir_idx) {
        assert!(
            last_dir < first_file,
            "Directories should sort before files"
        );
    }
}

#[test]
fn test_hidden_files_excluded() {
    let dir = setup_test_dir();
    let browser = FileBrowser::new(dir.path().to_path_buf());
    assert!(!browser.entries().iter().any(|e| e.name.starts_with('.')));
}

#[test]
fn test_selection_movement() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    assert_eq!(browser.selected(), 0);
    browser.move_down();
    assert_eq!(browser.selected(), 1);
    browser.move_up();
    assert_eq!(browser.selected(), 0);
}

#[test]
fn test_selection_wraps() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.move_up(); // at 0, should wrap to last
    assert_eq!(browser.selected(), browser.visible_entries().len() - 1);
}

#[test]
fn test_enter_directory() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    // Find "src" directory
    let src_idx = browser
        .entries()
        .iter()
        .position(|e| e.name == "src")
        .unwrap();
    browser.set_selected(src_idx);
    let result = browser.enter_selected();
    assert!(result.is_none()); // entered dir, no file to open
    assert!(browser.current_dir().ends_with("src"));
    assert!(browser.entries().iter().any(|e| e.name == "main.rs"));
}

#[test]
fn test_enter_file() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let file_idx = browser
        .entries()
        .iter()
        .position(|e| e.name == "README.md")
        .unwrap();
    browser.set_selected(file_idx);
    let result = browser.enter_selected();
    assert!(result.is_some()); // returns file path to open
}

#[test]
fn test_go_parent() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let src_idx = browser
        .entries()
        .iter()
        .position(|e| e.name == "src")
        .unwrap();
    browser.set_selected(src_idx);
    browser.enter_selected();
    browser.go_parent();
    assert_eq!(browser.current_dir(), dir.path());
}

#[test]
fn test_filter() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.set_filter("read");
    let visible = browser.visible_entries();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "README.md");
}

#[test]
fn test_filter_case_insensitive() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    browser.set_filter("readme");
    let visible = browser.visible_entries();
    assert_eq!(visible.len(), 1);
}

#[test]
fn test_clear_filter() {
    let dir = setup_test_dir();
    let mut browser = FileBrowser::new(dir.path().to_path_buf());
    let all_count = browser.visible_entries().len();
    browser.set_filter("read");
    assert_eq!(browser.visible_entries().len(), 1);
    browser.clear_filter();
    assert_eq!(browser.visible_entries().len(), all_count);
}
