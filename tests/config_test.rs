use yalda::acp_channel::{DEFAULT_PERMISSION_MODE, PermissionMode};
use yalda::config::Config;
use yalda::keybind::KeybindManager;
use yalda::keys::{Key, KeyPress, Modifiers};
use yalda::theme::ThemeName;

fn parse_config(content: &str) -> Config {
    Config::load_from_str(content).unwrap()
}

#[test]
fn test_empty_config() {
    let config = parse_config("");
    assert_eq!(config.max_line_width, 80);
    assert_eq!(config.theme, ThemeName::Dracula);
    assert!(config.keybinds.is_none());
    assert!(config.menu.is_none());
}

#[test]
fn test_keybindings_basic() {
    let config = parse_config(
        r#"
        keybindings {
            "ctrl-d" "half-page-down"
            "g g" "goto-top"
        }
    "#,
    );
    let kb = config.keybinds.unwrap();
    assert!(!kb.reset_defaults);
    assert_eq!(kb.bindings.len(), 2);
    assert_eq!(kb.bindings[0].1, "half-page-down");
    assert_eq!(kb.bindings[1].1, "goto-top");
}

#[test]
fn test_keybindings_reset_defaults() {
    let config = parse_config(
        r#"
        keybindings {
            reset-defaults #true
            "j" "move-down"
        }
    "#,
    );
    let kb = config.keybinds.unwrap();
    assert!(kb.reset_defaults);
    assert_eq!(kb.bindings.len(), 1);
}

#[test]
fn test_keybindings_command_with_args() {
    let config = parse_config(
        r#"
        keybindings {
            "ctrl-k h" ":goto-heading 2"
        }
    "#,
    );
    let kb = config.keybinds.unwrap();
    assert_eq!(kb.bindings[0].1, ":goto-heading 2");
}

#[test]
fn test_menu_basic() {
    let config = parse_config(
        r#"
        menu {
            entry key="f" label="file browser" action="file-browser"
            entry key="q" label="quit" action="quit"
        }
    "#,
    );
    let menu = config.menu.unwrap();
    assert!(!menu.reset_defaults);
    assert_eq!(menu.nodes.len(), 2);
}

#[test]
fn test_menu_with_submenu() {
    let config = parse_config(
        r#"
        menu {
            submenu key="g" label="goto" {
                entry key="g" label="top" action="goto-top"
                entry key="e" label="bottom" action="goto-bottom"
            }
        }
    "#,
    );
    let menu = config.menu.unwrap();
    assert_eq!(menu.nodes.len(), 1);
}

#[test]
fn test_menu_separator_and_label() {
    let config = parse_config(
        r#"
        menu {
            entry key="f" label="files" action="file-browser"
            separator
            label "Navigation"
            entry key="g" label="goto top" action="goto-top"
        }
    "#,
    );
    let menu = config.menu.unwrap();
    assert_eq!(menu.nodes.len(), 4);
}

#[test]
fn test_menu_reset_defaults() {
    let config = parse_config(
        r#"
        menu {
            reset-defaults #true
            entry key="q" label="quit" action="quit"
        }
    "#,
    );
    let menu = config.menu.unwrap();
    assert!(menu.reset_defaults);
}

#[test]
fn test_invalid_key_sequence_fails() {
    let result = Config::load_from_str(
        r#"
        keybindings {
            "ctrl-" "half-page-down"
        }
    "#,
    );
    assert!(result.is_err());
}

#[test]
fn test_unknown_command_fails() {
    let result = Config::load_from_str(
        r#"
        keybindings {
            "ctrl-d" "nonexistent-command"
        }
    "#,
    );
    assert!(result.is_err());
}

#[test]
fn test_missing_menu_entry_action_fails() {
    let result = Config::load_from_str(
        r#"
        menu {
            entry key="f" label="file browser"
        }
    "#,
    );
    assert!(result.is_err());
}

#[test]
fn test_unknown_theme_fails() {
    let result = Config::load_from_str(
        r#"
        theme "nonexistent"
    "#,
    );
    assert!(result.is_err());
}

#[test]
fn test_default_permission_mode_parses() {
    let config = parse_config(
        r#"
        default-permission-mode "auto-edit"
    "#,
    );
    assert_eq!(config.default_permission_mode, PermissionMode::AutoEdit);
}

#[test]
fn test_unknown_permission_mode_fails() {
    let result = Config::load_from_str(
        r#"
        default-permission-mode "nonsense"
    "#,
    );
    assert!(result.is_err());
}

#[test]
fn test_default_permission_mode_absent_yields_default() {
    let config = parse_config("");
    assert_eq!(config.default_permission_mode, DEFAULT_PERMISSION_MODE);
    assert_eq!(config.default_permission_mode, PermissionMode::Yolo);
}

#[test]
fn test_config_keybinds_override_default() {
    let config = Config::load_from_str(
        r#"
        keybindings {
            "j" "scroll-up"
        }
    "#,
    )
    .unwrap();

    let kb = config.keybinds.unwrap();
    let mut mgr = KeybindManager::default();
    mgr.apply_bindings(&kb.bindings);

    // j should now be scroll-up instead of move-down
    let result = mgr.process_key(KeyPress::new(Key::Char('j'), Modifiers::NONE));
    assert_eq!(result, Some("scroll-up".to_string()));

    // k should still be the default
    let result = mgr.process_key(KeyPress::new(Key::Char('k'), Modifiers::NONE));
    assert_eq!(result, Some("move-up".to_string()));
}

#[test]
fn test_config_keybinds_reset_defaults() {
    let config = Config::load_from_str(
        r#"
        keybindings {
            reset-defaults #true
            "j" "move-down"
        }
    "#,
    )
    .unwrap();

    let kb = config.keybinds.unwrap();
    let mut mgr = if kb.reset_defaults {
        KeybindManager::new(
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    } else {
        KeybindManager::default()
    };
    mgr.apply_bindings(&kb.bindings);

    // j works
    let result = mgr.process_key(KeyPress::new(Key::Char('j'), Modifiers::NONE));
    assert_eq!(result, Some("move-down".to_string()));

    // k should NOT work (defaults were reset)
    let result = mgr.process_key(KeyPress::new(Key::Char('k'), Modifiers::NONE));
    assert_eq!(result, None);
}

#[test]
fn test_config_full_round_trip() {
    let config = Config::load_from_str(
        r#"
        theme "nightfox"

        display {
            max-line-width 120
        }

        keybindings {
            "ctrl-d" "half-page-down"
            "g g" "goto-top"
        }

        menu {
            entry key="f" label="files" action="file-browser"
            submenu key="g" label="goto" {
                entry key="g" label="top" action="goto-top"
            }
        }
    "#,
    )
    .unwrap();

    assert_eq!(config.theme, ThemeName::Nightfox);
    assert_eq!(config.max_line_width, 120);
    assert!(config.keybinds.is_some());
    assert!(config.menu.is_some());
}
