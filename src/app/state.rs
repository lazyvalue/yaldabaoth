use std::path::PathBuf;

use sketch::acp_channel::AcpChannelClient;
use sketch::buffer::Buffer;
use sketch::claude_channel::ChannelClient;
use sketch::command::CommandRegistry;
use sketch::config::Config;
use sketch::editor::Editor;
use sketch::file_browser::FileBrowser;
use sketch::keybind::{Action, KeybindManager};
use sketch::menu::{self, MenuNode, MenuState};
use sketch::theme::Theme;
use sketch::view::ViewMode;

use super::merge_menu;

/// A stable editing surface for composing messages in the *claude* buffer.
/// Lives as `Option<ComposeTextbox>` on `App`; `None` = closed.
pub(crate) struct ComposeTextbox {
    /// Standalone editor with its own Document, cursor, and undo stack.
    pub editor: Editor,
    /// The textbox's own modal state (Normal or Insert), independent of App::mode.
    pub mode: AppMode,
}

impl ComposeTextbox {
    pub fn new() -> Self {
        Self {
            editor: Editor::new(String::new(), PathBuf::from("*compose*")),
            mode: AppMode::Insert,
        }
    }

    pub fn text(&self) -> String {
        self.editor.document().full_text()
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum AppScreen {
    Editor,
    FileBrowser { came_from_dropdown: bool },
    BufferList,
}

#[derive(Debug, PartialEq)]
pub(crate) enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
    Outline,
}

pub struct App {
    pub(super) buffers: Vec<Buffer>,
    pub(super) active_buffer: usize,
    pub(super) max_line_width: usize,
    pub(super) theme: Theme,
    pub(super) keybinds: KeybindManager,
    pub(super) registry: CommandRegistry,
    pub(super) should_quit: bool,
    pub(super) screen: AppScreen,
    pub(super) search_query: String,
    pub(super) search_input_mode: bool,
    pub(super) search_input_buffer: String,
    pub(super) search_matches: Vec<(usize, usize)>,
    pub(super) search_match_index: usize,
    pub(super) mode: AppMode,
    pub(super) menu_state: MenuState,
    pub(super) menu_tree: Vec<MenuNode>,
    pub(super) file_browser: Option<FileBrowser>,
    pub(super) command_buffer: String,
    pub(super) command_error: String,
    pub(super) buffer_list_selected: usize,
    pub(super) buffer_list_filter_mode: bool,
    pub(super) buffer_list_filter_text: String,
    pub(super) outline_selected: usize,
    pub(super) outline_filter_mode: bool,
    pub(super) outline_filter_text: String,
    /// Stack of (heading_level, y_offset) for descended headings.
    /// Empty = top-level view. Last entry = current parent.
    pub(super) outline_stack: Vec<(u8, usize)>,
    /// Saved scroll offset to restore if Esc without selecting
    pub(super) outline_saved_scroll: usize,
    pub(super) full_browser_pending_g: bool,
    pub(super) pending_count: Option<usize>,
    /// Set after pressing f/F/t/T — the next keypress is consumed as the
    /// target character and the corresponding find motion is executed.
    pub(super) pending_find_char: Option<Action>,
    /// SKETCH_DEBUG=1 state: dedupe identical frames so the log only grows
    /// when something changes (or when off-screen, which is always logged).
    pub(super) debug_last_off_screen: bool,
    pub(super) debug_last_signature: u64,
    /// Cached viewport height from the most recent input/draw cycle, so helper
    /// methods that don't take it as a parameter (like programmatic edits to
    /// the *claude* buffer) can still scroll the viewport.
    pub(super) last_viewport_height: usize,
    /// Cached raw-mode wrap width (terminal width minus the gutter and
    /// max_line_width cap). Used by visual-row cursor math.
    pub(super) last_wrap_width: usize,
    /// Live MCP channel connection to a `sketch-channel` server. When attached,
    /// `:claude-send` and `:claude-send-selection` push payloads to the server,
    /// which forwards them to Claude Code as `notifications/claude/channel`.
    /// Replies come back via the `reply` MCP tool and are appended to a
    /// `*claude*` buffer.
    pub(super) claude_channel: Option<ChannelClient>,
    /// Alternative path: a Claude (or any ACP-compliant) agent spawned as a
    /// local subprocess and driven over the Agent Client Protocol over stdio.
    /// Coexists with `claude_channel` — both write into the same `*claude*`
    /// buffer; the user picks which one to attach. Replies are streamed in
    /// chunks and spliced via `append_to_claude_buffer`.
    pub(super) acp_channel: Option<AcpChannelClient>,
    /// Last-seen ACP turn count. Compared against the live counter each
    /// `pump_acp_replies` tick — when it advances, the in-flight turn just
    /// ended, so we finalize the buffer (ensure an editable line below the
    /// frozen content).
    pub(super) acp_last_seen_turns: usize,
    /// Compose textbox for the *claude* buffer. When `Some`, key dispatch
    /// and rendering target the textbox instead of the main buffer.
    pub(super) compose_textbox: Option<ComposeTextbox>,
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::from_name(config.theme);
        // Build keybinds from config
        let mut keybinds = if let Some(kb_config) = &config.keybinds {
            if kb_config.reset_defaults {
                KeybindManager::new(
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                )
            } else {
                KeybindManager::default()
            }
        } else {
            KeybindManager::default()
        };
        if let Some(kb_config) = &config.keybinds {
            keybinds.apply_bindings(&kb_config.bindings);
        }

