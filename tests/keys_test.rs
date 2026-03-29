use crossterm::event::{KeyCode, KeyModifiers};
use sketch::keys::{parse_key_sequence, format_key_sequence, KeyParseError};

#[test]
fn test_parse_single_char() {
    let keys = parse_key_sequence("j").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('j'));
    assert_eq!(keys[0].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_uppercase_char() {
    let keys = parse_key_sequence("G").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('G'));
    assert_eq!(keys[0].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_ctrl_modifier() {
    let keys = parse_key_sequence("ctrl-d").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('d'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_ctrl_shift() {
    let keys = parse_key_sequence("ctrl-shift-k").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('k'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
}

#[test]
fn test_parse_alt_modifier() {
    let keys = parse_key_sequence("alt-x").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].code, KeyCode::Char('x'));
    assert_eq!(keys[0].modifiers, KeyModifiers::ALT);
}

#[test]
fn test_parse_multi_key_sequence() {
    let keys = parse_key_sequence("g g").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].code, KeyCode::Char('g'));
    assert_eq!(keys[1].code, KeyCode::Char('g'));
}

#[test]
fn test_parse_mixed_sequence() {
    let keys = parse_key_sequence("ctrl-k h").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].code, KeyCode::Char('k'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
    assert_eq!(keys[1].code, KeyCode::Char('h'));
    assert_eq!(keys[1].modifiers, KeyModifiers::NONE);
}

#[test]
fn test_parse_named_keys() {
    let keys = parse_key_sequence("space").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(' '));

    let keys = parse_key_sequence("enter").unwrap();
    assert_eq!(keys[0].code, KeyCode::Enter);

    let keys = parse_key_sequence("tab").unwrap();
    assert_eq!(keys[0].code, KeyCode::Tab);

    let keys = parse_key_sequence("esc").unwrap();
    assert_eq!(keys[0].code, KeyCode::Esc);

    let keys = parse_key_sequence("backspace").unwrap();
    assert_eq!(keys[0].code, KeyCode::Backspace);
}

#[test]
fn test_parse_arrow_keys() {
    assert_eq!(parse_key_sequence("up").unwrap()[0].code, KeyCode::Up);
    assert_eq!(parse_key_sequence("down").unwrap()[0].code, KeyCode::Down);
    assert_eq!(parse_key_sequence("left").unwrap()[0].code, KeyCode::Left);
    assert_eq!(parse_key_sequence("right").unwrap()[0].code, KeyCode::Right);
}

#[test]
fn test_parse_function_keys() {
    assert_eq!(parse_key_sequence("f1").unwrap()[0].code, KeyCode::F(1));
    assert_eq!(parse_key_sequence("f12").unwrap()[0].code, KeyCode::F(12));
}

#[test]
fn test_parse_home_end_page() {
    assert_eq!(parse_key_sequence("home").unwrap()[0].code, KeyCode::Home);
    assert_eq!(parse_key_sequence("end").unwrap()[0].code, KeyCode::End);
    assert_eq!(parse_key_sequence("pageup").unwrap()[0].code, KeyCode::PageUp);
    assert_eq!(parse_key_sequence("pagedown").unwrap()[0].code, KeyCode::PageDown);
}

#[test]
fn test_parse_ctrl_named_key() {
    let keys = parse_key_sequence("ctrl-space").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(' '));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_case_insensitive_modifiers() {
    let keys = parse_key_sequence("Ctrl-D").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('d'));
    assert_eq!(keys[0].modifiers, KeyModifiers::CONTROL);
}

#[test]
fn test_parse_shift_k_equivalent() {
    let keys1 = parse_key_sequence("shift-k").unwrap();
    let keys2 = parse_key_sequence("K").unwrap();
    assert_eq!(keys1[0].code, keys2[0].code);
}

#[test]
fn test_parse_symbols() {
    let keys = parse_key_sequence("/").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('/'));

    let keys = parse_key_sequence(":").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char(':'));

    let keys = parse_key_sequence("$").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('$'));

    let keys = parse_key_sequence("{").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('{'));

    let keys = parse_key_sequence("}").unwrap();
    assert_eq!(keys[0].code, KeyCode::Char('}'));
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
    assert!(err.reason.len() > 0);
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
