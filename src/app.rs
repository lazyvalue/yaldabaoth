use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent};
use ratatui::DefaultTerminal;

use sketch::config::Config;
use sketch::editor::Editor;
use sketch::file_browser::FileBrowser;
use sketch::keybind::{Action, KeybindManager};
use sketch::menu::{self, MenuNode, MenuState};
use sketch::render;
use sketch::theme::Theme;
use sketch::view::{self, ViewBlock, ViewState};
use sketch::viewport::Viewport;

#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
}

pub struct App {
    editor: Editor,
    viewport: Viewport,
    theme: Theme,
    keybinds: KeybindManager,
    should_quit: bool,
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
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::dark();
        let editor = Editor::new(markdown, std::path::PathBuf::from(&filename));
        let viewport = Viewport::new(config.max_line_width);
        let keybinds = KeybindManager::default();

        Self {
            editor,
            viewport,
            theme,
            keybinds,
            should_quit: false,
            search_query: String::new(),
            search_input_mode: false,
            search_input_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            mode: AppMode::Normal,
            menu_state: MenuState::new(),
            menu_tree: menu::default_menu(),
            file_browser: None,
            command_buffer: String::new(),
            command_error: String::new(),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let content_width = self.effective_content_width(size.width as usize);
        self.update_total_lines(content_width);

        loop {
            terminal.draw(|frame| {
                let menu_nodes: Vec<(char, String, bool)> = if self.menu_state.is_active() {
                    self.menu_state
                        .current_nodes(&self.menu_tree)
                        .iter()
                        .map(|n| {
                            let is_sub = matches!(n.action, menu::MenuAction::Submenu(_));
                            (n.key, n.label.clone(), is_sub)
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let (
                    fb_open,
                    fb_dir,
                    fb_entries,
                    fb_filter_mode,
                    fb_filter_text,
                    fb_panel_width,
                    fb_hint,
                ) = if let Some(browser) = &self.file_browser {
                    let entries: Vec<(String, bool, bool)> = browser
                        .visible_entries()
                        .iter()
                        .enumerate()
                        .map(|(i, e)| (e.name.clone(), e.is_dir, i == browser.selected()))
                        .collect();
                    let hint = if browser.filter_mode {
                        format!("{} matches · Space open · Esc clear", entries.len())
                    } else {
                        "j/k nav · Space open · / filter · Esc close".to_string()
                    };
                    (
                        true,
                        browser.current_dir().display().to_string(),
                        entries,
                        browser.filter_mode,
                        browser.filter_text().to_string(),
                        browser.panel_width(frame.area().width),
                        hint,
                    )
                } else {
                    (
                        false,
                        String::new(),
                        Vec::new(),
                        false,
                        String::new(),
                        0,
                        String::new(),
                    )
                };

                let view_blocks = self.build_view_blocks();
                let filename_display = self.editor.document().file_path.display().to_string();

                let state = ViewState {
                    filename: &filename_display,
                    modified: self.editor.document().is_modified(),
                    blocks: &view_blocks,
                    viewport: &self.viewport,
                    theme: &self.theme,
                    mode_label: match self.mode {
                        AppMode::Normal => "NORMAL",
                        AppMode::Insert => "INSERT",
                        AppMode::Command => "NORMAL",
                        AppMode::Menu => "NORMAL",
                        AppMode::FileBrowser => "NORMAL",
                    },
                    cursor_line: self.editor.cursor().line,
                    cursor_col: self.editor.cursor().col,
                    show_block_cursor: self.mode != AppMode::Insert,
                    search_query: &self.search_query,
                    search_input_mode: self.search_input_mode,
                    search_input_buffer: &self.search_input_buffer,
                    search_match_count: self.search_matches.len(),
                    menu_active: self.menu_state.is_active(),
                    menu_nodes,
                    menu_label: self.menu_state.current_label(&self.menu_tree),
                    file_browser_open: fb_open,
                    file_browser_dir: fb_dir,
                    file_browser_entries: fb_entries,
                    file_browser_filter_mode: fb_filter_mode,
                    file_browser_filter_text: fb_filter_text,
                    file_browser_panel_width: fb_panel_width,
                    file_browser_hint: fb_hint,
                    command_mode: self.mode == AppMode::Command,
                    command_buffer: &self.command_buffer,
                    command_error: &self.command_error,
                };
                view::draw(frame, &state);
            })?;

            if self.should_quit {
                break;
            }

            let timeout = if self.keybinds.has_pending() {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key_event) => self.handle_key(key_event, terminal)?,
                    Event::Resize(w, _h) => {
                        let cw = self.effective_content_width(w as usize);
                        self.update_total_lines(cw);
                    }
                    _ => {}
                }
            } else if self.keybinds.has_pending() {
                self.keybinds.reset_pending();
            }
        }

        Ok(())
    }

    fn update_total_lines(&mut self, content_width: usize) {
        let view_blocks = self.build_view_blocks();
        self.viewport.total_lines = view_blocks
            .iter()
            .map(|b| view_block_height(b, &self.viewport, content_width))
            .sum();
    }

    fn build_view_blocks(&self) -> Vec<ViewBlock> {
        let boundaries = self.editor.block_boundaries();
        let active_idx = self.editor.active_block_index();

        if boundaries.is_empty() {
            let text = self.editor.document().full_text();
            let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
            return vec![ViewBlock::Raw {
                lines,
                start_line: 0,
            }];
        }

        let mut view_blocks = Vec::new();
        for (i, block_info) in boundaries.iter().enumerate() {
            if Some(i) == active_idx {
                let block_text = self.editor.block_text(i);
                let lines: Vec<String> = block_text.lines().map(|l| l.to_string()).collect();
                view_blocks.push(ViewBlock::Raw {
                    lines,
                    start_line: block_info.start_line,
                });
            } else {
                let block_text = self.editor.block_text(i);
                let rendered = render::render(&block_text, &self.theme);
                if let Some(rb) = rendered.into_iter().next() {
                    view_blocks.push(ViewBlock::Rendered(rb));
                }
            }
        }

        view_blocks
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
        if !self.command_error.is_empty() && self.mode != AppMode::Command {
            self.command_error.clear();
        }

        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2);
        let content_width = self.effective_content_width(size.width as usize);

        match self.mode {
            AppMode::Normal => self.handle_normal_key(key, viewport_height, content_width),
            AppMode::Insert => self.handle_insert_key(key, viewport_height),
            AppMode::Command => self.handle_command_key(key),
            AppMode::Menu => self.handle_menu_key(key, viewport_height, content_width),
            AppMode::FileBrowser => {
                self.handle_file_browser_key(key, size.width, viewport_height, content_width)
            }
        }

        self.update_total_lines(content_width);
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

        if let Some(action) = self.keybinds.process_key(key) {
            self.execute_action(action, viewport_height, content_width);
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent, viewport_height: usize) {
        match key.code {
            KeyCode::Esc => {
                self.editor.end_insert();
                self.mode = AppMode::Normal;
                if self.editor.cursor().col > 0 {
                    self.editor.cursor_mut().move_left();
                }
            }
            KeyCode::Enter => {
                self.editor.insert_char('\n');
            }
            KeyCode::Backspace => {
                self.editor.backspace();
            }
            KeyCode::Char(c) => {
                self.editor.insert_char(c);
            }
            KeyCode::Left => {
                self.editor.cursor_mut().move_left();
            }
            KeyCode::Right => {
                self.editor.move_right_clamped(true);
            }
            KeyCode::Up => {
                self.editor.cursor_mut().move_up();
                self.editor.clamp_cursor_col(true);
            }
            KeyCode::Down => {
                self.editor.move_down(true);
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
            KeyCode::Char(c) => {
                if let Some(action) = self.menu_state.process_key(c, &self.menu_tree) {
                    self.mode = AppMode::Normal;
                    self.execute_action(action, viewport_height, content_width);
                }
            }
            _ => {}
        }
    }

    fn handle_file_browser_key(
        &mut self,
        key: KeyEvent,
        term_width: u16,
        _viewport_height: usize,
        content_width: usize,
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
                KeyCode::Esc => browser.clear_filter(),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(path) = browser.enter_selected()
                        && self.load_file(path, content_width)
                    {
                        self.file_browser = None;
                        self.mode = AppMode::Normal;
                    }
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
            KeyCode::Char(' ') => {
                if let Some(path) = browser.enter_selected()
                    && self.load_file(path, content_width)
                {
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Backspace => browser.go_parent(),
            KeyCode::Char('/') => browser.filter_mode = true,
            KeyCode::Char('q') | KeyCode::Esc => {
                self.file_browser = None;
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn execute_action(&mut self, action: Action, viewport_height: usize, _content_width: usize) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::MoveDown => {
                self.editor.move_down(false);
                self.ensure_cursor_visible(viewport_height);
            }
            Action::MoveUp => {
                self.editor.cursor_mut().move_up();
                self.editor.clamp_cursor_col(false);
                self.ensure_cursor_visible(viewport_height);
            }
            Action::MoveLeft => {
                self.editor.cursor_mut().move_left();
            }
            Action::MoveRight => {
                self.editor.move_right_clamped(false);
            }
            Action::MoveWordForward => {
                self.editor.move_cursor_word_forward();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::MoveWordBackward => {
                self.editor.move_cursor_word_backward();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::MoveWordEnd => {
                self.editor.move_cursor_word_end();
            }
            Action::MoveLineStart => {
                self.editor.cursor_mut().move_line_start();
            }
            Action::MoveLineEnd => {
                self.editor.move_cursor_line_end(false);
            }
            Action::InsertMode => {
                self.editor.begin_insert();
                self.mode = AppMode::Insert;
            }
            Action::InsertAfter => {
                self.editor.move_right_clamped(true);
                self.editor.begin_insert();
                self.mode = AppMode::Insert;
            }
            Action::OpenLineBelow => {
                self.editor.open_line_below();
                self.mode = AppMode::Insert;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::OpenLineAbove => {
                self.editor.open_line_above();
                self.mode = AppMode::Insert;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::DeleteChar => {
                self.editor.delete_char_at_cursor();
            }
            Action::DeleteLine => {
                self.editor.delete_current_line();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Undo => {
                self.editor.undo();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Redo => {
                self.editor.redo();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::EnterCommand => {
                self.command_buffer.clear();
                self.command_error.clear();
                self.mode = AppMode::Command;
            }
            Action::ScrollDown => {
                self.viewport.scroll_down(1, viewport_height);
            }
            Action::ScrollUp => {
                self.viewport.scroll_up(1);
            }
            Action::HalfPageDown => {
                self.viewport
                    .scroll_down(viewport_height / 2, viewport_height);
                let new_top = self.viewport.scroll_offset;
                if self.editor.cursor().line < new_top {
                    self.editor.cursor_mut().line = new_top;
                    self.editor.clamp_cursor_col(false);
                }
            }
            Action::HalfPageUp => {
                self.viewport.scroll_up(viewport_height / 2);
                let new_bottom = self.viewport.scroll_offset + viewport_height;
                if self.editor.cursor().line >= new_bottom {
                    self.editor.cursor_mut().line = new_bottom.saturating_sub(1);
                    self.editor.clamp_cursor_col(false);
                }
            }
            Action::FullPageDown => {
                self.viewport.scroll_down(viewport_height, viewport_height);
                let new_top = self.viewport.scroll_offset;
                if self.editor.cursor().line < new_top {
                    self.editor.cursor_mut().line = new_top;
                    self.editor.clamp_cursor_col(false);
                }
            }
            Action::FullPageUp => {
                self.viewport.scroll_up(viewport_height);
                let new_bottom = self.viewport.scroll_offset + viewport_height;
                if self.editor.cursor().line >= new_bottom {
                    self.editor.cursor_mut().line = new_bottom.saturating_sub(1);
                    self.editor.clamp_cursor_col(false);
                }
            }
            Action::JumpTop => {
                self.editor.cursor_mut().jump_top();
                self.viewport.jump_top();
            }
            Action::JumpBottom => {
                self.editor.jump_cursor_bottom();
                self.viewport.jump_bottom(viewport_height);
            }
            Action::NextHeading | Action::PrevHeading => {
                // TODO: implement heading navigation via tree-sitter
            }
            Action::NextHeadingSameLevel | Action::PrevHeadingSameLevel => {
                // TODO: implement via tree-sitter
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
                // TODO: implement link finding via tree-sitter/cursor position
            }
            Action::YankLine => {
                let line_text = self.editor.document().line_text(self.editor.cursor().line);
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
            Action::None
            | Action::FileBrowserDown
            | Action::FileBrowserUp
            | Action::FileBrowserEnter
            | Action::FileBrowserParentDir
            | Action::FileBrowserFilter
            | Action::FileBrowserClose => {}
        }
    }

    fn execute_command(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
        match parts[0] {
            "w" => {
                let result = if parts.len() > 1 {
                    let path = std::path::Path::new(parts[1]);
                    self.editor.save_to(path)
                } else {
                    self.editor.save()
                };
                if let Err(e) = result {
                    self.command_error = format!("Error writing file: {}", e);
                }
            }
            "q" => {
                if self.editor.document().is_modified() {
                    self.command_error =
                        "No write since last change (add ! to override)".to_string();
                } else {
                    self.should_quit = true;
                }
            }
            "q!" => {
                self.should_quit = true;
            }
            "wq" => {
                if let Err(e) = self.editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                } else {
                    self.should_quit = true;
                }
            }
            _ => {
                self.command_error = format!("Not an editor command: {}", cmd);
            }
        }
    }

    fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let cursor_line = self.editor.cursor().line;
        self.viewport
            .ensure_cursor_visible(cursor_line, viewport_height);
    }

    fn open_file_browser(&mut self) {
        if self.editor.document().is_modified() {
            self.command_error = "Save changes first (:w) before browsing files".to_string();
            return;
        }
        let dir = self
            .editor
            .document()
            .file_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }

    fn load_file(&mut self, path: std::path::PathBuf, _content_width: usize) -> bool {
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    self.editor = Editor::new(content, path);
                    self.viewport.scroll_offset = 0;
                    self.viewport.cursor_line = 0;
                    true
                }
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    fn effective_content_width(&self, terminal_width: usize) -> usize {
        let available = if let Some(browser) = &self.file_browser {
            terminal_width.saturating_sub(browser.panel_width(terminal_width as u16) as usize + 1)
        } else {
            terminal_width
        };
        self.viewport.content_width(available)
    }

    fn perform_search(&mut self) {
        self.search_matches.clear();
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            return;
        }

        let line_count = self.editor.document().line_count();
        for line_idx in 0..line_count {
            let line_text = self.editor.document().line_text(line_idx);
            if line_text.to_lowercase().contains(&query) {
                self.search_matches.push((line_idx, 0));
            }
        }
        self.search_match_index = 0;
    }

    fn jump_to_match(&mut self, viewport_height: usize) {
        if let Some(&(line_idx, _)) = self.search_matches.get(self.search_match_index) {
            self.editor.cursor_mut().line = line_idx;
            self.editor.cursor_mut().col = 0;
            self.ensure_cursor_visible(viewport_height);
        }
    }
}

fn view_block_height(block: &ViewBlock, viewport: &Viewport, width: usize) -> usize {
    match block {
        ViewBlock::Rendered(rb) => viewport.block_height(rb, width),
        ViewBlock::Raw { lines, .. } => lines.len() + 1,
    }
}
