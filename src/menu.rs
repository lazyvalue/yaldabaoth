use crate::keybind::Action;

#[derive(Debug, Clone)]
pub struct MenuNode {
    pub key: char,
    pub label: String,
    pub action: MenuAction,
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    Submenu(Vec<MenuNode>),
    Command(Action),
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

    /// Process a key press in the menu. Returns Some(Action) if a command was selected.
    /// Returns None if a submenu was entered or key was unrecognized.
    /// Closes the menu when a command is executed.
    pub fn process_key(&mut self, key: char, menu: &[MenuNode]) -> Option<Action> {
        let nodes = self.current_nodes(menu);
        for node in nodes.iter() {
            if node.key == key {
                match &node.action {
                    MenuAction::Command(action) => {
                        let action = *action;
                        self.close();
                        return Some(action);
                    }
                    MenuAction::Submenu(_) => {
                        // Find the index in the actual menu tree (not the slice)
                        let idx = self.resolve_node_index(menu, key);
                        if let Some(idx) = idx {
                            self.path.push(idx);
                        }
                        return None;
                    }
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

    fn resolve_node_index(&self, menu: &[MenuNode], key: char) -> Option<usize> {
        // We need the index in the current level's node list
        // But current_nodes returns a slice — we need to find the index
        // Walk the tree to the current level and find the matching key
        let mut target = menu;
        for &idx in &self.path {
            if let Some(node) = target.get(idx)
                && let MenuAction::Submenu(children) = &node.action
            {
                target = children;
            }
        }
        target.iter().position(|n| n.key == key)
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
        MenuNode {
            key: 'f',
            label: "file browser".into(),
            action: MenuAction::Command(Action::OpenFileBrowser),
        },
        MenuNode {
            key: '/',
            label: "search".into(),
            action: MenuAction::Command(Action::SearchForward),
        },
        MenuNode {
            key: 'g',
            label: "goto".into(),
            action: MenuAction::Submenu(vec![
                MenuNode {
                    key: 'g',
                    label: "top".into(),
                    action: MenuAction::Command(Action::JumpTop),
                },
                MenuNode {
                    key: 'e',
                    label: "bottom".into(),
                    action: MenuAction::Command(Action::JumpBottom),
                },
                MenuNode {
                    key: 'h',
                    label: "next heading".into(),
                    action: MenuAction::Command(Action::NextHeading),
                },
            ]),
        },
    ]
}
