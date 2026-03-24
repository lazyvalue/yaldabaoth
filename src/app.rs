use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent};
use ratatui::DefaultTerminal;

use sketch::blocks::RenderedBlock;
use sketch::config::Config;
use sketch::file_browser::FileBrowser;
use sketch::keybind::{Action, KeybindManager};
use sketch::menu::{self, MenuNode, MenuState};
use sketch::render;
use sketch::theme::Theme;
use sketch::view::{self, ViewState};
use sketch::viewport::Viewport;

#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Menu,
    FileBrowser,
}

pub struct App {
    filename: String,
    blocks: Vec<RenderedBlock>,
    viewport: Viewport,
    theme: Theme,
    keybinds: KeybindManager,
    should_quit: bool,
    search_query: String,
    search_input_mode: bool,
    search_input_buffer: String,
    search_matches: Vec<(usize, usize)>, // (block_index, span_index)
    search_match_index: usize,
    mode: AppMode,
    menu_state: MenuState,
    menu_tree: Vec<MenuNode>,
    file_browser: Option<FileBrowser>,
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::dark();
        let blocks = render::render(&markdown, &theme);
        let viewport = Viewport::new(config.max_line_width);
        let keybinds = KeybindManager::default();

        Self {
            filename,
            blocks,
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
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // Calculate initial dimensions
        let size = terminal.size()?;
        let content_width = self.effective_content_width(size.width as usize);
        self.viewport
            .calculate_total_lines(&self.blocks, content_width);

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

                let state = ViewState {
                    filename: &self.filename,
                    blocks: &self.blocks,
                    viewport: &self.viewport,
                    theme: &self.theme,
                    mode_label: match self.mode {
                        AppMode::Normal => "NORMAL",
                        AppMode::Menu => "NORMAL",
                        AppMode::FileBrowser => "NORMAL",
                    },
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
                };
                view::draw(frame, &state);
            })?;

            if self.should_quit {
                break;
            }

            // Poll for events with a timeout (for multi-key sequence timeout)
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
                        self.viewport.calculate_total_lines(&self.blocks, cw);
                    }
                    _ => {}
                }
            } else if self.keybinds.has_pending() {
                // Timeout with pending keys — reset
                self.keybinds.reset_pending();
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2);
        let content_width = self.effective_content_width(size.width as usize);

        match self.mode {
            AppMode::Normal => self.handle_normal_key(key, viewport_height, content_width),
            AppMode::Menu => self.handle_menu_key(key, viewport_height, content_width),
            AppMode::FileBrowser => {
                self.handle_file_browser_key(key, size.width, viewport_height, content_width)
            }
        }

        Ok(())
    }

    fn handle_normal_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        if self.search_input_mode {
            match key.code {
                KeyCode::Enter => {
                    self.search_query = self.search_input_buffer.clone();
                    self.search_input_mode = false;
                    self.perform_search();
                    self.jump_to_match(content_width, viewport_height);
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
            _ => {} // ignore unrecognized keys
        }
    }

    fn handle_file_browser_key(
        &mut self,
        key: KeyEvent,
        term_width: u16,
        _viewport_height: usize,
        content_width: usize,
    ) {
        let _ = term_width; // reserved for future use

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

    fn execute_action(&mut self, action: Action, viewport_height: usize, content_width: usize) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::ScrollDown => self.viewport.scroll_down(1, viewport_height),
            Action::ScrollUp => self.viewport.scroll_up(1),
            Action::HalfPageDown => self
                .viewport
                .scroll_down(viewport_height / 2, viewport_height),
            Action::HalfPageUp => self.viewport.scroll_up(viewport_height / 2),
            Action::FullPageDown => self.viewport.scroll_down(viewport_height, viewport_height),
            Action::FullPageUp => self.viewport.scroll_up(viewport_height),
            Action::JumpTop => self.viewport.jump_top(),
            Action::JumpBottom => self.viewport.jump_bottom(viewport_height),
            Action::NextHeading => {
                self.jump_to_next_heading(content_width, viewport_height);
            }
            Action::PrevHeading => {
                self.jump_to_prev_heading(content_width, viewport_height);
            }
            Action::NextHeadingSameLevel => {
                self.jump_to_heading_same_level(content_width, viewport_height, true);
            }
            Action::PrevHeadingSameLevel => {
                self.jump_to_heading_same_level(content_width, viewport_height, false);
            }
            Action::SearchForward | Action::SearchBackward => {
                self.search_input_mode = true;
                self.search_input_buffer.clear();
            }
            Action::SearchNext => {
                if !self.search_matches.is_empty() {
                    self.search_match_index =
                        (self.search_match_index + 1) % self.search_matches.len();
                    self.jump_to_match(content_width, viewport_height);
                }
            }
            Action::SearchPrev => {
                if !self.search_matches.is_empty() {
                    self.search_match_index = if self.search_match_index == 0 {
                        self.search_matches.len() - 1
                    } else {
                        self.search_match_index - 1
                    };
                    self.jump_to_match(content_width, viewport_height);
                }
            }
            Action::OpenLink => {
                if let Some(url) = self.find_link_at_cursor(content_width) {
                    let _ = std::process::Command::new("open") // macOS
                        .arg(&url)
                        .spawn();
                }
            }
            Action::YankLine => {
                if let Some(text) = self.get_cursor_line_text(content_width) {
                    use std::io::Write;
                    use std::process::{Command, Stdio};
                    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
                        && let Some(mut stdin) = child.stdin.take()
                    {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                }
            }
            Action::OpenMenu => {
                self.menu_state.open();
                self.mode = AppMode::Menu;
            }
            Action::OpenFileBrowser => {
                self.open_file_browser();
            }
            Action::MoveLeft
            | Action::MoveRight
            | Action::MoveUp
            | Action::MoveDown
            | Action::MoveWordForward
            | Action::MoveWordBackward
            | Action::MoveWordEnd
            | Action::MoveLineStart
            | Action::MoveLineEnd
            | Action::InsertMode
            | Action::InsertAfter
            | Action::OpenLineBelow
            | Action::OpenLineAbove
            | Action::DeleteChar
            | Action::DeleteLine
            | Action::Undo
            | Action::Redo
            | Action::EnterCommand
            | Action::None
            | Action::FileBrowserDown
            | Action::FileBrowserUp
            | Action::FileBrowserEnter
            | Action::FileBrowserParentDir
            | Action::FileBrowserFilter
            | Action::FileBrowserClose => {}
        }
    }

    fn open_file_browser(&mut self) {
        let dir = std::path::Path::new(&self.filename)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }

    /// Load a file into the viewer. Returns true on success, false on error
    /// (browser should stay open on failure).
    fn load_file(&mut self, path: std::path::PathBuf, content_width: usize) -> bool {
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    self.filename = path.display().to_string();
                    self.blocks = render::render(&content, &self.theme);
                    self.viewport.scroll_offset = 0;
                    self.viewport.cursor_line = 0;
                    self.viewport
                        .calculate_total_lines(&self.blocks, content_width);
                    true
                }
                Err(_) => false, // Non-UTF-8: keep browser open
            },
            Err(_) => false, // Read error: keep browser open
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

    fn jump_to_next_heading(&mut self, width: usize, viewport_height: usize) {
        let mut y = 0;

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if y > self.viewport.scroll_offset && matches!(block, RenderedBlock::Heading { .. }) {
                self.viewport.scroll_offset = y.saturating_sub(viewport_height / 3);
                return;
            }
            y += h;
        }
    }

    fn jump_to_prev_heading(&mut self, width: usize, viewport_height: usize) {
        let mut positions = Vec::new();
        let mut y = 0;

        for block in &self.blocks {
            if matches!(block, RenderedBlock::Heading { .. }) {
                positions.push(y);
            }
            y += self.viewport.block_height(block, width);
        }

        // Find the last heading position before current scroll
        if let Some(&pos) = positions
            .iter()
            .rev()
            .find(|&&p| p < self.viewport.scroll_offset)
        {
            self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
        }
    }

    fn perform_search(&mut self) {
        self.search_matches.clear();
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            return;
        }

        let mut matches = Vec::new();
        for (bi, block) in self.blocks.iter().enumerate() {
            Self::search_block_collect(&query, block, bi, &mut matches);
        }
        self.search_matches = matches;
        self.search_match_index = 0;
    }

    fn search_block_collect(
        query: &str,
        block: &RenderedBlock,
        block_index: usize,
        matches: &mut Vec<(usize, usize)>,
    ) {
        match block {
            RenderedBlock::Heading { content, .. } => {
                if content.text_content().to_lowercase().contains(query) {
                    matches.push((block_index, 0));
                }
            }
            RenderedBlock::Paragraph { lines } | RenderedBlock::CodeBlock { lines, .. } => {
                for (li, line) in lines.iter().enumerate() {
                    if line.text_content().to_lowercase().contains(query) {
                        matches.push((block_index, li));
                    }
                }
            }
            RenderedBlock::BlockQuote { blocks } => {
                for b in blocks {
                    Self::search_block_collect(query, b, block_index, matches);
                }
            }
            RenderedBlock::List { items, .. } => {
                for item in items {
                    for b in &item.content {
                        Self::search_block_collect(query, b, block_index, matches);
                    }
                }
            }
            _ => {}
        }
    }

    fn jump_to_match(&mut self, width: usize, viewport_height: usize) {
        if let Some(&(block_idx, _)) = self.search_matches.get(self.search_match_index) {
            let mut y: usize = 0;
            for (i, block) in self.blocks.iter().enumerate() {
                if i == block_idx {
                    self.viewport.scroll_offset = y.saturating_sub(viewport_height / 3);
                    return;
                }
                y += self.viewport.block_height(block, width);
            }
        }
    }

    fn find_link_at_cursor(&self, width: usize) -> Option<String> {
        let mut y = 0;
        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if y + h > self.viewport.cursor_line {
                return self.find_link_in_block(block);
            }
            y += h;
        }
        None
    }

    fn find_link_in_block(&self, block: &RenderedBlock) -> Option<String> {
        match block {
            RenderedBlock::Heading { content, .. } => {
                content.spans.iter().find_map(|s| s.link.clone())
            }
            RenderedBlock::Paragraph { lines } => lines
                .iter()
                .flat_map(|l| &l.spans)
                .find_map(|s| s.link.clone()),
            RenderedBlock::BlockQuote { blocks } => {
                blocks.iter().find_map(|b| self.find_link_in_block(b))
            }
            RenderedBlock::List { items, .. } => items
                .iter()
                .flat_map(|i| &i.content)
                .find_map(|b| self.find_link_in_block(b)),
            _ => None,
        }
    }

    fn get_cursor_line_text(&self, width: usize) -> Option<String> {
        let mut y = 0;
        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if y + h > self.viewport.cursor_line {
                let line_in_block = self.viewport.cursor_line - y;
                return self.get_block_line_text(block, line_in_block);
            }
            y += h;
        }
        None
    }

    fn get_block_line_text(&self, block: &RenderedBlock, line: usize) -> Option<String> {
        match block {
            RenderedBlock::Heading { content, .. } if line == 0 => Some(content.text_content()),
            RenderedBlock::Paragraph { lines } if line < lines.len() => {
                Some(lines[line].text_content())
            }
            RenderedBlock::CodeBlock { lines, .. } if line < lines.len() => {
                Some(lines[line].text_content())
            }
            _ => None,
        }
    }

    fn jump_to_heading_same_level(&mut self, width: usize, viewport_height: usize, forward: bool) {
        // Find the current heading level (most recently passed heading)
        let mut current_level = None;
        let mut y = 0;
        let mut headings: Vec<(usize, u8)> = Vec::new(); // (y_offset, level)

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if let RenderedBlock::Heading { level, .. } = block {
                if y <= self.viewport.scroll_offset {
                    current_level = Some(*level);
                }
                headings.push((y, *level));
            }
            y += h;
        }

        let target_level = match current_level {
            Some(l) => l,
            None => return,
        };

        if forward {
            if let Some(&(pos, _)) = headings
                .iter()
                .find(|(y, l)| *y > self.viewport.scroll_offset && *l == target_level)
            {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        } else {
            if let Some(&(pos, _)) = headings
                .iter()
                .rev()
                .find(|(y, l)| *y < self.viewport.scroll_offset && *l == target_level)
            {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        }
    }
}
