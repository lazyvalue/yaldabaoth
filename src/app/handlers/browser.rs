use sketch::file_browser::FileBrowser;
use sketch::keys::{Key, KeyPress};

use crate::app::{App, AppMode, AppScreen};

impl App {
    pub(crate) fn handle_file_browser_key(
        &mut self,
        key: KeyPress,
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
            match key.key {
                Key::Esc => {
                    // Exit file browser entirely
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                    return;
                }
                Key::Enter => {
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
                Key::Backspace => {
                    let mut text = browser.filter_text().to_string();
                    text.pop();
                    browser.set_filter(&text);
                }
                Key::Char(c) => {
                    let mut text = browser.filter_text().to_string();
                    text.push(c);
                    browser.set_filter(&text);
                }
                _ => {}
            }
            return;
        }

        match key.key {
            Key::Char('j') => browser.move_down(),
            Key::Char('k') => browser.move_up(),
            Key::Char('l') | Key::Char(' ') => {
                if let Some(path) = browser.enter_selected()
                    && self.open_buffer(path)
                {
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                }
            }
            Key::Char('h') | Key::Backspace => browser.go_parent(),
            Key::Char('.') => browser.toggle_hidden(),
            Key::Char('/') => browser.filter_mode = true,
            Key::Char('q') | Key::Esc => {
                self.file_browser = None;
                self.mode = AppMode::Normal;
            }
            Key::Tab => {
                self.open_file_browser_full(true);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_full_browser_key(
        &mut self,
        key: KeyPress,
        _term_width: u16,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        let browser = match &mut self.file_browser {
            Some(b) => b,
            None => {
                self.screen = AppScreen::Editor;
                self.mode = AppMode::Normal;
                return;
            }
        };

        if browser.filter_mode {
            match key.key {
                Key::Esc => {
                    self.close_full_browser();
                    return;
                }
                Key::Enter => {
                    let count = browser.visible_entries().len();
                    if count == 1 {
                        if let Some(path) = browser.enter_selected()
                            && self.open_buffer(path)
                        {
                            self.screen = AppScreen::Editor;
                            self.mode = AppMode::Normal;
                        }
                    } else if count > 0 {
                        browser.filter_mode = false;
                    }
                    return;
                }
                Key::Backspace => {
                    let mut text = browser.filter_text().to_string();
                    text.pop();
                    browser.set_filter(&text);
                }
                Key::Char(c) => {
                    let mut text = browser.filter_text().to_string();
                    text.push(c);
                    browser.set_filter(&text);
                }
                _ => {}
            }
            return;
        }

        // Normal mode
        match key.key {
            Key::Char('j') | Key::Down => browser.move_down(),
            Key::Char('k') | Key::Up => browser.move_up(),
            Key::Char('l') | Key::Enter => {
                if let Some(path) = browser.enter_selected()
                    && self.open_buffer(path)
                {
                    self.screen = AppScreen::Editor;
                    self.mode = AppMode::Normal;
                }
            }
            Key::Char('o') => {
                // Open file but stay in browser
                if let Some(path) = browser.enter_selected() {
                    let _ = self.open_buffer(path);
                    // Stay in full browser screen
                }
            }
            Key::Char('h') | Key::Char('-') | Key::Backspace => browser.go_parent(),
            Key::Char('.') => browser.toggle_hidden(),
            Key::Char('s') => browser.cycle_sort(),
            Key::Char('/') => browser.filter_mode = true,
            Key::Char('G') => {
                let len = browser.visible_entries().len();
                if len > 0 {
                    browser.set_selected(len - 1);
                }
            }
            Key::Char('g') => {
                if self.full_browser_pending_g {
                    // gg — jump to first entry
                    browser.set_selected(0);
                    self.full_browser_pending_g = false;
                } else {
                    self.full_browser_pending_g = true; // wait for next key
                }
            }
            Key::Tab => {
                if let AppScreen::FileBrowser {
                    came_from_dropdown: true,
                } = self.screen
                {
                    self.close_full_browser();
                }
            }
            Key::Char('q') | Key::Esc => {
                self.close_full_browser();
            }
            _ => {
                self.full_browser_pending_g = false;
            }
        }
    }

    pub(crate) fn close_full_browser(&mut self) {
        match self.screen {
            AppScreen::FileBrowser {
                came_from_dropdown: true,
            } => {
                self.screen = AppScreen::Editor;
                // file_browser stays Some, mode stays FileBrowser for dropdown
                self.mode = AppMode::FileBrowser;
            }
            AppScreen::FileBrowser {
                came_from_dropdown: false,
            } => {
                self.screen = AppScreen::Editor;
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    pub(crate) fn open_file_browser(&mut self) {
        let dir = std::env::current_dir().unwrap_or_default();
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }

    pub(crate) fn open_file_browser_full(&mut self, came_from_dropdown: bool) {
        if self.file_browser.is_none() {
            let dir = std::env::current_dir().unwrap_or_default();
            self.file_browser = Some(FileBrowser::new(dir));
        }
        self.screen = AppScreen::FileBrowser { came_from_dropdown };
    }
}
