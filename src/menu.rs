use crossterm::event::KeyEvent;

use crate::keys::KeyPress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuNodeKind {
    Command,
    Submenu,
    Separator,
    Label,
}

#[derive(Debug, Clone)]
pub struct MenuNode {
    pub key: Vec<KeyPress>,
    pub label: String,
    pub action: MenuAction,
}

impl MenuNode {
    pub fn entry(key_str: &str, label: &str, action: &str) -> Self {
        let key = crate::keys::parse_key_sequence(key_str)
            .unwrap_or_else(|e| panic!("invalid menu key \"{}\": {}", key_str, e));
        Self {
            key,
            label: label.into(),
            action: MenuAction::Command(action.into()),
        }
    }

    pub fn submenu(key_str: &str, label: &str, children: Vec<MenuNode>) -> Self {
        let key = crate::keys::parse_key_sequence(key_str)
            .unwrap_or_else(|e| panic!("invalid menu key \"{}\": {}", key_str, e));
        Self {
            key,
            label: label.into(),
            action: MenuAction::Submenu(children),
        }
    }

    pub fn separator() -> Self {
        Self {
            key: Vec::new(),
            label: String::new(),
            action: MenuAction::Separator,
        }
    }

    pub fn label(text: &str) -> Self {
        Self {
            key: Vec::new(),
            label: text.into(),
            action: MenuAction::Label(text.into()),
        }
    }

    pub fn kind(&self) -> MenuNodeKind {
        match &self.action {
            MenuAction::Command(_) => MenuNodeKind::Command,
            MenuAction::Submenu(_) => MenuNodeKind::Submenu,
            MenuAction::Separator => MenuNodeKind::Separator,
            MenuAction::Label(_) => MenuNodeKind::Label,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(String),
    Separator,
    Label(String),
}

pub struct MenuState {
    active: bool,
    pub path: Vec<usize>,
}

impl MenuState {
    pub fn new() -> Self {
        Self {
            active: false,
            path: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn open(&mut self) {
        self.active = true;
        self.path.clear();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.path.clear();
    }

    /// Process a key event in the menu. Returns Some(command_name) if a command was selected.
    /// Returns None if a submenu was entered or key was unrecognized.
    /// Closes the menu when a command is executed.
    pub fn process_key_event(&mut self, event: KeyEvent, menu: &[MenuNode]) -> Option<String> {
        let press = KeyPress::from_event(event);
        let nodes = self.current_nodes(menu);

        for node in nodes.iter() {
            if matches!(node.action, MenuAction::Separator | MenuAction::Label(_)) {
                continue;
            }
            if node.key.len() == 1 && node.key[0] == press {
                match &node.action {
                    MenuAction::Command(cmd) => {
                        let cmd = cmd.clone();
                        self.close();
                        return Some(cmd);
                    }
                    MenuAction::Submenu(_) => {
                        let idx = self.resolve_node_index(menu, &press);
                        if let Some(idx) = idx {
                            self.path.push(idx);
                        }
                        return None;
                    }
                    _ => {}
                }
            }
        }
        None // unrecognized key — ignored, menu stays open
    }

    /// Handle escape: go up one level, or close if at root.
    pub fn handle_escape(&mut self) {
        if self.path.is_empty() {
            self.close();
        } else {
            self.path.pop();
        }
    }

    /// Get the menu nodes for the current depth.
    pub fn current_nodes<'a>(&self, menu: &'a [MenuNode]) -> &'a [MenuNode] {
        let mut nodes = menu;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                } else {
                    return &[];
                }
            } else {
                return &[];
            }
        }
        nodes
    }

    /// Get the current submenu label for display (e.g., "goto").
    pub fn current_label(&self, menu: &[MenuNode]) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        let mut nodes = menu;
        let mut label = None;
        for &idx in &self.path {
            if let Some(node) = nodes.get(idx) {
                label = Some(node.label.clone());
                if let MenuAction::Submenu(children) = &node.action {
                    nodes = children;
                }
            }
        }
        label
    }

    fn resolve_node_index(&self, menu: &[MenuNode], press: &KeyPress) -> Option<usize> {
        let mut target = menu;
        for &idx in &self.path {
            if let Some(node) = target.get(idx)
                && let MenuAction::Submenu(children) = &node.action
            {
                target = children;
            }
        }
        target.iter().position(|n| n.key.len() == 1 && n.key[0] == *press)
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default menu tree.
pub fn default_menu() -> Vec<MenuNode> {
    vec![
        MenuNode::entry("f", "file browser", "file-browser"),
        MenuNode::entry("F", "file browser (full)", "file-browser-full"),
        MenuNode::entry("/", "search", "search-forward"),
        MenuNode::entry("q", "quit", "quit"),
        MenuNode::entry("s", "save", "save"),
        MenuNode::entry("v", "toggle view", "toggle-view"),
        MenuNode::entry("b", "buffers", "buffer-list"),
        MenuNode::entry("o", "outline", "outline"),
        MenuNode::submenu("n", "navigate", vec![
            MenuNode::entry("l", "links", "nav-links"),
            MenuNode::entry("h", "headings", "nav-headings"),
            MenuNode::entry("i", "list items", "nav-list-items"),
            MenuNode::entry("c", "code blocks", "nav-code-blocks"),
            MenuNode::entry("m", "cycle mode", "nav-cycle"),
        ]),
        MenuNode::submenu("g", "goto", vec![
            MenuNode::entry("g", "top", "goto-top"),
            MenuNode::entry("e", "bottom", "goto-bottom"),
            MenuNode::entry("h", "next heading", "goto-heading"),
        ]),
    ]
}
