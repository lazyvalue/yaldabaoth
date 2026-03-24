use sketch::keybind::Action;
use sketch::menu::{MenuState, default_menu};

#[test]
fn test_menu_starts_inactive() {
    let state = MenuState::new();
    assert!(!state.is_active());
}

#[test]
fn test_menu_open_close() {
    let mut state = MenuState::new();
    let _menu = default_menu();
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
    let result = state.process_key('f', &menu);
    assert_eq!(result, Some(Action::OpenFileBrowser));
    assert!(!state.is_active()); // command closes menu
}

#[test]
fn test_menu_submenu_key() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key('g', &menu);
    assert_eq!(result, None); // submenu opened, no action dispatched
    assert!(state.is_active()); // still in menu
}

#[test]
fn test_menu_submenu_then_command() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu); // enter goto submenu
    let result = state.process_key('g', &menu);
    assert_eq!(result, Some(Action::JumpTop));
    assert!(!state.is_active());
}

#[test]
fn test_menu_escape_from_submenu() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu); // enter goto submenu
    state.handle_escape();
    assert!(state.is_active()); // back to root menu, not closed
    assert!(state.path.is_empty());
}

#[test]
fn test_menu_escape_from_root() {
    let mut state = MenuState::new();
    let _menu = default_menu();
    state.open();
    state.handle_escape();
    assert!(!state.is_active()); // closed
}

#[test]
fn test_menu_unrecognized_key_ignored() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let result = state.process_key('z', &menu);
    assert_eq!(result, None);
    assert!(state.is_active()); // still open
}

#[test]
fn test_menu_current_nodes() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    let nodes = state.current_nodes(&menu);
    assert!(nodes.iter().any(|n| n.key == 'f'));
    assert!(nodes.iter().any(|n| n.key == 'g'));
}

#[test]
fn test_menu_submenu_current_nodes() {
    let mut state = MenuState::new();
    let menu = default_menu();
    state.open();
    state.process_key('g', &menu);
    let nodes = state.current_nodes(&menu);
    assert!(nodes.iter().any(|n| n.key == 'g')); // goto > g = top
    assert!(nodes.iter().any(|n| n.key == 'e')); // goto > e = bottom
}
