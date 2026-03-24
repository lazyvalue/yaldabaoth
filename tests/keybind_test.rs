use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::keybind::{Action, KeybindManager};

#[test]
fn test_single_key_binding() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::MoveDown));
}

#[test]
fn test_multi_key_sequence_gg() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result2, Some(Action::JumpTop));
}

#[test]
fn test_multi_key_timeout_resets() {
    let mut mgr = KeybindManager::default();
    let _ = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    mgr.reset_pending();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::MoveDown));
}

#[test]
fn test_ctrl_modifier() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(result, Some(Action::HalfPageDown));
}

#[test]
fn test_unknown_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(result, None);
}

#[test]
fn test_space_opens_menu() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::OpenMenu));
}

#[test]
fn test_insert_mode_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::InsertMode));
}

#[test]
fn test_dd_delete_line() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(result2, Some(Action::DeleteLine));
}

#[test]
fn test_gx_open_link() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(result2, Some(Action::OpenLink));
}

#[test]
fn test_enter_command() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::EnterCommand));
}
