use sketch::buffer::Buffer;
use sketch::keys::{Key, KeyPress};

use super::{App, AppScreen, fuzzy_match};

impl App {
    pub(crate) fn open_buffer(&mut self, path: std::path::PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        // Check if already open
        for (i, buf) in self.buffers.iter().enumerate() {
            let buf_path = buf
                .file_path()
                .canonicalize()
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

    /// Open `path` as a buffer, creating a new empty buffer if the file doesn't exist yet.
    pub(crate) fn edit_path(&mut self, path_str: &str) {
        let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(rest)
            } else {
                std::path::PathBuf::from(path_str)
            }
        } else {
            std::path::PathBuf::from(path_str)
        };

        let path = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded))
                .unwrap_or(expanded)
        };

        if path.exists() {
            if !self.open_buffer(path.clone()) {
                self.command_error = format!("Error opening: {}", path.display());
            }
            return;
        }

        // Check parent directory exists before creating an unsaved buffer.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            self.command_error = format!("No such directory: {}", parent.display());
            return;
        }

        // Already open with this target path? Switch to it.
        for (i, buf) in self.buffers.iter().enumerate() {
            if buf.file_path() == path {
                self.active_buffer = i;
                return;
            }
        }

        let buffer = Buffer::new(
            path.display().to_string(),
            String::new(),
            self.max_line_width,
            &self.theme,
        );
        self.buffers.push(buffer);
        self.active_buffer = self.buffers.len() - 1;
    }

    pub(crate) fn reload_current_buffer(&mut self) {
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

    pub(crate) fn close_current_buffer(&mut self) {
        if self.buffers[self.active_buffer]
            .editor
            .document()
            .is_modified()
        {
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

    pub(crate) fn handle_full_buffer_list_key(
        &mut self,
        key: KeyPress,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        if self.buffer_list_filter_mode {
            match key.key {
                Key::Esc => {
                    self.close_buffer_list();
                    return;
                }
                Key::Enter => {
                    let filtered = self.filtered_buffer_indices();
                    if filtered.len() == 1 {
                        self.active_buffer = filtered[0];
                        self.close_buffer_list();
                    } else if !filtered.is_empty() {
                        self.buffer_list_filter_mode = false;
                    }
                    return;
                }
                Key::Backspace => {
                    self.buffer_list_filter_text.pop();
                    self.buffer_list_selected = 0;
                }
                Key::Char(c) => {
                    self.buffer_list_filter_text.push(c);
                    self.buffer_list_selected = 0;
                }
                _ => {}
            }
            return;
        }

        match key.key {
            Key::Esc | Key::Char('q') => {
                self.close_buffer_list();
            }
            Key::Char('j') | Key::Down => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = (self.buffer_list_selected + 1) % count;
                }
            }
            Key::Char('k') | Key::Up => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = if self.buffer_list_selected == 0 {
                        count - 1
                    } else {
                        self.buffer_list_selected - 1
                    };
                }
            }
            Key::Char('g') => {
                self.buffer_list_selected = 0;
            }
            Key::Char('G') => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = count - 1;
                }
            }
            Key::Enter | Key::Char('l') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.active_buffer = buf_idx;
                    self.close_buffer_list();
                }
            }
            Key::Char('d') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.close_buffer_at(buf_idx);
                }
            }
            Key::Char('/') => {
                self.buffer_list_filter_mode = true;
                self.buffer_list_filter_text.clear();
                self.buffer_list_selected = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn close_buffer_list(&mut self) {
        self.screen = AppScreen::Editor;
        self.buffer_list_filter_mode = false;
        self.buffer_list_filter_text.clear();
    }

    pub(crate) fn close_buffer_at(&mut self, index: usize) {
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

    pub(crate) fn filtered_buffer_indices(&self) -> Vec<usize> {
        if self.buffer_list_filter_text.is_empty() {
            return (0..self.buffers.len()).collect();
        }
        let query = self.buffer_list_filter_text.to_lowercase();
        (0..self.buffers.len())
            .filter(|&i| {
                let path = self.buffers[i]
                    .file_path()
                    .display()
                    .to_string()
                    .to_lowercase();
                fuzzy_match(&path, &query)
            })
            .collect()
    }
}
