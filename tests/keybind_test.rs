use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sketch::keybind::{Action, KeybindManager};

#[test]
fn test_single_key_binding() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::ScrollDown));
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
    assert_eq!(result, Some(Action::ScrollDown));
}

#[test]
fn test_ctrl_modifier() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(result, Some(Action::HalfPageDown));
}

#[test]
fn test_quit() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert_eq!(result, Some(Action::Quit));
}

#[test]
fn test_unknown_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(result, None);
}
