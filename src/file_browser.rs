use std::fs;
use std::path::{Path, PathBuf};

const MAX_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_DEPTH: usize = 8;

#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

pub struct FileBrowser {
    #[allow(dead_code)]
    root: PathBuf,
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    selected: usize,
    filter_text: String,
    filtered_indices: Vec<usize>,
    /// Recursive search results (populated when filter is non-empty).
    search_results: Vec<BrowserEntry>,
    pub filter_mode: bool,
    pub show_hidden: bool,
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
            search_results: Vec::new(),
            filter_mode: false,
            show_hidden: false,
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
            self.search_results.iter().collect()
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
        self.update_search();
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        self.filtered_indices.clear();
        self.search_results.clear();
        self.filter_mode = false;
        self.selected = 0;
    }

    pub fn filter_text(&self) -> &str {
        &self.filter_text
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh();
        if !self.filter_text.is_empty() {
            self.update_search();
        }
        self.selected = 0;
    }

    fn refresh(&mut self) {
        self.entries = Self::list_directory(&self.current_dir, self.show_hidden);
    }

    fn list_directory(dir: &Path, show_hidden: bool) -> Vec<BrowserEntry> {
        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless toggled on
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = entry.path();

            // Follow symlinks — check the resolved metadata
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue, // broken symlink — skip
            };

            let is_dir = metadata.is_dir();
            let size = if metadata.is_file() { Some(metadata.len()) } else { None };
            let modified = metadata.modified().ok();
            let browser_entry = BrowserEntry { name, is_dir, path, size, modified };

            if is_dir {
                dirs.push(browser_entry);
            } else {
                files.push(browser_entry);
            }
        }

        // Sort each group alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Parent entry first, then directories, then files
        let mut result = Vec::new();
        if let Some(parent) = dir.parent() {
            result.push(BrowserEntry {
                name: "..".to_string(),
                is_dir: true,
                path: parent.to_path_buf(),
                size: None,
                modified: None,
            });
        }
        result.extend(dirs);
        result.extend(files);
        result
    }

    /// Recursively search for files matching the query.
    fn update_search(&mut self) {
        self.search_results.clear();
        if self.filter_text.is_empty() {
            return;
        }
        let query = self.filter_text.to_lowercase();
        Self::search_recursive(
            &self.current_dir,
            &self.current_dir,
            &query,
            &mut self.search_results,
            0,
            self.show_hidden,
        );
        // Sort by match quality: exact filename > starts-with > shorter path > alphabetical
        let q = query.clone();
        self.search_results.sort_by(|a, b| {
            fn score(name: &str, query: &str) -> u8 {
                let lower = name.to_lowercase();
                // Extract just the filename from the relative path
                let filename = name.rsplit('/').next().unwrap_or(name).to_lowercase();
                if filename == query {
                    0 // exact filename match
                } else if filename.starts_with(query) {
                    1 // filename starts with query
                } else if lower == query {
                    2 // exact path match
                } else if filename.contains(query) {
                    3 // filename contains query
                } else {
                    4 // path contains query
                }
            }
            let sa = score(&a.name, &q);
            let sb = score(&b.name, &q);
            sa.cmp(&sb)
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    fn search_recursive(
        base: &Path,
        dir: &Path,
        query: &str,
        results: &mut Vec<BrowserEntry>,
        depth: usize,
        show_hidden: bool,
    ) {
        if depth > MAX_SEARCH_DEPTH || results.len() >= MAX_SEARCH_RESULTS {
            return;
        }

        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            if results.len() >= MAX_SEARCH_RESULTS {
                return;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let is_dir = metadata.is_dir();

            // Show relative path from the current directory
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .display()
                .to_string();

            if relative.to_lowercase().contains(query) {
                let size = if metadata.is_file() { Some(metadata.len()) } else { None };
                let modified = metadata.modified().ok();
                results.push(BrowserEntry {
                    name: relative,
                    is_dir,
                    path: path.clone(),
                    size,
                    modified,
                });
            }

            if is_dir {
                Self::search_recursive(base, &path, query, results, depth + 1, show_hidden);
            }
        }
    }
}
