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

/// B17: a failed multi-key prefix resolves its FIRST key's own single binding
/// (it used to be dropped) and then re-feeds the rest.
#[test]
fn failed_prefix_fires_first_keys_single_binding_then_the_rest() {
    let mut mgr = KeybindManager::default();
    // `]` is both a single binding and the prefix of `]]`.
    mgr.apply_bindings(&[(vec![k(']')], "bracket-single".to_string())]);
    assert_eq!(mgr.process_key(k(']')), None, "still a possible `]]`");
    assert_eq!(
        mgr.process_key(k('j')).as_deref(),
        Some("bracket-single"),
        "the prefix key's own binding fires first"
    );
    assert_eq!(mgr.next_queued_action().as_deref(), Some("move-down"));
    assert_eq!(mgr.next_queued_action(), None);
    // The full sequence still wins when it completes.
    assert_eq!(mgr.process_key(k(']')), None);
    assert_eq!(
        mgr.process_key(k(']')).as_deref(),
        Some("next-heading-same-level")
    );
}

/// B17: a digit typed while a multi-key prefix is pending breaks the prefix;
/// it must not be folded into the count that preceded the prefix.
#[test]
fn digit_mid_prefix_does_not_corrupt_the_count() {
    let mut mgr = KeybindManager::default();
    assert_eq!(mgr.process_key(k('2')), None);
    assert_eq!(mgr.process_key(k('g')), None);
    assert_eq!(mgr.process_key(k('3')), None);
    assert_eq!(mgr.process_key(k('j')).as_deref(), Some("move-down"));
    assert_eq!(mgr.take_count(), Some(3), "the aborted `2g` discards its count");
}
