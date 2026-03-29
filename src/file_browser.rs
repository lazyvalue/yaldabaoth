use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
}

pub struct FileBrowser {
    #[allow(dead_code)]
    root: PathBuf,
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    selected: usize,
    filter_text: String,
    filtered_indices: Vec<usize>,
    pub filter_mode: bool,
}

impl FileBrowser {
    pub fn new(start_dir: PathBuf) -> Self {
        let mut browser = Self {
            root: start_dir.clone(),
            current_dir: start_dir,
            entries: Vec::new(),
            selected: 0,
            filter_text: String::new(),
            filtered_indices: Vec::new(),
            filter_mode: false,
        };
        browser.refresh();
        browser
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn entries(&self) -> &[BrowserEntry] {
        &self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        let max = self.visible_entries().len().saturating_sub(1);
        self.selected = idx.min(max);
    }

    /// Get entries visible after filtering.
    pub fn visible_entries(&self) -> Vec<&BrowserEntry> {
        if self.filter_text.is_empty() {
            self.entries.iter().collect()
        } else {
            self.filtered_indices
                .iter()
                .filter_map(|&i| self.entries.get(i))
                .collect()
        }
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        let visible = self.visible_entries();
        visible.get(self.selected).copied()
    }

    pub fn move_down(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
    }

    pub fn move_up(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
    }

    /// Enter the selected entry. Returns Some(path) if a file was selected (to open),
    /// or None if a directory was entered.
    pub fn enter_selected(&mut self) -> Option<PathBuf> {
        let entry = self.selected_entry()?.clone();
        if entry.is_dir {
            self.current_dir = entry.path;
            self.selected = 0;
            self.clear_filter();
            self.refresh();
            None
        } else {
            Some(entry.path)
        }
    }

    /// Navigate to parent directory. No-op at filesystem root.
    pub fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.selected = 0;
            self.clear_filter();
            self.refresh();
        }
    }

    pub fn set_filter(&mut self, text: &str) {
        self.filter_text = text.to_string();
        self.update_filtered();
        // Reset selection to 0 when filter changes
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        self.filtered_indices.clear();
        self.filter_mode = false;
        self.selected = 0;
    }

    pub fn filter_text(&self) -> &str {
        &self.filter_text
    }

    fn refresh(&mut self) {
        self.entries = Self::list_directory(&self.current_dir);
        self.update_filtered();
    }

    fn list_directory(dir: &Path) -> Vec<BrowserEntry> {
        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            let path = entry.path();

            // Follow symlinks — check the resolved metadata
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue, // broken symlink — skip
            };

            let is_dir = metadata.is_dir();
            let browser_entry = BrowserEntry { name, is_dir, path };

            if is_dir {
                dirs.push(browser_entry);
            } else {
                files.push(browser_entry);
            }
        }

        // Sort each group alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Directories first, then files
        dirs.extend(files);
        dirs
    }

    fn update_filtered(&mut self) {
        if self.filter_text.is_empty() {
            self.filtered_indices.clear();
            return;
        }
        let query = self.filter_text.to_lowercase();
        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
    }
}
