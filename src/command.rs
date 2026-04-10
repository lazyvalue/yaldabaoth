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

    /// Resolve a full command string (optionally prefixed with ':') into an
    /// Action and optional argument string.
    pub fn resolve(&self, input: &str) -> Option<(Action, Option<String>)> {
        let input = input.strip_prefix(':').unwrap_or(input).trim();
        let (cmd_name, args) = match input.split_once(' ') {
            Some((name, rest)) => (name, Some(rest.to_string())),
            None => (input, None),
        };
        let cmd = self.lookup(cmd_name)?;
        Some((cmd.action, args))
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
                name: "search-forward".into(),
                aliases: vec!["search".into()],
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
                aliases: vec!["next-heading".into()],
                action: Action::NextHeading,
                description: "Next heading".into(),
            },
            // New commands
            CommandDef {
                name: "scroll-down".into(),
                aliases: vec![],
                action: Action::ScrollDown,
                description: "Scroll down".into(),
            },
            CommandDef {
                name: "scroll-up".into(),
                aliases: vec![],
                action: Action::ScrollUp,
                description: "Scroll up".into(),
            },
            CommandDef {
                name: "half-page-down".into(),
                aliases: vec![],
                action: Action::HalfPageDown,
                description: "Scroll half page down".into(),
            },
            CommandDef {
                name: "half-page-up".into(),
                aliases: vec![],
                action: Action::HalfPageUp,
                description: "Scroll half page up".into(),
            },
            CommandDef {
                name: "full-page-down".into(),
                aliases: vec![],
                action: Action::FullPageDown,
                description: "Scroll full page down".into(),
            },
            CommandDef {
                name: "full-page-up".into(),
                aliases: vec![],
                action: Action::FullPageUp,
                description: "Scroll full page up".into(),
            },
            CommandDef {
                name: "prev-heading".into(),
                aliases: vec![],
                action: Action::PrevHeading,
                description: "Previous heading".into(),
            },
            CommandDef {
                name: "next-heading-same-level".into(),
                aliases: vec![],
                action: Action::NextHeadingSameLevel,
                description: "Next heading at same level".into(),
            },
            CommandDef {
                name: "prev-heading-same-level".into(),
                aliases: vec![],
                action: Action::PrevHeadingSameLevel,
                description: "Previous heading at same level".into(),
            },
            CommandDef {
                name: "search-backward".into(),
                aliases: vec![],
                action: Action::SearchBackward,
                description: "Search backward".into(),
            },
            CommandDef {
                name: "search-next".into(),
                aliases: vec![],
                action: Action::SearchNext,
                description: "Next search result".into(),
            },
            CommandDef {
                name: "search-prev".into(),
                aliases: vec![],
                action: Action::SearchPrev,
                description: "Previous search result".into(),
            },
            CommandDef {
                name: "open-link".into(),
                aliases: vec![],
                action: Action::OpenLink,
                description: "Open link under cursor".into(),
            },
            CommandDef {
                name: "yank-line".into(),
                aliases: vec![],
                action: Action::YankLine,
                description: "Yank current line".into(),
            },
            CommandDef {
                name: "open-menu".into(),
                aliases: vec![],
                action: Action::OpenMenu,
                description: "Open command menu".into(),
            },
            CommandDef {
                name: "move-left".into(),
                aliases: vec![],
                action: Action::MoveLeft,
                description: "Move cursor left".into(),
            },
            CommandDef {
                name: "move-right".into(),
                aliases: vec![],
                action: Action::MoveRight,
                description: "Move cursor right".into(),
            },
            CommandDef {
                name: "move-up".into(),
                aliases: vec![],
                action: Action::MoveUp,
                description: "Move cursor up".into(),
            },
            CommandDef {
                name: "move-down".into(),
                aliases: vec![],
                action: Action::MoveDown,
                description: "Move cursor down".into(),
            },
            CommandDef {
                name: "move-word-forward".into(),
                aliases: vec![],
                action: Action::MoveWordForward,
                description: "Move forward by word".into(),
            },
            CommandDef {
                name: "move-word-backward".into(),
                aliases: vec![],
                action: Action::MoveWordBackward,
                description: "Move backward by word".into(),
            },
            CommandDef {
                name: "move-word-end".into(),
                aliases: vec![],
                action: Action::MoveWordEnd,
                description: "Move to end of word".into(),
            },
            CommandDef {
                name: "move-line-start".into(),
                aliases: vec![],
                action: Action::MoveLineStart,
                description: "Move to start of line".into(),
            },
            CommandDef {
                name: "move-line-end".into(),
                aliases: vec![],
                action: Action::MoveLineEnd,
                description: "Move to end of line".into(),
            },
            CommandDef {
                name: "insert-mode".into(),
                aliases: vec![],
                action: Action::InsertMode,
                description: "Enter insert mode".into(),
            },
            CommandDef {
                name: "insert-after".into(),
                aliases: vec![],
                action: Action::InsertAfter,
                description: "Insert after cursor".into(),
            },
            CommandDef {
                name: "open-line-below".into(),
                aliases: vec![],
                action: Action::OpenLineBelow,
                description: "Open new line below".into(),
            },
            CommandDef {
                name: "open-line-above".into(),
                aliases: vec![],
                action: Action::OpenLineAbove,
                description: "Open new line above".into(),
            },
            CommandDef {
                name: "delete-char".into(),
                aliases: vec![],
                action: Action::DeleteChar,
                description: "Delete character under cursor".into(),
            },
            CommandDef {
                name: "delete-line".into(),
                aliases: vec![],
                action: Action::DeleteLine,
                description: "Delete current line".into(),
            },
            CommandDef {
                name: "undo".into(),
                aliases: vec![],
                action: Action::Undo,
                description: "Undo last change".into(),
            },
            CommandDef {
                name: "redo".into(),
                aliases: vec![],
                action: Action::Redo,
                description: "Redo last undone change".into(),
            },
            CommandDef {
                name: "enter-command".into(),
                aliases: vec![],
                action: Action::EnterCommand,
                description: "Enter command mode".into(),
            },
            CommandDef {
                name: "next-buffer".into(),
                aliases: vec!["bn".into()],
                action: Action::NextBuffer,
                description: "Switch to next buffer".into(),
            },
            CommandDef {
                name: "prev-buffer".into(),
                aliases: vec!["bp".into()],
                action: Action::PrevBuffer,
                description: "Switch to previous buffer".into(),
            },
            CommandDef {
                name: "buffer-list".into(),
                aliases: vec!["buffers".into(), "ls".into()],
                action: Action::BufferList,
                description: "Show buffer list".into(),
            },
            CommandDef {
                name: "close-buffer".into(),
                aliases: vec!["bd".into()],
                action: Action::CloseBuffer,
                description: "Close current buffer".into(),
            },
            CommandDef {
                name: "reload".into(),
                aliases: vec!["e".into(), "edit".into()],
                action: Action::Reload,
                description: "Reload current file from disk".into(),
            },
            CommandDef {
                name: "outline".into(),
                aliases: vec!["toc".into()],
                action: Action::Outline,
                description: "Show document outline".into(),
            },
            CommandDef {
                name: "nav-cycle".into(),
                aliases: vec![],
                action: Action::NavCycle,
                description: "Cycle navigation mode".into(),
            },
            CommandDef {
                name: "nav-character".into(),
                aliases: vec![],
                action: Action::NavCharacter,
                description: "Character navigation mode".into(),
            },
            CommandDef {
                name: "nav-links".into(),
                aliases: vec![],
                action: Action::NavLinks,
                description: "Link navigation mode".into(),
            },
            CommandDef {
                name: "nav-headings".into(),
                aliases: vec![],
                action: Action::NavHeadings,
                description: "Heading navigation mode".into(),
            },
            CommandDef {
                name: "nav-list-items".into(),
                aliases: vec![],
                action: Action::NavListItems,
                description: "List item navigation mode".into(),
            },
            CommandDef {
                name: "nav-code-blocks".into(),
                aliases: vec![],
                action: Action::NavCodeBlocks,
                description: "Code block navigation mode".into(),
            },
            CommandDef {
                name: "nav-activate".into(),
                aliases: vec![],
                action: Action::NavActivate,
                description: "Activate selected nav object".into(),
            },
            CommandDef {
                name: "file-browser-full".into(),
                aliases: vec![],
                action: Action::OpenFileBrowserFull,
                description: "Open full-screen file browser".into(),
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

    #[test]
    fn test_all_actions_have_commands() {
        let reg = CommandRegistry::default_registry();
        let expected = vec![
            "scroll-down", "scroll-up", "half-page-down", "half-page-up",
            "full-page-down", "full-page-up", "next-heading", "prev-heading",
            "next-heading-same-level", "prev-heading-same-level",
            "search-forward", "search-backward", "search-next", "search-prev",
            "toggle-view", "open-link", "yank-line", "open-menu",
            "move-left", "move-right", "move-up", "move-down",
            "move-word-forward", "move-word-backward", "move-word-end",
            "move-line-start", "move-line-end",
            "insert-mode", "insert-after", "open-line-below", "open-line-above",
            "delete-char", "delete-line", "undo", "redo", "enter-command",
            "save", "quit", "force-quit", "save-quit", "save-as",
            "goto-top", "goto-bottom", "goto-heading", "file-browser",
            "next-buffer", "prev-buffer", "buffer-list", "close-buffer",
            "nav-cycle", "nav-character", "nav-links", "nav-headings",
            "nav-list-items", "nav-code-blocks", "nav-activate",
            "reload", "outline", "file-browser-full",
        ];
        for name in &expected {
            assert!(reg.lookup(name).is_some(), "missing command: {}", name);
        }
    }

    #[test]
    fn test_resolve_bare_command() {
        let reg = CommandRegistry::default_registry();
        let (action, args) = reg.resolve("save").unwrap();
        assert_eq!(action, Action::Save);
        assert!(args.is_none());
    }

    #[test]
    fn test_resolve_command_with_args() {
        let reg = CommandRegistry::default_registry();
        let (action, args) = reg.resolve("goto-heading 2").unwrap();
        assert_eq!(action, Action::NextHeading);
        assert_eq!(args.as_deref(), Some("2"));
    }

    #[test]
    fn test_resolve_colon_prefix_stripped() {
        let reg = CommandRegistry::default_registry();
        let (action, args) = reg.resolve(":goto-heading 2").unwrap();
        assert_eq!(action, Action::NextHeading);
        assert_eq!(args.as_deref(), Some("2"));
    }

    #[test]
    fn test_resolve_alias() {
        let reg = CommandRegistry::default_registry();
        let (action, args) = reg.resolve("w").unwrap();
        assert_eq!(action, Action::Save);
        assert!(args.is_none());
    }

    #[test]
    fn test_resolve_unknown() {
        let reg = CommandRegistry::default_registry();
        assert!(reg.resolve("nonexistent").is_none());
    }
}
