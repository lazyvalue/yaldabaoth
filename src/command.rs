use crate::keybind::Action;

pub struct CommandDef {
    pub name: String,
    pub aliases: Vec<String>,
    pub action: Action,
    pub description: String,
}

pub struct CommandRegistry {
    commands: Vec<CommandDef>,
}

impl CommandRegistry {
    pub fn new(commands: Vec<CommandDef>) -> Self {
        Self { commands }
    }

    /// Look up a command by name or alias. Returns the first match.
    pub fn lookup(&self, input: &str) -> Option<&CommandDef> {
        self.commands.iter().find(|cmd| {
            cmd.name == input || cmd.aliases.iter().any(|a| a == input)
        })
    }

    /// Build the default registry with all built-in commands.
    pub fn default_registry() -> Self {
        Self::new(vec![
            CommandDef {
                name: "save".into(),
                aliases: vec!["w".into()],
                action: Action::Save,
                description: "Save file".into(),
            },
            CommandDef {
                name: "save-as".into(),
                aliases: vec![],
                action: Action::SaveAs,
                description: "Save to new path".into(),
            },
            CommandDef {
                name: "quit".into(),
                aliases: vec!["q".into()],
                action: Action::Quit,
                description: "Quit (warns if modified)".into(),
            },
            CommandDef {
                name: "force-quit".into(),
                aliases: vec!["q!".into()],
                action: Action::ForceQuit,
                description: "Quit without saving".into(),
            },
            CommandDef {
                name: "save-quit".into(),
                aliases: vec!["wq".into()],
                action: Action::SaveQuit,
                description: "Save and quit".into(),
            },
            CommandDef {
                name: "toggle-view".into(),
                aliases: vec![],
                action: Action::ToggleView,
                description: "Switch rendered/raw".into(),
            },
            CommandDef {
                name: "file-browser".into(),
                aliases: vec![],
                action: Action::OpenFileBrowser,
                description: "Open file browser".into(),
            },
            CommandDef {
                name: "search".into(),
                aliases: vec![],
                action: Action::SearchForward,
                description: "Search forward".into(),
            },
            CommandDef {
                name: "goto-top".into(),
                aliases: vec![],
                action: Action::JumpTop,
                description: "Go to top".into(),
            },
            CommandDef {
                name: "goto-bottom".into(),
                aliases: vec![],
                action: Action::JumpBottom,
                description: "Go to bottom".into(),
            },
            CommandDef {
                name: "goto-heading".into(),
                aliases: vec![],
                action: Action::NextHeading,
                description: "Next heading".into(),
            },
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_by_name() {
        let reg = CommandRegistry::default_registry();
        let cmd = reg.lookup("save").unwrap();
        assert_eq!(cmd.action, Action::Save);
    }

    #[test]
    fn test_lookup_by_alias() {
        let reg = CommandRegistry::default_registry();
        let cmd = reg.lookup("w").unwrap();
        assert_eq!(cmd.action, Action::Save);
    }

    #[test]
    fn test_lookup_unknown() {
        let reg = CommandRegistry::default_registry();
        assert!(reg.lookup("nonexistent").is_none());
    }

    #[test]
    fn test_lookup_force_quit_alias() {
        let reg = CommandRegistry::default_registry();
        let cmd = reg.lookup("q!").unwrap();
        assert_eq!(cmd.action, Action::ForceQuit);
    }
}
