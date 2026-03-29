use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::menu::{MenuState, MenuNode, MenuAction, default_menu};

#[test]
fn test_menu_starts_inactive() {
    let state = MenuState::new();
    assert!(!state.is_active());
}

#[test]
fn test_menu_open_close() {
    let mut state = MenuState::new();
    state.open();
    assert!(state.is_active());
    state.close();
    assert!(!state.is_active());
}

#[test]
fn test_menu_command_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("file-browser".to_string()));
    assert!(!state.is_active());
}

#[test]
fn test_menu_submenu_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, None);
    assert!(state.is_active());
}

#[test]
fn test_menu_submenu_then_command() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE), &menu);
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("goto-top".to_string()));
    assert!(!state.is_active());
}

#[test]
fn test_menu_escape_from_submenu() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE), &menu);
    state.handle_escape();
    assert!(state.is_active());
    assert!(state.path.is_empty());
}

#[test]
fn test_menu_escape_from_root() {
    let mut state = MenuState::new();
    state.open();
    state.handle_escape();
    assert!(!state.is_active());
}

#[test]
fn test_menu_unrecognized_key_ignored() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, None);
    assert!(state.is_active());
}

#[test]
fn test_menu_separator_and_label() {
    let menu = vec![
        MenuNode::entry("f", "file", "file-browser"),
        MenuNode::separator(),
        MenuNode::label("Navigation"),
        MenuNode::entry("g", "goto top", "goto-top"),
    ];
    let mut state = MenuState::new();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &menu,
    );
    assert_eq!(result, Some("file-browser".to_string()));
}

#[test]
fn test_menu_modifier_key_entry() {
    use sketch::keys::KeyPress;
    let menu = vec![
        MenuNode {
            key: vec![KeyPress::new(KeyCode::Char('h'), KeyModifiers::CONTROL)],
            label: "prev heading".into(),
            action: MenuAction::Command("prev-heading".into()),
        },
    ];
    let mut state = MenuState::new();
    state.open();
    let result = state.process_key_event(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        &menu,
    );
    assert_eq!(result, Some("prev-heading".to_string()));
}
