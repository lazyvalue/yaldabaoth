use yalda::keys::{Key, Modifiers, format_key_sequence, parse_key_sequence};

#[test]
fn test_parse_single_char() {
    let keys = parse_key_sequence("j").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, Key::Char('j'));
    assert_eq!(keys[0].modifiers, Modifiers::NONE);
}

#[test]
fn test_parse_uppercase_char() {
    let keys = parse_key_sequence("G").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, Key::Char('G'));
    assert_eq!(keys[0].modifiers, Modifiers::NONE);
}

#[test]
fn test_parse_ctrl_modifier() {
    let keys = parse_key_sequence("ctrl-d").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, Key::Char('d'));
    assert_eq!(keys[0].modifiers, Modifiers::CONTROL);
}

#[test]
fn test_parse_ctrl_shift() {
    let keys = parse_key_sequence("ctrl-shift-k").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, Key::Char('k'));
    assert_eq!(keys[0].modifiers, Modifiers::CONTROL | Modifiers::SHIFT);
}

#[test]
fn test_parse_alt_modifier() {
    let keys = parse_key_sequence("alt-x").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, Key::Char('x'));
    assert_eq!(keys[0].modifiers, Modifiers::ALT);
}

#[test]
fn test_parse_multi_key_sequence() {
    let keys = parse_key_sequence("g g").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].key, Key::Char('g'));
    assert_eq!(keys[1].key, Key::Char('g'));
}

#[test]
fn test_parse_mixed_sequence() {
    let keys = parse_key_sequence("ctrl-k h").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].key, Key::Char('k'));
    assert_eq!(keys[0].modifiers, Modifiers::CONTROL);
    assert_eq!(keys[1].key, Key::Char('h'));
    assert_eq!(keys[1].modifiers, Modifiers::NONE);
}

#[test]
fn test_parse_named_keys() {
    let keys = parse_key_sequence("space").unwrap();
    assert_eq!(keys[0].key, Key::Char(' '));

    let keys = parse_key_sequence("enter").unwrap();
    assert_eq!(keys[0].key, Key::Enter);

    let keys = parse_key_sequence("tab").unwrap();
    assert_eq!(keys[0].key, Key::Tab);

    let keys = parse_key_sequence("esc").unwrap();
    assert_eq!(keys[0].key, Key::Esc);

    let keys = parse_key_sequence("backspace").unwrap();
    assert_eq!(keys[0].key, Key::Backspace);
}

#[test]
fn test_parse_arrow_keys() {
    assert_eq!(parse_key_sequence("up").unwrap()[0].key, Key::Up);
    assert_eq!(parse_key_sequence("down").unwrap()[0].key, Key::Down);
    assert_eq!(parse_key_sequence("left").unwrap()[0].key, Key::Left);
    assert_eq!(parse_key_sequence("right").unwrap()[0].key, Key::Right);
}

#[test]
fn test_parse_function_keys() {
    assert_eq!(parse_key_sequence("f1").unwrap()[0].key, Key::F(1));
    assert_eq!(parse_key_sequence("f12").unwrap()[0].key, Key::F(12));
}

#[test]
fn test_parse_home_end_page() {
    assert_eq!(parse_key_sequence("home").unwrap()[0].key, Key::Home);
    assert_eq!(parse_key_sequence("end").unwrap()[0].key, Key::End);
    assert_eq!(parse_key_sequence("pageup").unwrap()[0].key, Key::PageUp);
    assert_eq!(
        parse_key_sequence("pagedown").unwrap()[0].key,
        Key::PageDown
    );
}

#[test]
fn test_parse_ctrl_named_key() {
    let keys = parse_key_sequence("ctrl-space").unwrap();
    assert_eq!(keys[0].key, Key::Char(' '));
    assert_eq!(keys[0].modifiers, Modifiers::CONTROL);
}

#[test]
fn test_parse_case_insensitive_modifiers() {
    let keys = parse_key_sequence("Ctrl-D").unwrap();
    assert_eq!(keys[0].key, Key::Char('d'));
    assert_eq!(keys[0].modifiers, Modifiers::CONTROL);
}

#[test]
fn test_parse_shift_k_equivalent() {
    let keys1 = parse_key_sequence("shift-k").unwrap();
    let keys2 = parse_key_sequence("K").unwrap();
    assert_eq!(keys1[0].key, keys2[0].key);
}

#[test]
fn test_parse_symbols() {
    let keys = parse_key_sequence("/").unwrap();
    assert_eq!(keys[0].key, Key::Char('/'));

    let keys = parse_key_sequence(":").unwrap();
    assert_eq!(keys[0].key, Key::Char(':'));

    let keys = parse_key_sequence("$").unwrap();
    assert_eq!(keys[0].key, Key::Char('$'));

    let keys = parse_key_sequence("{").unwrap();
    assert_eq!(keys[0].key, Key::Char('{'));

    let keys = parse_key_sequence("}").unwrap();
    assert_eq!(keys[0].key, Key::Char('}'));
}

#[test]
fn test_parse_error_empty() {
    let err = parse_key_sequence("").unwrap_err();
    assert!(err.reason.contains("empty"));
}

#[test]
fn test_parse_error_trailing_modifier() {
    let err = parse_key_sequence("ctrl-").unwrap_err();
    assert!(err.reason.contains("missing key"));
}

#[test]
fn test_parse_error_unknown_modifier() {
    let err = parse_key_sequence("super-k").unwrap_err();
    assert!(!err.reason.is_empty());
}

#[test]
fn test_format_single_char() {
    let keys = parse_key_sequence("j").unwrap();
    assert_eq!(format_key_sequence(&keys), "j");
}

#[test]
fn test_format_ctrl() {
    let keys = parse_key_sequence("ctrl-d").unwrap();
    assert_eq!(format_key_sequence(&keys), "ctrl-d");
}

#[test]
fn test_format_multi_key() {
    let keys = parse_key_sequence("g g").unwrap();
    assert_eq!(format_key_sequence(&keys), "g g");
}

#[test]
fn test_format_mixed_sequence() {
    let keys = parse_key_sequence("ctrl-k h").unwrap();
    assert_eq!(format_key_sequence(&keys), "ctrl-k h");
}

#[test]
fn test_format_named_key() {
    let keys = parse_key_sequence("space").unwrap();
    assert_eq!(format_key_sequence(&keys), "space");

    let keys = parse_key_sequence("enter").unwrap();
    assert_eq!(format_key_sequence(&keys), "enter");
}
