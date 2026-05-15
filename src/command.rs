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
                name: "find-char-forward".into(),
                aliases: vec![],
                action: Action::FindCharForward,
                description: "Find next character on line (f<char>)".into(),
            },
            CommandDef {
                name: "find-char-backward".into(),
                aliases: vec![],
                action: Action::FindCharBackward,
                description: "Find previous character on line (F<char>)".into(),
            },
            CommandDef {
                name: "till-char-forward".into(),
                aliases: vec![],
                action: Action::TillCharForward,
                description: "Move till next character on line (t<char>)".into(),
            },
            CommandDef {
                name: "till-char-backward".into(),
                aliases: vec![],
                action: Action::TillCharBackward,
                description: "Move till previous character on line (T<char>)".into(),
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
            CommandDef {
                name: "set-heading-1".into(),
                aliases: vec!["h1".into()],
                action: Action::SetHeading1,
                description: "Set current line to H1".into(),
            },
            CommandDef {
                name: "set-heading-2".into(),
                aliases: vec!["h2".into()],
                action: Action::SetHeading2,
                description: "Set current line to H2".into(),
            },
            CommandDef {
                name: "set-heading-3".into(),
                aliases: vec!["h3".into()],
                action: Action::SetHeading3,
                description: "Set current line to H3".into(),
            },
            CommandDef {
                name: "set-heading-4".into(),
                aliases: vec!["h4".into()],
                action: Action::SetHeading4,
                description: "Set current line to H4".into(),
            },
            CommandDef {
                name: "set-heading-5".into(),
                aliases: vec!["h5".into()],
                action: Action::SetHeading5,
                description: "Set current line to H5".into(),
            },
            CommandDef {
                name: "set-heading-6".into(),
                aliases: vec!["h6".into()],
                action: Action::SetHeading6,
                description: "Set current line to H6".into(),
            },
            CommandDef {
                name: "clear-heading".into(),
                aliases: vec!["h0".into()],
                action: Action::ClearHeading,
                description: "Remove heading markers from current line".into(),
            },
            CommandDef {
                name: "set-width".into(),
                aliases: vec!["width".into(), "set-line-width".into()],
                action: Action::SetMaxLineWidth,
                description: "Set max line width (0 = full terminal)".into(),
            },
            // --- Helix-style selection commands ---
            CommandDef {
                name: "delete-selection".into(),
                aliases: vec!["delete".into()],
                action: Action::DeleteSelection,
                description: "Delete current selection (or current line)".into(),
            },
            CommandDef {
                name: "change-selection".into(),
                aliases: vec!["change".into()],
                action: Action::ChangeSelection,
                description: "Delete selection and enter insert mode".into(),
            },
            CommandDef {
                name: "yank-selection".into(),
                aliases: vec!["yank".into()],
                action: Action::YankSelection,
                description: "Yank current selection (or line) to clipboard".into(),
            },
            CommandDef {
                name: "collapse-selection".into(),
                aliases: vec![],
                action: Action::CollapseSelection,
                description: "Collapse selection to cursor".into(),
            },
            CommandDef {
                name: "flip-selection".into(),
                aliases: vec![],
                action: Action::FlipSelection,
                description: "Swap cursor and selection anchor".into(),
            },
            CommandDef {
                name: "select-all".into(),
                aliases: vec![],
                action: Action::SelectAll,
                description: "Select the whole buffer".into(),
            },
            CommandDef {
                name: "extend-line".into(),
                aliases: vec![],
                action: Action::ExtendByLine,
                description: "Extend selection by one line".into(),
            },
            CommandDef {
                name: "toggle-extend-mode".into(),
                aliases: vec!["select-mode".into()],
                action: Action::ToggleExtendMode,
                description: "Toggle extend (select) mode".into(),
            },
            // --- Claude Code channel commands ---
            CommandDef {
                name: "claude-attach".into(),
                aliases: vec!["cattach".into()],
                action: Action::ClaudeAttach,
                description: "Attach to a Claude Code channel (path)".into(),
            },
            CommandDef {
                name: "claude-detach".into(),
                aliases: vec!["cdetach".into()],
                action: Action::ClaudeDetach,
                description: "Detach from the current Claude Code channel".into(),
            },
            CommandDef {
                name: "claude-send".into(),
                aliases: vec!["csend".into()],
                action: Action::ClaudeSend,
                description: "Send the entire buffer to the Claude channel".into(),
            },
            CommandDef {
                name: "claude-send-selection".into(),
                aliases: vec!["csendsel".into()],
                action: Action::ClaudeSendSelection,
                description: "Send the current selection to the Claude channel".into(),
            },
            CommandDef {
                name: "claude-status".into(),
                aliases: vec!["cstatus".into()],
                action: Action::ClaudeStatus,
                description: "Show the current Claude channel attachment".into(),
            },
            CommandDef {
                name: "claude-test".into(),
                aliases: vec!["ctest".into()],
                action: Action::ClaudeTest,
                description: "Inject a fake Claude reply (for diagnostics)".into(),
            },
            // --- Claude Code via ACP commands (spawns local subprocess) ---
            CommandDef {
                name: "claude-acp-attach".into(),
                aliases: vec!["acp-attach".into(), "acpattach".into()],
                action: Action::ClaudeAcpAttach,
                description:
                    "Spawn an ACP agent (default claude-agent-acp; arg = custom cmd)".into(),
            },
            CommandDef {
                name: "claude-acp-detach".into(),
                aliases: vec!["acp-detach".into(), "acpdetach".into()],
                action: Action::ClaudeAcpDetach,
                description: "Kill the ACP agent subprocess".into(),
            },
            CommandDef {
                name: "claude-acp-send".into(),
                aliases: vec!["acp-send".into(), "acpsend".into()],
                action: Action::ClaudeAcpSend,
                description: "Send the buffer / draft to the ACP agent as a prompt".into(),
            },
            CommandDef {
                name: "claude-acp-send-selection".into(),
                aliases: vec!["acp-send-selection".into(), "acpsendsel".into()],
                action: Action::ClaudeAcpSendSelection,
                description: "Send the selection to the ACP agent as a prompt".into(),
            },
            CommandDef {
                name: "claude-acp-status".into(),
                aliases: vec!["acp-status".into(), "acpstatus".into()],
                action: Action::ClaudeAcpStatus,
                description: "Show the current ACP agent attachment".into(),
            },
            // --- Compose textbox commands ---
            CommandDef {
                name: "compose".into(),
                aliases: vec![],
                action: Action::ComposeToggle,
                description: "Toggle compose textbox in *claude* buffer".into(),
            },
            CommandDef {
                name: "compose-send".into(),
                aliases: vec![],
                action: Action::ComposeSend,
                description: "Send compose textbox contents and close".into(),
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
            "find-char-forward", "find-char-backward",
            "till-char-forward", "till-char-backward",
            "insert-mode", "insert-after", "open-line-below", "open-line-above",
            "delete-char", "delete-line", "undo", "redo", "enter-command",
            "save", "quit", "force-quit", "save-quit", "save-as",
            "goto-top", "goto-bottom", "goto-heading", "file-browser",
            "next-buffer", "prev-buffer", "buffer-list", "close-buffer",
            "nav-cycle", "nav-character", "nav-links", "nav-headings",
            "nav-list-items", "nav-code-blocks", "nav-activate",
            "reload", "outline", "file-browser-full",
            "set-heading-1", "set-heading-2", "set-heading-3",
            "set-heading-4", "set-heading-5", "set-heading-6",
            "clear-heading", "set-width",
            "delete-selection", "change-selection", "yank-selection",
            "collapse-selection", "flip-selection", "select-all",
            "extend-line", "toggle-extend-mode",
            "claude-attach", "claude-detach", "claude-send",
            "claude-send-selection", "claude-status", "claude-test",
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
