use yalda::keybind::KeybindManager;
use yalda::keys::{Key, KeyPress, Modifiers};

fn k(c: char) -> KeyPress {
    KeyPress::new(Key::Char(c), Modifiers::NONE)
}

fn ctrl(c: char) -> KeyPress {
    KeyPress::new(Key::Char(c), Modifiers::CONTROL)
}

#[test]
fn test_single_key_binding() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k('j'));
    assert_eq!(result, Some("move-down".to_string()));
}

#[test]
fn test_multi_key_sequence_gg() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(k('g'));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(k('g'));
    assert_eq!(result2, Some("goto-top".to_string()));
}

#[test]
fn test_multi_key_timeout_resets() {
    let mut mgr = KeybindManager::default();
    let _ = mgr.process_key(k('g'));
    mgr.reset_pending();
    let result = mgr.process_key(k('j'));
    assert_eq!(result, Some("move-down".to_string()));
}

#[test]
fn test_ctrl_modifier() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(ctrl('d'));
    assert_eq!(result, Some("half-page-down".to_string()));
}

#[test]
fn test_unknown_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k('z'));
    assert_eq!(result, None);
}

#[test]
fn test_space_opens_menu() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k(' '));
    assert_eq!(result, Some("open-menu".to_string()));
}

#[test]
fn test_insert_mode_key() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k('i'));
    assert_eq!(result, Some("insert-mode".to_string()));
}

#[test]
fn test_d_delete_selection() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k('d'));
    assert_eq!(result, Some("delete-selection".to_string()));
}

#[test]
fn test_gx_open_link() {
    let mut mgr = KeybindManager::default();
    let result1 = mgr.process_key(k('g'));
    assert_eq!(result1, None);
    let result2 = mgr.process_key(k('x'));
    assert_eq!(result2, Some("open-link".to_string()));
}

#[test]
fn test_enter_command() {
    let mut mgr = KeybindManager::default();
    let result = mgr.process_key(k(':'));
    assert_eq!(result, Some("enter-command".to_string()));
}
