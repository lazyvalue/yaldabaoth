use std::fmt;
use std::path::PathBuf;

use crate::command::CommandRegistry;
use crate::keys::{self, KeyPress};
use crate::menu::MenuNode;
use crate::theme::ThemeName;

#[derive(Debug)]
pub struct ConfigError {
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Debug)]
pub struct KeybindConfig {
    pub reset_defaults: bool,
    pub bindings: Vec<(Vec<KeyPress>, String)>,
}

#[derive(Debug)]
pub struct MenuConfig {
    pub reset_defaults: bool,
    pub nodes: Vec<MenuNode>,
}

pub struct Config {
    pub max_line_width: usize,
    pub theme: ThemeName,
    pub keybinds: Option<KeybindConfig>,
    pub menu: Option<MenuConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_line_width: 80,
            theme: ThemeName::default(),
            keybinds: None,
            menu: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path();
        match path {
            Some(p) if p.exists() => Self::load_from_file(&p),
            _ => Ok(Self::default()),
        }
    }

    pub fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let doc: kdl::KdlDocument = content.parse().map_err(|e: kdl::KdlError| ConfigError {
            message: format!("invalid KDL syntax: {}", e),
        })?;
        Self::parse_document(&doc)
    }

    fn load_from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError {
            message: format!("{}: {}", path.display(), e),
        })?;

        let doc: kdl::KdlDocument = content.parse().map_err(|e: kdl::KdlError| ConfigError {
            message: format!("{}: invalid KDL: {}", path.display(), e),
        })?;

        Self::parse_document(&doc).map_err(|e| ConfigError {
            message: format!("{}: {}", path.display(), e.message),
        })
    }

    fn parse_document(doc: &kdl::KdlDocument) -> Result<Self, ConfigError> {
        let registry = CommandRegistry::default_registry();
        let mut config = Self::default();

        // Display settings
        if let Some(display) = doc.get("display")
            && let Some(children) = display.children()
            && let Some(node) = children.get("max-line-width")
            && let Some(val) = node.get(0).and_then(|v| v.as_integer())
        {
            config.max_line_width = val as usize;
        }

        // Theme
        if let Some(node) = doc.get("theme")
            && let Some(val) = node.get(0).and_then(|v| v.as_string())
        {
            config.theme = ThemeName::parse(val).ok_or_else(|| ConfigError {
                message: format!("unknown theme \"{}\"", val),
            })?;
        }

        // Keybindings
        if let Some(kb_node) = doc.get("keybindings") {
            config.keybinds = Some(Self::parse_keybindings(kb_node, &registry)?);
        }

        // Menu
        if let Some(menu_node) = doc.get("menu") {
            config.menu = Some(Self::parse_menu(menu_node, &registry)?);
        }

        Ok(config)
    }

    fn parse_keybindings(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<KeybindConfig, ConfigError> {
        let children = node.children().ok_or_else(|| ConfigError {
            message: "keybindings block requires children".into(),
        })?;

        let mut reset_defaults = false;
        let mut bindings = Vec::new();

        for child in children.nodes() {
            let name = child.name().value().to_string();

            if name == "reset-defaults" {
                if let Some(val) = child.get(0).and_then(|v| v.as_bool()) {
                    reset_defaults = val;
                }
                continue;
            }

            // Node name is the key sequence, first arg is the command
            let key_seq = keys::parse_key_sequence(&name).map_err(|e| ConfigError {
                message: format!("invalid key sequence \"{}\": {}", name, e),
            })?;

            let cmd_string = child
                .get(0)
                .and_then(|v| v.as_string())
                .ok_or_else(|| ConfigError {
                    message: format!("keybinding \"{}\" missing command string", name),
                })?
                .to_string();

            // Validate command name
            let cmd_name = cmd_string.strip_prefix(':').unwrap_or(&cmd_string);
            let cmd_name = cmd_name.split_whitespace().next().unwrap_or(cmd_name);
            if registry.lookup(cmd_name).is_none() {
                return Err(ConfigError {
                    message: format!(
                        "unknown command \"{}\" in keybinding \"{}\"",
                        cmd_name, name
                    ),
                });
            }

            bindings.push((key_seq, cmd_string));
        }

        Ok(KeybindConfig {
            reset_defaults,
            bindings,
        })
    }

    fn parse_menu(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<MenuConfig, ConfigError> {
        let children = node.children().ok_or_else(|| ConfigError {
            message: "menu block requires children".into(),
        })?;

        let mut reset_defaults = false;
        let mut nodes = Vec::new();

        for child in children.nodes() {
            let name = child.name().value().to_string();

            if name == "reset-defaults" {
                if let Some(val) = child.get(0).and_then(|v| v.as_bool()) {
                    reset_defaults = val;
                }
                continue;
            }

            let menu_node = Self::parse_menu_node(child, registry)?;
            nodes.push(menu_node);
        }

        Ok(MenuConfig {
            reset_defaults,
            nodes,
        })
    }

    fn parse_menu_node(
        node: &kdl::KdlNode,
        registry: &CommandRegistry,
    ) -> Result<MenuNode, ConfigError> {
        let name = node.name().value().to_string();

        match name.as_str() {
            "separator" => Ok(MenuNode::separator()),
            "label" => {
                let text = node
                    .get(0)
                    .and_then(|v| v.as_string())
                    .unwrap_or("---")
                    .to_string();
                Ok(MenuNode::label(&text))
            }
            "entry" => {
                let key_str = node
                    .get("key")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"key\"".into(),
                    })?;
                let label = node
                    .get("label")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"label\"".into(),
                    })?;
                let action = node
                    .get("action")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "menu entry missing required attribute \"action\"".into(),
                    })?;

                let key = keys::parse_key_sequence(key_str).map_err(|e| ConfigError {
                    message: format!("invalid key \"{}\" in menu entry: {}", key_str, e),
                })?;

                let cmd_name = action.strip_prefix(':').unwrap_or(action);
                let cmd_name = cmd_name.split_whitespace().next().unwrap_or(cmd_name);
                if registry.lookup(cmd_name).is_none() {
                    return Err(ConfigError {
                        message: format!("unknown command \"{}\" in menu entry", cmd_name),
                    });
                }

                Ok(MenuNode {
                    key,
                    label: label.into(),
                    action: crate::menu::MenuAction::Command(action.into()),
                })
            }
            "submenu" => {
                let key_str = node
                    .get("key")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "submenu missing required attribute \"key\"".into(),
                    })?;
                let label = node
                    .get("label")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| ConfigError {
                        message: "submenu missing required attribute \"label\"".into(),
                    })?;

                let key = keys::parse_key_sequence(key_str).map_err(|e| ConfigError {
                    message: format!("invalid key \"{}\" in submenu: {}", key_str, e),
                })?;

                let mut children = Vec::new();
                if let Some(child_doc) = node.children() {
                    for child in child_doc.nodes() {
                        children.push(Self::parse_menu_node(child, registry)?);
                    }
                }

                Ok(MenuNode {
                    key,
                    label: label.into(),
                    action: crate::menu::MenuAction::Submenu(children),
                })
            }
            _ => Err(ConfigError {
                message: format!("unknown menu node type \"{}\"", name),
            }),
        }
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SKETCH_CONFIG") {
        return Some(PathBuf::from(p));
    }

    if let Some(home) = dirs::home_dir() {
        let xdg = home.join(".config").join("sketch").join("config.kdl");
        if xdg.exists() {
            return Some(xdg);
        }
    }

    dirs::config_dir().map(|d| d.join("sketch").join("config.kdl"))
}