        // Build menu from config
        let menu_tree = if let Some(menu_config) = &config.menu {
            if menu_config.reset_defaults {
                menu_config.nodes.clone()
            } else {
                merge_menu(menu::default_menu(), &menu_config.nodes)
            }
        } else {
            menu::default_menu()
        };

        let registry = CommandRegistry::default_registry();
        let buffer = Buffer::new(filename, markdown, config.max_line_width, &theme);

        Self {
            buffers: vec![buffer],
            active_buffer: 0,
            max_line_width: config.max_line_width,
            theme,
            keybinds,
            registry,
            should_quit: false,
            screen: AppScreen::Editor,
            search_query: String::new(),
            search_input_mode: false,
            search_input_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            mode: AppMode::Normal,
            menu_state: MenuState::new(),
            menu_tree,
            file_browser: None,
            command_buffer: String::new(),
            command_error: String::new(),
            buffer_list_selected: 0,
            buffer_list_filter_mode: false,
            buffer_list_filter_text: String::new(),
            outline_selected: 0,
            outline_filter_mode: false,
            outline_filter_text: String::new(),
            outline_stack: Vec::new(),
            outline_saved_scroll: 0,
            full_browser_pending_g: false,
            pending_count: None,
            pending_find_char: None,
            debug_last_off_screen: false,
            debug_last_signature: 0,
            claude_channel: None,
            acp_channel: None,
            acp_last_seen_turns: 0,
            last_viewport_height: 24,
            last_wrap_width: 80,
            compose_textbox: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn active(&self) -> &Buffer {
        &self.buffers[self.active_buffer]
    }

    #[allow(dead_code)]
    pub(crate) fn active_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_buffer]
    }

    pub(crate) fn effective_content_width(&self, terminal_width: usize) -> usize {
        self.buffers[self.active_buffer]
            .viewport
            .content_width(terminal_width)
    }

    pub(crate) fn ensure_raw_for_editing(&mut self) {
        if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
            self.buffers[self.active_buffer].view_mode = ViewMode::Raw;
        }
    }

    pub(crate) fn current_line_indent(&self) -> String {
        let editor = &self.buffers[self.active_buffer].editor;
        let line = editor.document().line_text(editor.cursor().line);
        let indent_len = line.len() - line.trim_start().len();
        line[..indent_len].replace('\t', "  ")
    }
}
