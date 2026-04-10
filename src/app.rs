use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent};
use ratatui::DefaultTerminal;

use sketch::blocks::RenderedBlock;
use sketch::buffer::Buffer;
use sketch::command::CommandRegistry;
use sketch::config::Config;
use sketch::file_browser::FileBrowser;
use sketch::keybind::{Action, KeybindManager};
use sketch::menu::{self, MenuNode, MenuState};
use sketch::theme::Theme;
use sketch::buffer::NavMode;
use sketch::view::{self, ViewMode, ViewState};

#[derive(Debug, PartialEq)]
enum AppScreen {
    Editor,
    FileBrowser { came_from_dropdown: bool },
}

#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
    BufferList,
    Outline,
}

pub struct App {
    buffers: Vec<Buffer>,
    active_buffer: usize,
    max_line_width: usize,
    theme: Theme,
    keybinds: KeybindManager,
    registry: CommandRegistry,
    should_quit: bool,
    screen: AppScreen,
    search_query: String,
    search_input_mode: bool,
    search_input_buffer: String,
    search_matches: Vec<(usize, usize)>,
    search_match_index: usize,
    mode: AppMode,
    menu_state: MenuState,
    menu_tree: Vec<MenuNode>,
    file_browser: Option<FileBrowser>,
    command_buffer: String,
    command_error: String,
    buffer_list_selected: usize,
    buffer_list_filter_mode: bool,
    buffer_list_filter_text: String,
    outline_selected: usize,
    outline_filter_mode: bool,
    outline_filter_text: String,
    /// The heading level currently being viewed (None = show all top-level)
    outline_parent_level: Option<u8>,
    /// Y offset of the parent heading (to scope children)
    outline_parent_y: Option<usize>,
    /// Saved scroll offset to restore if Esc without selecting
    outline_saved_scroll: usize,
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
            outline_parent_level: None,
            outline_parent_y: None,
            outline_saved_scroll: 0,
        }
    }

    #[allow(dead_code)]
    fn active(&self) -> &Buffer {
        &self.buffers[self.active_buffer]
    }

    #[allow(dead_code)]
    fn active_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_buffer]
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let content_width = self.effective_content_width(size.width as usize);
        self.buffers[self.active_buffer].rebuild_render_cache(&self.theme);
        self.buffers[self.active_buffer].update_total_lines(content_width);

        loop {
            // Rebuild render cache if needed
            if self.buffers[self.active_buffer].view_cache_dirty {
                self.buffers[self.active_buffer].rebuild_render_cache(&self.theme);
                self.buffers[self.active_buffer].view_cache_dirty = false;
            }

            // Build raw lines for raw mode (cheap — just reads from rope)
            let raw_lines: Vec<String> = if self.buffers[self.active_buffer].view_mode == ViewMode::Raw {
                let text = self.buffers[self.active_buffer].editor.document().full_text();
                text.lines().map(|l| l.to_string()).collect()
            } else {
                Vec::new()
            };

            terminal.draw(|frame| {
                let menu_nodes: Vec<(String, String, sketch::menu::MenuNodeKind)> = if self.menu_state.is_active() {
                    self.menu_state
                        .current_nodes(&self.menu_tree)
                        .iter()
                        .map(|n| {
                            let key_display = sketch::keys::format_key_sequence(&n.key);
                            (key_display, n.label.clone(), n.kind())
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let (fb_open, fb_dir, fb_entries, fb_filter_mode, fb_filter_text) =
                    if let Some(browser) = &self.file_browser {
                        let entries: Vec<(String, bool, bool)> = browser
                            .visible_entries()
                            .iter()
                            .enumerate()
                            .map(|(i, e)| (e.name.clone(), e.is_dir, i == browser.selected()))
                            .collect();
                        (
                            true,
                            browser.current_dir().display().to_string(),
                            entries,
                            browser.filter_mode,
                            browser.filter_text().to_string(),
                        )
                    } else {
                        (false, String::new(), Vec::new(), false, String::new())
                    };

                let buffer_list_entries: Vec<(String, bool, bool, bool)> = if self.mode == AppMode::BufferList {
                    let filtered = self.filtered_buffer_indices();
                    filtered.iter().enumerate().map(|(i, &buf_idx)| {
                        let path = self.buffers[buf_idx].file_path().display().to_string();
                        let modified = self.buffers[buf_idx].editor.document().is_modified();
                        let is_active = buf_idx == self.active_buffer;
                        let selected = i == self.buffer_list_selected;
                        (path, modified, is_active, selected)
                    }).collect()
                } else {
                    Vec::new()
                };

                let full_browser_state = if let AppScreen::FileBrowser { came_from_dropdown } = self.screen {
                    if let Some(browser) = &self.file_browser {
                        let entries: Vec<view::FullBrowserEntry> = browser
                            .visible_entries()
                            .iter()
                            .enumerate()
                            .map(|(i, e)| view::FullBrowserEntry {
                                name: e.name.clone(),
                                is_dir: e.is_dir,
                                is_selected: i == browser.selected(),
                                size: e.size,
                                modified: e.modified,
                            })
                            .collect();
                        Some(view::FullBrowserViewState {
                            dir: browser.current_dir().display().to_string(),
                            entries,
                            filter_mode: browser.filter_mode,
                            filter_text: browser.filter_text().to_string(),
                            came_from_dropdown,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                let buf = &self.buffers[self.active_buffer];
                let filename_display = buf.editor.document().file_path.display().to_string();

                let state = ViewState {
                    filename: &filename_display,
                    modified: buf.editor.document().is_modified(),
                    view_mode: buf.view_mode,
                    rendered_blocks: &buf.rendered_cache,
                    raw_lines: &raw_lines,
                    viewport: &buf.viewport,
                    theme: &self.theme,
                    mode_label: match self.mode {
                        AppMode::Normal => match buf.view_mode {
                            ViewMode::Rendered => "NORMAL",
                            ViewMode::Raw => "RAW",
                        },
                        AppMode::Insert => "INSERT",
                        AppMode::Command => "NORMAL",
                        AppMode::Menu => "NORMAL",
                        AppMode::FileBrowser => "NORMAL",
                        AppMode::BufferList => "BUFFERS",
                        AppMode::Outline => "OUTLINE",
                    },
                    cursor_line: buf.editor.cursor().line,
                    cursor_col: buf.editor.cursor().col,
                    show_block_cursor: self.mode != AppMode::Insert,
                    search_query: &self.search_query,
                    search_input_mode: self.search_input_mode,
                    search_input_buffer: &self.search_input_buffer,
                    search_match_count: self.search_matches.len(),
                    search_matches: &self.search_matches,
                    search_current_match: self.search_match_index,
                    rendered_cursor_row: self.buffers[self.active_buffer].rendered_cursor_row,
                    rendered_cursor_col: self.buffers[self.active_buffer].rendered_cursor_col,
                    menu_active: self.menu_state.is_active(),
                    menu_nodes,
                    menu_label: self.menu_state.current_label(&self.menu_tree),
                    file_browser_open: fb_open,
                    file_browser_dir: fb_dir,
                    file_browser_entries: fb_entries,
                    file_browser_filter_mode: fb_filter_mode,
                    file_browser_filter_text: fb_filter_text,
                    command_mode: self.mode == AppMode::Command,
                    command_buffer: &self.command_buffer,
                    command_error: &self.command_error,
                    buffer_list_open: self.mode == AppMode::BufferList,
                    buffer_list_entries,
                    buffer_list_filter_mode: self.buffer_list_filter_mode,
                    buffer_list_filter_text: self.buffer_list_filter_text.clone(),
                    buffer_count: self.buffers.len(),
                    active_buffer_index: self.active_buffer,
                    outline_open: self.mode == AppMode::Outline,
                    outline_entries: if self.mode == AppMode::Outline {
                        let entries = self.filtered_outline_entries();
                        entries.iter().enumerate().map(|(i, e)| {
                            (e.title.clone(), e.level, i == self.outline_selected)
                        }).collect()
                    } else {
                        Vec::new()
                    },
                    outline_filter_mode: self.outline_filter_mode,
                    outline_filter_text: self.outline_filter_text.clone(),
                    outline_breadcrumb: if self.outline_parent_level.is_some() {
                        // Find parent title
                        if let Some(parent_y) = self.outline_parent_y {
                            self.outline_entries().iter()
                                .find(|e| e.y_offset == parent_y)
                                .map(|e| e.title.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    },
                    nav_mode_label: self.buffers[self.active_buffer].nav_mode.label()
                        .map(|s| s.to_string()),
                    nav_highlight: {
                        let buf = &self.buffers[self.active_buffer];
                        if buf.nav_mode != NavMode::Character {
                            buf.nav_objects.get(buf.nav_object_index).map(|obj| {
                                (obj.rendered_row, obj.col_start, obj.col_end)
                            })
                        } else {
                            None
                        }
                    },
                    full_browser: full_browser_state,
                };
                view::draw(frame, &state);
            })?;

            if self.should_quit {
                break;
            }

            let timeout = Duration::from_millis(100);

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key_event) => self.handle_key(key_event, terminal)?,
                    Event::Resize(w, _h) => {
                        let cw = self.effective_content_width(w as usize);
                        self.buffers[self.active_buffer].update_total_lines(cw);
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }


    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
        if !self.command_error.is_empty() && self.mode != AppMode::Command {
            self.command_error.clear();
        }

        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2);
        let content_width = self.effective_content_width(size.width as usize);

        if let AppScreen::FileBrowser { .. } = self.screen {
            self.handle_full_browser_key(key, size.width, viewport_height, content_width);
            self.buffers[self.active_buffer].update_total_lines(content_width);
            return Ok(());
        }

        match self.mode {
            AppMode::Normal => self.handle_normal_key(key, viewport_height, content_width),
            AppMode::Insert => self.handle_insert_key(key, viewport_height),
            AppMode::Command => self.handle_command_key(key),
            AppMode::Menu => self.handle_menu_key(key, viewport_height, content_width),
            AppMode::FileBrowser => {
                self.handle_file_browser_key(key, size.width, viewport_height, content_width)
            }
            AppMode::BufferList => self.handle_buffer_list_key(key, viewport_height, content_width),
            AppMode::Outline => self.handle_outline_key(key, viewport_height, content_width),
        }

        self.buffers[self.active_buffer].update_total_lines(content_width);
        Ok(())
    }

    fn handle_normal_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        if self.search_input_mode {
            match key.code {
                KeyCode::Enter => {
                    self.search_query = self.search_input_buffer.clone();
                    self.search_input_mode = false;
                    self.perform_search();
                    self.jump_to_match(viewport_height);
                }
                KeyCode::Esc => {
                    self.search_input_mode = false;
                    self.search_input_buffer.clear();
                }
                KeyCode::Backspace => {
                    self.search_input_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.search_input_buffer.push(c);
                }
                _ => {}
            }
            return;
        }

        if key.code == KeyCode::Esc
            && self.buffers[self.active_buffer].nav_mode != NavMode::Character
            && self.buffers[self.active_buffer].view_mode == ViewMode::Rendered
        {
            self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            return;
        }

        if let Some(cmd_string) = self.keybinds.process_key(key) {
            self.dispatch_command(&cmd_string, viewport_height, content_width);
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent, viewport_height: usize) {
        match key.code {
            KeyCode::Esc => {
                self.buffers[self.active_buffer].editor.end_insert();
                self.mode = AppMode::Normal;
                self.buffers[self.active_buffer].view_cache_dirty = true;
                if self.buffers[self.active_buffer].editor.cursor().col > 0 {
                    self.buffers[self.active_buffer].editor.cursor_mut().move_left();
                }
            }
            KeyCode::Enter => {
                self.buffers[self.active_buffer].editor.insert_char('\n');
            }
            KeyCode::Backspace => {
                self.buffers[self.active_buffer].editor.backspace();
            }
            KeyCode::Char(c) => {
                self.buffers[self.active_buffer].editor.insert_char(c);
            }
            KeyCode::Left => {
                self.buffers[self.active_buffer].editor.cursor_mut().move_left();
            }
            KeyCode::Right => {
                self.buffers[self.active_buffer].editor.move_right_clamped(true);
            }
            KeyCode::Up => {
                self.buffers[self.active_buffer].editor.cursor_mut().move_up();
                self.buffers[self.active_buffer].editor.clamp_cursor_col(true);
            }
            KeyCode::Down => {
                self.buffers[self.active_buffer].editor.move_down(true);
            }
            _ => {}
        }
        self.ensure_cursor_visible(viewport_height);
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                let cmd = self.command_buffer.clone();
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
                self.execute_command(&cmd);
            }
            KeyCode::Backspace => {
                self.command_buffer.pop();
                if self.command_buffer.is_empty() {
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            }
            _ => {}
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        match key.code {
            KeyCode::Esc => {
                self.menu_state.handle_escape();
                if !self.menu_state.is_active() {
                    self.mode = AppMode::Normal;
                }
            }
            _ => {
                if let Some(cmd_string) = self.menu_state.process_key_event(key, &self.menu_tree) {
                    self.mode = AppMode::Normal;
                    self.dispatch_command(&cmd_string, viewport_height, content_width);
                }
            }
        }
    }

    fn handle_file_browser_key(
        &mut self,
        key: KeyEvent,
        term_width: u16,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        let _ = term_width;

        let browser = match &mut self.file_browser {
            Some(b) => b,
            None => {
                self.mode = AppMode::Normal;
                return;
            }
        };

        if browser.filter_mode {
            match key.code {
                KeyCode::Esc => {
                    // Exit file browser entirely
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                    return;
                }
                KeyCode::Enter => {
                    let count = browser.visible_entries().len();
                    if count == 1 {
                        // Single result — open it directly
                        if let Some(path) = browser.enter_selected()
                            && self.open_buffer(path)
                        {
                            self.file_browser = None;
                            self.mode = AppMode::Normal;
                        }
                    } else if count > 0 {
                        // Multiple results — exit filter mode, navigate the list
                        browser.filter_mode = false;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    let mut text = browser.filter_text().to_string();
                    text.pop();
                    browser.set_filter(&text);
                }
                KeyCode::Char(c) => {
                    let mut text = browser.filter_text().to_string();
                    text.push(c);
                    browser.set_filter(&text);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('j') => browser.move_down(),
            KeyCode::Char('k') => browser.move_up(),
            KeyCode::Char('l') | KeyCode::Char(' ') => {
                if let Some(path) = browser.enter_selected()
                    && self.open_buffer(path)
                {
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('h') | KeyCode::Backspace => browser.go_parent(),
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Char('/') => browser.filter_mode = true,
            KeyCode::Char('q') | KeyCode::Esc => {
                self.file_browser = None;
                self.mode = AppMode::Normal;
            }
            KeyCode::Tab => {
                self.open_file_browser_full(true);
            }
            _ => {}
        }
    }

    fn handle_full_browser_key(
        &mut self,
        key: KeyEvent,
        _term_width: u16,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        // Close on q/Esc for now — full implementation in Task 5
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.close_full_browser();
            }
            _ => {}
        }
    }

    fn close_full_browser(&mut self) {
        match self.screen {
            AppScreen::FileBrowser { came_from_dropdown: true } => {
                self.screen = AppScreen::Editor;
                // file_browser stays Some, mode stays FileBrowser for dropdown
                self.mode = AppMode::FileBrowser;
            }
            AppScreen::FileBrowser { came_from_dropdown: false } => {
                self.screen = AppScreen::Editor;
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    /// Look up a command name in the registry and dispatch its action.
    fn dispatch_command(
        &mut self,
        cmd_input: &str,
        viewport_height: usize,
        content_width: usize,
    ) {
        if let Some((action, _args)) = self.registry.resolve(cmd_input) {
            self.execute_action(action, viewport_height, content_width);
        } else {
            self.command_error = format!("Unknown command: {}", cmd_input);
        }
    }

    /// Auto-switch to Raw mode if we're in Rendered mode and about to edit.
    fn ensure_raw_for_editing(&mut self) {
        if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
            self.buffers[self.active_buffer].view_mode = ViewMode::Raw;
        }
    }

    fn execute_action(&mut self, action: Action, viewport_height: usize, content_width: usize) {
        match action {
            Action::Quit => {
                if self.buffers[self.active_buffer].editor.document().is_modified() {
                    self.command_error =
                        "No write since last change (add ! to override)".to_string();
                } else {
                    self.should_quit = true;
                }
            }
            Action::MoveDown => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_next();
                    } else {
                        let total = self.buffers[self.active_buffer].viewport.total_lines;
                        if self.buffers[self.active_buffer].rendered_cursor_row + 1 < total {
                            self.buffers[self.active_buffer].rendered_cursor_row += 1;
                        }
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.move_down(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
            Action::MoveUp => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_prev();
                    } else {
                        self.buffers[self.active_buffer].rendered_cursor_row =
                            self.buffers[self.active_buffer].rendered_cursor_row.saturating_sub(1);
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.cursor_mut().move_up();
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
            Action::MoveLeft | Action::MoveRight
            | Action::MoveWordForward | Action::MoveWordBackward | Action::MoveWordEnd
            | Action::MoveLineStart | Action::MoveLineEnd => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let nav = self.buffers[self.active_buffer].nav_mode;
                    if nav == NavMode::Link || nav == NavMode::ListItem {
                        match action {
                            Action::MoveLeft => self.nav_move_prev(),
                            Action::MoveRight => self.nav_move_next(),
                            _ => {}
                        }
                        self.ensure_rendered_cursor_visible(viewport_height);
                    } else if nav == NavMode::Character {
                        match action {
                            Action::MoveLeft => {
                                self.buffers[self.active_buffer].rendered_cursor_col =
                                    self.buffers[self.active_buffer].rendered_cursor_col.saturating_sub(1);
                            }
                            Action::MoveRight => {
                                self.buffers[self.active_buffer].rendered_cursor_col += 1;
                            }
                            Action::MoveLineStart => {
                                self.buffers[self.active_buffer].rendered_cursor_col = 0;
                            }
                            _ => {}
                        }
                    }
                    // Heading and CodeBlock modes: h/l are no-ops
                } else {
                    match action {
                        Action::MoveLeft => self.buffers[self.active_buffer].editor.cursor_mut().move_left(),
                        Action::MoveRight => self.buffers[self.active_buffer].editor.move_right_clamped(false),
                        Action::MoveWordForward => {
                            self.buffers[self.active_buffer].editor.move_cursor_word_forward();
                            self.ensure_cursor_visible(viewport_height);
                        }
                        Action::MoveWordBackward => {
                            self.buffers[self.active_buffer].editor.move_cursor_word_backward();
                            self.ensure_cursor_visible(viewport_height);
                        }
                        Action::MoveWordEnd => self.buffers[self.active_buffer].editor.move_cursor_word_end(),
                        Action::MoveLineStart => self.buffers[self.active_buffer].editor.cursor_mut().move_line_start(),
                        Action::MoveLineEnd => self.buffers[self.active_buffer].editor.move_cursor_line_end(false),
                        _ => {}
                    }
                }
            }
            Action::InsertMode => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.begin_insert();
                self.mode = AppMode::Insert;
            }
            Action::InsertAfter => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.move_right_clamped(true);
                self.buffers[self.active_buffer].editor.begin_insert();
                self.mode = AppMode::Insert;
            }
            Action::OpenLineBelow => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.open_line_below();
                self.mode = AppMode::Insert;
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::OpenLineAbove => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.open_line_above();
                self.mode = AppMode::Insert;
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::DeleteChar => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.delete_char_at_cursor();
                self.buffers[self.active_buffer].view_cache_dirty = true;
            }
            Action::DeleteLine => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.delete_current_line();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Undo => {
                self.buffers[self.active_buffer].editor.undo();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Redo => {
                self.buffers[self.active_buffer].editor.redo();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::EnterCommand => {
                self.command_buffer.clear();
                self.command_error.clear();
                self.mode = AppMode::Command;
            }
            Action::ScrollDown => {
                self.buffers[self.active_buffer].viewport.scroll_down(1, viewport_height);
            }
            Action::ScrollUp => {
                self.buffers[self.active_buffer].viewport.scroll_up(1);
            }
            Action::HalfPageDown => {
                self.buffers[self.active_buffer].viewport
                    .scroll_down(viewport_height / 2, viewport_height);
                let new_top = self.buffers[self.active_buffer].viewport.scroll_offset;
                if self.buffers[self.active_buffer].editor.cursor().line < new_top {
                    self.buffers[self.active_buffer].editor.cursor_mut().line = new_top;
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                }
            }
            Action::HalfPageUp => {
                self.buffers[self.active_buffer].viewport.scroll_up(viewport_height / 2);
                let new_bottom = self.buffers[self.active_buffer].viewport.scroll_offset + viewport_height;
                if self.buffers[self.active_buffer].editor.cursor().line >= new_bottom {
                    self.buffers[self.active_buffer].editor.cursor_mut().line = new_bottom.saturating_sub(1);
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                }
            }
            Action::FullPageDown => {
                self.buffers[self.active_buffer].viewport.scroll_down(viewport_height, viewport_height);
                let new_top = self.buffers[self.active_buffer].viewport.scroll_offset;
                if self.buffers[self.active_buffer].editor.cursor().line < new_top {
                    self.buffers[self.active_buffer].editor.cursor_mut().line = new_top;
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                }
            }
            Action::FullPageUp => {
                self.buffers[self.active_buffer].viewport.scroll_up(viewport_height);
                let new_bottom = self.buffers[self.active_buffer].viewport.scroll_offset + viewport_height;
                if self.buffers[self.active_buffer].editor.cursor().line >= new_bottom {
                    self.buffers[self.active_buffer].editor.cursor_mut().line = new_bottom.saturating_sub(1);
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                }
            }
            Action::JumpTop => {
                self.buffers[self.active_buffer].editor.cursor_mut().jump_top();
                self.buffers[self.active_buffer].viewport.jump_top();
            }
            Action::JumpBottom => {
                self.buffers[self.active_buffer].editor.jump_cursor_bottom();
                self.buffers[self.active_buffer].viewport.jump_bottom(viewport_height);
            }
            Action::NextHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if let Some(y) = self.find_next_heading(None) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::PrevHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if let Some(y) = self.find_prev_heading(None) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::NextHeadingSameLevel => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let current_level = self.heading_level_at_offset();
                    if let Some(y) = self.find_next_heading(current_level) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::PrevHeadingSameLevel => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let current_level = self.heading_level_at_offset();
                    if let Some(y) = self.find_prev_heading(current_level) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::SearchForward | Action::SearchBackward => {
                self.search_input_mode = true;
                self.search_input_buffer.clear();
            }
            Action::SearchNext => {
                if !self.search_matches.is_empty() {
                    self.search_match_index =
                        (self.search_match_index + 1) % self.search_matches.len();
                    self.jump_to_match(viewport_height);
                }
            }
            Action::SearchPrev => {
                if !self.search_matches.is_empty() {
                    self.search_match_index = if self.search_match_index == 0 {
                        self.search_matches.len() - 1
                    } else {
                        self.search_match_index - 1
                    };
                    self.jump_to_match(viewport_height);
                }
            }
            Action::OpenLink => {
                if let Some(url) = self.link_under_rendered_cursor() {
                    self.open_link(&url);
                }
            }
            Action::YankLine => {
                let buf = &self.buffers[self.active_buffer];
                let line_text = buf.editor.document().line_text(buf.editor.cursor().line);
                let text = line_text.trim_end_matches('\n');
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
                    && let Some(mut stdin) = child.stdin.take()
                {
                    let _ = stdin.write_all(text.as_bytes());
                }
            }
            Action::OpenMenu => {
                self.menu_state.open();
                self.mode = AppMode::Menu;
            }
            Action::OpenFileBrowser => {
                self.open_file_browser();
            }
            Action::Save => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                }
            }
            Action::SaveAs => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                }
            }
            Action::ForceQuit => {
                self.should_quit = true;
            }
            Action::SaveQuit => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                } else {
                    self.should_quit = true;
                }
            }
            Action::ToggleView => {
                let buf = &mut self.buffers[self.active_buffer];
                buf.view_mode = match buf.view_mode {
                    ViewMode::Rendered => ViewMode::Raw,
                    ViewMode::Raw => ViewMode::Rendered,
                };
                buf.view_cache_dirty = true;
            }
            Action::NextBuffer => {
                if self.buffers.len() > 1 {
                    self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
                }
            }
            Action::PrevBuffer => {
                if self.buffers.len() > 1 {
                    self.active_buffer = if self.active_buffer == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.active_buffer - 1
                    };
                }
            }
            Action::BufferList => {
                self.buffer_list_selected = self.active_buffer;
                self.buffer_list_filter_mode = false;
                self.buffer_list_filter_text.clear();
                self.mode = AppMode::BufferList;
            }
            Action::CloseBuffer => {
                self.close_current_buffer();
            }
            Action::Reload => {
                self.reload_current_buffer();
            }
            Action::Outline => {
                self.outline_filter_mode = false;
                self.outline_filter_text.clear();
                self.outline_parent_level = None;
                self.outline_parent_y = None;
                self.outline_saved_scroll =
                    self.buffers[self.active_buffer].viewport.scroll_offset;
                self.mode = AppMode::Outline;
                // Select the heading closest to current scroll position
                let scroll = self.buffers[self.active_buffer].viewport.scroll_offset;
                let entries = self.filtered_outline_entries();
                self.outline_selected = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.y_offset <= scroll)
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.scroll_to_outline_entry();
            }
            Action::OpenFileBrowserFull => {
                self.open_file_browser_full(false);
            }
            Action::None
            | Action::FileBrowserDown
            | Action::FileBrowserUp
            | Action::FileBrowserEnter
            | Action::FileBrowserParentDir
            | Action::FileBrowserFilter
            | Action::FileBrowserClose => {}
            Action::NavCycle => {
                let current = self.buffers[self.active_buffer].nav_mode;
                let next = current.next();
                self.enter_nav_mode(next, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCharacter => {
                self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            }
            Action::NavLinks => {
                self.enter_nav_mode(NavMode::Link, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavHeadings => {
                self.enter_nav_mode(NavMode::Heading, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavListItems => {
                self.enter_nav_mode(NavMode::ListItem, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCodeBlocks => {
                self.enter_nav_mode(NavMode::CodeBlock, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavActivate => {
                self.nav_activate();
            }
        }
    }

    fn execute_command(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
        let cmd_name = parts[0];

        // Special case: `:w filename` → save-as with filename
        if cmd_name == "w" && parts.len() > 1 {
            let path = std::path::Path::new(parts[1]);
            if let Err(e) = self.buffers[self.active_buffer].editor.save_to(path) {
                self.command_error = format!("Error writing file: {}", e);
            }
            return;
        }

        // Look up in registry by name or alias
        if let Some(cmd_def) = self.registry.lookup(cmd_name) {
            let action = cmd_def.action;
            // Use a reasonable viewport height for dispatching
            self.execute_action(action, 40, 80);
        } else {
            self.command_error = format!("Not an editor command: {}", cmd);
        }
    }

    fn enter_nav_mode(&mut self, mode: NavMode, content_width: usize) {
        let buf = &mut self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return;
        }
        buf.nav_mode = mode;
        if mode == NavMode::Character {
            return;
        }
        buf.rebuild_nav_objects(&self.theme, content_width);
        let current_row = buf.rendered_cursor_row;
        if let Some(idx) = buf.nearest_object_index(current_row) {
            buf.nav_object_index = idx;
            let obj = &buf.nav_objects[idx];
            buf.rendered_cursor_row = obj.rendered_row;
            buf.rendered_cursor_col = obj.col_start;
        }
    }

    fn nav_activate(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered || buf.nav_mode == NavMode::Character {
            return;
        }
        let obj = match buf.nav_objects.get(buf.nav_object_index) {
            Some(o) => o.clone(),
            None => return,
        };
        match obj.kind {
            NavMode::Link => {
                self.open_link(&obj.action_data);
            }
            NavMode::Heading => {
                let buf = &mut self.buffers[self.active_buffer];
                buf.rendered_cursor_row = obj.rendered_row;
                buf.nav_mode = NavMode::Character;
            }
            NavMode::CodeBlock => {
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(obj.action_data.as_bytes());
                    }
                }
            }
            NavMode::ListItem => {
                // No-op for now
            }
            NavMode::Character => {}
        }
    }

    fn nav_move_next(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let next_pos = (pos + 1) % filtered.len();
        let next_idx = filtered[next_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = next_idx;
        let obj = &buf.nav_objects[next_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    fn nav_move_prev(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let prev_pos = if pos == 0 { filtered.len() - 1 } else { pos - 1 };
        let prev_idx = filtered[prev_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = prev_idx;
        let obj = &buf.nav_objects[prev_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    fn open_link(&mut self, url: &str) {
        // Check if it's a local file path (relative or absolute, no scheme)
        let is_url = url.contains("://");
        if !is_url {
            // Resolve relative to the current file's directory
            let base_dir = self.buffers[self.active_buffer]
                .file_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let path = base_dir.join(url);
            if path.exists() {
                self.open_buffer(path);
                return;
            }
        }
        // External URL or file not found — open with system handler
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    /// Find the link URL under the rendered cursor, if any.
    fn link_under_rendered_cursor(&self) -> Option<String> {
        let buf = &self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return None;
        }
        let content_width = buf.viewport.content_width(200);
        let target_row = buf.rendered_cursor_row;
        let target_col = buf.rendered_cursor_col;

        let mut row = 0;
        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if row + h > target_row {
                // Target is in this block — get rendered lines
                let lines = sketch::view::render_block_to_lines(block, content_width, &self.theme);
                let line_idx = target_row - row;
                if let Some(line) = lines.get(line_idx) {
                    // Walk spans to find which one the cursor col falls in
                    let mut col = 0;
                    for span in &line.spans {
                        let span_len = span.text.chars().count();
                        if target_col >= col && target_col < col + span_len {
                            return span.link.clone();
                        }
                        col += span_len;
                    }
                }
                return None;
            }
            row += h;
        }
        None
    }

    fn ensure_rendered_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        let row = buf.rendered_cursor_row;
        self.buffers[self.active_buffer].viewport
            .ensure_cursor_visible(row, viewport_height);
    }

    fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        let cursor_line = buf.editor.cursor().line;
        let rendered_y = match buf.view_mode {
            ViewMode::Raw => cursor_line,
            ViewMode::Rendered => self.doc_line_to_rendered_y(cursor_line),
        };
        self.buffers[self.active_buffer].viewport
            .ensure_cursor_visible(rendered_y, viewport_height);
    }

    /// Convert a document line number to its approximate rendered y position
    /// using the cached rendered blocks.
    fn doc_line_to_rendered_y(&self, doc_line: usize) -> usize {
        let buf = &self.buffers[self.active_buffer];
        let boundaries = buf.editor.block_boundaries();
        let content_width = buf.viewport.content_width(200); // approximate

        let mut rendered_y = 0;
        for (i, block_info) in boundaries.iter().enumerate() {
            if doc_line >= block_info.start_line && doc_line <= block_info.end_line {
                let line_in_block = doc_line - block_info.start_line;
                return rendered_y + line_in_block;
            }
            if let Some(rb) = buf.rendered_cache.get(i) {
                rendered_y += buf.viewport.block_height(rb, content_width);
            } else {
                rendered_y += (block_info.end_line - block_info.start_line).max(1) + 1;
            }
        }
        rendered_y
    }

    fn open_file_browser(&mut self) {
        let dir = std::env::current_dir().unwrap_or_default();
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }

    fn open_file_browser_full(&mut self, came_from_dropdown: bool) {
        if self.file_browser.is_none() {
            let dir = std::env::current_dir().unwrap_or_default();
            self.file_browser = Some(FileBrowser::new(dir));
        }
        self.screen = AppScreen::FileBrowser { came_from_dropdown };
    }

    fn open_buffer(&mut self, path: std::path::PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        // Check if already open
        for (i, buf) in self.buffers.iter().enumerate() {
            let buf_path = buf.file_path().canonicalize()
                .unwrap_or_else(|_| buf.file_path().to_path_buf());
            if buf_path == canonical {
                self.active_buffer = i;
                return true;
            }
        }
        // Open new
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    let buffer = Buffer::new(
                        canonical.display().to_string(),
                        content,
                        self.max_line_width,
                        &self.theme,
                    );
                    self.buffers.push(buffer);
                    self.active_buffer = self.buffers.len() - 1;
                    true
                }
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    fn effective_content_width(&self, terminal_width: usize) -> usize {
        self.buffers[self.active_buffer].viewport.content_width(terminal_width)
    }

    fn perform_search(&mut self) {
        self.search_matches.clear();
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            return;
        }

        let buf = &self.buffers[self.active_buffer];
        let line_count = buf.editor.document().line_count();
        for line_idx in 0..line_count {
            let line_text = buf.editor.document().line_text(line_idx);
            let line_lower = line_text.to_lowercase();
            let mut start = 0;
            while let Some(pos) = line_lower[start..].find(&query) {
                self.search_matches.push((line_idx, start + pos));
                start += pos + query.len();
            }
        }

        // Jump to the first match at or after the current cursor position
        let buf = &self.buffers[self.active_buffer];
        let cursor_line = buf.editor.cursor().line;
        let cursor_col = buf.editor.cursor().col;
        self.search_match_index = self
            .search_matches
            .iter()
            .position(|&(line, col)| line > cursor_line || (line == cursor_line && col >= cursor_col))
            .unwrap_or(0);
    }

    fn jump_to_match(&mut self, viewport_height: usize) {
        if let Some(&(line_idx, col_idx)) = self.search_matches.get(self.search_match_index) {
            let buf = &mut self.buffers[self.active_buffer];
            buf.editor.cursor_mut().line = line_idx;
            buf.editor.cursor_mut().col = col_idx;
            if buf.view_mode == ViewMode::Rendered {
                // Find rendered position by counting matches (same as view does)
                if let Some((row, col)) = self.find_rendered_match_position(self.search_match_index) {
                    self.buffers[self.active_buffer].rendered_cursor_row = row;
                    self.buffers[self.active_buffer].rendered_cursor_col = col;
                }
                self.ensure_rendered_cursor_visible(viewport_height);
            } else {
                self.ensure_cursor_visible(viewport_height);
            }
        }
    }

    /// Find the rendered (row, col) of the Nth search match by scanning
    /// rendered lines the same way the view does.
    fn find_rendered_match_position(&self, match_index: usize) -> Option<(usize, usize)> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let query: Vec<char> = self.search_query.to_lowercase().chars().collect();
        let qlen = query.len();
        if qlen == 0 {
            return None;
        }

        let mut counter = 0;
        let mut row = 0;
        for block in &buf.rendered_cache {
            let lines = sketch::view::render_block_to_lines(block, content_width, &self.theme);
            for line in &lines {
                let lower: Vec<char> = line.text_content().to_lowercase().chars().collect();
                let mut ci = 0;
                while ci + qlen <= lower.len() {
                    if lower[ci..ci + qlen] == query[..] {
                        if counter == match_index {
                            return Some((row, ci));
                        }
                        counter += 1;
                        ci += qlen;
                    } else {
                        ci += 1;
                    }
                }
                row += 1;
            }
        }
        None
    }

    /// Find the y offset of the next heading after current scroll position.
    /// If `level_filter` is Some, only match headings at that level.
    fn find_next_heading(&self, level_filter: Option<u8>) -> Option<usize> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y > current {
                if let RenderedBlock::Heading { level, .. } = block {
                    if level_filter.is_none() || level_filter == Some(*level) {
                        return Some(y);
                    }
                }
            }
            y += h;
        }
        None
    }

    /// Find the y offset of the previous heading before current scroll position.
    /// If `level_filter` is Some, only match headings at that level.
    fn find_prev_heading(&self, level_filter: Option<u8>) -> Option<usize> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;
        let mut last_match = None;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y >= current {
                break;
            }
            if let RenderedBlock::Heading { level, .. } = block {
                if level_filter.is_none() || level_filter == Some(*level) {
                    last_match = Some(y);
                }
            }
            y += h;
        }
        last_match
    }

    fn reload_current_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        if buf.editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        let path = buf.file_path().to_path_buf();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let new_buf = Buffer::new(
                    path.display().to_string(),
                    content,
                    self.max_line_width,
                    &self.theme,
                );
                self.buffers[self.active_buffer] = new_buf;
            }
            Err(e) => {
                self.command_error = format!("Error reading file: {}", e);
            }
        }
    }

    fn close_current_buffer(&mut self) {
        if self.buffers[self.active_buffer].editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(self.active_buffer);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
    }

    fn handle_buffer_list_key(&mut self, key: KeyEvent, _viewport_height: usize, _content_width: usize) {
        if self.buffer_list_filter_mode {
            match key.code {
                KeyCode::Esc => {
                    // Exit buffer list entirely
                    self.mode = AppMode::Normal;
                    return;
                }
                KeyCode::Enter => {
                    let filtered = self.filtered_buffer_indices();
                    if filtered.len() == 1 {
                        // Single result — select it directly
                        self.active_buffer = filtered[0];
                        self.mode = AppMode::Normal;
                    } else if !filtered.is_empty() {
                        // Multiple results — exit filter mode, navigate
                        self.buffer_list_filter_mode = false;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    self.buffer_list_filter_text.pop();
                    self.buffer_list_selected = 0;
                }
                KeyCode::Char(c) => {
                    self.buffer_list_filter_text.push(c);
                    self.buffer_list_selected = 0;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = (self.buffer_list_selected + 1) % count;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = if self.buffer_list_selected == 0 {
                        count - 1
                    } else {
                        self.buffer_list_selected - 1
                    };
                }
            }
            KeyCode::Enter => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.active_buffer = buf_idx;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('d') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.close_buffer_at(buf_idx);
                }
            }
            KeyCode::Char('/') => {
                self.buffer_list_filter_mode = true;
                self.buffer_list_filter_text.clear();
                self.buffer_list_selected = 0;
            }
            _ => {}
        }
    }

    fn close_buffer_at(&mut self, index: usize) {
        if self.buffers[index].editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(index);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
        let count = self.filtered_buffer_indices().len();
        if self.buffer_list_selected >= count && count > 0 {
            self.buffer_list_selected = count - 1;
        }
    }

    fn filtered_buffer_indices(&self) -> Vec<usize> {
        if self.buffer_list_filter_text.is_empty() {
            return (0..self.buffers.len()).collect();
        }
        let query = self.buffer_list_filter_text.to_lowercase();
        (0..self.buffers.len())
            .filter(|&i| {
                let path = self.buffers[i].file_path().display().to_string().to_lowercase();
                fuzzy_match(&path, &query)
            })
            .collect()
    }

    /// Get the heading level at the current scroll offset, if any.
    fn heading_level_at_offset(&self) -> Option<u8> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y == current {
                if let RenderedBlock::Heading { level, .. } = block {
                    return Some(*level);
                }
            }
            if y > current {
                break;
            }
            y += h;
        }
        None
    }

    // --- Outline (TOC) ---

    fn handle_outline_key(&mut self, key: KeyEvent, _viewport_height: usize, _content_width: usize) {
        if self.outline_filter_mode {
            match key.code {
                KeyCode::Esc => {
                    // Exit outline entirely, restore scroll
                    self.buffers[self.active_buffer].viewport.scroll_offset =
                        self.outline_saved_scroll;
                    self.mode = AppMode::Normal;
                    return;
                }
                KeyCode::Enter => {
                    let entries = self.filtered_outline_entries();
                    if entries.len() == 1 {
                        // Single result — jump to it
                        self.buffers[self.active_buffer].viewport.scroll_offset =
                            entries[0].y_offset;
                        self.mode = AppMode::Normal;
                    } else if !entries.is_empty() {
                        // Multiple results — exit filter mode, navigate
                        self.outline_filter_mode = false;
                        self.scroll_to_outline_entry();
                    }
                    return;
                }
                KeyCode::Backspace => {
                    self.outline_filter_text.pop();
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                KeyCode::Char(c) => {
                    self.outline_filter_text.push(c);
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                // Restore saved scroll position
                self.buffers[self.active_buffer].viewport.scroll_offset =
                    self.outline_saved_scroll;
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.filtered_outline_entries().len();
                if count > 0 {
                    self.outline_selected = (self.outline_selected + 1) % count;
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = self.filtered_outline_entries().len();
                if count > 0 {
                    self.outline_selected = if self.outline_selected == 0 {
                        count - 1
                    } else {
                        self.outline_selected - 1
                    };
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Enter => {
                let entries = self.filtered_outline_entries();
                if let Some(entry) = entries.get(self.outline_selected) {
                    self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                // Descend: show children of the selected heading
                let entries = self.filtered_outline_entries();
                if let Some(entry) = entries.get(self.outline_selected) {
                    self.outline_parent_level = Some(entry.level);
                    self.outline_parent_y = Some(entry.y_offset);
                    self.outline_selected = 0;
                    self.outline_filter_text.clear();
                    self.outline_filter_mode = false;
                    // If no children, don't descend
                    if self.filtered_outline_entries().is_empty() {
                        self.outline_parent_level = None;
                        self.outline_parent_y = None;
                    } else {
                        self.scroll_to_outline_entry();
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                // Ascend: go back to parent level
                if self.outline_parent_level.is_some() {
                    // Find the parent heading and restore to its level
                    let parent_y = self.outline_parent_y;
                    self.outline_parent_level = None;
                    self.outline_parent_y = None;
                    self.outline_filter_text.clear();
                    self.outline_filter_mode = false;
                    // Try to select the entry we came from
                    let entries = self.filtered_outline_entries();
                    self.outline_selected = entries
                        .iter()
                        .position(|e| Some(e.y_offset) == parent_y)
                        .unwrap_or(0);
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Char('/') => {
                self.outline_filter_mode = true;
                self.outline_filter_text.clear();
                self.outline_selected = 0;
            }
            _ => {}
        }
    }

    /// Scroll the document to the currently selected outline entry.
    fn scroll_to_outline_entry(&mut self) {
        let entries = self.filtered_outline_entries();
        if let Some(entry) = entries.get(self.outline_selected) {
            self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
        }
    }

    /// Get all headings with their rendered y offsets.
    fn outline_entries(&self) -> Vec<OutlineEntry> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let mut entries = Vec::new();
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if let RenderedBlock::Heading { level, content } = block {
                entries.push(OutlineEntry {
                    level: *level,
                    title: content.text_content(),
                    y_offset: y,
                });
            }
            y += h;
        }
        entries
    }

    /// Get outline entries filtered by current hierarchy level and search text.
    fn filtered_outline_entries(&self) -> Vec<OutlineEntry> {
        let all = self.outline_entries();

        // Apply hierarchy filter
        let scoped: Vec<OutlineEntry> = if let Some(parent_level) = self.outline_parent_level {
            let parent_y = self.outline_parent_y.unwrap_or(0);
            let child_level = parent_level + 1;
            // Show headings at child_level that come after parent_y
            // and before the next heading at parent_level or above
            all.into_iter()
                .skip_while(|e| e.y_offset <= parent_y)
                .take_while(|e| e.level > parent_level)
                .filter(|e| e.level == child_level)
                .collect()
        } else {
            // Show top-level: find the minimum heading level and show only those
            let min_level = all.iter().map(|e| e.level).min().unwrap_or(1);
            all.into_iter().filter(|e| e.level == min_level).collect()
        };

        // Apply text filter
        if self.outline_filter_text.is_empty() {
            scoped
        } else {
            let query = self.outline_filter_text.to_lowercase();
            scoped
                .into_iter()
                .filter(|e| fuzzy_match(&e.title.to_lowercase(), &query))
                .collect()
        }
    }
}

#[derive(Debug, Clone)]
struct OutlineEntry {
    level: u8,
    title: String,
    y_offset: usize,
}

fn fuzzy_match(text: &str, query: &str) -> bool {
    let mut text_chars = text.chars();
    for qc in query.chars() {
        loop {
            match text_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Merge user menu nodes on top of defaults.
/// User entries with the same key at the same level replace the default entry.
/// New entries are appended.
fn merge_menu(mut defaults: Vec<MenuNode>, user_nodes: &[MenuNode]) -> Vec<MenuNode> {
    for user_node in user_nodes {
        if user_node.key.is_empty() {
            // Separator or label — just append
            defaults.push(user_node.clone());
            continue;
        }
        if let Some(pos) = defaults.iter().position(|d| d.key == user_node.key) {
            defaults[pos] = user_node.clone();
        } else {
            defaults.push(user_node.clone());
        }
    }
    defaults
}
